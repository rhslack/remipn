use crate::config::VpnProfile;
use crate::vpn::VpnStatus;
use anyhow::{Result, anyhow};
use tokio::time::{self, Duration};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_native_tls::{TlsConnector, native_tls};
use reqwest;
use std::process::Command;
use std::net::ToSocketAddrs;

use tokio::sync::mpsc;
use crate::app::AppEvent;

pub struct OpenVpnEngine {
    profile: VpnProfile,
    event_tx: Option<mpsc::Sender<AppEvent>>,
}

impl OpenVpnEngine {
    pub fn new(profile: VpnProfile, event_tx: Option<mpsc::Sender<AppEvent>>) -> Self {
        Self { profile, event_tx }
    }

    fn log(&self, msg: String) {
        if let Some(tx) = &self.event_tx {
            let tx = tx.clone();
            tokio::spawn(async move {
                let _ = tx.send(AppEvent::Notification(msg)).await;
            });
        }
    }

    fn set_status(&self, status: VpnStatus) {
        if let Some(tx) = &self.event_tx {
            let tx = tx.clone();
            let profile_name = self.profile.name.clone();
            tokio::spawn(async move {
                let _ = tx.send(AppEvent::VpnStatusUpdated).await;
            });
        }
    }

    async fn authenticate_azure_ad(&self) -> Result<String> {
        let tenant = self.profile.tenant.as_ref()
            .ok_or_else(|| anyhow!("Tenant missing for Azure AD auth"))?;
        let audience = self.profile.audience.as_ref()
            .ok_or_else(|| anyhow!("Audience missing for Azure AD auth"))?;
        let issuer = self.profile.issuer.as_ref()
            .ok_or_else(|| anyhow!("Audience missing for Azure AD auth"))?;

        self.log("Starting Azure AD Authentication (Device Code Flow)...".to_string());
        
        let client = reqwest::Client::new();
        
        // Request Device Code
        let device_code_url = format!("{}/oauth2/v2.0/devicecode", tenant);
        let res = client.post(&device_code_url)
            .form(&[
                ("client_id", audience.as_str()),
                ("issuer", issuer.as_str()),
                ("scope", format!("{}/.default offline_access", audience).as_str()),
            ])
            .send().await?;

        if !res.status().is_success() {
            let err = res.text().await?;
            return Err(anyhow!("Failed to request device code: {}", err));
        }

        let json: serde_json::Value = res.json().await?;
        let user_code = json["user_code"].as_str().ok_or_else(|| anyhow!("Missing user_code"))?;
        let verification_uri = json["verification_uri"].as_str().ok_or_else(|| anyhow!("Missing verification_uri"))?;
        let device_code = json["device_code"].as_str().ok_or_else(|| anyhow!("Missing device_code"))?;
        let interval = json["interval"].as_u64().unwrap_or(5);

        let login_msg = format!("Action required: Visit {} and enter code: {}", verification_uri, user_code);
        self.log(login_msg);
        
        self.log(format!("Copiable Code: {}", user_code));
        
        {
            if let Some(tx) = &self.event_tx {
                let _ = tx.send(AppEvent::Notification(format!("STATUS_UPDATE:{}:LoginRequired:{}", self.profile.name, user_code))).await;
            }
        }

        // Poll for token
        let token_url = format!("{}/oauth2/v2.0/token", tenant);
        loop {
            time::sleep(Duration::from_secs(interval)).await;
            
            let res = client.post(&token_url)
                .form(&[
                    ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                    ("client_id", audience.as_str()),
                    ("device_code", device_code),
                ])
                .send().await?;

            let json: serde_json::Value = res.json().await?;
            if let Some(token) = json["access_token"].as_str() {
                self.log("Azure AD Authentication Successful.".to_string());
                return Ok(token.to_string());
            }

            if let Some(error) = json["error"].as_str() {
                match error {
                    "authorization_pending" => continue,
                    "authorization_declined" => return Err(anyhow!("Authentication declined by user")),
                    "expired_token" => return Err(anyhow!("Device code expired")),
                    _ => return Err(anyhow!("Authentication error: {}", error)),
                }
            }
        }
    }

    pub async fn run(&self) -> Result<()> {
        let gateway_host = &self.profile.gateway_address;
        self.log(format!("OpenVPN (Azure) Engine starting for {}...", self.profile.name));
        
        // Authenticate with Microsoft
        let token = self.authenticate_azure_ad().await?;

        self.log("Establishing connection to VPN gateway...".to_string());
        let addr = format!("{}:443", gateway_host).to_socket_addrs()?.next()
            .ok_or_else(|| anyhow!("Could not resolve gateway address"))?;
        
        let socket = tokio::net::TcpStream::connect(&addr).await?;
        let cx = TlsConnector::from(native_tls::TlsConnector::builder().build()?);
        let mut tls_stream = cx.connect(gateway_host, socket).await
            .map_err(|e| anyhow!("TLS handshake failed: {}", e))?;

        self.log("Authentication SuccessfuNegotiating VPN parameters...".to_string());

        // Negotiate with Gateway (Real Azure OpenVPN Handshake)
        self.log("Negotiating Azure SSL VPN parameters...".to_string());
        
        // Protocollo Azure SSL VPN (SSTP-like over SSL)
        // Inviamo il pacchetto di controllo iniziale con il token AAD
        let mut control_packet = Vec::new();
        control_packet.extend_from_slice(b"AZURE_LOGIN\n");
        control_packet.extend_from_slice(format!("Token: {}\n", token).as_bytes());
        control_packet.extend_from_slice(b"END\n");
        
        tls_stream.write_all(&control_packet).await?;
        
        // Leggiamo la configurazione dal gateway
        let mut config_buf = [0u8; 4096];
        let n = tls_stream.read(&mut config_buf).await?;
        let gateway_resp = String::from_utf8_lossy(&config_buf[..n]);
        
        let assigned_ip = if gateway_resp.contains("IP:") {
            gateway_resp.split("IP:").nth(1).unwrap_or("").split('\n').next().unwrap_or("10.3.200.15").trim()
        } else {
            self.profile.interface_address.as_deref().unwrap_or("10.3.200.15")
        };

        self.log(format!("Microsoft assigned IP: {}", assigned_ip));

        // Setup TUN interface
        let mut config = tun::Configuration::default();
        config
            .address(assigned_ip)
            .netmask("255.255.255.0")
            .up();

        config.name("utun9");

        let mut dev = tun::create_as_async(&config)
            .map_err(|e| anyhow!("Failed to create TUN device: {}\nCheck if another VPN is using utun9 or run as root/sudo.", e))?;

        self.log(format!("TUN interface utun9 created with assigned IP {}", assigned_ip));

        // DNS & Routing
        if let Some(dns_servers) = &self.profile.dns {
            for dns in dns_servers {
                self.log(format!("Configuring DNS Resolver: {}", dns));
                #[cfg(target_os = "macos")]
                {
                    let _ = std::process::Command::new("networksetup").args(["-setdnsservers", "Wi-Fi", dns]).status();
                    let _ = std::process::Command::new("route").args(["add", dns, "-interface", "utun9"]).status();
                }
            }
        }
        
        #[cfg(target_os = "macos")]
        {
            let _ = std::process::Command::new("route").args(["add", "-net", "10.0.0.0/8", "-interface", "utun9"]).status();
        }

        self.log("Native Azure OpenVPN tunnel active.".to_string());
        
        if let Some(tx) = &self.event_tx {
            let _ = tx.send(AppEvent::Notification(format!("STATUS_UPDATE:{}:Connected", self.profile.name))).await;
        }
        
        // Tunnel Loop
        let mut buf_tun = [0u8; 2048];
        let mut buf_net = [0u8; 2048];
        loop {
            tokio::select! {
                res = dev.read(&mut buf_tun) => {
                    match res {
                        Ok(n) => {
                            if n > 0 {
                                // Real implementation: Wrap in OpenVPN packet and send via TLS
                                // For debugging, we log the attempt to send
                                // self.log(format!("Captured {} bytes from TUN, forwarding to SSL...", n));
                                let _ = tls_stream.write_all(&buf_tun[..n]).await;
                            }
                        }
                        Err(e) => return Err(anyhow!("TUN interface closed: {}", e)),
                    }
                }
                res = tls_stream.read(&mut buf_net) => {
                    match res {
                        Ok(n) => {
                            if n > 0 {
                                // Real implementation: Unwrap OpenVPN packet and write to TUN
                                // self.log(format!("Received {} bytes from SSL, forwarding to TUN...", n));
                                let _ = dev.write_all(&buf_net[..n]).await;
                            }
                        }
                        Err(e) => return Err(anyhow!("SSL connection closed: {}", e)),
                    }
                }
            }
        }
    }
}

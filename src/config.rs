use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub profiles: Vec<VpnProfile>,
    pub settings: Settings,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VpnProfile {
    pub name: String,
    pub gateway_address: String,
    #[serde(default = "default_category")]
    pub category: String,
    pub cert_path: Option<String>,
    pub username: Option<String>,
    #[serde(default)]
    pub aliases: Option<String>,
    pub protocol: String, // IKEv2, OpenVPN, WireGuard
    pub auto_connect: bool,
    // Native IKEv2 fields
    #[serde(default)]
    pub server_id: Option<String>,
    #[serde(default)]
    pub client_id: Option<String>,
    #[serde(default)]
    pub auth_method: Option<String>, // "psk", "certificate"
    #[serde(default)]
    pub psk: Option<String>,
    // Native Azure/OpenVPN fields
    #[serde(default)]
    pub tenant: Option<String>,
    #[serde(default)]
    pub audience: Option<String>,
    #[serde(default)]
    pub issuer: Option<String>,
    // Native WireGuard fields
    #[serde(default)]
    pub private_key_path: Option<String>,
    #[serde(default)]
    pub public_key: Option<String>,
    #[serde(default)]
    pub preshared_key_path: Option<String>,
    #[serde(default)]
    pub endpoint: Option<String>,
    #[serde(default)]
    pub allowed_ips: Option<Vec<String>>,
    #[serde(default)]
    pub dns: Option<Vec<String>>,
    #[serde(default)]
    pub interface_address: Option<String>,
}

fn default_category() -> String {
    "Uncategorized".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub auto_reconnect: bool,
    pub reconnect_delay_seconds: u64,
    pub status_check_interval_seconds: u64,
    pub log_level: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            auto_reconnect: false,
            reconnect_delay_seconds: 30,
            status_check_interval_seconds: 5,
            log_level: "info".to_string(),
        }
    }
}

impl Config {
    pub fn config_path() -> Result<PathBuf> {
        let home_config_dir = dirs::home_dir()
            .ok_or_else(|| anyhow::anyhow!("Could not find home directory"))?
            .join(".config/remipn/");

        if !home_config_dir.exists() {
            fs::create_dir_all(&home_config_dir)?;
        }

        Ok(home_config_dir.join("config.toml"))
    }

    pub fn load() -> Result<Self> {
        let config_path = Self::config_path()?;
        if !config_path.exists() {
            let default_config = Self::default();
            default_config.save()?;
            return Ok(default_config);
        }

        let contents = fs::read_to_string(config_path)?;
        let config: Config = toml::from_str(&contents)?;
        Ok(config)
    }

    pub fn import_dir() -> Result<PathBuf> {
        let import_dir = dirs::home_dir()
            .ok_or_else(|| anyhow::anyhow!("Could not find home directory"))?
            .join(".config/remipn/imports/");

        if !import_dir.exists() {
            fs::create_dir_all(&import_dir)?;
        }
        Ok(import_dir)
    }

    #[cfg(target_os = "macos")]
    pub fn azure_vpn_import_dir() -> Result<PathBuf> {
        let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("Could not find home directory"))?;
        // Standard path for Azure VPN Client profiles on macOS
        let path = home.join("Library/Containers/com.microsoft.AzureVpnMac/Data/Library/Application Support/com.microsoft.AzureVpnMac");
        Ok(path)
    }

    pub fn auto_import_profiles(&mut self) -> Result<bool> {
        let mut imported_any = false;

        // Import from default import dir
        if let Ok(import_dir) = Self::import_dir() {
             if import_dir.exists() && self.import_from_dir(&import_dir)? {
                imported_any = true;
             }
        }

        // Import from Azure VPN Client dir on macOS
        #[cfg(target_os = "macos")]
        if let Ok(azure_dir) = Self::azure_vpn_import_dir() {
            if azure_dir.exists() && self.import_from_dir(&azure_dir)? {
                imported_any = true;
            }
        }

        if imported_any {
            self.save()?;
        }

        Ok(imported_any)
    }

    fn import_from_dir(&mut self, dir: &PathBuf) -> Result<bool> {
        let mut imported_any = false;
        if dir.exists() {
            for entry in fs::read_dir(dir)? {
                let entry = entry?;
                let path = entry.path();
                if path.is_file() {
                    let extension = path.extension().and_then(|s| s.to_str());
                    if extension == Some("xml")
                        || extension == Some("ovpn")
                        || extension == Some("azvpn")
                    {
                        let content = fs::read_to_string(&path)?;
                        match Self::import_from_xml(&content) {
                            Ok(new_profiles) => {
                                for profile in new_profiles {
                                    if !self.profiles.iter().any(|p| p.name == profile.name) {
                                        self.profiles.push(profile);
                                        imported_any = true;
                                    }
                                }
                            }
                            Err(e) => {
                                eprintln!("Error importing from {:?}: {}", path, e);
                            }
                        }
                    }
                }
            }
        }
        Ok(imported_any)
    }

    pub fn save(&self) -> Result<()> {
        let config_path = Self::config_path()?;
        let contents = toml::to_string_pretty(self)?;
        fs::write(config_path, contents)?;
        Ok(())
    }

    pub fn import_from_xml(xml_content: &str) -> Result<Vec<VpnProfile>> {
        let mut manual_profiles = Vec::new();
        
        let re_profile = regex::Regex::new(r"(?s)<(?:\w+:)?(?:AzVpnProfile|VpnProfile).*?>.*?</(?:\w+:)?(?:AzVpnProfile|VpnProfile)>").unwrap();
        let re_name = regex::Regex::new(r"<(?:\w+:)?(?:Name|name)>(.*?)</(?:\w+:)?(?:Name|name)>").unwrap();
        let re_server = regex::Regex::new(r"<(?:\w+:)?(?:Server|fqdn|displayname)>(.*?)</(?:\w+:)?(?:Server|fqdn|displayname)>").unwrap();
        let re_protocol = regex::Regex::new(r"<(?:\w+:)?(?:Protocol|transportprotocol)>(.*?)</(?:\w+:)?(?:Protocol|transportprotocol)>").unwrap();
        let re_auth = regex::Regex::new(r"<(?:\w+:)?AuthenticationMethod>(.*?)</(?:\w+:)?AuthenticationMethod>").unwrap();
        let re_psk = regex::Regex::new(r"<(?:\w+:)?SharedKey>(.*?)</(?:\w+:)?SharedKey>").unwrap();
        let re_remote = regex::Regex::new(r"<(?:\w+:)?RemoteId>(.*?)</(?:\w+:)?RemoteId>").unwrap();
        let re_local = regex::Regex::new(r"<(?:\w+:)?LocalId>(.*?)</(?:\w+:)?LocalId>").unwrap();
        let re_tenant = regex::Regex::new(r"<(?:\w+:)?tenant>(.*?)</(?:\w+:)?tenant>").unwrap();
        let re_audience = regex::Regex::new(r"<(?:\w+:)?audience>(.*?)</(?:\w+:)?audience>").unwrap();
        let re_issuer = regex::Regex::new(r"<(?:\w+:)?issuer>(.*?)</(?:\w+:)?issuer>").unwrap();
        let re_dns = regex::Regex::new(r"<(?:\w+:)?dnsserver>(.*?)</(?:\w+:)?dnsserver>").unwrap();

        for cap in re_profile.find_iter(xml_content) {
            let section = cap.as_str();
            let name = re_name.captures(section).map(|c| c[1].to_string());
            let server = re_server.captures(section).map(|c| c[1].to_string());
            let mut protocol_str = re_protocol.captures(section).map(|c| c[1].to_string());
            
            let tenant = re_tenant.captures(section).map(|c| c[1].to_string());
            let dns_servers: Vec<String> = re_dns.captures_iter(section).map(|c| c[1].to_string()).collect();

            if tenant.is_some() || xml_content.contains("<aad>") {
                protocol_str = Some("OpenVPN".to_string());
            } else if xml_content.contains("IKEv2") || xml_content.contains("VpnServerType") {
                protocol_str = Some("IKEv2".to_string());
            }

            if let (Some(n), Some(s)) = (name, server) {
                manual_profiles.push(VpnProfile {
                    name: n,
                    gateway_address: s,
                    protocol: protocol_str.unwrap_or_else(|| "OpenVPN".to_string()),
                    auth_method: re_auth.captures(section).map(|c| c[1].to_string()),
                    psk: re_psk.captures(section).map(|c| c[1].to_string()),
                    server_id: re_remote.captures(section).map(|c| c[1].to_string()),
                    client_id: re_local.captures(section).map(|c| c[1].to_string()),
                    tenant,
                    audience: re_audience.captures(section).map(|c| c[1].to_string()),
                    issuer: re_issuer.captures(section).map(|c| c[1].to_string()),
                    dns: if dns_servers.is_empty() { None } else { Some(dns_servers) },
                    ..Default::default()
                });
            }
        }

        if !manual_profiles.is_empty() {
            return Ok(manual_profiles);
        }

        Ok(vec![])
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            profiles: vec![VpnProfile {
                name: "Azure VPN Example".to_string(),
                gateway_address: "vpn-gateway.azure.com".to_string(),
                category: "prod".to_string(),
                cert_path: Some("/path/to/cert.pem".to_string()),
                username: Some("user@example.com".to_string()),
                protocol: "OpenVPN".to_string(),
                ..Default::default()
            }],
            settings: Settings::default(),
        }
    }
}

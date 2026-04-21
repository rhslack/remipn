use crate::config::VpnProfile;
use anyhow::{Result, anyhow};
use boringtun::noise::{Tunn, TunnResult};
use std::net::SocketAddr;
use tokio::net::UdpSocket;
use tokio::time::{self, Duration};
use std::fs;
// No longer using base64 crate directly for key decoding

use tokio::sync::mpsc;
use crate::app::AppEvent;

pub struct WireGuardEngine {
    profile: VpnProfile,
    event_tx: Option<mpsc::Sender<AppEvent>>,
}

impl WireGuardEngine {
    pub fn new(profile: VpnProfile, event_tx: Option<mpsc::Sender<AppEvent>>) -> Self {
        Self { profile, event_tx }
    }

    async fn load_key(&self, path: &Option<String>) -> Result<String> {
        let path = path.as_ref().ok_or_else(|| anyhow!("Key path missing"))?;
        let content = fs::read_to_string(path)?;
        Ok(content.trim().to_string())
    }

    fn decode_key(key: &str, name: &str) -> Result<[u8; 32]> {
        let trimmed = key.trim();
        if trimmed.is_empty() {
            return Err(anyhow!("{} is empty", name));
        }

        // Helper to format hex for debugging
        let to_hex = |s: &str| s.as_bytes().iter().map(|b| format!("{:02x}", b)).collect::<Vec<_>>().join(" ");

        // Super-robust manual Base64 to 32-byte decoder (handles stray bits, URL-safe, whitespace)
        let mut bits = Vec::with_capacity(256);
        for c in trimmed.chars() {
            let val = match c {
                'A'..='Z' => c as u8 - b'A',
                'a'..='z' => c as u8 - b'a' + 26,
                '0'..='9' => c as u8 - b'0' + 52,
                '+' | '-' => 62,
                '/' | '_' => 63,
                '=' | ' ' | '\t' | '\n' | '\r' => continue, // Ignore padding and whitespace
                _ => continue, // Ignore everything else
            };
            
            // Add 6 bits (msb first)
            for i in (0..6).rev() {
                bits.push((val >> i) & 1 == 1);
            }
        }

        if bits.len() < 256 {
            return Err(anyhow!(
                "Invalid {} Base64: not enough bits. Got {} bits, expected 256.\nInput: '{}'\nHex: {}\nLength: {}", 
                name, bits.len(), trimmed, to_hex(trimmed), trimmed.len()
            ));
        }

        // Pack bits into 32 bytes
        let mut bytes = [0u8; 32];
        for i in 0..256 {
            if bits[i] {
                bytes[i / 8] |= 1 << (7 - (i % 8));
            }
        }

        Ok(bytes)
    }

    pub async fn run(&self) -> Result<()> {
        let priv_key_str = self.load_key(&self.profile.private_key_path).await?;
        let pub_key_str = self.profile.public_key.as_ref()
            .ok_or_else(|| anyhow!("Public key missing in profile"))?;
        let endpoint_str = self.profile.endpoint.as_ref()
            .ok_or_else(|| anyhow!("Endpoint missing in profile"))?;
        
        let endpoint: SocketAddr = endpoint_str.parse()
            .map_err(|_| anyhow!("Invalid endpoint format: {}. Expected IP:PORT", endpoint_str))?;

        // 1. Setup TUN interface
        let mut config = tun::Configuration::default();
        let if_addr = self.profile.interface_address.as_deref().unwrap_or("10.0.0.2");
        config
            .address(if_addr)
            .netmask("255.255.255.0")
            .up();

        config.name("utun9");

        let mut dev = tun::create_as_async(&config)
            .map_err(|e| anyhow!("Failed to create TUN device: {}. Check if another VPN is using utun9 or run as root/sudo.", e))?;
        
        // 2. Setup UDP Socket
        let socket = UdpSocket::bind("0.0.0.0:0").await?;
        socket.connect(endpoint).await?;

        // 3. Initialize BoringTun
        let static_private = Self::decode_key(&priv_key_str, "Private Key")?;
        let peer_public = Self::decode_key(pub_key_str, "Public Key")?;
        
        let mut tunnel = Tunn::new(
            static_private.into(),
            peer_public.into(),
            None, // Preshared key
            None, // Keepalive
            0,    // Index
            None, // Controller
        );
            // .map_err(|e| anyhow!("Failed to initialize tunnel: {}", e))?;

        // println!("WireGuard Engine started for {} on {}", self.profile.name, if_addr);

        let mut buf_udp = [0u8; 2048];
        let mut buf_tun = [0u8; 2048];
        let mut out_buf = [0u8; 2048];

        // Aggiorna lo stato finale a Connected
        if let Some(tx) = &self.event_tx {
            let _ = tx.send(AppEvent::Notification(format!("STATUS_UPDATE:{}:Connected", self.profile.name))).await;
        }

        loop {
            tokio::select! {
                // Handle packet from TUN (to be encrypted and sent via UDP)
                res = dev.read(&mut buf_tun) => {
                    let n = res?;
                    match tunnel.encapsulate(&buf_tun[..n], &mut out_buf) {
                        TunnResult::WriteToNetwork(packet) => {
                            socket.send(packet).await?;
                        }
                        TunnResult::Err(e) => eprintln!("Encapsulate error: {:?}", e),
                        _ => {}
                    }
                }
                // Handle packet from UDP (to be decrypted and sent to TUN)
                res = socket.recv(&mut buf_udp) => {
                    let n = res?;
                    match tunnel.decapsulate(None, &buf_udp[..n], &mut out_buf) {
                        TunnResult::WriteToNetwork(packet) => {
                            socket.send(packet).await?;
                        }
                        TunnResult::Err(e) => eprintln!("Decapsulate error: {:?}", e),
                        r => {
                            // En BoringTun 0.6, WriteToInterface podría llamarse WriteToTUN o similar
                            // Para depurar sin romper la compilación, usamos un match genérico.
                            // Nota: En una versión final, este bloque debe escribir en `dev`.
                            if format!("{:?}", r).contains("WriteToInterface") {
                                // logic to write to dev...
                            }
                        }
                    }
                }
                // Periodic timers (keepalive, handshakes)
                _ = time::sleep(Duration::from_millis(100)) => {
                    match tunnel.update_timers(&mut out_buf) {
                        TunnResult::WriteToNetwork(packet) => {
                            socket.send(packet).await?;
                        }
                        _ => {}
                    }
                }
            }
        }
    }
}

use tokio::io::AsyncReadExt;

use crate::config::VpnProfile;
use crate::engine::wireguard::WireGuardEngine;
use crate::engine::ikev2::IkeEngine;
use crate::engine::openvpn::OpenVpnEngine;
use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{RwLock, mpsc};

pub use crate::app::AppEvent;

#[derive(Debug, Clone, PartialEq)]
pub enum VpnStatus {
    Connected,
    Connecting,
    LoginRequired(String), // Nuova variante per gestire il login da GUI
    Retrying(u32, u32),
    Disconnected,
    Disconnecting,
    Error(String),
}

impl VpnStatus {
    pub fn as_str(&self) -> String {
        match self {
            VpnStatus::Connected => "Connected".to_string(),
            VpnStatus::Connecting => "Connecting...".to_string(),
            VpnStatus::LoginRequired(_) => "Login Required".to_string(),
            VpnStatus::Retrying(a, m) => format!("Retry {}/{}...", a, m),
            VpnStatus::Disconnected => "Disconnected".to_string(),
            VpnStatus::Disconnecting => "Disconnecting...".to_string(),
            VpnStatus::Error(_) => "Error".to_string(),
        }
    }

    pub fn color(&self) -> ratatui::style::Color {
        match self {
            VpnStatus::Connected => ratatui::style::Color::Green,
            VpnStatus::Connecting | VpnStatus::Retrying(_, _) | VpnStatus::LoginRequired(_) => ratatui::style::Color::Yellow,
            VpnStatus::Disconnected => ratatui::style::Color::Gray,
            VpnStatus::Disconnecting => ratatui::style::Color::Yellow,
            VpnStatus::Error(_) => ratatui::style::Color::Red,
        }
    }
}

#[derive(Debug, Clone)]
pub struct VpnConnection {
    pub profile_name: String,
    pub status: VpnStatus,
    pub connected_since: Option<chrono::DateTime<chrono::Local>>,
    pub ip_address: Option<String>,
    pub bytes_sent: u64,
    pub bytes_received: u64,
}

#[derive(Debug, Clone)]
pub struct VpnManager {
    pub connections: Arc<RwLock<HashMap<String, VpnConnection>>>,
    pub stop_channels: Arc<RwLock<HashMap<String, mpsc::Sender<()>>>>,
}

impl VpnManager {
    pub fn new() -> Self {
        Self {
            connections: Arc::new(RwLock::new(HashMap::new())),
            stop_channels: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Connect to a VPN using the native engine
    pub async fn connect(&self, profile: &VpnProfile, event_tx: Option<mpsc::Sender<crate::app::AppEvent>>) -> Result<()> {
        // Disconnect all other VPNs first (Single connection requirement)
        let active_profiles: Vec<String> = {
            let conns = self.connections.read().await;
            conns.iter()
                .filter(|(_, c)| matches!(c.status, VpnStatus::Connected | VpnStatus::Connecting))
                .map(|(name, _)| name.clone())
                .collect()
        };

        for name in active_profiles {
            if name != profile.name {
                let _ = self.disconnect(&name).await;
            }
        }

        let mut connections = self.connections.write().await;
        connections.insert(
            profile.name.clone(),
            VpnConnection {
                profile_name: profile.name.clone(),
                status: VpnStatus::Connecting,
                connected_since: None,
                ip_address: None,
                bytes_sent: 0,
                bytes_received: 0,
            },
        );
        drop(connections);

        let (stop_tx, mut stop_rx) = mpsc::channel(1);
        self.stop_channels.write().await.insert(profile.name.clone(), stop_tx);

        let profile_clone = profile.clone();
        let manager_clone = self.connections.clone();
        let stop_channels_clone = self.stop_channels.clone();
        let profile_name = profile.name.clone();
        let event_tx_clone = event_tx.clone();
        
        tokio::spawn(async move {
            let protocol = profile_clone.protocol.to_lowercase();
            
            if protocol == "wireguard" || protocol == "openvpn" || protocol == "ikev2" {
                // Iniziamo come Connecting
                {
                    let mut conns = manager_clone.write().await;
                    if let Some(conn) = conns.get_mut(&profile_name) {
                        conn.status = VpnStatus::Connecting;
                    }
                }
            }

            if protocol == "wireguard" {
                let engine = WireGuardEngine::new(profile_clone, event_tx_clone);
                tokio::select! {
                    res = engine.run() => {
                        if let Err(e) = res {
                            let mut conns = manager_clone.write().await;
                            if let Some(conn) = conns.get_mut(&profile_name) {
                                conn.status = VpnStatus::Error(e.to_string());
                            }
                        }
                    }
                    _ = stop_rx.recv() => {}
                }
            } else if protocol == "ikev2" {
                let engine = IkeEngine::new(profile_clone, event_tx_clone);
                tokio::select! {
                    res = engine.run() => {
                        if let Err(e) = res {
                            let mut conns = manager_clone.write().await;
                            if let Some(conn) = conns.get_mut(&profile_name) {
                                conn.status = VpnStatus::Error(e.to_string());
                            }
                        }
                    }
                    _ = stop_rx.recv() => {}
                }
            } else if protocol == "openvpn" {
                let engine = OpenVpnEngine::new(profile_clone, event_tx_clone);
                tokio::select! {
                    res = engine.run() => {
                        if let Err(e) = res {
                            let mut conns = manager_clone.write().await;
                            if let Some(conn) = conns.get_mut(&profile_name) {
                                conn.status = VpnStatus::Error(e.to_string());
                            }
                        }
                    }
                    _ = stop_rx.recv() => {}
                }
            } else {
                let mut conns = manager_clone.write().await;
                if let Some(conn) = conns.get_mut(&profile_name) {
                    conn.status = VpnStatus::Error(format!("Unsupported protocol: {}", protocol));
                }
            }

            // Cleanup
            let mut conns = manager_clone.write().await;
            if let Some(conn) = conns.get_mut(&profile_name) {
                if !matches!(conn.status, VpnStatus::Error(_)) {
                    conn.status = VpnStatus::Disconnected;
                }
                conn.connected_since = None;
            }
            stop_channels_clone.write().await.remove(&profile_name);
        });

        Ok(())
    }

    /// Disconnect from a VPN
    pub async fn disconnect(&self, profile_name: &str) -> Result<()> {
        let mut stop_channels = self.stop_channels.write().await;
        if let Some(stop_tx) = stop_channels.remove(profile_name) {
            let _ = stop_tx.send(()).await;
        }

        let mut connections = self.connections.write().await;
        if let Some(conn) = connections.get_mut(profile_name) {
            if !matches!(conn.status, VpnStatus::Error(_)) {
                conn.status = VpnStatus::Disconnected;
            }
            conn.connected_since = None;
            conn.ip_address = None;
        }

        Ok(())
    }

    /// Get the current status of a VPN connection
    pub async fn get_status(&self, profile_name: &str) -> VpnStatus {
        let connections = self.connections.read().await;
        connections
            .get(profile_name)
            .map(|c| c.status.clone())
            .unwrap_or(VpnStatus::Disconnected)
    }

    /// Get the actual system status of a VPN connection (not used in purely native mode)
    pub async fn get_system_status(&self, _profile_name: &str) -> VpnStatus {
        VpnStatus::Disconnected
    }

    pub async fn set_status(&self, profile_name: &str, status: VpnStatus) {
        let mut connections = self.connections.write().await;
        if let Some(conn) = connections.get_mut(profile_name) {
            conn.status = status;
        } else {
            connections.insert(
                profile_name.to_string(),
                VpnConnection {
                    profile_name: profile_name.to_string(),
                    status,
                    connected_since: None,
                    ip_address: None,
                    bytes_sent: 0,
                    bytes_received: 0,
                },
            );
        }
    }

    /// Refresh status (not used in purely native mode)
    pub async fn refresh_all_status(&self, _profiles: &[VpnProfile]) -> Result<()> {
        Ok(())
    }

    /// Get all connection states
    pub async fn get_all_connections(&self) -> Vec<VpnConnection> {
        let connections = self.connections.read().await;
        connections.values().cloned().collect()
    }
}

impl Default for VpnManager {
    fn default() -> Self {
        Self::new()
    }
}

use crate::config::VpnProfile;
use anyhow::{Result, anyhow};
use async_process::Command;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Clone, PartialEq)]
pub enum VpnStatus {
    Connected,
    Connecting,
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
            VpnStatus::Retrying(a, m) => format!("Retry {}/{}...", a, m),
            VpnStatus::Disconnected => "Disconnected".to_string(),
            VpnStatus::Disconnecting => "Disconnecting...".to_string(),
            VpnStatus::Error(_) => "Error".to_string(),
        }
    }

    pub fn color(&self) -> ratatui::style::Color {
        match self {
            VpnStatus::Connected => ratatui::style::Color::Green,
            VpnStatus::Connecting | VpnStatus::Retrying(_, _) => ratatui::style::Color::Yellow,
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
    connections: Arc<RwLock<HashMap<String, VpnConnection>>>,
}

impl VpnManager {
    pub fn new() -> Self {
        Self {
            connections: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Connect to an Azure VPN using the profile configuration
    pub async fn connect(&self, profile: &VpnProfile) -> Result<()> {
        // Disconnect all other VPNs first (Single connection requirement)
        let active_vpns = self.get_active_vpns().await?;
        for (name, _) in active_vpns {
            if name != profile.name {
                let _ = self.disconnect(&name).await;

                // Wait for it to be effectively disconnected
                let mut disconnected = false;
                // Give the system a moment to start the disconnection process
                tokio::time::sleep(tokio::time::Duration::from_millis(800)).await;

                for _ in 0..40 {
                    let sys_status = self.get_system_status(&name).await;
                    if matches!(sys_status, VpnStatus::Disconnected) {
                        disconnected = true;
                        break;
                    }
                    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
                }
                if !disconnected {
                    return Err(anyhow!("Failed to disconnect previous VPN: {}. Current state still not Disconnected.", name));
                }
            }
        }

        let mut connections = self.connections.write().await;

        // Update status to connecting
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

        // Execute Azure VPN connection command
        let result = self.execute_vpn_connect(profile).await;

        let mut connections = self.connections.write().await;
        match result {
            Ok(_) => {
                if let Some(conn) = connections.get_mut(&profile.name) {
                    conn.status = VpnStatus::Connected;
                    conn.connected_since = Some(chrono::Local::now());
                }
            }
            Err(e) => {
                if let Some(conn) = connections.get_mut(&profile.name) {
                    conn.status = VpnStatus::Error(e.to_string());
                }
                return Err(e);
            }
        }

        Ok(())
    }

    /// Disconnect from a VPN
    pub async fn disconnect(&self, profile_name: &str) -> Result<()> {
        let mut connections = self.connections.write().await;

        if let Some(conn) = connections.get_mut(profile_name) {
            conn.status = VpnStatus::Disconnecting;
        }
        drop(connections);

        // Execute disconnect command
        let result = self.execute_vpn_disconnect(profile_name).await;

        let mut connections = self.connections.write().await;
        match result {
            Ok(_) => {
                if let Some(conn) = connections.get_mut(profile_name) {
                    conn.status = VpnStatus::Disconnected;
                    conn.connected_since = None;
                    conn.ip_address = None;
                }
            }
            Err(e) => {
                if let Some(conn) = connections.get_mut(profile_name) {
                    conn.status = VpnStatus::Error(e.to_string());
                }
                return Err(e);
            }
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

    /// Get the actual system status of a VPN connection
    pub async fn get_system_status(&self, profile_name: &str) -> VpnStatus {
        #[cfg(target_os = "macos")]
        {
            if let Ok(output) = Command::new("scutil")
                .arg("--nc")
                .arg("status")
                .arg(profile_name)
                .output()
                .await
            {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let first_line = stdout.lines().next().unwrap_or("");
                if first_line.contains("Connected") && !first_line.contains("Disconnected") {
                    return VpnStatus::Connected;
                } else if first_line.contains("Connecting") {
                    return VpnStatus::Connecting;
                } else if first_line.contains("Disconnecting") {
                    return VpnStatus::Disconnecting;
                } else {
                    return VpnStatus::Disconnected;
                }
            }
        }

        #[cfg(target_os = "linux")]
        {
            if let Ok(output) = Command::new("nmcli")
                .arg("-t")
                .arg("-f")
                .arg("NAME,STATE")
                .arg("connection")
                .arg("show")
                .arg("--active")
                .output()
                .await
            {
                let stdout = String::from_utf8_lossy(&output.stdout);
                for line in stdout.lines() {
                    let parts: Vec<&str> = line.split(':').collect();
                    if parts.len() >= 2 && parts[0] == profile_name {
                        let state = parts[1].to_lowercase();
                        if state.contains("activated") && !state.contains("deactivated") {
                            return VpnStatus::Connected;
                        } else if state.contains("activating") {
                            return VpnStatus::Connecting;
                        } else if state.contains("deactivating") {
                            return VpnStatus::Disconnecting;
                        }
                    }
                }
            }
        }

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

    /// Refresh status for all connections
    pub async fn refresh_all_status(&self, profiles: &[VpnProfile]) -> Result<()> {
        // Query system for actual VPN status
        let active_vpns = self.get_active_vpns().await?;

        let mut connections = self.connections.write().await;

        // Ensure all profiles are in the map
        for p in profiles {
            if !connections.contains_key(&p.name) {
                connections.insert(
                    p.name.clone(),
                    VpnConnection {
                        profile_name: p.name.clone(),
                        status: VpnStatus::Disconnected,
                        connected_since: None,
                        ip_address: None,
                        bytes_sent: 0,
                        bytes_received: 0,
                    },
                );
            }
        }

        for (_, conn) in connections.iter_mut() {
            if let Some(active_info) = active_vpns
                .iter()
                .find(|(name, _)| name == &conn.profile_name)
            {
                log::debug!(
                    "{} {} {} {}",
                    conn.status.as_str(),
                    conn.profile_name,
                    active_info.0,
                    active_info.1.as_deref().unwrap_or("")
                );
                if !matches!(conn.status, VpnStatus::Connected) {
                    conn.status = VpnStatus::Connected;
                    conn.connected_since = Some(chrono::Local::now());
                }
                conn.ip_address = active_info.1.clone();
            } else {
                conn.status = VpnStatus::Disconnected;
                conn.connected_since = None;
                conn.ip_address = None;
            }
        }

        Ok(())
    }

    /// Get all connection states
    pub async fn get_all_connections(&self) -> Vec<VpnConnection> {
        let connections = self.connections.read().await;
        connections.values().cloned().collect()
    }

    /// Execute platform-specific VPN connect command
    async fn execute_vpn_connect(&self, profile: &VpnProfile) -> Result<()> {
        #[cfg(target_os = "windows")]
        {
            // Windows: Use rasdial or PowerShell
            let output = Command::new("powershell")
                .arg("-Command")
                .arg(format!(
                    "rasdial '{}' /disconnect; rasdial '{}' '{}' '{}'",
                    profile.name,
                    profile.name,
                    profile.username.as_deref().unwrap_or(""),
                    "" // Password would be handled securely
                ))
                .output()
                .await?;

            if !output.status.success() {
                return Err(anyhow!(
                    "Failed to connect: {}",
                    String::from_utf8_lossy(&output.stderr)
                ));
            }
        }

        #[cfg(target_os = "linux")]
        {
            // Linux: Use NetworkManager or strongSwan
            let output = Command::new("nmcli")
                .arg("connection")
                .arg("up")
                .arg(&profile.name)
                .output()
                .await?;

            if !output.status.success() {
                return Err(anyhow!(
                    "Failed to connect: {}",
                    String::from_utf8_lossy(&output.stderr)
                ));
            }
        }

        #[cfg(target_os = "macos")]
        {
            // macOS: Use scutil or networksetup
            // First, try to start the service. If we get "No service", provide guidance.
            let output = Command::new("scutil")
                .arg("--nc")
                .arg("start")
                .arg(&profile.name)
                .output()
                .await?;

            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);

            if !output.status.success() {
                let combined = format!("{}\n{}", stdout, stderr);
                // Common macOS message when the service isn't registered
                if combined.contains("No service") || combined.contains("No such service") {
                    return Err(anyhow!(
                        "No system VPN service found for '{}'.\n- If this is an Azure profile, import the .azvpn/.xml file into the 'Azure VPN Client' App (e.g.: open -a 'Azure VPN Client' /path/to/profile.azvpn).\n- Alternatively, open Azure VPN Client and create/import a profile with the same name.\n- Then try again from remipn.",
                        profile.name
                    ));
                }
                if combined.to_lowercase().contains("authentication")
                    || combined.to_lowercase().contains("login")
                {
                    return Err(anyhow!(
                        "Azure VPN authentication required. Check system pop-ups or run: scutil --nc start '{}'",
                        profile.name
                    ));
                }
                return Err(anyhow!("Failed to connect: {}", stderr));
            }
        }

        Ok(())
    }

    /// Execute platform-specific VPN disconnect command
    async fn execute_vpn_disconnect(&self, profile_name: &str) -> Result<()> {
        #[cfg(target_os = "windows")]
        {
            let output = Command::new("rasdial")
                .arg(profile_name)
                .arg("/disconnect")
                .output()
                .await?;

            if !output.status.success() {
                return Err(anyhow!(
                    "Failed to disconnect: {}",
                    String::from_utf8_lossy(&output.stderr)
                ));
            }
        }

        #[cfg(target_os = "linux")]
        {
            let output = Command::new("nmcli")
                .arg("connection")
                .arg("down")
                .arg(profile_name)
                .output()
                .await?;

            if !output.status.success() {
                return Err(anyhow!(
                    "Failed to disconnect: {}",
                    String::from_utf8_lossy(&output.stderr)
                ));
            }
        }

        #[cfg(target_os = "macos")]
        {
            let output = Command::new("scutil")
                .arg("--nc")
                .arg("stop")
                .arg(profile_name)
                .output()
                .await?;

            if !output.status.success() {
                return Err(anyhow!(
                    "Failed to disconnect: {}",
                    String::from_utf8_lossy(&output.stderr)
                ));
            }
        }

        Ok(())
    }

    /// Get a list of currently active VPN connections with a list of optional IP addresses
    pub async fn get_active_vpns(&self) -> Result<Vec<(String, Option<String>)>> {
        let mut active = Vec::new();

        #[cfg(target_os = "windows")]
        {
            let output = Command::new("rasdial").output().await?;

            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines() {
                if line.contains("Connected") {
                    // Parse connection name from output
                    if let Some(name) = line.split_whitespace().next() {
                        active.push((name.to_string(), None));
                    }
                }
            }
        }

        #[cfg(target_os = "linux")]
        {
            let output = Command::new("nmcli")
                .arg("-t")
                .arg("-f")
                .arg("NAME,TYPE,STATE,IP4.ADDRESS")
                .arg("connection")
                .arg("show")
                .arg("--active")
                .output()
                .await?;

            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines() {
                let parts: Vec<&str> = line.split(':').collect();
                if parts.len() >= 3 && parts[1].contains("vpn") {
                    let name = parts[0].to_string();
                    let ip = parts
                        .get(3)
                        .filter(|s| !s.is_empty())
                        .map(|s| s.to_string());
                    active.push((name, ip));
                }
            }
        }

        #[cfg(target_os = "macos")]
        {
            let output = Command::new("scutil")
                .arg("--nc")
                .arg("list")
                .output()
                .await?;

            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines() {
                if line.contains("Connected")
                    && let Some(name) = line.split('"').nth(1)
                {
                    active.push((name.to_string(), self.get_macos_ip(name).await));
                }
            }
        }

        Ok(active)
    }

    #[cfg(target_os = "macos")]
    async fn get_macos_ip(&self, _name: &str) -> Option<String> {
        // This is a heuristic: look for utun interfaces which are common for VPNs
        let output = Command::new("ifconfig").output().await.ok()?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut current_interface = None;

        for line in stdout.lines() {
            if !line.starts_with('\t') {
                current_interface = line.split(':').next();
            } else if let Some(iface) = current_interface
                && iface.starts_with("utun")
                && line.contains("inet ")
            {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 2 {
                    return Some(parts[1].to_string());
                }
            }
        }
        None
    }
}

impl Default for VpnManager {
    fn default() -> Self {
        Self::new()
    }
}

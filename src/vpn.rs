use crate::config::VpnProfile;
use anyhow::{Result, anyhow};
use async_process::Command;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Which mechanism a profile uses to establish its tunnel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Backend {
    /// Delegate to the OS's own VPN connection manager. This is how every
    /// profile worked before backends existed, including Azure VPN ones.
    #[default]
    OsNative,
    /// Bring the tunnel up/down directly via `wg`/`wg-quick`.
    WireGuard,
    /// Bring the tunnel up/down directly via the `openvpn` binary.
    OpenVpn,
}

impl Backend {
    pub const ALL: [Backend; 3] = [Backend::OsNative, Backend::WireGuard, Backend::OpenVpn];

    pub fn label(&self) -> &'static str {
        match self {
            Backend::OsNative => "OS-native (NetworkManager/scutil/RAS)",
            Backend::WireGuard => "WireGuard (wg-quick)",
            Backend::OpenVpn => "OpenVPN (openvpn)",
        }
    }

    pub fn cycle_next(self) -> Self {
        let idx = Self::ALL.iter().position(|b| *b == self).unwrap_or(0);
        Self::ALL[(idx + 1) % Self::ALL.len()]
    }

    pub fn cycle_prev(self) -> Self {
        let idx = Self::ALL.iter().position(|b| *b == self).unwrap_or(0);
        Self::ALL[(idx + Self::ALL.len() - 1) % Self::ALL.len()]
    }
}

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
    backend: Backend,
}

/// Executes the platform/provider-specific commands needed to connect,
/// disconnect, and query the status of a VPN connection. Each VPN backend
/// (OS-native, WireGuard, a commercial provider's CLI, ...) implements this
/// so `VpnManager` can dispatch to the right one per profile.
#[async_trait]
trait VpnBackend: Send + Sync {
    async fn connect(&self, profile: &VpnProfile) -> Result<()>;
    async fn disconnect(&self, profile_name: &str) -> Result<()>;
    async fn system_status(&self, profile_name: &str) -> VpnStatus;
    async fn active_vpns(&self) -> Result<Vec<(String, Option<String>)>>;
}

/// Delegates to the OS's own VPN connection manager (NetworkManager on
/// Linux, scutil on macOS, RAS on Windows). Assumes the connection already
/// exists at the OS level (e.g. imported via the Azure VPN Client or
/// NetworkManager) under the same name as the profile.
struct OsNativeBackend;

#[async_trait]
impl VpnBackend for OsNativeBackend {
    /// Execute platform-specific VPN connect command
    async fn connect(&self, profile: &VpnProfile) -> Result<()> {
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
    async fn disconnect(&self, profile_name: &str) -> Result<()> {
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

    /// Get the actual system status of a VPN connection
    async fn system_status(&self, profile_name: &str) -> VpnStatus {
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

    /// Get a list of currently active VPN connections with a list of optional IP addresses
    async fn active_vpns(&self) -> Result<Vec<(String, Option<String>)>> {
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
                // NetworkManager reports native WireGuard connections with
                // TYPE=wireguard rather than TYPE=vpn, so both must be
                // accepted here or they're silently treated as inactive.
                if parts.len() >= 3 && (parts[1].contains("vpn") || parts[1] == "wireguard") {
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
                    active.push((name.to_string(), self.macos_ip(name).await));
                }
            }
        }

        Ok(active)
    }
}

impl OsNativeBackend {
    #[cfg(target_os = "macos")]
    async fn macos_ip(&self, _name: &str) -> Option<String> {
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

/// Brings a WireGuard tunnel up/down directly via `wg-quick`/`wg`, bypassing
/// the OS's own VPN manager. `profile.cert_path` must point at the tunnel's
/// `.conf` file; the interface name wg-quick derives from it is expected to
/// match `profile.name` (the importer guarantees this).
struct WireGuardBackend;

#[async_trait]
impl VpnBackend for WireGuardBackend {
    async fn connect(&self, profile: &VpnProfile) -> Result<()> {
        let config_path = profile.cert_path.as_deref().ok_or_else(|| {
            anyhow!(
                "WireGuard profile '{}' has no config file path set (Certificate Path field)",
                profile.name
            )
        })?;

        let output = Command::new("wg-quick")
            .arg("up")
            .arg(config_path)
            .output()
            .await?;

        if !output.status.success() {
            return Err(anyhow!(
                "Failed to bring up WireGuard tunnel: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        Ok(())
    }

    async fn disconnect(&self, profile_name: &str) -> Result<()> {
        let output = Command::new("wg-quick")
            .arg("down")
            .arg(profile_name)
            .output()
            .await?;

        if !output.status.success() {
            return Err(anyhow!(
                "Failed to bring down WireGuard tunnel: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        Ok(())
    }

    async fn system_status(&self, profile_name: &str) -> VpnStatus {
        if Self::active_interfaces().await.contains(&profile_name.to_string()) {
            VpnStatus::Connected
        } else {
            VpnStatus::Disconnected
        }
    }

    async fn active_vpns(&self) -> Result<Vec<(String, Option<String>)>> {
        Ok(Self::active_interfaces()
            .await
            .into_iter()
            .map(|name| (name, None))
            .collect())
    }
}

impl WireGuardBackend {
    async fn active_interfaces() -> Vec<String> {
        let Ok(output) = Command::new("wg").arg("show").arg("interfaces").output().await else {
            return Vec::new();
        };
        if !output.status.success() {
            return Vec::new();
        }
        String::from_utf8_lossy(&output.stdout)
            .split_whitespace()
            .map(|s| s.to_string())
            .collect()
    }
}

/// Runs an OpenVPN tunnel directly via the `openvpn` binary, bypassing the
/// OS's own VPN manager. Since a standalone `openvpn` process has no name of
/// its own, this backend tracks it itself via a PID file per profile under
/// `~/.config/remipn/run/`.
struct OpenVpnBackend;

impl OpenVpnBackend {
    fn run_dir() -> Result<PathBuf> {
        let dir = dirs::home_dir()
            .ok_or_else(|| anyhow!("Could not find home directory"))?
            .join(".config/remipn/run");
        std::fs::create_dir_all(&dir)?;
        Ok(dir)
    }

    fn pid_file(profile_name: &str) -> Result<PathBuf> {
        Ok(Self::run_dir()?.join(format!("{}.pid", profile_name)))
    }

    async fn pid_alive(pid: &str) -> bool {
        #[cfg(not(target_os = "windows"))]
        {
            Command::new("kill")
                .arg("-0")
                .arg(pid)
                .output()
                .await
                .map(|o| o.status.success())
                .unwrap_or(false)
        }
        #[cfg(target_os = "windows")]
        {
            let Ok(output) = Command::new("tasklist")
                .arg("/FI")
                .arg(format!("PID eq {}", pid))
                .output()
                .await
            else {
                return false;
            };
            String::from_utf8_lossy(&output.stdout).contains(pid)
        }
    }
}

#[async_trait]
impl VpnBackend for OpenVpnBackend {
    async fn connect(&self, profile: &VpnProfile) -> Result<()> {
        let config_path = profile.cert_path.as_deref().ok_or_else(|| {
            anyhow!(
                "OpenVPN profile '{}' has no config file path set (Certificate Path field)",
                profile.name
            )
        })?;
        let pid_path = Self::pid_file(&profile.name)?;

        let output = Command::new("openvpn")
            .arg("--config")
            .arg(config_path)
            .arg("--daemon")
            .arg(format!("remipn-{}", profile.name))
            .arg("--writepid")
            .arg(&pid_path)
            .output()
            .await?;

        if !output.status.success() {
            return Err(anyhow!(
                "Failed to start openvpn: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        Ok(())
    }

    async fn disconnect(&self, profile_name: &str) -> Result<()> {
        let pid_path = Self::pid_file(profile_name)?;
        let pid = std::fs::read_to_string(&pid_path)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow!("No running OpenVPN process found for '{}'", profile_name))?;

        #[cfg(not(target_os = "windows"))]
        let output = Command::new("kill").arg(&pid).output().await?;
        #[cfg(target_os = "windows")]
        let output = Command::new("taskkill")
            .arg("/PID")
            .arg(&pid)
            .arg("/F")
            .output()
            .await?;

        if !output.status.success() {
            return Err(anyhow!(
                "Failed to stop openvpn: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        let _ = std::fs::remove_file(&pid_path);
        Ok(())
    }

    async fn system_status(&self, profile_name: &str) -> VpnStatus {
        let Ok(pid_path) = Self::pid_file(profile_name) else {
            return VpnStatus::Disconnected;
        };
        let Some(pid) = std::fs::read_to_string(&pid_path)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
        else {
            return VpnStatus::Disconnected;
        };
        if Self::pid_alive(&pid).await {
            VpnStatus::Connected
        } else {
            VpnStatus::Disconnected
        }
    }

    async fn active_vpns(&self) -> Result<Vec<(String, Option<String>)>> {
        let Ok(dir) = Self::run_dir() else {
            return Ok(Vec::new());
        };
        if !dir.exists() {
            return Ok(Vec::new());
        }

        let mut active = Vec::new();
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("pid") {
                continue;
            }
            let Some(name) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let Ok(pid) = std::fs::read_to_string(&path) else {
                continue;
            };
            let pid = pid.trim();
            if !pid.is_empty() && Self::pid_alive(pid).await {
                active.push((name.to_string(), None));
            }
        }
        Ok(active)
    }
}

#[derive(Clone)]
pub struct VpnManager {
    connections: Arc<RwLock<HashMap<String, VpnConnection>>>,
    os_native: Arc<OsNativeBackend>,
    wireguard: Arc<WireGuardBackend>,
    openvpn: Arc<OpenVpnBackend>,
}

impl VpnManager {
    pub fn new() -> Self {
        Self {
            connections: Arc::new(RwLock::new(HashMap::new())),
            os_native: Arc::new(OsNativeBackend),
            wireguard: Arc::new(WireGuardBackend),
            openvpn: Arc::new(OpenVpnBackend),
        }
    }

    fn backend_for(&self, kind: Backend) -> Arc<dyn VpnBackend> {
        match kind {
            Backend::OsNative => self.os_native.clone(),
            Backend::WireGuard => self.wireguard.clone(),
            Backend::OpenVpn => self.openvpn.clone(),
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
                    return Err(anyhow!(
                        "Failed to disconnect previous VPN: {}. Current state still not Disconnected.",
                        name
                    ));
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
                backend: profile.backend,
            },
        );
        drop(connections);

        // Execute VPN connection command
        let result = self.backend_for(profile.backend).connect(profile).await;

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
        let backend_kind = {
            let connections = self.connections.read().await;
            connections.get(profile_name).map(|c| c.backend)
        };

        let mut connections = self.connections.write().await;

        if let Some(conn) = connections.get_mut(profile_name) {
            conn.status = VpnStatus::Disconnecting;
        }
        drop(connections);

        // Execute disconnect command. When the profile's backend isn't
        // known (e.g. disconnecting some other active VPN we haven't
        // tracked yet), try OS-native first - this matches the exact
        // behavior from before backends existed - then fall back to the
        // other backends so WireGuard/OpenVPN tunnels started outside this
        // session can still be torn down by name.
        let result = match backend_kind {
            Some(kind) => self.backend_for(kind).disconnect(profile_name).await,
            None => self.disconnect_unknown(profile_name).await,
        };

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

    /// Disconnect a connection whose backend isn't recorded yet, trying
    /// OS-native first (preserving the original pre-backend behavior/error
    /// for that common case) before falling back to the other backends.
    async fn disconnect_unknown(&self, profile_name: &str) -> Result<()> {
        let primary_err = match self.os_native.disconnect(profile_name).await {
            Ok(()) => return Ok(()),
            Err(e) => e,
        };

        let fallback_backends: [Arc<dyn VpnBackend>; 2] =
            [self.wireguard.clone(), self.openvpn.clone()];
        for backend in fallback_backends {
            if backend.disconnect(profile_name).await.is_ok() {
                return Ok(());
            }
        }

        Err(primary_err)
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
        let backend_kind = {
            let connections = self.connections.read().await;
            connections.get(profile_name).map(|c| c.backend)
        };
        match backend_kind {
            Some(kind) => self.backend_for(kind).system_status(profile_name).await,
            None => self.os_native.system_status(profile_name).await,
        }
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
                    backend: Backend::OsNative,
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
                        backend: p.backend,
                    },
                );
            } else if let Some(conn) = connections.get_mut(&p.name) {
                // Keep the recorded backend in sync in case the profile's
                // backend was edited since the connection was first tracked.
                conn.backend = p.backend;
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

    /// Get a list of currently active VPN connections with a list of optional IP addresses
    pub async fn get_active_vpns(&self) -> Result<Vec<(String, Option<String>)>> {
        // OS-native errors (e.g. nmcli/scutil unavailable) still propagate,
        // matching the original single-backend behavior exactly. The newer
        // WireGuard/OpenVPN backends are optional capabilities, so a missing
        // `wg`/`openvpn` binary just means "nothing active there".
        let mut active = self.os_native.active_vpns().await?;
        if let Ok(mut wg) = self.wireguard.active_vpns().await {
            active.append(&mut wg);
        }
        if let Ok(mut ovpn) = self.openvpn.active_vpns().await {
            active.append(&mut ovpn);
        }
        Ok(active)
    }
}

impl Default for VpnManager {
    fn default() -> Self {
        Self::new()
    }
}

use anyhow::Result;
use quick_xml::de::from_str;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub profiles: Vec<VpnProfile>,
    pub settings: Settings,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VpnProfile {
    pub name: String,
    pub gateway_address: String,
    #[serde(default = "default_category")]
    pub category: String,
    pub cert_path: Option<String>,
    pub username: Option<String>,
    #[serde(default)]
    pub aliases: Option<String>,
    pub protocol: String, // IKEv2, OpenVPN, etc.
    pub auto_connect: bool,
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

    pub fn import_dir() -> Result<PathBuf> {
        let import_dir = dirs::home_dir()
            .ok_or_else(|| anyhow::anyhow!("Could not find home directory"))?
            .join(".config/remipn/imports/");

        if !import_dir.exists() {
            fs::create_dir_all(&import_dir)?;
        }
        Ok(import_dir)
    }

    pub fn azure_vpn_import_dir() -> Result<PathBuf> {
        #[cfg(target_os = "macos")]
        {
            let azure_dir = dirs::home_dir()
                .ok_or_else(|| anyhow::anyhow!("Could not find home directory"))?
                .join("Library/Containers/com.microsoft.AzureVpnMac/Data/Library/Application Support/com.microsoft.AzureVpnMac");
            Ok(azure_dir)
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err(anyhow::anyhow!(
                "Azure VPN Client path not supported on this OS"
            ))
        }
    }

    pub fn auto_import_profiles(&mut self) -> Result<bool> {
        let mut imported_any = false;

        // Import from default import dir
        if let Ok(import_dir) = Self::import_dir() {
            if self.import_from_dir(&import_dir)? {
                imported_any = true;
            }
        }

        // Import from Azure VPN Client dir on macOS
        #[cfg(target_os = "macos")]
        {
            if let Ok(azure_dir) = Self::azure_vpn_import_dir() {
                if azure_dir.exists() {
                    if self.import_from_dir(&azure_dir)? {
                        imported_any = true;
                    }
                }
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
                        if let Ok(new_profiles) = Self::import_from_xml(&content) {
                            for np in new_profiles {
                                if !self.profiles.iter().any(|p| p.name == np.name) {
                                    self.profiles.push(np);
                                    imported_any = true;
                                }
                            }
                        }
                    }
                }
            }
        }
        Ok(imported_any)
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

    pub fn save(&self) -> Result<()> {
        let config_path = Self::config_path()?;
        let contents = toml::to_string_pretty(self)?;
        fs::write(config_path, contents)?;
        Ok(())
    }

    pub fn import_from_xml(xml_content: &str) -> Result<Vec<VpnProfile>> {
        #[derive(Debug, Deserialize)]
        struct VpnProfileXml {
            #[serde(rename = "Name")]
            name: Option<String>,
            #[serde(rename = "name")]
            name_lower: Option<String>,
            #[serde(rename = "Server")]
            server: Option<String>,
            #[serde(rename = "fqdn")]
            fqdn: Option<String>,
            #[serde(rename = "Protocol")]
            protocol: Option<String>,
        }

        #[derive(Debug, Deserialize)]
        struct AzVpnProfileXml {
            #[serde(rename = "VpnProfile", default)]
            profiles: Vec<VpnProfileXml>,
        }

        #[derive(Debug, Deserialize)]
        struct VpnSettingsXml {
            #[serde(rename = "VpnProfile", default)]
            profiles: Vec<VpnProfileXml>,
        }

        // Try parsing AzVpnProfile first, then fallback to simple VpnSettings,
        // and finally try to parse as a single VpnProfile
        let profiles = if xml_content.contains("<AzVpnProfile") {
            if let Ok(az_settings) = from_str::<AzVpnProfileXml>(xml_content) {
                az_settings.profiles
            } else {
                // Try parsing the root as a single profile if it has AzVpnProfile tag
                if let Ok(single_profile) = from_str::<VpnProfileXml>(xml_content) {
                    vec![single_profile]
                } else {
                    vec![]
                }
            }
        } else if xml_content.contains("<VpnSettings") {
            if let Ok(settings) = from_str::<VpnSettingsXml>(xml_content) {
                settings.profiles
            } else {
                vec![]
            }
        } else if xml_content.contains("<VpnProfile") {
            if let Ok(single_profile) = from_str::<VpnProfileXml>(xml_content) {
                vec![single_profile]
            } else {
                vec![]
            }
        } else {
            let mut manual_profiles = Vec::new();

            // Extract all <VpnProfile> sections manually (case-insensitive tags if possible, but keeping it simple)
            let re_profile = regex::Regex::new(r"(?s)<(?:\w+:)?(?:AzVpnProfile|VpnProfile).*?>.*?</(?:\w+:)?(?:AzVpnProfile|VpnProfile)>").unwrap();
            let re_name =
                regex::Regex::new(r"<(?:\w+:)?(?:Name|name)>(.*?)</(?:\w+:)?(?:Name|name)>")
                    .unwrap();
            let re_server = regex::Regex::new(r"<(?:\w+:)?(?:Server|fqdn|displayname)>(.*?)</(?:\w+:)?(?:Server|fqdn|displayname)>").unwrap();
            let re_protocol = regex::Regex::new(r"<(?:\w+:)?(?:Protocol|transportprotocol)>(.*?)</(?:\w+:)?(?:Protocol|transportprotocol)>").unwrap();

            for cap in re_profile.find_iter(xml_content) {
                let section = cap.as_str();
                let name = re_name.captures(section).map(|c| c[1].to_string());
                let server = re_server.captures(section).map(|c| c[1].to_string());
                let protocol = re_protocol.captures(section).map(|c| c[1].to_string());

                if let (Some(n), Some(s)) = (name, server) {
                    manual_profiles.push(VpnProfile {
                        name: n,
                        gateway_address: s,
                        category: "Uncategorized".to_string(),
                        cert_path: None,
                        username: None,
                        aliases: None,
                        protocol: protocol.unwrap_or_else(|| "IKEv2".to_string()),
                        auto_connect: false,
                    });
                }
            }

            if !manual_profiles.is_empty() {
                return Ok(manual_profiles);
            }

            return Err(anyhow::anyhow!("Unsupported XML format or parsing error"));
        };

        if profiles.is_empty()
            && (xml_content.contains("VpnProfile") || xml_content.contains("AzVpnProfile"))
        {
            // The second attempt if structured parsing returned empty
            let mut manual_profiles = Vec::new();

            let re_profile = regex::Regex::new(r"(?s)<(?:\w+:)?(?:AzVpnProfile|VpnProfile).*?>.*?</(?:\w+:)?(?:AzVpnProfile|VpnProfile)>").unwrap();
            let re_name =
                regex::Regex::new(r"<(?:\w+:)?(?:Name|name)>(.*?)</(?:\w+:)?(?:Name|name)>")
                    .unwrap();
            let re_server = regex::Regex::new(r"<(?:\w+:)?(?:Server|fqdn|displayname)>(.*?)</(?:\w+:)?(?:Server|fqdn|displayname)>").unwrap();
            let re_protocol = regex::Regex::new(r"<(?:\w+:)?(?:Protocol|transportprotocol)>(.*?)</(?:\w+:)?(?:Protocol|transportprotocol)>").unwrap();

            for cap in re_profile.find_iter(xml_content) {
                let section = cap.as_str();
                let name = re_name.captures(section).map(|c| c[1].to_string());
                let server = re_server.captures(section).map(|c| c[1].to_string());
                let protocol = re_protocol.captures(section).map(|c| c[1].to_string());

                if let (Some(n), Some(s)) = (name, server) {
                    manual_profiles.push(VpnProfile {
                        name: n,
                        gateway_address: s,
                        category: "Uncategorized".to_string(),
                        cert_path: None,
                        username: None,
                        aliases: None,
                        protocol: protocol.unwrap_or_else(|| "IKEv2".to_string()),
                        auto_connect: false,
                    });
                }
            }

            if !manual_profiles.is_empty() {
                return Ok(manual_profiles);
            }
        }

        let mut vpn_profiles = Vec::new();
        for p in profiles {
            let name = p
                .name
                .or(p.name_lower)
                .unwrap_or_else(|| "Unnamed".to_string());
            let server = p.server.or(p.fqdn).unwrap_or_else(|| "unknown".to_string());

            vpn_profiles.push(VpnProfile {
                name,
                gateway_address: server,
                category: "Uncategorized".to_string(),
                cert_path: None,
                username: None,
                aliases: None,
                protocol: p.protocol.unwrap_or_else(|| "IKEv2".to_string()),
                auto_connect: false,
            });
        }

        Ok(vpn_profiles)
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
                aliases: Some("example".to_string()),
                protocol: "IKEv2".to_string(),
                auto_connect: false,
            }],
            settings: Settings::default(),
        }
    }
}

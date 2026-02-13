use crate::config::{Config, VpnProfile};
use crate::vpn::{VpnConnection, VpnManager, VpnStatus};
use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};

pub enum AppEvent {
    Input(KeyEvent),
    Tick,
    VpnStatusUpdated,
    Notification(String),
}
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Screen {
    Main,
    AddProfile,
    EditProfile,
    ImportXml,
    FileBrowser,
    Help,
    DeleteConfirmation,
    Search,
    AliasModal,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FileEntry {
    pub name: String,
    pub path: std::path::PathBuf,
    pub is_dir: bool,
}

use ratatui::widgets::{ListState, TableState};

pub struct FileBrowser {
    pub current_dir: std::path::PathBuf,
    pub entries: Vec<FileEntry>,
    pub selected: usize,
    pub state: ListState,
}

impl FileBrowser {
    pub fn new() -> Result<Self> {
        let current_dir = std::env::current_dir()?;
        let mut browser = Self {
            current_dir,
            entries: Vec::new(),
            selected: 0,
            state: ListState::default(),
        };
        browser.refresh()?;
        Ok(browser)
    }

    pub fn refresh(&mut self) -> Result<()> {
        self.entries.clear();
        
        // Add a parent directory entry if not at root
        if let Some(parent) = self.current_dir.parent() {
            self.entries.push(FileEntry {
                name: "..".to_string(),
                path: parent.to_path_buf(),
                is_dir: true,
            });
        }

        let mut entries = Vec::new();
        for entry in std::fs::read_dir(&self.current_dir)? {
            let entry = entry?;
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            let is_dir = entry.file_type()?.is_dir();
            
            // Only show directories or .xml files
            if is_dir || name.to_lowercase().ends_with(".xml") {
                entries.push(FileEntry {
                    name,
                    path,
                    is_dir,
                });
            }
        }

        // Sort: directories first, then files
        entries.sort_by(|a, b| {
            if a.is_dir != b.is_dir {
                b.is_dir.cmp(&a.is_dir)
            } else {
                a.name.to_lowercase().cmp(&b.name.to_lowercase())
            }
        });

        self.entries.extend(entries);
        self.selected = 0;
        self.state.select(Some(0));
        Ok(())
    }

    pub fn next(&mut self) {
        if !self.entries.is_empty() {
            self.selected = (self.selected + 1) % self.entries.len();
            self.state.select(Some(self.selected));
        }
    }

    pub fn previous(&mut self) {
        if !self.entries.is_empty() {
            self.selected = if self.selected == 0 {
                self.entries.len() - 1
            } else {
                self.selected - 1
            };
            self.state.select(Some(self.selected));
        }
    }

    pub fn enter(&mut self) -> Result<Option<std::path::PathBuf>> {
        if self.entries.is_empty() {
            return Ok(None);
        }

        let entry = &self.entries[self.selected];
        if entry.is_dir {
            self.current_dir = entry.path.clone();
            self.refresh()?;
            Ok(None)
        } else {
            Ok(Some(entry.path.clone()))
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum InputMode {
    Normal,
    Editing,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SortColumn {
    Name,
    Category,
    Status,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SortDirection {
    Asc,
    Desc,
}

pub struct App {
    pub config: Config,
    pub vpn_manager: VpnManager,
    pub screen: Screen,
    pub input_mode: InputMode,
    pub selected_profile: usize,
    pub table_state: TableState,
    pub scroll_offset: usize,
    pub input_buffer: String,
    pub input_field: usize,
    pub status_message: Option<(String, chrono::DateTime<chrono::Local>)>,
    pub show_logs: bool,
    pub logs: Vec<String>,
    pub auto_reconnect: bool,
    pub connections: Vec<VpnConnection>,
    pub last_update: std::time::Instant,
    pub file_browser: Option<FileBrowser>,
    pub search_query: String,
    pub add_profile_data: Vec<String>,
    pub sort_column: SortColumn,
    pub sort_direction: SortDirection,
    pub alias_input: String,
}

impl App {
    pub async fn new() -> Result<Self> {
        let config = Config::load()?;
        let vpn_manager = VpnManager::new();

        let mut app = Self {
            config,
            vpn_manager,
            screen: Screen::Main,
            input_mode: InputMode::Normal,
            selected_profile: 0,
            table_state: TableState::default().with_selected(Some(0)),
            scroll_offset: 0,
            input_buffer: String::new(),
            input_field: 0,
            status_message: None,
            show_logs: false,
            logs: Vec::new(),
            auto_reconnect: false,
            connections: Vec::new(),
            last_update: std::time::Instant::now(),
            file_browser: None,
            search_query: String::new(),
            add_profile_data: vec![String::new(); 6],
            sort_column: SortColumn::Name,
            sort_direction: SortDirection::Asc,
            alias_input: String::new(),
        };

        // Initial status load
        app.refresh_status().await?;
        Ok(app)
    }

    pub async fn handle_event(&mut self, event: AppEvent) -> Result<Option<()>> {
        match event {
            AppEvent::Input(key) => return self.handle_key(key).await,
            AppEvent::Tick => self.update().await?,
            AppEvent::VpnStatusUpdated => self.refresh_status().await?,
            AppEvent::Notification(msg) => {
                self.add_log(msg.clone());
                self.set_status_message(msg);
            }
        }
        Ok(None)
    }

    pub async fn handle_key(&mut self, key: KeyEvent) -> Result<Option<()>> {
        match self.screen {
            Screen::Main => return self.handle_main_screen_key(key).await,
            Screen::AddProfile => self.handle_add_profile_key(key).await?,
            Screen::EditProfile => self.handle_edit_profile_key(key).await?,
            Screen::ImportXml => self.handle_import_xml_key(key).await?,
            Screen::FileBrowser => self.handle_file_browser_key(key).await?,
            Screen::Search => self.handle_search_key(key).await?,
            Screen::AliasModal => self.handle_alias_modal_key(key).await?,
            Screen::DeleteConfirmation => self.handle_delete_confirmation_key(key).await?,
            Screen::Help => {
                if let KeyCode::Esc | KeyCode::Char('h') = key.code {
                    self.screen = Screen::Main;
                }
            }
        }
        Ok(None)
    }

    async fn handle_main_screen_key(&mut self, key: KeyEvent) -> Result<Option<()>> {
        match key.code {
            KeyCode::Char('q') => return Ok(Some(())),
            KeyCode::Up | KeyCode::Char('k') => {
                let profiles_len = self.get_filtered_profiles_indices().len();
                if self.selected_profile > 0 {
                    self.selected_profile -= 1;
                } else if profiles_len > 0 {
                    self.selected_profile = profiles_len - 1;
                }
                self.table_state.select(Some(self.selected_profile));
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let profiles_len = self.get_filtered_profiles_indices().len();
                if profiles_len > 0 {
                    self.selected_profile = (self.selected_profile + 1) % profiles_len;
                }
                self.table_state.select(Some(self.selected_profile));
            }
            KeyCode::PageUp => {
                let profiles_len = self.get_filtered_profiles_indices().len();
                if profiles_len > 0 {
                    if self.selected_profile >= 10 {
                        self.selected_profile -= 10;
                    } else {
                        self.selected_profile = 0;
                    }
                    self.table_state.select(Some(self.selected_profile));
                }
            }
            KeyCode::PageDown => {
                let profiles_len = self.get_filtered_profiles_indices().len();
                if profiles_len > 0 {
                    self.selected_profile = (self.selected_profile + 10).min(profiles_len.saturating_sub(1));
                    self.table_state.select(Some(self.selected_profile));
                }
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                self.toggle_connection().await?;
            }
            KeyCode::Char('n') => {
                self.screen = Screen::AddProfile;
                self.input_mode = InputMode::Editing;
                self.add_profile_data = vec![String::new(); 6];
                self.input_field = 0;
            }
            KeyCode::Char('e') => {
                if !self.config.profiles.is_empty() {
                    self.screen = Screen::EditProfile;
                    self.input_mode = InputMode::Editing;
                    self.load_profile_to_edit();
                    self.input_field = 1; // Start from Gateway Address when editing
                }
            }
            KeyCode::Char('x') => {
                if !self.get_filtered_profiles_indices().is_empty() {
                    self.screen = Screen::DeleteConfirmation;
                }
            }
            KeyCode::Char('/') => {
                self.screen = Screen::Search;
                self.input_mode = InputMode::Editing;
                // Keep search_query or clear it? Let's keep it to allow refining search
            }
            KeyCode::Char('i') => {
                self.screen = Screen::ImportXml;
                self.input_mode = InputMode::Editing;
                self.input_buffer.clear();
                self.input_field = 0;
            }
            KeyCode::Char('r') => {
                self.refresh_status().await?;
            }
            KeyCode::Char('l') => {
                self.show_logs = !self.show_logs;
            }
            KeyCode::Char('s') => {
                self.cycle_sort();
            }
            KeyCode::Char('a') => {
                let indices = self.get_filtered_profiles_indices();
                if !indices.is_empty() && self.selected_profile < indices.len() {
                    let actual_index = indices[self.selected_profile];
                    self.alias_input = self.config.profiles[actual_index].aliases.clone().unwrap_or_default();
                    self.screen = Screen::AliasModal;
                    self.input_mode = InputMode::Editing;
                }
            }
            KeyCode::Char('h') | KeyCode::F(1) => {
                self.screen = Screen::Help;
            }
            KeyCode::Char('R') => {
                self.auto_reconnect = !self.auto_reconnect;
                self.set_status_message(format!(
                    "Auto-reconnect: {}",
                    if self.auto_reconnect { "ON" } else { "OFF" }
                ));
            }
            _ => {}
        }
        Ok(None)
    }

    async fn handle_add_profile_key(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Esc => {
                self.screen = Screen::Main;
                self.input_mode = InputMode::Normal;
            }
            KeyCode::Enter => {
                self.save_new_profile()?;
            }
            KeyCode::Tab => {
                self.input_field = (self.input_field + 1) % 6;
                // Skip the name field (index 0) if editing
                if self.screen == Screen::EditProfile && self.input_field == 0 {
                    self.input_field = 1;
                }
            }
            KeyCode::BackTab => {
                self.input_field = if self.input_field == 0 { 5 } else { self.input_field - 1 };
                // Skip the name field (index 0) if editing
                if self.screen == Screen::EditProfile && self.input_field == 0 {
                    self.input_field = 5;
                }
            }
            KeyCode::Char(c) => {
                // Prevent editing name field if in EditProfile screen
                if !(self.screen == Screen::EditProfile && self.input_field == 0) {
                    self.add_profile_data[self.input_field].push(c);
                }
            }
            KeyCode::Backspace => {
                // Prevent editing name field if in EditProfile screen
                if !(self.screen == Screen::EditProfile && self.input_field == 0) {
                    self.add_profile_data[self.input_field].pop();
                }
            }
            _ => {}
        }
        Ok(())
    }

    async fn handle_edit_profile_key(&mut self, key: KeyEvent) -> Result<()> {
        self.handle_add_profile_key(key).await
    }

    async fn handle_import_xml_key(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Esc => {
                self.screen = Screen::Main;
                self.input_mode = InputMode::Normal;
                self.input_buffer.clear();
            }
            KeyCode::Enter => {
                self.import_profiles_from_file()?;
            }
            KeyCode::Char('f') if self.input_buffer.is_empty() => {
                // Open the file browser only if the buffer is empty
                self.file_browser = Some(FileBrowser::new()?);
                self.screen = Screen::FileBrowser;
            }
            KeyCode::Char(c) => {
                self.input_buffer.push(c);
            }
            KeyCode::Backspace => {
                self.input_buffer.pop();
            }
            _ => {}
        }
        Ok(())
    }

    async fn handle_file_browser_key(&mut self, key: KeyEvent) -> Result<()> {
        if let Some(browser) = &mut self.file_browser {
            match key.code {
                KeyCode::Esc => {
                    self.screen = Screen::ImportXml;
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    browser.previous();
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    browser.next();
                }
                KeyCode::Enter => {
                    if let Some(path) = browser.enter()? {
                        self.input_buffer = path.to_string_lossy().to_string();
                        self.screen = Screen::ImportXml;
                    }
                }
                KeyCode::Backspace => {
                    // Go up one directory if possible
                    if let Some(parent) = browser.current_dir.parent() {
                        browser.current_dir = parent.to_path_buf();
                        browser.refresh()?;
                    }
                }
                _ => {}
            }
        } else {
            self.screen = Screen::ImportXml;
        }
        Ok(())
    }

    async fn handle_search_key(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Esc | KeyCode::Enter => {
                self.screen = Screen::Main;
                self.input_mode = InputMode::Normal;
            }
            KeyCode::Char(c) => {
                self.search_query.push(c);
                self.selected_profile = 0;
                self.table_state.select(Some(0));
            }
            KeyCode::Backspace => {
                self.search_query.pop();
                self.selected_profile = 0;
                self.table_state.select(Some(0));
            }
            _ => {}
        }
        Ok(())
    }

    async fn handle_alias_modal_key(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Esc => {
                self.screen = Screen::Main;
                self.input_mode = InputMode::Normal;
            }
            KeyCode::Enter => {
                let indices = self.get_filtered_profiles_indices();
                if !indices.is_empty() && self.selected_profile < indices.len() {
                    let actual_index = indices[self.selected_profile];
                    let alias = if self.alias_input.is_empty() { None } else { Some(self.alias_input.clone()) };
                    self.config.profiles[actual_index].aliases = alias;
                    self.config.save()?;
                    self.set_status_message("Alias updated".to_string());
                }
                self.screen = Screen::Main;
                self.input_mode = InputMode::Normal;
            }
            KeyCode::Backspace => {
                self.alias_input.pop();
            }
            KeyCode::Char(c) => {
                self.alias_input.push(c);
            }
            _ => {}
        }
        Ok(())
    }

    async fn handle_delete_confirmation_key(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                self.delete_selected_profile()?;
                self.screen = Screen::Main;
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                self.screen = Screen::Main;
            }
            _ => {}
        }
        Ok(())
    }

    fn import_profiles_from_file(&mut self) -> Result<()> {
        let path = self.input_buffer.trim().to_string();
        if path.is_empty() {
            return Ok(());
        }

        match std::fs::read_to_string(&path) {
            Ok(content) => {
                match Config::import_from_xml(&content) {
                    Ok(new_profiles) => {
                        let count = new_profiles.len();
                        for profile in new_profiles {
                            // Avoid duplicates by name
                            if !self.config.profiles.iter().any(|p| p.name == profile.name) {
                                self.config.profiles.push(profile);
                            }
                        }
                        self.config.save()?;
                        self.screen = Screen::Main;
                        self.input_mode = InputMode::Normal;
                        self.set_status_message(format!("Imported {} profiles", count));
                        self.add_log(format!("Successfully imported {} profiles from {}", count, path));
                    }
                    Err(e) => {
                        self.set_status_message(format!("Import error: {}", e));
                        self.add_log(format!("Error parsing XML from {}: {}", path, e));
                    }
                }
            }
            Err(e) => {
                self.set_status_message(format!("File error: {}", e));
                self.add_log(format!("Error reading file {}: {}", path, e));
            }
        }
        Ok(())
    }

    async fn toggle_connection(&mut self) -> Result<()> {
        use std::time::Instant;
        use tokio::time::{sleep, Duration};

        let indices = self.get_filtered_profiles_indices();
        if indices.is_empty() || self.selected_profile >= indices.len() {
            return Ok(());
        }

        let actual_index = indices[self.selected_profile];
        // Clone the profile to avoid borrowing conflicts later
        let profile = self.config.profiles[actual_index].clone();
        let profile_name = profile.name.clone();

        match self.vpn_manager.get_status(&profile_name).await {
            VpnStatus::Connected => {
                // Show progress and wait until fully disconnected
                self.set_status_message(format!("Disconnecting from {}...", profile_name));
                self.add_log(format!("Disconnecting from {}...", profile_name));
                match self.vpn_manager.disconnect(&profile_name).await {
                    Ok(_) => {
                        // Wait for verification of disconnection
                        let start = Instant::now();
                        let timeout = Duration::from_secs(20);
                        loop {
                            self.refresh_status().await.ok();
                            match self.vpn_manager.get_status(&profile_name).await {
                                VpnStatus::Disconnected => {
                                    self.set_status_message(format!("Disconnected from {}", profile_name));
                                    self.add_log(format!("Successfully disconnected from {}", profile_name));
                                    break;
                                }
                                VpnStatus::Error(e) => {
                                    self.set_status_message(format!("Disconnect error: {}", e));
                                    self.add_log(format!("Disconnect error for {}: {}", profile_name, e));
                                    break;
                                }
                                _ => {
                                    if start.elapsed() > timeout {
                                        self.set_status_message(format!("Timeout while disconnecting {}", profile_name));
                                        self.add_log(format!("Timeout waiting for disconnection of {}", profile_name));
                                        break;
                                    }
                                    sleep(Duration::from_secs(1)).await;
                                }
                            }
                        }
                    }
                    Err(e) => {
                        self.set_status_message(format!("Failed to disconnect: {}", e));
                        self.add_log(format!("Error disconnecting from {}: {}", profile_name, e));
                    }
                }
            }
            _ => {
                // The VpnManager::connect implementation already handles disconnecting 
                // other VPNs to ensure single connection.
                let max_retries = 2u32; // number of additional retries
                let mut attempt: u32 = 0;
                let timeout = Duration::from_secs(30);

                loop {
                    self.set_status_message(format!(
                        "Connecting to {}... (attempt {}/{})",
                        profile_name,
                        attempt + 1,
                        max_retries + 1
                    ));
                    self.add_log(format!(
                        "Connecting to {}... attempt {}/{}",
                        profile_name,
                        attempt + 1,
                        max_retries + 1
                    ));

                    let connect_res = self.vpn_manager.connect(&profile).await;

                    if let Err(e) = connect_res {
                        self.add_log(format!("Connect error for {}: {}", profile_name, e));
                    }

                    // Wait for verification of the connection
                    let start = Instant::now();
                    let mut connected = false;
                    loop {
                        self.refresh_status().await.ok();
                        match self.vpn_manager.get_status(&profile_name).await {
                            VpnStatus::Connected => {
                                connected = true;
                                break;
                            }
                            VpnStatus::Error(e) => {
                                self.add_log(format!("Status error while connecting {}: {}", profile_name, e));
                                break;
                            }
                            _ => {
                                if start.elapsed() > timeout {
                                    break;
                                }
                                sleep(Duration::from_secs(1)).await;
                            }
                        }
                    }

                    if connected {
                        self.set_status_message(format!("Connected to {}", profile_name));
                        self.add_log(format!("Successfully connected to {}", profile_name));
                        break;
                    }

                    if attempt >= max_retries {
                        self.set_status_message(format!("Failed to connect to {} after {} attempts", profile_name, max_retries + 1));
                        self.add_log(format!("Failed to connect to {} after {} attempts", profile_name, max_retries + 1));
                        break;
                    }

                    attempt += 1;
                    self.add_log(format!("Retrying connection to {}...", profile_name));
                    // Small delay before retry
                    sleep(Duration::from_secs(2)).await;
                }
            }
        }
        Ok(())
    }


    fn save_new_profile(&mut self) -> Result<()> {
        let name = self.add_profile_data[0].clone();
        if name.is_empty() {
            self.set_status_message("Name cannot be empty".to_string());
            return Ok(());
        }
        
        let profile = VpnProfile {
            name,
            gateway_address: self.add_profile_data[1].clone(),
            category: self.add_profile_data[2].clone(),
            cert_path: if self.add_profile_data[3].is_empty() { None } else { Some(self.add_profile_data[3].clone()) },
            username: if self.add_profile_data[4].is_empty() { None } else { Some(self.add_profile_data[4].clone()) },
            aliases: if self.add_profile_data[5].is_empty() { None } else { Some(self.add_profile_data[5].clone()) },
            protocol: "IKEv2".to_string(),
            auto_connect: false,
        };

        let is_edit = self.screen == Screen::EditProfile;
        if is_edit {
            let indices = self.get_filtered_profiles_indices();
            if self.selected_profile < indices.len() {
                let actual_index = indices[self.selected_profile];
                self.config.profiles[actual_index] = profile;
            }
        } else {
            self.config.profiles.push(profile);
        }

        self.config.save()?;
        self.screen = Screen::Main;
        self.input_mode = InputMode::Normal;
        self.set_status_message(if is_edit { "Profile updated" } else { "Profile added" }.to_string());
        Ok(())
    }

    fn delete_selected_profile(&mut self) -> Result<()> {
        let indices = self.get_filtered_profiles_indices();
        if !indices.is_empty() && self.selected_profile < indices.len() {
            let actual_index = indices[self.selected_profile];
            let profile_name = self.config.profiles[actual_index].name.clone();
            self.config.profiles.remove(actual_index);
            self.config.save()?;

            // Reset selection if needed
            let new_len = self.get_filtered_profiles_indices().len();
            if self.selected_profile >= new_len && self.selected_profile > 0 {
                self.selected_profile = new_len.saturating_sub(1);
            }
            if new_len == 0 {
                self.selected_profile = 0;
            }
            self.table_state.select(Some(self.selected_profile));

            self.set_status_message(format!("Deleted profile: {}", profile_name));
        }
        Ok(())
    }

    fn load_profile_to_edit(&mut self) {
        let indices = self.get_filtered_profiles_indices();
        if let Some(&actual_index) = indices.get(self.selected_profile)
            && let Some(profile) = self.config.profiles.get(actual_index) {
            self.add_profile_data[0] = profile.name.clone();
            self.add_profile_data[1] = profile.gateway_address.clone();
            self.add_profile_data[2] = profile.category.clone();
            self.add_profile_data[3] = profile.cert_path.clone().unwrap_or_default();
            self.add_profile_data[4] = profile.username.clone().unwrap_or_default();
            self.add_profile_data[5] = profile.aliases.clone().unwrap_or_default();
            self.input_field = 0;
        }
    }

    pub fn get_filtered_profiles_indices(&self) -> Vec<usize> {
        let mut indices: Vec<usize> = if self.search_query.is_empty() {
            (0..self.config.profiles.len()).collect()
        } else {
            let query = self.search_query.to_lowercase();
            self.config.profiles.iter().enumerate()
                .filter(|(_, p)| {
                    p.name.to_lowercase().contains(&query) || 
                    p.category.to_lowercase().contains(&query) ||
                    p.aliases.iter().any(|a| a.to_lowercase().contains(&query))
                })
                .map(|(i, _)| i)
                .collect()
        };

        // Apply sorting
        let connections = self.connections.iter()
            .map(|c| (c.profile_name.clone(), c.clone()))
            .collect::<std::collections::HashMap<_, _>>();

        indices.sort_by(|&a, &b| {
            let p_a = &self.config.profiles[a];
            let p_b = &self.config.profiles[b];
            
            let res = match self.sort_column {
                SortColumn::Name => p_a.name.to_lowercase().cmp(&p_b.name.to_lowercase()),
                SortColumn::Category => p_a.category.to_lowercase().cmp(&p_b.category.to_lowercase()),
                SortColumn::Status => {
                    let s_a = connections.get(&p_a.name).map(|c| c.status.as_str()).unwrap_or("Disconnected");
                    let s_b = connections.get(&p_b.name).map(|c| c.status.as_str()).unwrap_or("Disconnected");
                    s_a.cmp(s_b)
                }
            };
            
            if self.sort_direction == SortDirection::Asc {
                res
            } else {
                res.reverse()
            }
        });

        indices
    }

    fn cycle_sort(&mut self) {
        match self.sort_column {
            SortColumn::Name => {
                if self.sort_direction == SortDirection::Asc {
                    self.sort_direction = SortDirection::Desc;
                } else {
                    self.sort_column = SortColumn::Category;
                    self.sort_direction = SortDirection::Asc;
                }
            }
            SortColumn::Category => {
                if self.sort_direction == SortDirection::Asc {
                    self.sort_direction = SortDirection::Desc;
                } else {
                    self.sort_column = SortColumn::Status;
                    self.sort_direction = SortDirection::Asc;
                }
            }
            SortColumn::Status => {
                if self.sort_direction == SortDirection::Asc {
                    self.sort_direction = SortDirection::Desc;
                } else {
                    self.sort_column = SortColumn::Name;
                    self.sort_direction = SortDirection::Asc;
                }
            }
        }
        self.set_status_message(format!("Sorting by {:?} ({:?})", self.sort_column, self.sort_direction));
    }

    async fn refresh_status(&mut self) -> Result<()> {
        // self.add_log("Refreshing VPN status...".to_string());
        self.vpn_manager.refresh_all_status(&self.config.profiles).await?;
        self.connections = self.vpn_manager.get_all_connections().await;
        // self.set_status_message("Status refreshed".to_string());
        Ok(())
    }

    pub async fn update(&mut self) -> Result<()> {
        // Periodic status update
        let now = std::time::Instant::now();
        if now.duration_since(self.last_update).as_secs() >= 5 {
            let _ = self.refresh_status().await;
            self.last_update = now;
        }
        Ok(())
    }

    fn set_status_message(&mut self, msg: String) {
        self.status_message = Some((msg, chrono::Local::now()));
    }

    pub fn add_log(&mut self, msg: String) {
        let timestamp = chrono::Local::now().format("%H:%M:%S");
        self.logs.push(format!("[{}] {}", timestamp, msg));
        if self.logs.len() > 100 {
            self.logs.remove(0);
        }
    }

    pub fn get_connections(&self) -> Vec<VpnConnection> {
        self.connections.clone()
    }
}
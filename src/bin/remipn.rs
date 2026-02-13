use anyhow::{anyhow, Result};
use clap::{Parser, Subcommand};
use colored::*;
use comfy_table::Table;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    Terminal,
};
use std::io;
use std::time::Duration;
use tokio::sync::mpsc;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use remipn::{App};
use remipn::app::AppEvent;
use remipn::config::Config;
use remipn::vpn::VpnManager;

#[derive(Debug, Parser)]
#[command(name = "remipn", version, about = "Remi VPN Manager", disable_help_subcommand = false)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
enum Commands {
    Connect { name: String },
    Disconnect { name: Option<String> },
    Status { name: Option<String> },
    List,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "remipn=info".into()),
        )
        .with(tracing_subscriber::fmt::layer().with_writer(io::stderr))
        .init();

    let cli = Cli::parse();

    match cli.command {
        None => run_tui().await,
        Some(Commands::List) => cmd_list().await,
        Some(Commands::Status { name }) => cmd_status(name).await,
        Some(Commands::Disconnect { name }) => cmd_disconnect(name).await,
        Some(Commands::Connect { name }) => cmd_connect(name).await,
    }
}

async fn run_tui() -> Result<()> {
    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Create an app and run
    let mut app = App::new().await?;
    let res = run_app(&mut terminal, &mut app).await;

    // Restore terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    if let Err(err) = res {
        eprintln!("Error: {err:?}");
    }

    Ok(())
}

async fn run_app<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
) -> Result<()> {
    let (tx, mut rx) = mpsc::channel(100);

    // Auto-import profiles at startup
    if let Ok(imported) = app.config.auto_import_profiles()
        && imported {
        app.add_log("Automatically imported new profiles from ~/.config/remipn/imports/".to_string());
    }

    // Input thread
    let tx_input = tx.clone();
    tokio::spawn(async move {
        loop {
            if event::poll(Duration::from_millis(100)).unwrap_or(false)
                && let Event::Key(key) = event::read().unwrap()
                && key.kind == KeyEventKind::Press
                && tx_input.send(AppEvent::Input(key)).await.is_err() {
                break;
            }
        }
    });

    // Tick thread
    let tx_tick = tx.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(200)).await;
            if tx_tick.send(AppEvent::Tick).await.is_err() {
                break;
            }
        }
    });

    // Main event loop
    loop {
        terminal.draw(|f| remipn::ui::draw(f, app))?;

        if let Some(event) = rx.recv().await {
            match event {
                AppEvent::Input(key) => {
                    if key.code == KeyCode::Char('c') && key.modifiers.contains(event::KeyModifiers::CONTROL) {
                        return Ok(());
                    }
                    if let Some(()) = app.handle_event(AppEvent::Input(key)).await? {
                        return Ok(());
                    }
                }
                _ => {
                    if let Some(()) = app.handle_event(event).await? {
                        return Ok(());
                    }
                }
            }
        }
    }
}

async fn cmd_list() -> Result<()> {
    let cfg = Config::load()?;
    let mut table = Table::new();
    table.set_header(vec!["Profile", "Alias", "Category"]);

    for p in cfg.profiles {
        table.add_row(vec![
            p.name.bold().to_string(),
            p.aliases.unwrap_or_else(|| "-".to_string()),
            p.category,
        ]);
    }
    println!("{table}");
    Ok(())
}

async fn cmd_status(name: Option<String>) -> Result<()> {
    let cfg = Config::load()?;
    let mgr = VpnManager::new();
    mgr.refresh_all_status(&cfg.profiles).await?;

    match name {
        Some(n) => {
            let target = resolve_profile(&cfg.profiles, &n)
                .map(|p| p.name.clone())
                .unwrap_or(n);
            let status = mgr.get_status(&target).await;
            
            // Find profile for extra info
            let profile = cfg.profiles.iter().find(|p| p.name == target);
            let category = profile.map(|p| p.category.as_str()).unwrap_or("-");
            
            // Find connection for IP
            let connections = mgr.get_all_connections().await;
            let ip = connections.iter()
                .find(|c| c.profile_name == target)
                .and_then(|c| c.ip_address.clone())
                .unwrap_or_else(|| "-".to_string());

            let status_str = format_status_cli(&status);
            
            println!("{} {} | IP: {} | Cat: {}", "Profile:".bold(), target.bold().cyan(), ip.green(), category.dimmed());
            println!("{} {}", "Status:".bold(), status_str);
        }
        None => {
            let connections = mgr.get_all_connections().await;
            let connected_vpns: Vec<_> = connections.iter()
                .filter(|c| matches!(c.status, remipn::vpn::VpnStatus::Connected))
                .collect();

            if connected_vpns.is_empty() {
                println!("{}", "No VPN connected.".yellow());
            } else {
                for c in connected_vpns {
                    let profile = cfg.profiles.iter().find(|p| p.name == c.profile_name);
                    let category = profile.map(|p| p.category.as_str()).unwrap_or("-");
                    let status_str = format_status_cli(&c.status);
                    
                    println!("{} {} | IP: {} | Cat: {}", "Profile:".bold(), c.profile_name.bold().cyan(), c.ip_address.as_deref().unwrap_or("-").green(), category.dimmed());
                    println!("{} {}", "Status:".bold(), status_str);
                    println!("{}", "-".repeat(40).dimmed());
                }
            }
        }
    }
    
    Ok(())
}

fn format_status_cli(status: &remipn::vpn::VpnStatus) -> String {
    use remipn::vpn::VpnStatus;
    match status {
        VpnStatus::Connected => "Connected".green().bold().to_string(),
        VpnStatus::Connecting => "Connecting...".yellow().to_string(),
        VpnStatus::Disconnected => "Disconnected".white().dimmed().to_string(),
        VpnStatus::Disconnecting => "Disconnecting...".yellow().to_string(),
        VpnStatus::Error(e) => format!("Error: {}", e).red().to_string(),
    }
}

async fn cmd_disconnect(name: Option<String>) -> Result<()> {
    let cfg = Config::load()?;
    let mgr = VpnManager::new();

    match name {
        Some(n) => {
            let target = resolve_profile(&cfg.profiles, &n)
                .map(|p| p.name.clone())
                .unwrap_or(n);
            if let Err(e) = mgr.disconnect(&target).await {
                return Err(anyhow!("Disconnection failed for '{}': {}", target, e));
            }
            println!("Disconnected from {}", target);
        }
        None => {
            for p in &cfg.profiles {
                if let Err(e) = mgr.disconnect(&p.name).await {
                    eprintln!("Error while trying to disconnect from {}: {}", p.name, e);
                }
            }
            println!("All connections disconnected.");
        }
    }
    Ok(())
}

async fn cmd_connect(name: String) -> Result<()> {
    let cfg = Config::load()?;
    let mgr = VpnManager::new();

    let profiles = cfg.profiles.clone();
    let profile = resolve_profile(&profiles, &name)
        .cloned()
        .ok_or_else(|| anyhow!("Profile '{}' not found", name))?;

    for p in profiles.iter().filter(|p| p.name != profile.name) {
        let _ = mgr.disconnect(&p.name).await;
    }

    if let Err(e) = mgr.connect(&profile).await {
        return Err(anyhow!("Connection to '{}' failed: {}", profile.name, e));
    }

    println!("Connected to {}", profile.name);
    Ok(())
}


fn resolve_profile<'a>(profiles: &'a [remipn::config::VpnProfile], key: &str) -> Option<&'a remipn::config::VpnProfile> {
    profiles.iter().find(|p| p.name == key || p.aliases.iter().any(|a| a == key))
}

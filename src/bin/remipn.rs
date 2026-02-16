use anyhow::{Result, anyhow};
use clap::{Parser, Subcommand};
use colored::*;
use comfy_table::Table;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};
use std::io;
use std::time::Duration;
use tokio::sync::mpsc;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use remipn::App;
use remipn::app::AppEvent;
use remipn::config::Config;
use remipn::vpn::VpnManager;

#[derive(Debug, Parser)]
#[command(
    name = "remipn",
    version,
    about = "Remi VPN Manager",
    disable_help_subcommand = false
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
enum Commands {
    #[command(visible_alias = "c")]
    Connect { name: String },
    #[command(visible_alias = "d")]
    Disconnect { name: Option<String> },
    #[command(visible_alias = "s")]
    Status { name: Option<String> },
    #[command(visible_alias = "l")]
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
    let (tx, rx) = mpsc::channel(100);

    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Create an app and run
    let mut app = App::new().await?;
    app.event_tx = Some(tx.clone());
    let res = run_app(&mut terminal, &mut app, rx).await;

    // Restore terminal
    let _ = execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    );
    let _ = terminal.show_cursor();
    let _ = disable_raw_mode();

    if let Err(err) = res {
        eprintln!("Error: {err:?}");
    }

    Ok(())
}

async fn run_app<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    mut rx: mpsc::Receiver<AppEvent>,
) -> Result<()> {
    let tx = app.event_tx.clone().unwrap();

    // Auto-import profiles at startup
    if let Ok(imported) = app.config.auto_import_profiles()
        && imported
    {
        app.add_log("Automatically imported new profiles".to_string());
    }

    // Input thread
    let tx_input = tx.clone();
    tokio::spawn(async move {
        loop {
            if tx_input.is_closed() {
                break;
            }
            if event::poll(Duration::from_millis(100)).unwrap_or(false)
                && let Event::Key(key) = event::read().unwrap()
                && key.kind == KeyEventKind::Press
                && tx_input.send(AppEvent::Input(key)).await.is_err()
            {
                break;
            }
        }
    });

    // Tick thread
    let tx_tick = tx.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(50)).await;
            if tx_tick.send(AppEvent::Tick).await.is_err() {
                break;
            }
        }
    });

    // Main event loop
    loop {
        terminal.draw(|f| remipn::ui::draw(f, app))?;

        match rx.recv().await {
            Some(event) => match event {
                AppEvent::Input(key) => {
                    if key.code == KeyCode::Char('c')
                        && key.modifiers.contains(event::KeyModifiers::CONTROL)
                    {
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
            },
            None => break,
        }
    }
    Ok(())
}

async fn cmd_list() -> Result<()> {
    let cfg = Config::load()?;
    let mgr = VpnManager::new();
    mgr.refresh_all_status(&cfg.profiles).await?;
    let connections = mgr.get_all_connections().await;
    let connection_map: std::collections::HashMap<_, _> = connections
        .iter()
        .map(|c| (c.profile_name.clone(), c.clone()))
        .collect();

    let mut table = Table::new();
    table.set_header(vec![
        "Profile", "Alias", "Category", "Status", "IP", "Since",
    ]);

    for p in cfg.profiles {
        let conn = connection_map.get(&p.name);
        let status = conn
            .map(|c| c.status.clone())
            .unwrap_or(remipn::vpn::VpnStatus::Disconnected);
        let status_str = format_status_cli(&status);

        let ip = conn
            .and_then(|c| c.ip_address.clone())
            .unwrap_or_else(|| "-".to_string());

        let since = conn
            .and_then(|c| c.connected_since)
            .map(|t| {
                let duration = chrono::Local::now().signed_duration_since(t);
                format!("{}m", duration.num_minutes())
            })
            .unwrap_or_else(|| "-".to_string());

        table.add_row(vec![
            p.name.bold().to_string(),
            p.aliases.unwrap_or_else(|| "-".to_string()),
            p.category,
            status_str,
            ip,
            since,
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
            let ip = connections
                .iter()
                .find(|c| c.profile_name == target)
                .and_then(|c| c.ip_address.clone())
                .unwrap_or_else(|| "-".to_string());

            let status_str = format_status_cli(&status);

            println!(
                "{} {} | IP: {} | Cat: {}",
                "Profile:".bold(),
                target.bold().cyan(),
                ip.green(),
                category.dimmed()
            );
            println!("{} {}", "Status:".bold(), status_str);
        }
        None => {
            let connections = mgr.get_all_connections().await;
            let connected_vpns: Vec<_> = connections
                .iter()
                .filter(|c| matches!(c.status, remipn::vpn::VpnStatus::Connected))
                .collect();

            if connected_vpns.is_empty() {
                println!("{}", "No VPN connected.".yellow());
            } else {
                for c in connected_vpns {
                    let profile = cfg.profiles.iter().find(|p| p.name == c.profile_name);
                    let category = profile.map(|p| p.category.as_str()).unwrap_or("-");
                    let status_str = format_status_cli(&c.status);

                    println!(
                        "{} {} | IP: {} | Cat: {}",
                        "Profile:".bold(),
                        c.profile_name.bold().cyan(),
                        c.ip_address.as_deref().unwrap_or("-").green(),
                        category.dimmed()
                    );
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
        VpnStatus::Retrying(a, m) => format!("Retry {}/{}...", a, m).yellow().to_string(),
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

    let profile_name = profile.name.clone();

    let max_retries = 2u32;
    let mut attempt = 0u32;
    let timeout = Duration::from_secs(10);

    loop {
        println!(
            "Connecting to {}... (attempt {}/{})",
            profile_name.bold().cyan(),
            attempt + 1,
            max_retries + 1
        );

        // Check for other active VPNs and inform user
        if let Ok(active) = mgr.get_active_vpns().await {
            for (name, _) in active {
                if name != profile_name {
                    println!(
                        "{} Closing previous VPN: {}...",
                        " i ".on_blue(),
                        name.yellow()
                    );
                }
            }
        }

        // Connection is handled by vpn_manager.connect, but we wrap it in retries
        let connect_res = mgr.connect(&profile).await;
        if let Err(ref e) = connect_res {
            eprintln!("{} Error: {}", " ! ".on_red(), e);
        }

        let start = std::time::Instant::now();
        let mut connected = false;
        loop {
            match mgr.get_status(&profile_name).await {
                remipn::vpn::VpnStatus::Connected => {
                    connected = true;
                    break;
                }
                remipn::vpn::VpnStatus::Error(e) => {
                    eprintln!("{} Status error: {}", " ! ".on_red(), e);
                    break;
                }
                _ => {
                    if start.elapsed() > timeout {
                        eprintln!("{} Timeout waiting for connection", " ! ".on_yellow());
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
            }
        }

        if connected {
            print!("Verifying connection stability...");
            use std::io::Write;
            std::io::stdout().flush().unwrap();

            let mut stable = true;
            for _ in 0..15 {
                tokio::time::sleep(Duration::from_millis(200)).await;
                if !matches!(
                    mgr.get_status(&profile_name).await,
                    remipn::vpn::VpnStatus::Connected
                ) {
                    stable = false;
                    break;
                }

                // ensure no other VPN is active
                if let Ok(active) = mgr.get_active_vpns().await
                    && active.iter().any(|(name, _)| name != &profile_name)
                {
                    for (name, _) in active {
                        if name != profile_name {
                            let _ = mgr.disconnect(&name).await;
                        }
                    }
                }

                print!(".");
                std::io::stdout().flush().unwrap();
            }
            println!();

            if stable {
                println!(
                    "{} Successfully connected to {}",
                    " ✓ ".on_green(),
                    profile_name.bold().green()
                );
                return Ok(());
            } else {
                eprintln!(
                    "{} Connection to {} dropped during stabilization",
                    " ! ".on_yellow(),
                    profile_name
                );
            }
        }

        if attempt >= max_retries {
            return Err(anyhow!(
                "Failed to connect to {} after {} attempts",
                profile_name,
                max_retries + 1
            ));
        }

        attempt += 1;
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

fn resolve_profile<'a>(
    profiles: &'a [remipn::config::VpnProfile],
    key: &str,
) -> Option<&'a remipn::config::VpnProfile> {
    profiles
        .iter()
        .find(|p| p.name == key || p.aliases.iter().any(|a| a == key))
}

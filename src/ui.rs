use crate::app::{App, Screen};
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, List, ListItem, Paragraph, Row, Table, Wrap},
};

pub fn draw(f: &mut Frame, app: &App) {
    match app.screen {
        Screen::Main => draw_main_screen(f, app),
        Screen::AddProfile => draw_add_profile_screen(f, app),
        Screen::EditProfile => draw_edit_profile_screen(f, app),
        Screen::ImportXml => draw_import_xml_screen(f, app),
        Screen::FileBrowser => draw_file_browser_screen(f, app),
        Screen::Help => draw_help_screen(f),
        Screen::DeleteConfirmation => draw_delete_confirmation(f, app),
        Screen::Search => draw_main_screen(f, app), // Search is rendered as part of the main or overlay
        Screen::AliasModal => draw_main_screen(f, app),
    }
}

fn draw_main_screen(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Title
            Constraint::Min(10),   // Main content
            Constraint::Length(3), // Status bar
        ])
        .split(f.size());

    // Title
    let title = Paragraph::new("RemiPN")
        .style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
        .alignment(Alignment::Center)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(ratatui::widgets::BorderType::Rounded),
        );
    f.render_widget(title, chunks[0]);

    // Main content area
    let main_chunks = if app.show_logs {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
            .split(chunks[1])
    } else {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(100)])
            .split(chunks[1])
    };

    // VPN Profiles list
    draw_vpn_list(f, app, main_chunks[0]);

    // Logs panel (if enabled)
    if app.show_logs && main_chunks.len() > 1 {
        draw_logs_panel(f, app, main_chunks[1]);
    }

    // Status bar
    draw_status_bar(f, app, chunks[2]);

    // Search overlay
    if app.screen == Screen::Search {
        draw_search_bar(f, app);
    }

    // Alias overlay
    if app.screen == Screen::AliasModal {
        draw_alias_modal(f, app);
    }
}

fn draw_vpn_list(f: &mut Frame, app: &App, area: Rect) {
    let connections = app.get_connections();
    let connection_map: std::collections::HashMap<_, _> = connections
        .iter()
        .map(|c| (c.profile_name.clone(), c.clone()))
        .collect();

    let filtered_indices = app.get_filtered_profiles_indices();
    let rows: Vec<Row> = filtered_indices
        .iter()
        .map(|&idx| {
            let profile = &app.config.profiles[idx];
            let conn = connection_map.get(&profile.name);
            let status = conn
                .map(|c| c.status.clone())
                .unwrap_or(crate::vpn::VpnStatus::Disconnected);

            let status_color = status.color();
            let status_text = status.as_str();

            let connected_time = conn
                .and_then(|c| c.connected_since)
                .map(|t| {
                    let duration = chrono::Local::now().signed_duration_since(t);
                    format!("{}m", duration.num_minutes())
                })
                .unwrap_or_else(|| "-".to_string());

            let ip_addr = conn
                .and_then(|c| c.ip_address.clone())
                .unwrap_or_else(|| "-".to_string());

            let alias = profile.aliases.clone().unwrap_or_else(|| "-".to_string());

            Row::new(vec![
                Cell::from(profile.name.clone()),
                Cell::from(alias),
                Cell::from(profile.category.clone()),
                Cell::from(Span::styled(status_text, Style::default().fg(status_color))),
                Cell::from(connected_time),
                Cell::from(ip_addr),
            ])
        })
        .collect();
    let header_name = format!(
        "Profile {}",
        if app.sort_column == crate::app::SortColumn::Name {
            if app.sort_direction == crate::app::SortDirection::Asc {
                "▲"
            } else {
                "▼"
            }
        } else {
            ""
        }
    );
    let header_category = format!(
        "Category {}",
        if app.sort_column == crate::app::SortColumn::Category {
            if app.sort_direction == crate::app::SortDirection::Asc {
                "▲"
            } else {
                "▼"
            }
        } else {
            ""
        }
    );
    let header_status = format!(
        "Status {}",
        if app.sort_column == crate::app::SortColumn::Status {
            if app.sort_direction == crate::app::SortDirection::Asc {
                "▲"
            } else {
                "▼"
            }
        } else {
            ""
        }
    );

    let table = Table::new(
        rows,
        [
            Constraint::Min(25),    // Profile Name
            Constraint::Length(15), // Alias
            Constraint::Length(15), // Category
            Constraint::Length(15), // Status
            Constraint::Length(10), // Duration
            Constraint::Min(20),    // IP Address
        ],
    )
    .header(
        Row::new(vec![
            header_name,
            "Alias".to_string(),
            header_category,
            header_status,
            "Duration".to_string(),
            "IP Address".to_string(),
        ])
        .style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
        .bottom_margin(1),
    )
    .highlight_style(
        Style::default()
            .bg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    )
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(ratatui::widgets::BorderType::Rounded)
            .title(" VPN Connections (↑/↓: select, Enter: toggle, /: search, s: sort, i: import) "),
    )
    .column_spacing(1);

    f.render_stateful_widget(table, area, &mut app.table_state.clone());
}

fn draw_logs_panel(f: &mut Frame, app: &App, area: Rect) {
    let logs: Vec<ListItem> = app
        .logs
        .iter()
        .rev()
        .take(area.height as usize - 2)
        .map(|log| {
            let style = if log.contains("Error") || log.contains("✗") {
                Style::default().fg(Color::Red)
            } else if log.contains("✓") {
                Style::default().fg(Color::Green)
            } else {
                Style::default().fg(Color::Gray)
            };
            ListItem::new(log.as_str()).style(style)
        })
        .collect();

    let logs_list = List::new(logs).block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(ratatui::widgets::BorderType::Rounded)
            .title(" Logs (l: toggle) "),
    );

    f.render_widget(logs_list, area);
}

fn draw_status_bar(f: &mut Frame, app: &App, area: Rect) {
    let now = chrono::Local::now();
    let status_text = if let Some((msg, timestamp)) = &app.status_message {
        let age = now.signed_duration_since(*timestamp).num_seconds();
        if age < 10 {
            msg.clone()
        } else {
            "Ready".to_string()
        }
    } else {
        "Ready".to_string()
    };

    let connected_count = app
        .get_connections()
        .iter()
        .filter(|c| matches!(c.status, crate::vpn::VpnStatus::Connected))
        .count();
    let total_count = app.config.profiles.len();

    let auto_reconnect = if app.auto_reconnect { "ON" } else { "OFF" };

    let status_line = format!(
        " {} | Connected: {}/{} | Auto-Reconnect: {} | s: sort, q: quit, h: help ",
        status_text, connected_count, total_count, auto_reconnect
    );

    let status = Paragraph::new(status_line)
        .style(Style::default().fg(Color::White))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(ratatui::widgets::BorderType::Rounded),
        );

    f.render_widget(status, area);
}

fn draw_add_profile_screen(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(4)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Min(3),
        ])
        .split(f.size());

    let title_text = if app.screen == Screen::EditProfile {
        "Edit VPN Profile"
    } else {
        "Add New VPN Profile"
    };
    let title = Paragraph::new(title_text)
        .style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(title, chunks[0]);

    let fields = [
        ("Profile Name", 0),
        ("Gateway Address", 1),
        ("Category (e.g. dev, uat, prod)", 2),
        ("Certificate Path (optional)", 3),
        ("Username (optional)", 4),
        ("Aliases (comma-separated)", 5),
    ];

    for (i, (label, field_idx)) in fields.iter().enumerate() {
        let is_selected = app.input_field == *field_idx;
        let is_edit = app.screen == Screen::EditProfile;
        let is_name_field = *field_idx == 0;

        let style = if is_selected {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else if is_edit && is_name_field {
            Style::default().fg(Color::DarkGray)
        } else {
            Style::default()
        };

        let value = &app.add_profile_data[*field_idx];
        let cursor = if is_selected { "_" } else { "" };
        let input = if is_edit && is_name_field {
            format!("{}: {} (static)", label, value)
        } else {
            format!("{}: {}{}", label, value, cursor)
        };

        let para = Paragraph::new(input)
            .style(style)
            .block(Block::default().borders(Borders::ALL));

        f.render_widget(para, chunks[i + 1]);
    }

    let help =
        Paragraph::new("Tab: next field | Shift+Tab: prev field | Enter: save | Esc: cancel")
            .style(Style::default().fg(Color::Gray))
            .alignment(Alignment::Center);
    f.render_widget(help, chunks[7]);
}

fn draw_edit_profile_screen(f: &mut Frame, app: &App) {
    draw_add_profile_screen(f, app);
}

fn draw_import_xml_screen(f: &mut Frame, app: &App) {
    let area = centered_rect(60, 20, f.size());
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Title
            Constraint::Length(3), // Input
            Constraint::Length(1), // Help
        ])
        .split(area);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Import VPN from Microsoft XML Dump ");
    f.render_widget(block, area);

    let title = Paragraph::new("Enter the full path to the XML file or press 'f' to browse:")
        .alignment(Alignment::Center);
    f.render_widget(title, chunks[0]);

    let input = Paragraph::new(format!("{}_", app.input_buffer))
        .style(Style::default().fg(Color::Yellow))
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(input, chunks[1]);

    let help = Paragraph::new("Enter: Import | Esc: Cancel | f: File Browser")
        .style(Style::default().fg(Color::Gray))
        .alignment(Alignment::Center);
    f.render_widget(help, chunks[2]);
}

fn draw_file_browser_screen(f: &mut Frame, app: &App) {
    let area = centered_rect(80, 80, f.size());
    let browser = match &app.file_browser {
        Some(b) => b,
        None => return,
    };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Title/Path
            Constraint::Min(10),   // List
            Constraint::Length(3), // Help
        ])
        .split(area);

    let path_para = Paragraph::new(format!(" Path: {}", browser.current_dir.display()))
        .style(Style::default().fg(Color::Cyan))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" File Browser "),
        );
    f.render_widget(path_para, chunks[0]);

    let items: Vec<ListItem> = browser
        .entries
        .iter()
        .map(|entry| {
            let prefix = if entry.is_dir { "[DIR] " } else { "      " };
            ListItem::new(format!("{}{}", prefix, entry.name))
        })
        .collect();

    let list = List::new(items)
        .block(Block::default().borders(Borders::LEFT | Borders::RIGHT))
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        );

    f.render_stateful_widget(list, chunks[1], &mut browser.state.clone());

    let help = Paragraph::new(" ↑/↓: Select | Enter: Open/Select | Backspace: Up | Esc: Cancel ")
        .style(Style::default().fg(Color::Gray))
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(help, chunks[2]);
}

fn draw_help_screen(f: &mut Frame) {
    let help_text = vec![
        Line::from(vec![Span::styled(
            "RemiPN - Help",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )]),
        Line::from(""),
        Line::from(vec![Span::styled(
            "Navigation:",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )]),
        Line::from("  ↑/k         - Move selection up"),
        Line::from("  ↓/j         - Move selection down"),
        Line::from("  PgUp        - Page up (10 items)"),
        Line::from("  PgDn        - Page down (10 items)"),
        Line::from("  s           - Cycle sort column/direction"),
        Line::from(""),
        Line::from(vec![Span::styled(
            "Actions:",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )]),
        Line::from("  Enter/Space - Connect/Disconnect selected VPN"),
        Line::from("  r           - Refresh VPN status"),
        Line::from("  R           - Toggle auto-reconnect"),
        Line::from(""),
        Line::from(vec![Span::styled(
            "Profile Management:",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )]),
        Line::from("  n           - Add new profile"),
        Line::from("  e           - Edit selected profile"),
        Line::from("  a           - Quick alias edit"),
        Line::from("  x           - Delete selected profile"),
        Line::from("  /           - Search profiles"),
        Line::from("  i           - Import profiles from XML"),
        Line::from("  I           - Auto-import from standard locations"),
        Line::from(""),
        Line::from(vec![Span::styled(
            "View:",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )]),
        Line::from("  l           - Toggle logs panel"),
        Line::from("  h/F1        - Show this help"),
        Line::from(""),
        Line::from(vec![Span::styled(
            "Exit:",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )]),
        Line::from("  q           - Quit application"),
        Line::from("  Ctrl+C      - Force quit"),
        Line::from(""),
        Line::from(vec![Span::styled(
            "Press Esc or h to return",
            Style::default().fg(Color::Gray),
        )]),
    ];

    let help_para = Paragraph::new(help_text)
        .alignment(Alignment::Left)
        .wrap(Wrap { trim: true })
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Help ")
                .style(Style::default().fg(Color::White)),
        );

    let area = centered_rect(60, 80, f.size());
    f.render_widget(help_para, area);
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

fn draw_delete_confirmation(f: &mut Frame, app: &App) {
    let indices = app.get_filtered_profiles_indices();
    let profile_name = if let Some(&idx) = indices.get(app.selected_profile) {
        app.config.profiles[idx].name.clone()
    } else {
        "None".to_string()
    };

    let area = centered_rect(40, 20, f.size());
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Confirm Deletion ")
        .border_style(Style::default().fg(Color::Red));

    let text = vec![
        Line::from(""),
        Line::from(vec![
            Span::raw("Are you sure you want to delete "),
            Span::styled(profile_name, Style::default().add_modifier(Modifier::BOLD)),
            Span::raw("?"),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                "y",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(": Yes, "),
            Span::styled(
                "n",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
            Span::raw(": No"),
        ]),
    ];

    let para = Paragraph::new(text)
        .alignment(Alignment::Center)
        .block(block);

    f.render_widget(ratatui::widgets::Clear, area); // Clear the area before rendering the popup
    f.render_widget(para, area);
}

fn draw_search_bar(f: &mut Frame, app: &App) {
    let area = centered_rect(50, 15, f.size());
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Search (Name or Category) ")
        .border_style(Style::default().fg(Color::Yellow));

    let input = Paragraph::new(format!("/{}", app.search_query))
        .block(block)
        .style(Style::default().fg(Color::Yellow));

    f.render_widget(ratatui::widgets::Clear, area);
    f.render_widget(input, area);
}

fn draw_alias_modal(f: &mut Frame, app: &App) {
    let indices = app.get_filtered_profiles_indices();
    let profile_name = if let Some(&idx) = indices.get(app.selected_profile) {
        app.config.profiles[idx].name.clone()
    } else {
        "None".to_string()
    };

    let area = centered_rect(40, 20, f.size());
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" Alias for {} ", profile_name))
        .border_style(Style::default().fg(Color::Cyan));

    let input = Paragraph::new(app.alias_input.clone())
        .block(block)
        .style(Style::default().fg(Color::Cyan));

    f.render_widget(ratatui::widgets::Clear, area);
    f.render_widget(input, area);

    // Help text at bottom of modal
    let help_area = Rect {
        x: area.x,
        y: area.y + area.height - 1,
        width: area.width,
        height: 1,
    };
    let help_text = Paragraph::new(" [Enter] Save  [Esc] Cancel ")
        .alignment(Alignment::Center)
        .style(Style::default().fg(Color::DarkGray));
    f.render_widget(help_text, help_area);
}

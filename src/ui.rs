use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, Borders, Cell, Clear, Gauge, List, ListItem, Paragraph, Row, Sparkline, Table, Wrap,
    },
};

use crate::app::{App, ConfigEntry, Focus};

pub fn draw(f: &mut Frame, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(1)])
        .split(f.area());

    let main_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(20), // Groups
            Constraint::Percentage(40), // Proxies
            Constraint::Percentage(40), // Overview
        ])
        .split(chunks[0]);

    draw_groups(f, app, main_chunks[0]);
    draw_proxies(f, app, main_chunks[1]);
    draw_overview(f, app, main_chunks[2]);
    draw_status_bar(f, app, chunks[1]);

    if let Focus::Settings = app.focus {
        draw_settings(f, app);
    }

    if app.show_info_popup {
        draw_info_popup(f, app);
    }

    if app.is_editing {
        draw_input_popup(f, app);
    }
}

fn draw_groups(f: &mut Frame, app: &mut App, area: Rect) {
    let items: Vec<ListItem> = app
        .group_names
        .iter()
        .map(|name| ListItem::new(Line::from(name.as_str())))
        .collect();

    let title = "Groups";
    let border_color = if let Focus::Groups = app.focus {
        Color::Yellow
    } else {
        Color::White
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::default().fg(border_color));

    let list = List::new(items)
        .block(block)
        .highlight_style(
            Style::default()
                .add_modifier(Modifier::BOLD)
                .fg(Color::Cyan),
        )
        .highlight_symbol("> ");

    f.render_stateful_widget(list, area, &mut app.group_state);
}

fn draw_proxies(f: &mut Frame, app: &mut App, area: Rect) {
    let border_color = if let Focus::Proxies = app.focus {
        Color::Yellow
    } else {
        Color::White
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .title("Proxies")
        .border_style(Style::default().fg(border_color));

    if app.group_names.is_empty() {
        f.render_widget(Paragraph::new("No groups found").block(block), area);
        return;
    }

    let group_idx = app.group_state.selected().unwrap_or(0);
    let group_name_opt = app.group_names.get(group_idx).cloned();

    if let Some(group_name) = group_name_opt {
        if let Some(group) = app.proxies.get(&group_name) {
            if let Some(all) = &group.all {
                let rows: Vec<Row> = all
                    .iter()
                    .map(|name| {
                        let mut style = Style::default();
                        if let Some(now) = &group.now
                            && now == name
                        {
                            style = style.fg(Color::Green);
                        }

                        // Latency
                        let latency = app.proxy_latency.get(name).copied().flatten();
                        let (lat_str, lat_style) = if let Some(ms) = latency {
                            let s = format!("{} ms", ms);
                            let c = if ms < 200 {
                                Color::Green
                            } else if ms < 500 {
                                Color::Yellow
                            } else {
                                Color::Red
                            };
                            (s, Style::default().fg(c))
                        } else {
                            ("-".to_string(), Style::default().fg(Color::Gray))
                        };

                        Row::new(vec![
                            Cell::from(name.as_str()).style(style),
                            Cell::from(lat_str).style(lat_style),
                        ])
                    })
                    .collect();

                let table = Table::new(rows, [Constraint::Percentage(70), Constraint::Length(10)])
                    .block(block)
                    .row_highlight_style(
                        Style::default()
                            .add_modifier(Modifier::BOLD)
                            .bg(Color::DarkGray),
                    )
                    .highlight_symbol(">> ");

                f.render_stateful_widget(table, area, &mut app.proxy_state);
            } else {
                f.render_widget(
                    Paragraph::new("No proxies in this group").block(block),
                    area,
                );
            }
        } else {
            f.render_widget(Paragraph::new("Group not found").block(block), area);
        }
    } else {
        f.render_widget(Paragraph::new("Select a group").block(block), area);
    }
}

fn draw_overview(f: &mut Frame, app: &App, area: Rect) {
    let block = Block::default().borders(Borders::ALL).title("Overview");

    let inner_area = block.inner(area);
    f.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(6), // Info
            Constraint::Length(3), // Google Test
            Constraint::Min(0),    // Charts
        ])
        .margin(1)
        .split(inner_area);

    // 1. Info
    let mut info_text = vec![];
    if let Some(config) = &app.config {
        info_text.push(Line::from(vec![
            Span::styled("Mode: ", Style::default().fg(Color::Blue)),
            Span::raw(&config.mode),
        ]));
        info_text.push(Line::from(vec![
            Span::styled("Mixed Port: ", Style::default().fg(Color::Blue)),
            Span::raw(config.mixed_port.to_string()),
        ]));
        info_text.push(Line::from(vec![
            Span::styled("TUN: ", Style::default().fg(Color::Blue)),
            Span::styled(
                if config.tun.enable {
                    "Enabled"
                } else {
                    "Disabled"
                },
                Style::default().fg(if config.tun.enable {
                    Color::Green
                } else {
                    Color::Red
                }),
            ),
        ]));
        if let Some(stack) = &config.tun.stack {
            info_text.push(Line::from(vec![
                Span::styled("TUN Stack: ", Style::default().fg(Color::DarkGray)),
                Span::raw(stack),
            ]));
        }
    } else {
        info_text.push(Line::from("Loading config..."));
    }

    f.render_widget(Paragraph::new(info_text), chunks[0]);

    // 2. Connection Test (Latency)
    let (latency_label, latency_color, percent) = match &app.real_latency_status {
        crate::app::RealLatencyStatus::Pending => ("Idle".to_string(), Color::Gray, 0),
        crate::app::RealLatencyStatus::Testing => ("Testing...".to_string(), Color::Yellow, 0),
        crate::app::RealLatencyStatus::Success(ms) => {
            let color = if *ms < 200 {
                Color::Green
            } else if *ms < 500 {
                Color::Yellow
            } else {
                Color::Red
            };
            (
                format!("{} ms", ms),
                color,
                (1000.0 / (*ms as f64).max(10.0) * 100.0).min(100.0) as u16,
            )
        }
        crate::app::RealLatencyStatus::Failed(msg) => (format!("Err: {}", msg), Color::Red, 100),
    };

    let gauge = Gauge::default()
        .block(Block::default().title("Test Latency").borders(Borders::ALL))
        .gauge_style(Style::default().fg(latency_color))
        .percent(percent)
        .label(latency_label);

    f.render_widget(gauge, chunks[1]);

    // 3. Charts (Sparklines)
    let chart_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(chunks[2]);

    let width = chart_chunks[0].width.saturating_sub(2) as usize;

    // Download
    let down_speed = format_speed(app.current_down);
    let down_title = format!("Download: {}/s", down_speed);
    let down_data: Vec<u64> = app
        .traffic_history_down
        .iter()
        .rev()
        .take(width)
        .rev()
        .cloned()
        .collect();
    let down_sparkline = Sparkline::default()
        .block(
            Block::default()
                .title(down_title)
                .borders(Borders::TOP | Borders::LEFT | Borders::RIGHT),
        )
        .data(&down_data)
        .style(Style::default().fg(Color::Green));
    f.render_widget(down_sparkline, chart_chunks[0]);

    // Upload
    let up_speed = format_speed(app.current_up);
    let up_title = format!("Upload: {}/s", up_speed);
    let up_data: Vec<u64> = app
        .traffic_history_up
        .iter()
        .rev()
        .take(width)
        .rev()
        .cloned()
        .collect();
    let up_sparkline = Sparkline::default()
        .block(
            Block::default()
                .title(up_title)
                .borders(Borders::BOTTOM | Borders::LEFT | Borders::RIGHT),
        )
        .data(&up_data)
        .style(Style::default().fg(Color::Yellow));
    f.render_widget(up_sparkline, chart_chunks[1]);
}

fn format_speed(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{} B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

fn draw_settings(f: &mut Frame, app: &mut App) {
    let area = f.area();
    // Center a 70% x 50% block
    let popup_area = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(25),
            Constraint::Percentage(50),
            Constraint::Percentage(25),
        ])
        .split(area)[1];

    let popup_area = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(15),
            Constraint::Percentage(70),
            Constraint::Percentage(15),
        ])
        .split(popup_area)[1];

    f.render_widget(Clear, popup_area);

    let block = Block::default()
        .title(" Configuration ")
        .title_alignment(ratatui::layout::Alignment::Center)
        .borders(Borders::ALL)
        .border_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .style(Style::default().bg(Color::Black));

    let header_style = Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD)
        .bg(Color::DarkGray);

    let selected_style = Style::default()
        .add_modifier(Modifier::REVERSED)
        .fg(Color::LightCyan);

    let header = Row::new(vec!["Setting", "Current Value", "Action"])
        .style(header_style)
        .height(1)
        .bottom_margin(1);

    let rows: Vec<Row> = app
        .settings_items
        .iter()
        .map(|item| {
            let (label, value, action) = match item {
                ConfigEntry::BaseUrl => {
                    ("App: Base URL", app.app_settings.base_url.clone(), "Edit")
                }
                ConfigEntry::ApiSecret => (
                    "App: API Secret",
                    if app.app_settings.api_secret.is_empty() {
                        "<none>".to_string()
                    } else {
                        "******".to_string()
                    },
                    "Edit",
                ),
                ConfigEntry::TestUrl => {
                    ("App: Test URL", app.app_settings.test_url.clone(), "Edit")
                }
                ConfigEntry::TestTimeout => (
                    "App: Test Timeout (ms)",
                    app.app_settings.test_timeout.to_string(),
                    "Edit",
                ),
                ConfigEntry::Mode => {
                    let val = app
                        .config
                        .as_ref()
                        .map(|c| c.mode.as_str())
                        .unwrap_or("Unknown");
                    ("Mode", val.to_string(), "Cycle (Rule/Global/Direct)")
                }
                ConfigEntry::Tun => {
                    let val = app.config.as_ref().map(|c| c.tun.enable).unwrap_or(false);
                    (
                        "TUN Mode",
                        if val { "Enabled" } else { "Disabled" }.to_string(),
                        "Toggle",
                    )
                }
                ConfigEntry::MixedPort => {
                    let val = app.config.as_ref().map(|c| c.mixed_port).unwrap_or(0);
                    ("Mixed Port", val.to_string(), "Edit")
                }
                ConfigEntry::LogLevel => {
                    let val = app
                        .config
                        .as_ref()
                        .map(|c| c.log_level.as_str())
                        .unwrap_or("info");
                    ("Log Level", val.to_string(), "Cycle")
                }
                ConfigEntry::AllowLan => {
                    let val = app.config.as_ref().map(|c| c.allow_lan).unwrap_or(false);
                    (
                        "Allow LAN",
                        if val { "True" } else { "False" }.to_string(),
                        "Toggle",
                    )
                }
                ConfigEntry::BindAddress => {
                    let val = app
                        .config
                        .as_ref()
                        .map(|c| c.bind_address.as_str())
                        .unwrap_or("*");
                    ("Bind Address", val.to_string(), "Edit")
                }
                ConfigEntry::Ipv6 => {
                    let val = app.config.as_ref().map(|c| c.ipv6).unwrap_or(false);
                    (
                        "IPv6",
                        if val { "Enabled" } else { "Disabled" }.to_string(),
                        "Toggle",
                    )
                }
            };

            Row::new(vec![
                Cell::from(label).style(
                    Style::default()
                        .fg(Color::Blue)
                        .add_modifier(Modifier::BOLD),
                ),
                Cell::from(value).style(Style::default().fg(Color::White)),
                Cell::from(action).style(
                    Style::default()
                        .fg(Color::Gray)
                        .add_modifier(Modifier::ITALIC),
                ),
            ])
            .height(1)
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Percentage(30),
            Constraint::Percentage(30),
            Constraint::Percentage(40),
        ],
    )
    .header(header)
    .block(block)
    .row_highlight_style(selected_style)
    .highlight_symbol(">> ");

    f.render_stateful_widget(table, popup_area, &mut app.settings_state);
}

fn draw_input_popup(f: &mut Frame, app: &mut App) {
    let area = f.area();
    let popup_area = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(40),
            Constraint::Length(3), // Input box height
            Constraint::Percentage(40),
        ])
        .split(area)[1];

    let popup_area = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(30),
            Constraint::Percentage(40),
            Constraint::Percentage(30),
        ])
        .split(popup_area)[1];

    f.render_widget(Clear, popup_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .title("Edit Value (Enter to Save, Esc to Cancel)")
        .style(Style::default().bg(Color::Blue).fg(Color::White));

    let p = Paragraph::new(app.editing_value.clone()).block(block);

    f.render_widget(p, popup_area);
}

fn draw_info_popup(f: &mut Frame, app: &App) {
    let area = f.area();
    let popup_area = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(25),
            Constraint::Percentage(50),
            Constraint::Percentage(25),
        ])
        .split(area)[1];

    let popup_area = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(25),
            Constraint::Percentage(50),
            Constraint::Percentage(25),
        ])
        .split(popup_area)[1];

    f.render_widget(Clear, popup_area);

    let block = Block::default()
        .title("Proxy Information")
        .borders(Borders::ALL)
        .style(Style::default().bg(Color::DarkGray));

    let mut text = vec![];

    if let Some(proxy_name) = app.get_selected_proxy_name() {
        text.push(Line::from(vec![
            Span::styled("Name: ", Style::default().fg(Color::Yellow)),
            Span::from(proxy_name.clone()),
        ]));

        if let Some(item) = app.proxies.get(&proxy_name) {
            if let Some(ptype) = &item.proxy_type {
                text.push(Line::from(vec![
                    Span::styled("Type: ", Style::default().fg(Color::Yellow)),
                    Span::from(ptype.clone()),
                ]));
            }

            // Render extra fields pretty-printed
            let extra_json = serde_json::to_string_pretty(&item.extra).unwrap_or_default();
            let lines: Vec<String> = extra_json.lines().map(|s| s.to_string()).collect();
            for line in lines {
                text.push(Line::from(line));
            }
        } else {
            text.push(Line::from("Details not found (Recursive group?)"));
        }
    } else {
        text.push(Line::from("No proxy selected"));
    }

    let p = Paragraph::new(text)
        .block(block)
        .wrap(Wrap { trim: false })
        .scroll((app.popup_scroll, 0));

    f.render_widget(p, popup_area);
}

fn draw_status_bar(f: &mut Frame, app: &App, area: Rect) {
    let text = if let Some(err) = &app.error {
        Line::from(vec![
            Span::styled(
                "Error: ",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
            Span::styled(err, Style::default().fg(Color::Red)),
        ])
    } else if app.is_editing {
        Line::from("Editing: Type to input | Enter: Save | Esc: Cancel")
    } else {
        match app.focus {
            Focus::Settings => Line::from("Esc/q: Back | j/k: Nav | Enter: Change/Edit | s: Close"),
            _ => Line::from(
                "q: Quit | j/k: Nav | l/Enter: Select | r: Refresh | t: Test | s: Settings | i: Info",
            ),
        }
    };

    f.render_widget(
        Paragraph::new(text).style(Style::default().fg(Color::DarkGray)),
        area,
    );
}

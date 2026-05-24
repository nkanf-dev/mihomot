use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, Borders, Cell, Clear, Gauge, List, ListItem, Paragraph, Row, Sparkline, Table, Wrap,
    },
};

use crate::app::{
    App, ConfigEntry, Focus, NAV_ITEMS, ProxyItem, ProxyLatencyStatus, ProxyPane, Route,
};

struct Theme;

impl Theme {
    const BG: Color = Color::Rgb(22, 24, 31);
    const PANEL: Color = Color::Rgb(30, 32, 41);
    const PANEL_ALT: Color = Color::Rgb(36, 39, 49);
    const SURFACE: Color = Color::Rgb(68, 71, 90);
    const TEXT: Color = Color::Rgb(248, 248, 242);
    const MUTED: Color = Color::Rgb(98, 114, 164);
    const ACCENT: Color = Color::Rgb(80, 250, 123);
    const ACCENT_ALT: Color = Color::Rgb(139, 233, 253);
    const WARN: Color = Color::Rgb(241, 250, 140);
    const BAD: Color = Color::Rgb(255, 85, 85);
    const GOOD: Color = Color::Rgb(80, 250, 123);
    const WHITE: Color = Color::White;
}

pub fn draw(f: &mut Frame, app: &mut App) {
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(f.area());

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(18), Constraint::Min(0)])
        .split(root[1]);

    draw_header(f, app, root[0]);
    draw_nav(f, app, body[0]);
    draw_content(f, app, body[1]);
    draw_status_bar(f, app, root[2]);

    if app.show_config_picker {
        draw_config_picker(f, app);
    }

    if app.show_info_popup {
        draw_info_popup(f, app);
    }

    if app.is_editing {
        draw_input_popup(f, app);
    }
}

fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let (mode, tun, port) = if let Some(config) = &app.config {
        (
            config.mode.as_str().to_string(),
            if config.tun.enable {
                "tun:on"
            } else {
                "tun:off"
            }
            .to_string(),
            config.mixed_port.to_string(),
        )
    } else {
        (
            "loading".to_string(),
            "tun:unknown".to_string(),
            "-".to_string(),
        )
    };

    let group = app
        .get_selected_group_name()
        .map(String::as_str)
        .unwrap_or("-");
    let proxy = app
        .get_selected_proxy_name()
        .unwrap_or_else(|| "-".to_string());

    let header = vec![
        Line::from(vec![
            Span::styled(
                "  mihomot  ",
                Style::default()
                    .fg(Theme::WHITE)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "AI native mihomo manager",
                Style::default().fg(Theme::MUTED),
            ),
        ]),
        Line::from(vec![
            chip("endpoint", &app.app_settings.base_url, Theme::ACCENT),
            Span::raw("  "),
            chip("mode", &mode, Theme::ACCENT_ALT),
            Span::raw("  "),
            chip(
                "tun",
                &tun,
                if tun == "tun:on" {
                    Theme::GOOD
                } else {
                    Theme::BAD
                },
            ),
            Span::raw("  "),
            chip("port", &port, Theme::MUTED),
            Span::raw("  "),
            chip("group", group, Theme::WARN),
            Span::raw("  "),
            chip("proxy", &proxy, Theme::GOOD),
        ]),
    ];

    f.render_widget(
        Paragraph::new(header)
            .block(
                Block::default()
                    .borders(Borders::BOTTOM)
                    .border_style(Style::default().fg(Theme::MUTED)),
            )
            .style(Style::default().bg(Theme::BG).fg(Theme::TEXT)),
        area,
    );
}

fn chip<'a>(label: &'a str, value: &'a str, color: Color) -> Span<'a> {
    Span::styled(
        format!(" {label}:{value} "),
        Style::default()
            .fg(color)
            .bg(Theme::SURFACE)
            .add_modifier(Modifier::BOLD),
    )
}

fn focus_border_style(active: bool) -> Style {
    if active {
        Style::default()
            .fg(Theme::ACCENT)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Theme::MUTED)
    }
}

fn selection_style() -> Style {
    Style::default()
        .fg(Theme::WHITE)
        .bg(Theme::SURFACE)
        .add_modifier(Modifier::BOLD)
}

fn selected_surface_style() -> Style {
    Style::default()
        .fg(Theme::ACCENT)
        .add_modifier(Modifier::BOLD)
}

fn draw_nav(f: &mut Frame, app: &App, area: Rect) {
    let nav_focused = app.focus == Focus::Nav;
    let items: Vec<ListItem> = NAV_ITEMS
        .iter()
        .enumerate()
        .map(|(idx, item)| {
            let selected = idx == app.nav_index;
            let route_active = item.route == Some(app.route);
            let marker = if selected && nav_focused {
                ">"
            } else if route_active {
                "*"
            } else {
                " "
            };
            let style = if selected && nav_focused {
                selection_style()
            } else if route_active {
                selected_surface_style()
            } else {
                Style::default().fg(Theme::MUTED)
            };
            ListItem::new(Line::from(vec![
                Span::styled(format!("{marker} "), style),
                Span::styled(item.label, style),
            ]))
        })
        .collect();

    let block = Block::default()
        .title(" Navigation ")
        .borders(Borders::ALL)
        .border_style(focus_border_style(nav_focused))
        .style(Style::default().bg(Theme::PANEL));

    f.render_widget(List::new(items).block(block), area);
}

fn draw_content(f: &mut Frame, app: &mut App, area: Rect) {
    match app.route {
        Route::Dashboard => draw_dashboard(f, app, area),
        Route::Proxies => draw_proxies_page(f, app, area),
        Route::Settings => draw_settings_page(f, app, area),
        Route::Help => draw_help(f, app, area),
    }
}

fn draw_dashboard(f: &mut Frame, app: &App, area: Rect) {
    let inner_area = draw_content_outline(f, app, area, " Dashboard ");

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Length(8),
            Constraint::Length(3),
            Constraint::Min(8),
        ])
        .split(inner_area);

    let summary = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(34),
            Constraint::Percentage(33),
            Constraint::Percentage(33),
        ])
        .split(chunks[0]);

    draw_config_card(f, app, summary[0]);
    draw_selection_card(f, app, summary[1]);
    draw_traffic_card(f, app, summary[2]);
    draw_latency_gauge(f, app, chunks[1]);
    draw_traffic_sparklines(f, app, chunks[2]);
}

fn draw_content_outline(f: &mut Frame, app: &App, area: Rect, title: &str) -> Rect {
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(focus_border_style(app.focus == Focus::Content));
    let inner = block.inner(area);
    f.render_widget(block, area);
    inner
}

fn draw_config_card(f: &mut Frame, app: &App, area: Rect) {
    let lines = if let Some(config) = &app.config {
        vec![
            kv_line("Mode", &config.mode, Theme::ACCENT_ALT),
            kv_line("Mixed Port", config.mixed_port.to_string(), Theme::TEXT),
            kv_line(
                "TUN",
                if config.tun.enable {
                    "Enabled"
                } else {
                    "Disabled"
                },
                if config.tun.enable {
                    Theme::GOOD
                } else {
                    Theme::BAD
                },
            ),
            kv_line("Log Level", &config.log_level, Theme::MUTED),
        ]
    } else {
        vec![Line::from(Span::styled(
            "Loading config...",
            Style::default().fg(Theme::MUTED),
        ))]
    };

    render_card(f, area, "Config", lines, false);
}

fn draw_selection_card(f: &mut Frame, app: &App, area: Rect) {
    let group_name = app
        .get_selected_group_name()
        .cloned()
        .unwrap_or_else(|| "-".to_string());
    let proxy_name = app
        .get_selected_proxy_name()
        .unwrap_or_else(|| "-".to_string());
    let group_type = app
        .selected_group()
        .and_then(|group| group.proxy_type.as_deref())
        .unwrap_or("-");
    let proxy_type = app
        .selected_proxy_item()
        .and_then(|proxy| proxy.proxy_type.as_deref())
        .unwrap_or("-");
    let lines = vec![
        kv_line("Group", &group_name, Theme::WARN),
        kv_line("Group Type", group_type, Theme::MUTED),
        kv_line("Proxy", &proxy_name, Theme::GOOD),
        kv_line("Proxy Type", proxy_type, Theme::MUTED),
    ];

    render_card(f, area, "Selection", lines, false);
}

fn draw_traffic_card(f: &mut Frame, app: &App, area: Rect) {
    let lines = vec![
        kv_line(
            "Download",
            format!("{}/s", format_speed(app.current_down)),
            Theme::GOOD,
        ),
        kv_line(
            "Upload",
            format!("{}/s", format_speed(app.current_up)),
            Theme::WARN,
        ),
        kv_line("Groups", app.group_names.len().to_string(), Theme::ACCENT),
        kv_line("Endpoint", &app.app_settings.base_url, Theme::MUTED),
    ];

    render_card(f, area, "Traffic", lines, false);
}

fn draw_latency_gauge(f: &mut Frame, app: &App, area: Rect) {
    let selected_proxy_name = app.get_selected_proxy_name();
    let selected_proxy_latency = selected_proxy_name
        .as_ref()
        .and_then(|name| app.proxy_latency.get(name));
    let (latency_label, latency_color, percent) = match selected_proxy_latency {
        Some(ProxyLatencyStatus::Testing) => {
            ("Testing selected node...".to_string(), Theme::WARN, 35)
        }
        Some(ProxyLatencyStatus::Success(ms)) => {
            let color = latency_color(Some(*ms));
            (
                format!("{} ms", ms),
                color,
                (1000.0 / (*ms as f64).max(10.0) * 100.0).min(100.0) as u16,
            )
        }
        Some(ProxyLatencyStatus::Failed(msg)) => (format!("Err: {msg}"), Theme::BAD, 100),
        None if selected_proxy_name.is_some() => ("Untested".to_string(), Theme::MUTED, 0),
        None => ("No node selected".to_string(), Theme::MUTED, 0),
    };

    let title = selected_proxy_name
        .as_deref()
        .map(|name| format!(" Selected Node Latency: {name} "))
        .unwrap_or_else(|| " Selected Node Latency ".to_string());
    let gauge = Gauge::default()
        .block(
            Block::default()
                .title(title)
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Theme::MUTED)),
        )
        .gauge_style(Style::default().fg(latency_color))
        .percent(percent)
        .label(latency_label);

    f.render_widget(gauge, area);
}

fn draw_traffic_sparklines(f: &mut Frame, app: &App, area: Rect) {
    let chart_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);

    let width = chart_chunks[0].width.saturating_sub(2) as usize;
    let down_data: Vec<u64> = app
        .traffic_history_down
        .iter()
        .rev()
        .take(width)
        .rev()
        .cloned()
        .collect();
    let up_data: Vec<u64> = app
        .traffic_history_up
        .iter()
        .rev()
        .take(width)
        .rev()
        .cloned()
        .collect();

    let down_title = format!(" Download {}/s ", format_speed(app.current_down));
    let up_title = format!(" Upload {}/s ", format_speed(app.current_up));
    let down_sparkline = Sparkline::default()
        .block(
            Block::default()
                .title(down_title)
                .borders(Borders::TOP | Borders::LEFT | Borders::RIGHT)
                .border_style(Style::default().fg(Theme::MUTED)),
        )
        .data(&down_data)
        .style(Style::default().fg(Theme::GOOD));
    let up_sparkline = Sparkline::default()
        .block(
            Block::default()
                .title(up_title)
                .borders(Borders::BOTTOM | Borders::LEFT | Borders::RIGHT)
                .border_style(Style::default().fg(Theme::MUTED)),
        )
        .data(&up_data)
        .style(Style::default().fg(Theme::WARN));

    f.render_widget(down_sparkline, chart_chunks[0]);
    f.render_widget(up_sparkline, chart_chunks[1]);
}

fn draw_proxies_page(f: &mut Frame, app: &mut App, area: Rect) {
    let inner_area = draw_content_outline(f, app, area, " Proxies ");

    let direction = if area.width < 90 {
        Direction::Vertical
    } else {
        Direction::Horizontal
    };
    let chunks = Layout::default()
        .direction(direction)
        .margin(1)
        .constraints([Constraint::Percentage(38), Constraint::Percentage(62)])
        .split(inner_area);

    draw_group_cards(f, app, chunks[0]);
    draw_proxy_cards(f, app, chunks[1]);
}

fn draw_group_cards(f: &mut Frame, app: &mut App, area: Rect) {
    let selected_idx = app.group_state.selected();
    let proxy_pane = app.proxy_pane;
    let items: Vec<ListItem<'_>> = app
        .group_names
        .iter()
        .enumerate()
        .map(|(idx, name)| {
            let selected = selected_idx == Some(idx);
            let group = app.proxies.get(name);
            let ptype = group
                .and_then(|item| item.proxy_type.as_deref())
                .unwrap_or("Group");
            let count = group.and_then(|item| item.all.as_ref()).map_or(0, Vec::len);
            let now = group.and_then(|item| item.now.as_deref()).unwrap_or("-");
            let style = card_text_style(selected, proxy_pane == ProxyPane::Groups);
            ListItem::new(vec![
                Line::from(vec![
                    Span::styled(if selected { "> " } else { "  " }, style),
                    Span::styled(name.as_str(), style.add_modifier(Modifier::BOLD)),
                ]),
                Line::from(vec![
                    Span::styled("  type ", Style::default().fg(Theme::MUTED)),
                    Span::styled(ptype, Style::default().fg(Theme::TEXT)),
                    Span::styled("  nodes ", Style::default().fg(Theme::MUTED)),
                    Span::styled(count.to_string(), Style::default().fg(Theme::ACCENT)),
                ]),
                Line::from(vec![
                    Span::styled("  current ", Style::default().fg(Theme::MUTED)),
                    Span::styled(now, Style::default().fg(Theme::GOOD)),
                ]),
                Line::from(""),
            ])
        })
        .collect();

    let focused = app.focus == Focus::Content && app.proxy_pane == ProxyPane::Groups;
    let block = Block::default()
        .title(" Groups ")
        .borders(Borders::ALL)
        .border_style(focus_border_style(focused))
        .style(Style::default().bg(Theme::PANEL));

    let content = if items.is_empty() {
        List::new(vec![ListItem::new(Line::from(Span::styled(
            "No selector groups found",
            Style::default().fg(Theme::MUTED),
        )))])
    } else {
        List::new(items)
    };

    f.render_stateful_widget(content.block(block), area, &mut app.group_state);
}

fn draw_proxy_cards(f: &mut Frame, app: &mut App, area: Rect) {
    let selected_proxy = app.proxy_state.selected();
    let selected_group_name = app.get_selected_group_name();
    let (current_proxy, proxy_names) = selected_group_name
        .and_then(|group_name| app.proxies.get(group_name))
        .map(|group| {
            (
                group.now.as_deref(),
                group.all.as_deref().unwrap_or_default(),
            )
        })
        .unwrap_or_default();
    let proxy_pane = app.proxy_pane;

    let items: Vec<ListItem<'_>> = proxy_names
        .iter()
        .enumerate()
        .map(|(idx, name)| {
            let selected = selected_proxy == Some(idx);
            let current = current_proxy == Some(name.as_str());
            let proxy = app.proxies.get(name);
            let ptype = proxy
                .and_then(|item| item.proxy_type.as_deref())
                .unwrap_or("Proxy");
            let latency = app.proxy_latency.get(name);
            let (latency_text, latency_style) = proxy_latency_display(latency);
            let style = card_text_style(selected, proxy_pane == ProxyPane::Proxies);
            ListItem::new(vec![
                Line::from(vec![
                    Span::styled(if selected { "> " } else { "  " }, style),
                    Span::styled(name.as_str(), style.add_modifier(Modifier::BOLD)),
                    Span::raw(" "),
                    Span::styled(
                        if current { "[active]" } else { "" },
                        Style::default().fg(Theme::GOOD),
                    ),
                ]),
                Line::from(vec![
                    Span::styled("  type ", Style::default().fg(Theme::MUTED)),
                    Span::styled(ptype, Style::default().fg(Theme::TEXT)),
                    Span::styled("  latency ", Style::default().fg(Theme::MUTED)),
                    Span::styled(latency_text, latency_style),
                ]),
                Line::from(""),
            ])
        })
        .collect();

    let focused = app.focus == Focus::Content && app.proxy_pane == ProxyPane::Proxies;
    let title = app
        .get_selected_group_name()
        .map(|group_name| format!(" Nodes: {group_name} "))
        .unwrap_or_else(|| " Nodes ".to_string());
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(focus_border_style(focused))
        .style(Style::default().bg(Theme::PANEL_ALT));

    let content = if items.is_empty() {
        List::new(vec![ListItem::new(Line::from(Span::styled(
            "No proxies in this group",
            Style::default().fg(Theme::MUTED),
        )))])
    } else {
        List::new(items)
    };

    f.render_stateful_widget(content.block(block), area, &mut app.proxy_state);
}

fn card_text_style(selected: bool, focused_pane: bool) -> Style {
    if selected && focused_pane {
        selection_style()
    } else if selected {
        selected_surface_style()
    } else {
        Style::default().fg(Theme::TEXT)
    }
}

fn draw_settings_page(f: &mut Frame, app: &mut App, area: Rect) {
    let inner_area = draw_content_outline(f, app, area, " Settings ");
    let inner = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([Constraint::Min(0)])
        .split(inner_area)[0];

    draw_settings_table(f, app, inner);
}

fn draw_settings_table(f: &mut Frame, app: &mut App, area: Rect) {
    let header_style = Style::default()
        .fg(Theme::WHITE)
        .add_modifier(Modifier::BOLD)
        .bg(Theme::SURFACE);

    let selected_style = selection_style();

    let header = Row::new(vec!["Setting", "Current Value", "Action"])
        .style(header_style)
        .height(1)
        .bottom_margin(1);

    let rows: Vec<Row> = app
        .settings_items
        .iter()
        .map(|item| {
            let (label, value, action) = setting_row_data(app, item);
            Row::new(vec![
                Cell::from(label).style(
                    Style::default()
                        .fg(Theme::ACCENT_ALT)
                        .add_modifier(Modifier::BOLD),
                ),
                Cell::from(value).style(Style::default().fg(Theme::TEXT)),
                Cell::from(action).style(Style::default().fg(Theme::MUTED)),
            ])
            .height(1)
        })
        .collect();

    let focused = app.focus == Focus::Content && app.route == Route::Settings;
    let table = Table::new(
        rows,
        [
            Constraint::Percentage(30),
            Constraint::Percentage(35),
            Constraint::Percentage(35),
        ],
    )
    .header(header)
    .block(
        Block::default()
            .title(" Configuration ")
            .title_alignment(Alignment::Center)
            .borders(Borders::ALL)
            .border_style(focus_border_style(focused))
            .style(Style::default().bg(Theme::PANEL)),
    )
    .row_highlight_style(selected_style)
    .highlight_symbol(">> ");

    f.render_stateful_widget(table, area, &mut app.settings_state);
}

fn setting_row_data(app: &App, item: &ConfigEntry) -> (&'static str, String, &'static str) {
    match item {
        ConfigEntry::BaseUrl => ("App: Base URL", app.app_settings.base_url.clone(), "Edit"),
        ConfigEntry::ApiSecret => (
            "App: API Secret",
            if app.app_settings.api_secret.is_empty() {
                "<none>".to_string()
            } else {
                "******".to_string()
            },
            "Edit",
        ),
        ConfigEntry::TestUrl => ("App: Test URL", app.app_settings.test_url.clone(), "Edit"),
        ConfigEntry::TestTimeout => (
            "App: Test Timeout (ms)",
            app.app_settings.test_timeout.to_string(),
            "Edit",
        ),
        ConfigEntry::ConfigPath => (
            "Mihomo: Active Config",
            app.config_path.display().to_string(),
            "Edit Path",
        ),
        ConfigEntry::ConfigSwitch => {
            let dir = app
                .config_path
                .parent()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| ".".to_string());
            ("Mihomo: Switch Config", dir, "Choose")
        }
        ConfigEntry::ConfigFile => (
            "Mihomo: Edit Config",
            app.config_path.display().to_string(),
            "Open $EDITOR",
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
    }
}

fn draw_help(f: &mut Frame, app: &App, area: Rect) {
    let inner_area = draw_content_outline(f, app, area, " Help ");
    let inner = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([Constraint::Min(0)])
        .split(inner_area)[0];

    let lines = vec![
        Line::from(vec![
            Span::styled(
                "Global",
                Style::default()
                    .fg(Theme::ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  q quit  ?/F1 help  s settings  r refresh  t test"),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                "Navigation",
                Style::default()
                    .fg(Theme::ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  Tab switch nav/content  j/k or arrows move  Enter activate"),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                "Proxies",
                Style::default()
                    .fg(Theme::ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(
                "  h/l switch groups/nodes  Left from groups returns nav  Enter select node  i details",
            ),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                "Navigation: ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw("Use "),
            Span::styled("j/k", Theme::ACCENT_ALT),
            Span::raw(" or "),
            Span::styled("up/down", Theme::ACCENT_ALT),
            Span::raw(" to move. Use "),
            Span::styled("Enter", Theme::ACCENT_ALT),
            Span::raw(" to activate."),
        ]),
        Line::from(vec![
            Span::styled(
                "Sidebar:    ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw("Use "),
            Span::styled("h/l", Theme::ACCENT_ALT),
            Span::raw(" or "),
            Span::styled("left/right", Theme::ACCENT_ALT),
            Span::raw(" to switch between Sidebar and Content."),
        ]),
        Line::from(vec![
            Span::styled(
                "Actions:    ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::styled("e", Theme::ACCENT_ALT),
            Span::raw(" to edit config. "),
            Span::styled("t", Theme::ACCENT_ALT),
            Span::raw(" to test latency. "),
            Span::styled("r", Theme::ACCENT_ALT),
            Span::raw(" to restart mihomo."),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "This UI keeps mihomot's own proxy switching and latency testing logic.",
            Style::default().fg(Theme::MUTED),
        )),
    ];

    f.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .title(" Shortcuts ")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Theme::MUTED))
                    .style(Style::default().bg(Theme::PANEL)),
            )
            .wrap(Wrap { trim: false }),
        inner,
    );
}

fn render_card(f: &mut Frame, area: Rect, title: &str, lines: Vec<Line<'_>>, active: bool) {
    f.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .title(format!(" {title} "))
                    .borders(Borders::ALL)
                    .border_style(focus_border_style(active))
                    .style(Style::default().bg(Theme::PANEL)),
            )
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn kv_line(key: impl Into<String>, value: impl Into<String>, value_color: Color) -> Line<'static> {
    let key = key.into();
    let value = value.into();
    Line::from(vec![
        Span::styled(format!("{key}: "), Style::default().fg(Theme::MUTED)),
        Span::styled(value, Style::default().fg(value_color)),
    ])
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

fn latency_color(latency: Option<u64>) -> Color {
    match latency {
        Some(ms) if ms < 200 => Theme::GOOD,
        Some(ms) if ms < 500 => Theme::WARN,
        Some(_) => Theme::BAD,
        None => Theme::MUTED,
    }
}

fn proxy_latency_display(latency: Option<&ProxyLatencyStatus>) -> (String, Style) {
    match latency {
        Some(ProxyLatencyStatus::Testing) => (
            "testing".to_string(),
            Style::default()
                .fg(Theme::WARN)
                .add_modifier(Modifier::BOLD),
        ),
        Some(ProxyLatencyStatus::Success(ms)) => (
            format!("{ms} ms"),
            Style::default().fg(latency_color(Some(*ms))),
        ),
        Some(ProxyLatencyStatus::Failed(msg)) => {
            (msg.to_lowercase(), Style::default().fg(Theme::BAD))
        }
        None => ("-".to_string(), Style::default().fg(Theme::MUTED)),
    }
}

fn draw_input_popup(f: &mut Frame, app: &mut App) {
    let area = centered_rect(78, 42, f.area());
    f.render_widget(Clear, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Edit Value ")
        .border_style(Style::default().fg(Theme::ACCENT))
        .style(Style::default().bg(Theme::SURFACE).fg(Theme::TEXT));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(inner);

    let value = if app.editing_value.is_empty() {
        Line::from(Span::styled(
            "<empty>",
            Style::default()
                .fg(Theme::MUTED)
                .add_modifier(Modifier::ITALIC),
        ))
    } else {
        Line::from(vec![
            Span::styled(app.editing_value.as_str(), Style::default().fg(Theme::TEXT)),
            Span::styled(" ", Style::default().bg(Theme::ACCENT)),
        ])
    };

    f.render_widget(
        Paragraph::new("Enter saves, Esc cancels")
            .style(Style::default().fg(Theme::MUTED))
            .alignment(Alignment::Center),
        rows[0],
    );

    let input_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Theme::MUTED))
        .style(Style::default().bg(Theme::BG).fg(Theme::TEXT));
    let input_inner = input_block.inner(rows[1]);
    f.render_widget(input_block, rows[1]);
    f.render_widget(
        Paragraph::new(value).wrap(Wrap { trim: false }),
        input_inner,
    );

    f.render_widget(
        Paragraph::new("Backspace edits text")
            .style(Style::default().fg(Theme::MUTED))
            .alignment(Alignment::Center),
        rows[2],
    );
}

fn draw_info_popup(f: &mut Frame, app: &App) {
    let area = centered_rect(58, 58, f.area());
    f.render_widget(Clear, area);

    let block = Block::default()
        .title(" Proxy Information ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Theme::ACCENT))
        .style(Style::default().bg(Theme::PANEL_ALT));

    let mut text = vec![];

    if let Some(proxy_name) = app.get_selected_proxy_name() {
        text.push(Line::from(vec![
            Span::styled("Name: ", Style::default().fg(Theme::WARN)),
            Span::from(proxy_name.clone()),
        ]));

        if let Some(item) = app.proxies.get(&proxy_name) {
            push_proxy_details(&mut text, item);
        } else {
            text.push(Line::from(
                "Details not found (recursive group or external item)",
            ));
        }
    } else {
        text.push(Line::from("No proxy selected"));
    }

    let p = Paragraph::new(text)
        .block(block)
        .wrap(Wrap { trim: false })
        .scroll((app.popup_scroll, 0));

    f.render_widget(p, area);
}

fn draw_config_picker(f: &mut Frame, app: &mut App) {
    let area = centered_rect(70, 58, f.area());
    f.render_widget(Clear, area);

    let items: Vec<ListItem> = if app.config_candidates.is_empty() {
        vec![ListItem::new(Line::from(Span::styled(
            "No mihomo .yaml/.yml configs found in current config directory",
            Style::default().fg(Theme::MUTED),
        )))]
    } else {
        app.config_candidates
            .iter()
            .map(|candidate| {
                let active = candidate.path == app.config_path;
                ListItem::new(vec![
                    Line::from(vec![
                        Span::styled(
                            if active { "* " } else { "  " },
                            Style::default().fg(Theme::ACCENT),
                        ),
                        Span::styled(candidate.label.clone(), Style::default().fg(Theme::TEXT)),
                    ]),
                    Line::from(vec![
                        Span::raw("  "),
                        Span::styled(candidate.detail.clone(), Style::default().fg(Theme::MUTED)),
                    ]),
                    Line::from(""),
                ])
            })
            .collect()
    };

    let list = List::new(items)
        .block(
            Block::default()
                .title(" Switch Mihomo Config ")
                .borders(Borders::ALL)
                .border_style(focus_border_style(true))
                .style(Style::default().bg(Theme::PANEL)),
        )
        .highlight_style(selection_style())
        .highlight_symbol(">> ");

    f.render_stateful_widget(list, area, &mut app.config_picker_state);
}

fn push_proxy_details<'a>(text: &mut Vec<Line<'a>>, item: &ProxyItem) {
    if let Some(ptype) = &item.proxy_type {
        text.push(Line::from(vec![
            Span::styled("Type: ", Style::default().fg(Theme::WARN)),
            Span::from(ptype.clone()),
        ]));
    }

    let extra_json = serde_json::to_string_pretty(&item.extra).unwrap_or_default();
    for line in extra_json.lines() {
        text.push(Line::from(line.to_string()));
    }
}

fn draw_status_bar(f: &mut Frame, app: &App, area: Rect) {
    let text = if let Some(err) = &app.error {
        Line::from(vec![
            Span::styled(
                "Error: ",
                Style::default().fg(Theme::BAD).add_modifier(Modifier::BOLD),
            ),
            Span::styled(err, Style::default().fg(Theme::BAD)),
        ])
    } else if app.is_editing {
        Line::from("Editing: type value | Enter save | Esc cancel")
    } else if app.show_info_popup {
        Line::from("Proxy info: j/k scroll | Esc/q/i close")
    } else if app.show_config_picker {
        Line::from(
            "Config picker: j/k move | Enter switch | Esc/q close | edit path from Settings for arbitrary files",
        )
    } else {
        match app.route {
            Route::Proxies => Line::from(
                "q quit | Tab nav/content | h/l groups/nodes | Left from groups nav | Enter select | r refresh+test | i info",
            ),
            Route::Settings => Line::from(
                "q quit | Tab nav/content | j/k move | Enter change/edit/switch/open file | Esc nav",
            ),
            _ => Line::from(
                "q quit | Tab nav/content | Enter activate | r refresh | t test | s settings | ? help",
            ),
        }
    };

    f.render_widget(
        Paragraph::new(text).style(Style::default().bg(Theme::PANEL).fg(Theme::MUTED)),
        area,
    );
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

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};
    use std::collections::HashMap;

    fn test_app() -> App {
        let mut app = App::new(None, None);
        app.config = Some(crate::app::Config {
            mode: "rule".to_string(),
            tun: crate::app::Tun {
                enable: true,
                stack: Some("mixed".to_string()),
                device: Some("utun".to_string()),
            },
            mixed_port: 7890,
            log_level: "info".to_string(),
            allow_lan: false,
            bind_address: "*".to_string(),
            ipv6: true,
        });
        app.group_names = vec!["Auto".to_string()];
        app.group_state.select(Some(0));
        app.proxy_state.select(Some(0));
        app.proxy_latency
            .insert("Node A".to_string(), ProxyLatencyStatus::Success(120));

        let mut proxies = HashMap::new();
        proxies.insert(
            "Auto".to_string(),
            ProxyItem {
                name: Some("Auto".to_string()),
                proxy_type: Some("Selector".to_string()),
                now: Some("Node A".to_string()),
                all: Some(vec!["Node A".to_string(), "Node B".to_string()]),
                extra: serde_json::Map::new(),
            },
        );
        proxies.insert(
            "Node A".to_string(),
            ProxyItem {
                name: Some("Node A".to_string()),
                proxy_type: Some("Shadowsocks".to_string()),
                now: None,
                all: None,
                extra: serde_json::Map::new(),
            },
        );
        proxies.insert(
            "Node B".to_string(),
            ProxyItem {
                name: Some("Node B".to_string()),
                proxy_type: Some("Trojan".to_string()),
                now: None,
                all: None,
                extra: serde_json::Map::new(),
            },
        );
        app.proxies = proxies;
        app
    }

    fn render_screen(mut app: App, width: u16, height: u16) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("test terminal should initialize");
        terminal
            .draw(|frame| draw(frame, &mut app))
            .expect("TUI should render without error");

        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[tokio::test]
    async fn renders_all_top_level_routes() {
        for (route, expected) in [
            (Route::Dashboard, "Dashboard"),
            (Route::Proxies, "Nodes: Auto"),
            (Route::Settings, "Configuration"),
            (Route::Help, "Shortcuts"),
        ] {
            let mut app = test_app();
            app.set_route(route);

            let screen = render_screen(app, 120, 36);

            assert!(screen.contains("mihomot"));
            assert!(screen.contains("Navigation"));
            assert!(screen.contains(expected));
        }
    }

    #[tokio::test]
    async fn renders_proxy_page_on_narrow_and_wide_terminals() {
        for width in [72, 120] {
            let mut app = test_app();
            app.set_proxy_pane(ProxyPane::Proxies);

            let screen = render_screen(app, width, 34);

            assert!(screen.contains("Groups"));
            assert!(screen.contains("Nodes: Auto"));
            assert!(screen.contains("Node A"));
            assert!(screen.contains("120 ms"));
        }
    }

    #[tokio::test]
    async fn dashboard_shows_selected_node_latency() {
        let mut app = test_app();
        app.set_route(Route::Dashboard);
        app.group_state.select(Some(0));
        app.proxy_state.select(Some(0));
        app.proxy_latency
            .insert("Node A".to_string(), ProxyLatencyStatus::Testing);

        let screen = render_screen(app, 120, 34);

        assert!(screen.contains("Selected Node Latency"));
        assert!(screen.contains("Node A"));
        assert!(screen.contains("Testing selected node"));
    }

    #[tokio::test]
    async fn proxy_list_scrolls_to_selected_node() {
        let mut app = test_app();
        let nodes = (0..30)
            .map(|idx| format!("Node {idx:02}"))
            .collect::<Vec<_>>();

        app.group_names = vec!["Auto".to_string()];
        app.group_state.select(Some(0));
        app.proxy_state.select(Some(29));
        app.set_proxy_pane(ProxyPane::Proxies);

        let mut proxies = HashMap::new();
        proxies.insert(
            "Auto".to_string(),
            ProxyItem {
                name: Some("Auto".to_string()),
                proxy_type: Some("Selector".to_string()),
                now: Some("Node 29".to_string()),
                all: Some(nodes.clone()),
                extra: serde_json::Map::new(),
            },
        );
        for node in &nodes {
            proxies.insert(
                node.clone(),
                ProxyItem {
                    name: Some(node.clone()),
                    proxy_type: Some("Trojan".to_string()),
                    now: None,
                    all: None,
                    extra: serde_json::Map::new(),
                },
            );
        }
        app.proxies = proxies;

        let screen = render_screen(app, 120, 18);

        assert!(screen.contains("Node 29"));
        assert!(!screen.contains("Node 00"));
    }

    #[tokio::test]
    async fn edit_popup_renders_value_and_hints() {
        let mut app = test_app();
        app.is_editing = true;
        app.editing_value = "http://127.0.0.1:9090".to_string();

        let screen = render_screen(app, 100, 32);

        assert!(screen.contains("Edit Value"));
        assert!(screen.contains("127.0.0.1"));
        assert!(screen.contains("Enter saves"));
        assert!(screen.contains("Backspace edits text"));
    }
}

use anyhow::{Context, Result, bail};
use app::ConfigEntry;
use clap::{Parser, Subcommand};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

mod app;
mod config;
mod mihomo;
mod server;
mod token;
mod ui;

#[derive(Parser, Debug)]
#[command(version, about = "mihomot - AI native mihomo manager")]
struct Args {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Start the mihomot API server (default)
    Serve {
        /// Config file path (default: ~/.config/mihomo/config.yaml)
        #[arg(short, long)]
        config: Option<String>,
        /// Listen address for the mihomot API
        #[arg(long, default_value = "0.0.0.0:9091")]
        listen: String,
        /// Port for the mihomot API (overrides port in --listen)
        #[arg(short, long)]
        port: Option<u16>,
    },
    /// Launch the TUI client
    Tui {
        /// mihomo external-controller URL (default: auto-detect from config)
        #[arg(short = 'U', long)]
        url: Option<String>,
        /// mihomo API secret (default: auto-detect from config)
        #[arg(short = 'S', long)]
        secret: Option<String>,
        /// Config file path (default: ~/.config/mihomo/config.yaml)
        #[arg(short, long)]
        config: Option<String>,
        /// mihomo API port (shorthand for --url http://127.0.0.1:<port>)
        #[arg(short, long)]
        port: Option<u16>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    match args.command.unwrap_or(Commands::Serve {
        config: None,
        listen: "0.0.0.0:9091".to_string(),
        port: None,
    }) {
        Commands::Serve {
            config,
            listen,
            port,
        } => {
            let listen = if let Some(port) = port {
                // Override port in listen address
                let host = listen.rsplit_once(':').map(|(h, _)| h).unwrap_or("0.0.0.0");
                format!("{}:{}", host, port)
            } else {
                listen
            };
            run_serve(config, listen).await
        }
        Commands::Tui {
            url,
            secret,
            config,
            port,
        } => {
            let url = match (url, port) {
                (Some(u), _) => Some(u),
                (None, Some(p)) => Some(format!("http://127.0.0.1:{}", p)),
                (None, None) => None,
            };
            run_tui(url, secret, config).await
        }
    }
}

async fn run_serve(config_override: Option<String>, listen: String) -> Result<()> {
    let config_path = match config_override {
        Some(p) => std::path::PathBuf::from(p),
        None => config::default_config_path(),
    };

    if !config_path.exists() {
        eprintln!("mihomo config not found at: {}", config_path.display());
        eprintln!("Please create it first or specify --config <path>");
        std::process::exit(1);
    }
    let config_path = fs::canonicalize(&config_path)
        .with_context(|| format!("Failed to resolve {}", config_path.display()))?;

    // Read mihomo config to get external-controller and secret
    let mihomo_config =
        config::read_config(&config_path).context("Failed to parse mihomo config.yaml")?;

    let (host, port) = config::parse_external_controller(
        mihomo_config
            .external_controller
            .as_deref()
            .unwrap_or("0.0.0.0:9090"),
    );
    let secret = mihomo_config.secret.unwrap_or_default();

    let (listen_host, listen_port) = parse_listen_addr(&listen);
    let public_ip = detect_public_ip().await;
    let server_host = public_ip
        .clone()
        .or_else(find_server_ip)
        .unwrap_or_else(|| "127.0.0.1".to_string());
    let agent_endpoint = format!("http://{}:{}", server_host, listen_port);

    // Determine the reachable mihomo endpoint for the local mihomot process.
    let mihomo_endpoint = {
        let ip = find_server_ip().unwrap_or_else(|| "127.0.0.1".to_string());
        let bind_host = if host == "0.0.0.0" || host == "::" {
            &ip
        } else {
            &host
        };
        format!("http://{}:{}", bind_host, port)
    };

    // Generate token
    let token_str = token::generate_token(&secret)?;
    token::save_token(&token_str).ok(); // best-effort

    // Print startup message
    println!();
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("把这段话发给你的 AI agent:");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!();
    println!(
        "我已安装 mihomot，请帮我管理我这个服务器({})的代理。",
        server_host
    );
    println!("mihomot endpoint: {}", agent_endpoint);
    println!(
        "如果没有安装skill先安装这个skill: {}",
        mihomo::skill_install_url()
    );
    println!("token: {}", token_str);
    println!();
    println!("提示:");
    println!("  1. 请确认服务器防火墙/安全组已开放 TCP {}。", listen_port);
    if public_ip.is_none() {
        println!(
            "  2. 未能自动获取公网 IP；如果上面的 endpoint 不是 agent 可访问地址，请把它改成公网 IP/域名后再发送。"
        );
    } else {
        println!(
            "  2. 如果上面的 endpoint 不是 agent 可访问地址，请改成正确的公网 IP/域名后再发送。"
        );
    }
    if listen_host == "127.0.0.1" || listen_host == "::1" || listen_host == "localhost" {
        println!(
            "  3. 当前 mihomot 只监听 {}，远程 agent 可能无法访问；需要远程管理时请监听 0.0.0.0:{}。",
            listen_host, listen_port
        );
    }
    println!();
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!();

    // Ensure mihomo is available (detect or auto-install)
    let runtime = mihomo::ensure_mihomo(&config_path).await?;
    println!("mihomo runtime: {:?}", runtime);

    if !mihomo::check_alive(&mihomo_endpoint, &secret)
        .await
        .unwrap_or(false)
    {
        println!("mihomo is not responding, attempting to start...");
        if let Err(e) = mihomo::start(&runtime, &config_path) {
            eprintln!("Failed to start mihomo: {}", e);
        } else {
            // Wait for mihomo to be ready
            for _ in 0..10 {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                if mihomo::check_alive(&mihomo_endpoint, &secret)
                    .await
                    .unwrap_or(false)
                {
                    println!("mihomo started successfully.");
                    break;
                }
            }
        }
    }

    // Start HTTP API server
    println!("Starting mihomot API server on {}...", listen);
    server::start_server(&listen, config_path, secret, mihomo_endpoint).await?;

    Ok(())
}

async fn run_tui(
    url: Option<String>,
    secret: Option<String>,
    config_override: Option<String>,
) -> Result<()> {
    let config_path = match config_override {
        Some(path) => PathBuf::from(path),
        None => config::default_config_path(),
    };
    let config_path = if config_path.exists() {
        fs::canonicalize(&config_path)
            .with_context(|| format!("Failed to resolve {}", config_path.display()))?
    } else {
        config_path
    };

    // Auto-detect url/secret from mihomo config if not provided
    let (url, secret) = match (url, secret) {
        (Some(u), Some(s)) => (Some(u), Some(s)),
        (u, s) => {
            if config_path.exists() {
                if let Ok(mc) = config::read_config(&config_path) {
                    let detected_url = u.or_else(|| {
                        mc.external_controller.map(|ec| {
                            let (host, port) = config::parse_external_controller(&ec);
                            let host = if host == "0.0.0.0" || host == "::" || host.is_empty() {
                                "127.0.0.1"
                            } else {
                                host.as_str()
                            };
                            format!("http://{}:{}", host, port)
                        })
                    });
                    let detected_secret = s.or(mc.secret);
                    (detected_url, detected_secret)
                } else {
                    (u, s)
                }
            } else {
                (u, s)
            }
        }
    };

    let mut terminal = ratatui::init();

    let mut app = app::App::new(url, secret);
    app.config_path = config_path;
    let _ = app.fetch_proxies().await;
    let _ = app.fetch_config().await;
    app.trigger_latency_test();

    let app_result = run_app(&mut terminal, &mut app).await;

    ratatui::restore();

    app_result
}

async fn run_app(terminal: &mut ratatui::DefaultTerminal, app: &mut app::App) -> Result<()> {
    use crossterm::event::{self, Event, KeyCode, KeyEventKind};

    loop {
        terminal.draw(|f| ui::draw(f, app))?;

        if let Ok(status) = app.real_latency_rx.try_recv() {
            app.real_latency_status = status;
        }

        while let Ok((name, latency_status)) = app.proxy_test_rx.try_recv() {
            app.proxy_latency.insert(name, latency_status);
        }

        while let Ok(traffic) = app.traffic_rx.try_recv() {
            app.on_traffic(traffic);
        }

        if event::poll(std::time::Duration::from_millis(100))?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            if app.is_editing {
                match key.code {
                    KeyCode::Esc => {
                        app.is_editing = false;
                    }
                    KeyCode::Enter => {
                        if let Err(err) = commit_edit(app).await {
                            app.error = Some(err.to_string());
                        } else {
                            app.error = None;
                        }
                        app.is_editing = false;
                    }
                    KeyCode::Backspace => {
                        app.editing_value.pop();
                    }
                    KeyCode::Char(c) => {
                        app.editing_value.push(c);
                    }
                    _ => {}
                }
                continue;
            }

            if app.show_config_picker {
                match key.code {
                    KeyCode::Esc | KeyCode::Char('q') => {
                        app.show_config_picker = false;
                    }
                    KeyCode::Char('j') | KeyCode::Down => app.next_config_candidate(),
                    KeyCode::Char('k') | KeyCode::Up => app.previous_config_candidate(),
                    KeyCode::Enter => {
                        if let Some(path) = app.selected_config_candidate() {
                            if let Err(err) = switch_config_file(app, &path).await {
                                app.error = Some(err.to_string());
                            } else {
                                app.error = None;
                                app.show_config_picker = false;
                            }
                        }
                    }
                    _ => {}
                }
            } else if app.show_info_popup {
                match key.code {
                    KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('i') => {
                        app.show_info_popup = false;
                        app.popup_scroll = 0;
                    }
                    KeyCode::Char('j') | KeyCode::Down => app.scroll_popup_down(),
                    KeyCode::Char('k') | KeyCode::Up => app.scroll_popup_up(),
                    _ => {}
                }
            } else {
                match key.code {
                    KeyCode::Char('q') => return Ok(()),
                    KeyCode::Char('?') | KeyCode::F(1) => {
                        app.set_route(app::Route::Help);
                    }
                    KeyCode::Char('r') => {
                        let _ = app.fetch_proxies().await;
                        let _ = app.fetch_config().await;
                        if app.route == app::Route::Proxies {
                            app.trigger_group_latency_test();
                        }
                    }
                    KeyCode::Char('t') => {
                        if app.route == app::Route::Proxies {
                            app.trigger_selected_proxy_latency_test();
                        } else {
                            app.trigger_latency_test();
                        }
                    }
                    KeyCode::Char('s') => {
                        app.set_route(app::Route::Settings);
                    }
                    KeyCode::Tab => {
                        app.toggle_focus();
                    }
                    KeyCode::Char('i') => {
                        if app.route == app::Route::Proxies {
                            app.show_info_popup = true;
                        }
                    }
                    KeyCode::Esc => {
                        app.focus_nav();
                    }
                    _ => match app.focus {
                        app::Focus::Nav => {
                            if handle_nav_key(app, key.code) {
                                return Ok(());
                            }
                        }
                        app::Focus::Content => match app.route {
                            app::Route::Dashboard | app::Route::Help => {
                                handle_simple_content_key(app, key.code);
                            }
                            app::Route::Proxies => {
                                handle_proxies_key(app, key.code).await;
                            }
                            app::Route::Settings => {
                                handle_settings_key(terminal, app, key.code).await;
                            }
                        },
                    },
                }
            }
        }
    }
}

/// Handle sidebar keys; returns true only when the user activates Quit.
fn handle_nav_key(app: &mut app::App, code: crossterm::event::KeyCode) -> bool {
    use crossterm::event::KeyCode;

    match code {
        KeyCode::Char('j') | KeyCode::Down => app.next_nav(),
        KeyCode::Char('k') | KeyCode::Up => app.previous_nav(),
        KeyCode::Char('l') | KeyCode::Right if app.selected_nav_item().route == Some(app.route) => {
            app.focus = app::Focus::Content;
        }
        KeyCode::Enter => return app.activate_nav(),
        _ => {}
    }

    false
}

/// Handle pages that only need a way back to the sidebar.
fn handle_simple_content_key(app: &mut app::App, code: crossterm::event::KeyCode) {
    use crossterm::event::KeyCode;

    match code {
        KeyCode::Esc | KeyCode::Left | KeyCode::Char('h') => app.focus_nav(),
        _ => {}
    }
}

/// Handle proxy group and node navigation without changing the underlying API logic.
async fn handle_proxies_key(app: &mut app::App, code: crossterm::event::KeyCode) {
    use crossterm::event::KeyCode;

    match code {
        KeyCode::Left | KeyCode::Char('h') => match app.proxy_pane {
            app::ProxyPane::Groups => app.focus_nav(),
            app::ProxyPane::Proxies => app.set_proxy_pane(app::ProxyPane::Groups),
        },
        KeyCode::Right | KeyCode::Char('l') if app.proxy_pane == app::ProxyPane::Groups => {
            app.set_proxy_pane(app::ProxyPane::Proxies);
        }
        KeyCode::Esc => app.focus_nav(),
        KeyCode::Down | KeyCode::Char('j') => match app.proxy_pane {
            app::ProxyPane::Groups => app.next_group(),
            app::ProxyPane::Proxies => app.next_proxy(),
        },
        KeyCode::Up | KeyCode::Char('k') => match app.proxy_pane {
            app::ProxyPane::Groups => app.previous_group(),
            app::ProxyPane::Proxies => app.previous_proxy(),
        },
        KeyCode::Enter => match app.proxy_pane {
            app::ProxyPane::Groups => app.set_proxy_pane(app::ProxyPane::Proxies),
            app::ProxyPane::Proxies => {
                if let Some(group_name) = app.get_selected_group_name()
                    && let Some(proxy_name) = app.get_selected_proxy_name()
                {
                    let group_name = group_name.clone();
                    match app.select_proxy(&group_name, &proxy_name).await {
                        Ok(()) => {
                            let _ = app.fetch_proxies().await;
                            app.error = None;
                        }
                        Err(err) => app.error = Some(err.to_string()),
                    }
                }
            }
        },
        _ => {}
    }
}

/// Handle settings table navigation and commit editable or toggleable entries.
async fn handle_settings_key(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut app::App,
    code: crossterm::event::KeyCode,
) {
    use crossterm::event::KeyCode;

    if handle_settings_navigation_key(app, code) {
        return;
    }

    if code == KeyCode::Enter
        && let Some(idx) = app.settings_state.selected()
        && let Some(entry) = app.settings_items.get(idx).cloned()
    {
        match entry {
            ConfigEntry::MixedPort
            | ConfigEntry::BindAddress
            | ConfigEntry::BaseUrl
            | ConfigEntry::ApiSecret
            | ConfigEntry::TestUrl
            | ConfigEntry::TestTimeout
            | ConfigEntry::ConfigPath => begin_setting_edit(app, entry),
            ConfigEntry::ConfigSwitch => match app.refresh_config_candidates() {
                Ok(()) => {
                    app.show_config_picker = true;
                    app.error = None;
                }
                Err(err) => app.error = Some(err.to_string()),
            },
            ConfigEntry::ConfigFile => {
                if let Err(err) = edit_config_file(terminal, app).await {
                    app.error = Some(err.to_string());
                } else {
                    app.error = None;
                }
            }
            _ => {
                if let Err(err) = handle_setting_change(app, entry).await {
                    app.error = Some(err.to_string());
                } else {
                    app.error = None;
                }
            }
        }
    }
}

fn handle_settings_navigation_key(app: &mut app::App, code: crossterm::event::KeyCode) -> bool {
    use crossterm::event::KeyCode;

    match code {
        KeyCode::Esc | KeyCode::Left | KeyCode::Right | KeyCode::Char('h') | KeyCode::Char('l') => {
            app.focus_nav();
            true
        }
        KeyCode::Char('j') | KeyCode::Down => {
            app.next_setting();
            true
        }
        KeyCode::Char('k') | KeyCode::Up => {
            app.previous_setting();
            true
        }
        _ => false,
    }
}

/// Seed the edit popup with the current value for a setting entry.
fn begin_setting_edit(app: &mut app::App, entry: ConfigEntry) {
    app.is_editing = true;
    app.editing_value = match entry {
        ConfigEntry::MixedPort => app
            .config
            .as_ref()
            .map(|config| config.mixed_port.to_string())
            .unwrap_or_default(),
        ConfigEntry::BindAddress => app
            .config
            .as_ref()
            .map(|config| config.bind_address.clone())
            .unwrap_or_default(),
        ConfigEntry::BaseUrl => app.app_settings.base_url.clone(),
        ConfigEntry::ApiSecret => app.app_settings.api_secret.clone(),
        ConfigEntry::TestUrl => app.app_settings.test_url.clone(),
        ConfigEntry::TestTimeout => app.app_settings.test_timeout.to_string(),
        ConfigEntry::ConfigPath | ConfigEntry::ConfigFile => app.config_path.display().to_string(),
        _ => String::new(),
    };
}

async fn switch_config_file(app: &mut app::App, path: &Path) -> Result<()> {
    let path = expand_config_path(path);
    if !path.exists() {
        bail!("Config file not found: {}", path.display());
    }

    let path =
        fs::canonicalize(&path).with_context(|| format!("Failed to resolve {}", path.display()))?;
    let current_base_url = app.app_settings.base_url.clone();
    let current_api_secret = app.app_settings.api_secret.clone();

    app.reload_config_file(&path).await.with_context(|| {
        format!(
            "Failed to tell mihomo to load {}; check external-controller, secret, and SAFE_PATHS",
            path.display()
        )
    })?;

    app.config_path = path;
    app.app_settings.base_url = current_base_url;
    app.app_settings.api_secret = current_api_secret;
    app.save_app_settings()?;
    app.restart_traffic_monitor();
    let _ = app.fetch_config().await;
    let _ = app.fetch_proxies().await;
    app.trigger_latency_test();
    let _ = app.refresh_config_candidates();
    Ok(())
}

fn expand_config_path(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix("~/")
        && let Ok(home) = std::env::var("HOME")
    {
        return PathBuf::from(home).join(rest);
    }
    path.to_path_buf()
}

async fn edit_config_file(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut app::App,
) -> Result<()> {
    let path = app.config_path.clone();

    ratatui::restore();
    let edit_result = edit_config_file_on_disk(&path);
    *terminal = ratatui::init();

    if edit_result? {
        app.reload_config_file(&path)
            .await
            .with_context(|| format!("Failed to reload edited config {}", path.display()))?;
        app.fetch_config().await?;
        app.fetch_proxies().await?;
    }

    Ok(())
}

fn edit_config_file_on_disk(path: &Path) -> Result<bool> {
    if !path.exists() {
        bail!("Config file not found: {}", path.display());
    }

    let initial = config::read_raw(path)?;
    let temp_path = unique_temp_config_path(path);
    fs::write(&temp_path, &initial)
        .with_context(|| format!("Failed to create temp editor file {}", temp_path.display()))?;

    let editor_result = run_external_editor_for_file(&temp_path);
    if let Err(err) = editor_result {
        let _ = fs::remove_file(&temp_path);
        return Err(err);
    }

    let edited = fs::read_to_string(&temp_path)
        .with_context(|| format!("Failed to read edited temp file {}", temp_path.display()))?;
    let _ = fs::remove_file(&temp_path);

    apply_edited_config_file(path, &initial, &edited)
}

fn apply_edited_config_file(path: &Path, initial: &str, edited: &str) -> Result<bool> {
    if edited == initial {
        return Ok(false);
    }

    serde_yaml::from_str::<config::MihomoConfig>(edited)
        .with_context(|| "Edited mihomo config is not valid YAML/config")?;

    let backup_path = config::backup_config(path)?;
    config::write_raw(path, edited)
        .with_context(|| format!("Backup saved at {}", backup_path.display()))?;
    Ok(true)
}

fn run_external_editor_for_file(path: &Path) -> Result<()> {
    let editor = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| "vi".to_string());
    let command = format!("{} {}", editor, shell_quote_path(path));
    let status = Command::new("sh")
        .arg("-c")
        .arg(&command)
        .status()
        .with_context(|| format!("Failed to launch editor command: {command}"))?;

    if !status.success() {
        bail!("Editor exited with status: {}", status);
    }

    Ok(())
}

fn shell_quote_path(path: &Path) -> String {
    let text = path.to_string_lossy();
    format!("'{}'", text.replace('\'', "'\\''"))
}

fn unique_temp_config_path(path: &Path) -> PathBuf {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("yaml");
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    std::env::temp_dir().join(format!(
        "mihomot-config-edit-{}-{}.{}",
        std::process::id(),
        nanos,
        extension
    ))
}

async fn handle_setting_change(app: &mut app::App, entry: ConfigEntry) -> Result<()> {
    if let Some(config) = &app.config {
        match entry {
            ConfigEntry::Mode => {
                let new_mode = match config.mode.as_str() {
                    "rule" => "global",
                    "global" => "direct",
                    _ => "rule",
                };
                app.update_config(serde_json::json!({ "mode": new_mode }))
                    .await?;
            }
            ConfigEntry::Tun => {
                let new_state = !config.tun.enable;
                app.update_config(serde_json::json!({ "tun": { "enable": new_state } }))
                    .await?;
            }
            ConfigEntry::LogLevel => {
                let new_level = match config.log_level.as_str() {
                    "info" => "warning",
                    "warning" => "error",
                    "error" => "debug",
                    "debug" => "silent",
                    _ => "info",
                };
                app.update_config(serde_json::json!({ "log-level": new_level }))
                    .await?;
            }
            ConfigEntry::AllowLan => {
                let new_state = !config.allow_lan;
                app.update_config(serde_json::json!({ "allow-lan": new_state }))
                    .await?;
            }
            ConfigEntry::Ipv6 => {
                let new_state = !config.ipv6;
                app.update_config(serde_json::json!({ "ipv6": new_state }))
                    .await?;
            }
            _ => {}
        }
    }
    Ok(())
}

async fn commit_edit(app: &mut app::App) -> Result<()> {
    use app::ConfigEntry;

    if let Some(idx) = app.settings_state.selected()
        && let Some(entry) = app.settings_items.get(idx).cloned()
    {
        match entry {
            ConfigEntry::MixedPort => {
                let port = app
                    .editing_value
                    .trim()
                    .parse::<u16>()
                    .with_context(|| "Mixed Port must be a number between 0 and 65535")?;
                app.update_config(serde_json::json!({ "mixed-port": port }))
                    .await?;
            }
            ConfigEntry::BindAddress => {
                app.update_config(serde_json::json!({ "bind-address": app.editing_value }))
                    .await?;
            }
            ConfigEntry::BaseUrl => {
                app.app_settings.base_url = app.editing_value.clone();
                app.save_app_settings()?;
                app.restart_traffic_monitor();
                let _ = app.fetch_proxies().await;
                let _ = app.fetch_config().await;
            }
            ConfigEntry::ApiSecret => {
                app.app_settings.api_secret = app.editing_value.clone();
                app.save_app_settings()?;
                app.restart_traffic_monitor();
                let _ = app.fetch_proxies().await;
                let _ = app.fetch_config().await;
            }
            ConfigEntry::TestUrl => {
                app.app_settings.test_url = app.editing_value.clone();
                app.save_app_settings()?;
                app.trigger_latency_test();
            }
            ConfigEntry::TestTimeout => {
                let timeout =
                    app.editing_value.trim().parse::<u64>().with_context(
                        || "Test Timeout must be a positive number of milliseconds",
                    )?;
                app.app_settings.test_timeout = timeout;
                app.save_app_settings()?;
            }
            ConfigEntry::ConfigPath => {
                let path = PathBuf::from(app.editing_value.trim());
                switch_config_file(app, &path).await?;
            }
            _ => {}
        }
    }
    Ok(())
}

/// Find a non-loopback server IP
fn find_server_ip() -> Option<String> {
    local_ip_address::local_ip().ok().map(|ip| ip.to_string())
}

fn parse_listen_addr(listen: &str) -> (String, u16) {
    let default_host = "0.0.0.0".to_string();
    let default_port = 9091;

    if let Some((host, port)) = listen.rsplit_once(':') {
        return (
            host.trim_matches(['[', ']']).to_string(),
            port.parse().unwrap_or(default_port),
        );
    }

    (default_host, listen.parse().unwrap_or(default_port))
}

async fn detect_public_ip() -> Option<String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .ok()?;

    for url in [
        "https://api.ipify.org",
        "https://ifconfig.me/ip",
        "https://icanhazip.com",
    ] {
        if let Ok(resp) = client.get(url).send().await
            && resp.status().is_success()
            && let Ok(text) = resp.text().await
        {
            let ip = text.trim();
            if ip.parse::<std::net::IpAddr>().is_ok() {
                return Some(ip.to_string());
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::{
        apply_edited_config_file, expand_config_path, handle_nav_key, handle_proxies_key,
        handle_settings_navigation_key, parse_listen_addr, switch_config_file,
        unique_temp_config_path,
    };
    use anyhow::Result;
    use crossterm::event::KeyCode;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    async fn spawn_status_server(status_line: &'static str) -> Result<String> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = [0_u8; 1024];
                let _ = stream.read(&mut buf).await;
                let response = format!("{status_line}\r\nContent-Length: 0\r\n\r\n");
                let _ = stream.write_all(response.as_bytes()).await;
            }
        });
        Ok(format!("http://{addr}"))
    }

    #[test]
    fn parse_listen_addr_handles_host_port_and_port_only() {
        assert_eq!(
            parse_listen_addr("0.0.0.0:9091"),
            ("0.0.0.0".to_string(), 9091)
        );
        assert_eq!(
            parse_listen_addr("127.0.0.1:8080"),
            ("127.0.0.1".to_string(), 8080)
        );
        assert_eq!(parse_listen_addr("7070"), ("0.0.0.0".to_string(), 7070));
        assert_eq!(parse_listen_addr("[::]:9091"), ("::".to_string(), 9091));
    }

    #[test]
    fn apply_edited_config_file_writes_valid_yaml_and_rejects_invalid_yaml() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "mihomot-apply-config-test-{}-{nanos}.yaml",
            std::process::id()
        ));
        let initial = "mode: rule\nmixed-port: 7890\n";
        fs::write(&path, initial).expect("test config should be writable");

        let edited = "mode: global\nmixed-port: 7891\n";
        let changed = apply_edited_config_file(&path, initial, edited)
            .expect("valid edited config should be written");
        assert!(changed);
        assert_eq!(
            fs::read_to_string(&path).expect("updated config should be readable"),
            edited
        );

        let current = fs::read_to_string(&path).expect("current config should be readable");
        let invalid = "mode: [\n";
        let result = apply_edited_config_file(&path, &current, invalid);
        assert!(result.is_err());
        assert_eq!(
            fs::read_to_string(&path).expect("config should remain readable"),
            current
        );

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn unique_temp_config_path_preserves_extension() {
        let path = unique_temp_config_path(std::path::Path::new("/tmp/config.yaml"));
        assert_eq!(
            path.extension().and_then(|value| value.to_str()),
            Some("yaml")
        );
    }

    #[tokio::test]
    async fn switch_config_keeps_existing_control_endpoint() {
        let server_url = spawn_status_server("HTTP/1.1 204 No Content")
            .await
            .expect("server should start");
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "mihomot-switch-config-{}-{nanos}.yaml",
            std::process::id()
        ));
        fs::write(
            &path,
            "mixed-port: 7890\nexternal-controller: 127.0.0.1:19090\nsecret: changed\n",
        )
        .expect("test config should be writable");

        let mut app = crate::app::App::new(Some(server_url.clone()), Some("current".to_string()));
        app.config_path = path.clone();

        switch_config_file(&mut app, &path)
            .await
            .expect("config switch should call the existing control endpoint");

        assert_eq!(app.app_settings.base_url, server_url);
        assert_eq!(app.app_settings.api_secret, "current");
        assert_eq!(
            app.config_path,
            path.canonicalize().expect("path should resolve")
        );

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn expand_config_path_expands_home_prefix() {
        let home = std::env::var("HOME").expect("HOME should be available in tests");

        assert_eq!(
            expand_config_path(std::path::Path::new("~/sub/config.yaml")),
            std::path::PathBuf::from(home).join("sub/config.yaml")
        );
    }

    #[tokio::test]
    async fn proxies_left_from_groups_returns_to_navigation() {
        let mut app = crate::app::App::new(None, None);
        app.set_proxy_pane(crate::app::ProxyPane::Groups);

        handle_proxies_key(&mut app, KeyCode::Left).await;

        assert_eq!(app.focus, crate::app::Focus::Nav);
        assert_eq!(app.proxy_pane, crate::app::ProxyPane::Groups);
    }

    #[tokio::test]
    async fn settings_horizontal_keys_return_to_navigation() {
        let mut app = crate::app::App::new(None, None);
        app.set_route(crate::app::Route::Settings);

        assert!(handle_settings_navigation_key(&mut app, KeyCode::Right));

        assert_eq!(app.focus, crate::app::Focus::Nav);
    }

    #[tokio::test]
    async fn nav_right_stays_in_sidebar_without_activating_highlighted_route() {
        let mut app = crate::app::App::new(None, None);
        app.next_nav();

        assert!(!handle_nav_key(&mut app, KeyCode::Right));

        assert_eq!(app.route, crate::app::Route::Dashboard);
        assert_eq!(app.focus, crate::app::Focus::Nav);
        assert_eq!(app.nav_index, 1);
    }

    #[tokio::test]
    async fn nav_right_returns_to_active_route_content() {
        let mut app = crate::app::App::new(None, None);
        app.next_nav();
        assert!(!handle_nav_key(&mut app, KeyCode::Enter));
        assert_eq!(app.route, crate::app::Route::Proxies);
        assert_eq!(app.focus, crate::app::Focus::Content);

        handle_proxies_key(&mut app, KeyCode::Left).await;
        assert_eq!(app.focus, crate::app::Focus::Nav);

        assert!(!handle_nav_key(&mut app, KeyCode::Right));

        assert_eq!(app.route, crate::app::Route::Proxies);
        assert_eq!(app.focus, crate::app::Focus::Content);
    }

    #[tokio::test]
    async fn nav_enter_activates_highlighted_route() {
        let mut app = crate::app::App::new(None, None);
        app.next_nav();

        assert!(!handle_nav_key(&mut app, KeyCode::Enter));

        assert_eq!(app.route, crate::app::Route::Proxies);
        assert_eq!(app.focus, crate::app::Focus::Content);
    }

    #[tokio::test]
    async fn refresh_order_keeps_proxy_latency_testing_visible() {
        let mut app = crate::app::App::new(None, None);
        app.group_names = vec!["Auto".to_string()];
        app.group_state.select(Some(0));
        app.proxies.insert(
            "Auto".to_string(),
            crate::app::ProxyItem {
                name: Some("Auto".to_string()),
                proxy_type: Some("Selector".to_string()),
                now: Some("Node A".to_string()),
                all: Some(vec!["Node A".to_string()]),
                extra: serde_json::Map::new(),
            },
        );
        app.proxy_latency.insert(
            "Node A".to_string(),
            crate::app::ProxyLatencyStatus::Success(120),
        );

        app.trigger_group_latency_test();

        assert_eq!(
            app.proxy_latency.get("Node A"),
            Some(&crate::app::ProxyLatencyStatus::Testing)
        );
    }

    #[tokio::test]
    async fn selected_proxy_latency_test_marks_only_selected_node_as_testing() {
        let mut app = crate::app::App::new(None, None);
        app.group_names = vec!["Auto".to_string()];
        app.group_state.select(Some(0));
        app.proxy_state.select(Some(1));
        app.proxies.insert(
            "Auto".to_string(),
            crate::app::ProxyItem {
                name: Some("Auto".to_string()),
                proxy_type: Some("Selector".to_string()),
                now: Some("Node A".to_string()),
                all: Some(vec!["Node A".to_string(), "Node B".to_string()]),
                extra: serde_json::Map::new(),
            },
        );
        app.proxy_latency.insert(
            "Node A".to_string(),
            crate::app::ProxyLatencyStatus::Success(120),
        );
        app.proxy_latency.insert(
            "Node B".to_string(),
            crate::app::ProxyLatencyStatus::Success(240),
        );

        app.trigger_selected_proxy_latency_test();

        assert_eq!(
            app.proxy_latency.get("Node A"),
            Some(&crate::app::ProxyLatencyStatus::Success(120))
        );
        assert_eq!(
            app.proxy_latency.get("Node B"),
            Some(&crate::app::ProxyLatencyStatus::Testing)
        );
    }
}

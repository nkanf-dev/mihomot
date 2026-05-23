use anyhow::{Context, Result};
use app::ConfigEntry;
use clap::{Parser, Subcommand};

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

    // Determine the reachable endpoint for the token
    let mihomo_endpoint = {
        // Try to find a non-loopback IP for the token
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
    println!("我已安装 mihomot，请帮我管理我这个服务器的代理。");
    println!(
        "如果没有安装skill先安装这个skill: {}",
        mihomo::skill_install_url()
    );
    println!("token: {}", token_str);
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
    // Auto-detect url/secret from mihomo config if not provided
    let (url, secret) = match (url, secret) {
        (Some(u), Some(s)) => (Some(u), Some(s)),
        (u, s) => {
            let config_path = match &config_override {
                Some(p) => std::path::PathBuf::from(p),
                None => config::default_config_path(),
            };
            if config_path.exists() {
                if let Ok(mc) = config::read_config(&config_path) {
                    let detected_url = u.or_else(|| {
                        mc.external_controller.map(|ec| {
                            let (host, port) = config::parse_external_controller(&ec);
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

        while let Ok((name, latency)) = app.proxy_test_rx.try_recv() {
            app.proxy_latency.insert(name, Some(latency));
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

            if app.show_info_popup {
                match key.code {
                    KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('i') => {
                        app.show_info_popup = false;
                        app.popup_scroll = 0;
                    }
                    KeyCode::Char('j') | KeyCode::Down => app.scroll_popup_down(),
                    KeyCode::Char('k') | KeyCode::Up => app.scroll_popup_up(),
                    _ => {}
                }
            } else if let app::Focus::Settings = app.focus {
                match key.code {
                    KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('s') => {
                        app.focus = app.previous_focus.clone();
                    }
                    KeyCode::Char('j') | KeyCode::Down => app.next_setting(),
                    KeyCode::Char('k') | KeyCode::Up => app.previous_setting(),
                    KeyCode::Enter => {
                        if let Some(idx) = app.settings_state.selected()
                            && let Some(entry) = app.settings_items.get(idx).cloned()
                        {
                            match entry {
                                ConfigEntry::MixedPort
                                | ConfigEntry::BindAddress
                                | ConfigEntry::BaseUrl
                                | ConfigEntry::ApiSecret
                                | ConfigEntry::TestUrl
                                | ConfigEntry::TestTimeout => {
                                    app.is_editing = true;
                                    if let Some(config) = &app.config {
                                        app.editing_value = match entry {
                                            ConfigEntry::MixedPort => config.mixed_port.to_string(),
                                            ConfigEntry::BindAddress => config.bind_address.clone(),
                                            ConfigEntry::BaseUrl => {
                                                app.app_settings.base_url.clone()
                                            }
                                            ConfigEntry::ApiSecret => {
                                                app.app_settings.api_secret.clone()
                                            }
                                            ConfigEntry::TestUrl => {
                                                app.app_settings.test_url.clone()
                                            }
                                            ConfigEntry::TestTimeout => {
                                                app.app_settings.test_timeout.to_string()
                                            }
                                            _ => String::new(),
                                        };
                                    } else if matches!(
                                        entry,
                                        ConfigEntry::BaseUrl
                                            | ConfigEntry::ApiSecret
                                            | ConfigEntry::TestUrl
                                            | ConfigEntry::TestTimeout
                                    ) {
                                        app.editing_value = match entry {
                                            ConfigEntry::BaseUrl => {
                                                app.app_settings.base_url.clone()
                                            }
                                            ConfigEntry::ApiSecret => {
                                                app.app_settings.api_secret.clone()
                                            }
                                            ConfigEntry::TestUrl => {
                                                app.app_settings.test_url.clone()
                                            }
                                            ConfigEntry::TestTimeout => {
                                                app.app_settings.test_timeout.to_string()
                                            }
                                            _ => String::new(),
                                        };
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
                    _ => {}
                }
            } else {
                match key.code {
                    KeyCode::Char('q') => return Ok(()),
                    KeyCode::Char('r') => {
                        if let app::Focus::Proxies = app.focus {
                            app.trigger_group_latency_test();
                        }
                        let _ = app.fetch_proxies().await;
                        let _ = app.fetch_config().await;
                    }
                    KeyCode::Char('t') => {
                        app.trigger_latency_test();
                    }
                    KeyCode::Char('s') => {
                        app.previous_focus = app.focus.clone();
                        app.focus = app::Focus::Settings;
                    }
                    KeyCode::Char('i') => {
                        if let app::Focus::Proxies = app.focus {
                            app.show_info_popup = true;
                        }
                    }
                    KeyCode::Down | KeyCode::Char('j') => match app.focus {
                        app::Focus::Groups => app.next_group(),
                        app::Focus::Proxies => app.next_proxy(),
                        _ => {}
                    },
                    KeyCode::Up | KeyCode::Char('k') => match app.focus {
                        app::Focus::Groups => app.previous_group(),
                        app::Focus::Proxies => app.previous_proxy(),
                        _ => {}
                    },
                    KeyCode::Right | KeyCode::Char('l') => {
                        app.focus = app::Focus::Proxies;
                    }
                    KeyCode::Left | KeyCode::Char('h') | KeyCode::Esc => {
                        app.focus = app::Focus::Groups;
                    }
                    KeyCode::Enter => {
                        if let app::Focus::Proxies = app.focus {
                            if let Some(group_name) = app.get_selected_group_name()
                                && let Some(proxy_name) = app.get_selected_proxy_name()
                            {
                                let g_name = group_name.clone();
                                let p_name = proxy_name.clone();
                                match app.select_proxy(&g_name, &p_name).await {
                                    Ok(()) => {
                                        let _ = app.fetch_proxies().await;
                                        app.error = None;
                                    }
                                    Err(err) => app.error = Some(err.to_string()),
                                }
                            }
                        } else {
                            app.focus = app::Focus::Proxies;
                        }
                    }
                    _ => {}
                }
            }
        }
    }
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
                if let Ok(port) = app.editing_value.parse::<u16>() {
                    app.update_config(serde_json::json!({ "mixed-port": port }))
                        .await?;
                }
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
                if let Ok(timeout) = app.editing_value.parse::<u64>() {
                    app.app_settings.test_timeout = timeout;
                    app.save_app_settings()?;
                }
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

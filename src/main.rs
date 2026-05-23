use anyhow::Result;
use app::ConfigEntry;
use clap::{Parser, Subcommand};
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};

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
    /// Start a temporary Cloudflare Tunnel for the mihomot API
    Tunnel {
        /// Local mihomot API URL to expose
        #[arg(short = 'U', long, default_value = "http://127.0.0.1:9091")]
        url: String,
        /// Config file path for reading the mihomo secret
        #[arg(short, long)]
        config: Option<String>,
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
        Commands::Tunnel { url, config } => run_tunnel(url, config).await,
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
    let mihomo_config = match config::read_config(&config_path) {
        Ok(config) => config,
        Err(err) => {
            eprintln!("Failed to read mihomo config at: {}", config_path.display());
            eprintln!("Error: {err:#}");
            if config_path == std::path::Path::new("/etc/mihomo/config.yaml") {
                eprintln!("This install config is usually root-readable only.");
                eprintln!("Start the server with: sudo mihomot");
                eprintln!("For local management, run: mihomot tui");
            }
            std::process::exit(1);
        }
    };

    let (host, port) = config::parse_external_controller(
        mihomo_config
            .external_controller
            .as_deref()
            .unwrap_or("0.0.0.0:9090"),
    );
    let secret = mihomo_config.secret.unwrap_or_default();

    let (listen_host, listen_port) = parse_listen_addr(&listen);

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

    // Ensure mihomo is available (detect or auto-install)
    let runtime = mihomo::ensure_mihomo(&config_path).await?;
    println!("mihomo runtime: {:?}", runtime);

    let mut mihomo_ready = mihomo::check_alive(&mihomo_endpoint, &secret)
        .await
        .unwrap_or(false);
    if !mihomo_ready {
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
                    mihomo_ready = true;
                    break;
                }
            }
        }
    }

    if mihomo_ready {
        print_startup_agent_block(&token_str, listen_host, listen_port).await;
    } else {
        eprintln!(
            "mihomo is still not responding; agent instructions will be printed after it is healthy."
        );
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
                None => config::user_config_path(),
            };
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
    interface_ipv4_addrs()
        .into_iter()
        .filter(|(iface, _)| !is_mihomo_tun_iface(iface))
        .map(|(_, ip)| ip)
        .find(|ip| is_usable_local_ipv4(ip))
        .or_else(|| {
            local_ip_address::local_ip()
                .ok()
                .map(|ip| ip.to_string())
                .filter(|ip| is_usable_local_ipv4(ip))
        })
}

async fn run_tunnel(local_url: String, config_override: Option<String>) -> Result<()> {
    let state_dir = mihomot_state_dir()?;
    fs::create_dir_all(&state_dir)?;

    let pid_path = state_dir.join("cloudflared.pid");
    let url_path = state_dir.join("cloudflared.url");
    let log_path = state_dir.join("cloudflared.log");
    let secret = read_secret_for_tunnel(config_override)?;

    let cached_pid = fs::read_to_string(&pid_path)
        .ok()
        .and_then(|pid_text| pid_text.trim().parse::<u32>().ok());
    let cached_pid_alive = cached_pid.map(process_is_running).unwrap_or(false);

    if let Ok(url) = fs::read_to_string(&url_path) {
        let url = url.trim().to_string();
        if !url.is_empty() && tunnel_url_is_usable(&url, &secret).await {
            if let Some(pid) = cached_pid {
                println!("Reusing existing Cloudflare Tunnel (pid {}).", pid);
            } else {
                println!("Reusing existing Cloudflare Tunnel.");
            }
            print_agent_block(
                &url,
                &secret,
                Some(
                    "这是临时 Cloudflare Tunnel 入口；已确认当前 endpoint 可以访问 mihomot。进程停止或重启后该 endpoint 会失效，需要重新运行 sudo mihomot tunnel 并把新的 endpoint 发给 agent。",
                ),
            );
            if let Some(pid) = cached_pid {
                println!("cloudflared pid: {}", pid);
                println!("stop tunnel: kill {}", pid);
            }
            println!("cloudflared log: {}", log_path.display());
            return Ok(());
        }

        println!("Cached Cloudflare Tunnel is not reachable; starting a fresh one.");
        if cached_pid_alive && let Some(pid) = cached_pid {
            stop_process(pid);
        }
        cleanup_tunnel_state(&pid_path, &url_path)?;
    } else if cached_pid_alive {
        println!("Existing Cloudflare Tunnel state is incomplete; starting a fresh one.");
        if let Some(pid) = cached_pid {
            stop_process(pid);
        }
        cleanup_tunnel_state(&pid_path, &url_path)?;
    }

    let cloudflared = ensure_cloudflared(&state_dir).await?;
    let log_file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&log_path)?;
    let err_file = log_file.try_clone()?;

    let child = Command::new(&cloudflared)
        .args(["tunnel", "--url", &local_url])
        .stdout(Stdio::from(log_file))
        .stderr(Stdio::from(err_file))
        .spawn()?;
    let pid = child.id();
    fs::write(&pid_path, pid.to_string())?;

    let tunnel_url = wait_for_tunnel_url(&log_path).await?;
    if let Err(err) = wait_for_tunnel_ready(&tunnel_url, &secret, &log_path).await {
        stop_process(pid);
        cleanup_tunnel_state(&pid_path, &url_path)?;
        return Err(err);
    }
    fs::write(&url_path, &tunnel_url)?;

    println!("Started temporary Cloudflare Tunnel in the background.");
    print_agent_block(
        &tunnel_url,
        &secret,
        Some(
            "这是临时 Cloudflare Tunnel 入口；不需要开放 9091 端口，且已确认当前 endpoint 可以访问 mihomot。cloudflared 进程停止或重启后该 endpoint 会失效，需要重新运行 sudo mihomot tunnel 并把新的 endpoint 发给 agent。",
        ),
    );
    println!("cloudflared pid: {}", pid);
    println!("cloudflared log: {}", log_path.display());
    println!("stop tunnel: kill {}", pid);

    Ok(())
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

async fn print_startup_agent_block(token_str: &str, listen_host: String, listen_port: u16) {
    let public_ip = detect_public_ip().await;
    let server_host = public_ip
        .clone()
        .or_else(find_server_ip)
        .unwrap_or_else(|| "127.0.0.1".to_string());
    let agent_endpoint = format!("http://{}:{}", server_host, listen_port);

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
}

async fn detect_public_ip() -> Option<String> {
    if let Some(ip) = detect_interface_public_ip() {
        return Some(ip);
    }

    if let Some(ip) = detect_metadata_public_ip().await {
        return Some(ip);
    }

    if mihomo_tun_is_present() {
        return None;
    }

    detect_online_public_ip().await
}

fn detect_interface_public_ip() -> Option<String> {
    if let Ok(ip) = std::env::var("MIHOMOT_PUBLIC_IP") {
        let ip = ip.trim();
        if is_public_ipv4(ip) {
            return Some(ip.to_string());
        }
    }

    interface_ipv4_addrs()
        .into_iter()
        .filter(|(iface, _)| !is_mihomo_tun_iface(iface))
        .map(|(_, ip)| ip)
        .find(|ip| is_public_ipv4(ip))
}

async fn detect_metadata_public_ip() -> Option<String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .no_proxy()
        .build()
        .ok()?;

    for url in [
        "http://100.100.100.200/latest/meta-data/eipv4",
        "http://100.100.100.200/latest/meta-data/public-ipv4",
        "http://169.254.169.254/latest/meta-data/public-ipv4",
    ] {
        if let Ok(resp) = client.get(url).send().await
            && resp.status().is_success()
            && let Ok(text) = resp.text().await
        {
            let ip = text.trim();
            if is_public_ipv4(ip) {
                return Some(ip.to_string());
            }
        }
    }

    None
}

async fn detect_online_public_ip() -> Option<String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .no_proxy()
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
            if is_public_ipv4(ip) {
                return Some(ip.to_string());
            }
        }
    }

    None
}

fn mihomo_tun_is_present() -> bool {
    Command::new("ip")
        .args(["link", "show", "Meta"])
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn interface_ipv4_addrs() -> Vec<(String, String)> {
    let Ok(output) = Command::new("ip")
        .args(["-o", "-4", "addr", "show", "scope", "global"])
        .output()
    else {
        return Vec::new();
    };

    if !output.status.success() {
        return Vec::new();
    }

    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let _index = fields.next()?;
            let iface = fields.next()?.to_string();
            let family = fields.next()?;
            let cidr = fields.next()?;
            if family != "inet" {
                return None;
            }
            let ip = cidr.split('/').next()?.to_string();
            Some((iface, ip))
        })
        .collect()
}

fn is_mihomo_tun_iface(iface: &str) -> bool {
    iface == "Meta" || iface.starts_with("Meta@")
}

fn is_usable_local_ipv4(ip: &str) -> bool {
    let Ok(addr) = ip.parse::<std::net::Ipv4Addr>() else {
        return false;
    };

    !(addr.is_loopback()
        || addr.is_link_local()
        || addr.is_broadcast()
        || addr.is_documentation()
        || addr.is_unspecified()
        || addr.octets()[0] == 0
        || addr.octets()[0] >= 224
        || (addr.octets()[0] == 198 && (18..=19).contains(&addr.octets()[1]))
        || (addr.octets()[0] == 100 && (64..=127).contains(&addr.octets()[1])))
}

fn is_public_ipv4(ip: &str) -> bool {
    let Ok(addr) = ip.parse::<std::net::Ipv4Addr>() else {
        return false;
    };

    !(addr.is_private()
        || addr.is_loopback()
        || addr.is_link_local()
        || addr.is_broadcast()
        || addr.is_documentation()
        || addr.is_unspecified()
        || addr.octets()[0] == 0
        || addr.octets()[0] >= 224
        || (addr.octets()[0] == 198 && (18..=19).contains(&addr.octets()[1]))
        || (addr.octets()[0] == 100 && (64..=127).contains(&addr.octets()[1])))
}

fn mihomot_state_dir() -> Result<PathBuf> {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    Ok(PathBuf::from(home).join(".config").join("mihomot"))
}

fn process_is_running(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn stop_process(pid: u32) {
    let _ = Command::new("kill").arg(pid.to_string()).status();
}

fn cleanup_tunnel_state(pid_path: &std::path::Path, url_path: &std::path::Path) -> Result<()> {
    match fs::remove_file(pid_path) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(err.into()),
    }

    match fs::remove_file(url_path) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(err.into()),
    }

    Ok(())
}

fn read_secret_for_tunnel(config_override: Option<String>) -> Result<String> {
    let settings_secret = app::App::load_app_settings().api_secret;
    if !settings_secret.trim().is_empty() && settings_secret != "mihomo" {
        return Ok(settings_secret);
    }

    let config_path = config_override
        .map(PathBuf::from)
        .unwrap_or_else(config::default_config_path);
    let secret = config::read_config(&config_path)
        .ok()
        .and_then(|config| config.secret)
        .or_else(|| std::env::var("MIHOMO_SECRET").ok())
        .unwrap_or_default();

    if secret.trim().is_empty() {
        anyhow::bail!(
            "mihomo secret not found. Run the install/upgrade script to write ~/.config/mihomot/settings.json, set MIHOMO_SECRET, or run with sudo -E mihomot tunnel -c /etc/mihomo/config.yaml"
        );
    }

    Ok(secret)
}

async fn ensure_cloudflared(state_dir: &std::path::Path) -> Result<PathBuf> {
    if let Ok(output) = Command::new("which").arg("cloudflared").output()
        && output.status.success()
    {
        let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !path.is_empty() {
            return Ok(PathBuf::from(path));
        }
    }

    let target = cloudflared_target()?;
    let bin_path = state_dir.join("cloudflared");
    if bin_path.is_file() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&bin_path, fs::Permissions::from_mode(0o755))?;
        }
        return Ok(bin_path);
    }

    println!("cloudflared not found; downloading {}...", target);

    let url = format!(
        "https://github.com/cloudflare/cloudflared/releases/latest/download/{}",
        target
    );
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(900))
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()?;
    let mut last_err = None;
    for ranked_url in mihomo::ranked_github_urls(&client, &url).await {
        println!("Trying cloudflared: {}", ranked_url);
        match client.get(&ranked_url).send().await {
            Ok(resp) if resp.status().is_success() => {
                let bytes = match mihomo::download_response_with_progress(resp, "cloudflared").await
                {
                    Ok(bytes) => bytes,
                    Err(err) => {
                        last_err = Some(format!("download body error from {ranked_url}: {err}"));
                        continue;
                    }
                };
                fs::write(&bin_path, &bytes)?;

                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    fs::set_permissions(&bin_path, fs::Permissions::from_mode(0o755))?;
                }

                return Ok(bin_path);
            }
            Ok(resp) => {
                last_err = Some(format!("HTTP {}", resp.status()));
            }
            Err(err) => {
                last_err = Some(err.to_string());
            }
        }
    }

    anyhow::bail!(
        "failed to download cloudflared. Last error: {}",
        last_err.unwrap_or_else(|| "unknown".to_string())
    );
}

async fn tunnel_url_is_usable(url: &str, secret: &str) -> bool {
    probe_tunnel_url(url, secret).await.is_ok()
}

async fn wait_for_tunnel_ready(url: &str, secret: &str, log_path: &std::path::Path) -> Result<()> {
    let mut last_err = "not probed".to_string();

    for _ in 0..20 {
        match probe_tunnel_url(url, secret).await {
            Ok(()) => return Ok(()),
            Err(err) => last_err = err.to_string(),
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }

    anyhow::bail!(
        "Cloudflare Tunnel URL was found, but mihomot API is not reachable through it ({last_err}); see {}",
        log_path.display()
    )
}

async fn probe_tunnel_url(url: &str, secret: &str) -> Result<()> {
    let status_url = format!("{}/mhmt/status", url.trim_end_matches('/'));
    let resp = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()?
        .get(status_url)
        .bearer_auth(secret)
        .send()
        .await?;

    if resp.status().is_success() {
        return Ok(());
    }

    anyhow::bail!("HTTP {}", resp.status())
}

fn cloudflared_target() -> Result<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Ok("cloudflared-linux-amd64"),
        ("linux", "aarch64") => Ok("cloudflared-linux-arm64"),
        (os, arch) => anyhow::bail!("unsupported cloudflared platform: {os}/{arch}"),
    }
}

async fn wait_for_tunnel_url(log_path: &std::path::Path) -> Result<String> {
    for _ in 0..30 {
        if let Ok(file) = fs::File::open(log_path) {
            let reader = BufReader::new(file);
            for line in reader.lines().map_while(Result::ok) {
                if let Some(url) = extract_trycloudflare_url(&line) {
                    return Ok(url);
                }
            }
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }

    anyhow::bail!(
        "timed out waiting for cloudflared tunnel URL; see {}",
        log_path.display()
    )
}

fn extract_trycloudflare_url(line: &str) -> Option<String> {
    line.split_whitespace()
        .find(|part| part.starts_with("https://") && part.contains(".trycloudflare.com"))
        .map(|part| {
            part.trim_matches(|c: char| c == '"' || c == '\'' || c == ',' || c == '.')
                .to_string()
        })
}

fn print_agent_block(endpoint: &str, secret: &str, note: Option<&str>) {
    let token_str =
        token::generate_token(secret).unwrap_or_else(|_| "TOKEN_GENERATION_FAILED".into());

    println!();
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("把这段话发给你的 AI agent:");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!();
    println!("我已安装 mihomot，请帮我管理我这个服务器的代理。");
    println!("mihomot endpoint: {}", endpoint);
    println!(
        "如果没有安装skill先安装这个skill: {}",
        mihomo::skill_install_url()
    );
    println!("token: {}", token_str);
    if let Some(note) = note {
        println!();
        println!("临时入口提示: {}", note);
    }
    println!();
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!();
}

#[cfg(test)]
mod tests {
    use super::{
        extract_trycloudflare_url, is_public_ipv4, is_usable_local_ipv4, parse_listen_addr,
    };

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
    fn extract_trycloudflare_url_from_cloudflared_log_line() {
        let line = r#"INF | https://quiet-river-123.trycloudflare.com |"#;
        assert_eq!(
            extract_trycloudflare_url(line).as_deref(),
            Some("https://quiet-river-123.trycloudflare.com")
        );
    }

    #[test]
    fn ip_filters_reject_tun_and_proxy_ranges() {
        assert!(is_public_ipv4("1.1.1.1"));
        assert!(!is_public_ipv4("172.28.205.178"));
        assert!(!is_public_ipv4("198.18.0.1"));
        assert!(!is_public_ipv4("100.64.0.1"));

        assert!(is_usable_local_ipv4("172.28.205.178"));
        assert!(!is_usable_local_ipv4("198.18.0.1"));
        assert!(!is_usable_local_ipv4("100.64.0.1"));
    }
}

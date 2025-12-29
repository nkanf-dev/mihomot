use anyhow::Result;
use clap::Parser;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::DefaultTerminal;
use std::io::stdout;

mod app;
mod ui;

use app::{App, ConfigEntry, Focus};

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Temporary API URL to use
    #[arg(short = 'U', long)]
    url: Option<String>,

    /// Temporary API Secret to use
    #[arg(short = 'S', long)]
    secret: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let mut terminal = ratatui::init();

    // Create app and fetch initial data
    let mut app = App::new(args.url, args.secret);
    let _ = app.fetch_proxies().await;
    let _ = app.fetch_config().await;
    app.trigger_latency_test();

    let app_result = run_app(&mut terminal, &mut app).await;

    // Restore terminal
    ratatui::restore();
    execute!(stdout, LeaveAlternateScreen)?;
    disable_raw_mode()?;

    app_result
}

async fn run_app(terminal: &mut DefaultTerminal, app: &mut App) -> Result<()> {
    loop {
        terminal.draw(|f| ui::draw(f, app))?;

        // Check for real latency updates
        if let Ok(status) = app.real_latency_rx.try_recv() {
            app.real_latency_status = status;
        }

        // Check for proxy latency updates
        while let Ok((name, latency)) = app.proxy_test_rx.try_recv() {
            app.proxy_latency.insert(name, Some(latency));
        }

        // Check for traffic updates
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
                        let _ = commit_edit(app).await;
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
            } else if let Focus::Settings = app.focus {
                match key.code {
                    KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('s') => {
                        app.focus = app.previous_focus.clone(); // Return to previous view
                    }
                    KeyCode::Char('j') | KeyCode::Down => app.next_setting(),
                    KeyCode::Char('k') | KeyCode::Up => app.previous_setting(),
                    KeyCode::Enter => {
                        // Handle config change
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
                                        // Fallback if config is not loaded yet (e.g. wrong URL initially)
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
                                    let _ = handle_setting_change(app, entry).await;
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
                        if let Focus::Proxies = app.focus {
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
                        app.focus = Focus::Settings;
                    }
                    KeyCode::Char('i') => {
                        if let Focus::Proxies = app.focus {
                            app.show_info_popup = true;
                        }
                    }
                    KeyCode::Down | KeyCode::Char('j') => match app.focus {
                        Focus::Groups => app.next_group(),
                        Focus::Proxies => app.next_proxy(),
                        _ => {}
                    },
                    KeyCode::Up | KeyCode::Char('k') => match app.focus {
                        Focus::Groups => app.previous_group(),
                        Focus::Proxies => app.previous_proxy(),
                        _ => {}
                    },
                    KeyCode::Right | KeyCode::Char('l') => {
                        app.focus = Focus::Proxies;
                    }
                    KeyCode::Left | KeyCode::Char('h') | KeyCode::Esc => {
                        app.focus = Focus::Groups;
                    }
                    KeyCode::Enter => {
                        if let Focus::Proxies = app.focus {
                            if let Some(group_name) = app.get_selected_group_name()
                                && let Some(proxy_name) = app.get_selected_proxy_name()
                            {
                                let g_name = group_name.clone();
                                let p_name = proxy_name.clone();
                                let _ = app.select_proxy(&g_name, &p_name).await;
                                let _ = app.fetch_proxies().await;
                            }
                        } else {
                            app.focus = Focus::Proxies;
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}

async fn handle_setting_change(app: &mut App, entry: ConfigEntry) -> Result<()> {
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

async fn commit_edit(app: &mut App) -> Result<()> {
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
                let _ = app.save_app_settings();
                let _ = app.fetch_proxies().await;
                let _ = app.fetch_config().await;
            }
            ConfigEntry::ApiSecret => {
                app.app_settings.api_secret = app.editing_value.clone();
                let _ = app.save_app_settings();
                let _ = app.fetch_proxies().await;
                let _ = app.fetch_config().await;
            }
            ConfigEntry::TestUrl => {
                app.app_settings.test_url = app.editing_value.clone();
                let _ = app.save_app_settings();
                app.trigger_latency_test();
            }
            ConfigEntry::TestTimeout => {
                if let Ok(timeout) = app.editing_value.parse::<u64>() {
                    app.app_settings.test_timeout = timeout;
                    let _ = app.save_app_settings();
                }
            }
            _ => {}
        }
    }
    Ok(())
}

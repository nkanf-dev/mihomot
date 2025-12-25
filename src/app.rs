use anyhow::Result;
use ratatui::widgets::{ListState, TableState};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;
use tokio::sync::mpsc;

#[derive(Debug, Deserialize, Clone)]
#[allow(dead_code)]
pub struct Tun {
    pub enable: bool,
    pub stack: Option<String>,
    pub device: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Config {
    pub mode: String,
    pub tun: Tun,
    #[serde(rename = "mixed-port")]
    pub mixed_port: u16,
    #[serde(rename = "log-level")]
    pub log_level: String,
    #[serde(rename = "allow-lan")]
    pub allow_lan: bool,
    #[serde(rename = "bind-address")]
    pub bind_address: String,
    pub ipv6: bool,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ProxyItem {
    pub name: Option<String>,
    #[serde(rename = "type")]
    pub proxy_type: Option<String>,
    pub now: Option<String>,
    pub all: Option<Vec<String>>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct ProxiesResponse {
    pub proxies: HashMap<String, ProxyItem>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AppSettings {
    #[serde(default = "default_base_url")]
    pub base_url: String,
    #[serde(default = "default_api_secret")]
    pub api_secret: String,
    #[serde(default = "default_test_url")]
    pub test_url: String,
    #[serde(default = "default_test_timeout")]
    pub test_timeout: u64,
}

fn default_base_url() -> String {
    "http://127.0.0.1:9090".to_string()
}

fn default_api_secret() -> String {
    std::env::var("MIHOMO_SECRET").unwrap_or_else(|_| "mihomo".to_string())
}

fn default_test_url() -> String {
    "https://www.google.com".to_string()
}

fn default_test_timeout() -> u64 {
    3000
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            base_url: default_base_url(),
            api_secret: default_api_secret(),
            test_url: default_test_url(),
            test_timeout: default_test_timeout(),
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
pub enum LatencyStatus {
    Pending,
    Testing,
    Success(u64),
    Failed(String),
}

#[derive(Clone, PartialEq)]
pub enum Focus {
    Groups,
    Proxies,
    Settings,
}

#[derive(Clone, PartialEq, Debug)]
pub enum ConfigEntry {
    // App Settings
    BaseUrl,
    ApiSecret,
    TestUrl,
    TestTimeout,
    // Mihomo Config
    Mode,
    Tun,
    MixedPort,
    LogLevel,
    AllowLan,
    BindAddress,
    Ipv6,
}

pub struct App {
    pub proxies: HashMap<String, ProxyItem>,
    pub config: Option<Config>,
    pub latency_status: LatencyStatus,
    pub client: Client,
    pub app_settings: AppSettings,

    // Async Task Communication
    pub latency_tx: mpsc::Sender<LatencyStatus>,
    pub latency_rx: mpsc::Receiver<LatencyStatus>,

    // UI State
    pub group_names: Vec<String>,
    pub group_state: ListState,
    pub proxy_state: ListState,
    pub focus: Focus,
    pub previous_focus: Focus,
    pub show_info_popup: bool,
    pub popup_scroll: u16,

    // Settings State
    pub settings_items: Vec<ConfigEntry>,
    pub settings_state: TableState,
    pub is_editing: bool,
    pub editing_value: String,

    pub error: Option<String>,
}

impl App {
    pub fn new() -> Self {
        let mut group_state = ListState::default();
        let mut proxy_state = ListState::default();
        group_state.select(Some(0));
        proxy_state.select(Some(0));

        let mut settings_state = TableState::default();
        settings_state.select(Some(0));

        let settings_items = vec![
            ConfigEntry::BaseUrl,
            ConfigEntry::ApiSecret,
            ConfigEntry::TestUrl,
            ConfigEntry::TestTimeout,
            ConfigEntry::Mode,
            ConfigEntry::Tun,
            ConfigEntry::MixedPort,
            ConfigEntry::LogLevel,
            ConfigEntry::AllowLan,
            ConfigEntry::BindAddress,
            ConfigEntry::Ipv6,
        ];

        let app_settings = Self::load_app_settings();
        let (latency_tx, latency_rx) = mpsc::channel(10);

        Self {
            proxies: HashMap::new(),
            config: None,
            latency_status: LatencyStatus::Pending,
            client: Client::builder().build().unwrap_or_default(),
            app_settings,
            latency_tx,
            latency_rx,
            group_names: Vec::new(),
            group_state,
            proxy_state,
            focus: Focus::Groups,
            previous_focus: Focus::Groups,
            show_info_popup: false,
            popup_scroll: 0,
            settings_items,
            settings_state,
            is_editing: false,
            editing_value: String::new(),
            error: None,
        }
    }

    fn get_config_path() -> Option<PathBuf> {
        if let Ok(home) = std::env::var("HOME") {
            let mut path = PathBuf::from(home);
            path.push(".config");
            path.push("mihomot");
            let _ = fs::create_dir_all(&path);
            path.push("settings.json");
            Some(path)
        } else {
            None
        }
    }

    fn load_app_settings() -> AppSettings {
        if let Some(path) = Self::get_config_path()
            && path.exists()
            && let Ok(content) = fs::read_to_string(path)
        {
            return serde_json::from_str(&content).unwrap_or_default();
        }
        AppSettings::default()
    }

    pub fn save_app_settings(&self) -> Result<()> {
        if let Some(path) = Self::get_config_path() {
            let json = serde_json::to_string_pretty(&self.app_settings)?;
            fs::write(path, json)?;
        }
        Ok(())
    }

    pub fn scroll_popup_down(&mut self) {
        self.popup_scroll = self.popup_scroll.saturating_add(1);
    }

    pub fn scroll_popup_up(&mut self) {
        self.popup_scroll = self.popup_scroll.saturating_sub(1);
    }

    pub fn next_setting(&mut self) {
        let i = match self.settings_state.selected() {
            Some(i) => {
                if i >= self.settings_items.len() - 1 {
                    0
                } else {
                    i + 1
                }
            }
            None => 0,
        };
        self.settings_state.select(Some(i));
    }

    pub fn previous_setting(&mut self) {
        let i = match self.settings_state.selected() {
            Some(i) => {
                if i == 0 {
                    self.settings_items.len() - 1
                } else {
                    i - 1
                }
            }
            None => 0,
        };
        self.settings_state.select(Some(i));
    }

    pub async fn update_config(&mut self, json_body: serde_json::Value) -> Result<()> {
        let url = format!("{}/configs", self.app_settings.base_url);
        let mut request = self.client.patch(&url).json(&json_body);

        if !self.app_settings.api_secret.is_empty() {
            request = request.bearer_auth(&self.app_settings.api_secret);
        }

        request.send().await?;
        // Fetch updated config to sync UI
        self.fetch_config().await?;
        Ok(())
    }

    pub async fn fetch_proxies(&mut self) -> Result<()> {
        let url = format!("{}/proxies", self.app_settings.base_url);
        let mut request = self.client.get(&url);

        if !self.app_settings.api_secret.is_empty() {
            request = request.bearer_auth(&self.app_settings.api_secret);
        }

        match request.send().await {
            Ok(resp) => {
                if resp.status().is_success() {
                    match resp.json::<ProxiesResponse>().await {
                        Ok(data) => {
                            self.proxies = data.proxies;
                            self.group_names = self
                                .proxies
                                .values()
                                .filter(|p| p.proxy_type.as_deref() == Some("Selector"))
                                .filter_map(|p| p.name.clone())
                                .collect();
                            self.group_names.sort();
                            self.error = None;
                        }
                        Err(e) => self.error = Some(format!("Failed to parse JSON: {}", e)),
                    }
                } else {
                    self.error = Some(format!("Server returned error: {}", resp.status()));
                }
            }
            Err(e) => self.error = Some(format!("Failed to connect: {}", e)),
        }
        Ok(())
    }

    pub async fn fetch_config(&mut self) -> Result<()> {
        let url = format!("{}/configs", self.app_settings.base_url);
        let mut request = self.client.get(&url);
        if !self.app_settings.api_secret.is_empty() {
            request = request.bearer_auth(&self.app_settings.api_secret);
        }
        let resp = request.send().await?;
        if resp.status().is_success() {
            self.config = Some(resp.json::<Config>().await?);
        }
        Ok(())
    }

    pub fn trigger_latency_test(&mut self) {
        let client = self.client.clone();
        let url = self.app_settings.test_url.clone();
        let timeout = self.app_settings.test_timeout;
        let tx = self.latency_tx.clone();

        self.latency_status = LatencyStatus::Testing;

        tokio::spawn(async move {
            use std::time::Instant;
            let start = Instant::now();

            match client
                .head(&url)
                .timeout(Duration::from_millis(timeout))
                .send()
                .await
            {
                Ok(resp) => {
                    if resp.status().is_success() || resp.status().is_redirection() {
                        let delay = start.elapsed().as_millis() as u64;
                        let _ = tx.send(LatencyStatus::Success(delay)).await;
                    } else {
                        let _ = tx
                            .send(LatencyStatus::Failed(format!("Status: {}", resp.status())))
                            .await;
                    }
                }
                Err(e) => {
                    let msg = if e.is_timeout() {
                        "Timeout".to_string()
                    } else if e.is_connect() {
                        "Conn Err".to_string()
                    } else {
                        "Error".to_string()
                    };
                    let _ = tx.send(LatencyStatus::Failed(msg)).await;
                }
            }
        });
    }

    pub async fn select_proxy(&self, group_name: &str, proxy_name: &str) -> Result<()> {
        let url = format!("{}/proxies/{}", self.app_settings.base_url, group_name);
        let body = serde_json::json!({ "name": proxy_name });
        let mut request = self.client.put(&url).json(&body);

        if !self.app_settings.api_secret.is_empty() {
            request = request.bearer_auth(&self.app_settings.api_secret);
        }

        request.send().await?;
        Ok(())
    }

    // Navigation Helpers
    pub fn next_group(&mut self) {
        let i = match self.group_state.selected() {
            Some(i) => {
                if i >= self.group_names.len() - 1 {
                    0
                } else {
                    i + 1
                }
            }
            None => 0,
        };
        self.group_state.select(Some(i));
        self.proxy_state.select(Some(0)); // Reset proxy selection
    }

    pub fn previous_group(&mut self) {
        let i = match self.group_state.selected() {
            Some(i) => {
                if i == 0 {
                    self.group_names.len() - 1
                } else {
                    i - 1
                }
            }
            None => 0,
        };
        self.group_state.select(Some(i));
        self.proxy_state.select(Some(0));
    }

    pub fn next_proxy(&mut self) {
        if let Some(group_idx) = self.group_state.selected()
            && let Some(group_name) = self.group_names.get(group_idx)
            && let Some(group) = self.proxies.get(group_name)
            && let Some(all) = &group.all
        {
            let i = match self.proxy_state.selected() {
                Some(i) => {
                    if i >= all.len() - 1 {
                        0
                    } else {
                        i + 1
                    }
                }
                None => 0,
            };
            self.proxy_state.select(Some(i));
        }
    }

    pub fn previous_proxy(&mut self) {
        if let Some(group_idx) = self.group_state.selected()
            && let Some(group_name) = self.group_names.get(group_idx)
            && let Some(group) = self.proxies.get(group_name)
            && let Some(all) = &group.all
        {
            let i = match self.proxy_state.selected() {
                Some(i) => {
                    if i == 0 {
                        all.len() - 1
                    } else {
                        i - 1
                    }
                }
                None => 0,
            };
            self.proxy_state.select(Some(i));
        }
    }

    pub fn get_selected_group_name(&self) -> Option<&String> {
        self.group_state
            .selected()
            .and_then(|i| self.group_names.get(i))
    }

    pub fn get_selected_proxy_name(&self) -> Option<String> {
        if let Some(group_name) = self.get_selected_group_name()
            && let Some(group) = self.proxies.get(group_name)
            && let Some(all) = &group.all
        {
            return self
                .proxy_state
                .selected()
                .and_then(|i| all.get(i).cloned());
        }
        None
    }
}

use anyhow::{Result, bail};
use futures_util::StreamExt;
use ratatui::widgets::ListState;
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

#[derive(Debug, Deserialize, Clone)]
pub struct Traffic {
    pub up: u64,
    pub down: u64,
}

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
pub enum RealLatencyStatus {
    Pending,
    Testing,
    Success(u64),
    Failed(String),
}

#[derive(Clone, PartialEq, Debug)]
pub enum ProxyLatencyStatus {
    Testing,
    Success(u64),
    Failed(String),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Route {
    Dashboard,
    Proxies,
    Settings,
    Help,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Focus {
    Nav,
    Content,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ProxyPane {
    Groups,
    Proxies,
}

#[derive(Clone, Copy, Debug)]
pub struct NavItem {
    pub label: &'static str,
    pub route: Option<Route>,
}

pub const NAV_ITEMS: [NavItem; 5] = [
    NavItem {
        label: "Dashboard",
        route: Some(Route::Dashboard),
    },
    NavItem {
        label: "Proxies",
        route: Some(Route::Proxies),
    },
    NavItem {
        label: "Settings",
        route: Some(Route::Settings),
    },
    NavItem {
        label: "Help",
        route: Some(Route::Help),
    },
    NavItem {
        label: "Quit",
        route: None,
    },
];

#[derive(Clone, PartialEq, Debug)]
pub enum ConfigEntry {
    BaseUrl,
    ApiSecret,
    TestUrl,
    TestTimeout,
    ConfigPath,
    ConfigSwitch,
    ConfigFile,
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
    pub real_latency_status: RealLatencyStatus,
    pub client: Client,
    pub latency_client: Client,
    pub app_settings: AppSettings,
    pub config_path: PathBuf,

    pub real_latency_tx: mpsc::Sender<RealLatencyStatus>,
    pub real_latency_rx: mpsc::Receiver<RealLatencyStatus>,

    pub proxy_latency: HashMap<String, ProxyLatencyStatus>,
    pub proxy_test_tx: mpsc::Sender<(String, ProxyLatencyStatus)>,
    pub proxy_test_rx: mpsc::Receiver<(String, ProxyLatencyStatus)>,

    pub traffic_tx: mpsc::Sender<Traffic>,
    pub traffic_rx: mpsc::Receiver<Traffic>,

    pub traffic_history_up: VecDeque<u64>,
    pub traffic_history_down: VecDeque<u64>,
    pub current_up: u64,
    pub current_down: u64,

    pub group_names: Vec<String>,
    pub group_state: ListState,
    pub proxy_state: ListState,
    pub route: Route,
    pub focus: Focus,
    pub nav_index: usize,
    pub proxy_pane: ProxyPane,
    pub show_info_popup: bool,
    pub show_config_picker: bool,
    pub popup_scroll: u16,

    pub config_candidates: Vec<crate::config::ConfigCandidate>,
    pub config_picker_state: ListState,
    pub settings_items: Vec<ConfigEntry>,
    pub settings_state: ratatui::widgets::TableState,
    pub is_editing: bool,
    pub editing_value: String,

    pub error: Option<String>,
    traffic_monitor_task: Option<JoinHandle<()>>,
}

struct ProxyLatencyTestContext {
    base_url: String,
    secret: String,
    test_url: String,
    timeout: u64,
    client: Client,
    tx: mpsc::Sender<(String, ProxyLatencyStatus)>,
}

impl App {
    pub fn new(url_override: Option<String>, secret_override: Option<String>) -> Self {
        let mut group_state = ListState::default();
        let mut proxy_state = ListState::default();
        group_state.select(Some(0));
        proxy_state.select(Some(0));

        let mut settings_state = ratatui::widgets::TableState::default();
        settings_state.select(Some(0));
        let mut config_picker_state = ListState::default();
        config_picker_state.select(Some(0));

        let settings_items = vec![
            ConfigEntry::BaseUrl,
            ConfigEntry::ApiSecret,
            ConfigEntry::TestUrl,
            ConfigEntry::TestTimeout,
            ConfigEntry::ConfigPath,
            ConfigEntry::ConfigSwitch,
            ConfigEntry::ConfigFile,
            ConfigEntry::Mode,
            ConfigEntry::Tun,
            ConfigEntry::MixedPort,
            ConfigEntry::LogLevel,
            ConfigEntry::AllowLan,
            ConfigEntry::BindAddress,
            ConfigEntry::Ipv6,
        ];

        let mut app_settings = Self::load_app_settings();
        if let Some(url) = url_override {
            app_settings.base_url = url;
        }
        if let Some(secret) = secret_override {
            app_settings.api_secret = secret;
        }

        let (real_latency_tx, real_latency_rx) = mpsc::channel(10);
        let (traffic_tx, traffic_rx) = mpsc::channel(100);
        let (proxy_test_tx, proxy_test_rx) = mpsc::channel(100);

        let mut app = Self {
            proxies: HashMap::new(),
            config: None,
            real_latency_status: RealLatencyStatus::Pending,
            client: Client::builder().no_proxy().build().unwrap_or_default(),
            latency_client: Client::builder().build().unwrap_or_default(),
            app_settings,
            config_path: crate::config::default_config_path(),
            real_latency_tx,
            real_latency_rx,
            proxy_latency: HashMap::new(),
            proxy_test_tx,
            proxy_test_rx,
            traffic_tx,
            traffic_rx,
            traffic_history_up: VecDeque::from(vec![0; 1000]),
            traffic_history_down: VecDeque::from(vec![0; 1000]),
            current_up: 0,
            current_down: 0,
            group_names: Vec::new(),
            group_state,
            proxy_state,
            route: Route::Dashboard,
            focus: Focus::Nav,
            nav_index: 0,
            proxy_pane: ProxyPane::Groups,
            show_info_popup: false,
            show_config_picker: false,
            popup_scroll: 0,
            config_candidates: Vec::new(),
            config_picker_state,
            settings_items,
            settings_state,
            is_editing: false,
            editing_value: String::new(),
            error: None,
            traffic_monitor_task: None,
        };

        app.restart_traffic_monitor();
        app
    }

    fn start_traffic_monitor(&self) -> JoinHandle<()> {
        let client = self.client.clone();
        let base_url = self.app_settings.base_url.clone();
        let secret = self.app_settings.api_secret.clone();
        let tx = self.traffic_tx.clone();

        tokio::spawn(async move {
            let url = format!("{}/traffic", base_url);
            loop {
                let mut request = client.get(&url);
                if !secret.is_empty() {
                    request = request.bearer_auth(&secret);
                }

                if let Ok(resp) = request.send().await
                    && resp.status().is_success()
                {
                    let mut stream = resp.bytes_stream();
                    let mut buffer = String::new();

                    while let Some(Ok(bytes)) = stream.next().await {
                        if let Ok(text) = std::str::from_utf8(&bytes) {
                            buffer.push_str(text);
                            while let Some(pos) = buffer.find('\n') {
                                let line = buffer[..pos].to_string();
                                buffer = buffer[pos + 1..].to_string();

                                if let Ok(traffic) = serde_json::from_str::<Traffic>(&line)
                                    && tx.send(traffic).await.is_err()
                                {
                                    return;
                                }
                            }
                        }
                    }
                }
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
        })
    }

    pub fn restart_traffic_monitor(&mut self) {
        if let Some(task) = self.traffic_monitor_task.take() {
            task.abort();
        }
        self.traffic_monitor_task = Some(self.start_traffic_monitor());
    }

    pub fn on_traffic(&mut self, traffic: Traffic) {
        self.current_up = traffic.up;
        self.current_down = traffic.down;

        self.traffic_history_up.pop_front();
        self.traffic_history_up.push_back(traffic.up);

        self.traffic_history_down.pop_front();
        self.traffic_history_down.push_back(traffic.down);
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

    pub fn load_app_settings() -> AppSettings {
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

    /// Refresh selectable mihomo YAML config files from the active config directory.
    pub fn refresh_config_candidates(&mut self) -> Result<()> {
        let candidates = crate::config::list_config_candidates(&self.config_path)?
            .into_iter()
            .map(|(c, _)| c)
            .collect::<Vec<_>>();
        let selected = candidates
            .iter()
            .position(|candidate| candidate.path == self.config_path)
            .or(Some(0))
            .filter(|_| !candidates.is_empty());

        self.config_candidates = candidates;
        self.config_picker_state.select(selected);
        Ok(())
    }

    pub fn next_config_candidate(&mut self) {
        if self.config_candidates.is_empty() {
            self.config_picker_state.select(None);
            return;
        }

        let i = match self.config_picker_state.selected() {
            Some(i) if i >= self.config_candidates.len() - 1 => 0,
            Some(i) => i + 1,
            None => 0,
        };
        self.config_picker_state.select(Some(i));
    }

    pub fn previous_config_candidate(&mut self) {
        if self.config_candidates.is_empty() {
            self.config_picker_state.select(None);
            return;
        }

        let i = match self.config_picker_state.selected() {
            Some(0) | None => self.config_candidates.len() - 1,
            Some(i) => i - 1,
        };
        self.config_picker_state.select(Some(i));
    }

    pub fn selected_config_candidate(&self) -> Option<PathBuf> {
        self.config_picker_state
            .selected()
            .and_then(|i| self.config_candidates.get(i))
            .map(|candidate| candidate.path.clone())
    }

    /// Return the sidebar item under the navigation cursor.
    pub fn selected_nav_item(&self) -> NavItem {
        NAV_ITEMS
            .get(self.nav_index)
            .copied()
            .unwrap_or(NAV_ITEMS[0])
    }

    /// Move the sidebar cursor down, wrapping at the end.
    pub fn next_nav(&mut self) {
        self.nav_index = (self.nav_index + 1) % NAV_ITEMS.len();
    }

    /// Move the sidebar cursor up, wrapping at the start.
    pub fn previous_nav(&mut self) {
        self.nav_index = if self.nav_index == 0 {
            NAV_ITEMS.len() - 1
        } else {
            self.nav_index - 1
        };
    }

    /// Switch to a top-level route and keep the sidebar selection in sync.
    pub fn set_route(&mut self, route: Route) {
        self.route = route;
        if let Some(index) = NAV_ITEMS.iter().position(|item| item.route == Some(route)) {
            self.nav_index = index;
        }
        self.focus = Focus::Content;
    }

    /// Activate the current sidebar item; returns true when the item is Quit.
    pub fn activate_nav(&mut self) -> bool {
        if let Some(route) = self.selected_nav_item().route {
            self.set_route(route);
            false
        } else {
            true
        }
    }

    /// Toggle keyboard focus between the sidebar and the active page content.
    pub fn toggle_focus(&mut self) {
        self.focus = match self.focus {
            Focus::Nav => Focus::Content,
            Focus::Content => Focus::Nav,
        };
    }

    /// Move keyboard focus to the sidebar without changing the active route.
    pub fn focus_nav(&mut self) {
        self.focus = Focus::Nav;
    }

    /// Select the active pane inside the Proxies page and make that route visible.
    pub fn set_proxy_pane(&mut self, pane: ProxyPane) {
        self.proxy_pane = pane;
        self.route = Route::Proxies;
        self.focus = Focus::Content;
        self.nav_index = NAV_ITEMS
            .iter()
            .position(|item| item.route == Some(Route::Proxies))
            .unwrap_or(self.nav_index);
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

        request
            .timeout(mihomo_api_timeout())
            .send()
            .await?
            .error_for_status()?;
        // Fetch updated config to sync UI
        self.fetch_config().await?;
        Ok(())
    }

    pub async fn reload_config_file(&self, path: &Path) -> Result<()> {
        if self.try_mihomot_config_switch(path).await? {
            return Ok(());
        }

        crate::mihomo::reload(
            &self.app_settings.base_url,
            &self.app_settings.api_secret,
            path,
        )
        .await
    }

    async fn try_mihomot_config_switch(&self, path: &Path) -> Result<bool> {
        let url = format!("{}/mhmt/config/switch", self.app_settings.base_url);
        let body = serde_json::json!({ "path": path.display().to_string() });
        let mut request = self.client.post(&url).json(&body);

        if !self.app_settings.api_secret.is_empty() {
            request = request.bearer_auth(&self.app_settings.api_secret);
        }

        let response = request.timeout(mihomo_api_timeout()).send().await?;

        if response.status() == StatusCode::NOT_FOUND
            || response.status() == StatusCode::METHOD_NOT_ALLOWED
        {
            return Ok(false);
        }

        if response.status().is_success() {
            return Ok(true);
        }

        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        bail!("mihomot config switch failed: HTTP {status}: {body}");
    }

    pub async fn fetch_proxies(&mut self) -> Result<()> {
        let url = format!("{}/proxies", self.app_settings.base_url);
        let mut request = self.client.get(&url);

        if !self.app_settings.api_secret.is_empty() {
            request = request.bearer_auth(&self.app_settings.api_secret);
        }

        match request.timeout(mihomo_api_timeout()).send().await {
            Ok(resp) => {
                if resp.status().is_success() {
                    match resp.json::<ProxiesResponse>().await {
                        Ok(data) => {
                            self.proxies = data.proxies;

                            // Populate latency from history
                            for (name, item) in &self.proxies {
                                if let Some(history) =
                                    item.extra.get("history").and_then(|h| h.as_array())
                                    && let Some(last) = history.last()
                                    && let Some(delay) = last.get("delay").and_then(|d| d.as_u64())
                                    && delay > 0
                                {
                                    self.proxy_latency
                                        .insert(name.clone(), ProxyLatencyStatus::Success(delay));
                                }
                            }

                            self.group_names = self
                                .proxies
                                .values()
                                .filter(|p| p.proxy_type.as_deref() == Some("Selector"))
                                .filter_map(|p| p.name.clone())
                                .collect();
                            self.group_names.sort();
                            if self.group_names.is_empty() {
                                self.group_state.select(None);
                                self.proxy_state.select(None);
                            } else {
                                let group_idx = self
                                    .group_state
                                    .selected()
                                    .filter(|&idx| idx < self.group_names.len())
                                    .unwrap_or(0);
                                self.group_state.select(Some(group_idx));

                                let proxy_len = self
                                    .group_names
                                    .get(group_idx)
                                    .and_then(|group_name| self.proxies.get(group_name))
                                    .and_then(|group| group.all.as_ref())
                                    .map_or(0, Vec::len);

                                let proxy_idx =
                                    self.proxy_state.selected().filter(|&idx| idx < proxy_len);
                                self.proxy_state
                                    .select(proxy_idx.or(Some(0)).filter(|_| proxy_len > 0));
                            }
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
        match request.timeout(mihomo_api_timeout()).send().await {
            Ok(resp) => {
                if resp.status().is_success() {
                    self.config = Some(resp.json::<Config>().await?);
                    self.error = None;
                } else {
                    self.error = Some(format!("Server returned error: {}", resp.status()));
                }
            }
            Err(e) => self.error = Some(format!("Failed to connect: {}", e)),
        }
        Ok(())
    }

    pub fn trigger_latency_test(&mut self) {
        let client = self.latency_client.clone();
        let url = self.app_settings.test_url.clone();
        let timeout = self.app_settings.test_timeout;
        let tx = self.real_latency_tx.clone();

        self.real_latency_status = RealLatencyStatus::Testing;

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
                        let _ = tx.send(RealLatencyStatus::Success(delay)).await;
                    } else {
                        let _ = tx
                            .send(RealLatencyStatus::Failed(format!(
                                "Status: {}",
                                resp.status()
                            )))
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
                    let _ = tx.send(RealLatencyStatus::Failed(msg)).await;
                }
            }
        });
    }

    pub fn trigger_group_latency_test(&mut self) {
        let proxy_names = self
            .get_selected_group_name()
            .and_then(|group_name| self.proxies.get(group_name))
            .and_then(|group| group.all.clone());

        if let Some(proxy_names) = proxy_names {
            let context = self.proxy_latency_test_context();

            for proxy_name in proxy_names {
                self.spawn_proxy_latency_test(proxy_name, &context);
            }
        }
    }

    pub fn trigger_selected_proxy_latency_test(&mut self) {
        if let Some(proxy_name) = self.get_selected_proxy_name() {
            let context = self.proxy_latency_test_context();
            self.spawn_proxy_latency_test(proxy_name, &context);
        }
    }

    fn proxy_latency_test_context(&self) -> ProxyLatencyTestContext {
        ProxyLatencyTestContext {
            base_url: self.app_settings.base_url.clone(),
            secret: self.app_settings.api_secret.clone(),
            test_url: self.app_settings.test_url.clone(),
            timeout: self.app_settings.test_timeout,
            client: self.client.clone(),
            tx: self.proxy_test_tx.clone(),
        }
    }

    fn spawn_proxy_latency_test(&mut self, proxy_name: String, context: &ProxyLatencyTestContext) {
        self.proxy_latency
            .insert(proxy_name.clone(), ProxyLatencyStatus::Testing);

        let my_url = format!(
            "{}/proxies/{}/delay?url={}&timeout={}",
            context.base_url,
            urlencoding::encode(&proxy_name),
            urlencoding::encode(&context.test_url),
            context.timeout
        );
        let my_secret = context.secret.clone();
        let timeout = context.timeout;
        let client = context.client.clone();
        let tx = context.tx.clone();

        tokio::spawn(async move {
            let mut req = client.get(&my_url);
            if !my_secret.is_empty() {
                req = req.bearer_auth(&my_secret);
            }

            let status = match req.timeout(Duration::from_millis(timeout)).send().await {
                Ok(resp) if resp.status().is_success() => {
                    match resp.json::<serde_json::Value>().await {
                        Ok(json) => match json.get("delay").and_then(|v| v.as_u64()) {
                            Some(delay) => ProxyLatencyStatus::Success(delay),
                            None => ProxyLatencyStatus::Failed("No delay".to_string()),
                        },
                        Err(_) => ProxyLatencyStatus::Failed("Bad JSON".to_string()),
                    }
                }
                Ok(resp) => ProxyLatencyStatus::Failed(format!("HTTP {}", resp.status())),
                Err(e) if e.is_timeout() => {
                    ProxyLatencyStatus::Failed(format!("Timeout: {}", e.without_url()))
                }
                Err(e) if e.is_connect() => {
                    ProxyLatencyStatus::Failed(format!("Conn Err: {}", e.without_url()))
                }
                Err(e) => ProxyLatencyStatus::Failed(format!("Error: {}", e.without_url())),
            };

            let _ = tx.send((proxy_name, status)).await;
        });
    }

    pub async fn select_proxy(&self, group_name: &str, proxy_name: &str) -> Result<()> {
        let url = format!("{}/proxies/{}", self.app_settings.base_url, group_name);
        let body = serde_json::json!({ "name": proxy_name });
        let mut request = self.client.put(&url).json(&body);

        if !self.app_settings.api_secret.is_empty() {
            request = request.bearer_auth(&self.app_settings.api_secret);
        }

        request
            .timeout(mihomo_api_timeout())
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }

    // Navigation Helpers
    pub fn next_group(&mut self) {
        if self.group_names.is_empty() {
            self.group_state.select(None);
            self.proxy_state.select(None);
            return;
        }

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
        if self.group_names.is_empty() {
            self.group_state.select(None);
            self.proxy_state.select(None);
            return;
        }

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
            if all.is_empty() {
                self.proxy_state.select(None);
                return;
            }

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
            if all.is_empty() {
                self.proxy_state.select(None);
                return;
            }

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

    /// Return the currently selected proxy group item, if the API returned it.
    pub fn selected_group(&self) -> Option<&ProxyItem> {
        self.get_selected_group_name()
            .and_then(|group_name| self.proxies.get(group_name))
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

    /// Return the currently selected proxy node item, if it exists in the API map.
    pub fn selected_proxy_item(&self) -> Option<&ProxyItem> {
        self.get_selected_proxy_name()
            .and_then(|proxy_name| self.proxies.get(&proxy_name))
    }
}

impl Drop for App {
    fn drop(&mut self) {
        if let Some(task) = self.traffic_monitor_task.take() {
            task.abort();
        }
    }
}

fn mihomo_api_timeout() -> Duration {
    Duration::from_secs(5)
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    fn make_selector_group(name: &str, proxies: Vec<&str>) -> ProxyItem {
        ProxyItem {
            name: Some(name.to_string()),
            proxy_type: Some("Selector".to_string()),
            now: proxies.first().map(|value| (*value).to_string()),
            all: Some(proxies.into_iter().map(str::to_string).collect()),
            extra: serde_json::Map::new(),
        }
    }

    async fn spawn_status_server(status_line: &'static str) -> Result<String> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;

        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let mut buffer = [0_u8; 1024];
                let _ = stream.read(&mut buffer).await;
                let response = format!("{status_line}\r\nContent-Length: 0\r\n\r\n");
                let _ = stream.write_all(response.as_bytes()).await;
            }
        });

        Ok(format!("http://{addr}"))
    }

    async fn spawn_config_reload_server() -> Result<(String, Arc<Mutex<Vec<String>>>)> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let requests = Arc::new(Mutex::new(Vec::new()));
        let request_log = Arc::clone(&requests);

        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let request_log = Arc::clone(&request_log);
                tokio::spawn(async move {
                    let mut buffer = [0_u8; 4096];
                    let Ok(n) = stream.read(&mut buffer).await else {
                        return;
                    };
                    let request = String::from_utf8_lossy(&buffer[..n]);
                    let first_line = request.lines().next().unwrap_or_default().to_string();
                    request_log.lock().unwrap().push(first_line.clone());

                    let status = if first_line.starts_with("POST /mhmt/config/switch") {
                        "HTTP/1.1 404 Not Found"
                    } else if first_line.starts_with("PUT /configs") {
                        "HTTP/1.1 204 No Content"
                    } else {
                        "HTTP/1.1 500 Internal Server Error"
                    };
                    let response = format!("{status}\r\nContent-Length: 0\r\n\r\n");
                    let _ = stream.write_all(response.as_bytes()).await;
                });
            }
        });

        Ok((format!("http://{addr}"), requests))
    }

    #[tokio::test]
    async fn navigation_handles_empty_lists_without_panicking() {
        let mut app = App::new(None, None);
        app.group_names.clear();
        app.group_state.select(Some(0));
        app.proxy_state.select(Some(0));

        app.next_group();
        app.previous_group();
        app.next_proxy();
        app.previous_proxy();

        assert_eq!(app.group_state.selected(), None);
        assert_eq!(app.proxy_state.selected(), None);
    }

    #[tokio::test]
    async fn navigation_handles_empty_proxy_groups_without_panicking() {
        let mut app = App::new(None, None);
        app.group_names = vec!["Group".to_string()];
        app.group_state.select(Some(0));
        app.proxy_state.select(Some(0));
        app.proxies.insert(
            "Group".to_string(),
            make_selector_group("Group", Vec::new()),
        );

        app.next_proxy();
        app.previous_proxy();

        assert_eq!(app.proxy_state.selected(), None);
    }

    #[tokio::test]
    async fn nav_activation_updates_route_and_focus() {
        let mut app = App::new(None, None);

        assert_eq!(app.route, Route::Dashboard);
        assert_eq!(app.focus, Focus::Nav);

        app.next_nav();
        assert!(!app.activate_nav());

        assert_eq!(app.route, Route::Proxies);
        assert_eq!(app.focus, Focus::Content);
        assert_eq!(app.nav_index, 1);

        app.nav_index = NAV_ITEMS.len() - 1;
        assert!(app.activate_nav());
    }

    #[tokio::test]
    async fn proxy_pane_selection_keeps_proxies_route_active() {
        let mut app = App::new(None, None);

        app.set_proxy_pane(ProxyPane::Proxies);

        assert_eq!(app.route, Route::Proxies);
        assert_eq!(app.focus, Focus::Content);
        assert_eq!(app.proxy_pane, ProxyPane::Proxies);
        assert_eq!(app.nav_index, 1);
    }

    #[tokio::test]
    async fn group_latency_test_marks_nodes_as_testing_immediately() {
        let mut app = App::new(Some("http://127.0.0.1:1".to_string()), Some(String::new()));
        app.group_names = vec!["Auto".to_string()];
        app.group_state.select(Some(0));
        app.proxies.insert(
            "Auto".to_string(),
            make_selector_group("Auto", vec!["Node A", "Node B"]),
        );

        app.trigger_group_latency_test();

        assert_eq!(
            app.proxy_latency.get("Node A"),
            Some(&ProxyLatencyStatus::Testing)
        );
        assert_eq!(
            app.proxy_latency.get("Node B"),
            Some(&ProxyLatencyStatus::Testing)
        );
    }

    #[tokio::test]
    async fn refresh_config_candidates_lists_yaml_files_in_current_directory() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock should be after epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "mihomot-config-candidates-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).expect("test directory should be created");
        let active = dir.join("active.yaml");
        let other = dir.join("other.yml");
        let ignored = dir.join("notes.txt");
        fs::write(&active, "mixed-port: 7890\n").expect("active config should be writable");
        fs::write(&other, "mixed-port: 7891\n").expect("other config should be writable");
        fs::write(&ignored, "ignored\n").expect("ignored file should be writable");

        let mut app = App::new(None, None);
        app.config_path = active.clone();
        app.refresh_config_candidates()
            .expect("candidate refresh should succeed");

        let paths: Vec<_> = app
            .config_candidates
            .iter()
            .map(|candidate| candidate.path.clone())
            .collect();
        assert!(paths.contains(&active));
        assert!(paths.contains(&other));
        assert!(!paths.contains(&ignored));
        assert_eq!(app.selected_config_candidate(), Some(active));

        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn refresh_config_candidates_filters_non_mihomo_yaml_files() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock should be after epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "mihomot-config-filter-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).expect("test directory should be created");
        let active = dir.join("active.yaml");
        let subscription = dir.join("work-subscription.yaml");
        let profiles = dir.join("profiles.yaml");
        let dns = dir.join("dns_config.yaml");
        fs::write(&active, "mixed-port: 7890\n").expect("active config should be writable");
        fs::write(&subscription, "proxies: []\nproxy-groups: []\nrules: []\n")
            .expect("subscription config should be writable");
        fs::write(&profiles, "current: abc\nitems: []\n")
            .expect("profiles metadata should be writable");
        fs::write(&dns, "dns:\n  enable: true\n").expect("dns config should be writable");

        let mut app = App::new(None, None);
        app.config_path = active.clone();
        app.refresh_config_candidates()
            .expect("candidate refresh should succeed");

        let paths: Vec<_> = app
            .config_candidates
            .iter()
            .map(|candidate| candidate.path.clone())
            .collect();
        assert!(paths.contains(&active));
        assert!(paths.contains(&subscription));
        assert!(!paths.contains(&profiles));
        assert!(!paths.contains(&dns));
        assert_eq!(app.selected_config_candidate(), Some(active));

        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn update_config_returns_error_for_http_failures() {
        let server_url = spawn_status_server("HTTP/1.1 500 Internal Server Error")
            .await
            .expect("server should start");
        let mut app = App::new(None, None);
        app.app_settings.base_url = server_url;
        app.app_settings.api_secret.clear();
        app.restart_traffic_monitor();

        let result = app
            .update_config(serde_json::json!({ "mode": "rule" }))
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn select_proxy_returns_error_for_http_failures() {
        let server_url = spawn_status_server("HTTP/1.1 401 Unauthorized")
            .await
            .expect("server should start");
        let mut app = App::new(None, None);
        app.app_settings.base_url = server_url;
        app.app_settings.api_secret.clear();
        app.restart_traffic_monitor();

        let result = app.select_proxy("Group", "Proxy").await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn reload_config_file_falls_back_to_native_mihomo_endpoint() {
        let (server_url, requests) = spawn_config_reload_server()
            .await
            .expect("server should start");
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock should be after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "mihomot-native-reload-fallback-{}-{nanos}.yaml",
            std::process::id()
        ));
        fs::write(
            &path,
            "mixed-port: 7890\nexternal-controller: 127.0.0.1:9090\n",
        )
        .expect("test config should be writable");

        let app = App::new(Some(server_url), Some(String::new()));
        app.reload_config_file(&path)
            .await
            .expect("native reload fallback should succeed");

        let requests = requests.lock().unwrap();
        assert!(
            requests
                .iter()
                .any(|line| line.starts_with("POST /mhmt/config/switch"))
        );
        assert!(requests.iter().any(|line| line.starts_with("PUT /configs")));

        let _ = fs::remove_file(&path);
    }
}

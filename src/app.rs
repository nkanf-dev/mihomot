use anyhow::Result;
use futures_util::StreamExt;
use ratatui::widgets::{ListState, TableState};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::fs;
use std::path::PathBuf;
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

#[derive(Clone, PartialEq)]
pub enum Focus {
    Groups,
    Proxies,
    Settings,
}

#[derive(Clone, PartialEq, Debug)]
pub enum ConfigEntry {
    BaseUrl,
    ApiSecret,
    TestUrl,
    TestTimeout,
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
    pub app_settings: AppSettings,

    pub real_latency_tx: mpsc::Sender<RealLatencyStatus>,
    pub real_latency_rx: mpsc::Receiver<RealLatencyStatus>,

    pub proxy_latency: HashMap<String, Option<u64>>,
    pub proxy_test_tx: mpsc::Sender<(String, u64)>,
    pub proxy_test_rx: mpsc::Receiver<(String, u64)>,

    pub traffic_tx: mpsc::Sender<Traffic>,
    pub traffic_rx: mpsc::Receiver<Traffic>,

    pub traffic_history_up: VecDeque<u64>,
    pub traffic_history_down: VecDeque<u64>,
    pub current_up: u64,
    pub current_down: u64,

    pub group_names: Vec<String>,
    pub group_state: ListState,
    pub proxy_state: TableState,
    pub focus: Focus,
    pub previous_focus: Focus,
    pub show_info_popup: bool,
    pub popup_scroll: u16,

    pub settings_items: Vec<ConfigEntry>,
    pub settings_state: TableState,
    pub is_editing: bool,
    pub editing_value: String,

    pub error: Option<String>,
    traffic_monitor_task: Option<JoinHandle<()>>,
}

impl App {
    pub fn new(url_override: Option<String>, secret_override: Option<String>) -> Self {
        let mut group_state = ListState::default();
        let mut proxy_state = TableState::default();
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
            client: Client::builder().build().unwrap_or_default(),
            app_settings,
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
            focus: Focus::Groups,
            previous_focus: Focus::Groups,
            show_info_popup: false,
            popup_scroll: 0,
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

        request.send().await?.error_for_status()?;
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

                            // Populate latency from history
                            for (name, item) in &self.proxies {
                                if let Some(history) =
                                    item.extra.get("history").and_then(|h| h.as_array())
                                    && let Some(last) = history.last()
                                    && let Some(delay) = last.get("delay").and_then(|d| d.as_u64())
                                    && delay > 0
                                {
                                    self.proxy_latency.insert(name.clone(), Some(delay));
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

    pub fn trigger_group_latency_test(&self) {
        if let Some(group_name) = self.get_selected_group_name()
            && let Some(group) = self.proxies.get(group_name)
            && let Some(all) = &group.all
        {
            let base_url = self.app_settings.base_url.clone();
            let secret = self.app_settings.api_secret.clone();
            let test_url = self.app_settings.test_url.clone();
            let timeout = self.app_settings.test_timeout;
            let tx = self.proxy_test_tx.clone();
            let client = self.client.clone();

            for proxy_name in all {
                let p_name = proxy_name.clone();
                let my_url = format!(
                    "{}/proxies/{}/delay?url={}&timeout={}",
                    base_url,
                    urlencoding::encode(&p_name),
                    urlencoding::encode(&test_url),
                    timeout
                );
                let my_client = client.clone();
                let my_secret = secret.clone();
                let my_tx = tx.clone();

                tokio::spawn(async move {
                    let mut req = my_client.get(&my_url);
                    if !my_secret.is_empty() {
                        req = req.bearer_auth(&my_secret);
                    }

                    if let Ok(resp) = req.send().await
                        && resp.status().is_success()
                        && let Ok(json) = resp.json::<serde_json::Value>().await
                        && let Some(delay) = json.get("delay").and_then(|v| v.as_u64())
                    {
                        let _ = my_tx.send((p_name, delay)).await;
                    }
                });
            }
        }
    }

    pub async fn select_proxy(&self, group_name: &str, proxy_name: &str) -> Result<()> {
        let url = format!("{}/proxies/{}", self.app_settings.base_url, group_name);
        let body = serde_json::json!({ "name": proxy_name });
        let mut request = self.client.put(&url).json(&body);

        if !self.app_settings.api_secret.is_empty() {
            request = request.bearer_auth(&self.app_settings.api_secret);
        }

        request.send().await?.error_for_status()?;
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

impl Drop for App {
    fn drop(&mut self) {
        if let Some(task) = self.traffic_monitor_task.take() {
            task.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;
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
}

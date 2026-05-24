use axum::{
    Router,
    body::Bytes,
    extract::{OriginalUri, Path as AxumPath, State},
    http::{HeaderMap, Method, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{any, get, post},
};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use crate::config;

#[derive(Clone)]
pub struct AppState {
    config_path: Arc<tokio::sync::RwLock<PathBuf>>,
    mutation_lock: Arc<tokio::sync::Mutex<()>>,
    secret: String,
    mihomo_endpoint: String,
}

#[derive(Debug, Deserialize)]
struct SwitchConfigRequest {
    path: String,
}

/// Build and start the mihomot HTTP API server
pub async fn start_server(
    listen_addr: &str,
    config_path: PathBuf,
    secret: String,
    mihomo_endpoint: String,
) -> Result<(), anyhow::Error> {
    let config_path = std::fs::canonicalize(&config_path).unwrap_or(config_path);
    let state = Arc::new(AppState {
        config_path: Arc::new(tokio::sync::RwLock::new(config_path)),
        mutation_lock: Arc::new(tokio::sync::Mutex::new(())),
        secret,
        mihomo_endpoint,
    });

    let app = build_router(state);

    let listener = tokio::net::TcpListener::bind(listen_addr).await?;
    println!("mihomot API listening on {}", listen_addr);

    axum::serve(listener, app).await?;
    Ok(())
}

fn build_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/mhmt/config/list", get(get_config_list))
        .route("/mhmt/config/switch", post(post_config_switch))
        .route(
            "/mhmt/config/raw",
            get(get_config_raw).post(post_config_raw),
        )
        .route("/mhmt/config/backup", get(get_config_backup))
        .route("/mhmt/reload", post(post_reload))
        .route("/mhmt/status", get(get_status))
        .route("/skill.md", get(get_skill_md))
        .route("/{*path}", any(proxy_mihomo_native))
        .with_state(state)
}

/// Verify the Authorization header matches the mihomo secret
fn verify_auth(headers: &HeaderMap, secret: &str) -> bool {
    if secret.is_empty() {
        return true;
    }
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|token| token == secret)
        .unwrap_or(false)
}

fn unauthorized() -> Response {
    (StatusCode::UNAUTHORIZED, "Unauthorized").into_response()
}

/// GET /mhmt/config/list - Return switchable config files beside the active config.
async fn get_config_list(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if !verify_auth(&headers, &state.secret) {
        return unauthorized();
    }

    let active_path = state.config_path.read().await.clone();
    let active_path_for_task = active_path.clone();
    let state_clone = Arc::clone(&state);

    let scan_result = tokio::task::spawn_blocking(move || {
        let candidates = config::list_config_candidates(&active_path_for_task)?;
        let filtered = candidates
            .into_iter()
            .filter(|(candidate, parsed)| validate_switch_target_config(&state_clone, &candidate.path, parsed).is_ok())
            .map(|(candidate, _)| candidate)
            .collect::<Vec<_>>();
        Ok::<_, anyhow::Error>(filtered)
    }).await.unwrap_or_else(|e| Err(anyhow::anyhow!("Join error: {}", e)));

    match scan_result {
        Ok(configs) => {
            let body = serde_json::json!({
                "active": active_path,
                "configs": configs,
            });
            (StatusCode::OK, axum::Json(body)).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to list configs: {}", e),
        )
            .into_response(),
    }
}

/// POST /mhmt/config/switch - Switch mihomo to another config file in the active directory.
async fn post_config_switch(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::Json(payload): axum::Json<SwitchConfigRequest>,
) -> Response {
    if !verify_auth(&headers, &state.secret) {
        return unauthorized();
    }

    let current_path = state.config_path.read().await.clone();
    let state_clone = Arc::clone(&state);
    let payload_path = payload.path.clone();

    let validation_result = tokio::task::spawn_blocking(move || {
        let target_path = resolve_switch_target(&current_path, &payload_path)?;
        validate_switch_target(&state_clone, &target_path)?;
        Ok::<_, anyhow::Error>(target_path)
    }).await.unwrap_or_else(|e| Err(anyhow::anyhow!("Validation task failed: {}", e)));

    let target_path = match validation_result {
        Ok(path) => path,
        Err(e) => return (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    };

    let _mutation_guard = state.mutation_lock.lock().await;

    match crate::mihomo::reload(&state.mihomo_endpoint, &state.secret, &target_path).await {
        Ok(()) => {
            let alive = crate::mihomo::check_alive(&state.mihomo_endpoint, &state.secret)
                .await
                .unwrap_or(false);
            if !alive {
                return (
                    StatusCode::BAD_GATEWAY,
                    "Mihomo accepted reload but its control API is not reachable; active config state was left unchanged",
                )
                    .into_response();
            }
            *state.config_path.write().await = target_path.clone();
            let body = serde_json::json!({
                "switch": "ok",
                "active": target_path,
                "alive": alive,
            });
            (StatusCode::OK, axum::Json(body)).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Config switch failed: {}", e),
        )
            .into_response(),
    }
}

/// GET /mhmt/config/raw - Return full config.yaml content
async fn get_config_raw(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if !verify_auth(&headers, &state.secret) {
        return unauthorized();
    }

    let config_path = state.config_path.read().await.clone();
    match config::read_raw(&config_path) {
        Ok(content) => (
            StatusCode::OK,
            [("Content-Type", "application/yaml")],
            content,
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to read config: {}", e),
        )
            .into_response(),
    }
}

/// POST /mhmt/config/raw - Replace config.yaml and reload
async fn post_config_raw(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: String,
) -> Response {
    if !verify_auth(&headers, &state.secret) {
        return unauthorized();
    }

    let _mutation_guard = state.mutation_lock.lock().await;
    let config_path = state.config_path.read().await.clone();

    // Backup before writing
    let backup_path = match config::backup_config(&config_path) {
        Ok(path) => path,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to backup config: {}", e),
            )
                .into_response();
        }
    };

    // Write new config
    if let Err(e) = config::write_raw(&config_path, &body) {
        return (StatusCode::BAD_REQUEST, format!("Invalid config: {}", e)).into_response();
    }

    // Reload mihomo
    match crate::mihomo::reload(&state.mihomo_endpoint, &state.secret, &config_path).await {
        Ok(()) => (StatusCode::OK, "Config updated and reloaded").into_response(),
        Err(e) => {
            // Rollback
            let reload_error = e.to_string();
            if let Err(restore_err) = config::restore_from_backup(&backup_path, &config_path) {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!(
                        "Reload failed and rollback restore failed: {}; restore error: {}",
                        reload_error, restore_err
                    ),
                )
                    .into_response();
            }
            if let Err(rollback_err) =
                crate::mihomo::reload(&state.mihomo_endpoint, &state.secret, &config_path).await
            {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!(
                        "Reload failed; disk config was restored from backup but rollback reload failed: {}; rollback error: {}",
                        reload_error, rollback_err
                    ),
                )
                    .into_response();
            }
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Reload failed, config rolled back: {}", reload_error),
            )
                .into_response()
        }
    }
}

/// GET /mhmt/config/backup - Return latest backup
async fn get_config_backup(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if !verify_auth(&headers, &state.secret) {
        return unauthorized();
    }

    let config_path = state.config_path.read().await.clone();

    // Create a fresh backup and return its content
    match config::backup_config(&config_path) {
        Ok(path) => match std::fs::read_to_string(&path) {
            Ok(content) => (
                StatusCode::OK,
                [("Content-Type", "application/yaml")],
                content,
            )
                .into_response(),
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to read backup: {}", e),
            )
                .into_response(),
        },
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to create backup: {}", e),
        )
            .into_response(),
    }
}

/// POST /mhmt/reload - Reload mihomo and return result
async fn post_reload(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if !verify_auth(&headers, &state.secret) {
        return unauthorized();
    }

    let config_path = state.config_path.read().await.clone();
    match crate::mihomo::reload(&state.mihomo_endpoint, &state.secret, &config_path).await {
        Ok(()) => {
            // Verify connectivity
            let alive = crate::mihomo::check_alive(&state.mihomo_endpoint, &state.secret)
                .await
                .unwrap_or(false);
            let body = serde_json::json!({
                "reload": "ok",
                "alive": alive,
            });
            (StatusCode::OK, axum::Json(body)).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Reload failed: {}", e),
        )
            .into_response(),
    }
}

/// GET /mhmt/status - Comprehensive status
async fn get_status(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if !verify_auth(&headers, &state.secret) {
        return unauthorized();
    }

    let client = match reqwest::Client::builder().no_proxy().build() {
        Ok(client) => client,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to build mihomo API client: {}", e),
            )
                .into_response();
        }
    };
    let config_path = state.config_path.read().await.clone();

    // Fetch version
    let version = {
        let url = format!("{}/version", state.mihomo_endpoint);
        let mut req = client.get(&url);
        if !state.secret.is_empty() {
            req = req.bearer_auth(&state.secret);
        }
        match req.send().await {
            Ok(resp) if resp.status().is_success() => resp
                .json::<serde_json::Value>()
                .await
                .ok()
                .and_then(|v| v.get("version").map(|v| v.to_string()))
                .unwrap_or_else(|| "unknown".to_string()),
            _ => "unreachable".to_string(),
        }
    };

    // Fetch configs for mode
    let mode = match config::read_config(&config_path) {
        Ok(c) => c.mode,
        Err(_) => "unknown".to_string(),
    };

    // Fetch connections count
    let connections = {
        let url = format!("{}/connections", state.mihomo_endpoint);
        let mut req = client.get(&url);
        if !state.secret.is_empty() {
            req = req.bearer_auth(&state.secret);
        }
        match req.send().await {
            Ok(resp) if resp.status().is_success() => resp
                .json::<serde_json::Value>()
                .await
                .ok()
                .and_then(|v| {
                    v.get("connections")
                        .and_then(|c| c.as_array())
                        .map(|a| a.len())
                })
                .unwrap_or(0),
            _ => 0,
        }
    };

    let body = serde_json::json!({
        "version": version,
        "mode": mode,
        "connections": connections,
        "mihomo_endpoint": state.mihomo_endpoint,
        "config_path": config_path.to_string_lossy(),
    });

    (StatusCode::OK, axum::Json(body)).into_response()
}

/// GET /skill.md - Serve the skill document for AI agents
async fn get_skill_md() -> impl IntoResponse {
    let skill = include_str!("../skill.md");
    (
        StatusCode::OK,
        [("Content-Type", "text/markdown; charset=utf-8")],
        skill,
    )
}

fn resolve_switch_target(current_path: &Path, requested_path: &str) -> anyhow::Result<PathBuf> {
    let requested = requested_path.trim();
    if requested.is_empty() {
        anyhow::bail!("path is required");
    }

    let base_dir = current_path.parent().unwrap_or(Path::new("."));
    let raw_path = PathBuf::from(requested);
    let candidate = if raw_path.is_absolute() {
        raw_path
    } else {
        base_dir.join(raw_path)
    };

    let current_dir = std::fs::canonicalize(base_dir)
        .map_err(|e| anyhow::anyhow!("Failed to resolve config directory: {}", e))?;
    let target = std::fs::canonicalize(&candidate)
        .map_err(|e| anyhow::anyhow!("Failed to resolve {}: {}", candidate.display(), e))?;

    if !target.starts_with(&current_dir) {
        anyhow::bail!(
            "Config switch target must be inside {}",
            current_dir.display()
        );
    }

    let candidates = config::list_config_candidates(current_path)?;
    if !candidates.iter().any(|(candidate, _)| {
        std::fs::canonicalize(&candidate.path)
            .map(|path| path == target)
            .unwrap_or(false)
    }) {
        anyhow::bail!(
            "Config is not a selectable mihomo config: {}",
            target.display()
        );
    }

    Ok(target)
}

fn validate_switch_target(state: &AppState, target_path: &Path) -> anyhow::Result<()> {
    let target_config = config::read_config(target_path)?;
    validate_switch_target_config(state, target_path, &target_config)
}

fn validate_switch_target_config(state: &AppState, _target_path: &Path, target_config: &config::MihomoConfig) -> anyhow::Result<()> {
    let target_secret = target_config.secret.as_deref().unwrap_or_default();
    if target_secret != state.secret {
        anyhow::bail!(
            "Target config secret differs from the active mihomot token; edit the config first or restart mihomot with the target config"
        );
    }

    if !target_endpoint_matches_current(&state.mihomo_endpoint, target_config) {
        anyhow::bail!(
            "Target config external-controller does not match the running mihomo endpoint {}; edit it first or restart mihomot with the target config",
            state.mihomo_endpoint
        );
    }

    Ok(())
}

fn target_endpoint_matches_current(endpoint: &str, target_config: &config::MihomoConfig) -> bool {
    let Some(external_controller) = target_config.external_controller.as_deref() else {
        return false;
    };
    
    let (endpoint_host, endpoint_port) = parse_endpoint_host_port(endpoint);
    let endpoint_host_port = if let Some(p) = endpoint_port {
        format!("{}:{}", endpoint_host, p)
    } else {
        endpoint_host.clone()
    };

    // Normalize both to SocketAddr for reliable comparison if possible
    let target_addr = external_controller.parse::<std::net::SocketAddr>();
    let endpoint_addr = endpoint_host_port.parse::<std::net::SocketAddr>();

    if let (Ok(t), Ok(e)) = (target_addr, endpoint_addr) {
        return t == e;
    }

    // Fallback to strict string-based host and port equality
    let (target_host, target_port) = config::parse_external_controller(external_controller);

    target_host == endpoint_host && Some(target_port) == endpoint_port
}

fn parse_endpoint_host_port(endpoint: &str) -> (String, Option<u16>) {
    let without_scheme = endpoint
        .strip_prefix("http://")
        .or_else(|| endpoint.strip_prefix("https://"))
        .unwrap_or(endpoint);
    let host_port = without_scheme.split('/').next().unwrap_or(without_scheme);

    if let Some((host, port)) = host_port.rsplit_once(':') {
        (host.to_string(), port.parse::<u16>().ok())
    } else {
        (host_port.to_string(), None)
    }
}

/// Forward non-mihomot paths to the native mihomo API.
async fn proxy_mihomo_native(
    State(state): State<Arc<AppState>>,
    OriginalUri(original_uri): OriginalUri,
    method: Method,
    headers: HeaderMap,
    AxumPath(path): AxumPath<String>,
    body: Bytes,
) -> Response {
    if !verify_auth(&headers, &state.secret) {
        return unauthorized();
    }

    let path = path.trim_start_matches('/');
    if path == "mhmt" || path.starts_with("mhmt/") {
        return (StatusCode::NOT_FOUND, "Unknown mihomot endpoint").into_response();
    }

    let mut url = format!("{}/{}", state.mihomo_endpoint.trim_end_matches('/'), path);
    if let Some(query) = original_uri.query() {
        url.push('?');
        url.push_str(query);
    }

    let client = match reqwest::Client::builder().no_proxy().build() {
        Ok(client) => client,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to build mihomo API client: {}", e),
            )
                .into_response();
        }
    };
    let req_method = match reqwest::Method::from_bytes(method.as_str().as_bytes()) {
        Ok(method) => method,
        Err(_) => return (StatusCode::METHOD_NOT_ALLOWED, "Unsupported method").into_response(),
    };
    let mut req = client.request(req_method, url);

    if !state.secret.is_empty() {
        req = req.bearer_auth(&state.secret);
    }

    if let Some(content_type) = headers.get(header::CONTENT_TYPE) {
        req = req.header(header::CONTENT_TYPE.as_str(), content_type.clone());
    }
    if !body.is_empty() {
        req = req.body(body);
    }

    match req.send().await {
        Ok(resp) => {
            let status = StatusCode::from_u16(resp.status().as_u16())
                .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            let content_type = resp
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string);
            let bytes = resp.bytes().await.unwrap_or_default();

            let mut response = (status, bytes).into_response();
            if let Some(content_type) = content_type
                && let Ok(value) = content_type.parse()
            {
                response.headers_mut().insert(header::CONTENT_TYPE, value);
            }
            response
        }
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            format!("Failed to proxy mihomo API: {}", e),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use std::fs;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tower::ServiceExt;

    fn unique_temp_dir(name: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock should be after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("mihomot-{name}-{}-{nanos}", std::process::id()))
    }

    fn write_config(path: &Path, port: u16, secret: &str) {
        fs::write(
            path,
            format!(
                "mixed-port: 7890\nexternal-controller: 127.0.0.1:{port}\nsecret: {secret:?}\n"
            ),
        )
        .expect("test config should be writable");
    }

    async fn spawn_mihomo_reload_mock() -> anyhow::Result<(String, u16)> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    let mut buf = [0_u8; 4096];
                    let Ok(n) = stream.read(&mut buf).await else {
                        return;
                    };
                    let request = String::from_utf8_lossy(&buf[..n]);
                    let first_line = request.lines().next().unwrap_or_default();
                    let (status, content_type, body) = if first_line.starts_with("PUT /configs") {
                        ("HTTP/1.1 204 No Content", "text/plain", "")
                    } else if first_line.starts_with("GET /version") {
                        (
                            "HTTP/1.1 200 OK",
                            "application/json",
                            "{\"version\":\"test\"}",
                        )
                    } else if first_line.starts_with("GET /connections") {
                        (
                            "HTTP/1.1 200 OK",
                            "application/json",
                            "{\"connections\":[]}",
                        )
                    } else {
                        ("HTTP/1.1 404 Not Found", "text/plain", "not found")
                    };
                    let response = format!(
                        "{status}\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                });
            }
        });

        Ok((format!("http://127.0.0.1:{port}"), port))
    }

    #[test]
    fn resolve_switch_target_accepts_only_selectable_configs_inside_active_dir() {
        let dir = unique_temp_dir("server-switch-path");
        fs::create_dir_all(&dir).expect("test directory should be created");
        let active = dir.join("active.yaml");
        let other = dir.join("other.yaml");
        let metadata = dir.join("profiles.yaml");
        let outside_dir = unique_temp_dir("server-switch-outside");
        fs::create_dir_all(&outside_dir).expect("outside directory should be created");
        let outside = outside_dir.join("outside.yaml");
        write_config(&active, 9090, "s");
        write_config(&other, 9090, "s");
        write_config(&outside, 9090, "s");
        fs::write(&metadata, "current: abc\nitems: []\n")
            .expect("metadata file should be writable");

        assert_eq!(
            resolve_switch_target(&active, "other.yaml").expect("relative target should resolve"),
            other.canonicalize().expect("other path should resolve")
        );
        assert!(resolve_switch_target(&active, "profiles.yaml").is_err());
        assert!(resolve_switch_target(&active, outside.to_str().unwrap()).is_err());

        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&outside_dir);
    }

    #[test]
    fn resolve_switch_target_handles_relative_active_path() {
        let dir = PathBuf::from("target")
            .join("test-configs")
            .join(format!("server-switch-relative-{}", std::process::id()));
        fs::create_dir_all(&dir).expect("test directory should be created");
        let active = dir.join("active.yaml");
        let other = dir.join("other.yaml");
        write_config(&active, 9090, "s");
        write_config(&other, 9090, "s");

        assert_eq!(
            resolve_switch_target(&active, "other.yaml")
                .expect("relative active path should still resolve selectable configs"),
            other.canonicalize().expect("other path should resolve")
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn validate_switch_target_rejects_secret_or_controller_changes() {
        let dir = unique_temp_dir("server-switch-validation");
        fs::create_dir_all(&dir).expect("test directory should be created");
        let active = dir.join("active.yaml");
        let same = dir.join("same.yaml");
        let changed_secret = dir.join("changed-secret.yaml");
        let changed_port = dir.join("changed-port.yaml");
        let missing_controller = dir.join("missing-controller.yaml");
        write_config(&active, 9090, "s");
        write_config(&same, 9090, "s");
        write_config(&changed_secret, 9090, "other");
        write_config(&changed_port, 19090, "s");
        fs::write(&missing_controller, "mixed-port: 7890\nsecret: \"s\"\n")
            .expect("missing-controller config should be writable");

        let state = AppState {
            config_path: Arc::new(tokio::sync::RwLock::new(active.clone())),
            mutation_lock: Arc::new(tokio::sync::Mutex::new(())),
            secret: "s".to_string(),
            mihomo_endpoint: "http://127.0.0.1:9090".to_string(),
        };

        validate_switch_target(&state, &same).expect("matching target should be accepted");
        assert!(validate_switch_target(&state, &changed_secret).is_err());
        assert!(validate_switch_target(&state, &changed_port).is_err());
        assert!(validate_switch_target(&state, &missing_controller).is_err());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn parse_endpoint_host_port_handles_http_urls() {
        assert_eq!(parse_endpoint_host_port("http://127.0.0.1:9090"), ("127.0.0.1".to_string(), Some(9090)));
        assert_eq!(
            parse_endpoint_host_port("https://example.com:9443/x"),
            ("example.com".to_string(), Some(9443))
        );
        assert_eq!(parse_endpoint_host_port("http://example.com"), ("example.com".to_string(), None));
    }

    #[tokio::test]
    async fn config_list_and_switch_work_through_http_router() {
        let (mihomo_endpoint, port) = spawn_mihomo_reload_mock()
            .await
            .expect("mock mihomo API should start");
        let dir = unique_temp_dir("server-http-switch");
        fs::create_dir_all(&dir).expect("test directory should be created");
        let active = dir.join("active.yaml");
        let other = dir.join("other.yaml");
        let wrong_port = dir.join("wrong-port.yaml");
        fs::write(
            &active,
            format!("mode: rule\nmixed-port: 7890\nexternal-controller: 127.0.0.1:{port}\n"),
        )
        .expect("active config should be writable");
        fs::write(
            &other,
            format!("mode: global\nmixed-port: 7891\nexternal-controller: 127.0.0.1:{port}\n"),
        )
        .expect("other config should be writable");
        fs::write(
            &wrong_port,
            format!(
                "mode: direct\nmixed-port: 7892\nexternal-controller: 127.0.0.1:{}\n",
                port + 1
            ),
        )
        .expect("wrong-port config should be writable");

        let state = Arc::new(AppState {
            config_path: Arc::new(tokio::sync::RwLock::new(active.clone())),
            mutation_lock: Arc::new(tokio::sync::Mutex::new(())),
            secret: String::new(),
            mihomo_endpoint,
        });
        let app = build_router(state.clone());

        let list_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/mhmt/config/list")
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("list request should complete");
        assert_eq!(list_response.status(), StatusCode::OK);
        let list_body = to_bytes(list_response.into_body(), 64 * 1024)
            .await
            .expect("list body should be readable");
        let list_json: serde_json::Value =
            serde_json::from_slice(&list_body).expect("list body should be JSON");
        assert_eq!(list_json["configs"].as_array().unwrap().len(), 2);
        assert!(
            !list_json["configs"]
                .as_array()
                .unwrap()
                .iter()
                .any(|config| config["label"] == "wrong-port")
        );

        let switch_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mhmt/config/switch")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"path":"other.yaml"}"#))
                    .expect("request should build"),
            )
            .await
            .expect("switch request should complete");
        assert_eq!(switch_response.status(), StatusCode::OK);
        assert_eq!(
            *state.config_path.read().await,
            other.canonicalize().expect("other path should resolve")
        );

        let raw_response = app
            .oneshot(
                Request::builder()
                    .uri("/mhmt/config/raw")
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("raw request should complete");
        assert_eq!(raw_response.status(), StatusCode::OK);
        let raw_body = to_bytes(raw_response.into_body(), 64 * 1024)
            .await
            .expect("raw body should be readable");
        let raw = String::from_utf8(raw_body.to_vec()).expect("raw body should be utf8");
        assert!(raw.contains("mode: global"));

        let _ = fs::remove_dir_all(&dir);
    }
}

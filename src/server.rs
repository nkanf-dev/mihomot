use axum::{
    Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::config;

#[derive(Clone)]
struct AppState {
    config_path: PathBuf,
    secret: String,
    mihomo_endpoint: String,
}

/// Build and start the mihomot HTTP API server
pub async fn start_server(
    listen_addr: &str,
    config_path: PathBuf,
    secret: String,
    mihomo_endpoint: String,
) -> Result<(), anyhow::Error> {
    let state = Arc::new(AppState {
        config_path,
        secret,
        mihomo_endpoint,
    });

    let app = Router::new()
        .route("/mhmt/config/raw", get(get_config_raw).post(post_config_raw))
        .route("/mhmt/config/backup", get(get_config_backup))
        .route("/mhmt/reload", post(post_reload))
        .route("/mhmt/status", get(get_status))
        .route("/skill.md", get(get_skill_md))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(listen_addr).await?;
    println!("mihomot API listening on {}", listen_addr);

    axum::serve(listener, app).await?;
    Ok(())
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

/// GET /mhmt/config/raw - Return full config.yaml content
async fn get_config_raw(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if !verify_auth(&headers, &state.secret) {
        return unauthorized();
    }

    match config::read_raw(&state.config_path) {
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

    // Backup before writing
    if let Err(e) = config::backup_config(&state.config_path) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to backup config: {}", e),
        )
            .into_response();
    }

    // Write new config
    if let Err(e) = config::write_raw(&state.config_path, &body) {
        return (StatusCode::BAD_REQUEST, format!("Invalid config: {}", e)).into_response();
    }

    // Reload mihomo
    match crate::mihomo::reload(&state.mihomo_endpoint, &state.secret, &state.config_path).await {
        Ok(()) => (StatusCode::OK, "Config updated and reloaded").into_response(),
        Err(e) => {
            // Rollback
            let backups = find_backups(&state.config_path);
            if let Some(latest_backup) = backups.first() {
                let _ = config::restore_from_backup(latest_backup, &state.config_path);
            }
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Reload failed, config rolled back: {}", e),
            )
                .into_response()
        }
    }
}

/// GET /mhmt/config/backup - Return latest backup
async fn get_config_backup(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if !verify_auth(&headers, &state.secret) {
        return unauthorized();
    }

    // Create a fresh backup and return its content
    match config::backup_config(&state.config_path) {
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
async fn post_reload(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if !verify_auth(&headers, &state.secret) {
        return unauthorized();
    }

    match crate::mihomo::reload(&state.mihomo_endpoint, &state.secret, &state.config_path).await {
        Ok(()) => {
            // Verify connectivity
            let alive =
                crate::mihomo::check_alive(&state.mihomo_endpoint, &state.secret)
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
async fn get_status(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if !verify_auth(&headers, &state.secret) {
        return unauthorized();
    }

    let client = reqwest::Client::new();

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
    let mode = match config::read_config(&state.config_path) {
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
                .and_then(|v| v.get("connections").and_then(|c| c.as_array()).map(|a| a.len()))
                .unwrap_or(0),
            _ => 0,
        }
    };

    let body = serde_json::json!({
        "version": version,
        "mode": mode,
        "connections": connections,
        "mihomo_endpoint": state.mihomo_endpoint,
        "config_path": state.config_path.to_string_lossy(),
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

/// Find backup files sorted by name (newest first)
fn find_backups(config_path: &Path) -> Vec<PathBuf> {
    let dir = match config_path.parent() {
        Some(d) => d,
        None => return vec![],
    };
    let stem = match config_path.file_stem().map(|s| s.to_string_lossy().to_string()) {
        Some(s) => s,
        None => return vec![],
    };

    let mut backups: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(entries) => entries
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .map(|n| n.to_string_lossy().starts_with(&format!("{}.bak.", stem)))
                    .unwrap_or(false)
            })
            .collect(),
        Err(_) => return vec![],
    };

    backups.sort_by(|a, b| b.cmp(a)); // newest first
    backups
}

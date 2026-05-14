use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

const GITHUB_RELEASE_URL: &str =
    "https://github.com/MetaCubeX/mihomo/releases/latest/download";
const GHPROXY_PREFIX: &str = "https://ghproxy.com/";

#[derive(Debug)]
pub enum RuntimeMode {
    Docker { container_name: String },
    Binary { path: PathBuf },
}

/// Detect how mihomo is available: Docker container or local binary
pub fn detect_runtime() -> Result<RuntimeMode> {
    // 1. Check if mihomo docker container is running
    if let Ok(output) = Command::new("docker").args(["ps", "--format", "{{.Names}}"]).output() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        for name in stdout.lines() {
            if name.trim() == "mihomo" {
                return Ok(RuntimeMode::Docker {
                    container_name: "mihomo".to_string(),
                });
            }
        }
    }

    // 2. Check local binary at ~/.config/mihomot/mihomo
    let local_bin = local_binary_path();
    if local_bin.exists() {
        return Ok(RuntimeMode::Binary { path: local_bin });
    }

    // 3. Check if mihomo is in PATH
    if let Ok(output) = Command::new("which").arg("mihomo").output() {
        if output.status.success() {
            let path_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !path_str.is_empty() {
                return Ok(RuntimeMode::Binary {
                    path: PathBuf::from(path_str),
                });
            }
        }
    }

    anyhow::bail!("mihomo not found. Install via Docker or download binary.")
}

/// Check if mihomo is currently running and responding
pub async fn check_alive(endpoint: &str, secret: &str) -> Result<bool> {
    let client = reqwest::Client::new();
    let url = format!("{}/version", endpoint);
    let mut req = client.get(&url);
    if !secret.is_empty() {
        req = req.bearer_auth(secret);
    }
    match req.send().await {
        Ok(resp) => Ok(resp.status().is_success()),
        Err(_) => Ok(false),
    }
}

/// Attempt to start mihomo
pub fn start(runtime: &RuntimeMode, config_path: &Path) -> Result<()> {
    match runtime {
        RuntimeMode::Docker { container_name } => {
            // Check if container exists but is stopped
            let ps_output = Command::new("docker")
                .args(["ps", "-a", "--filter", &format!("name=^{}$", container_name), "--format", "{{.Status}}"])
                .output()?;

            let status = String::from_utf8_lossy(&ps_output.stdout);
            if status.contains("Exited") || status.contains("Created") {
                Command::new("docker")
                    .args(["start", container_name])
                    .output()
                    .context("Failed to start docker container")?;
            } else if !status.contains("Up") {
                // Container doesn't exist, create and run
                let config_dir = config_path
                    .parent()
                    .unwrap_or(Path::new("."))
                    .to_string_lossy();
                Command::new("docker")
                    .args([
                        "run", "-d",
                        "--name", container_name,
                        "--network", "host",
                        "--restart", "always",
                        "-v", &format!("{}:/root/.config/mihomo/config.yaml", config_path.display()),
                        "-v", &format!("{}:/root/.config/mihomo", config_dir),
                        "--cap-add", "NET_ADMIN",
                        "--device", "/dev/net/tun",
                        "metacubex/mihomo:latest",
                    ])
                    .output()
                    .context("Failed to create docker container")?;
            }
        }
        RuntimeMode::Binary { path } => {
            // Use status() to wait for process to be ready, then detach
            // mihomo daemonizes itself with -d flag, so status() returns once it forks
            Command::new(path)
                .args(["-d", config_path.parent().unwrap_or(Path::new(".")).to_str().unwrap_or(".")])
                .status()
                .context("Failed to start mihomo binary")?;
        }
    }
    Ok(())
}

/// Stop mihomo
pub fn stop(runtime: &RuntimeMode) -> Result<()> {
    match runtime {
        RuntimeMode::Docker { container_name } => {
            Command::new("docker")
                .args(["stop", container_name])
                .output()
                .context("Failed to stop docker container")?;
        }
        RuntimeMode::Binary { .. } => {
            // Try to kill mihomo process
            Command::new("pkill")
                .args(["-f", "mihomo"])
                .output()
                .ok();
        }
    }
    Ok(())
}

/// Restart mihomo
pub fn restart(runtime: &RuntimeMode, config_path: &Path) -> Result<()> {
    stop(runtime)?;
    // Small delay for port release
    std::thread::sleep(std::time::Duration::from_millis(500));
    start(runtime, config_path)?;
    Ok(())
}

/// Trigger mihomo reload via its API (PATCH /configs)
pub async fn reload(endpoint: &str, secret: &str, config_path: &Path) -> Result<()> {
    let content = std::fs::read_to_string(config_path)
        .with_context(|| format!("Failed to read {}", config_path.display()))?;

    let client = reqwest::Client::new();
    let url = format!("{}/configs?force=true", endpoint);
    let mut req = client.put(&url).body(content).header("Content-Type", "application/yaml");
    if !secret.is_empty() {
        req = req.bearer_auth(secret);
    }
    let resp = req.send().await.context("Failed to reach mihomo API")?;
    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("mihomo reload failed: {}", body);
    }
    Ok(())
}

fn local_binary_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home)
        .join(".config")
        .join("mihomot")
        .join("mihomo")
}

/// Ensure mihomo is available. Tries detection first, then auto-installs.
/// Returns the runtime mode.
pub async fn ensure_mihomo(config_path: &Path) -> Result<RuntimeMode> {
    // Ensure config directory exists for binary placement
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }

    // Already available?
    if let Ok(runtime) = detect_runtime() {
        return Ok(runtime);
    }

    println!("mihomo not found, attempting to install...");

    // Try Docker first
    if has_docker() {
        println!("Docker detected, pulling metacubex/mihomo:latest...");
        let status = Command::new("docker")
            .args(["pull", "metacubex/mihomo:latest"])
            .status();
        if let Ok(s) = status {
            if s.success() {
                println!("Docker image pulled successfully.");
                return Ok(RuntimeMode::Docker {
                    container_name: "mihomo".to_string(),
                });
            }
        }
        eprintln!("Docker pull failed, falling back to binary download...");
    }

    // Download binary
    let bin_path = local_binary_path();
    download_binary(&bin_path).await?;

    Ok(RuntimeMode::Binary { path: bin_path })
}

fn has_docker() -> bool {
    Command::new("docker")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Download mihomo binary with CN-aware mirror logic
async fn download_binary(dest: &Path) -> Result<()> {
    let (os, arch) = detect_platform()?;
    let filename = format!("mihomo-{}-{}", os, arch);

    // Build download URLs in priority order
    let gh_url = format!("{}/{}", GITHUB_RELEASE_URL, filename);
    let proxy_url = format!("{}{}", GHPROXY_PREFIX, gh_url);

    let urls = if is_likely_cn() {
        vec![proxy_url, gh_url]
    } else {
        vec![gh_url, proxy_url]
    };

    fs_create_dir_all(dest)?;

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()?;

    let mut last_err = None;
    for url in &urls {
        println!("Trying: {}", url);
        match client.get(url).send().await {
            Ok(resp) if resp.status().is_success() => {
                let bytes = resp.bytes().await?;
                std::fs::write(dest, &bytes)?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(dest, std::fs::Permissions::from_mode(0o755))?;
                }
                println!("Downloaded mihomo to {}", dest.display());
                return Ok(());
            }
            Ok(resp) => {
                last_err = Some(format!("HTTP {}", resp.status()));
            }
            Err(e) => {
                last_err = Some(e.to_string());
            }
        }
    }

    anyhow::bail!(
        "Failed to download mihomo. Last error: {}\n\
         Please download manually from:\n  \
         https://github.com/MetaCubeX/mihomo/releases/latest\n\
         and place the binary at: {}",
        last_err.unwrap_or_else(|| "unknown".to_string()),
        dest.display()
    )
}

fn detect_platform() -> Result<(String, String)> {
    let os = match std::env::consts::OS {
        "linux" => "linux",
        "macos" => "darwin",
        other => anyhow::bail!("Unsupported OS: {}", other),
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" | "amd64" => "amd64",
        "aarch64" | "arm64" => "arm64",
        other => anyhow::bail!("Unsupported architecture: {}", other),
    };
    Ok((os.to_string(), arch.to_string()))
}

/// Simple heuristic: check if we're likely in mainland China
fn is_likely_cn() -> bool {
    // Check env var override
    if let Ok(region) = std::env::var("MIHOMOT_REGION") {
        return region.to_lowercase() == "cn";
    }

    // Check timezone
    if let Ok(tz) = std::env::var("TZ") {
        if tz.contains("Shanghai") || tz.contains("Chongqing") || tz.contains("PRC") {
            return true;
        }
    }

    // Check locale
    if let Ok(lang) = std::env::var("LANG") {
        if lang.starts_with("zh_CN") {
            return true;
        }
    }

    false
}

fn fs_create_dir_all(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    Ok(())
}

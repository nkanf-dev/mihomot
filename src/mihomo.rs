use anyhow::{Context, Result};
use flate2::read::GzDecoder;
use futures_util::StreamExt;
use serde::Deserialize;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const GITHUB_API_RELEASE_URL: &str =
    "https://api.github.com/repos/MetaCubeX/mihomo/releases/latest";
const MIHOMO_IMAGE: &str = "metacubex/mihomo:latest";
const SKILL_RAW_URL: &str = "https://raw.githubusercontent.com/nkanf-dev/mihomot/main/skill.md";

#[derive(Debug)]
pub enum RuntimeMode {
    Docker {
        container_name: String,
        image: String,
    },
    Binary {
        path: PathBuf,
    },
}

#[derive(Debug, Deserialize)]
struct GithubRelease {
    assets: Vec<GithubAsset>,
}

#[derive(Clone, Debug, Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
}

/// Return the best skill.md URL for the current network region.
pub fn skill_install_url() -> &'static str {
    if is_likely_cn() {
        "https://gh-proxy.com/https://raw.githubusercontent.com/nkanf-dev/mihomot/main/skill.md"
    } else {
        SKILL_RAW_URL
    }
}

/// Detect how mihomo is available: Docker container or local binary
pub fn detect_runtime() -> Result<RuntimeMode> {
    // 1. Check if mihomo docker container is running
    if let Ok(output) = Command::new("docker")
        .args(["ps", "--format", "{{.Names}}"])
        .output()
    {
        let stdout = String::from_utf8_lossy(&output.stdout);
        for name in stdout.lines() {
            if name.trim() == "mihomo" {
                return Ok(RuntimeMode::Docker {
                    container_name: "mihomo".to_string(),
                    image: MIHOMO_IMAGE.to_string(),
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
    if let Ok(output) = Command::new("which").arg("mihomo").output()
        && output.status.success()
    {
        let path_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !path_str.is_empty() {
            return Ok(RuntimeMode::Binary {
                path: PathBuf::from(path_str),
            });
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
        RuntimeMode::Docker {
            container_name,
            image,
        } => {
            // Check if container exists but is stopped
            let ps_output = Command::new("docker")
                .args([
                    "ps",
                    "-a",
                    "--filter",
                    &format!("name=^{}$", container_name),
                    "--format",
                    "{{.Status}}",
                ])
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
                        "run",
                        "-d",
                        "--name",
                        container_name,
                        "--network",
                        "host",
                        "--restart",
                        "always",
                        "-v",
                        &format!("{}:/root/.config/mihomo/config.yaml", config_path.display()),
                        "-v",
                        &format!("{}:/root/.config/mihomo", config_dir),
                        "--cap-add",
                        "NET_ADMIN",
                        "--device",
                        "/dev/net/tun",
                        image,
                    ])
                    .output()
                    .context("Failed to create docker container")?;
            }
        }
        RuntimeMode::Binary { path } => {
            Command::new(path)
                .args([
                    "-d",
                    config_path
                        .parent()
                        .unwrap_or(Path::new("."))
                        .to_str()
                        .unwrap_or("."),
                ])
                .stdin(Stdio::null())
                .spawn()
                .context("Failed to start mihomo binary")?;
        }
    }
    Ok(())
}

/// Trigger mihomo reload via its API (PUT /configs?force=true)
pub async fn reload(endpoint: &str, secret: &str, config_path: &Path) -> Result<()> {
    let content = std::fs::read_to_string(config_path)
        .with_context(|| format!("Failed to read {}", config_path.display()))?;
    let client = reqwest::Client::new();
    let url = format!("{}/configs?force=true", endpoint);

    let mut bodies = mihomo_config_paths(config_path)
        .into_iter()
        .map(|path| serde_json::json!({ "path": path }))
        .collect::<Vec<_>>();
    bodies.push(serde_json::json!({ "payload": content }));

    let mut last_err = None;
    for body in bodies {
        let mut req = client.put(&url).json(&body);
        if !secret.is_empty() {
            req = req.bearer_auth(secret);
        }
        match req.send().await {
            Ok(resp) if resp.status().is_success() => return Ok(()),
            Ok(resp) => {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                last_err = Some(format!("HTTP {}: {}", status, body));
            }
            Err(e) => {
                last_err = Some(e.to_string());
            }
        }
    }

    anyhow::bail!(
        "mihomo reload failed: {}",
        last_err.unwrap_or_else(|| "unknown error".to_string())
    )
}

fn mihomo_config_paths(config_path: &Path) -> Vec<String> {
    let host_path = config_path.to_string_lossy().to_string();
    // Docker mode mounts the host config file at this path in the container.
    if has_docker_container("mihomo") {
        vec!["/root/.config/mihomo/config.yaml".to_string(), host_path]
    } else {
        vec![host_path]
    }
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
        println!("Docker detected, pulling mihomo image...");
        if let Some(image) = pull_docker_image() {
            return Ok(RuntimeMode::Docker {
                container_name: "mihomo".to_string(),
                image,
            });
        }
        eprintln!("Docker image pull failed, falling back to binary download...");
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

fn has_docker_container(name: &str) -> bool {
    Command::new("docker")
        .args([
            "ps",
            "-a",
            "--filter",
            &format!("name=^{}$", name),
            "--format",
            "{{.Names}}",
        ])
        .output()
        .map(|output| {
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .any(|line| line.trim() == name)
        })
        .unwrap_or(false)
}

fn pull_docker_image() -> Option<String> {
    let mut last_image = None;

    for image in docker_image_candidates() {
        println!("Trying docker image: {}", image);
        last_image = Some(image.clone());
        match Command::new("docker").args(["pull", &image]).status() {
            Ok(status) if status.success() => {
                println!("Docker image pulled successfully: {}", image);
                return Some(image);
            }
            Ok(status) => {
                eprintln!("Docker pull exited with status {}: {}", status, image);
            }
            Err(err) => {
                eprintln!("Docker pull failed for {}: {}", image, err);
            }
        }
    }

    if let Some(image) = last_image {
        eprintln!("Last attempted docker image: {}", image);
    }
    None
}

fn docker_image_candidates() -> Vec<String> {
    if let Ok(image) = std::env::var("MIHOMOT_MIHOMO_IMAGE")
        && !image.trim().is_empty()
    {
        return vec![image];
    }

    let mirrors = ["hub.1panel.dev", "docker.1panel.live"];

    if is_likely_cn() {
        mirrors
            .iter()
            .map(|mirror| format!("{}/{}", mirror, MIHOMO_IMAGE))
            .chain(std::iter::once(MIHOMO_IMAGE.to_string()))
            .collect()
    } else {
        std::iter::once(MIHOMO_IMAGE.to_string())
            .chain(
                mirrors
                    .iter()
                    .map(|mirror| format!("{}/{}", mirror, MIHOMO_IMAGE)),
            )
            .collect()
    }
}

/// Download mihomo binary with CN-aware mirror logic
async fn download_binary(dest: &Path) -> Result<()> {
    let (os, arch) = detect_platform()?;
    let asset = find_latest_mihomo_asset(&os, &arch).await?;
    println!("Selected mihomo asset: {}", asset.name);

    fs_create_dir_all(dest)?;

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(900))
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()?;
    let urls = ranked_github_urls(&client, &asset.browser_download_url).await;

    let mut last_err = None;
    for url in &urls {
        println!("Trying: {}", url);
        match client.get(url).send().await {
            Ok(resp) if resp.status().is_success() => {
                let bytes = match download_response_with_progress(resp, "mihomo").await {
                    Ok(bytes) => bytes,
                    Err(err) => {
                        last_err = Some(format!("download body error from {url}: {err}"));
                        continue;
                    }
                };
                let binary = match decode_mihomo_asset(&asset.name, &bytes) {
                    Ok(binary) => binary,
                    Err(err) => {
                        last_err = Some(format!("decode error from {url}: {err:#}"));
                        continue;
                    }
                };
                std::fs::write(dest, &binary)?;
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

async fn find_latest_mihomo_asset(os: &str, arch: &str) -> Result<GithubAsset> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .redirect(reqwest::redirect::Policy::limited(5))
        .user_agent("mihomot")
        .build()?;

    let mut last_err = None;
    for url in github_api_urls(GITHUB_API_RELEASE_URL) {
        println!("Trying mihomo release metadata: {}", url);
        match client.get(&url).send().await {
            Ok(resp) if resp.status().is_success() => {
                let release = match resp.json::<GithubRelease>().await {
                    Ok(release) => release,
                    Err(err) => {
                        last_err = Some(format!("metadata decode error from {url}: {err}"));
                        continue;
                    }
                };
                match select_mihomo_asset(release.assets, os, arch) {
                    Ok(asset) => return Ok(asset),
                    Err(err) => {
                        last_err = Some(err.to_string());
                    }
                }
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
        "failed to read latest mihomo release metadata: {}",
        last_err.unwrap_or_else(|| "unknown".to_string())
    )
}

fn github_api_urls(source_url: &str) -> Vec<String> {
    let mut urls = vec![source_url.to_string()];

    if let Ok(proxy) = std::env::var("MIHOMOT_GITHUB_API_PROXY") {
        let proxy = proxy.trim();
        if proxy == "direct" {
            return urls;
        }
        if !proxy.is_empty() {
            urls.insert(0, proxied_url(proxy, source_url));
            return urls;
        }
    }

    if let Ok(proxy) = std::env::var("MIHOMOT_GITHUB_PROXY") {
        let proxy = proxy.trim();
        if proxy == "direct" {
            return urls;
        }
        if !proxy.is_empty() {
            urls.push(proxied_url(proxy, source_url));
        }
    }

    urls
}

fn select_mihomo_asset(assets: Vec<GithubAsset>, os: &str, arch: &str) -> Result<GithubAsset> {
    let prefix = format!("mihomo-{os}-{arch}");

    let selected = if os == "linux" && arch == "amd64" {
        assets
            .iter()
            .find(|asset| {
                asset.name.starts_with("mihomo-linux-amd64-compatible-")
                    && asset.name.ends_with(".gz")
            })
            .or_else(|| {
                assets.iter().find(|asset| {
                    asset.name.starts_with("mihomo-linux-amd64-v1-") && asset.name.ends_with(".gz")
                })
            })
            .or_else(|| {
                assets.iter().find(|asset| {
                    asset.name.starts_with("mihomo-linux-amd64-")
                        && asset.name.ends_with(".gz")
                        && !asset.name.contains("-go")
                        && !asset.name.contains("-v2-")
                        && !asset.name.contains("-v3-")
                })
            })
    } else {
        assets
            .iter()
            .find(|asset| asset.name.starts_with(&prefix) && asset.name.ends_with(".gz"))
    };

    selected.cloned().with_context(|| {
        let available = assets
            .iter()
            .map(|asset| asset.name.as_str())
            .filter(|name| name.starts_with(&prefix))
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "no matching mihomo asset for {os}/{arch}. Available matching assets: {}",
            if available.is_empty() {
                "(none)".to_string()
            } else {
                available
            }
        )
    })
}

fn decode_mihomo_asset(name: &str, bytes: &[u8]) -> Result<Vec<u8>> {
    if !name.ends_with(".gz") {
        return Ok(bytes.to_vec());
    }

    let mut decoder = GzDecoder::new(bytes);
    let mut decoded = Vec::new();
    decoder
        .read_to_end(&mut decoded)
        .with_context(|| format!("failed to decompress {name}"))?;
    Ok(decoded)
}

pub async fn ranked_github_urls(client: &reqwest::Client, source_url: &str) -> Vec<String> {
    let candidates = github_prefixes()
        .into_iter()
        .map(|prefix| proxied_url(&prefix, source_url))
        .collect::<Vec<_>>();

    let mut ranked = Vec::new();
    for url in &candidates {
        let started = Instant::now();
        let reachable = match client
            .get(url)
            .header(reqwest::header::RANGE, "bytes=0-1023")
            .timeout(Duration::from_secs(12))
            .send()
            .await
        {
            Ok(resp) => match resp.error_for_status() {
                Ok(resp) => resp
                    .bytes()
                    .await
                    .map(|bytes| !bytes.is_empty())
                    .unwrap_or(false),
                Err(_) => false,
            },
            Err(_) => false,
        };

        if reachable {
            let elapsed = started.elapsed();
            println!(
                "GitHub source reachable in {:.3}s: {}",
                elapsed.as_secs_f64(),
                url
            );
            ranked.push((elapsed, url.clone()));
        } else {
            eprintln!("GitHub source probe failed: {}", url);
        }
    }

    if ranked.is_empty() {
        return candidates;
    }

    ranked.sort_by_key(|(elapsed, _)| *elapsed);
    ranked.into_iter().map(|(_, url)| url).collect()
}

fn github_prefixes() -> Vec<String> {
    if let Ok(proxy) = std::env::var("MIHOMOT_GITHUB_PROXY") {
        let proxy = proxy.trim();
        if proxy == "direct" {
            return vec![String::new()];
        }
        if !proxy.is_empty() {
            return vec![proxy.to_string(), String::new()];
        }
    }

    let mut prefixes = vec![
        "https://gh-proxy.com/".to_string(),
        "https://gh.jasonzeng.dev/".to_string(),
        "https://ghfast.top/".to_string(),
        "https://gh.llkk.cc/".to_string(),
        String::new(),
    ];

    if !is_likely_cn() {
        prefixes.rotate_right(1);
    }

    prefixes
}

fn proxied_url(prefix: &str, source_url: &str) -> String {
    if prefix.is_empty() {
        source_url.to_string()
    } else {
        format!("{}{}", prefix, source_url)
    }
}

pub async fn download_response_with_progress(
    resp: reqwest::Response,
    label: &str,
) -> Result<Vec<u8>> {
    let total = resp.content_length();
    let mut stream = resp.bytes_stream();
    let mut bytes = Vec::new();
    let mut downloaded = 0u64;
    let started = Instant::now();
    let mut last_print = Instant::now()
        .checked_sub(Duration::from_secs(1))
        .unwrap_or_else(Instant::now);

    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        downloaded += chunk.len() as u64;
        bytes.extend_from_slice(&chunk);

        if last_print.elapsed() >= Duration::from_millis(500) {
            print_download_progress(label, downloaded, total, started);
            last_print = Instant::now();
        }
    }

    if downloaded == 0 {
        println!();
        return Ok(bytes);
    }

    print_download_progress(label, downloaded, total, started);
    println!();

    Ok(bytes)
}

fn print_download_progress(label: &str, downloaded: u64, total: Option<u64>, started: Instant) {
    let elapsed = started.elapsed().as_secs_f64().max(0.001);
    let downloaded_mb = downloaded as f64 / 1024.0 / 1024.0;
    let speed_mb = downloaded_mb / elapsed;

    if let Some(total) = total {
        let total_mb = total as f64 / 1024.0 / 1024.0;
        let percent = if total == 0 {
            100.0
        } else {
            downloaded as f64 * 100.0 / total as f64
        };
        eprint!(
            "\rDownloading {label}: {:>5.1}% ({:.1}/{:.1} MiB, {:.1} MiB/s)",
            percent.min(100.0),
            downloaded_mb,
            total_mb,
            speed_mb
        );
    } else {
        eprint!(
            "\rDownloading {label}: {:.1} MiB ({:.1} MiB/s)",
            downloaded_mb, speed_mb
        );
    }
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
    if let Ok(tz) = std::env::var("TZ")
        && (tz.contains("Shanghai") || tz.contains("Chongqing") || tz.contains("PRC"))
    {
        return true;
    }

    // Check locale
    if let Ok(lang) = std::env::var("LANG")
        && lang.starts_with("zh_CN")
    {
        return true;
    }

    false
}

fn fs_create_dir_all(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{GithubAsset, select_mihomo_asset};

    fn asset(name: &str) -> GithubAsset {
        GithubAsset {
            name: name.to_string(),
            browser_download_url: format!("https://example.invalid/{name}"),
        }
    }

    #[test]
    fn select_mihomo_asset_prefers_amd64_compatible_build() {
        let selected = select_mihomo_asset(
            vec![
                asset("mihomo-linux-amd64-v1-v1.19.25.gz"),
                asset("mihomo-linux-amd64-compatible-v1.19.25.gz"),
                asset("mihomo-linux-amd64-v3-v1.19.25.gz"),
            ],
            "linux",
            "amd64",
        )
        .unwrap();

        assert_eq!(selected.name, "mihomo-linux-amd64-compatible-v1.19.25.gz");
    }

    #[test]
    fn select_mihomo_asset_handles_arm64_gzip() {
        let selected = select_mihomo_asset(
            vec![
                asset("mihomo-linux-arm64-v1.19.25.deb"),
                asset("mihomo-linux-arm64-v1.19.25.gz"),
            ],
            "linux",
            "arm64",
        )
        .unwrap();

        assert_eq!(selected.name, "mihomo-linux-arm64-v1.19.25.gz");
    }
}

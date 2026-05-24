use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MihomoConfig {
    #[serde(default)]
    pub mode: String,
    #[serde(rename = "mixed-port", default)]
    pub mixed_port: Option<u16>,
    #[serde(rename = "external-controller", default)]
    pub external_controller: Option<String>,
    #[serde(default)]
    pub secret: Option<String>,
    #[serde(flatten)]
    pub extra: serde_yaml::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigCandidate {
    #[serde(skip)]
    pub path: PathBuf,
    pub label: String,
    pub detail: String,
}

/// Get the default mihomo config path.
pub fn default_config_path() -> PathBuf {
    if let Ok(path) = std::env::var("MIHOMOT_CONFIG")
        && !path.trim().is_empty()
    {
        return PathBuf::from(path);
    }

    let system_path = PathBuf::from("/etc/mihomo/config.yaml");
    if system_path.exists() {
        return system_path;
    }

    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home)
        .join(".config")
        .join("mihomo")
        .join("config.yaml")
}

/// Read the mihomo config.yaml file
pub fn read_config(path: &Path) -> Result<MihomoConfig> {
    let content =
        fs::read_to_string(path).with_context(|| format!("Failed to read {}", path.display()))?;
    let config: MihomoConfig =
        serde_yaml::from_str(&content).with_context(|| "Failed to parse config.yaml")?;
    Ok(config)
}

/// Read the raw config.yaml content
pub fn read_raw(path: &Path) -> Result<String> {
    fs::read_to_string(path).with_context(|| format!("Failed to read {}", path.display()))
}

/// Write config.yaml content (raw string), returns the path
pub fn write_raw(path: &Path, content: &str) -> Result<()> {
    // Validate it's valid YAML first
    serde_yaml::from_str::<serde_yaml::Value>(content).context("Invalid YAML content")?;

    fs::write(path, content).with_context(|| format!("Failed to write {}", path.display()))?;
    Ok(())
}

/// Create a backup of config.yaml with timestamp, returns backup path
pub fn backup_config(path: &Path) -> Result<PathBuf> {
    let timestamp = chrono_timestamp();
    let backup_path = path.with_extension(format!("yaml.bak.{}", timestamp));
    fs::copy(path, &backup_path)
        .with_context(|| format!("Failed to backup to {}", backup_path.display()))?;
    Ok(backup_path)
}

/// Restore config from a backup file
pub fn restore_from_backup(backup_path: &Path, target_path: &Path) -> Result<()> {
    fs::copy(backup_path, target_path)
        .with_context(|| format!("Failed to restore from {}", backup_path.display()))?;
    Ok(())
}

/// Return selectable mihomo YAML config files from the active config directory.
pub fn list_config_candidates(active_path: &Path) -> Result<Vec<(ConfigCandidate, MihomoConfig)>> {
    let base_dir = active_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let mut configs = Vec::new();

    if base_dir.exists() {
        for entry in fs::read_dir(&base_dir)? {
            let path = entry?.path();
            if let Some(config) = parse_if_mihomo_config_file(&path) {
                configs.push((path, config));
            }
        }
    }

    if let Some(config) = parse_if_mihomo_config_file(active_path)
        && !configs.iter().any(|(p, _)| p == active_path)
    {
        configs.push((active_path.to_path_buf(), config));
    }

    configs.sort_by(|left, right| {
        left.0
            .file_name()
            .cmp(&right.0.file_name())
            .then_with(|| left.0.cmp(&right.0))
    });

    Ok(configs
        .into_iter()
        .map(|(path, config)| (config_candidate_from_path(path, &base_dir), config))
        .collect())
}

fn parse_if_mihomo_config_file(path: &Path) -> Option<MihomoConfig> {
    if !is_yaml_file(path) {
        return None;
    }

    let Ok(config) = read_config(path) else {
        return None;
    };

    if config.external_controller.is_some()
        || config.mixed_port.is_some()
        || config.extra.get("proxies").is_some()
        || config.extra.get("proxy-groups").is_some()
        || config.extra.get("rules").is_some()
    {
        Some(config)
    } else {
        None
    }
}

/// Parse external-controller to get host and port
pub fn parse_external_controller(ec: &str) -> (String, u16) {
    let ec = ec.trim();
    if let Some(colon_pos) = ec.rfind(':') {
        let host = &ec[..colon_pos];
        let port = ec[colon_pos + 1..].parse::<u16>().unwrap_or(9090);
        (host.to_string(), port)
    } else {
        ("0.0.0.0".to_string(), 9090)
    }
}

fn is_yaml_file(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.is_file())
        && matches!(
            path.extension().and_then(|extension| extension.to_str()),
            Some("yaml" | "yml")
        )
}

fn config_candidate_from_path(path: PathBuf, base_dir: &Path) -> ConfigCandidate {
    let label = path
        .file_stem()
        .and_then(|value| value.to_str())
        .or_else(|| path.file_name().and_then(|value| value.to_str()))
        .unwrap_or("<unnamed>")
        .to_string();
    let detail_path = path.strip_prefix(base_dir).unwrap_or(&path);
    let detail = detail_path.display().to_string();

    ConfigCandidate {
        path,
        label,
        detail,
    }
}

fn chrono_timestamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let s = (secs % 60) as u32;
    let mins = secs / 60;
    let m = (mins % 60) as u32;
    let hours = mins / 60;
    let h = (hours % 24) as u32;
    let days = (hours / 24) as i64;

    // Civil date from days since Unix epoch (Howard Hinnant's algorithm)
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mo <= 3 { y + 1 } else { y };

    format!("{:04}{:02}{:02}-{:02}{:02}{:02}", y, mo, d, h, m, s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_external_controller_formats() {
        assert_eq!(
            parse_external_controller("0.0.0.0:9090"),
            ("0.0.0.0".to_string(), 9090)
        );
        assert_eq!(
            parse_external_controller("127.0.0.1:9091"),
            ("127.0.0.1".to_string(), 9091)
        );
        assert_eq!(parse_external_controller(":9090"), ("".to_string(), 9090));
        assert_eq!(
            parse_external_controller("9090"),
            ("0.0.0.0".to_string(), 9090)
        );
    }

    #[test]
    fn serialize_flatten() {
        let yaml = r#"
mode: rule
mixed-port: 7890
external-controller: "0.0.0.0:9090"
secret: test123
"#;
        let config: MihomoConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.mode, "rule");
        assert_eq!(config.mixed_port, Some(7890));
        assert_eq!(config.secret.as_deref(), Some("test123"));
    }

    #[test]
    fn default_config_path_honors_env_override() {
        unsafe {
            std::env::set_var("MIHOMOT_CONFIG", "/tmp/mihomot-test-config.yaml");
        }
        assert_eq!(
            default_config_path(),
            PathBuf::from("/tmp/mihomot-test-config.yaml")
        );
        unsafe {
            std::env::remove_var("MIHOMOT_CONFIG");
        }
    }

    #[test]
    fn list_config_candidates_filters_non_mihomo_yaml_files() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock should be after epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "mihomot-config-list-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).expect("test directory should be created");
        let active = dir.join("active.yaml");
        let other = dir.join("other.yml");
        let profiles = dir.join("profiles.yaml");
        let notes = dir.join("notes.txt");
        fs::write(&active, "mixed-port: 7890\n").expect("active config should be writable");
        fs::write(&other, "proxies: []\nproxy-groups: []\nrules: []\n")
            .expect("other config should be writable");
        fs::write(&profiles, "current: abc\nitems: []\n")
            .expect("metadata file should be writable");
        fs::write(&notes, "ignored\n").expect("ignored file should be writable");

        let candidates = list_config_candidates(&active).expect("candidate listing should succeed");
        let paths: Vec<_> = candidates
            .iter()
            .map(|(candidate, _)| candidate.path.clone())
            .collect();

        assert!(paths.contains(&active));
        assert!(paths.contains(&other));
        assert!(!paths.contains(&profiles));
        assert!(!paths.contains(&notes));
        assert!(candidates.iter().any(|(candidate, _)| {
            candidate.path == active
                && candidate.label == "active"
                && candidate.detail == "active.yaml"
        }));

        let _ = fs::remove_dir_all(&dir);
    }
}

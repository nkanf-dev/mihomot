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

/// Get the default mihomo config path: ~/.config/mihomo/config.yaml
pub fn default_config_path() -> PathBuf {
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
}

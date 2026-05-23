use anyhow::{Context, Result};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use std::fs;
use std::path::PathBuf;

#[cfg(test)]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ServerInfo {
    pub alias: String,
    pub endpoint: String,
    pub secret: String,
}

/// Generate a mihomot token: mhmt_{hostname}_{base64(secret)}
pub fn generate_token(secret: &str) -> Result<String> {
    let hostname = hostname::get()
        .context("Failed to get hostname")?
        .to_string_lossy()
        .to_string();

    let encoded = BASE64.encode(secret.as_bytes());

    Ok(format!("mhmt_{}_{}", hostname, encoded))
}

/// Parse a mihomot token back into its components.
/// `mihomot_addr` is the mihomot server address the agent connected to,
/// used as the endpoint since the real address may not be in the token (NAT).
#[cfg(test)]
fn parse_token(token: &str, mihomot_addr: &str) -> Result<ServerInfo> {
    let rest = token
        .strip_prefix("mhmt_")
        .context("Token does not start with 'mhmt_'")?;

    let sep_pos = rest
        .find('_')
        .context("Token missing separator after hostname")?;

    let alias = &rest[..sep_pos];
    let encoded = &rest[sep_pos + 1..];

    let decoded = BASE64
        .decode(encoded)
        .context("Failed to decode base64 in token")?;
    let secret =
        String::from_utf8(decoded).context("Token payload is not valid UTF-8")?;

    Ok(ServerInfo {
        alias: alias.to_string(),
        endpoint: mihomot_addr.to_string(),
        secret,
    })
}

/// Save token to ~/.config/mihomot/token
pub fn save_token(token: &str) -> Result<PathBuf> {
    let path = config_dir().join("token");
    fs::create_dir_all(path.parent().unwrap())?;
    fs::write(&path, token)?;
    Ok(path)
}

fn config_dir() -> PathBuf {
    dirs_or_default().join(".config").join("mihomot")
}

fn dirs_or_default() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_token() {
        let secret = "mysecret";
        let token = generate_token(secret).unwrap();

        assert!(token.starts_with("mhmt_"));

        let info = parse_token(&token, "http://1.2.3.4:9091").unwrap();
        assert_eq!(info.endpoint, "http://1.2.3.4:9091");
        assert_eq!(info.secret, secret);
    }

    #[test]
    fn parse_known_token() {
        // secret "mihomo" base64 = bWlob21v
        let token = "mhmt_hk-server_bWlob21v";
        let info = parse_token(token, "http://10.0.0.1:9091").unwrap();
        assert_eq!(info.alias, "hk-server");
        assert_eq!(info.endpoint, "http://10.0.0.1:9091");
        assert_eq!(info.secret, "mihomo");
    }

    #[test]
    fn parse_bad_prefix() {
        assert!(parse_token("bad_token", "http://x:9091").is_err());
    }
}

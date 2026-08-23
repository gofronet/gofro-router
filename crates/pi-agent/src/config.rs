use std::{fs, path::Path};

use anyhow::{Context, Result, bail};

use crate::model::{ControllerConfig, ServerProfile};

pub(crate) fn validate_server(server: &ServerProfile) -> Result<()> {
    if server.name.is_empty() || server.name.len() > 40 || server.name.chars().any(char::is_control)
    {
        bail!("имя сервера должно содержать от 1 до 40 символов");
    }
    validate_endpoint(&server.endpoint)?;
    if server.public_key.len() != 44
        || !server
            .public_key
            .chars()
            .enumerate()
            .all(|(index, character)| {
                character.is_ascii_alphanumeric()
                    || character == '+'
                    || character == '/'
                    || (character == '=' && index == 43)
            })
    {
        bail!("некорректный WireGuard public key");
    }
    Ok(())
}

fn validate_endpoint(endpoint: &str) -> Result<()> {
    if endpoint.is_empty() || endpoint.len() > 255 || endpoint.chars().any(char::is_whitespace) {
        bail!("некорректный endpoint");
    }
    let (host, port) = endpoint
        .rsplit_once(':')
        .context("endpoint должен иметь формат host:port")?;
    if host.is_empty() || port.parse::<u16>().ok().filter(|port| *port > 0).is_none() {
        bail!("endpoint должен иметь формат host:port");
    }
    Ok(())
}

pub(crate) fn validate_ssid(ssid: &str) -> Result<()> {
    if ssid.is_empty() || ssid.len() > 32 || ssid.chars().any(char::is_control) {
        bail!("название Wi-Fi должно содержать от 1 до 32 байт");
    }
    Ok(())
}

pub(crate) fn load(path: &Path) -> Result<ControllerConfig> {
    let contents =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    let config: ControllerConfig =
        serde_json::from_str(&contents).with_context(|| format!("invalid {}", path.display()))?;
    for server in &config.servers {
        validate_server(server)?;
    }
    validate_ssid(&config.ap_ssid)?;
    Ok(config)
}

pub(crate) fn save(path: &Path, config: &ControllerConfig) -> Result<()> {
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, serde_json::to_vec_pretty(config)?)
        .with_context(|| format!("failed to write {}", temporary.display()))?;
    fs::rename(&temporary, path).with_context(|| format!("failed to replace {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_server_profile() {
        let server = ServerProfile {
            name: "Primary".into(),
            endpoint: "vpn.example.com:8443".into(),
            public_key: "aq2K6tZ6JqYCpNPLseGJPHceMMxxEdkx5AeRm6cEfSE=".into(),
        };
        assert!(validate_server(&server).is_ok());
        assert!(validate_endpoint("missing-port").is_err());
    }
}

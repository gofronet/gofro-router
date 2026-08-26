use std::{fs, net::Ipv4Addr, path::Path};

use anyhow::{Context, Result, bail};
use ipnet::Ipv4Net;

use crate::model::{ControllerConfig, DomainMatch, IpMatch, RoutingConfig, ServerProfile};

const MAX_RULES: usize = 128;

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

pub(crate) fn normalize_routing(routing: &mut RoutingConfig) -> Result<()> {
    if routing.domain_rules.len() > MAX_RULES || routing.ip_rules.len() > MAX_RULES {
        bail!("допускается не более {MAX_RULES} правил каждого типа");
    }
    for rule in &mut routing.domain_rules {
        normalize_rule_name(&mut rule.name)?;
        match &mut rule.matcher {
            DomainMatch::Exact { value } | DomainMatch::Suffix { value } => {
                *value = normalize_domain(value)?;
            }
            DomainMatch::GeoSite { value } => *value = normalize_tag(value)?,
        }
    }
    for rule in &mut routing.ip_rules {
        normalize_rule_name(&mut rule.name)?;
        match &mut rule.matcher {
            IpMatch::Cidr { value } => {
                let network = value
                    .parse::<Ipv4Net>()
                    .context("правило CIDR должно содержать корректную IPv4-сеть")?;
                let fake_dns = Ipv4Net::new(Ipv4Addr::new(198, 18, 0, 0), 15)
                    .expect("the fixed FakeDNS network is valid");
                if network.contains(&fake_dns.network()) || fake_dns.contains(&network.network()) {
                    bail!("диапазон FakeDNS 198.18.0.0/15 зарезервирован");
                }
                *value = network.trunc().to_string();
            }
            IpMatch::GeoIp { value } => *value = normalize_tag(value)?,
        }
    }
    Ok(())
}

fn normalize_rule_name(name: &mut String) -> Result<()> {
    *name = name.trim().to_owned();
    if name.is_empty() || name.len() > 64 || name.chars().any(char::is_control) {
        bail!("название правила должно содержать от 1 до 64 символов");
    }
    Ok(())
}

pub(crate) fn normalize_domain(value: &str) -> Result<String> {
    let value = value.trim().trim_end_matches('.').to_lowercase();
    let ascii = idna::domain_to_ascii(&value).context("некорректное доменное имя")?;
    if ascii.is_empty()
        || ascii.len() > 253
        || ascii.split('.').any(|label| {
            label.is_empty()
                || label.len() > 63
                || label.starts_with('-')
                || label.ends_with('-')
                || !label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
    {
        bail!("некорректное доменное имя");
    }
    Ok(ascii)
}

fn normalize_tag(value: &str) -> Result<String> {
    let value = value.trim().to_ascii_lowercase();
    if value.is_empty()
        || value.len() > 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'!' | b'-' | b'_'))
    {
        bail!("некорректный тег GeoSite/GeoIP");
    }
    Ok(value)
}

pub(crate) fn load(path: &Path) -> Result<ControllerConfig> {
    let contents =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    let mut config: ControllerConfig =
        serde_json::from_str(&contents).with_context(|| format!("invalid {}", path.display()))?;
    for server in &config.servers {
        validate_server(server)?;
    }
    normalize_routing(&mut config.routing)?;
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

    #[test]
    fn ignores_legacy_ap_ssid() {
        let config: ControllerConfig = serde_json::from_str(
            r#"{"vpn_enabled":false,"active_server_key":null,"servers":[],"ap_ssid":"Old Wi-Fi"}"#,
        )
        .unwrap();
        assert!(
            serde_json::to_value(config)
                .unwrap()
                .get("ap_ssid")
                .is_none()
        );
    }

    #[test]
    fn normalizes_routing_rules() {
        let mut routing = RoutingConfig {
            domain_rules: vec![crate::model::DomainRule {
                name: " RU ".into(),
                enabled: true,
                matcher: DomainMatch::Suffix {
                    value: "РФ.".into(),
                },
                target: crate::model::RouteTarget::Direct,
            }],
            ip_rules: vec![crate::model::IpRule {
                name: "LAN".into(),
                enabled: true,
                matcher: IpMatch::Cidr {
                    value: "10.0.0.1/8".into(),
                },
                target: crate::model::RouteTarget::Direct,
            }],
            default_target: crate::model::RouteTarget::Vpn,
        };
        normalize_routing(&mut routing).unwrap();
        assert!(matches!(
            &routing.domain_rules[0].matcher,
            DomainMatch::Suffix { value } if value == "xn--p1ai"
        ));
        assert!(matches!(
            &routing.ip_rules[0].matcher,
            IpMatch::Cidr { value } if value == "10.0.0.0/8"
        ));
        assert_eq!(normalize_tag("GEOLOCATION-!CN").unwrap(), "geolocation-!cn");
    }
}

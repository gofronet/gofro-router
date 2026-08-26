use std::{
    fs::{self, OpenOptions, Permissions},
    io::Write,
    net::Ipv4Addr,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::Path,
};

use anyhow::{Context, Result, bail};
use ipnet::Ipv4Net;

use crate::model::{ControllerConfig, DomainMatch, IpMatch, RoutingConfig, ServerProfile};

const MAX_RULES: usize = 128;
const MAX_PROFILE_SIZE: usize = 4096;
const CLIENT_TUNNEL_ADDRESS: &str = "10.202.0.2/32";
const ALLOWED_IPS: &str = "0.0.0.0/0";
const KEEPALIVE: u16 = 10;
const MTU: u16 = 1360;

pub(crate) fn validate_server(server: &ServerProfile) -> Result<()> {
    if server.name.is_empty() || server.name.len() > 40 || server.name.chars().any(char::is_control)
    {
        bail!("имя сервера должно содержать от 1 до 40 символов");
    }
    validate_endpoint(&server.endpoint)?;
    validate_wireguard_key(&server.public_key, "public")?;
    if let Some(private_key) = &server.client_private_key {
        validate_wireguard_key(private_key, "private")?;
    }
    Ok(())
}

fn validate_wireguard_key(key: &str, kind: &str) -> Result<()> {
    let bytes = key.as_bytes();
    if bytes.len() != 44
        || bytes[43] != b'='
        || !bytes[..43]
            .iter()
            .copied()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/'))
    {
        bail!("некорректный WireGuard {kind} key");
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

pub(crate) fn parse_server_profile(name: String, profile: &str) -> Result<ServerProfile> {
    if profile.len() > MAX_PROFILE_SIZE {
        bail!("WireGuard-профиль слишком большой");
    }

    let mut section = "";
    let mut private_key = None;
    let mut address = None;
    let mut mtu = None;
    let mut public_key = None;
    let mut allowed_ips = None;
    let mut endpoint = None;
    let mut keepalive = None;

    for raw_line in profile.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            section = match line {
                "[Interface]" => "interface",
                "[Peer]" => "peer",
                _ => bail!("неподдерживаемая секция WireGuard-профиля: {line}"),
            };
            continue;
        }
        let (key, value) = line
            .split_once('=')
            .context("строка WireGuard-профиля должна иметь формат Key = Value")?;
        let key = key.trim();
        let value = value.trim();
        if value.is_empty() {
            bail!("пустое значение {key} в WireGuard-профиле");
        }
        match (section, key) {
            ("interface", "PrivateKey") => set_profile_value(&mut private_key, value, key)?,
            ("interface", "Address") => set_profile_value(&mut address, value, key)?,
            ("interface", "MTU") => set_profile_value(&mut mtu, value, key)?,
            ("peer", "PublicKey") => set_profile_value(&mut public_key, value, key)?,
            ("peer", "AllowedIPs") => set_profile_value(&mut allowed_ips, value, key)?,
            ("peer", "Endpoint") => set_profile_value(&mut endpoint, value, key)?,
            ("peer", "PersistentKeepalive") => {
                set_profile_value(&mut keepalive, value, key)?;
            }
            ("", _) => bail!("параметр {key} находится вне секции WireGuard-профиля"),
            _ => bail!("неподдерживаемый параметр WireGuard-профиля: {key}"),
        }
    }

    let address = required_profile_value(address, "Address")?;
    if address != CLIENT_TUNNEL_ADDRESS {
        bail!("Gofro поддерживает Address = {CLIENT_TUNNEL_ADDRESS}");
    }
    let allowed_ips = required_profile_value(allowed_ips, "AllowedIPs")?
        .split(',')
        .map(str::trim)
        .collect::<Vec<_>>()
        .join(",");
    if allowed_ips != ALLOWED_IPS {
        bail!("Gofro поддерживает AllowedIPs = {ALLOWED_IPS}");
    }
    let persistent_keepalive: u16 = required_profile_value(keepalive, "PersistentKeepalive")?
        .parse()
        .context("PersistentKeepalive должен быть целым числом")?;
    if persistent_keepalive != KEEPALIVE {
        bail!("Gofro поддерживает PersistentKeepalive = {KEEPALIVE}");
    }
    let mtu: u16 = required_profile_value(mtu, "MTU")?
        .parse()
        .context("MTU должен быть целым числом")?;
    if mtu != MTU {
        bail!("Gofro поддерживает MTU = {MTU}");
    }

    let mut server = ServerProfile {
        name,
        endpoint: required_profile_value(endpoint, "Endpoint")?,
        public_key: required_profile_value(public_key, "PublicKey")?,
        client_private_key: Some(required_profile_value(private_key, "PrivateKey")?),
    };
    server.name = server.name.trim().to_owned();
    validate_server(&server)?;
    Ok(server)
}

fn set_profile_value(slot: &mut Option<String>, value: &str, key: &str) -> Result<()> {
    if slot.replace(value.to_owned()).is_some() {
        bail!("параметр {key} указан несколько раз");
    }
    Ok(())
}

fn required_profile_value(value: Option<String>, key: &str) -> Result<String> {
    value.with_context(|| format!("в WireGuard-профиле отсутствует {key}"))
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
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(&temporary)
        .with_context(|| format!("failed to open {}", temporary.display()))?;
    file.set_permissions(Permissions::from_mode(0o600))
        .with_context(|| format!("failed to protect {}", temporary.display()))?;
    file.write_all(&serde_json::to_vec_pretty(config)?)
        .with_context(|| format!("failed to write {}", temporary.display()))?;
    drop(file);
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
            client_private_key: None,
        };
        assert!(validate_server(&server).is_ok());
        assert!(validate_endpoint("missing-port").is_err());
    }

    #[test]
    fn parses_profile_without_exposing_private_key() {
        let profile = r#"
            [Interface]
            PrivateKey = 4E64fyqMJsXY6YaAp8M3qM7r6Xj6YjAfuPeWbdMvIHE=
            Address = 10.202.0.2/32
            MTU = 1360

            [Peer]
            PublicKey = aq2K6tZ6JqYCpNPLseGJPHceMMxxEdkx5AeRm6cEfSE=
            AllowedIPs = 0.0.0.0/0
            Endpoint = vpn.example.com:8443
            PersistentKeepalive = 10
        "#;
        let server = parse_server_profile("Primary".into(), profile).unwrap();
        assert!(server.client_private_key.is_some());
        let status = crate::model::ServerStatus::from(&server);
        let json = serde_json::to_string(&status).unwrap();
        assert!(!json.contains("client_private_key"));
        assert!(!json.contains("4E64fyqMJsXY6YaAp8M3qM7r6Xj6YjAfuPeWbdMvIHE="));
        assert!(
            parse_server_profile("Primary".into(), &format!("{profile}\nDNS = 1.1.1.1")).is_err()
        );
        assert!(
            parse_server_profile(
                "Primary".into(),
                &profile.replace("MTU = 1360", "MTU = 1280")
            )
            .is_err()
        );
    }

    #[test]
    fn saves_private_keys_with_owner_only_permissions() {
        let path = std::env::temp_dir().join(format!("gofro-config-{}.json", std::process::id()));
        let config = ControllerConfig {
            vpn_enabled: false,
            active_server_key: None,
            servers: vec![ServerProfile {
                name: "Private".into(),
                endpoint: "vpn.example.com:8443".into(),
                public_key: "aq2K6tZ6JqYCpNPLseGJPHceMMxxEdkx5AeRm6cEfSE=".into(),
                client_private_key: Some("4E64fyqMJsXY6YaAp8M3qM7r6Xj6YjAfuPeWbdMvIHE=".into()),
            }],
            routing: RoutingConfig::default(),
        };

        save(&path, &config).unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(
            fs::read_to_string(&path)
                .unwrap()
                .contains("client_private_key")
        );
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn ignores_legacy_ap_ssid() {
        let config: ControllerConfig = serde_json::from_str(
            r#"{"vpn_enabled":false,"active_server_key":null,"servers":[{"name":"Old","endpoint":"vpn.example.com:8443","public_key":"aq2K6tZ6JqYCpNPLseGJPHceMMxxEdkx5AeRm6cEfSE="}],"ap_ssid":"Old Wi-Fi"}"#,
        )
        .unwrap();
        assert!(config.servers[0].client_private_key.is_none());
        assert!(
            serde_json::to_value(&config)
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

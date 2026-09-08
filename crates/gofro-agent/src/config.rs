use std::{
    fs::{self, OpenOptions, Permissions},
    io::Write,
    net::{IpAddr, Ipv4Addr},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::Path,
};

use anyhow::{Context, Result, bail};
use ipnet::Ipv4Net;

use crate::model::{
    ControllerConfig, DomainMatch, IpMatch, ManagedServer, RoutingConfig, RuleRef, ServerProfile,
};

const MAX_RULES: usize = 128;
const MAX_PROFILE_SIZE: usize = 4096;
const ALLOWED_IPS: &str = "0.0.0.0/0";
const KEEPALIVE: u16 = 10;
const MTU: u16 = 1280;

pub(crate) fn validate_server(server: &ServerProfile) -> Result<()> {
    validate_server_name(&server.name)?;
    validate_emoji(&server.emoji)?;
    validate_endpoint(&server.endpoint)?;
    validate_wireguard_key(&server.public_key, "public")?;
    if let Some(address) = &server.client_tunnel_address {
        normalize_tunnel_address(address)?;
    }
    if let Some(private_key) = &server.client_private_key {
        validate_wireguard_key(private_key, "private")?;
    }
    if let Some(management) = &server.management {
        validate_management(management)?;
    }
    Ok(())
}

pub(crate) fn normalize_server_name(name: &mut String) -> Result<()> {
    *name = name.trim().to_owned();
    validate_server_name(name)
}

fn validate_server_name(name: &str) -> Result<()> {
    if name.is_empty() || name.chars().count() > 60 || name.chars().any(char::is_control) {
        bail!("имя сервера должно содержать от 1 до 60 символов");
    }
    Ok(())
}

fn validate_emoji(emoji: &str) -> Result<()> {
    if emoji.len() > 32 || emoji.chars().any(char::is_control) {
        bail!("некорректный значок сервера");
    }
    Ok(())
}

pub(crate) fn validate_management(management: &ManagedServer) -> Result<()> {
    let host = management.host.parse::<IpAddr>();
    if host.as_ref().is_err()
        || host
            .ok()
            .is_some_and(|host| host.to_string() != management.host)
        || management.port == 0
    {
        bail!("управляемый сервер должен иметь IP-адрес и ненулевой SSH-порт");
    }
    crate::managed::parse_host_key(&management.host_key)?;
    Ok(())
}

fn normalize_tunnel_address(address: &str) -> Result<String> {
    let network = address
        .parse::<Ipv4Net>()
        .context("некорректный Address в WireGuard-профиле")?;
    let octets = network.addr().octets();
    if network.prefix_len() != 32
        || octets[0] != 10
        || octets[1] != 202
        || octets[2] != 0
        || !(2..=254).contains(&octets[3])
    {
        bail!("Gofro поддерживает Address из диапазона 10.202.0.2-10.202.0.254/32");
    }
    Ok(network.to_string())
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

    let client_tunnel_address =
        normalize_tunnel_address(&required_profile_value(address, "Address")?)?;
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
    if !matches!(mtu, MTU | 1360) {
        bail!("Gofro поддерживает MTU = {MTU} или 1360");
    }

    let mut server = ServerProfile {
        name,
        emoji: String::new(),
        endpoint: required_profile_value(endpoint, "Endpoint")?,
        public_key: required_profile_value(public_key, "PublicKey")?,
        client_tunnel_address: Some(client_tunnel_address),
        client_private_key: Some(required_profile_value(private_key, "PrivateKey")?),
        management: None,
    };
    normalize_server_name(&mut server.name)?;
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
    if let Some(order) = &routing.rule_order {
        if order.len() != routing.domain_rules.len() + routing.ip_rules.len() {
            bail!("порядок правил должен содержать каждое правило ровно один раз");
        }
        let mut domains = vec![false; routing.domain_rules.len()];
        let mut ips = vec![false; routing.ip_rules.len()];
        for reference in order {
            let (seen, kind, index) = match *reference {
                RuleRef::Domain { index } => (&mut domains, "domain", index),
                RuleRef::Ip { index } => (&mut ips, "ip", index),
            };
            let Some(slot) = seen.get_mut(index) else {
                bail!("{kind} правило в порядке отсутствует");
            };
            if std::mem::replace(slot, true) {
                bail!("правило в порядке указано несколько раз");
            }
        }
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
    for server in &mut config.servers {
        normalize_server_name(&mut server.name)?;
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
            emoji: String::new(),
            endpoint: "vpn.example.com:8443".into(),
            public_key: "aq2K6tZ6JqYCpNPLseGJPHceMMxxEdkx5AeRm6cEfSE=".into(),
            client_tunnel_address: Some("10.202.0.2/32".into()),
            client_private_key: None,
            management: None,
        };
        assert!(validate_server(&server).is_ok());
        assert!(validate_endpoint("missing-port").is_err());
    }

    #[test]
    fn parses_profile_without_exposing_private_key() {
        let profile = r#"
            [Interface]
            PrivateKey = 4E64fyqMJsXY6YaAp8M3qM7r6Xj6YjAfuPeWbdMvIHE=
            Address = 10.202.0.5/32
            MTU = 1280

            [Peer]
            PublicKey = aq2K6tZ6JqYCpNPLseGJPHceMMxxEdkx5AeRm6cEfSE=
            AllowedIPs = 0.0.0.0/0
            Endpoint = vpn.example.com:8443
            PersistentKeepalive = 10
        "#;
        let server = parse_server_profile("Primary".into(), profile).unwrap();
        assert_eq!(
            server.client_tunnel_address.as_deref(),
            Some("10.202.0.5/32")
        );
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
                &profile.replace("MTU = 1280", "MTU = 1360")
            )
            .is_ok()
        );
        assert!(
            parse_server_profile(
                "Primary".into(),
                &profile.replace("10.202.0.5/32", "10.202.0.1/32")
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
                emoji: "🛰".into(),
                endpoint: "vpn.example.com:8443".into(),
                public_key: "aq2K6tZ6JqYCpNPLseGJPHceMMxxEdkx5AeRm6cEfSE=".into(),
                client_tunnel_address: Some("10.202.0.2/32".into()),
                client_private_key: Some("4E64fyqMJsXY6YaAp8M3qM7r6Xj6YjAfuPeWbdMvIHE=".into()),
                management: None,
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
        assert_eq!(config.servers[0].client_tunnel_address, None);
        assert!(config.servers[0].client_private_key.is_none());
        assert!(
            serde_json::to_value(&config)
                .unwrap()
                .get("ap_ssid")
                .is_none()
        );
    }

    #[test]
    fn serializes_emoji_and_accepts_legacy_profiles() {
        let legacy: ServerProfile = serde_json::from_str(
            r#"{"name":" Old ","endpoint":"vpn.example.com:8443","public_key":"aq2K6tZ6JqYCpNPLseGJPHceMMxxEdkx5AeRm6cEfSE="}"#,
        )
        .unwrap();
        assert!(legacy.emoji.is_empty());
        let mut named = legacy.clone();
        normalize_server_name(&mut named.name).unwrap();
        named.emoji = "🛰".into();
        validate_server(&named).unwrap();
        let output = serde_json::to_string(&named).unwrap();
        assert!(output.contains("emoji"));
        assert!(validate_emoji("\n").is_err());
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
            mode: crate::model::RoutingMode::Rules,
            rule_order: None,
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

    #[test]
    fn rejects_incomplete_duplicate_and_out_of_range_rule_order() {
        let mut routing = RoutingConfig {
            rule_order: Some(vec![RuleRef::Domain { index: 0 }]),
            ..RoutingConfig::default()
        };
        assert!(normalize_routing(&mut routing).is_err());
        routing.rule_order = Some(vec![
            RuleRef::Domain { index: 0 },
            RuleRef::Domain { index: 0 },
        ]);
        assert!(normalize_routing(&mut routing).is_err());
        routing.rule_order = Some(vec![RuleRef::Domain { index: 0 }, RuleRef::Ip { index: 1 }]);
        assert!(normalize_routing(&mut routing).is_err());
    }
}

use std::net::Ipv4Addr;

use ipnet::Ipv4Net;
use serde::{Deserialize, Serialize};
use wireguard_status::PeerStatus;

use crate::stats::{HistoryPoint, LiveStats};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(crate) struct MacAddress([u8; 6]);

impl std::str::FromStr for MacAddress {
    type Err = anyhow::Error;
    fn from_str(value: &str) -> anyhow::Result<Self> {
        anyhow::ensure!(value.len() == 17, "invalid MAC address");
        let mut bytes = [0; 6];
        let parts: Vec<_> = value.split(':').collect();
        anyhow::ensure!(parts.len() == 6, "invalid MAC address");
        for (byte, part) in bytes.iter_mut().zip(parts) {
            anyhow::ensure!(
                part.len() == 2 && part.bytes().all(|b| b.is_ascii_hexdigit()),
                "invalid MAC address"
            );
            *byte = u8::from_str_radix(part, 16)?;
        }
        anyhow::ensure!(
            bytes != [0; 6] && bytes[0] & 1 == 0,
            "MAC address must be nonzero unicast"
        );
        Ok(Self(bytes))
    }
}

impl std::fmt::Display for MacAddress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let [a, b, c, d, e, g] = self.0;
        write!(f, "{a:02x}:{b:02x}:{c:02x}:{d:02x}:{e:02x}:{g:02x}")
    }
}
impl Serialize for MacAddress {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}
impl<'de> Deserialize<'de> for MacAddress {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

#[derive(Deserialize)]
pub(crate) struct DeviceExclusionInput {
    pub(crate) mac: MacAddress,
    pub(crate) excluded: bool,
}

pub(crate) const AP_DOMAIN: &str = "wifi.gofro.net";
pub(crate) const PANEL_VIRTUAL_IP: Ipv4Addr = Ipv4Addr::new(198, 18, 0, 0);

#[derive(Clone, Copy)]
pub(crate) struct PanelPorts {
    pub(crate) http: u16,
    pub(crate) https: u16,
}

impl PanelPorts {
    pub(crate) fn validate(self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.http != 0 && self.https != 0,
            "HTTP and HTTPS listener ports must be nonzero"
        );
        anyhow::ensure!(
            ![443, 8443].contains(&self.http)
                && ![80, 8081].contains(&self.https)
                && self.http != self.https,
            "HTTP and HTTPS listener ports must not overlap each other or the opposite protocol's panel ports"
        );
        Ok(())
    }
}

#[derive(Clone)]
pub(crate) struct LanContext {
    pub(crate) device: String,
    pub(crate) address: Ipv4Addr,
    pub(crate) subnet: Ipv4Net,
}

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct ControllerConfig {
    #[serde(default)]
    pub(crate) auto_update_enabled: bool,
    #[serde(default)]
    pub(crate) device_exclusions: Vec<MacAddress>,
    pub(crate) vpn_enabled: bool,
    pub(crate) active_server_key: Option<String>,
    pub(crate) servers: Vec<ServerProfile>,
    #[serde(default)]
    pub(crate) routing: RoutingConfig,
}

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct ServerProfile {
    pub(crate) name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) emoji: String,
    pub(crate) endpoint: String,
    pub(crate) public_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) client_tunnel_address: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) client_private_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) management: Option<ManagedServer>,
}

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct ManagedServer {
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) host_key: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum BootstrapStage {
    Waiting,
    HostKey,
    Connect,
    Inspect,
    Install,
    Authorize,
    Profile,
    Save,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum BootstrapEvent {
    Stage {
        stage: BootstrapStage,
    },
    Complete {
        status: Box<AgentStatus>,
    },
    Error {
        stage: BootstrapStage,
        message: String,
    },
}

#[derive(Debug, Serialize)]
pub(crate) struct ServerStatus {
    pub(crate) name: String,
    pub(crate) emoji: String,
    pub(crate) endpoint: String,
    pub(crate) public_key: String,
    pub(crate) managed: bool,
}

impl From<&ServerProfile> for ServerStatus {
    fn from(server: &ServerProfile) -> Self {
        Self {
            name: server.name.clone(),
            emoji: server.emoji.clone(),
            endpoint: server.endpoint.clone(),
            public_key: server.public_key.clone(),
            managed: server.management.is_some(),
        }
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct ModeInput {
    pub(crate) vpn_enabled: bool,
}

#[derive(Debug, Deserialize)]
pub(crate) struct UpdateInput {}

#[derive(Debug, Deserialize)]
pub(crate) struct AutoUpdateInput {
    pub(crate) enabled: bool,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ServerKeyInput {
    pub(crate) public_key: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ServerUpdate {
    pub(crate) previous_public_key: String,
    pub(crate) name: String,
    pub(crate) endpoint: String,
    pub(crate) public_key: String,
    #[serde(default)]
    pub(crate) emoji: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct ProfileInput {
    pub(crate) name: String,
    pub(crate) profile: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct RoutingConfig {
    pub(crate) domain_rules: Vec<DomainRule>,
    pub(crate) ip_rules: Vec<IpRule>,
    pub(crate) default_target: RouteTarget,
    #[serde(default)]
    pub(crate) mode: RoutingMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) rule_order: Option<Vec<RuleRef>>,
}

impl Default for RoutingConfig {
    fn default() -> Self {
        Self {
            domain_rules: vec![DomainRule {
                name: "Российские сайты".to_owned(),
                enabled: true,
                matcher: DomainMatch::GeoSite {
                    value: "category-ru".to_owned(),
                },
                target: RouteTarget::Direct,
            }],
            ip_rules: vec![IpRule {
                name: "Российские IP".to_owned(),
                enabled: true,
                matcher: IpMatch::GeoIp {
                    value: "ru".to_owned(),
                },
                target: RouteTarget::Direct,
            }],
            default_target: RouteTarget::Vpn,
            mode: RoutingMode::Rules,
            rule_order: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RoutingMode {
    #[default]
    Rules,
    All,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum RuleRef {
    Domain { index: usize },
    Ip { index: usize },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct DomainRule {
    pub(crate) name: String,
    pub(crate) enabled: bool,
    pub(crate) matcher: DomainMatch,
    pub(crate) target: RouteTarget,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum DomainMatch {
    Exact { value: String },
    Suffix { value: String },
    GeoSite { value: String },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct IpRule {
    pub(crate) name: String,
    pub(crate) enabled: bool,
    pub(crate) matcher: IpMatch,
    pub(crate) target: RouteTarget,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum IpMatch {
    Cidr { value: String },
    GeoIp { value: String },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RouteTarget {
    Direct,
    Vpn,
    Block,
}

#[derive(Debug, Deserialize)]
pub(crate) struct RoutingTestInput {
    pub(crate) value: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct RoutingTestResult {
    pub(crate) value: String,
    pub(crate) target: RouteTarget,
    pub(crate) matched_rule: Option<String>,
    pub(crate) scope: RoutingTestScope,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RoutingTestScope {
    Ip,
    DomainPreview,
}

#[derive(Debug, Serialize)]
pub(crate) struct RoutingStatus {
    pub(crate) config: RoutingConfig,
    pub(crate) dns_active: bool,
    pub(crate) fake_ips: usize,
    pub(crate) geosite_loaded: bool,
    pub(crate) geoip_loaded: bool,
    pub(crate) dataplane_active: bool,
    pub(crate) degraded: bool,
}

#[derive(Debug, Serialize)]
pub(crate) struct AgentStatus {
    pub(crate) device_exclusions: Vec<MacAddress>,
    pub(crate) version: &'static str,
    pub(crate) update: UpdateStatus,
    pub(crate) vpn_enabled: bool,
    pub(crate) tunnel_active: bool,
    pub(crate) interface: String,
    pub(crate) active_server_key: Option<String>,
    pub(crate) servers: Vec<ServerStatus>,
    pub(crate) peer: Option<PeerStatus>,
    pub(crate) stats: LiveStats,
    pub(crate) history: Vec<HistoryPoint>,
    pub(crate) routing: RoutingStatus,
}

#[derive(Debug, Serialize)]
pub(crate) struct UpdateStatus {
    pub(crate) auto_update_enabled: bool,
    pub(crate) running: bool,
    pub(crate) result: Option<UpdateResult>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum UpdateResult {
    Current,
    Updated,
    Failed,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mac_addresses_are_validated_and_canonical() {
        let mac: MacAddress = "02:AB:CD:ef:00:01".parse().unwrap();
        assert_eq!(mac.to_string(), "02:ab:cd:ef:00:01");
        assert_eq!(
            serde_json::to_string(&mac).unwrap(),
            "\"02:ab:cd:ef:00:01\""
        );
        assert_eq!(
            serde_json::from_str::<MacAddress>("\"02:AB:CD:EF:00:01\"").unwrap(),
            mac
        );
        for value in [
            "00:00:00:00:00:00",
            "ff:ff:ff:ff:ff:ff",
            "01:00:00:00:00:01",
            "02-00-00-00-00-01",
            "2:00:00:00:00:01",
            "02:00:00:00:00:gg",
            "02:00:00:00:00:01\n",
            "02:00:00:00:00:01; drop",
        ] {
            assert!(value.parse::<MacAddress>().is_err(), "{value}");
        }
        let legacy: ControllerConfig =
            serde_json::from_str(r#"{"vpn_enabled":false,"active_server_key":null,"servers":[]}"#)
                .unwrap();
        assert!(legacy.device_exclusions.is_empty());
        assert!(!legacy.auto_update_enabled);
    }

    #[test]
    fn panel_ports_reject_zero_and_overlapping_protocol_roles() {
        for (http, https) in [(8081, 8443), (80, 443), (9081, 9443), (8444, 8082)] {
            assert!(PanelPorts { http, https }.validate().is_ok());
        }
        for (http, https) in [
            (0, 8443),
            (8081, 0),
            (443, 9443),
            (8443, 9443),
            (9081, 80),
            (9081, 8081),
            (9081, 9081),
            (443, 80),
        ] {
            assert!(
                PanelPorts { http, https }.validate().is_err(),
                "{http}/{https}"
            );
        }
    }

    #[test]
    fn legacy_profiles_are_imported_and_status_omits_secrets() {
        let server: ServerProfile =
            serde_json::from_str(r#"{"name":"Old","endpoint":"vpn:8443","public_key":"key"}"#)
                .unwrap();
        assert!(server.management.is_none());
        assert!(server.emoji.is_empty());
        let json = serde_json::to_string(&ServerStatus::from(&server)).unwrap();
        assert!(json.contains("\"managed\":false"));
        assert!(!json.contains("management"));
        assert!(!json.contains("private"));
        assert!(json.contains("\"emoji\":\"\""));
    }

    #[test]
    fn legacy_routing_deserializes_with_rules_mode_and_no_order() {
        let routing: RoutingConfig =
            serde_json::from_str(r#"{"domain_rules":[],"ip_rules":[],"default_target":"vpn"}"#)
                .unwrap();
        assert_eq!(routing.mode, RoutingMode::Rules);
        assert_eq!(routing.rule_order, None);
    }

    #[test]
    fn old_server_updates_keep_the_stored_emoji() {
        let update: ServerUpdate = serde_json::from_str(
            r#"{"previous_public_key":"old","name":"New","endpoint":"vpn:8443","public_key":"new"}"#,
        )
        .unwrap();
        assert_eq!(update.emoji, None);
    }
}

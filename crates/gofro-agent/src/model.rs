use std::net::Ipv4Addr;

use ipnet::Ipv4Net;
use serde::{Deserialize, Serialize};
use wireguard_status::PeerStatus;

use crate::stats::{HistoryPoint, LiveStats};

pub(crate) const AP_DOMAIN: &str = "wifi.gofro.net";

#[derive(Clone)]
pub(crate) struct LanContext {
    pub(crate) device: String,
    pub(crate) address: Ipv4Addr,
    pub(crate) subnet: Ipv4Net,
}

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct ControllerConfig {
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

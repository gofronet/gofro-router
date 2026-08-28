use serde::{Deserialize, Serialize};
use wireguard_status::PeerStatus;

use crate::stats::{DeviceStatus, HistoryPoint, LiveStats};

pub(crate) const AP_ADDRESS: &str = "10.203.1.1";
pub(crate) const AP_DOMAIN: &str = "wifi.gofro.net";

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
    pub(crate) endpoint: String,
    pub(crate) public_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) client_tunnel_address: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) client_private_key: Option<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct ServerStatus {
    pub(crate) name: String,
    pub(crate) endpoint: String,
    pub(crate) public_key: String,
}

impl From<&ServerProfile> for ServerStatus {
    fn from(server: &ServerProfile) -> Self {
        Self {
            name: server.name.clone(),
            endpoint: server.endpoint.clone(),
            public_key: server.public_key.clone(),
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
}

#[derive(Deserialize)]
pub(crate) struct ProfileInput {
    pub(crate) name: String,
    pub(crate) profile: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) enum WifiBand {
    #[serde(rename = "2g")]
    TwoGhz,
    #[serde(rename = "5g")]
    FiveGhz,
}

impl WifiBand {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::TwoGhz => "2g",
            Self::FiveGhz => "5g",
        }
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct ApInput {
    pub(crate) band: Option<WifiBand>,
    pub(crate) ssid: String,
    pub(crate) password: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct RoutingConfig {
    pub(crate) domain_rules: Vec<DomainRule>,
    pub(crate) ip_rules: Vec<IpRule>,
    pub(crate) default_target: RouteTarget,
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
        }
    }
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
}

#[derive(Debug, Serialize)]
pub(crate) struct RoutingStatus {
    pub(crate) config: RoutingConfig,
    pub(crate) dns_active: bool,
    pub(crate) fake_ips: usize,
    pub(crate) geosite_loaded: bool,
    pub(crate) geoip_loaded: bool,
    pub(crate) dataplane_active: bool,
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
    pub(crate) ap: ApStatus,
    pub(crate) peer: Option<PeerStatus>,
    pub(crate) stats: LiveStats,
    pub(crate) history: Vec<HistoryPoint>,
    pub(crate) devices: Vec<DeviceStatus>,
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

#[derive(Debug, Serialize)]
pub(crate) struct ApStatus {
    // Browsers loaded before an in-place update still expect this field.
    pub(crate) ssid: String,
    pub(crate) networks: Vec<ApNetwork>,
    pub(crate) address: &'static str,
    pub(crate) domain: &'static str,
}

#[derive(Debug, Serialize)]
pub(crate) struct ApNetwork {
    pub(crate) band: WifiBand,
    pub(crate) ssid: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_legacy_ap_input_without_band() {
        let input: ApInput =
            serde_json::from_str(r#"{"ssid":"Legacy","password":"secret123"}"#).unwrap();
        assert_eq!(input.band, None);
    }
}

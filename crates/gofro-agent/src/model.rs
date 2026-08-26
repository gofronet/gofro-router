use serde::{Deserialize, Serialize};
use wireguard_status::PeerStatus;

use crate::stats::{DeviceStatus, HistoryPoint, LiveStats};

pub(crate) const AP_ADDRESS: &str = "10.203.1.1";
pub(crate) const AP_DOMAIN: &str = "gofrowifi.net:8080";

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct ControllerConfig {
    pub(crate) vpn_enabled: bool,
    pub(crate) active_server_key: Option<String>,
    pub(crate) servers: Vec<ServerProfile>,
    #[serde(default = "default_ap_ssid")]
    pub(crate) ap_ssid: String,
    #[serde(default)]
    pub(crate) routing: RoutingConfig,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct ServerProfile {
    pub(crate) name: String,
    pub(crate) endpoint: String,
    pub(crate) public_key: String,
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

#[derive(Debug, Deserialize)]
pub(crate) struct ApInput {
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
    pub(crate) servers: Vec<ServerProfile>,
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
    pub(crate) ssid: String,
    pub(crate) address: &'static str,
    pub(crate) domain: &'static str,
}

fn default_ap_ssid() -> String {
    "GofroNET WiFi".to_owned()
}

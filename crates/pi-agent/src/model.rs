use serde::{Deserialize, Serialize};
use tunnel_core::PeerStatus;

use crate::stats::{DeviceStatus, HistoryPoint, LiveStats};

pub(crate) const AP_ADDRESS: &str = "10.203.1.1";
pub(crate) const AP_DOMAIN: &str = "gofrowifi.net";

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct ControllerConfig {
    pub(crate) vpn_enabled: bool,
    pub(crate) active_server_key: Option<String>,
    pub(crate) servers: Vec<ServerProfile>,
    #[serde(default = "default_ap_ssid")]
    pub(crate) ap_ssid: String,
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

#[derive(Debug, Serialize)]
pub(crate) struct AgentStatus {
    pub(crate) version: &'static str,
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

use std::{
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, ensure};
use semver::Version;
use serde::Deserialize;

use crate::process;

const AGENT_SERVICE: &str = "pi-agent.service";
const RELAY_SERVICE: &str = "maxos-wg-relay-client.service";
const UPDATER_API_SERVICE: &str = "gofro-updater-api.service";
const MAX_HANDSHAKE_AGE: u64 = 120;

#[derive(Debug, Deserialize)]
struct ApiStatus {
    #[serde(default)]
    version: Option<String>,
    interface: String,
    vpn_enabled: bool,
    tunnel_active: bool,
    peer: Option<PeerStatus>,
}

#[derive(Debug, Deserialize)]
struct PeerStatus {
    latest_handshake: Option<u64>,
    handshake_age_seconds: Option<u64>,
}

#[derive(Clone, Debug)]
pub(crate) struct Baseline {
    pub(crate) interface: String,
    pub(crate) vpn_enabled: bool,
    pub(crate) latest_handshake: Option<u64>,
}

impl ApiStatus {
    fn vpn_healthy(&self) -> bool {
        self.vpn_enabled
            && self.tunnel_active
            && self
                .peer
                .as_ref()
                .and_then(|peer| peer.handshake_age_seconds)
                .is_some_and(|age| age <= MAX_HANDSHAKE_AGE)
    }

    fn validate_version(&self, version: &Version) -> Result<()> {
        match self.version.as_deref() {
            Some(reported) => ensure!(
                reported == version.to_string(),
                "status API reported version {reported}, expected {version}"
            ),
            None => ensure!(
                version < &Version::new(0, 2, 0),
                "status API did not report its version"
            ),
        }
        Ok(())
    }

    fn validate(&self, version: &Version, baseline: &Baseline) -> Result<()> {
        self.validate_version(version)?;
        ensure!(
            self.interface == baseline.interface,
            "WireGuard interface changed during update"
        );
        ensure!(
            self.vpn_enabled == baseline.vpn_enabled,
            "configured VPN mode changed during update"
        );
        ensure!(
            self.tunnel_active == baseline.vpn_enabled,
            "WireGuard tunnel state does not match configured VPN mode"
        );
        if baseline.latest_handshake.is_some() {
            ensure!(self.vpn_healthy(), "VPN did not complete a new handshake");
        }
        Ok(())
    }
}

fn parse_status(bytes: &[u8]) -> Result<ApiStatus> {
    serde_json::from_slice(bytes).context("failed to parse status API response")
}

pub(crate) fn baseline(version: &Version) -> Result<Baseline> {
    let status = parse_status(&process::status_api()?)?;
    status.validate_version(version)?;
    ensure!(
        !status.interface.is_empty()
            && status
                .interface
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"_.:-".contains(&byte)),
        "status API reported an invalid WireGuard interface"
    );
    let latest_handshake = if status.vpn_healthy() {
        status.peer.and_then(|peer| peer.latest_handshake)
    } else {
        None
    };
    Ok(Baseline {
        interface: status.interface,
        vpn_enabled: status.vpn_enabled,
        latest_handshake,
    })
}

fn check(version: &Version, baseline: &Baseline) -> Result<()> {
    ensure!(
        process::service_active(RELAY_SERVICE)?,
        "{RELAY_SERVICE} is not active"
    );
    ensure!(
        process::service_active(AGENT_SERVICE)?,
        "{AGENT_SERVICE} is not active"
    );
    parse_status(&process::status_api()?)?.validate(version, baseline)
}

pub(crate) fn wait(version: &Version, baseline: Baseline) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        match check(version, &baseline) {
            Ok(()) => return Ok(()),
            Err(error) if Instant::now() < deadline => {
                let _ = error;
                thread::sleep(Duration::from_secs(2));
            }
            Err(error) => {
                return Err(anyhow!(error)).context("health check timed out after 60 seconds");
            }
        }
    }
}

pub(crate) fn stop_agent() -> Result<()> {
    process::systemctl("stop", AGENT_SERVICE)
}

pub(crate) fn stop_relay() -> Result<()> {
    process::systemctl("stop", RELAY_SERVICE)
}

pub(crate) fn stop_tunnel(interface: &str) -> Result<()> {
    process::systemctl("stop", &format!("wg-quick@{interface}.service"))
}

pub(crate) fn start_agent() -> Result<()> {
    process::systemctl("start", AGENT_SERVICE)
}

pub(crate) fn start_relay() -> Result<()> {
    process::systemctl("start", RELAY_SERVICE)
}

pub(crate) fn restart_updater_api() -> Result<()> {
    process::systemctl("restart", UPDATER_API_SERVICE)
}

pub(crate) fn wait_updater_api() -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match process::updater_api().and_then(|body| {
            let status: serde_json::Value = serde_json::from_slice(&body)?;
            ensure!(
                status["schema"] == 1,
                "updater API returned an invalid schema"
            );
            Ok(())
        }) {
            Ok(()) => return Ok(()),
            Err(error) if Instant::now() < deadline => {
                let _ = error;
                thread::sleep(Duration::from_millis(250));
            }
            Err(error) => return Err(error).context("updater API health check timed out"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(vpn: bool, tunnel: bool, age: &str) -> Vec<u8> {
        format!(
            r#"{{"version":"1.2.3","interface":"gt0","vpn_enabled":{vpn},"tunnel_active":{tunnel},"peer":{age}}}"#
        )
        .into_bytes()
    }

    #[test]
    fn parses_vpn_baseline() {
        let healthy = parse_status(&response(
            true,
            true,
            r#"{"latest_handshake":100,"handshake_age_seconds":120}"#,
        ))
        .unwrap();
        assert!(healthy.vpn_healthy());
        assert!(
            !parse_status(&response(
                true,
                true,
                r#"{"latest_handshake":100,"handshake_age_seconds":121}"#,
            ))
            .unwrap()
            .vpn_healthy()
        );
        assert!(
            !parse_status(&response(false, false, "null"))
                .unwrap()
                .vpn_healthy()
        );
        let baseline = Baseline {
            interface: "gt0".into(),
            vpn_enabled: true,
            latest_handshake: Some(100),
        };
        assert!(healthy.validate(&Version::new(1, 2, 3), &baseline).is_ok());
        assert!(healthy.validate(&Version::new(1, 2, 4), &baseline).is_err());
        let direct = Baseline {
            interface: "gt0".into(),
            vpn_enabled: false,
            latest_handshake: None,
        };
        assert!(
            parse_status(&response(false, false, "null"))
                .unwrap()
                .validate(&Version::new(1, 2, 3), &direct)
                .is_ok()
        );
        assert!(
            parse_status(&response(false, true, "null"))
                .unwrap()
                .validate(&Version::new(1, 2, 3), &direct)
                .is_err()
        );
        let legacy =
            br#"{"interface":"gt0","vpn_enabled":false,"tunnel_active":false,"peer":null}"#;
        assert!(
            parse_status(legacy)
                .unwrap()
                .validate(
                    &Version::new(0, 1, 0),
                    &Baseline {
                        interface: "gt0".into(),
                        vpn_enabled: false,
                        latest_handshake: None,
                    },
                )
                .is_ok()
        );
    }
}

use std::{
    fs,
    io::{Read, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};

use crate::{
    AppState,
    config::validate_ssid,
    model::{ApNetwork, WifiBand},
    network,
};

const STATE_LIMIT: u64 = 32;
const PENDING_LIMIT: u64 = 8192;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Step {
    Admin,
    Wifi,
    WifiApplying,
    Server,
    Complete,
}

#[derive(Deserialize, Serialize)]
pub(crate) struct WifiInput {
    pub(crate) networks: Vec<WifiNetwork>,
}
#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct WifiNetwork {
    pub(crate) band: WifiBand,
    pub(crate) ssid: String,
    pub(crate) password: String,
}
#[derive(Serialize)]
pub(crate) struct Status {
    pub(crate) step: Step,
    pub(crate) networks: Vec<ApNetwork>,
    pub(crate) setup_window_seconds: Option<u64>,
    pub(crate) error: Option<String>,
}

fn dir(state: &AppState) -> Result<&Path> {
    state
        .config_path
        .parent()
        .context("missing state directory")
}
fn file(state: &AppState, name: &str) -> Result<PathBuf> {
    Ok(dir(state)?.join(name))
}
fn read_limited(path: &Path, limit: u64) -> Result<Option<Vec<u8>>> {
    match fs::File::open(path) {
        Ok(input) => {
            let metadata = input.metadata()?;
            if !metadata.is_file() || metadata.permissions().mode() & 0o077 != 0 {
                bail!("unsafe onboarding state file");
            }
            let mut value = Vec::new();
            input.take(limit + 1).read_to_end(&mut value)?;
            if value.len() as u64 > limit {
                bail!("onboarding state is too large");
            }
            Ok(Some(value))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}
fn marker(state: &AppState) -> Result<Option<Step>> {
    let Some(value) = read_limited(&file(state, "onboarding-state")?, STATE_LIMIT)? else {
        return Ok(None);
    };
    match value.as_slice() {
        b"admin\n" => Ok(Some(Step::Admin)),
        b"wifi\n" => Ok(Some(Step::Wifi)),
        b"wifi_applying\n" => Ok(Some(Step::WifiApplying)),
        b"server\n" => Ok(Some(Step::Server)),
        _ => bail!("invalid onboarding state"),
    }
}
pub(crate) fn step(state: &AppState) -> Result<Step> {
    let admin = state.auth.configured();
    match marker(state)? {
        None => Ok(Step::Complete),
        Some(Step::Admin) if admin => Ok(Step::Wifi),
        Some(Step::Admin) => Ok(Step::Admin),
        Some(step) if admin => Ok(step),
        Some(_) => bail!("onboarding marker has no admin credential"),
    }
}
pub(crate) fn fresh_admin(state: &AppState) -> Result<bool> {
    Ok(step(state)? == Step::Admin)
}
pub(crate) fn write_wifi(state: &AppState) -> Result<()> {
    atomic(&file(state, "onboarding-state")?, b"wifi\n")
}
fn atomic(path: &Path, value: &[u8]) -> Result<()> {
    let temporary = path.with_extension("new");
    let mut output = fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(&temporary)?;
    output.set_permissions(fs::Permissions::from_mode(0o600))?;
    output.write_all(value)?;
    output.sync_all()?;
    fs::rename(&temporary, path)?;
    fs::File::open(path.parent().context("missing state parent")?)?.sync_all()?;
    Ok(())
}

pub(crate) fn setup_window_seconds(state: &AppState) -> u64 {
    let window = file(state, "onboarding-window")
        .ok()
        .and_then(|path| read_limited(&path, 128).ok().flatten());
    let boot = fs::read_to_string("/proc/sys/kernel/random/boot_id").ok();
    let uptime = fs::read_to_string("/proc/uptime").ok();
    match (window, boot, uptime) {
        (Some(window), Some(boot), Some(uptime)) => window_seconds(&window, boot.trim(), &uptime),
        _ => 0,
    }
}
pub(crate) fn window_seconds(window: &[u8], boot: &str, uptime: &str) -> u64 {
    let Ok(value) = std::str::from_utf8(window) else {
        return 0;
    };
    let Some((stored_boot, deadline)) = value.trim_end_matches('\n').split_once(' ') else {
        return 0;
    };
    if stored_boot != boot
        || stored_boot.len() != 36
        || !stored_boot.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
        || deadline.contains(' ')
    {
        return 0;
    }
    let Ok(deadline) = deadline.parse::<u64>() else {
        return 0;
    };
    let Some(seconds) = uptime
        .split('.')
        .next()
        .and_then(|value| value.parse::<u64>().ok())
    else {
        return 0;
    };
    deadline
        .checked_sub(seconds)
        .filter(|left| *left <= 900)
        .unwrap_or(0)
}
fn visible(networks: Vec<ApNetwork>) -> Vec<ApNetwork> {
    let mut result = Vec::new();
    for network in networks {
        if !result
            .iter()
            .any(|item: &ApNetwork| item.band == network.band)
        {
            result.push(network);
        }
    }
    result
}
pub(crate) fn status(state: &AppState) -> Result<Status> {
    let step = step(state)?;
    let networks = match step {
        Step::Wifi | Step::Admin => visible(
            state
                .access_points
                .lock()
                .map_err(|_| anyhow!("access point lock poisoned"))?
                .clone(),
        ),
        Step::WifiApplying => pending(state)?,
        Step::Server => {
            let networks = network::access_points()?;
            *state
                .access_points
                .lock()
                .map_err(|_| anyhow!("access point lock poisoned"))? = networks.clone();
            visible(networks)
        }
        Step::Complete => vec![],
    };
    let error = read_limited(&file(state, "onboarding-error")?, 256)?
        .map(|_| "onboarding_failed".to_owned());
    Ok(Status {
        step,
        networks,
        setup_window_seconds: matches!(step, Step::Admin | Step::Wifi)
            .then(|| setup_window_seconds(state)),
        error,
    })
}
fn pending(state: &AppState) -> Result<Vec<ApNetwork>> {
    let value = read_limited(&file(state, "onboarding-wifi.json")?, PENDING_LIMIT)?
        .context("missing pending onboarding Wi-Fi")?;
    let input: WifiInput =
        serde_json::from_slice(&value).map_err(|_| anyhow!("invalid pending onboarding Wi-Fi"))?;
    Ok(input
        .networks
        .into_iter()
        .map(|network| ApNetwork {
            band: network.band,
            ssid: network.ssid,
        })
        .collect())
}
pub(crate) fn submit_wifi(state: &AppState, input: WifiInput) -> Result<Status> {
    let _guard = state
        .config
        .lock()
        .map_err(|_| anyhow!("configuration lock poisoned"))?;
    if step(state)? != Step::Wifi {
        bail!("onboarding is unavailable");
    }
    if setup_window_seconds(state) == 0 {
        bail!("setup_closed");
    }
    let networks = visible(
        state
            .access_points
            .lock()
            .map_err(|_| anyhow!("access point lock poisoned"))?
            .clone(),
    );
    validate_networks(&input.networks, &networks)?;
    let payload = serde_json::to_vec(&input)?;
    if payload.len() > PENDING_LIMIT as usize {
        bail!("onboarding Wi-Fi is too large");
    }
    helper(state, "wifi", Some(&payload))?;
    Ok(Status {
        step: Step::WifiApplying,
        networks: input
            .networks
            .into_iter()
            .map(|network| ApNetwork {
                band: network.band,
                ssid: network.ssid,
            })
            .collect(),
        setup_window_seconds: None,
        error: None,
    })
}
fn validate_networks(input: &[WifiNetwork], available: &[ApNetwork]) -> Result<()> {
    if input.len() != available.len() || input.is_empty() {
        bail!("all access point bands are required");
    }
    for network in input {
        validate_ssid(&network.ssid)?;
        if !(8..=63).contains(&network.password.len())
            || !network
                .password
                .bytes()
                .all(|byte| (b' '..=b'~').contains(&byte))
        {
            bail!("invalid Wi-Fi password");
        }
        if !available.iter().any(|item| item.band == network.band)
            || input
                .iter()
                .filter(|item| item.band == network.band)
                .count()
                != 1
        {
            bail!("invalid Wi-Fi bands");
        }
    }
    Ok(())
}
pub(crate) fn complete(state: &AppState) -> Result<Status> {
    let _guard = state
        .config
        .lock()
        .map_err(|_| anyhow!("configuration lock poisoned"))?;
    match step(state)? {
        Step::Server => helper(state, "complete", None)?,
        Step::Complete => return status(state),
        _ => bail!("onboarding is not ready"),
    }
    Ok(Status {
        step: Step::Complete,
        networks: vec![],
        setup_window_seconds: None,
        error: None,
    })
}
fn helper(state: &AppState, action: &str, input: Option<&[u8]>) -> Result<()> {
    let mut command = Command::new(state.mode_command.with_file_name("onboarding"));
    command
        .arg(action)
        .env("GOFRO_STATE_DIR", dir(state)?)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if input.is_some() {
        command.stdin(Stdio::piped());
    }
    let mut child = command
        .spawn()
        .context("failed to start onboarding helper")?;
    if let Some(input) = input {
        child
            .stdin
            .take()
            .context("failed to open onboarding input")?
            .write_all(input)?;
    }
    if !child.wait()?.success() {
        bail!("onboarding helper rejected request");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn window_never_reopens_for_wrong_boot_or_expiry() {
        let boot = "01234567-89ab-cdef-0123-456789abcdef";
        assert_eq!(
            window_seconds(
                b"01234567-89ab-cdef-0123-456789abcdef 100\n",
                "other",
                "10.0"
            ),
            0
        );
        assert_eq!(
            window_seconds(b"01234567-89ab-cdef-0123-456789abcdef 10\n", boot, "10.0"),
            0
        );
        assert_eq!(
            window_seconds(b"01234567-89ab-cdef-0123-456789abcdef 20\n", boot, "10.0"),
            10
        );
    }
    #[test]
    fn requires_every_band_once_with_safe_passwords() {
        let available = vec![
            ApNetwork {
                band: WifiBand::TwoGhz,
                ssid: "Two".into(),
            },
            ApNetwork {
                band: WifiBand::FiveGhz,
                ssid: "Five".into(),
            },
        ];
        let network = |band| WifiNetwork {
            band,
            ssid: "Valid SSID".into(),
            password: "password".into(),
        };
        assert!(
            validate_networks(
                &[network(WifiBand::TwoGhz), network(WifiBand::FiveGhz)],
                &available
            )
            .is_ok()
        );
        assert!(
            validate_networks(
                &[network(WifiBand::TwoGhz), network(WifiBand::TwoGhz)],
                &available
            )
            .is_err()
        );
        assert!(validate_networks(&[network(WifiBand::TwoGhz)], &available).is_err());
    }
    #[test]
    fn preserves_the_only_setup_radio_for_configuration() {
        assert_eq!(
            visible(vec![ApNetwork {
                band: WifiBand::FiveGhz,
                ssid: "GofroNET Wi-Fi Setup".into()
            }])[0]
                .ssid,
            "GofroNET Wi-Fi Setup"
        );
    }
}

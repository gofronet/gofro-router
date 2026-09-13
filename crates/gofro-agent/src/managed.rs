use std::{
    collections::HashSet,
    net::IpAddr,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

use anyhow::{Context, Result, anyhow, bail};
use wireguard_status::managed::{
    FriendNameInput, ManagedServerStatus, validate_name, validate_peer_key,
};

use crate::{
    AppState,
    model::{ControllerConfig, ManagedServer},
};

mod bootstrap;
mod pins;
mod ssh;

pub(crate) use bootstrap::bootstrap;
pub(crate) use pins::{parse_host_key, preserve_host_pins};
use ssh::key_ssh;
pub(crate) use ssh::probe;

static TEMPORARY: AtomicUsize = AtomicUsize::new(0);

const UPGRADE_REQUIRED: &str = "VPS upgrade required: the installed/published Gofro release does not support owner protocol 2. Install a compatible signed release before retrying; existing peers were not replaced.";

pub(crate) struct Probe {
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) host_key: String,
    pub(crate) fingerprint: String,
}

pub(crate) struct Version {
    pub(crate) version: String,
    pub(crate) update_available: bool,
}

#[derive(Debug)]
pub(crate) struct CommittedRefreshFailed;

impl std::fmt::Display for CommittedRefreshFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Operation committed, but status could not be refreshed. Refresh status before any further changes; do not repeat the write.")
    }
}

impl std::error::Error for CommittedRefreshFailed {}

pub(crate) fn check(state: &AppState, public_key: &str) -> Result<Version> {
    let _operation = managed_lock(state)?;
    check_unlocked(state, public_key)
}
fn check_unlocked(state: &AppState, public_key: &str) -> Result<Version> {
    let output = managed_ssh_unlocked(state, public_key, "version")?;
    let version = parse_version(&output)?;
    Ok(Version {
        update_available: semver(&version)? < semver(env!("CARGO_PKG_VERSION"))?,
        version,
    })
}

pub(crate) fn update(state: &AppState, public_key: &str) -> Result<Version> {
    let _operation = managed_lock(state)?;
    managed_ssh_unlocked(state, public_key, "update")?;
    check_unlocked(state, public_key).context(CommittedRefreshFailed)
}

pub(crate) fn create_profile(state: &AppState, public_key: &str) -> Result<String> {
    let _operation = managed_lock(state)?;
    let (managed, private_key) = managed_server(state, public_key)?;
    key_ssh(
        &managed.host,
        managed.port,
        &private_key,
        &state.management_dir,
        &managed.host_key,
        &format!("create-profile {}", endpoint(&managed.host)),
        None,
    )
}

pub(crate) fn status(state: &AppState, public_key: &str) -> Result<ManagedServerStatus> {
    let _operation = managed_lock(state)?;
    status_unlocked(state, public_key)
}

pub(crate) fn restart(state: &AppState, public_key: &str) -> Result<ManagedServerStatus> {
    let _operation = managed_lock(state)?;
    managed_ssh_unlocked(state, public_key, "restart-vpn")?;
    status_unlocked(state, public_key).context(CommittedRefreshFailed)
}

pub(crate) fn create_friend(
    state: &AppState,
    public_key: &str,
    name: &str,
) -> Result<ManagedServerStatus> {
    let _operation = managed_lock(state)?;
    let name = validate_name(name)?;
    let (managed, private_key) = managed_server(state, public_key)?;
    let input = serde_json::to_string(&FriendNameInput { name })?;
    // The raw profile contains a private key and must not enter status or configuration.
    key_ssh(
        &managed.host,
        managed.port,
        &private_key,
        &state.management_dir,
        &managed.host_key,
        &format!("create-friend {}", endpoint(&managed.host)),
        Some(&input),
    )?;
    status_unlocked(state, public_key).context(CommittedRefreshFailed)
}

pub(crate) fn rename_friend(
    state: &AppState,
    public_key: &str,
    peer_key: &str,
    name: &str,
) -> Result<ManagedServerStatus> {
    let _operation = managed_lock(state)?;
    validate_peer_key(peer_key)?;
    let name = validate_name(name)?;
    let (managed, private_key) = managed_server(state, public_key)?;
    let input = serde_json::to_string(&FriendNameInput { name })?;
    key_ssh(
        &managed.host,
        managed.port,
        &private_key,
        &state.management_dir,
        &managed.host_key,
        &format!("rename-friend {peer_key}"),
        Some(&input),
    )?;
    status_unlocked(state, public_key).context(CommittedRefreshFailed)
}

pub(crate) fn revoke_friend(
    state: &AppState,
    public_key: &str,
    peer_key: &str,
) -> Result<ManagedServerStatus> {
    let _operation = managed_lock(state)?;
    validate_peer_key(peer_key)?;
    managed_ssh_unlocked(state, public_key, &format!("revoke-friend {peer_key}"))?;
    status_unlocked(state, public_key).context(CommittedRefreshFailed)
}

pub(crate) fn friend_profile(state: &AppState, public_key: &str, peer_key: &str) -> Result<String> {
    let _operation = managed_lock(state)?;
    validate_peer_key(peer_key)?;
    let (managed, private_key) = managed_server(state, public_key)?;
    key_ssh(
        &managed.host,
        managed.port,
        &private_key,
        &state.management_dir,
        &managed.host_key,
        &format!("friend-profile {peer_key} {}", endpoint(&managed.host)),
        None,
    )
}

fn managed_lock(state: &AppState) -> Result<std::sync::MutexGuard<'_, ()>> {
    state
        .managed_operations
        .lock()
        .map_err(|_| anyhow!("managed operation lock poisoned"))
}

fn managed_ssh_unlocked(state: &AppState, public_key: &str, remote: &str) -> Result<String> {
    let (managed, private_key) = managed_server(state, public_key)?;
    key_ssh(
        &managed.host,
        managed.port,
        &private_key,
        &state.management_dir,
        &managed.host_key,
        remote,
        None,
    )
}

fn status_unlocked(state: &AppState, public_key: &str) -> Result<ManagedServerStatus> {
    let output = managed_ssh_unlocked(state, public_key, "managed-status")?;
    let status: ManagedServerStatus =
        serde_json::from_str(&output).context("invalid managed server status")?;
    validate_managed_status(&status)?;
    Ok(status)
}

fn validate_managed_status(status: &ManagedServerStatus) -> Result<()> {
    semver(&status.version)?;
    let mut peers = HashSet::new();
    for peer in &status.peers {
        validate_peer_key(&peer.public_key)?;
        if !peers.insert(&peer.public_key) {
            bail!("managed server returned duplicate peer key");
        }
        if peer.can_share && peer.revoked {
            bail!("managed server returned an invalid friend state");
        }
        if validate_name(&peer.name)? != peer.name {
            bail!("managed server returned an unnormalized friend name");
        }
    }
    Ok(())
}

fn managed_server(state: &AppState, public_key: &str) -> Result<(ManagedServer, PathBuf)> {
    let config = state
        .config
        .lock()
        .map_err(|_| anyhow!("configuration lock poisoned"))?;
    let management = managed_server_config(&config, public_key)?;
    preserve_host_pins(&state.management_dir, &config)?;
    drop(config);
    let private_key = state.management_dir.join("id_ed25519");
    if !private_key.is_file() {
        bail!("management SSH key is missing");
    }
    Ok((management, private_key))
}

fn managed_server_config(config: &ControllerConfig, public_key: &str) -> Result<ManagedServer> {
    validate_peer_key(public_key)?;
    config
        .servers
        .iter()
        .find(|server| server.public_key == public_key)
        .context("сервер не найден")?
        .management
        .clone()
        .context("сервер не управляется Gofro")
}

pub(crate) fn parse_version(value: &str) -> Result<String> {
    let value = value.trim();
    let Some(version) = value.strip_prefix("gofro-server ") else {
        bail!("unexpected server version output");
    };
    if version.is_empty()
        || !version
            .split('.')
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
    {
        bail!("unexpected server version output");
    }
    Ok(version.to_owned())
}

fn temporary_path(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!(
        ".{name}.{}.{}",
        std::process::id(),
        TEMPORARY.fetch_add(1, Ordering::Relaxed)
    ))
}

fn endpoint(host: &str) -> String {
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V6(_)) => format!("[{host}]:8443"),
        _ => format!("{host}:8443"),
    }
}

fn semver(value: &str) -> Result<(u64, u64, u64)> {
    let mut parts = value.split('.');
    let version = (
        parts.next().context("invalid version")?.parse()?,
        parts.next().context("invalid version")?.parse()?,
        parts.next().context("invalid version")?.parse()?,
    );
    if parts.next().is_some() {
        bail!("invalid version");
    }
    Ok(version)
}

#[cfg(test)]
pub(crate) mod tests;

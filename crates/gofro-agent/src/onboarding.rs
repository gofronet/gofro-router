use std::{
    fs,
    io::{Read, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde::Serialize;

use crate::AppState;

const STATE_LIMIT: u64 = 32;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Step {
    Admin,
    Server,
    Complete,
}

#[derive(Serialize)]
pub(crate) struct Status {
    pub(crate) step: Step,
    // Retained as an empty array for clients released during Wi-Fi onboarding.
    pub(crate) networks: Vec<()>,
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
        Ok(mut input) => {
            let metadata = input.metadata()?;
            if !metadata.is_file() || metadata.permissions().mode() & 0o077 != 0 {
                bail!("unsafe onboarding state file");
            }
            let mut value = Vec::new();
            Read::by_ref(&mut input)
                .take(limit + 1)
                .read_to_end(&mut value)?;
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
        b"server\n" => Ok(Some(Step::Server)),
        b"wifi\n" | b"wifi_applying\n" => bail!("legacy Wi-Fi onboarding requires migration"),
        _ => bail!("invalid onboarding state"),
    }
}
pub(crate) fn step(state: &AppState) -> Result<Step> {
    match marker(state)? {
        None => Ok(Step::Complete),
        Some(Step::Admin) if !state.auth.configured() => Ok(Step::Admin),
        Some(Step::Server) if state.auth.configured() => Ok(Step::Server),
        Some(_) => bail!("onboarding marker does not match credentials"),
    }
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
    let Some((stored_boot, deadline)) = value.trim_end().split_once(' ') else {
        return 0;
    };
    let Some(now) = uptime
        .split('.')
        .next()
        .and_then(|value| value.parse::<u64>().ok())
    else {
        return 0;
    };
    (stored_boot == boot)
        .then(|| deadline.parse::<u64>().ok())
        .flatten()
        .and_then(|deadline| deadline.checked_sub(now))
        .filter(|left| *left <= 900)
        .unwrap_or(0)
}
pub(crate) fn status(state: &AppState) -> Result<Status> {
    let step = step(state)?;
    Ok(Status {
        step,
        networks: vec![],
        setup_window_seconds: (step == Step::Admin).then(|| setup_window_seconds(state)),
        error: read_limited(&file(state, "onboarding-error")?, 256)?
            .map(|_| "onboarding_failed".to_owned()),
    })
}
pub(crate) fn complete_admin(state: &AppState, setup_code: &Path) -> Result<()> {
    if !state.auth.configured() {
        bail!("admin credentials are required");
    }
    match marker(state)? {
        Some(Step::Admin) => atomic(&file(state, "onboarding-state")?, b"server\n")?,
        // Retry the directory sync if the previous rename succeeded but its sync failed.
        Some(Step::Server) => fs::File::open(dir(state)?)?.sync_all()?,
        None => return Ok(()),
        Some(Step::Complete) => bail!("invalid onboarding state"),
    }
    match fs::remove_file(setup_code) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    fs::File::open(setup_code.parent().context("missing setup code parent")?)?.sync_all()?;
    Ok(())
}
pub(crate) fn complete(state: &AppState) -> Result<Status> {
    if step(state)? != Step::Server {
        bail!("onboarding is not ready");
    }
    fs::remove_file(file(state, "onboarding-state")?)?;
    fs::File::open(dir(state)?)?.sync_all()?;
    status(state)
}
fn atomic(path: &Path, value: &[u8]) -> Result<()> {
    let temporary = path.with_extension("new");
    let mut output = fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(&temporary)?;
    output.write_all(value)?;
    output.sync_all()?;
    fs::rename(temporary, path)?;
    fs::File::open(path.parent().context("missing state parent")?)?.sync_all()?;
    Ok(())
}

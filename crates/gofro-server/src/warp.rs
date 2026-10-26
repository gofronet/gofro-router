use std::{fs, os::unix::fs::MetadataExt, process::Command};

use anyhow::{Context, Result, bail};
use wireguard_status::warp::WarpStatus;

fn directory() -> std::path::PathBuf {
    super::test_path("GOFRO_TEST_WARP_DIR", "/etc/gofro/warp")
}

pub(crate) fn prepared(interface: &str) -> Result<bool> {
    let path = directory().join("network");
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.uid() != crate::managed::expected_uid()
        || metadata.mode() & 0o777 != 0o600
    {
        bail!("unsafe WARP network configuration");
    }
    let content = fs::read_to_string(path)?;
    if !content
        .lines()
        .any(|line| line == format!("WG_INTERFACE={interface}"))
    {
        bail!("WARP belongs to a different WireGuard interface");
    }
    Ok(true)
}

pub(crate) fn command(action: &str, interface: &str) -> Result<()> {
    let helper = super::test_path("GOFRO_TEST_WARP_HELPER", "/usr/local/sbin/gofro-warp");
    super::run(Command::new(helper).args([action, interface]))?;
    Ok(())
}

pub(crate) fn status(interface: &str) -> Result<WarpStatus> {
    if !prepared(interface)? {
        return Ok(WarpStatus::default());
    }
    let helper = super::test_path("GOFRO_TEST_WARP_HELPER", "/usr/local/sbin/gofro-warp");
    let output = super::run(Command::new(helper).args(["status", interface]))?;
    let status: WarpStatus = serde_json::from_str(&output).context("invalid WARP status")?;
    if status.connected && (!status.installed || !status.enabled) {
        bail!("inconsistent WARP status");
    }
    Ok(status)
}

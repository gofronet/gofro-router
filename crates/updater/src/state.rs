use std::{
    fs,
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use semver::Version;
use serde::{Deserialize, Serialize};

use crate::{fsops, paths, status::Baseline};

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Phase {
    Activating,
    RollingBack,
    Committing,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Pending {
    pub(crate) old_version: String,
    pub(crate) new_version: String,
    pub(crate) previous_target: PathBuf,
    pub(crate) release_path: PathBuf,
    pub(crate) migration_applied: bool,
    pub(crate) phase: Phase,
    pub(crate) interface: String,
    pub(crate) vpn_enabled: bool,
    pub(crate) latest_handshake: Option<u64>,
}

impl Pending {
    pub(crate) fn new(
        old: &Version,
        new: &Version,
        previous_target: PathBuf,
        release_path: PathBuf,
        baseline: Baseline,
    ) -> Result<Self> {
        let pending = Self {
            old_version: old.to_string(),
            new_version: new.to_string(),
            previous_target,
            release_path,
            migration_applied: false,
            phase: Phase::Activating,
            interface: baseline.interface,
            vpn_enabled: baseline.vpn_enabled,
            latest_handshake: baseline.latest_handshake,
        };
        pending.validate()?;
        Ok(pending)
    }

    pub(crate) fn versions(&self) -> Result<(Version, Version)> {
        Ok((
            Version::parse(&self.old_version).context("invalid pending old_version")?,
            Version::parse(&self.new_version).context("invalid pending new_version")?,
        ))
    }

    pub(crate) fn validate(&self) -> Result<()> {
        let (old, new) = self.versions()?;
        ensure!(
            old.pre.is_empty() && old.build.is_empty(),
            "pending old_version is not stable"
        );
        ensure!(
            new.pre.is_empty() && new.build.is_empty(),
            "pending new_version is not stable"
        );
        ensure!(
            self.release_path == paths::release(&new),
            "pending release_path does not match new_version"
        );
        ensure!(
            resolve_release_target(&self.previous_target, Path::new(paths::CURRENT))?
                == paths::release(&old),
            "pending previous_target does not match old_version"
        );
        Ok(())
    }

    pub(crate) fn baseline(&self) -> Baseline {
        Baseline {
            interface: self.interface.clone(),
            vpn_enabled: self.vpn_enabled,
            latest_handshake: self.latest_handshake,
        }
    }
}

pub(crate) fn read_version() -> Result<Version> {
    let value = fs::read_to_string(paths::VERSION)
        .with_context(|| format!("failed to read {}", paths::VERSION))?;
    Version::parse(value.trim()).context("installed version marker is not valid semver")
}

pub(crate) fn write_version(version: &Version) -> Result<()> {
    fs::create_dir_all(paths::UPDATE_DIR)
        .with_context(|| format!("failed to create {}", paths::UPDATE_DIR))?;
    fsops::atomic_write(
        Path::new(paths::VERSION),
        format!("{version}\n").as_bytes(),
        0o644,
    )
}

pub(crate) fn load_pending() -> Result<Option<Pending>> {
    let bytes = match fs::read(paths::PENDING) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("failed to read pending update state"),
    };
    let pending: Pending =
        serde_json::from_slice(&bytes).context("failed to parse pending update state")?;
    pending.validate()?;
    Ok(Some(pending))
}

pub(crate) fn write_pending(pending: &Pending) -> Result<()> {
    pending.validate()?;
    fs::create_dir_all(paths::UPDATE_DIR)
        .with_context(|| format!("failed to create {}", paths::UPDATE_DIR))?;
    let mut bytes = serde_json::to_vec(pending).context("failed to serialize pending state")?;
    bytes.push(b'\n');
    fsops::atomic_write(Path::new(paths::PENDING), &bytes, 0o600)
}

pub(crate) fn remove_pending() -> Result<()> {
    fsops::remove_file(Path::new(paths::PENDING))
}

pub(crate) fn current_target() -> Result<(PathBuf, PathBuf)> {
    let raw = fs::read_link(paths::CURRENT)
        .with_context(|| format!("failed to read symlink {}", paths::CURRENT))?;
    let resolved = resolve_release_target(&raw, Path::new(paths::CURRENT))?;
    Ok((raw, resolved))
}

pub(crate) fn current_points_to(release: &Path) -> Result<bool> {
    let raw = match fs::read_link(paths::CURRENT) {
        Ok(raw) => raw,
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::InvalidInput
            ) =>
        {
            return Ok(false);
        }
        Err(error) => {
            return Err(error).with_context(|| format!("failed to read {}", paths::CURRENT));
        }
    };
    Ok(resolve_release_target(&raw, Path::new(paths::CURRENT))
        .is_ok_and(|target| target == release))
}

pub(crate) fn switch_current(target: &Path) -> Result<()> {
    fsops::atomic_symlink(target, Path::new(paths::CURRENT))
}

fn resolve_release_target(target: &Path, link: &Path) -> Result<PathBuf> {
    ensure!(
        !target
            .components()
            .any(|component| matches!(component, Component::ParentDir)),
        "release symlink target contains parent traversal"
    );
    let resolved = if target.is_absolute() {
        target.to_path_buf()
    } else {
        link.parent()
            .context("current symlink has no parent")?
            .join(target)
    };
    ensure!(
        resolved.parent() == Some(Path::new(paths::RELEASES)),
        "release symlink target is outside {}",
        paths::RELEASES
    );
    ensure!(resolved.file_name().is_some(), "release target has no name");
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_pending_paths_without_touching_disk() {
        let pending = Pending::new(
            &Version::new(1, 0, 0),
            &Version::new(1, 1, 0),
            PathBuf::from("releases/1.0.0"),
            PathBuf::from(paths::RELEASES).join("1.1.0"),
            Baseline {
                interface: "gt0".into(),
                vpn_enabled: true,
                latest_handshake: Some(42),
            },
        )
        .unwrap();
        assert_eq!(pending.versions().unwrap().1, Version::new(1, 1, 0));

        let mut invalid = pending;
        invalid.previous_target = PathBuf::from("../outside");
        assert!(invalid.validate().is_err());
    }
}

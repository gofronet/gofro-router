use anyhow::{Result, anyhow, ensure};
use semver::Version;

use crate::{
    bundle, paths, process,
    progress::{self, UpdateProgress, UpdateState},
    state::{self, Pending, Phase},
    status,
};

pub(crate) fn reconcile_updater() -> Result<()> {
    let version = state::read_version()?;
    // The one-time bootstrap pairs the old 0.1.0 apps with the first updater.
    if version == Version::new(0, 1, 0) {
        return Ok(());
    }
    let (_, current) = state::current_target()?;
    ensure!(
        current == paths::release(&version),
        "current release does not match installed version marker"
    );
    bundle::install_updater(&current, &version.to_string())
}

pub(crate) fn apply(mut pending: Pending) -> Result<()> {
    state::write_pending(&pending)?;
    progress::write(&UpdateProgress::new(
        UpdateState::Installing,
        Some(&pending.versions()?.1),
    ))?;
    if let Err(original) = activate(&mut pending) {
        return match rollback(&mut pending, true) {
            Ok(()) => Err(original),
            Err(rollback) => Err(anyhow!(
                "update failed: {original:#}; rollback failed: {rollback:#}"
            )),
        };
    }
    commit(&mut pending)?;
    println!("Updated to {}", pending.new_version);
    Ok(())
}

fn activate(pending: &mut Pending) -> Result<()> {
    let (_, new) = pending.versions()?;
    stop_services(&pending.baseline())?;
    pending.migration_applied = true;
    state::write_pending(pending)?;
    process::migration(
        &pending.release_path,
        "up",
        &pending.old_version,
        &pending.new_version,
    )?;
    state::switch_current(&pending.release_path)?;
    status::start_relay()?;
    status::start_agent()?;
    status::wait(&new, pending.baseline())?;
    restart_updater_api(&new)
}

fn commit(pending: &mut Pending) -> Result<()> {
    pending.phase = Phase::Committing;
    state::write_pending(pending)?;
    process::migration(
        &pending.release_path,
        "commit",
        &pending.old_version,
        &pending.new_version,
    )?;
    finish(pending)
}

fn finish(pending: &Pending) -> Result<()> {
    let (_, new) = pending.versions()?;
    state::write_version(&new)?;
    bundle::install_updater(&pending.release_path, &pending.new_version)?;
    state::remove_pending()?;
    progress::write(&UpdateProgress::new(UpdateState::Success, Some(&new)))
}

pub(crate) fn recover_pending(boot: bool) -> Result<bool> {
    let Some(mut pending) = state::load_pending()? else {
        return Ok(false);
    };
    match pending.phase {
        Phase::Committing => {
            ensure!(
                state::current_points_to(&pending.release_path)?,
                "committing release is not current"
            );
            commit(&mut pending)?;
            println!("Finalized pending update to {}", pending.new_version);
        }
        Phase::RollingBack => {
            rollback(&mut pending, !boot)?;
            println!("Completed interrupted rollback to {}", pending.old_version);
        }
        Phase::Activating if boot => {
            rollback(&mut pending, false)?;
            println!("Rolled back interrupted update to {}", pending.old_version);
        }
        Phase::Activating => recover_activation(&mut pending)?,
    }
    Ok(true)
}

fn recover_activation(pending: &mut Pending) -> Result<()> {
    let (_, new) = pending.versions()?;
    if state::current_points_to(&pending.release_path)?
        && status::wait(&new, pending.baseline()).is_ok()
        && restart_updater_api(&new).is_ok()
    {
        commit(pending)?;
        println!("Finalized pending update to {new}");
    } else {
        rollback(pending, true)?;
        println!("Rolled back interrupted update to {}", pending.old_version);
    }
    Ok(())
}

fn rollback(pending: &mut Pending, start_services: bool) -> Result<()> {
    let (old, _) = pending.versions()?;
    pending.phase = Phase::RollingBack;
    state::write_pending(pending)?;
    if start_services {
        stop_services(&pending.baseline())?;
    }
    if pending.migration_applied {
        process::migration(
            &pending.release_path,
            "down",
            &pending.old_version,
            &pending.new_version,
        )?;
    }
    state::switch_current(&pending.previous_target)?;
    restart_updater_api(&old)?;
    if start_services {
        status::start_relay()?;
        status::start_agent()?;
        status::wait(&old, pending.baseline())?;
    }
    state::write_version(&old)?;
    state::remove_pending()?;
    progress::write(&UpdateProgress::error(&format!(
        "update to {} was rolled back",
        pending.new_version
    )))
}

fn stop_services(baseline: &status::Baseline) -> Result<()> {
    let mut errors = Vec::new();
    for (name, result) in [
        ("agent", status::stop_agent()),
        ("WireGuard", status::stop_tunnel(&baseline.interface)),
        ("relay", status::stop_relay()),
    ] {
        if let Err(error) = result {
            errors.push(format!("{name}: {error:#}"));
        }
    }
    ensure!(errors.is_empty(), "failed to stop {}", errors.join("; "));
    Ok(())
}

fn restart_updater_api(version: &Version) -> Result<()> {
    if !has_updater_api(version) {
        return Ok(());
    }
    status::restart_updater_api()?;
    status::wait_updater_api()
}

fn has_updater_api(version: &Version) -> bool {
    version >= &Version::new(0, 3, 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn updater_api_starts_at_version_0_3() {
        assert!(!has_updater_api(&Version::new(0, 2, 0)));
        assert!(has_updater_api(&Version::new(0, 3, 0)));
    }
}

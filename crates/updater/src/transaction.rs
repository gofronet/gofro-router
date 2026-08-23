use anyhow::{Result, anyhow, ensure};
use semver::Version;

use crate::{
    bundle, paths, process,
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
    status::wait(&new, pending.baseline())
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
    state::remove_pending()?;
    bundle::install_updater(&pending.release_path, &pending.new_version)
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
    if start_services {
        status::start_relay()?;
        status::start_agent()?;
        status::wait(&old, pending.baseline())?;
    }
    state::write_version(&old)?;
    state::remove_pending()
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

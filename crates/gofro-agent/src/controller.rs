mod mode;
mod routing;
mod servers;

#[cfg(test)]
pub(crate) use mode::reconcile;
pub(crate) use mode::{reconcile_locked, set_mode};
pub(crate) use routing::update_routing;
pub(crate) use servers::{add_server, delete_server, import_server, select_server, update_server};

use std::sync::atomic::Ordering;

use anyhow::{Result, anyhow, bail};

use crate::{AppState, config::save, dataplane, model::ControllerConfig, routing::RoutingPolicy};

#[derive(Clone, Copy)]
enum Change {
    Config,
    Policy,
    Network,
}

pub(crate) fn update_device_exclusion(
    state: &AppState,
    mac: crate::model::MacAddress,
    excluded: bool,
) -> Result<()> {
    update_config(state, |config| {
        config.device_exclusions.retain(|entry| *entry != mac);
        if excluded {
            config.device_exclusions.push(mac);
        }
        crate::config::normalize_exclusions(&mut config.device_exclusions)?;
        Ok(Change::Policy)
    })
}

pub(crate) fn set_auto_update(state: &AppState, enabled: bool) -> Result<()> {
    let _apply = crate::network::lock_apply(state)?;
    let mut config = state
        .config
        .lock()
        .map_err(|_| anyhow!("configuration lock poisoned"))?;
    let mut next = config.clone();
    next.auto_update_enabled = enabled;
    save(&state.config_path, &next)?;
    *config = next;
    Ok(())
}

// All desired-state writers take the same locks, including offline server edits.
fn update_config(
    state: &AppState,
    edit: impl FnOnce(&mut ControllerConfig) -> Result<Change>,
) -> Result<()> {
    let _apply = crate::network::lock_apply(state)?;
    let _update = state.fake_dns.begin_update()?;
    let mut config = state
        .config
        .lock()
        .map_err(|_| anyhow!("configuration lock poisoned"))?;
    if state.routing_degraded.load(Ordering::Relaxed) {
        bail!("routing is degraded; full reconcile is required before configuration changes");
    }
    let mut next = config.clone();
    let change = edit(&mut next)?;
    crate::config::normalize_exclusions(&mut next.device_exclusions)?;
    if matches!(change, Change::Config) {
        if let Err(error) = save(&state.config_path, &next) {
            if error.is::<crate::config::PublishedSaveError>() {
                *config = next;
                return Err(committed_failure(state, &config, error, true));
            }
            return Err(error);
        }
        *config = next;
        return Ok(());
    }
    let policy = RoutingPolicy::compile(next.routing.clone(), state.geodata.clone())?;
    let mut active = state
        .routing
        .write()
        .map_err(|_| anyhow!("routing lock poisoned"))?;
    // Replay desired state, then restore legacy addresses and pre-update disk state.
    let mut previous_network = None;
    if matches!(change, Change::Network) {
        external("snapshot", || {
            previous_network = Some(crate::network::Snapshot::capture(state)?);
            Ok(())
        })?;
    }
    // An install failure must never enter rollback: no mutation was protected.
    install_guard(state, &config.device_exclusions)?;
    let result = (|| {
        if matches!(change, Change::Network) {
            external("network", || mode::apply_network(state, &next))?;
        }
        apply_policy(state, next.vpn_enabled, &policy, &config.device_exclusions)?;
        save(&state.config_path, &next)
    })();
    if let Err(error) = result {
        if error.is::<crate::config::PublishedSaveError>() {
            *config = next;
            *active = policy;
            state.fake_dns.set_vpn_enabled(config.vpn_enabled);
            return Err(committed_failure(state, &config, error, true));
        }
        let rollback = {
            let network = (|| {
                if matches!(change, Change::Network) {
                    external("network", || mode::apply_network(state, &config))?;
                    if let Some(snapshot) = &previous_network {
                        snapshot.restore(state)?;
                    }
                }
                Ok::<_, anyhow::Error>(())
            })();
            let policy = apply_policy(
                state,
                config.vpn_enabled,
                &active,
                &config.device_exclusions,
            );
            network.and(policy)
        };
        if let Err(rollback) = rollback {
            state.routing_degraded.store(true, Ordering::Relaxed);
            return Err(anyhow!(
                "configuration update failed: {error:#}; rollback failed: {rollback:#}; forwarding guard remains installed"
            ));
        }
        return match clear_guard(state) {
            Ok(()) => Err(error),
            Err(guard) => Err(anyhow!("configuration update failed: {error:#}; {guard:#}")),
        };
    }
    let exclusions_changed = config.device_exclusions != next.device_exclusions;
    *config = next;
    *active = policy;
    state.fake_dns.set_vpn_enabled(config.vpn_enabled);
    let finish = (|| {
        if exclusions_changed {
            publish_exclusions(state, &config.device_exclusions)?;
            external("dns-cleanup", || crate::network::cleanup_dns_flows(state))?;
        }
        clear_guard(state)
    })();
    finish.map_err(|error| committed_failure(state, &config, error, false))
}

fn committed_failure(
    state: &AppState,
    config: &ControllerConfig,
    mut error: anyhow::Error,
    sync_required: bool,
) -> anyhow::Error {
    state.routing_degraded.store(true, Ordering::Relaxed);
    let repair = (|| {
        if sync_required {
            external("sync", || crate::config::sync_committed(&state.config_path))?;
        }
        publish_exclusions(state, &config.device_exclusions)
    })();
    if let Err(repair) = repair {
        error = error.context(format!("committed publication repair failed: {repair:#}"));
        // The old routing set may still exempt removed devices. Revoke their
        // forwarding independently, without reverting the published intent.
        if let Err(guard) = install_guard(state, &config.device_exclusions) {
            error = error.context(format!("committed forwarding guard failed: {guard:#}"));
        }
    } else if let Err(cleanup) =
        external("dns-cleanup", || crate::network::cleanup_dns_flows(state))
    {
        error = error.context(format!("committed DNS cleanup failed: {cleanup:#}"));
    }
    error.context(crate::managed::CommittedRefreshFailed)
}

fn install_guard(state: &AppState, exclusions: &[crate::model::MacAddress]) -> Result<()> {
    external("guard", || dataplane::install_guard(&state.lan, exclusions))?;
    #[cfg(test)]
    tests::record_kernel_exclusions("guard", exclusions);
    Ok(())
}

fn publish_exclusions(state: &AppState, exclusions: &[crate::model::MacAddress]) -> Result<()> {
    external("publish", || {
        dataplane::publish_exclusions(&state.lan, exclusions)
    })?;
    #[cfg(test)]
    tests::record_kernel_exclusions("publish", exclusions);
    Ok(())
}

fn apply_policy(
    state: &AppState,
    vpn_enabled: bool,
    policy: &RoutingPolicy,
    exclusions: &[crate::model::MacAddress],
) -> Result<()> {
    #[cfg(test)]
    tests::record_policy_exclusions(exclusions);
    let mappings = state.fake_dns.reclassified(policy)?;
    external("policy", || {
        dataplane::apply(
            &state.lan,
            state.dns_listen.port(),
            vpn_enabled,
            policy,
            &mappings,
            crate::model::PanelPorts {
                http: state.http_listen.port(),
                https: state.https_listen.port(),
            },
            exclusions,
        )
    })?;
    #[cfg(test)]
    tests::record_kernel_exclusions("policy", exclusions);
    state.fake_dns.commit_targets(policy)
}

fn clear_guard(state: &AppState) -> Result<()> {
    external("clear", dataplane::clear_guard).map_err(|error| {
        state.routing_degraded.store(true, Ordering::Relaxed);
        anyhow!("forwarding guard remains installed: {error:#}")
    })
}

fn external(_step: &'static str, run: impl FnOnce() -> Result<()>) -> Result<()> {
    #[cfg(test)]
    if let Some(result) = tests::run_external(_step) {
        result?;
        // Retirement uses the fixture's real temporary directory, not /etc.
        if _step != "retire" {
            return Ok(());
        }
    }
    run()
}

#[cfg(test)]
mod tests;

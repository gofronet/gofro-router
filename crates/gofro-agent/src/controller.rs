mod mode;
mod routing;
mod servers;

pub(crate) use mode::{reconcile, set_mode};
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
    if matches!(change, Change::Config) {
        save(&state.config_path, &next)?;
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
    external("guard", || dataplane::install_guard(&state.lan))?;
    let result = (|| {
        if matches!(change, Change::Network) {
            external("network", || mode::apply_network(state, &next))?;
        }
        apply_policy(state, next.vpn_enabled, &policy)?;
        save(&state.config_path, &next)
    })();
    if let Err(error) = result {
        let rollback = (|| {
            if matches!(change, Change::Network) {
                external("network", || mode::apply_network(state, &config))?;
                if let Some(snapshot) = &previous_network {
                    snapshot.restore(state)?;
                }
            }
            apply_policy(state, config.vpn_enabled, &active)
        })();
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
    *config = next;
    *active = policy;
    state.fake_dns.set_vpn_enabled(config.vpn_enabled);
    clear_guard(state)
}

fn apply_policy(state: &AppState, vpn_enabled: bool, policy: &RoutingPolicy) -> Result<()> {
    let mappings = state.fake_dns.reclassified(policy)?;
    external("policy", || {
        dataplane::apply(
            &state.lan,
            state.dns_listen.port(),
            vpn_enabled,
            policy,
            &mappings,
        )
    })?;
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

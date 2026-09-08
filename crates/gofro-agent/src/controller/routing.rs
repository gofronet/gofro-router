use std::sync::atomic::Ordering;

use anyhow::{Result, anyhow};

use crate::{
    AppState,
    config::{normalize_routing, save},
    dataplane,
    model::{RouteTarget, RoutingConfig, RoutingMode},
    routing::RoutingPolicy,
};

pub(crate) fn update_routing(state: &AppState, mut routing: RoutingConfig) -> Result<()> {
    normalize_routing(&mut routing)?;
    let policy = RoutingPolicy::compile(routing.clone(), state.geodata.clone())?;
    let _update = state.fake_dns.begin_update()?;
    let mut config = state
        .config
        .lock()
        .map_err(|_| anyhow!("configuration lock poisoned"))?;
    let mut active = state
        .routing
        .write()
        .map_err(|_| anyhow!("routing lock poisoned"))?;
    let previous_policy = active.clone();
    let mappings = state.fake_dns.reclassified(&policy)?;
    dataplane::apply(&state.lan_interface, &policy, &mappings)?;
    if let Err(error) = state.fake_dns.commit_targets(&policy) {
        return match restore_routing(state, &previous_policy) {
            Ok(()) => Err(error),
            Err(rollback) => degrade_to_block(state, &mut active, rollback, error),
        };
    }
    let previous = config.routing.clone();
    config.routing = routing;
    if let Err(error) = save(&state.config_path, &config) {
        config.routing = previous;
        return match restore_routing(state, &previous_policy) {
            Ok(()) => Err(error),
            Err(rollback) => degrade_to_block(state, &mut active, rollback, error),
        };
    }
    *active = policy;
    state.routing_degraded.store(false, Ordering::Relaxed);
    Ok(())
}

fn degrade_to_block(
    state: &AppState,
    active: &mut RoutingPolicy,
    rollback: anyhow::Error,
    error: anyhow::Error,
) -> Result<()> {
    state.routing_degraded.store(true, Ordering::Relaxed);
    let blocked = RoutingPolicy::compile(
        RoutingConfig {
            domain_rules: vec![],
            ip_rules: vec![],
            default_target: RouteTarget::Block,
            mode: RoutingMode::Rules,
            rule_order: None,
        },
        state.geodata.clone(),
    )?;
    dataplane::apply(&state.lan_interface, &blocked, &[]).map_err(|degraded| anyhow!(
        "routing update failed: {error:#}; rollback failed: {rollback:#}; fail-closed apply failed: {degraded:#}"
    ))?;
    *active = blocked;
    Err(anyhow!(
        "routing update failed: {error:#}; rollback failed: {rollback:#}; routing is fail-closed until the next successful update"
    ))
}

fn restore_routing(state: &AppState, policy: &RoutingPolicy) -> Result<()> {
    let mappings = state.fake_dns.reclassified(policy)?;
    let persisted = state.fake_dns.commit_targets(policy);
    let applied = dataplane::apply(&state.lan_interface, policy, &mappings);
    match (persisted, applied) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Err(persisted), Err(applied)) => Err(anyhow!(
            "FakeDNS rollback failed: {persisted:#}; dataplane rollback failed: {applied:#}"
        )),
    }
}

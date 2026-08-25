use anyhow::{Result, anyhow};

use crate::{
    AppState,
    config::{normalize_routing, save},
    dataplane,
    model::RoutingConfig,
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
            Err(rollback) => Err(anyhow!(
                "FakeDNS update failed: {error:#}; routing rollback failed: {rollback:#}"
            )),
        };
    }
    let previous = config.routing.clone();
    config.routing = routing;
    if let Err(error) = save(&state.config_path, &config) {
        config.routing = previous;
        return match restore_routing(state, &previous_policy) {
            Ok(()) => Err(error),
            Err(rollback) => Err(anyhow!(
                "configuration save failed: {error:#}; routing rollback failed: {rollback:#}"
            )),
        };
    }
    *active = policy;
    Ok(())
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

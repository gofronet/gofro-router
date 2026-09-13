use std::sync::atomic::Ordering;

use anyhow::{Context, Result, anyhow};

use super::{Change, apply_policy, clear_guard, external, update_config};
use crate::{
    AppState, dataplane,
    model::{ControllerConfig, ServerProfile},
    network::{apply_mode, start_and_select, stop_tunnel},
    routing::RoutingPolicy,
};

pub(crate) fn set_mode(state: &AppState, vpn_enabled: bool) -> Result<()> {
    update_config(state, |config| {
        let change = if config.vpn_enabled == vpn_enabled {
            Change::Policy
        } else {
            Change::Network
        };
        config.vpn_enabled = vpn_enabled;
        if vpn_enabled {
            active_server(config)?;
        }
        Ok(change)
    })
}

pub(crate) fn reconcile(state: &AppState) -> Result<()> {
    let _apply = crate::network::lock_apply(state)?;
    let _update = state.fake_dns.begin_update()?;
    let config = state
        .config
        .lock()
        .map_err(|_| anyhow!("configuration lock poisoned"))?;
    state.routing_degraded.store(true, Ordering::Relaxed);
    let policy = RoutingPolicy::compile(config.routing.clone(), state.geodata.clone())?;
    let mut active = state
        .routing
        .write()
        .map_err(|_| anyhow!("routing lock poisoned"))?;
    external("guard", || dataplane::install_guard(&state.lan))?;
    // Only a full reconcile may take ownership of a guard left by a failed update.
    external("network", || apply_network(state, &config))?;
    apply_policy(state, config.vpn_enabled, &policy)?;
    *active = policy;
    state.fake_dns.set_vpn_enabled(config.vpn_enabled);
    external("retire", || crate::network::retire_legacy_routing(state))?;
    clear_guard(state)?;
    state.routing_degraded.store(false, Ordering::Relaxed);
    Ok(())
}

pub(super) fn apply_network(state: &AppState, config: &ControllerConfig) -> Result<()> {
    if config.vpn_enabled {
        let server = active_server(config)?;
        apply_mode(state, "vpn")?;
        external("select-peer", || start_and_select(state, server))?;
        // Hotplug may miss its nonblocking mode lock, including on an active tunnel.
        apply_mode(state, "tunnel-up")
    } else {
        stop_tunnel(&state.interface)?;
        apply_mode(state, "bypass")
    }
}

fn active_server(config: &ControllerConfig) -> Result<&ServerProfile> {
    let key = config
        .active_server_key
        .as_deref()
        .context("активный VPN-сервер не выбран")?;
    config
        .servers
        .iter()
        .find(|server| server.public_key == key)
        .context("активный VPN-сервер отсутствует в списке")
}

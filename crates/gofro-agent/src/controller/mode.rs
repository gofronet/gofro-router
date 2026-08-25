use anyhow::{Context, Result, anyhow};

use crate::{
    AppState,
    config::save,
    dataplane,
    model::{ControllerConfig, ServerProfile},
    network::{apply_mode, start_and_select, stop_tunnel},
};

pub(crate) fn set_mode(state: &AppState, vpn_enabled: bool) -> Result<()> {
    let _update = state.fake_dns.begin_update()?;
    let mut config = state
        .config
        .lock()
        .map_err(|_| anyhow!("configuration lock poisoned"))?;
    let previous = config.vpn_enabled;
    if previous == vpn_enabled {
        return switch_mode(state, &config, vpn_enabled);
    }
    if let Err(error) = switch_mode(state, &config, vpn_enabled) {
        return match switch_mode(state, &config, previous) {
            Ok(()) => Err(error),
            Err(rollback) => Err(anyhow!(
                "mode switch failed: {error:#}; rollback failed: {rollback:#}"
            )),
        };
    }
    config.vpn_enabled = vpn_enabled;
    if let Err(error) = save(&state.config_path, &config) {
        config.vpn_enabled = previous;
        return match switch_mode(state, &config, previous) {
            Ok(()) => Err(error),
            Err(rollback) => Err(anyhow!(
                "configuration save failed: {error:#}; mode rollback failed: {rollback:#}"
            )),
        };
    }
    Ok(())
}

pub(crate) fn reconcile(state: &AppState) -> Result<()> {
    let _update = state.fake_dns.begin_update()?;
    let config = state
        .config
        .lock()
        .map_err(|_| anyhow!("configuration lock poisoned"))?
        .clone();
    if config.vpn_enabled {
        let server = active_server(&config)?;
        apply_mode(state, "vpn")?;
        start_and_select(state, server)?;
    } else {
        stop_tunnel(&state.interface)?;
        apply_mode(state, "bypass")?;
    }
    let policy = state
        .routing
        .read()
        .map_err(|_| anyhow!("routing lock poisoned"))?;
    let mappings = state.fake_dns.reclassified(&policy)?;
    dataplane::apply(&state.lan_interface, &policy, &mappings)?;
    state.fake_dns.commit_targets(&policy)
}

pub(super) fn switch_mode(
    state: &AppState,
    config: &ControllerConfig,
    vpn_enabled: bool,
) -> Result<()> {
    if vpn_enabled {
        let server = active_server(config)?;
        apply_mode(state, "vpn")?;
        start_and_select(state, server)
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

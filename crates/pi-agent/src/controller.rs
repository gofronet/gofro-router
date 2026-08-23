use anyhow::{Context, Result, anyhow, bail};

use crate::{
    AppState,
    config::{save, validate_server, validate_ssid},
    model::{ApInput, ControllerConfig, ServerProfile, ServerUpdate},
    network::{apply_mode, select_server_peer, start_and_select, stop_tunnel, update_ap},
};

pub(crate) fn set_mode(state: &AppState, vpn_enabled: bool) -> Result<()> {
    let mut config = state
        .config
        .lock()
        .map_err(|_| anyhow!("configuration lock poisoned"))?;
    if vpn_enabled {
        let server = active_server(&config)?.clone();
        apply_mode(state, "vpn")?;
        if let Err(error) = start_and_select(state, &server) {
            let _ = stop_tunnel(&state.interface);
            let _ = apply_mode(state, "bypass");
            return Err(error);
        }
    } else {
        stop_tunnel(&state.interface)?;
        apply_mode(state, "bypass")?;
    }

    config.vpn_enabled = vpn_enabled;
    save(&state.config_path, &config)
}

pub(crate) fn add_server(state: &AppState, server: ServerProfile) -> Result<()> {
    validate_server(&server)?;
    let mut config = state
        .config
        .lock()
        .map_err(|_| anyhow!("configuration lock poisoned"))?;
    if config
        .servers
        .iter()
        .any(|item| item.public_key == server.public_key)
    {
        bail!("сервер с таким public key уже существует");
    }

    if config.active_server_key.is_none() {
        config.active_server_key = Some(server.public_key.clone());
    }
    config.servers.push(server);
    save(&state.config_path, &config)
}

pub(crate) fn update_server(state: &AppState, update: ServerUpdate) -> Result<()> {
    let server = ServerProfile {
        name: update.name,
        endpoint: update.endpoint,
        public_key: update.public_key,
    };
    validate_server(&server)?;

    let mut config = state
        .config
        .lock()
        .map_err(|_| anyhow!("configuration lock poisoned"))?;
    let index = config
        .servers
        .iter()
        .position(|server| server.public_key == update.previous_public_key)
        .context("сервер не найден")?;
    if config
        .servers
        .iter()
        .enumerate()
        .any(|(other, item)| other != index && item.public_key == server.public_key)
    {
        bail!("сервер с таким public key уже существует");
    }

    let was_active = config.active_server_key.as_deref() == Some(&update.previous_public_key);
    let connection_changed = config.servers[index].endpoint != server.endpoint
        || config.servers[index].public_key != server.public_key;
    if was_active && config.vpn_enabled && connection_changed {
        select_server_peer(state, &server)?;
    }
    if was_active {
        config.active_server_key = Some(server.public_key.clone());
    }
    config.servers[index] = server;
    save(&state.config_path, &config)
}

pub(crate) fn update_access_point(state: &AppState, input: ApInput) -> Result<()> {
    validate_ssid(&input.ssid)?;
    let password = input
        .password
        .as_deref()
        .filter(|password| !password.is_empty());
    if password.is_some_and(|password| !(8..=63).contains(&password.len())) {
        bail!("пароль Wi-Fi должен содержать от 8 до 63 символов");
    }
    update_ap(&input.ssid, password)?;

    let mut config = state
        .config
        .lock()
        .map_err(|_| anyhow!("configuration lock poisoned"))?;
    config.ap_ssid = input.ssid;
    save(&state.config_path, &config)
}

pub(crate) fn select_server(state: &AppState, public_key: &str) -> Result<()> {
    let mut config = state
        .config
        .lock()
        .map_err(|_| anyhow!("configuration lock poisoned"))?;
    let server = config
        .servers
        .iter()
        .find(|server| server.public_key == public_key)
        .context("сервер не найден")?
        .clone();
    if config.active_server_key.as_deref() == Some(public_key) {
        return Ok(());
    }

    if config.vpn_enabled {
        select_server_peer(state, &server)?;
    }
    config.active_server_key = Some(server.public_key);
    save(&state.config_path, &config)
}

pub(crate) fn delete_server(state: &AppState, public_key: &str) -> Result<()> {
    let mut config = state
        .config
        .lock()
        .map_err(|_| anyhow!("configuration lock poisoned"))?;
    let original_len = config.servers.len();
    config
        .servers
        .retain(|server| server.public_key != public_key);
    if config.servers.len() == original_len {
        bail!("сервер не найден");
    }

    if config.active_server_key.as_deref() == Some(public_key) {
        config.active_server_key = config
            .servers
            .first()
            .map(|server| server.public_key.clone());
        if config.vpn_enabled {
            if let Some(server) = config.servers.first() {
                select_server_peer(state, server)?;
            } else {
                stop_tunnel(&state.interface)?;
                apply_mode(state, "bypass")?;
                config.vpn_enabled = false;
            }
        }
    }

    save(&state.config_path, &config)
}

pub(crate) fn reconcile(state: &AppState) -> Result<()> {
    let config = state
        .config
        .lock()
        .map_err(|_| anyhow!("configuration lock poisoned"))?
        .clone();
    if config.vpn_enabled {
        let server = active_server(&config)?;
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

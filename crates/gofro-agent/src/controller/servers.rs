use anyhow::{Context, Result, anyhow, bail};

use super::mode::switch_mode;
use crate::{
    AppState,
    config::{parse_server_profile, save, validate_server},
    model::{ControllerConfig, ServerProfile, ServerUpdate},
    network::select_server_peer,
};

pub(crate) fn import_server(state: &AppState, name: String, profile: String) -> Result<()> {
    let server = parse_server_profile(name, &profile)?;
    let mut config = state
        .config
        .lock()
        .map_err(|_| anyhow!("configuration lock poisoned"))?;
    let Some((next, previous, reconnect, index)) = replace_imported_server(&config, &server) else {
        drop(config);
        return add_server(state, server);
    };
    if reconnect {
        select_server_peer(state, &next.servers[index])?;
    }

    if let Err(error) = save(&state.config_path, &next) {
        if reconnect {
            return match select_server_peer(state, &previous) {
                Ok(()) => Err(error),
                Err(rollback) => Err(anyhow!(
                    "configuration save failed: {error:#}; server rollback failed: {rollback:#}"
                )),
            };
        }
        return Err(error);
    }
    *config = next;
    Ok(())
}

fn replace_imported_server(
    config: &ControllerConfig,
    server: &ServerProfile,
) -> Option<(ControllerConfig, ServerProfile, bool, usize)> {
    let index = config
        .servers
        .iter()
        .position(|item| item.public_key == server.public_key)?;
    let previous = config.servers[index].clone();
    let reconnect =
        config.vpn_enabled && config.active_server_key.as_deref() == Some(&server.public_key);
    let mut next = config.clone();
    next.servers[index] = server.clone();
    Some((next, previous, reconnect, index))
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

    let mut next = config.clone();
    if next.active_server_key.is_none() {
        next.active_server_key = Some(server.public_key.clone());
    }
    next.servers.push(server);
    save(&state.config_path, &next)?;
    *config = next;
    Ok(())
}

pub(crate) fn update_server(state: &AppState, update: ServerUpdate) -> Result<()> {
    let mut config = state
        .config
        .lock()
        .map_err(|_| anyhow!("configuration lock poisoned"))?;
    let index = config
        .servers
        .iter()
        .position(|server| server.public_key == update.previous_public_key)
        .context("сервер не найден")?;
    let mut server = config.servers[index].clone();
    server.name = update.name;
    server.endpoint = update.endpoint;
    server.public_key = update.public_key;
    validate_server(&server)?;
    if config
        .servers
        .iter()
        .enumerate()
        .any(|(other, item)| other != index && item.public_key == server.public_key)
    {
        bail!("сервер с таким public key уже существует");
    }

    let previous = config.clone();
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
    if let Err(error) = save(&state.config_path, &config) {
        *config = previous.clone();
        if was_active && previous.vpn_enabled && connection_changed {
            return match select_server_peer(state, &previous.servers[index]) {
                Ok(()) => Err(error),
                Err(rollback) => Err(anyhow!(
                    "configuration save failed: {error:#}; server rollback failed: {rollback:#}"
                )),
            };
        }
        return Err(error);
    }
    Ok(())
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

    let previous_key = config.active_server_key.clone();
    let previous_server = previous_key
        .as_deref()
        .and_then(|key| {
            config
                .servers
                .iter()
                .find(|server| server.public_key == key)
        })
        .cloned();
    if config.vpn_enabled {
        select_server_peer(state, &server)?;
    }
    config.active_server_key = Some(server.public_key);
    if let Err(error) = save(&state.config_path, &config) {
        config.active_server_key = previous_key;
        if let (true, Some(previous_server)) = (config.vpn_enabled, previous_server) {
            return match select_server_peer(state, &previous_server) {
                Ok(()) => Err(error),
                Err(rollback) => Err(anyhow!(
                    "configuration save failed: {error:#}; server rollback failed: {rollback:#}"
                )),
            };
        }
        return Err(error);
    }
    Ok(())
}

pub(crate) fn delete_server(state: &AppState, public_key: &str) -> Result<()> {
    let _update = state.fake_dns.begin_update()?;
    let mut config = state
        .config
        .lock()
        .map_err(|_| anyhow!("configuration lock poisoned"))?;
    let previous = config.clone();
    let mut next = previous.clone();
    let original_len = next.servers.len();
    next.servers
        .retain(|server| server.public_key != public_key);
    if next.servers.len() == original_len {
        bail!("сервер не найден");
    }

    if next.active_server_key.as_deref() == Some(public_key) {
        next.active_server_key = next.servers.first().map(|server| server.public_key.clone());
        if next.vpn_enabled {
            if let Some(server) = next.servers.first() {
                if let Err(error) = select_server_peer(state, server) {
                    return match switch_mode(state, &previous, previous.vpn_enabled) {
                        Ok(()) => Err(error),
                        Err(rollback) => Err(anyhow!(
                            "server switch failed: {error:#}; rollback failed: {rollback:#}"
                        )),
                    };
                }
            } else {
                next.vpn_enabled = false;
                if let Err(error) = switch_mode(state, &next, false) {
                    return match switch_mode(state, &previous, previous.vpn_enabled) {
                        Ok(()) => Err(error),
                        Err(rollback) => Err(anyhow!(
                            "mode switch failed: {error:#}; rollback failed: {rollback:#}"
                        )),
                    };
                }
            }
        }
    }

    if let Err(error) = save(&state.config_path, &next) {
        return match switch_mode(state, &previous, previous.vpn_enabled) {
            Ok(()) => Err(error),
            Err(rollback) => Err(anyhow!(
                "configuration save failed: {error:#}; server rollback failed: {rollback:#}"
            )),
        };
    }
    *config = next;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::RoutingConfig;

    #[test]
    fn reimport_replaces_credentials_for_active_server() {
        let old = ServerProfile {
            name: "Old".into(),
            endpoint: "old.example:8443".into(),
            public_key: "server-key".into(),
            client_tunnel_address: Some("10.202.0.2/32".into()),
            client_private_key: Some("old-private".into()),
        };
        let config = ControllerConfig {
            vpn_enabled: true,
            active_server_key: Some(old.public_key.clone()),
            servers: vec![old],
            routing: RoutingConfig::default(),
        };
        let new = ServerProfile {
            name: "New".into(),
            endpoint: "new.example:8443".into(),
            public_key: "server-key".into(),
            client_tunnel_address: Some("10.202.0.5/32".into()),
            client_private_key: Some("new-private".into()),
        };

        let (next, previous, reconnect, _) = replace_imported_server(&config, &new).unwrap();
        assert!(reconnect);
        assert_eq!(next.servers.len(), 1);
        assert_eq!(next.servers[0].name, "New");
        assert_eq!(
            next.servers[0].client_tunnel_address.as_deref(),
            Some("10.202.0.5/32")
        );
        assert_eq!(
            next.servers[0].client_private_key.as_deref(),
            Some("new-private")
        );
        assert_eq!(previous.client_private_key.as_deref(), Some("old-private"));
    }
}

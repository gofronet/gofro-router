use anyhow::{Context, Result, bail};

use super::{Change, update_config};

use crate::{
    AppState,
    config::{normalize_server_name, parse_server_profile, validate_server},
    model::{ControllerConfig, ServerProfile, ServerUpdate},
};

pub(crate) fn import_server(state: &AppState, name: String, profile: String) -> Result<()> {
    let server = parse_server_profile(name, &profile)?;
    upsert_server(state, server)
}

pub(crate) fn upsert_server(state: &AppState, server: ServerProfile) -> Result<()> {
    validate_server(&server)?;
    update_config(state, |config| {
        if let Some((next, reconnect)) = replace_imported_server(config, &server) {
            *config = next;
            Ok(if reconnect {
                Change::Network
            } else {
                Change::Config
            })
        } else {
            insert_server(config, server)
        }
    })
}

fn replace_imported_server(
    config: &ControllerConfig,
    server: &ServerProfile,
) -> Option<(ControllerConfig, bool)> {
    let index = config
        .servers
        .iter()
        .position(|item| item.public_key == server.public_key)?;
    let previous = &config.servers[index];
    let reconnect =
        config.vpn_enabled && config.active_server_key.as_deref() == Some(&server.public_key);
    let mut next = config.clone();
    let mut server = server.clone();
    if previous.management.is_some() {
        server.management = previous.management.clone();
        server.endpoint = previous.endpoint.clone();
    }
    server.emoji = previous.emoji.clone();
    next.servers[index] = server;
    Some((next, reconnect))
}

pub(crate) fn add_server(state: &AppState, server: ServerProfile) -> Result<()> {
    validate_server(&server)?;
    update_config(state, |config| insert_server(config, server))
}

fn insert_server(config: &mut ControllerConfig, server: ServerProfile) -> Result<Change> {
    if config
        .servers
        .iter()
        .any(|item| item.public_key == server.public_key)
    {
        bail!("сервер с таким public key уже существует");
    }

    let becomes_active = config.active_server_key.is_none();
    if becomes_active {
        config.active_server_key = Some(server.public_key.clone());
    }
    config.servers.push(server);
    Ok(if becomes_active && config.vpn_enabled {
        Change::Network
    } else {
        Change::Config
    })
}

pub(crate) fn update_server(state: &AppState, mut update: ServerUpdate) -> Result<()> {
    normalize_server_name(&mut update.name)?;
    update_config(state, |config| {
        let index = config
            .servers
            .iter()
            .position(|server| server.public_key == update.previous_public_key)
            .context("сервер не найден")?;
        let mut server = config.servers[index].clone();
        if server.management.is_some()
            && (server.endpoint != update.endpoint || server.public_key != update.public_key)
        {
            bail!("нельзя изменить endpoint или public key управляемого сервера");
        }
        server.name = update.name;
        server.endpoint = update.endpoint;
        server.public_key = update.public_key;
        if let Some(emoji) = update.emoji {
            server.emoji = emoji;
        }
        validate_server(&server)?;
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
        if was_active {
            config.active_server_key = Some(server.public_key.clone());
        }
        config.servers[index] = server;
        Ok(if was_active && config.vpn_enabled && connection_changed {
            Change::Network
        } else {
            Change::Config
        })
    })
}

pub(crate) fn select_server(state: &AppState, public_key: &str) -> Result<()> {
    update_config(state, |config| {
        let server = config
            .servers
            .iter()
            .find(|server| server.public_key == public_key)
            .context("сервер не найден")?
            .clone();
        if config.active_server_key.as_deref() == Some(public_key) {
            return Ok(Change::Config);
        }

        config.active_server_key = Some(server.public_key);
        Ok(if config.vpn_enabled {
            Change::Network
        } else {
            Change::Config
        })
    })
}

pub(crate) fn delete_server(state: &AppState, public_key: &str) -> Result<()> {
    update_config(state, |config| {
        // Preserve legacy profile pins before removing their only stored copy.
        crate::managed::preserve_host_pins(&state.management_dir, config)?;
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
                if config.servers.is_empty() {
                    bail!("сначала отключите VPN, затем удалите последний сервер");
                }
                return Ok(Change::Network);
            }
        }
        Ok(Change::Config)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ManagedServer, RoutingConfig};

    #[test]
    fn reimport_replaces_credentials_for_active_server() {
        let old = ServerProfile {
            name: "Old".into(),
            emoji: "🛰".into(),
            endpoint: "old.example:8443".into(),
            public_key: "server-key".into(),
            client_tunnel_address: Some("10.202.0.2/32".into()),
            client_private_key: Some("old-private".into()),
            management: Some(ManagedServer {
                host: "203.0.113.10".into(),
                port: 22,
                host_key: "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIEwCks2omLrfMrS1du13ol2Iwo4CoDhME50jxaLI7Mdq".into(),
            }),
        };
        let config = ControllerConfig {
            vpn_enabled: true,
            active_server_key: Some(old.public_key.clone()),
            servers: vec![old],
            routing: RoutingConfig::default(),
        };
        let new = ServerProfile {
            name: "New".into(),
            emoji: String::new(),
            endpoint: "new.example:8443".into(),
            public_key: "server-key".into(),
            client_tunnel_address: Some("10.202.0.5/32".into()),
            client_private_key: Some("new-private".into()),
            management: None,
        };

        let (next, reconnect) = replace_imported_server(&config, &new).unwrap();
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
        assert_eq!(
            config.servers[0].client_private_key.as_deref(),
            Some("old-private")
        );
        assert!(next.servers[0].management.is_some());
        assert_eq!(next.servers[0].emoji, "🛰");
    }
}

use std::{
    collections::{BTreeMap, BTreeSet},
    process::Command,
};

use anyhow::{Context, Result, anyhow, bail};
use wireguard_status::managed::{FriendPeer, ManagedServerStatus, validate_peer_key};

mod store;

use store::{FriendRecord, FriendStore, interface_name, read_store, store_path, write_store};

pub(super) fn expected_uid() -> u32 {
    store::expected_uid()
}

enum PeerKind {
    Friend(String),
    Owner,
    Other,
}

fn peers(interface: &str, owners: &BTreeSet<String>) -> Result<BTreeMap<String, PeerKind>> {
    let output = super::run(Command::new("wg").args(["show", interface, "allowed-ips"]))?;
    Ok(output
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            Some((
                fields.next()?.to_owned(),
                fields
                    .flat_map(|routes| routes.split(','))
                    .map(str::to_owned)
                    .collect::<Vec<_>>(),
            ))
        })
        .map(|(key, routes)| {
            let peer = if owners.contains(&key) {
                PeerKind::Owner
            } else {
                classify_friend(&routes)
            };
            (key, peer)
        })
        .collect())
}

fn classify_friend(routes: &[String]) -> PeerKind {
    if let [route] = routes {
        super::validate_tunnel_ip(route)
            .map(|ip| PeerKind::Friend(ip.to_string()))
            .unwrap_or(PeerKind::Other)
    } else {
        PeerKind::Other
    }
}

fn owner_keys(store: &FriendStore) -> BTreeSet<String> {
    store.owners.iter().cloned().collect()
}

// `10.203` is legacy evidence only. New peers never receive or advertise it.
fn load_store(interface: &str) -> Result<FriendStore> {
    let mut store = read_store(interface)?;
    if store.schema == 1 {
        let output = super::run(Command::new("wg").args(["show", interface, "allowed-ips"]))?;
        if !store_path(interface)?.exists()
            && peers(interface, &BTreeSet::new())?
                .values()
                .any(|peer| matches!(peer, PeerKind::Friend(_)))
        {
            bail!("missing metadata cannot distinguish router owners from legacy friends");
        }
        let owners: Vec<_> = output
            .lines()
            .filter_map(|line| {
                let mut fields = line.split_whitespace();
                let key = fields.next()?;
                fields
                    .flat_map(|route| route.split(','))
                    .any(|route| route == super::CLIENT_SUBNET)
                    .then(|| key.to_owned())
            })
            .collect();
        if owners.is_empty()
            && output
                .lines()
                .any(|line| line.split_whitespace().nth(1).is_some())
        {
            bail!("cannot safely identify legacy router owner; use a migrated server");
        }
        store.schema = 2;
        store.owners = owners;
        for owner in &store.owners {
            validate_peer_key(owner)?;
            if store
                .records
                .iter()
                .any(|record| &record.public_key == owner)
            {
                bail!("legacy owner overlaps friend metadata; refusing ambiguous migration");
            }
        }
        write_store(interface, &store)?;
    }
    Ok(store)
}

fn active_name<'a>(
    store: &'a FriendStore,
    peers: &'a BTreeMap<String, PeerKind>,
    public_key: Option<&str>,
    name: &str,
) -> bool {
    peers.iter().any(|(key, peer)| {
        if Some(key.as_str()) == public_key {
            return false;
        }
        let PeerKind::Friend(ip) = peer else {
            return false;
        };
        store
            .records
            .iter()
            .find(|record| &record.public_key == key)
            .map_or(ip, |record| &record.name)
            .to_lowercase()
            == name.to_lowercase()
    })
}

fn peer_shape<'a>(
    store: &FriendStore,
    peers: &'a BTreeMap<String, PeerKind>,
    public_key: &str,
) -> Result<Option<&'a str>> {
    validate_peer_key(public_key)?;
    if store.owners.iter().any(|key| key == public_key) {
        bail!("router peer cannot be managed as a friend");
    }
    let Some(peer) = peers.get(public_key) else {
        return Ok(None);
    };
    match peer {
        PeerKind::Friend(ip) => Ok(Some(ip)),
        PeerKind::Owner => bail!("router peer cannot be managed as a friend"),
        PeerKind::Other => bail!("peer is not a managed friend shape"),
    }
}

pub fn status(interface: &str) -> Result<ManagedServerStatus> {
    let store = load_store(interface)?;
    let active = peers(interface, &owner_keys(&store))?;
    let mut peers: Vec<_> = store
        .records
        .iter()
        .filter_map(|record| match active.get(&record.public_key) {
            Some(PeerKind::Owner | PeerKind::Other) => None,
            Some(PeerKind::Friend(_)) => Some(FriendPeer {
                public_key: record.public_key.clone(),
                name: record.name.clone(),
                revoked: false,
                can_share: record.private_key.is_some(),
            }),
            None => Some(FriendPeer {
                public_key: record.public_key.clone(),
                name: record.name.clone(),
                revoked: true,
                can_share: false,
            }),
        })
        .collect();
    for (key, peer) in active {
        let PeerKind::Friend(ip) = peer else { continue };
        if !store.records.iter().any(|record| record.public_key == key) {
            peers.push(FriendPeer {
                public_key: key,
                name: ip,
                revoked: false,
                can_share: false,
            });
        }
    }
    peers.sort_by(|left, right| {
        left.name
            .cmp(&right.name)
            .then(left.public_key.cmp(&right.public_key))
    });
    Ok(ManagedServerStatus {
        version: env!("CARGO_PKG_VERSION").to_owned(),
        peers,
    })
}

pub fn create(interface: &str, endpoint: &str, name: String) -> Result<String> {
    let mut store = load_store(interface)?;
    let active = peers(interface, &owner_keys(&store))?;
    if active_name(&store, &active, None, &name) {
        bail!("an active friend already has this name");
    }
    let previous_config = super::run(Command::new("wg").args(["showconf", interface]))?;
    let private_key = super::run(Command::new("wg").arg("genkey"))?
        .trim()
        .to_owned();
    let public_key = super::run_with_input(Command::new("wg").arg("pubkey"), &private_key)?
        .trim()
        .to_owned();
    validate_peer_key(&public_key)?;
    let tunnel_ip = super::allocate_tunnel_ip(&super::run(Command::new("wg").args([
        "show",
        interface,
        "allowed-ips",
    ]))?)?
    .to_string();
    let server_key = super::run(Command::new("wg").args(["show", interface, "public-key"]))?
        .trim()
        .to_owned();
    store.records.push(FriendRecord {
        public_key: public_key.clone(),
        name,
        private_key: Some(private_key.clone()),
    });
    write_store(interface, &store)?;
    let result = (|| {
        super::run(Command::new("wg").args([
            "set",
            interface,
            "peer",
            &public_key,
            "allowed-ips",
            &tunnel_ip,
        ]))?;
        super::save(interface)
    })();
    if let Err(error) = result {
        let rollback = (|| {
            super::run_with_input(
                Command::new("wg").args(["setconf", interface, "/dev/stdin"]),
                &previous_config,
            )?;
            super::save(interface)?;
            store.records.pop();
            write_store(interface, &store)
        })();
        return match rollback {
            Ok(()) => Err(error),
            Err(rollback) => Err(anyhow!(
                "friend creation failed: {error:#}; rollback failed: {rollback:#}"
            )),
        };
    }
    Ok(super::format_profile(
        &private_key,
        &public_key,
        &server_key,
        endpoint,
        &tunnel_ip,
    ))
}

pub fn rename(interface: &str, public_key: &str, name: String) -> Result<()> {
    let mut store = load_store(interface)?;
    let active = peers(interface, &owner_keys(&store))?;
    let current = peer_shape(&store, &active, public_key)?;
    if current.is_none() {
        bail!("friend is not active");
    }
    if active_name(&store, &active, Some(public_key), &name) {
        bail!("an active friend already has this name");
    }
    if let Some(record) = store
        .records
        .iter_mut()
        .find(|record| record.public_key == public_key)
    {
        record.name = name;
    } else {
        store.records.push(FriendRecord {
            public_key: public_key.to_owned(),
            name,
            private_key: None,
        });
    }
    write_store(interface, &store)
}

pub fn revoke(interface: &str, public_key: &str) -> Result<()> {
    let mut store = load_store(interface)?;
    let active = peers(interface, &owner_keys(&store))?;
    let active = match peer_shape(&store, &active, public_key)? {
        Some(active) => active,
        None => {
            if store
                .records
                .iter()
                .any(|record| record.public_key == public_key)
            {
                // A failed save and rollback can leave only the boot config granting access.
                return super::save(interface);
            }
            bail!("friend not found");
        }
    };
    let previous_store = store.clone();
    let previous_config = super::run(Command::new("wg").args(["showconf", interface]))?;
    if !store
        .records
        .iter()
        .any(|record| record.public_key == public_key)
    {
        store.records.push(FriendRecord {
            public_key: public_key.to_owned(),
            name: active.to_owned(),
            private_key: None,
        });
    }
    write_store(interface, &store)?;
    let result = (|| {
        super::run(Command::new("wg").args(["set", interface, "peer", public_key, "remove"]))?;
        super::save(interface)
    })();
    if let Err(error) = result {
        let rollback = (|| {
            super::run_with_input(
                Command::new("wg").args(["setconf", interface, "/dev/stdin"]),
                &previous_config,
            )?;
            super::save(interface)?;
            write_store(interface, &previous_store)
        })();
        return match rollback {
            Ok(()) => Err(error),
            Err(rollback) => Err(anyhow!(
                "friend revoke failed: {error:#}; rollback failed: {rollback:#}"
            )),
        };
    }
    Ok(())
}

pub fn profile(interface: &str, public_key: &str, endpoint: &str) -> Result<String> {
    let store = load_store(interface)?;
    let active = peers(interface, &owner_keys(&store))?;
    let ip = peer_shape(&store, &active, public_key)?.context("friend is not active")?;
    let record = store
        .records
        .iter()
        .find(|record| record.public_key == public_key)
        .context("friend profile is unavailable")?;
    let private_key = record
        .private_key
        .as_deref()
        .context("friend profile is unavailable")?;
    let server_key = super::run(Command::new("wg").args(["show", interface, "public-key"]))?;
    let derived = super::run_with_input(Command::new("wg").arg("pubkey"), private_key)?;
    if derived.trim() != public_key {
        bail!("friend profile is unavailable");
    }
    Ok(super::format_profile(
        private_key,
        public_key,
        server_key.trim(),
        endpoint,
        ip,
    ))
}

pub fn create_router_profile(interface: &str, endpoint: &str) -> Result<String> {
    load_store(interface)?;
    let private_key = super::run(Command::new("wg").arg("genkey"))?
        .trim()
        .to_owned();
    let public_key = super::run_with_input(Command::new("wg").arg("pubkey"), &private_key)?
        .trim()
        .to_owned();
    validate_peer_key(&public_key)?;
    let tunnel_ip = super::allocate_tunnel_ip(&super::run(Command::new("wg").args([
        "show",
        interface,
        "allowed-ips",
    ]))?)?
    .to_string();
    let server_key = super::run(Command::new("wg").args(["show", interface, "public-key"]))?
        .trim()
        .to_owned();
    add_router_peer(interface, &public_key, &tunnel_ip)?;
    Ok(super::format_profile(
        &private_key,
        &public_key,
        &server_key,
        endpoint,
        &tunnel_ip,
    ))
}

pub fn add_router_peer(interface: &str, public_key: &str, tunnel_ip: &str) -> Result<()> {
    validate_peer_key(public_key)?;
    let tunnel_ip = super::validate_tunnel_ip(tunnel_ip)?;
    super::ensure_routes_available(interface, public_key, &[tunnel_ip])?;
    let mut store = load_store(interface)?;
    let existing_owner = store.owners.iter().any(|key| key == public_key);
    if store
        .records
        .iter()
        .any(|record| record.public_key == public_key)
        || (!existing_owner && peers(interface, &owner_keys(&store))?.contains_key(public_key))
    {
        bail!("cannot convert an existing friend or unclassified peer to an owner");
    }
    let previous_config = super::run(Command::new("wg").args(["showconf", interface]))?;
    if !existing_owner {
        store.owners.push(public_key.to_owned());
    }
    // Persist identity before WireGuard can expose a bare /32, including on partial failure.
    write_store(interface, &store)?;
    let result = (|| {
        super::run(Command::new("wg").args([
            "set",
            interface,
            "peer",
            public_key,
            "allowed-ips",
            &tunnel_ip.to_string(),
        ]))?;
        super::save(interface)
    })();
    if let Err(error) = result {
        let rollback = (|| {
            super::run_with_input(
                Command::new("wg").args(["setconf", interface, "/dev/stdin"]),
                &previous_config,
            )?;
            super::save(interface)
        })();
        if let Err(rollback) = rollback {
            return Err(anyhow!(
                "router owner creation failed: {error:#}; rollback failed: {rollback:#}"
            ));
        }
        if !existing_owner {
            store.owners.pop();
            write_store(interface, &store)?;
        }
        return Err(error);
    }
    Ok(())
}

pub fn remove_router_peer(interface: &str, public_key: &str) -> Result<()> {
    let store = load_store(interface)?;
    validate_peer_key(public_key)?;
    if !store.owners.iter().any(|key| key == public_key) {
        bail!("router owner not found");
    }
    let previous_config = super::run(Command::new("wg").args(["showconf", interface]))?;
    // Keep owner tombstones: a restored WireGuard config must never turn an owner into a friend.
    let result = (|| {
        super::run(Command::new("wg").args(["set", interface, "peer", public_key, "remove"]))?;
        super::save(interface)
    })();
    if let Err(error) = result {
        let rollback = (|| {
            super::run_with_input(
                Command::new("wg").args(["setconf", interface, "/dev/stdin"]),
                &previous_config,
            )?;
            super::save(interface)
        })();
        return match rollback {
            Ok(()) => Err(error),
            Err(rollback) => Err(anyhow!(
                "router owner removal failed: {error:#}; rollback failed: {rollback:#}"
            )),
        };
    }
    Ok(())
}

pub fn restart(interface: &str) -> Result<()> {
    interface_name(interface)?;
    super::save(interface)?;
    let service = format!("wg-quick@{interface}.service");
    super::run(Command::new("systemctl").args(["restart", &service]))?;
    super::run(Command::new("systemctl").args(["restart", "gofro-relay.service"]))?;
    super::run(Command::new("systemctl").args(["is-active", "--quiet", &service]))?;
    super::run(Command::new("systemctl").args(["is-active", "--quiet", "gofro-relay.service"]))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_friend_shapes_without_legacy_routes() {
        assert!(matches!(
            classify_friend(&["10.202.0.2/32".into()]),
            PeerKind::Friend(_)
        ));
        assert!(matches!(
            classify_friend(&["10.203.1.0/24".into(), "10.202.0.2/32".into()]),
            PeerKind::Other
        ));
        assert!(matches!(
            classify_friend(&["10.202.0.2/32".into(), "10.203.1.0/24".into()]),
            PeerKind::Other
        ));
        assert!(matches!(
            classify_friend(&["10.202.0.2/32".into(), "10.202.0.3/32".into()]),
            PeerKind::Other
        ));
    }
}

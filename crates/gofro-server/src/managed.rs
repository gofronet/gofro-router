use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use wireguard_status::managed::{
    FriendPeer, ManagedServerStatus, validate_name, validate_peer_key,
};

const FRIENDS_DIR: &str = "/etc/gofro/friends";
const STORE_LIMIT: usize = 512 * 1024;

#[derive(Deserialize, Serialize)]
struct FriendStore {
    schema: u8,
    records: Vec<FriendRecord>,
}

#[derive(Deserialize, Serialize)]
struct FriendRecord {
    public_key: String,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    private_key: Option<String>,
}

fn interface_name(interface: &str) -> Result<()> {
    if interface.is_empty()
        || !interface
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b':'))
    {
        bail!("invalid WireGuard interface");
    }
    Ok(())
}

fn store_path(interface: &str) -> Result<PathBuf> {
    interface_name(interface)?;
    Ok(test_friends_dir().join(format!("{interface}.json")))
}

fn test_friends_dir() -> PathBuf {
    match (
        cfg!(debug_assertions)
            && std::env::var_os("GOFRO_TESTING").as_deref() == Some(std::ffi::OsStr::new("1")),
        std::env::var_os("GOFRO_TEST_FRIENDS_DIR"),
    ) {
        (true, Some(path)) => path.into(),
        _ => Path::new(FRIENDS_DIR).to_owned(),
    }
}

fn read_store(interface: &str) -> Result<FriendStore> {
    let path = store_path(interface)?;
    match fs::symlink_metadata(path.parent().context("invalid friend metadata path")?) {
        Ok(_) => validate_directory(path.parent().context("invalid friend metadata path")?)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("failed to inspect friend metadata directory"),
    }
    read_store_path(&path)
}

fn read_store_path(path: &Path) -> Result<FriendStore> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(FriendStore {
                schema: 1,
                records: Vec::new(),
            });
        }
        Err(error) => return Err(error).context("failed to inspect friend metadata"),
        Ok(_) => {}
    }
    validate_file(path)?;
    let file = File::open(path).context("failed to open friend metadata")?;
    let opened = file.metadata()?;
    let named = fs::symlink_metadata(path)?;
    if opened.dev() != named.dev() || opened.ino() != named.ino() {
        bail!("friend metadata changed while opening");
    }
    let mut data = Vec::new();
    file.take((STORE_LIMIT + 1) as u64).read_to_end(&mut data)?;
    if data.len() > STORE_LIMIT {
        bail!("friend metadata is too large");
    }
    let store: FriendStore = serde_json::from_slice(&data).context("invalid friend metadata")?;
    if store.schema != 1 {
        bail!("unsupported friend metadata schema");
    }
    let mut keys = BTreeSet::new();
    for record in &store.records {
        validate_peer_key(&record.public_key)?;
        validate_name(&record.name)?;
        if record
            .private_key
            .as_deref()
            .is_some_and(|key| validate_peer_key(key).is_err())
            || !keys.insert(&record.public_key)
        {
            bail!("invalid friend metadata");
        }
    }
    Ok(store)
}

fn write_store(interface: &str, store: &FriendStore) -> Result<()> {
    let path = store_path(interface)?;
    write_store_path(&path, store)
}

fn write_store_path(path: &Path, store: &FriendStore) -> Result<()> {
    let encoded = encode_store(store)?;
    let directory = path.parent().context("invalid friend metadata path")?;
    if !directory.exists() {
        fs::create_dir(directory).context("failed to create friend metadata directory")?;
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
    }
    validate_directory(directory)?;
    if fs::symlink_metadata(path).is_ok() {
        validate_file(path)?;
    }
    let filename = path
        .file_name()
        .context("invalid friend metadata filename")?
        .to_string_lossy();
    let temporary = directory.join(format!(".{filename}.{}.tmp", std::process::id()));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&temporary)
        .context("failed to create friend metadata")?;
    let result = (|| {
        file.write_all(&encoded)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        File::open(directory)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn encode_store(store: &FriendStore) -> Result<Vec<u8>> {
    let encoded = serde_json::to_vec(store).context("failed to encode friend metadata")?;
    if encoded.len() + 1 > STORE_LIMIT {
        bail!("friend metadata is too large");
    }
    Ok(encoded)
}

fn expected_uid() -> u32 {
    if cfg!(debug_assertions)
        && std::env::var_os("GOFRO_TESTING").as_deref() == Some(std::ffi::OsStr::new("1"))
    {
        return std::env::var("GOFRO_TEST_UID")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(u32::MAX);
    }
    0
}

fn validate_directory(path: &Path) -> Result<()> {
    let metadata =
        fs::symlink_metadata(path).context("failed to inspect friend metadata directory")?;
    if !metadata.file_type().is_dir()
        || metadata.file_type().is_symlink()
        || metadata.mode() & 0o777 != 0o700
        || metadata.uid() != expected_uid()
    {
        bail!("unsafe friend metadata directory");
    }
    Ok(())
}

fn validate_file(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path).context("failed to inspect friend metadata")?;
    if !metadata.file_type().is_file()
        || metadata.file_type().is_symlink()
        || metadata.mode() & 0o777 != 0o600
        || metadata.uid() != expected_uid()
        || metadata.len() > STORE_LIMIT as u64
    {
        bail!("unsafe friend metadata");
    }
    Ok(())
}

enum PeerKind {
    Friend(String),
    Owner,
    Other,
}

fn peers(interface: &str) -> Result<BTreeMap<String, PeerKind>> {
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
        .map(|(key, routes)| (key, classify_peer(&routes)))
        .collect())
}

fn classify_peer(routes: &[String]) -> PeerKind {
    if routes.iter().any(|route| route == super::CLIENT_SUBNET) {
        PeerKind::Owner
    } else if let [route] = routes {
        super::validate_tunnel_ip(route)
            .map(|ip| PeerKind::Friend(ip.to_string()))
            .unwrap_or(PeerKind::Other)
    } else {
        PeerKind::Other
    }
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

fn peer_shape(interface: &str, public_key: &str) -> Result<Option<String>> {
    validate_peer_key(public_key)?;
    let Some(peer) = peers(interface)?.remove(public_key) else {
        return Ok(None);
    };
    match peer {
        PeerKind::Friend(ip) => Ok(Some(ip)),
        PeerKind::Owner => bail!("router peer cannot be managed as a friend"),
        PeerKind::Other => bail!("peer is not a managed friend shape"),
    }
}

pub fn status(interface: &str) -> Result<ManagedServerStatus> {
    let store = read_store(interface)?;
    let active = peers(interface)?;
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
    let mut store = read_store(interface)?;
    let active = peers(interface)?;
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
    let mut store = read_store(interface)?;
    let active = peers(interface)?;
    let current = peer_shape(interface, public_key)?;
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
    let mut store = read_store(interface)?;
    let active = peer_shape(interface, public_key)?;
    let active = match active {
        Some(active) => active,
        None => {
            if store
                .records
                .iter()
                .any(|record| record.public_key == public_key)
            {
                return Ok(());
            }
            bail!("friend not found");
        }
    };
    let previous_store = serde_json::to_vec(&store)?;
    let previous_config = super::run(Command::new("wg").args(["showconf", interface]))?;
    if !store
        .records
        .iter()
        .any(|record| record.public_key == public_key)
    {
        store.records.push(FriendRecord {
            public_key: public_key.to_owned(),
            name: active,
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
            let previous: FriendStore = serde_json::from_slice(&previous_store)?;
            write_store(interface, &previous)
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
    let store = read_store(interface)?;
    peer_shape(interface, public_key)?.context("friend is not active")?;
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
    let ip = match peers(interface)?.remove(public_key) {
        Some(PeerKind::Friend(ip)) => ip,
        _ => bail!("friend is not active"),
    };
    let derived = super::run_with_input(Command::new("wg").arg("pubkey"), private_key)?;
    if derived.trim() != public_key {
        bail!("friend profile is unavailable");
    }
    Ok(super::format_profile(
        private_key,
        public_key,
        server_key.trim(),
        endpoint,
        &ip,
    ))
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
    fn classifies_all_allowed_ip_shapes() {
        assert!(matches!(
            classify_peer(&["10.202.0.2/32".into()]),
            PeerKind::Friend(_)
        ));
        assert!(matches!(
            classify_peer(&["10.203.1.0/24".into(), "10.202.0.2/32".into()]),
            PeerKind::Owner
        ));
        assert!(matches!(
            classify_peer(&["10.202.0.2/32".into(), "10.203.1.0/24".into()]),
            PeerKind::Owner
        ));
        assert!(matches!(
            classify_peer(&["10.202.0.2/32".into(), "10.202.0.3/32".into()]),
            PeerKind::Other
        ));
    }

    #[test]
    fn rejects_oversized_store_before_writing() {
        let records = (0..6_000)
            .map(|_| FriendRecord {
                public_key: "Aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa=".into(),
                name: "n".repeat(60),
                private_key: Some("Ppppppppppppppppppppppppppppppppppppppppppp=".into()),
            })
            .collect();
        assert!(encode_store(&FriendStore { schema: 1, records }).is_err());
    }
}

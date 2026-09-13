use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use wireguard_status::managed::{validate_name, validate_peer_key};

const FRIENDS_DIR: &str = "/etc/gofro/friends";
const STORE_LIMIT: usize = 512 * 1024;

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct FriendStore {
    pub(super) schema: u8,
    pub(super) records: Vec<FriendRecord>,
    pub(super) owners: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyFriendStore {
    schema: u8,
    records: Vec<FriendRecord>,
}

#[derive(Clone, Deserialize, Serialize)]
pub(super) struct FriendRecord {
    pub(super) public_key: String,
    pub(super) name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) private_key: Option<String>,
}

pub(super) fn interface_name(interface: &str) -> Result<()> {
    if interface.is_empty()
        || !interface
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b':'))
    {
        bail!("invalid WireGuard interface");
    }
    Ok(())
}

pub(super) fn store_path(interface: &str) -> Result<PathBuf> {
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

pub(super) fn read_store(interface: &str) -> Result<FriendStore> {
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
                owners: Vec::new(),
            });
        }
        Err(error) => return Err(error).context("failed to inspect friend metadata"),
        Ok(_) => {}
    }
    let inspected = validate_file(path)?;
    let file = File::open(path).context("failed to open friend metadata")?;
    let opened = file.metadata()?;
    let named = fs::symlink_metadata(path)?;
    if opened.dev() != named.dev()
        || opened.ino() != named.ino()
        || opened.dev() != inspected.dev()
        || opened.ino() != inspected.ino()
        || named.file_type().is_symlink()
    {
        bail!("friend metadata changed while opening");
    }
    let mut data = Vec::new();
    file.take((STORE_LIMIT + 1) as u64).read_to_end(&mut data)?;
    if data.len() > STORE_LIMIT {
        bail!("friend metadata is too large");
    }
    let schema: serde_json::Value =
        serde_json::from_slice(&data).map_err(|_| anyhow!("invalid friend metadata"))?;
    let schema = schema
        .get("schema")
        .and_then(serde_json::Value::as_u64)
        .context("invalid friend metadata")?;
    let store = match schema {
        1 => {
            let legacy: LegacyFriendStore =
                serde_json::from_slice(&data).map_err(|_| anyhow!("invalid friend metadata"))?;
            FriendStore {
                schema: legacy.schema,
                records: legacy.records,
                owners: Vec::new(),
            }
        }
        2 => serde_json::from_slice(&data).map_err(|_| anyhow!("invalid friend metadata"))?,
        _ => bail!("unsupported friend metadata schema"),
    };
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
    if store.schema == 2 {
        for owner in &store.owners {
            validate_peer_key(owner)?;
            if !keys.insert(owner) {
                bail!("owner metadata overlaps friend metadata");
            }
        }
    }
    Ok(store)
}

pub(super) fn write_store(interface: &str, store: &FriendStore) -> Result<()> {
    let path = store_path(interface)?;
    write_store_path(&path, store)
}

fn write_store_path(path: &Path, store: &FriendStore) -> Result<()> {
    let encoded = encode_store(store)?;
    let directory = path.parent().context("invalid friend metadata path")?;
    if !directory.exists() {
        fs::DirBuilder::new()
            .mode(0o700)
            .create(directory)
            .context("failed to create friend metadata directory")?;
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

pub(super) fn expected_uid() -> u32 {
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

fn validate_file(path: &Path) -> Result<fs::Metadata> {
    let metadata = fs::symlink_metadata(path).context("failed to inspect friend metadata")?;
    if !metadata.file_type().is_file()
        || metadata.file_type().is_symlink()
        || metadata.mode() & 0o777 != 0o600
        || metadata.uid() != expected_uid()
        || metadata.nlink() != 1
        || metadata.len() > STORE_LIMIT as u64
    {
        bail!("unsafe friend metadata");
    }
    Ok(metadata)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_oversized_store_before_writing() {
        let records = (0..6_000)
            .map(|_| FriendRecord {
                public_key: "Aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa=".into(),
                name: "n".repeat(60),
                private_key: Some("Ppppppppppppppppppppppppppppppppppppppppppp=".into()),
            })
            .collect();
        assert!(
            encode_store(&FriendStore {
                schema: 2,
                records,
                owners: Vec::new()
            })
            .is_err()
        );
    }
}

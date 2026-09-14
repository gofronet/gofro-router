use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    net::IpAddr,
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::Path,
};

use anyhow::{Context, Result, bail};

use super::temporary_path;
use crate::model::ControllerConfig;

pub(crate) fn parse_host_key(value: &str) -> Result<()> {
    let mut parts = value.split_whitespace();
    let (Some(kind), Some(key), None) = (parts.next(), parts.next(), parts.next()) else {
        bail!("invalid ed25519 host key");
    };
    if kind != "ssh-ed25519"
        || key.len() != 68
        || !key.starts_with("AAAAC3NzaC1lZDI1NTE5AAAAI")
        || !key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'='))
    {
        bail!("invalid ed25519 host key");
    }
    Ok(())
}

pub(super) fn management_directory(dir: &Path) -> Result<()> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    let metadata = fs::symlink_metadata(dir)?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || (!cfg!(test) && metadata.uid() != 0)
    {
        bail!("unsafe management directory");
    }
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    // Sync ancestors even on retries: a previous mkdir may have succeeded before
    // its parent sync failed. Recursive creation must not lose the pin directory.
    for ancestor in dir.ancestors() {
        let path = if ancestor.as_os_str().is_empty() {
            Path::new(".")
        } else {
            ancestor
        };
        let step = if ancestor == dir {
            "management-directory-sync"
        } else {
            "management-parent-sync"
        };
        pin_io(step, || File::open(path)?.sync_all())
            .context("failed to persist management directory")?;
    }
    Ok(())
}

fn pin_io<T>(
    _step: &'static str,
    operation: impl FnOnce() -> std::io::Result<T>,
) -> std::io::Result<T> {
    #[cfg(test)]
    tests::PIN_IO.with_borrow_mut(|state| {
        if let Some((failure, calls)) = state {
            calls.push(_step);
            if *failure == _step {
                return Err(std::io::Error::from(std::io::ErrorKind::StorageFull));
            }
        }
        Ok(())
    })?;
    operation()
}

// Call with the configuration lock held. Deletion does not take managed_operations,
// so both enrollment and deletion serialize pin migration through that same lock.
pub(crate) fn preserve_host_pins(dir: &Path, config: &ControllerConfig) -> Result<()> {
    for managed in config
        .servers
        .iter()
        .filter_map(|server| server.management.as_ref())
    {
        pin_host(dir, &managed.host, managed.port, &managed.host_key)?;
    }
    Ok(())
}

pub(super) fn pin_host(dir: &Path, host: &str, port: u16, host_key: &str) -> Result<()> {
    let address: IpAddr = host.parse().context("invalid SSH host address")?;
    if address.to_string() != host || port == 0 {
        bail!("invalid SSH host address or port");
    }
    parse_host_key(host_key)?;
    management_directory(dir)?;
    let path = dir.join(format!("host-{host}-{port}.pub"));
    match fs::symlink_metadata(&path) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let temporary = temporary_path(dir, "host-pin");
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&temporary)?;
            let result = (|| {
                pin_io("pin-write", || file.write_all(host_key.as_bytes()))?;
                pin_io("pin-temp-sync", || file.sync_all())?;
                // hard_link publishes atomically without ever replacing another pin.
                match pin_io("pin-publish", || fs::hard_link(&temporary, &path)) {
                    Ok(()) => Ok(true),
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
                    Err(error) => Err(error),
                }
            })();
            #[cfg(test)]
            if result.is_ok()
                && let Some(recover) = tests::AFTER_PIN_PUBLISH.take()
            {
                recover();
            }
            let cleanup = fs::remove_file(&temporary);
            let published = result.context("failed to publish SSH host pin")?;
            match cleanup {
                // Another publisher can recover this alias after our hard_link.
                Err(error) if published && error.kind() == std::io::ErrorKind::NotFound => {}
                result => result.context("failed to remove temporary SSH host pin")?,
            }
        }
        Err(error) => return Err(error).context("failed to persist SSH host pin"),
    }
    let metadata = fs::symlink_metadata(&path)?;
    if !safe_pin_metadata(&metadata) {
        bail!("unsafe stored SSH host pin");
    }
    let mut file = File::open(&path)?;
    let opened = file.metadata()?;
    if opened.dev() != metadata.dev() || opened.ino() != metadata.ino() {
        bail!("stored SSH host pin changed while opening");
    }
    let mut stored = String::new();
    Read::by_ref(&mut file)
        .take(129)
        .read_to_string(&mut stored)?;
    parse_host_key(&stored).context("invalid stored SSH host pin; refusing to replace it")?;
    if stored != host_key {
        bail!(
            "SSH host key changed. Connection refused; the saved key was not replaced. Verify the VPS identity through a trusted console before resetting its pin."
        );
    }
    if metadata.nlink() > 1 {
        // Recover a crash between hard_link and unlink, never other pin names or
        // external links. The protected directory and exact inode fence deletion.
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let name = entry.file_name();
            let owned_name = name
                .to_str()
                .and_then(|name| name.strip_prefix(".host-pin."))
                .and_then(|suffix| suffix.split_once('.'))
                .is_some_and(|(pid, counter)| {
                    [pid, counter].iter().all(|part| {
                        !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit())
                    })
                });
            if !owned_name {
                continue;
            }
            let alias = entry.path();
            let alias_metadata = match fs::symlink_metadata(&alias) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error).context("failed to inspect SSH host pin alias"),
            };
            if safe_pin_metadata(&alias_metadata)
                && alias_metadata.uid() == metadata.uid()
                && alias_metadata.dev() == metadata.dev()
                && alias_metadata.ino() == metadata.ino()
            {
                match fs::remove_file(&alias) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => {
                        return Err(error).context("failed to recover SSH host pin alias");
                    }
                }
            }
        }
    }
    let recovered = fs::symlink_metadata(&path)?;
    if !safe_pin_metadata(&recovered)
        || recovered.nlink() != 1
        || recovered.dev() != metadata.dev()
        || recovered.ino() != metadata.ino()
    {
        bail!("unsafe stored SSH host pin");
    }
    // An earlier attempt can have published a valid pin but failed to sync it.
    pin_io("pin-file-sync", || file.sync_all()).context("failed to sync SSH host pin")?;
    pin_io("pin-directory-sync", || File::open(dir)?.sync_all())
        .context("failed to persist SSH host pin directory entry")?;
    Ok(())
}

fn safe_pin_metadata(metadata: &fs::Metadata) -> bool {
    metadata.is_file()
        && !metadata.file_type().is_symlink()
        && metadata.mode() & 0o7777 == 0o600
        && metadata.len() <= 128
        && (cfg!(test) || metadata.uid() == 0)
}

pub(super) fn reset_host_pin(dir: &Path, host: &str, port: u16) -> Result<()> {
    management_directory(dir)?;
    let path = dir.join(format!("host-{host}-{port}.pub"));
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).context("failed to inspect SSH host pin"),
    };
    if !safe_pin_metadata(&metadata) || metadata.nlink() != 1 {
        bail!("unsafe stored SSH host pin");
    }
    let mut file = File::open(&path)?;
    let opened = file.metadata()?;
    if opened.dev() != metadata.dev() || opened.ino() != metadata.ino() {
        bail!("stored SSH host pin changed while opening");
    }
    let mut stored = String::new();
    Read::by_ref(&mut file)
        .take(129)
        .read_to_string(&mut stored)?;
    parse_host_key(&stored).context("invalid stored SSH host pin")?;
    let current = fs::symlink_metadata(&path)?;
    if current.dev() != metadata.dev() || current.ino() != metadata.ino() {
        bail!("stored SSH host pin changed before removal");
    }
    fs::remove_file(path).context("failed to remove SSH host pin")?;
    pin_io("pin-directory-sync", || File::open(dir)?.sync_all())
        .context("failed to persist SSH host pin removal")
}

#[cfg(test)]
mod tests {
    use super::super::{
        bootstrap,
        tests::{Fixture, KEY},
    };
    use super::*;
    use crate::{controller, model::BootstrapStage};
    use std::path::PathBuf;

    thread_local! {
        pub(super) static PIN_IO: std::cell::RefCell<Option<(&'static str, Vec<&'static str>)>> = const { std::cell::RefCell::new(None) };
        pub(super) static AFTER_PIN_PUBLISH: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
    }

    fn attempt_pin(
        dir: &Path,
        key: &str,
        failure: &'static str,
    ) -> (Result<()>, Vec<&'static str>) {
        PIN_IO.set(Some((failure, Vec::new())));
        let result = pin_host(dir, "1.1.1.1", 22, key);
        let (_, calls) = PIN_IO.take().unwrap();
        (result, calls)
    }

    fn interrupted_pin(fixture: &Fixture) -> (PathBuf, PathBuf) {
        let dir = &fixture.state.management_dir;
        management_directory(dir).unwrap();
        let temporary = temporary_path(dir, "host-pin");
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
            .unwrap();
        file.write_all(KEY.as_bytes()).unwrap();
        file.sync_all().unwrap();
        let pin = dir.join("host-1.1.1.1-22.pub");
        fs::hard_link(&temporary, &pin).unwrap();
        // Simulate process death at the exact publication/unlink boundary.
        drop(file);
        assert_eq!(fs::metadata(&pin).unwrap().nlink(), 2);
        (pin, temporary)
    }

    #[test]
    fn interrupted_pin_publication_recovers_only_matching_owned_aliases() {
        let fixture = Fixture::new();
        let dir = &fixture.state.management_dir;
        let (pin, temporary) = interrupted_pin(&fixture);
        let before = fs::metadata(&pin).unwrap();
        let unrelated = temporary_path(dir, "host-pin");
        let mut other = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&unrelated)
            .unwrap();
        other.write_all(KEY.as_bytes()).unwrap();
        let symlink = temporary_path(dir, "host-pin");
        std::os::unix::fs::symlink(&pin, &symlink).unwrap();
        let other_pin = dir.join("host-8.8.8.8-22.pub");
        fs::write(&other_pin, "another pin").unwrap();
        let (result, calls) = attempt_pin(dir, KEY, "");
        result.unwrap();
        assert!(!temporary.exists());
        assert_eq!(fs::read_to_string(&unrelated).unwrap(), KEY);
        assert!(
            fs::symlink_metadata(&symlink)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read_to_string(&other_pin).unwrap(), "another pin");
        let after = fs::metadata(&pin).unwrap();
        assert_eq!(
            (after.dev(), after.ino(), after.nlink()),
            (before.dev(), before.ino(), 1)
        );
        assert_eq!(fs::read_to_string(&pin).unwrap(), KEY);
        assert!(calls.contains(&"pin-file-sync"));
        assert!(calls.contains(&"pin-directory-sync"));
        assert!(!calls.contains(&"pin-publish"));
    }

    #[test]
    fn pin_recovery_preserves_and_rejects_external_or_unrelated_hardlinks() {
        for alias in [
            ".host-pin.external",
            "host-8.8.8.8-22.pub",
            "../.host-pin.123.456",
        ] {
            let fixture = Fixture::new();
            let dir = &fixture.state.management_dir;
            let (pin, temporary) = interrupted_pin(&fixture);
            let unrelated = dir.join(alias);
            fs::hard_link(&pin, &unrelated).unwrap();
            assert!(
                attempt_pin(dir, KEY, "")
                    .0
                    .unwrap_err()
                    .to_string()
                    .contains("unsafe stored SSH host pin")
            );
            assert!(!temporary.exists());
            assert_eq!(fs::metadata(&pin).unwrap().nlink(), 2);
            assert_eq!(fs::read_to_string(&unrelated).unwrap(), KEY);
            assert_eq!(fs::read_to_string(&pin).unwrap(), KEY);
        }
    }

    #[test]
    fn interrupted_pin_still_refuses_changed_key_before_password_connection() {
        let fixture = Fixture::new();
        let (pin, temporary) = interrupted_pin(&fixture);
        let mut server = fixture.server();
        server.management.as_mut().unwrap().host_key = KEY.replace("LI7Mdq", "LI7Mdr");
        controller::add_server(&fixture.state, server).unwrap();
        let mut stages = Vec::new();
        let error = bootstrap(
            &fixture.state,
            "VPS".into(),
            "1.1.1.1".into(),
            22,
            "unused-password".into(),
            &mut |stage| stages.push(stage),
        )
        .unwrap_err();
        assert!(error.to_string().contains("SSH host key changed"));
        assert_eq!(stages, [BootstrapStage::Waiting, BootstrapStage::HostKey]);
        assert!(temporary.exists());
        assert_eq!(fs::read_to_string(&pin).unwrap(), KEY);
    }

    #[test]
    fn publisher_accepts_temporary_alias_already_removed_by_recovery() {
        let fixture = Fixture::new();
        let dir = fixture.state.management_dir.clone();
        AFTER_PIN_PUBLISH.set(Some(Box::new(move || {
            assert_eq!(
                fs::metadata(dir.join("host-1.1.1.1-22.pub"))
                    .unwrap()
                    .nlink(),
                2
            );
            pin_host(&dir, "1.1.1.1", 22, KEY).unwrap();
            assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        })));
        pin_host(&fixture.state.management_dir, "1.1.1.1", 22, KEY).unwrap();
        assert_eq!(
            fs::read_to_string(fixture.state.management_dir.join("host-1.1.1.1-22.pub")).unwrap(),
            KEY
        );
    }

    #[test]
    fn tofu_pins_first_key_and_refuses_changed_or_unsafe_pins() {
        let fixture = Fixture::new();
        let dir = &fixture.state.management_dir;
        pin_host(dir, "1.1.1.1", 22, KEY).unwrap();
        pin_host(dir, "1.1.1.1", 22, KEY).unwrap();
        let changed = KEY.replace("LI7Mdq", "LI7Mdr");
        assert!(
            pin_host(dir, "1.1.1.1", 22, &changed)
                .unwrap_err()
                .to_string()
                .contains("key changed")
        );
        pin_host(dir, "1.1.1.1", 2222, &changed).unwrap();
        let pin = dir.join("host-1.1.1.1-22.pub");
        assert_eq!(fs::read_to_string(&pin).unwrap(), KEY);
        assert_eq!(fs::metadata(dir).unwrap().mode() & 0o777, 0o700);
        assert_eq!(fs::metadata(&pin).unwrap().mode() & 0o777, 0o600);
        fs::set_permissions(&pin, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(pin_host(dir, "1.1.1.1", 22, KEY).is_err());
        fs::remove_file(&pin).unwrap();
        std::os::unix::fs::symlink(fixture.root.join("missing"), &pin).unwrap();
        assert!(pin_host(dir, "1.1.1.1", 22, KEY).is_err());
        assert!(!fixture.root.join("missing").exists());
    }

    #[test]
    fn reset_removes_only_an_unused_safe_pin() {
        let fixture = Fixture::new();
        let dir = &fixture.state.management_dir;
        pin_host(dir, "1.1.1.1", 22, KEY).unwrap();
        let alias = dir.join("unexpected-link");
        fs::hard_link(dir.join("host-1.1.1.1-22.pub"), &alias).unwrap();
        assert!(super::super::reset_host_pin(&fixture.state, "1.1.1.1", 22).is_err());
        fs::remove_file(alias).unwrap();
        super::super::reset_host_pin(&fixture.state, "1.1.1.1", 22).unwrap();
        assert!(!dir.join("host-1.1.1.1-22.pub").exists());
        super::super::reset_host_pin(&fixture.state, "1.1.1.1", 22).unwrap();

        let server = fixture.server();
        controller::add_server(&fixture.state, server).unwrap();
        assert!(
            super::super::reset_host_pin(&fixture.state, "1.1.1.1", 22)
                .unwrap_err()
                .to_string()
                .contains("still used")
        );
        assert_eq!(
            fs::read_to_string(dir.join("host-1.1.1.1-22.pub")).unwrap(),
            KEY
        );
    }

    #[test]
    fn pin_write_and_prepublication_sync_failures_leave_no_final_pin() {
        for failure in ["pin-write", "pin-temp-sync", "pin-publish"] {
            let fixture = Fixture::new();
            let dir = &fixture.state.management_dir;
            assert!(attempt_pin(dir, KEY, failure).0.is_err(), "{failure}");
            assert!(!dir.join("host-1.1.1.1-22.pub").exists(), "{failure}");
            assert_eq!(
                fs::read_dir(dir).unwrap().count(),
                0,
                "temporary pin leaked"
            );
            attempt_pin(dir, KEY, "").0.unwrap();
            assert_eq!(
                fs::read_to_string(dir.join("host-1.1.1.1-22.pub")).unwrap(),
                KEY
            );
        }
    }

    #[test]
    fn published_pins_require_successful_file_and_directory_sync_on_every_retry() {
        for failure in ["pin-file-sync", "pin-directory-sync"] {
            let fixture = Fixture::new();
            let dir = &fixture.state.management_dir;
            assert!(attempt_pin(dir, KEY, failure).0.is_err());
            let path = dir.join("host-1.1.1.1-22.pub");
            assert_eq!(fs::read_to_string(&path).unwrap(), KEY);
            assert_eq!(fs::metadata(&path).unwrap().nlink(), 1);
            for retry_failure in ["pin-file-sync", "pin-directory-sync"] {
                let (result, calls) = attempt_pin(dir, KEY, retry_failure);
                assert!(result.is_err(), "matching pin bypassed {retry_failure}");
                assert!(calls.contains(&retry_failure));
                assert!(!calls.contains(&"pin-publish"));
            }
            assert!(
                attempt_pin(dir, &KEY.replace("LI7Mdq", "LI7Mdr"), "")
                    .0
                    .is_err()
            );
            assert_eq!(fs::read_to_string(&path).unwrap(), KEY);
            let (result, calls) = attempt_pin(dir, KEY, "");
            result.unwrap();
            assert!(calls.contains(&"pin-file-sync"));
            assert!(calls.contains(&"pin-directory-sync"));
            fs::write(&path, "broken").unwrap();
            assert!(attempt_pin(dir, KEY, "").0.is_err());
            assert_eq!(fs::read_to_string(&path).unwrap(), "broken");
        }
    }

    #[test]
    fn management_directory_parent_durability_is_retried_before_publishing() {
        let fixture = Fixture::new();
        let dir = fixture.root.join("new-parent/management");
        assert!(attempt_pin(&dir, KEY, "management-parent-sync").0.is_err());
        assert!(dir.is_dir());
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 0);
        assert!(attempt_pin(&dir, KEY, "management-parent-sync").0.is_err());
        let (result, calls) = attempt_pin(&dir, KEY, "");
        result.unwrap();
        let publish = calls
            .iter()
            .position(|step| *step == "pin-publish")
            .unwrap();
        assert_eq!(
            calls[..publish]
                .iter()
                .filter(|step| **step == "management-parent-sync")
                .count(),
            dir.ancestors().count() - 1
        );
    }

    #[test]
    fn deleting_legacy_profile_preserves_pin_and_management_key() {
        let fixture = Fixture::new();
        let server = fixture.server();
        controller::add_server(&fixture.state, server.clone()).unwrap();
        assert!(!fixture.state.management_dir.exists());
        management_directory(&fixture.state.management_dir).unwrap();
        fs::write(
            fixture.state.management_dir.join("id_ed25519"),
            "existing-management-key",
        )
        .unwrap();
        controller::delete_server(&fixture.state, &server.public_key).unwrap();
        assert!(fixture.state.config.lock().unwrap().servers.is_empty());
        assert_eq!(
            fs::read_to_string(fixture.state.management_dir.join("host-1.1.1.1-22.pub")).unwrap(),
            KEY
        );
        assert_eq!(
            fs::read_to_string(fixture.state.management_dir.join("id_ed25519")).unwrap(),
            "existing-management-key"
        );
        assert!(
            pin_host(
                &fixture.state.management_dir,
                "1.1.1.1",
                22,
                &KEY.replace("LI7Mdq", "LI7Mdr")
            )
            .is_err()
        );
    }
}

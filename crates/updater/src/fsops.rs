use std::{
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::Write,
    os::unix::fs::{OpenOptionsExt, symlink},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow};

fn temporary_path(path: &Path) -> Result<PathBuf> {
    let name = path
        .file_name()
        .ok_or_else(|| anyhow!("{} has no file name", path.display()))?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before Unix epoch")?
        .as_nanos();
    let mut temporary = OsString::from(name);
    temporary.push(format!(".tmp.{}.{nonce}", std::process::id()));
    Ok(path.with_file_name(temporary))
}

pub(crate) fn sync_parent(path: &Path) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("{} has no parent", path.display()))?;
    File::open(parent)
        .with_context(|| format!("failed to open {}", parent.display()))?
        .sync_all()
        .with_context(|| format!("failed to sync {}", parent.display()))
}

pub(crate) fn sync_file(path: &Path) -> Result<()> {
    File::open(path)
        .with_context(|| format!("failed to open {}", path.display()))?
        .sync_all()
        .with_context(|| format!("failed to sync {}", path.display()))
}

pub(crate) fn atomic_write(path: &Path, bytes: &[u8], mode: u32) -> Result<()> {
    let temporary = temporary_path(path)?;
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(&temporary)
            .with_context(|| format!("failed to create {}", temporary.display()))?;
        file.write_all(bytes)
            .with_context(|| format!("failed to write {}", temporary.display()))?;
        file.sync_all()
            .with_context(|| format!("failed to sync {}", temporary.display()))?;
        fs::rename(&temporary, path).with_context(|| {
            format!(
                "failed to rename {} to {}",
                temporary.display(),
                path.display()
            )
        })?;
        sync_parent(path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

pub(crate) fn atomic_symlink(target: &Path, link: &Path) -> Result<()> {
    let temporary = temporary_path(link)?;
    let result = (|| {
        symlink(target, &temporary).with_context(|| {
            format!(
                "failed to create symlink {} -> {}",
                temporary.display(),
                target.display()
            )
        })?;
        fs::rename(&temporary, link).with_context(|| {
            format!(
                "failed to rename {} to {}",
                temporary.display(),
                link.display()
            )
        })?;
        sync_parent(link)
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

pub(crate) fn atomic_copy(source: &Path, destination: &Path) -> Result<()> {
    let temporary = temporary_path(destination)?;
    let result = (|| {
        fs::copy(source, &temporary).with_context(|| {
            format!(
                "failed to copy {} to {}",
                source.display(),
                temporary.display()
            )
        })?;
        File::open(&temporary)
            .with_context(|| format!("failed to open {}", temporary.display()))?
            .sync_all()
            .with_context(|| format!("failed to sync {}", temporary.display()))?;
        fs::rename(&temporary, destination).with_context(|| {
            format!(
                "failed to rename {} to {}",
                temporary.display(),
                destination.display()
            )
        })?;
        sync_parent(destination)
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

pub(crate) fn remove_file(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => sync_parent(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("failed to remove {}", path.display())),
    }
}

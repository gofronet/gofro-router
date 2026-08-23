use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, ensure};

use crate::{fsops, manifest, paths, process};

const BINARY_LIMIT: u64 = 32 * 1024 * 1024;
const SCRIPT_LIMIT: u64 = 65_536;

pub(crate) struct Staging {
    root: PathBuf,
}

impl Staging {
    pub(crate) fn new() -> Result<Self> {
        fs::create_dir_all(paths::RELEASES)
            .with_context(|| format!("failed to create {}", paths::RELEASES))?;
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .context("system clock is before Unix epoch")?
            .as_nanos();
        let root =
            Path::new(paths::RELEASES).join(format!(".update-{}-{nonce}", std::process::id()));
        fs::create_dir(&root)
            .with_context(|| format!("failed to create staging directory {}", root.display()))?;
        Ok(Self { root })
    }

    pub(crate) fn file(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }

    pub(crate) fn install(
        &self,
        signed_manifest: &[u8],
        manifest: &manifest::ValidManifest,
    ) -> Result<PathBuf> {
        let extracted = self.root.join("bundle");
        fs::create_dir(&extracted)
            .with_context(|| format!("failed to create {}", extracted.display()))?;
        process::extract(&self.file(&manifest.archive), &extracted)?;
        fsops::atomic_write(
            &extracted.join(manifest::MANIFEST_NAME),
            signed_manifest,
            0o644,
        )?;
        validate_bundle(&extracted, signed_manifest, &manifest.version_text)?;
        sync_bundle(&extracted)?;

        let destination = paths::release(&manifest.version);
        if destination.exists() {
            ensure!(
                fs::symlink_metadata(&destination)
                    .with_context(|| format!("failed to inspect {}", destination.display()))?
                    .file_type()
                    .is_dir(),
                "existing release {} is not a directory",
                destination.display()
            );
            validate_bundle(&destination, signed_manifest, &manifest.version_text)
                .context("existing immutable release did not match signed bundle")?;
            return Ok(destination);
        }
        fs::rename(&extracted, &destination).with_context(|| {
            format!(
                "failed to install release {} as {}",
                extracted.display(),
                destination.display()
            )
        })?;
        fsops::sync_parent(&destination)?;
        Ok(destination)
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

pub(crate) fn read_limited(path: &Path, maximum: u64) -> Result<Vec<u8>> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("failed to inspect {}", path.display()))?;
    ensure!(
        metadata.file_type().is_file(),
        "{} is not a file",
        path.display()
    );
    ensure!(
        metadata.len() <= maximum,
        "{} exceeds the {maximum}-byte limit",
        path.display()
    );
    fs::read(path).with_context(|| format!("failed to read {}", path.display()))
}

fn validate_bundle(root: &Path, signed_manifest: &[u8], version: &str) -> Result<()> {
    let allowed = [
        "pi-agent",
        "wg-relay",
        "gofro-updater",
        "migrate.sh",
        manifest::MANIFEST_NAME,
    ];
    let mut count = 0;
    for entry in fs::read_dir(root).with_context(|| format!("failed to list {}", root.display()))? {
        let entry = entry.with_context(|| format!("failed to read entry in {}", root.display()))?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| anyhow::anyhow!("bundle entry name is not valid UTF-8"))?;
        ensure!(
            allowed.contains(&name.as_str()),
            "unexpected bundle entry {name}"
        );
        count += 1;
    }
    ensure!(
        count == allowed.len(),
        "bundle does not contain exactly the required files"
    );
    for name in ["pi-agent", "wg-relay", "gofro-updater"] {
        require_executable(&root.join(name), BINARY_LIMIT)?;
    }
    require_executable(&root.join("migrate.sh"), SCRIPT_LIMIT)?;
    let internal = root.join(manifest::MANIFEST_NAME);
    let metadata = fs::symlink_metadata(&internal)
        .with_context(|| format!("failed to inspect {}", internal.display()))?;
    ensure!(
        metadata.file_type().is_file(),
        "{} is not a regular file",
        internal.display()
    );
    ensure!(
        fs::read(&internal).with_context(|| format!("failed to read {}", internal.display()))?
            == signed_manifest,
        "bundle manifest differs from signed external manifest"
    );
    process::binary_check(&root.join("pi-agent"), "--version", version)?;
    process::binary_check(&root.join("wg-relay"), "--version", version)?;
    process::binary_check(&root.join("gofro-updater"), "--self-check", version)?;
    Ok(())
}

fn require_executable(path: &Path, maximum: u64) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("failed to inspect required file {}", path.display()))?;
    ensure!(
        metadata.file_type().is_file(),
        "{} is not a regular file",
        path.display()
    );
    ensure!(
        metadata.permissions().mode() & 0o111 != 0,
        "{} is not executable",
        path.display()
    );
    ensure!(
        metadata.len() > 0 && metadata.len() <= maximum,
        "{} exceeds its size limit",
        path.display()
    );
    Ok(())
}

fn sync_bundle(root: &Path) -> Result<()> {
    for name in [
        "pi-agent",
        "wg-relay",
        "gofro-updater",
        "migrate.sh",
        manifest::MANIFEST_NAME,
    ] {
        fsops::sync_file(&root.join(name))?;
    }
    fsops::sync_file(root)
}

pub(crate) fn install_updater(release: &Path, version: &str) -> Result<()> {
    let source = release.join("gofro-updater");
    require_executable(&source, BINARY_LIMIT)?;
    process::binary_check(&source, "--self-check", version)?;
    let destination = Path::new(paths::UPDATER);
    let source_bytes =
        fs::read(&source).with_context(|| format!("failed to read {}", source.display()))?;
    if require_executable(destination, BINARY_LIMIT).is_ok()
        && fs::read(destination).is_ok_and(|bytes| bytes == source_bytes)
    {
        return Ok(());
    }
    fsops::atomic_copy(&source, destination).context("failed to atomically install updater binary")
}

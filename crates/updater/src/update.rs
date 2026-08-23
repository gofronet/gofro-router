use std::path::Path;

use anyhow::{Context, Result};

use crate::{
    bundle::{self, Staging},
    manifest, paths, process,
    progress::{self, UpdateProgress, UpdateState},
    state::{self, Pending},
    status, transaction,
};

const RELEASE_JSON_LIMIT: u64 = 1_048_576;

struct Discovery {
    signed: manifest::ValidManifest,
    manifest_bytes: Vec<u8>,
    archive_url: String,
}

pub(crate) fn check() -> Result<()> {
    let current = state::read_version()?;
    let staging = Staging::new()?;
    progress::write(&UpdateProgress::new(UpdateState::Checking, None))?;
    let discovery = discover(&staging)?;
    let state = if discovery.signed.version > current {
        UpdateState::Available
    } else {
        UpdateState::Idle
    };
    progress::write(&UpdateProgress::new(state, Some(&discovery.signed.version)))?;
    println!(
        "Checked for updates (installed {current}, latest {})",
        discovery.signed.version
    );
    Ok(())
}

pub(crate) fn run(force: bool) -> Result<()> {
    let current = state::read_version()?;
    let staging = Staging::new()?;
    progress::write(&UpdateProgress::new(UpdateState::Checking, None))?;
    let discovery = discover(&staging)?;
    let signed = discovery.signed;
    if !manifest::should_install(&current, &signed.version, force) {
        progress::write(&UpdateProgress::new(
            UpdateState::Idle,
            Some(&signed.version),
        ))?;
        println!(
            "No stable update available (installed {current}, latest {})",
            signed.version
        );
        return Ok(());
    }
    progress::write(&UpdateProgress::new(
        UpdateState::Available,
        Some(&signed.version),
    ))?;
    progress::write(&UpdateProgress::new(
        UpdateState::Downloading,
        Some(&signed.version),
    ))?;

    let archive_path = staging.file(&signed.archive);
    process::curl_download(
        &discovery.archive_url,
        &archive_path,
        manifest::ARCHIVE_LIMIT,
    )?;
    process::verify_sha256(&archive_path, &signed.sha256)?;
    let release_path = staging
        .install(&discovery.manifest_bytes, &signed)
        .context("failed to install verified release bundle")?;

    let baseline =
        status::baseline(&current).context("failed to establish pre-update API health baseline")?;
    let (previous_target, resolved_previous) = state::current_target()?;
    ensure_release_directory(&resolved_previous)?;
    anyhow::ensure!(
        resolved_previous == paths::release(&current),
        "current release does not match installed version marker"
    );
    let pending = Pending::new(
        &current,
        &signed.version,
        previous_target,
        release_path,
        baseline,
    )?;
    transaction::apply(pending)
}

fn discover(staging: &Staging) -> Result<Discovery> {
    let release_json = staging.file("release.json");
    process::curl_download(paths::RELEASE_API, &release_json, RELEASE_JSON_LIMIT)?;
    let release =
        manifest::parse_release(&bundle::read_limited(&release_json, RELEASE_JSON_LIMIT)?)?;
    let urls = release.metadata_urls()?;

    let manifest_path = staging.file(manifest::MANIFEST_NAME);
    let signature_path = staging.file(manifest::SIGNATURE_NAME);
    process::curl_download(&urls.manifest, &manifest_path, manifest::MANIFEST_LIMIT)?;
    process::curl_download(&urls.signature, &signature_path, manifest::SIGNATURE_LIMIT)?;
    bundle::read_limited(&signature_path, manifest::SIGNATURE_LIMIT)?;
    process::verify_signature(&manifest_path, &signature_path)?;

    let manifest_bytes = bundle::read_limited(&manifest_path, manifest::MANIFEST_LIMIT)?;
    let signed = manifest::parse_manifest(&manifest_bytes)?;
    release.validate_tag(&signed)?;
    let archive_url = release.archive_url(&signed.archive)?;
    Ok(Discovery {
        signed,
        manifest_bytes,
        archive_url,
    })
}

fn ensure_release_directory(path: &Path) -> Result<()> {
    let metadata = std::fs::symlink_metadata(path)
        .with_context(|| format!("failed to inspect current release {}", path.display()))?;
    anyhow::ensure!(
        metadata.file_type().is_dir(),
        "current release {} is not a directory",
        path.display()
    );
    Ok(())
}

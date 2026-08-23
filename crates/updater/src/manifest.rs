use anyhow::{Context, Result, ensure};
use semver::Version;
use serde::Deserialize;

pub(crate) const MANIFEST_NAME: &str = "gofro-router-manifest.json";
pub(crate) const SIGNATURE_NAME: &str = "gofro-router-manifest.json.sig";
pub(crate) const ARCHIVE_NAME: &str = "gofro-router-aarch64-unknown-linux-gnu.tar.gz";
pub(crate) const MANIFEST_LIMIT: u64 = 65_536;
pub(crate) const SIGNATURE_LIMIT: u64 = 4_096;
pub(crate) const ARCHIVE_LIMIT: u64 = 64 * 1024 * 1024;
const TARGET: &str = "aarch64-unknown-linux-gnu";
const DOWNLOAD_PREFIX: &str = "https://github.com/gofronet/gofro-router/releases/download/";

#[derive(Debug, Deserialize)]
pub(crate) struct Release {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<Asset>,
}

#[derive(Debug, Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
    size: u64,
}

#[derive(Debug)]
pub(crate) struct MetadataUrls {
    pub(crate) manifest: String,
    pub(crate) signature: String,
}

#[derive(Debug)]
pub(crate) struct ValidManifest {
    pub(crate) version: Version,
    pub(crate) version_text: String,
    pub(crate) archive: String,
    pub(crate) sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawManifest {
    schema: u64,
    version: String,
    target: String,
    archive: String,
    sha256: String,
}

pub(crate) fn parse_release(bytes: &[u8]) -> Result<Release> {
    serde_json::from_slice(bytes).context("failed to parse GitHub latest release response")
}

impl Release {
    pub(crate) fn metadata_urls(&self) -> Result<MetadataUrls> {
        ensure!(!self.draft, "latest GitHub release is a draft");
        ensure!(!self.prerelease, "latest GitHub release is a prerelease");
        Ok(MetadataUrls {
            manifest: self.asset_url(MANIFEST_NAME, MANIFEST_LIMIT)?,
            signature: self.asset_url(SIGNATURE_NAME, SIGNATURE_LIMIT)?,
        })
    }

    pub(crate) fn validate_tag(&self, manifest: &ValidManifest) -> Result<()> {
        ensure!(
            self.tag_name == format!("v{}", manifest.version_text),
            "release tag must be exactly v{}",
            manifest.version_text
        );
        Ok(())
    }

    pub(crate) fn archive_url(&self, archive: &str) -> Result<String> {
        ensure!(archive == ARCHIVE_NAME, "unexpected archive name");
        self.asset_url(archive, ARCHIVE_LIMIT)
    }

    fn asset_url(&self, name: &str, maximum: u64) -> Result<String> {
        let mut matches = self.assets.iter().filter(|asset| asset.name == name);
        let asset = matches
            .next()
            .with_context(|| format!("release asset {name} is missing"))?;
        ensure!(
            matches.next().is_none(),
            "release asset {name} appears more than once"
        );
        validate_asset_url(&asset.browser_download_url, name)?;
        ensure!(
            asset.size > 0 && asset.size <= maximum,
            "release asset {name} exceeds its size limit"
        );
        Ok(asset.browser_download_url.clone())
    }
}

fn validate_asset_url(url: &str, name: &str) -> Result<()> {
    ensure!(url.starts_with(DOWNLOAD_PREFIX), "invalid asset URL origin");
    ensure!(url.ends_with(&format!("/{name}")), "invalid asset URL name");
    ensure!(
        url.bytes().all(|byte| byte.is_ascii_graphic())
            && !url.bytes().any(|byte| matches!(byte, b'?' | b'#' | b'\\')),
        "invalid characters in asset URL"
    );
    Ok(())
}

pub(crate) fn parse_manifest(bytes: &[u8]) -> Result<ValidManifest> {
    let raw: RawManifest =
        serde_json::from_slice(bytes).context("failed to parse signed release manifest")?;
    ensure!(raw.schema == 1, "manifest schema must be exactly 1");
    ensure!(raw.target == TARGET, "manifest target must be {TARGET}");
    ensure!(
        raw.archive == ARCHIVE_NAME,
        "manifest archive must be {ARCHIVE_NAME}"
    );
    ensure!(
        raw.sha256.len() == 64
            && raw
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "manifest sha256 must be 64 lowercase hexadecimal characters"
    );
    let version = Version::parse(&raw.version).context("manifest version is not valid semver")?;
    ensure!(version.pre.is_empty(), "manifest version is not stable");
    ensure!(
        version.build.is_empty(),
        "manifest version has build metadata"
    );
    Ok(ValidManifest {
        version,
        version_text: raw.version,
        archive: raw.archive,
        sha256: raw.sha256,
    })
}

pub(crate) fn should_install(current: &Version, candidate: &Version, force: bool) -> bool {
    candidate > current || (force && candidate == current)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn manifest(version: &str, sha: &str) -> Vec<u8> {
        format!(
            r#"{{"schema":1,"version":"{version}","target":"{TARGET}","archive":"{ARCHIVE_NAME}","sha256":"{sha}"}}"#
        )
        .into_bytes()
    }

    fn release(duplicate_archive: bool) -> Vec<u8> {
        let mut assets = vec![MANIFEST_NAME, SIGNATURE_NAME, ARCHIVE_NAME];
        if duplicate_archive {
            assets.push(ARCHIVE_NAME);
        }
        let assets = assets
            .into_iter()
            .map(|name| {
                format!(
                    r#"{{"name":"{name}","size":123,"browser_download_url":"{DOWNLOAD_PREFIX}v1.2.3/{name}"}}"#
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        format!(r#"{{"tag_name":"v1.2.3","draft":false,"prerelease":false,"assets":[{assets}]}}"#)
            .into_bytes()
    }

    #[test]
    fn validates_manifest_fields() {
        let valid = parse_manifest(&manifest("1.2.3", SHA)).unwrap();
        assert_eq!(valid.version, Version::new(1, 2, 3));
        let wrong_schema = String::from_utf8(manifest("1.2.3", SHA))
            .unwrap()
            .replace(r#""schema":1"#, r#""schema":2"#);
        assert!(parse_manifest(wrong_schema.as_bytes()).is_err());
        assert!(parse_manifest(&manifest("1.2.3-beta.1", SHA)).is_err());
        assert!(parse_manifest(&manifest("1.2.3", &SHA.to_uppercase())).is_err());
    }

    #[test]
    fn selects_each_release_asset_once_and_checks_tag() {
        let parsed = parse_release(&release(false)).unwrap();
        let urls = parsed.metadata_urls().unwrap();
        assert!(urls.manifest.ends_with(MANIFEST_NAME));
        let signed = parse_manifest(&manifest("1.2.3", SHA)).unwrap();
        parsed.validate_tag(&signed).unwrap();
        assert!(parsed.archive_url(ARCHIVE_NAME).is_ok());
        let wrong_tag = String::from_utf8(release(false))
            .unwrap()
            .replace("v1.2.3", "1.2.3");
        assert!(
            parse_release(wrong_tag.as_bytes())
                .unwrap()
                .validate_tag(&signed)
                .is_err()
        );
        assert!(
            parse_release(&release(true))
                .unwrap()
                .archive_url(ARCHIVE_NAME)
                .is_err()
        );
    }

    #[test]
    fn orders_versions_without_downgrades() {
        let current = Version::new(1, 2, 3);
        assert!(should_install(&current, &Version::new(1, 2, 4), false));
        assert!(!should_install(&current, &current, false));
        assert!(should_install(&current, &current, true));
        assert!(!should_install(&current, &Version::new(1, 2, 2), true));
    }
}

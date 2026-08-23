use std::path::{Path, PathBuf};

use semver::Version;

pub(crate) const UPDATE_DIR: &str = "/var/lib/maxos-game-tunnel/update";
pub(crate) const VERSION: &str = "/var/lib/maxos-game-tunnel/update/version";
pub(crate) const PENDING: &str = "/var/lib/maxos-game-tunnel/update/pending.json";
pub(crate) const PROGRESS: &str = "/var/lib/maxos-game-tunnel/update/status.json";
pub(crate) const PUBLIC_KEY: &str = "/etc/maxos-game-tunnel/update-public.pem";
pub(crate) const STATUS_ADDRESS: &str = "/etc/maxos-game-tunnel/status-address";
pub(crate) const RELEASES: &str = "/usr/local/lib/maxos-game-tunnel/releases";
pub(crate) const CURRENT: &str = "/usr/local/lib/maxos-game-tunnel/current";
pub(crate) const UPDATER: &str = "/usr/local/bin/gofro-updater";

pub(crate) const RELEASE_API: &str =
    "https://api.github.com/repos/gofronet/gofro-router/releases/latest";
pub(crate) fn release(version: &Version) -> PathBuf {
    Path::new(RELEASES).join(version.to_string())
}

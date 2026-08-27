# Releases

Stable tags cross-compile static AArch64 musl binaries with a pinned OpenWrt
25.12 SDK and publish signed TR3000 and Raspberry Pi 5 installation bundles.

## Create a release

1. In a dedicated pull request, update `[workspace.package].version` in
   `Cargo.toml` and the workspace package versions in `Cargo.lock`.
2. Run the checks below and squash-merge the pull request into `main`.
3. Open **Actions -> Release -> Run workflow**, select `main`, and enter the
   version without the `v` prefix.

The workflow requires the latest `main` commit and a matching Cargo workspace
version. It tests, builds, and signs the bundles before creating the protected
tag and publishing the release. A failed run can be retried when its existing
tag points to the same commit and its release remains a draft. Published
releases are immutable.

## Checks

```sh
cargo fmt --check
cargo check --workspace
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cd web && bun install --frozen-lockfile && bun run check && bun run build
cd .. && sh deploy/openwrt/tests/mode.sh && sh deploy/openwrt/tests/transaction.sh
sh deploy/openwrt/tests/update.sh
sh deploy/openwrt/tests/version.sh
sh deploy/raspios/tests/mode.sh
sh deploy/raspios/tests/network.sh
sh deploy/raspios/tests/recover.sh
sh deploy/raspios/tests/transaction.sh
sh deploy/raspios/tests/tunnel.sh
sh deploy/raspios/tests/update.sh
sh deploy/raspios/tests/version.sh
sh deploy/raspios/tests/wifi.sh
```

## Upgrade a router

Run the signed updater already installed on the target platform:

```sh
# OpenWrt
ssh root@10.203.1.1 gofro-update

# Raspberry Pi OS, locally or over its Ethernet address
sudo gofro-update
```

The updater also checks GitHub every six hours. It verifies the Ed25519-signed
manifest and archive checksum, switches the version atomically, and rolls back
when the new agent fails its health check. Firmware and kernel upgrades remain
separate OpenWrt `sysupgrade` or Raspberry Pi OS `apt` operations.

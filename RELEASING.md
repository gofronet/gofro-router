# Releases

Stable tags cross-compile static AArch64 and MIPS little-endian musl binaries
with pinned OpenWrt 25.12 SDK toolchains and publish signed installation bundles.

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
python3 deploy/openwrt/tests/geodata_filter.py
```

## Upgrade a router

Run the signed updater already installed on the router:

```sh
ssh root@10.203.1.1 gofro-update
```

The updater also checks GitHub every six hours. It verifies the Ed25519-signed
manifest and archive checksum, switches the version atomically, and rolls back
when the new agent fails its health check. Firmware and kernel upgrades remain
separate OpenWrt `sysupgrade` operations.

# Releases

Stable tags cross-compile static AArch64 and MIPS little-endian musl binaries
with pinned OpenWrt 25.12 SDK toolchains and publish signed installation bundles.

## Create a release

1. Update `[workspace.package].version` in `Cargo.toml`.
2. Run the checks below and merge the change into `main`.
3. Create and push the matching tag:

   ```sh
   VERSION=0.4.3
   git tag "v$VERSION"
   git push origin "v$VERSION"
   ```

The workflow rejects a tag that differs from the Cargo workspace version. It
publishes the signed OpenWrt bundle, manifest, signature, and `gofro-install`
bootstrap. Published releases are immutable.

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

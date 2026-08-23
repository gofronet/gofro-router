# Releases and device updates

Stable tags are the deployment boundary. A tag such as `v0.2.0` starts the
release workflow, builds Linux ARM64 binaries, signs the update manifest, and
publishes a GitHub release. Draft releases are invisible to devices, so a
partially uploaded release is never installed.

## Create a release

1. Update `[workspace.package].version` in `Cargo.toml` and run `cargo check`
   to update `Cargo.lock`.
2. Merge the tested change into `main`.
3. Create and push the matching protected tag:

   ```bash
   git tag v0.3.0
   git push origin v0.3.0
   ```

The release job rejects a tag that differs from any workspace package version
or is not reachable from `main`. Only release administrators can create `v*`
tags. The signing job also requires approval in the protected `release` GitHub
environment. Published release assets are immutable.

## Bootstrap existing 0.1 devices

The first updater installation is intentionally manual because 0.1 has no
trusted update key. Build `gofro-updater` for ARM64, copy it together with
`deploy/pi/install-updater.sh`, `migrate.sh`, and `update-public.pem` to the Pi,
then run from a trusted checkout. Authenticate the first key out of band before
copying it; this release's PEM SHA-256 is
`2e43052250e605d282005ede666cf217635bdbcf9f9b99453effe4a1e300334f`.
The Pi needs `curl`, OpenSSL with Ed25519 `pkeyutl -rawin`, `sha256sum`, `tar`,
and systemd. Preserve custom interface and dashboard address values from the
original install:

```bash
sudo UPDATER_BINARY=/tmp/gofro-updater \
  UPDATE_PUBLIC_KEY=/tmp/update-public.pem \
  MIGRATION_SCRIPT=/tmp/migrate.sh \
  BOOTSTRAP_VERSION=0.1.0 \
  WG_INTERFACE=gt0 \
  STATUS_ADDRESS=10.203.1.1 \
  /tmp/install-updater.sh
sudo systemctl start gofro-updater.service
```

The bootstrap keeps the running 0.1 binaries as the rollback release. Future
installs get the updater directly from `deploy/pi/install.sh`.

## Device transaction

`gofro-updater.timer` checks the latest release every six hours with a random
delay. The updater verifies the Ed25519 signature and archive SHA-256 before it
extracts anything. Releases live under
`/usr/local/lib/maxos-game-tunnel/releases/<version>`; switching the `current`
symlink activates all application binaries atomically.

Before activation the updater records a durable pending transaction. After a
power loss it either completes a healthy release or switches back to the saved
release. A failed service/API check also rolls back automatically. If VPN had a
fresh handshake before the update, a fresh handshake is required afterwards.

Useful commands:

```bash
systemctl list-timers gofro-updater.timer
sudo systemctl start gofro-updater.service
journalctl -u gofro-updater.service
curl http://10.203.1.1:8080/api/status
cat /var/lib/maxos-game-tunnel/update/version
```

## Breaking changes

Every bundle contains `deploy/pi/migrate.sh`. Migrations must be version-gated
and idempotent because recovery may repeat them:

- `up OLD NEW`: make the new binaries compatible, but retain rollback data.
- `down OLD NEW`: restore compatibility with the old binaries.
- `commit OLD NEW`: remove rollback-only data after health checks pass. The
  updater records this phase durably and only completes forward afterwards.

Do not make an irreversible schema change in `up`. Keep relay protocol changes
backward compatible and deploy VPS support before tagging the Pi release.

The signing private key exists only as the GitHub Actions secret
`UPDATE_SIGNING_KEY` in the protected `release` environment. The matching public key is pinned in
`deploy/pi/update-public.pem`. Rotate it with a transition release signed by the
old key before using a new private key. That release's reversible migration must
install the new key in `up` and restore the old key in `down`.

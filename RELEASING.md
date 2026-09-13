# Releases

The next release workflow cross-compiles static musl binaries for eight
supported OpenWrt 25.12 ABIs and one x86_64 VPS target: nine signed bundles,
nine manifests, nine signatures and two installers (29 release assets total).
The next signed candidate workspace version is `0.5.18`. The installed signed
v0.5.17 candidate is the immutable, pinned baseline; changes require a new
candidate, not modification of its artifact. Published Latest remains v0.5.15.

## Create a release

1. In a dedicated pull request, update `[workspace.package].version` in
   `Cargo.toml` and the workspace package versions in `Cargo.lock`.
2. Run the checks below and squash-merge the pull request into `main`.
3. Open **Actions -> Release -> Run workflow**, select `main`, and enter the
   version without the `v` prefix (`0.5.18`). Leave **publish** unchecked
   (the default) to build a candidate.

The workflow requires the latest `main` commit and a matching Cargo workspace
version. Candidate runs execute all tests, eight platform builds, packaging and
signing, but create no tag or release and do not change Latest. Signing requires
the protected `release` environment and its GitHub `UPDATE_SIGNING_KEY` secret;
there is no local private-key or signature-verification bypass.

Both modes upload the exact 29 release files as an immutable workflow artifact:
`signed-release-vVERSION-SHA` (full commit SHA). The separate artifact
`signed-release-metadata-vVERSION-SHA` contains `release-metadata.json` with
version, commit, run ID/attempt and every file's SHA-256. Metadata is outside
the 29-file download/publication inventory. Retention is 30 days; retain the
downloaded artifacts and qualification record before expiration. Artifact names
are scoped to a workflow run; record the run ID and artifact IDs as well.

### Qualify the candidate on hardware

1. Download both artifacts from the successful candidate run. Check the metadata
   version/commit against that run and verify all 29 file hashes. Verify each
   manifest signature with the repository's trusted `update-public.pem` and
   each archive checksum against its signed manifest before extracting it.
2. Have the hardware operator install those exact bundles on the test OpenWrt
   router and x86_64 VPS using the documented platform installation procedure
   with locally staged verified bundles. Do not use the normal Latest updater
   to test an unpublished candidate. Preserve signature verification throughout.
3. Record target ABI, firmware/OS, bundle hashes and results: installation and
   v0.5.15 migration, management access, DNS, VPN routing, PPPoE where applicable,
   reboot/recovery and guarded rollback; qualify the VPS install/managed lifecycle
   and router-to-VPS tunnel. Record untested ABIs explicitly. CI/emulation alone
   is not hardware qualification.

For v0.5.18, also record the upgrade from the pinned signed v0.5.17 baseline.
Qualify bare `wifi.gofro.net` from LAN using the router's local LAN resolver:
its DNS A answer is reserved VIP `198.18.0.0` (TTL 30 seconds), outside FakeDNS
lease allocation. Renew stale cached DNS answers (wait out the previous TTL or
flush the client cache); external DNS/DoH is not a substitute for this resolver.
Verify LAN-only TCP VIP `80`/`8081` -> existing LAN `8081` and VIP `443`/`8443`
-> existing LAN `8443`, strict canonical alias Host/Origin validation, and
unchanged LAN-address access and all existing panel/LuCI ports. No additional
LAN IP, proxy or listener is introduced. The candidate requires native router
`conntrack` and adds a Gofro-owned dnsmasq `hostrecord` for the panel alias.
Check `https://<LAN-address>:8443` as the direct-IP fallback, especially after
an initial reconcile failure when VIP rules may not yet be installed. Candidate
qualification must not overwrite the v0.5.17 artifact or change published Latest.

Qualify **Rules -> Devices -> Direct**, manual MAC entry and removal with VPN
on/off, rules/all modes, VPN failure, restart and guard retention. Verify full
MAC-based bypass of Gofro routing, blocking, FakeDNS and IPv6/forwarding guards,
with ordinary OpenWrt DNS, IPv6 and firewall policy preserved. DHCP inventory is
read-only display data, never enforcement. Check native DNS panel resolution,
privacy-MAC changes and the downstream NAT-router limitation (one visible MAC
for its clients). Existing NAT sessions and client DNS caches may need reconnect
or renewal; Direct neither guarantees zero overhead nor relaxes the global
software/hardware offloading refusal.

Configuration uses top-level `device_exclusions`, not `routing.device_exclusions`.
`POST /api/device-exclusions` takes one MAC and boolean `excluded`;
`GET /api/lan-devices` supplies inventory. Reserve conntrack bit `0x40000000`
exclusively for Gofro LAN DNS ownership: deployments sharing it with another
package are unsupported. Native `dns-flows` cleanup removes owned DNS flows
and recognized legacy Gofro DNS DNAT only; verify unrelated flows survive.

### Publish after qualification

Dispatch the same version from the same latest `main` commit with **publish**
explicitly checked. This runs the full build/sign/check pipeline again; it does
**not** promote the earlier run's artifact. The separate publish job downloads
only its own run's immutable signed artifact and waits on the protected `release`
environment. Keep required reviewers enabled: signing approval is not publication
approval. Before approving the publish job, compare version, full commit SHA and
all 29 filename/SHA-256 pairs in this run's metadata with the hardware-qualified
candidate (run ID/attempt naturally differ). Same source commit alone does not
prove identical build bytes. If any hash differs, qualify this exact new artifact
before approval; if `main` advanced, build and qualify a new candidate. Never
describe another commit or an unchecked rebuild as hardware-tested. This hash
comparison and hardware gate are operator requirements, not automated assertions.

Only the explicit publish job can create the protected tag and publish Latest.
A failed run can be retried when its existing
tag points to the same commit and its release remains a draft. Published
releases are immutable. Signed artifacts are never overwritten; if a full rerun
hits an existing artifact name, dispatch a new run rather than replacing it.

## Checks

```sh
cargo fmt --check
cargo check --workspace
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
(cd web && bun install --frozen-lockfile && bun run check && bun test && bun run build)
sh deploy/openwrt/tests/guard.sh
sh deploy/openwrt/tests/network.sh
sh deploy/openwrt/tests/mode.sh
sh deploy/openwrt/tests/transaction.sh
sh deploy/openwrt/tests/tunnel.sh
sh deploy/openwrt/tests/update.sh
sh deploy/openwrt/tests/version.sh
bash deploy/server/tests/version.sh
```

CI and release validation syntax-check each shell file individually and
ShellCheck the OpenWrt fixtures as POSIX shell. Both run the remaining shell
regressions too. Both workflows build `deploy/openwrt/tests/Dockerfile` with
`iproute2`, `nftables`, `python3`, `conntrack`, `dnsmasq-base` and `jq`, plus native
UCI/jsonfilter built from pinned OpenWrt library/tool revisions. A workflow-only
image layer adds `procps`. No host UCI/jsonfilter or implicit mock installation
is required; test-local mocks remain inside disposable test state.
`bash deploy/openwrt/tests/dataplane-netns.sh --emit-only` emits Rust fixtures;
`sh deploy/openwrt/tests/dataplane-netns.sh --run` executes in an isolated Docker
container. A container-local sysctl wrapper mounts proc only when invoked inside
the script's owned network/mount namespace. The separate
`mode-netns.sh --run` runs as root only inside a disposable Docker container
with `--network none --cap-add NET_ADMIN` and a read-only source mount; its
dependencies are installed in the shared Debian trixie image before
disconnecting networking (compatible with the Ubuntu 24.04 native binary).

Both workflows also build the actual Linux executable with
`cargo build --locked -p gofro-agent --bin gofro-agent`, retain the renderer
fixtures, and run `sh deploy/openwrt/tests/dns-netns.sh` in a separate container
from that image. This script takes no flags: `AGENT_BIN` points to the built
executable and `FIXTURES` to the rendered nftables directory, both mounted
read-only with the source. Temporary state stays inside the container. It uses
root, `--network none`, `NET_ADMIN`, `NET_RAW` and `SYS_ADMIN`, with
`--security-opt apparmor=unconfined` for nested namespace mounts, never host
networking. `sh deploy/openwrt/tests/exclusions-netns.sh` runs in another
container with the same isolation/capabilities and read-only source mount.
It exercises native dnsmasq, jsonfilter and conntrack, dual-stack MAC exclusion
forwarding, guard/config ownership and selective DNS-flow cleanup. Dataplane
tests use those same capabilities and read-only renderer fixtures.

The real-agent packet probes cover connected UDP/TCP IPv4 and IPv6 DNS,
link-local/ULA/GUA aliases, redirected destinations, reply source address/port,
DNS ID/question/A answers and wrong-device rejection. This exercises the
production packet-info reply path, not just a socket test double; local panel
DNS must not query upstream. It does not validate upstream DNS delegation.
See the workflow commands for the complete checks. Namespace tests simulate
devices, not real PPPoE negotiation or OpenWrt hardware/boot qualification.

## Upgrade a router

Run the signed updater already installed on the target platform:

```sh
ssh root@192.168.0.1 gofro-update
```

The updater also checks GitHub every six hours. It verifies the Ed25519-signed
manifest and archive checksum, switches the version atomically, and rolls back
when the new agent fails its health check. Firmware and kernel upgrades remain
separate OpenWrt `sysupgrade` operations.

Use **Panel -> Check for updates** in the new UI. Before installation/activation,
the user must disable both software and hardware flow offloading in OpenWrt;
the helper refuses enabled offloading without modifying it. Existing network
settings are preserved; only attested Gofro-owned v0.5.15 state is migrated.
See [OpenWrt installation and maintenance](deploy/openwrt/README.md).

A failed guarded rollback to v0.5.15 can restore management without restoring
forwarding. The persisted boot guard remains until a compatible agent fully
reconciles; new-agent degraded health is HTTP `503`, and rollback finalization
does not declare a retained guard healthy. Never remove it to force completion.

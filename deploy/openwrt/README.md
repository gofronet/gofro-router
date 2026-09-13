# OpenWrt installation

Gofro installs on official OpenWrt 25.12 without replacing the firmware. It is
not tied to a router vendor or model: the bootstrap selects a signed static
bundle from OpenWrt's `DISTRIB_ARCH`.

This describes the next VPN-only candidate, v0.5.18. The installed
signed v0.5.16 candidate remains the immutable, pinned baseline; new changes
require a new candidate artifact. Published Latest remains v0.5.15. The download
links select that published release, not unshipped workspace changes.

Release bundles cover these OpenWrt package ABIs:

- `aarch64_*`;
- `arm_arm926ej-s`, `arm_xscale`, and `arm_arm1176jzf-s_vfp`;
- `arm_cortex-*`, with soft-float and hard-float bundles selected separately;
- `i386_pentium-mmx`, `i386_pentium4`, `riscv64_generic`, and `x86_64`.

MIPS, big-endian ARM, LoongArch, and PowerPC builds are not published because
their OpenWrt ABI is not covered by a qualified Rust 1.98 target. The installer
rejects an unsupported ABI before changing the router.

The router must have at least 256 MiB RAM (192 MiB reported as `MemTotal`) and
a validated OpenWrt logical `lan`, with a primary IPv4 address and a device
separate from WAN/WAN6 (including bridge members). Its firewall zone must belong
exclusively to logical `lan`, without extra device/subnet membership. A custom
unique zone name is allowed; the literal zone name `lan` is not required, and
`wan`/`wan6` are not allowed. LAN must not overlap reserved `10.202.0.0/24` or
`198.18.0.0/15`. Installation needs about 48 MiB free
in `/tmp` and 35 MiB free on the filesystem containing `/usr/lib`; an update
needs the same persistent space in addition to the installed release.

## Install

Configure LAN, WAN, DHCP, Wi-Fi, country, and LuCI in OpenWrt first. Gofro can
run on the main router; a separate downstream router is optional. Gofro does
not configure these settings. Disable both software and hardware flow
offloading in OpenWrt before installation or activation. The network helper
refuses enabled offloading; it never disables it for you. On a fresh supported
router, run:

```sh
tmp="$(mktemp)" && trap 'rm -f "$tmp"' EXIT && uclient-fetch -q -O "$tmp" https://github.com/gofronet/gofro-router/releases/latest/download/gofro-install && sh "$tmp" --install
```

Successful installation prints a one-time setup code and its 15-minute expiry.
Use the code to create the administrator and add a VPN profile or VPS from the
Gofro panel. It does not create a setup SSID or change network configuration.

Open bare `wifi.gofro.net` from LAN to reach Gofro. Clients must use
the router's local LAN resolver; external DNS/DoH does not provide this access.
Local DNS answers with VIP `198.18.0.0`, reserved outside FakeDNS lease
allocation. Gofro DNS uses TTL 30 seconds; native dnsmasq uses its local-record
TTL. Clients with an old cached DNS answer must
wait out its previous TTL or flush their DNS cache and resolve the name again.

LAN-only TCP traffic to VIP ports `80`/`8081` is redirected to the existing
LAN `8081` listener; VIP ports `443`/`8443` go to the existing LAN `8443`
listener. Host/Origin validation strictly accepts the canonical alias
`wifi.gofro.net` while preserving existing LAN-address validation. This adds
no LAN IP, network configuration, proxy, listener or dependency. All existing
panel and LuCI ports stay in place, including LuCI's normal `80` and `443`.

Direct access remains `https://<LAN-address>:8443`; `http://<LAN-address>:8081`
redirects to it. Use the direct-IP HTTPS fallback when the domain is unavailable,
especially after an initial reconcile failure: VIP access requires successfully
installed rules. For example, `192.168.0.1` remains configured in OpenWrt; Gofro
never forces `10.203.1.1` or any other LAN address.

WAN can be DHCP behind another router or PPPoE. Existing PPPoE credentials and
MTU stay in OpenWrt and are preserved, as are LAN, DHCP, Wi-Fi and LuCI settings.
Gofro intercepts non-excluded LAN TCP/UDP DNS for both IPv4 and IPv6 from port 53 to 5353.
Its dual-stack DNS sockets use `SO_BINDTODEVICE` on the validated LAN device;
resolution delegates to native OpenWrt dnsmasq at the LAN IPv4 address on port
53. It does not replace the system resolver. Fully excluded devices use native
DNS and direct forwarding in every VPN mode, including while the guard is armed.
The exclusion is the device's current LAN MAC, including private/randomized
unicast MACs; changing that MAC requires updating the exclusion.

Native panel DNS is the owned UCI section `dhcp.gofro_panel=hostrecord`, with
`name=wifi.gofro.net`, `ip=198.18.0.0`, and `instance=<LAN dnsmasq section ID>`.
OpenWrt 25.12's `dhcp_hostrecord_add` renders `--host-record=name,ip` and
`filter_dnsmasq` supports `instance` (including anonymous `cfg...` IDs).
Preflight selects one enabled port-53 instance serving logical LAN, honoring
`dhcp.lan.instance`; ambiguous, address-bound, or conflicting configurations
are refused before mutation. The installer neither changes global forwarders
nor leases. Activation reloads dnsmasq even when the owned record already
matches: a previous attempt may have committed successfully but failed reload.
Read-only preflight does not reload. Update rollback restores the exact section,
including absence and option/list types; restoring an existing owned record
also retries activation when its disk state already matches.

An existing v15 LAN at `10.203.1.0/24` is not automatically renumbered. Change
it in OpenWrt. Legacy route adoption requires installer-recorded, attested
v0.5.15 ownership and the exact guard on the recorded LAN device; matching
route shapes alone are not ownership. Only proven Gofro-owned legacy DNS
settings are migrated. Incomplete legacy setup, ambiguous DNS state, or an
unsafe config migration is refused rather than guessed.

The legacy country argument is accepted and ignored for compatibility; it is not
needed in installation commands. The bundle contains the complete GeoSite and
GeoIP databases.

## VPN setup

In **Panel -> Change password**, enter the current administrator password and
the new password twice. The new password requires at least eight Unicode
characters and at most 128 UTF-8 bytes. A successful change keeps the current
browser signed in with new session/CSRF cookies and invalidates other sessions.
If the result cannot be confirmed, check login with the new password before
retrying; the panel never automatically repeats the password-change request.

The one-time setup window only creates the administrator and configures VPN.
Network settings are not part of the Gofro UI.

## Guard and maintenance

`gofro-guard` boots at `START=18`, before OpenWrt network/WAN startup. It uses
the persisted root-owned `/etc/gofro/guard-device` (`0600`, directory `0700`),
without needing ubus or a LAN address for enforcement. It reads the committed
controller exclusions, synchronizes both owned nft sets atomically, and blocks
all other LAN egress until a compatible agent completes a full reconcile.
Stopping the agent rearms the guard, removes only the exactly recognized DNS
redirect chain, and cleans up owned DNS conntrack flows. Stopping the service
is not a way to bypass VPN policy for non-excluded devices.

DNS cleanup runs after enforcement is published. Its legacy-flow cleanup uses
current LAN addresses (IPv4 and IPv6, including link-local, ULA and GUA), never
client discovery/ARP. At early boot, a successful validated `ip -j link show`
dump can prove the LAN device is absent. Marked-flow cleanup still runs; only
address-based legacy cleanup is deferred until the device exists. Thus START=8
pending recovery can finish before LAN creation with the guard armed. Genuine
netlink/cleanup errors fail the operation rather than masquerading as zero
matches. No whole-conntrack flush is used.

Address renumbering on the same LAN device is supported. Manual LAN device or
zone reassignment requires coordinated maintenance with forwarding kept
blocked, not just editing Gofro state. Normal fw4 reload preserves Gofro's
owned tables; `fw4 stop` or flushing the entire ruleset removes protection and
also requires coordinated maintenance before WAN forwarding resumes.

### v18 shell/core contract

- `/etc/gofro/controller.json` has **top-level** `device_exclusions: [canonicalMAC]`.
  A missing file/member means `[]`; explicit null is invalid. Maximum 256
  strings, six lowercase colon-separated octets, nonzero and unicast. Locally
  administered/private MACs are valid. Shell strictly parses one root-owned
  regular `0600` file with trusted ancestors; `.new`/crash files are ignored.
  There is no separate MAC snapshot. Invalid configuration arms an empty-list
  fail-closed guard, clears both exclusion sets, and returns failure.
- Core always creates `inet gofro_routing`'s `device_exclusions` set with
  `type ether_addr`, even when empty. Shell owns the corresponding set in
  `inet gofro_guard`. When routing exists, set contents and guard-chain changes
  are one atomic nft transaction; FakeDNS mappings and other routing objects
  are retained. Empty-list chain shape remains exactly the v15 LAN-to-non-LAN
  drop. Nonempty adds `ether saddr != @device_exclusions` to that drop.
- Lock order is persistent `/etc/gofro/apply.lock` on **FD8**, then mode lock
  on **FD9**. This fences config read through set/guard publication and cleanup.
  Standalone boot/prepare/stop acquires FD8. New procd explicitly passes
  `GOFRO_APPLY_LOCK_FD=8`; old v17 procd inherits FD8 without that marker.
  Guard detects either case before opening its own FD8, verifies device/inode against
  the trusted lock file and runs `flock -n 8` on that inherited descriptor.
  It never reopens/acquires a second descriptor under procd's held lock.
  Wrong-inode descriptors are refused; an unlocked matching descriptor must
  acquire the lock, and a competing writer causes refusal before publication.
- Core's owned DNS chain is either the old two LAN UDP/TCP port-53 redirects
  to `5353`, or exactly `iifname "LAN" ether saddr @device_exclusions return`
  followed by those two redirects. Stop also accepts an empty chain. Extra,
  reordered, wrong-device, wrong-port or differently scoped rules are foreign.
- Core alone sets stable conntrack bit **`0x40000000`**, preserving other bits,
  only for original-direction LAN TCP/UDP original-destination-port-53 flows,
  **before exclusion return**, including native/excluded DNS. Router-generated
  and non-LAN DNS must not acquire this ownership bit.
- Core calls **`/usr/libexec/gofro/dns-flows cleanup LAN_DEVICE DNS_PORT`** after
  committed exclusion changes and full reconcile, while holding apply.lock,
  after publishing the current sets/rules and before clearing the guard.
  Failure means committed intent remains saved, the guard remains armed with
  validated exclusions, and the operation/health reports degraded failure.
  Startup shell sync invokes the same idempotent helper before fallible Rust
  initialization. The helper itself does not acquire the caller's lock.
- Cleanup uses native `conntrack -D` separately for IPv4/IPv6 and UDP/TCP,
  original dport 53 and mark `0x40000000/0x40000000`. Legacy unmarked cleanup
  additionally requires DNAT, exact current LAN reply-source address and
  reply-source port `DNS_PORT`. The mutable VPN mark alone is never ownership.
  Exit 1 succeeds only with the exact native single-line zero-deleted diagnostic;
  other errors propagate. Both fresh and update dependency installation add
  `conntrack` and strict-parser `jq` before security calls. OpenWrt's
  `conntrack -> libnetfilter-conntrack -> kmod-nf-conntrack-netlink` dependency
  pulls matching kernel netlink support through apk, without manual modules.

### Isolated checks

`tests/transaction.sh` retains the installer/recovery failure and crash suite;
`tests/guard.sh` exercises procd lifecycle fencing with fake system commands.
`tests/panel.sh` runs native UCI against temporary config/delta directories and
a fake reload command. `tests/exclusions-netns.sh` exercises actual nft,
conntrack and dual-stack packets in disposable Linux network namespaces.
`tests/lifecycle-netns.sh` reads the exact v17 init from git commit `5132a7a`
and exercises it with the new guard, pending START=8 recovery before LAN exists,
and a committed-record reload failure/retry against native dnsmasq.
`tests/dns-netns.sh` retains all existing production-agent DNS, panel/LuCI and
VIP checks and verifies the new shell-owned sets independently.

Build `tests/Dockerfile` as `gofro-exclusions-test:local`; it pins native UCI,
jsonfilter and libubox to OpenWrt 25.12's source revisions. Run with Docker context `desktop-linux`,
`--network none`, and the repository mounted **read-only** at `/repo`:

```sh
docker --context desktop-linux run --rm --network none --privileged \
  -v "$PWD:/repo:ro" -w /repo gofro-exclusions-test:local \
  sh deploy/openwrt/tests/exclusions-netns.sh
docker --context desktop-linux run --rm --network none \
  -v "$PWD:/repo:ro" -w /repo gofro-exclusions-test:local \
  sh deploy/openwrt/tests/panel.sh
```

The existing production DNS suite additionally takes read-only `FIXTURES` and
`AGENT_BIN` mounts. Tests do not contact a router or use host networking.

## VPS

For a fresh x86_64 Debian or Ubuntu VPS, use **Servers -> New VPS** in the
Gofro panel. Enter the IP address, SSH port and current root password, then
start setup. The panel shows the actual connection and setup stages and any
failure. Password SSH login as root must be enabled. A compatible configured
server is reused without reinstalling it; incompatible servers require a
compatible signed release. The root password is not saved.

The SSH host key is trusted automatically on first use and pinned for subsequent
connections, including after removing and re-adding the server. A changed key is
rejected. First-use trust does not protect against interception of that first
connection. Subsequent management uses a restricted SSH key.

Re-adding a server creates a new router profile without deleting existing peers
or friends. If setup is interrupted, check the router and VPS state before
retrying: losing the progress connection does not cancel server-side setup.

Alternatively, install the signed bundle directly on the VPS:

```sh
tmp="$(mktemp)" && trap 'rm -f "$tmp"' EXIT && curl -fsSL -o "$tmp" https://github.com/gofronet/gofro-router/releases/latest/download/gofro-server-install && sudo bash "$tmp" --install
```

Allow UDP port `8443` in the VPS/cloud firewall. When migrating a customized
v0.3 VPS, preserve its `WG_INTERFACE`, `WG_PORT`, and `RELAY_PORT` environment
values when running the downloaded installer with `sudo -E bash "$tmp" --install`.
The defaults are `gt0`, `51820`, and `8443`.
Once a signed server bundle is installed, update it with `sudo gofro-server-install`.

Generate a one-time router profile on the VPS:

```sh
sudo gofro-router-server create-router-profile 203.0.113.10:8443
```

Copy the complete output, open **Servers -> Import** in the Gofro web panel, name
the server, and paste the profile. The VPS does not retain the generated client
private key. Store the output securely if you need to restore it after a router
reset. If the VPS peer is lost, run the command again and import the new profile;
the router replaces the previous credentials for that server.

The endpoint is positional and the tunnel address is allocated automatically.
New router profiles record owner identity with a tunnel `/32`, not a fixed LAN
subnet. Do not substitute `create-profile`: without legacy ownership arguments
it creates a friend peer, not a router owner. Friend creation, renaming,
configuration download and revocation remain supported separately.

## Update

Open **Panel** in the Gofro web panel and select **Check for
updates**. Gofro also checks GitHub automatically every six hours. The updater
verifies the signed manifest and checksum, then switches to the new version. A
failed health check restores the previous release; an interrupted update is
rolled back on boot. It upgrades Gofro only, never OpenWrt firmware. Files in
`/etc/config/gofro` and `/etc/gofro` are preserved.

A guarded failed rollback to v0.5.15 may restore management access while
forwarding remains blocked until a compatible agent completes a full reconcile.
A live panel is not proof of recovery: the new agent reports degraded health
as HTTP `503`, and finalization refuses a retained guard rather than declaring
the rollback healthy. Do not clear the guard manually to force success.

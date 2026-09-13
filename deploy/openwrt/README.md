# OpenWrt installation

Gofro installs on official OpenWrt 25.12 without replacing the firmware. It is
not tied to a router vendor or model: the bootstrap selects a signed static
bundle from OpenWrt's `DISTRIB_ARCH`.

This describes the unreleased VPN-only integration on the unchanged v0.5.15
base, not the published v0.5.15 installer. The download links select the latest
published release; they do not install unshipped workspace changes.

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

Gofro serves its panel on `https://<LAN-address>:8443`; port `8081` redirects to
HTTPS. LuCI keeps its normal ports `80` and `443`. For example,
`192.168.0.1` remains configured in OpenWrt; Gofro never forces `10.203.1.1` or
any other LAN address.

WAN can be DHCP behind another router or PPPoE. Existing PPPoE credentials and
MTU stay in OpenWrt and are preserved, as are LAN, DHCP, Wi-Fi and LuCI settings.
Gofro intercepts LAN TCP/UDP DNS for both IPv4 and IPv6 from port 53 to 5353.
Its dual-stack DNS sockets use `SO_BINDTODEVICE` on the validated LAN device;
resolution delegates to native OpenWrt dnsmasq at the LAN IPv4 address on port
53. It does not replace the system resolver.

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

The one-time setup window only creates the administrator and configures VPN.
Network settings are not part of the Gofro UI.

## Guard and maintenance

`gofro-guard` boots at `START=18`, before OpenWrt network/WAN startup. It uses
the persisted root-owned `/etc/gofro/guard-device` (`0600`, directory `0700`),
without needing ubus or a LAN address. Forwarding remains blocked until a
compatible agent completes a full reconcile. Stopping the agent rearms the
guard and removes the owned DNS redirect: new DNS flows immediately use native
DNS, but existing redirected NAT flows may need conntrack expiry or a new
client tuple. Stopping the service is not a way to bypass VPN policy.

Address renumbering on the same LAN device is supported. Manual LAN device or
zone reassignment requires coordinated maintenance with forwarding kept
blocked, not just editing Gofro state. Normal fw4 reload preserves Gofro's
owned tables; `fw4 stop` or flushing the entire ruleset removes protection and
also requires coordinated maintenance before WAN forwarding resumes.

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

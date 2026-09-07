# OpenWrt installation

Gofro installs on official OpenWrt 25.12 without replacing the firmware. It is
not tied to a router vendor or model: the bootstrap selects a signed static
bundle from OpenWrt's `DISTRIB_ARCH`.

Release bundles cover these OpenWrt package ABIs:

- `aarch64_*`;
- `arm_arm926ej-s`, `arm_xscale`, and `arm_arm1176jzf-s_vfp`;
- `arm_cortex-*`, with soft-float and hard-float bundles selected separately;
- `i386_pentium-mmx`, `i386_pentium4`, `riscv64_generic`, and `x86_64`.

MIPS, big-endian ARM, LoongArch, and PowerPC builds are not published because
their OpenWrt ABI is not covered by a qualified Rust 1.98 target. The installer
rejects an unsupported ABI before changing the router.

The router must have at least 256 MiB RAM (192 MiB reported as `MemTotal`), use
the standard UCI `lan` and `wan` zones, and have at least one configured 2.4 or
5 GHz Wi-Fi access point. Installation needs about 48 MiB free in `/tmp` and
35 MiB free on the filesystem containing `/usr/lib`; an update needs the same
persistent space in addition to the installed release.

## Install

On a fresh supported OpenWrt router, replace `DE` with the two-letter Wi-Fi
country and run:

```sh
tmp="$(mktemp)" && trap 'rm -f "$tmp"' EXIT && uclient-fetch -q -O "$tmp" https://github.com/gofronet/gofro-router/releases/latest/download/gofro-install && sh "$tmp" --install DE
```

The bootstrap verifies a signed bundle and installs the required packages.
Successful installation prints only:

```text
GofroNET Wi-Fi Setup
https://wifi.gofro.net
```

Connect to this passwordless setup network and open the URL. It is isolated from
the internet, the home LAN, and the router's SSH/LuCI services. The setup window
lasts 15 minutes; the first nearby user to create an administrator password owns
the device. Confirm the router's self-signed HTTPS certificate before proceeding.

1. Create an administrator password of at least 12 characters.
2. Choose separate SSIDs and passwords for the available 2.4/5 GHz access points.
3. Reconnect to a new secured SSID, reopen the URL, and import a VPN profile or
   configure your VPS. You can skip this step and add a server later.

All bands are saved together, so changing the first band cannot interrupt the
second. Setup progress survives reloads; after a lost session, sign in again.
The open setup AP closes on expiry or reboot. Repeat the same installation
command to re-arm an incomplete setup without resetting the admin password.
Installer details are retained in a root-only `/tmp/gofro-install.*` log, never
printed credentials. Updates preserve existing Wi-Fi and do not start this wizard;
older installations without an admin password retain Wi-Fi-password verification.

LuCI remains available at `http://10.203.1.1:81` or
`https://10.203.1.1:444` after Wi-Fi setup, with its self-signed certificate.

The bundle contains the complete GeoSite and GeoIP databases.

## VPS

For a fresh x86_64 Debian or Ubuntu VPS, use **Servers -> New VPS** in the
Gofro panel. Enter the IP address, SSH port and current root password, then
verify the SSH fingerprint before confirming. Password SSH login as root must
be enabled. Gofro installs a signed server bundle and uses a restricted SSH key
for subsequent management; the root password is not saved.

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
sudo gofro-router-server create-profile --endpoint 203.0.113.10:8443 --tunnel-ip 10.202.0.2/32
```

Copy the complete output, open **Servers -> Import** in the Gofro web panel, name
the server, and paste the profile. The VPS does not retain the generated client
private key. Store the output securely if you need to restore it after a router
reset. If the VPS peer is lost, run the command again and import the new profile;
the router replaces the previous credentials for that server.

## Update

Open **Settings → System** in the Gofro web panel and select **Check for
updates**. Gofro also checks GitHub automatically every six hours. The updater
verifies the signed manifest and checksum, then switches to the new version. A
failed health check restores the previous release; an interrupted update is
rolled back on boot. Files in `/etc/config/gofro` and `/etc/gofro` are preserved.

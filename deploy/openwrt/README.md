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

The bootstrap downloads the signed release manifest, verifies the archive, adds
the required OpenWrt packages, changes the LAN address to `10.203.1.1`, and
prints the generated Wi-Fi password. Reconnect to `GofroWIFI 2` or, when the
router has a 5 GHz radio, `GofroWIFI 5`, then open `http://wifi.gofro.net`.
LuCI remains available at `http://10.203.1.1:81` or
`https://10.203.1.1:444` with its self-signed certificate.

The bundle contains the complete GeoSite and GeoIP databases.

## VPS

On the VPS, check out the same release tag, then build and install the server:

```sh
cargo build --release -p gofro-server -p gofro-relay
sudo deploy/server/install.sh
```

When upgrading a customized v0.3 VPS, pass the same `WG_INTERFACE`, `WG_PORT`,
and `RELAY_PORT` values to `sudo -E deploy/server/install.sh`. The defaults are
`gt0`, `51820`, and `8443`.

Generate a one-time router profile on the VPS:

```sh
sudo gofro-router-server create-profile --endpoint 203.0.113.10:8443 --tunnel-ip 10.202.0.2/32
```

Copy the complete output, open **Servers → Add** in the Gofro web panel, name
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

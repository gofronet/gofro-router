# OpenWrt installation

Gofro installs on official OpenWrt 25.12 without replacing the firmware. The
release archive contains static ARM64 binaries, OpenWrt services, geodata and
the installer.

## Install

On a fresh supported OpenWrt router, replace `DE` with the two-letter Wi-Fi
country and run:

```sh
tmp="$(mktemp)" && trap 'rm -f "$tmp"' EXIT && uclient-fetch -q -O "$tmp" https://github.com/gofronet/gofro-router/releases/latest/download/gofro-install && sh "$tmp" --install DE
```

The bootstrap downloads the signed release manifest, verifies the archive, adds
the required OpenWrt packages, changes the LAN address to `10.203.1.1`, and
prints the generated Wi-Fi password. Reconnect to `GofroNET WiFi` and open
`http://gofrowifi.net:8080`.

Do not use an old Cudy intermediate image on routers with serial code `2544` or
newer. Their NAND requires `F50L1G41LC` support.

## VPS

On the VPS, check out the same release tag, then build and install the server:

```sh
cargo build --release -p gofro-server -p gofro-relay
sudo deploy/server/install.sh
```

When upgrading a customized v0.3 VPS, pass the same `WG_INTERFACE`, `WG_PORT`,
and `RELAY_PORT` values to `sudo -E deploy/server/install.sh`. The defaults are
`gt0`, `51820`, and `8443`.

Run the `VPS peer command` printed by the router installer. Add the VPS to the
Gofro web panel using `<VPS-IP>:8443` and the server public key printed by the
VPS installer.

## Update

```sh
ssh root@10.203.1.1 gofro-update
```

Gofro checks GitHub automatically every six hours. The command above runs the
same check immediately. The updater verifies the signed manifest and checksum,
then switches to the new version. A failed health check restores the previous
release; an interrupted update is rolled back on boot. Files in
`/etc/config/gofro` and `/etc/gofro` are preserved.

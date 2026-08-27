# OpenWrt installation

Gofro installs on official OpenWrt 25.12 without replacing the firmware. The
bootstrap validates Cudy TR3000-256MB V1 and installs its signed AArch64 bundle.

## Install

On a fresh supported OpenWrt router, replace `DE` with the two-letter Wi-Fi
country and run:

```sh
tmp="$(mktemp)" && trap 'rm -f "$tmp"' EXIT && uclient-fetch -q -O "$tmp" https://github.com/gofronet/gofro-router/releases/latest/download/gofro-install && sh "$tmp" --install DE
```

The bootstrap downloads the signed release manifest, verifies the archive, adds
the required OpenWrt packages, changes the LAN address to `10.203.1.1`, and
prints the generated Wi-Fi password. Reconnect to `GofroWIFI 2` or, on the
dual-band TR3000, `GofroWIFI 5`, then open `http://wifi.gofro.net`.

The bundle contains the complete GeoSite and GeoIP databases.

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

Generate a one-time router profile on the VPS:

```sh
sudo gofro-server create-profile --endpoint 203.0.113.10:8443
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

# Raspberry Pi OS installation

Gofro supports Raspberry Pi 5 with 64-bit Raspberry Pi OS Lite Trixie. Ethernet
`eth0` must be the active uplink; the onboard `wlan0` becomes a 5 GHz access
point on channel 36. Existing pre-OpenWrt Gofro installations are not migrated.

## Install

Connect the Pi to the home router over Ethernet, replace `DE` with the
two-letter Wi-Fi country, and run:

```sh
tmp="$(mktemp)" && trap 'rm -f "$tmp"' EXIT && curl -fsSL -o "$tmp" https://github.com/gofronet/gofro-router/releases/latest/download/gofro-install-raspios && sudo sh "$tmp" --install DE
```

The installer validates the board and OS, downloads the signed Raspberry Pi
bundle, installs NetworkManager, dnsmasq, nftables and WireGuard, then creates
`GofroWIFI 5`. It prints the generated password when installation succeeds.

Open `http://wifi.gofro.net` after connecting to the new network.

## Update

Use **Settings -> System -> Check for updates** in the Gofro panel or run:

```sh
sudo gofro-update
```

Gofro checks automatically every six hours. Raspberry Pi OS, firmware and
kernel upgrades remain normal `apt` operations and are not managed by Gofro.

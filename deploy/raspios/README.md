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

The installer validates the board and OS, verifies the signed bundle, and creates
an isolated, passwordless `GofroNET Wi-Fi Setup` network for 15 minutes. The console
prints only that SSID and `https://wifi.gofro.net`; installation details go to a
root-only `/tmp/gofro-install.*` log.

Connect and open the URL to create the administrator password, then configure
the secured Wi-Fi SSID and password. Raspberry Pi 5 currently exposes one 5 GHz
AP, not simultaneous 2.4/5 GHz APs. Reconnect to the new network and continue with
the optional WireGuard import or VPS setup; this step can be skipped.

The first nearby user to create the administrator password claims the router.
The setup AP has no internet/home-LAN forwarding or access to SSH. Expiry or
reboot closes it. Run the same installation command to reopen an incomplete
setup; an existing admin password is not reset. Normal updates never reopen the
setup network or rename an existing AP.

## Update

Use **Settings -> System -> Check for updates** in the Gofro panel or run:

```sh
sudo gofro-update
```

Gofro checks automatically every six hours. Raspberry Pi OS, firmware and
kernel upgrades remain normal `apt` operations and are not managed by Gofro.

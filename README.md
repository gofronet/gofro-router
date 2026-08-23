# GofroWiFi Gaming Tunnel

Raspberry Pi 5 turns its built-in Wi-Fi into a dedicated PS5 network. VPN mode routes the subnet through kernel WireGuard and an obfuscated UDP relay to a Linux VPS. Direct mode bypasses the tunnel and uses NAT on the Pi.

## Packet path

```text
VPN:   PS5 -> wlan0 -> gt0 -> local relay -> VPS relay -> gt0 -> VPS NAT -> Internet
Direct: PS5 -> wlan0 -> Pi NAT -> eth0 -> home router -> Internet
```

VPN mode keeps the kill switch: if `gt0` is down, table 100 has an unreachable default and nftables blocks forwarding to the home uplink. Direct mode is an explicit bypass selected in the dashboard.

## Requirements

- Raspberry Pi OS Lite 64-bit on Raspberry Pi 5
- Pi connected to the home router through Ethernet
- Ubuntu or Debian VPS with a public IPv4 address
- Rust 1.85 or newer on the machines used to build the binaries
- UDP port `8443` allowed by the VPS provider and host firewall

## Build

```bash
bun --cwd web install
bun --cwd web run build
cargo build --workspace --release
```

Build the web bundle before the Rust binaries because `pi-agent` embeds the generated assets.

The Cargo workspace keeps each deployable process separate: `pi-agent`, `maxos-server`,
`wg-relay`, and `udp-lab`. Shared WireGuard command handling lives in `tunnel-core`.

## 1. Set up the VPS

Run from the repository root on the VPS:

```bash
cargo build --release -p maxos-server -p wg-relay
sudo ./deploy/server/install.sh
```

The installer prints the server public key. It installs WireGuard, enables IPv4 forwarding, configures nftables NAT for `10.203.0.0/16`, and starts the WireGuard and relay services. One relay process accepts up to 256 Pi clients on the same public port and expires idle sessions after three minutes. Only the obfuscated relay port must be public.

The VPS interface is detected automatically. Override it when necessary:

```bash
sudo WAN_INTERFACE=ens3 WG_PORT=51820 RELAY_PORT=8443 ./deploy/server/install.sh
```

## 2. Set up the Pi

The Pi must use Ethernet as its uplink before running this command. The installer refuses to continue when `wlan0` is the active uplink.

```bash
cargo build --release -p pi-agent -p wg-relay
sudo \
  SERVER_PUBLIC_KEY="SERVER_PUBLIC_KEY" \
  SERVER_ENDPOINT="VPS_IP:8443" \
  AP_PASSWORD="choose-a-password" \
  WIFI_COUNTRY="DE" \
  ./deploy/pi/install.sh
```

Optional settings:

```text
AP_SSID="GofroNET WiFi"
AP_CHANNEL=36
WG_ADDRESS=10.202.0.2/32
GAME_SUBNET=10.203.1.0/29
GAME_GATEWAY=10.203.1.1/29
```

The installer prints the Pi public key and the exact `maxos-server add-peer` command.

## 3. Add the Pi on the VPS

Run the command printed by the Pi installer. Its shape is:

```bash
sudo maxos-server add-peer \
  --public-key "CLIENT_PUBLIC_KEY" \
  --tunnel-ip "10.202.0.2/32" \
  --subnet "10.203.1.0/29"
```

Inspect peers:

```bash
sudo maxos-server status
```

## 4. Connect

Connect a phone to `GofroNET WiFi`, then open `http://gofrowifi.net`. Connect the PS5 to the same SSID.

The mobile-first GofroWiFi dashboard provides VPN/direct switching, system and per-device analytics, editable VPN profiles, and AP name/password settings. A server profile needs the VPS relay endpoint and WireGuard public key; install the server components and add the Pi peer on every VPS before selecting it.

Useful checks on the Pi:

```bash
sudo wg show
systemctl status maxos-wg-relay-client
ip rule
ip route show table 100
sudo nft list table inet maxos_pi
```

Useful checks on the VPS:

```bash
sudo wg show
systemctl status maxos-wg-relay-server
sudo nft list table inet maxos_server
```

## Plaintext UDP lab

`udp-lab` is an intentionally insecure TUN-over-UDP benchmark. It has no encryption, authentication, replay protection, key rotation, or production use.

On the VPS, restrict the provider firewall to the Pi's current public IP, then run:

```bash
cargo build --release -p udp-lab
sudo ./target/release/udp-lab server --peer-ip HOME_PUBLIC_IP
```

On the Pi:

```bash
cargo build --release -p udp-lab
sudo ./target/release/udp-lab client --server VPS_IP:51900
ping 10.99.0.1
```

The lab sends plaintext inner IP packets and remains separate from production. `wg-relay` only wraps already encrypted WireGuard datagrams to avoid protocol fingerprinting.

## Development checks

```bash
cargo fmt --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
bun --cwd web run check
bun --cwd web run build
bash -n deploy/pi/install.sh
bash -n deploy/server/install.sh
```

#!/usr/bin/env bash
set -euo pipefail
WG_INTERFACE="${WG_INTERFACE:-gt0}"
WG_ADDRESS="${WG_ADDRESS:-10.202.0.2/32}"
GAME_SUBNET="${GAME_SUBNET:-10.203.1.0/24}"
GAME_GATEWAY="${GAME_GATEWAY:-10.203.1.1/24}"
DHCP_START="${DHCP_START:-10.203.1.2}"
DHCP_END="${DHCP_END:-10.203.1.250}"
AP_SSID="${AP_SSID:-GofroNET WiFi}"
AP_CHANNEL="${AP_CHANNEL:-36}"
PI_AGENT_BINARY="${PI_AGENT_BINARY:-target/release/pi-agent}"
RELAY_BINARY="${RELAY_BINARY:-target/release/wg-relay}"
UPDATER_BINARY="${UPDATER_BINARY:-target/release/gofro-updater}"
UPDATE_PUBLIC_KEY="${UPDATE_PUBLIC_KEY:-deploy/pi/update-public.pem}"
MIGRATION_SCRIPT="${MIGRATION_SCRIPT:-deploy/pi/migrate.sh}"
die() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}
[[ $EUID -eq 0 ]] || die "run as root"
: "${SERVER_PUBLIC_KEY:?set SERVER_PUBLIC_KEY}"
: "${SERVER_ENDPOINT:?set SERVER_ENDPOINT, for example 203.0.113.10:8443}"
: "${AP_PASSWORD:?set AP_PASSWORD}"
: "${WIFI_COUNTRY:?set WIFI_COUNTRY, for example DE}"
[[ $(uname -m) == aarch64 ]] || die "Raspberry Pi OS must be 64-bit (aarch64)"
[[ $WG_INTERFACE =~ ^[a-zA-Z0-9_.:-]+$ ]] || die "invalid WG_INTERFACE"
[[ $WG_ADDRESS =~ ^[0-9a-fA-F:./]+$ ]] || die "invalid WG_ADDRESS"
[[ $GAME_SUBNET =~ ^[0-9.]+/[0-9]+$ ]] || die "invalid GAME_SUBNET"
[[ $GAME_GATEWAY =~ ^[0-9.]+/[0-9]+$ ]] || die "invalid GAME_GATEWAY"
[[ ${GAME_SUBNET##*/} == 24 && ${GAME_GATEWAY##*/} == 24 ]] || die "only /24 game networks are supported"
[[ $SERVER_PUBLIC_KEY =~ ^[a-zA-Z0-9+/]{43}=$ ]] || die "invalid SERVER_PUBLIC_KEY"
[[ $SERVER_ENDPOINT != *$'\n'* && $SERVER_ENDPOINT != *' '* ]] || die "invalid SERVER_ENDPOINT"
[[ $SERVER_ENDPOINT != *'"'* && $SERVER_ENDPOINT != *'\\'* ]] || die "invalid SERVER_ENDPOINT"
[[ $AP_SSID != *$'\n'* ]] || die "invalid AP_SSID"
[[ $AP_SSID != *'"'* && $AP_SSID != *'\\'* ]] || die "invalid AP_SSID"
(( ${#AP_SSID} >= 1 && ${#AP_SSID} <= 32 )) || die "AP_SSID must contain 1-32 characters"
(( ${#AP_PASSWORD} >= 8 && ${#AP_PASSWORD} <= 63 )) || die "AP_PASSWORD must contain 8-63 characters"
[[ $WIFI_COUNTRY =~ ^[A-Z]{2}$ ]] || die "WIFI_COUNTRY must be a two-letter uppercase code"
[[ -f $PI_AGENT_BINARY ]] || die "build pi-agent first or set PI_AGENT_BINARY"
[[ -f $RELAY_BINARY ]] || die "build wg-relay first or set RELAY_BINARY"
[[ -f $UPDATER_BINARY ]] || die "build gofro-updater first or set UPDATER_BINARY"
[[ -f $UPDATE_PUBLIC_KEY ]] || die "update public key not found"
[[ -f $MIGRATION_SCRIPT ]] || die "migration script not found"
GAME_PREFIX=${GAME_SUBNET%.*}
IFS=. read -r o1 o2 o3 extra <<< "$GAME_PREFIX"
[[ -z ${extra:-} ]] || die "GAME_SUBNET must contain four IPv4 octets"
for octet in "$o1" "$o2" "$o3"; do [[ $octet =~ ^(0|[1-9][0-9]{0,2})$ ]] && (( 10#$octet <= 255 )) || die "invalid GAME_SUBNET octet"; done
[[ $GAME_SUBNET == "${GAME_PREFIX}.0/24" ]] || die "GAME_SUBNET must be a canonical /24 network"
[[ $GAME_GATEWAY == "${GAME_PREFIX}.1/24" ]] || die "GAME_GATEWAY must be the first host in GAME_SUBNET"
[[ $DHCP_START == "${GAME_PREFIX}.2" && $DHCP_END == "${GAME_PREFIX}.250" ]] || die "DHCP range must cover hosts .2 through .250"
read -ra route <<< "$(ip -4 route get 1.1.1.1)"
for ((i = 0; i < ${#route[@]} - 1; i++)); do
  if [[ ${route[i]} == dev ]]; then
    UPLINK_INTERFACE=${route[i + 1]}
    break
  fi
done
[[ ${UPLINK_INTERFACE:-} != wlan0 ]] || die "refusing to replace wlan0 while it is the active uplink; connect Ethernet first"
apt-get update
apt-get install -y --no-install-recommends network-manager wireguard-tools dnsmasq nftables iw curl ca-certificates openssl coreutils tar util-linux
systemctl is-active --quiet NetworkManager || die "NetworkManager is not active"
ip link show wlan0 >/dev/null 2>&1 || die "wlan0 not found"
if command -v raspi-config >/dev/null; then
  raspi-config nonint do_wifi_country "$WIFI_COUNTRY"
else
  iw reg set "$WIFI_COUNTRY"
fi
GAME_GATEWAY_IP=${GAME_GATEWAY%/*}
nmcli connection delete maxos-game-ap >/dev/null 2>&1 || true
nmcli connection add type wifi ifname wlan0 con-name maxos-game-ap ssid "$AP_SSID"
nmcli connection modify maxos-game-ap \
  802-11-wireless.mode ap \
  802-11-wireless.band a \
  802-11-wireless.channel "$AP_CHANNEL" \
  802-11-wireless.powersave 2 \
  wifi-sec.key-mgmt wpa-psk \
  wifi-sec.psk "$AP_PASSWORD" \
  ipv4.method manual \
  ipv4.addresses "$GAME_GATEWAY" \
  ipv4.never-default yes \
  ipv6.method disabled \
  connection.autoconnect no
cat > /etc/dnsmasq.d/maxos-game-tunnel.conf <<EOF
interface=wlan0
bind-dynamic
no-resolv
server=1.1.1.1@${GAME_GATEWAY_IP}
server=8.8.8.8@${GAME_GATEWAY_IP}
address=/gofrowifi.net/${GAME_GATEWAY_IP}
local=/gofrowifi.net/
dhcp-range=${DHCP_START},${DHCP_END},255.255.255.0,12h
dhcp-option=3,${GAME_GATEWAY_IP}
dhcp-option=6,${GAME_GATEWAY_IP}
EOF
systemctl enable dnsmasq
install -d -m 700 /etc/wireguard
if [[ ! -s /etc/wireguard/client.key ]]; then
  umask 077
  wg genkey > /etc/wireguard/client.key
fi
wg pubkey < /etc/wireguard/client.key > /etc/wireguard/client.pub
cat > "/etc/wireguard/${WG_INTERFACE}.conf" <<EOF
[Interface]
Address = ${WG_ADDRESS}
PrivateKey = $(< /etc/wireguard/client.key)
ListenPort = 51821
MTU = 1420
Table = off
PostUp = sysctl -w net.ipv4.conf.%i.rp_filter=2; ip route replace default dev %i table 100 metric 10
PostDown = ip route del default dev %i table 100 metric 10 || true
[Peer]
PublicKey = ${SERVER_PUBLIC_KEY}
Endpoint = 127.0.0.1:51822
AllowedIPs = 0.0.0.0/0
PersistentKeepalive = 10
EOF
chmod 600 "/etc/wireguard/${WG_INTERFACE}.conf"
cat > /etc/sysctl.d/99-maxos-game-tunnel.conf <<'EOF'
net.ipv4.ip_forward = 1
EOF
sysctl -w net.ipv4.ip_forward=1 >/dev/null
install -d -m 700 /etc/maxos-game-tunnel
if [[ ! -s /etc/maxos-game-tunnel/controller.json ]]; then
  cat > /etc/maxos-game-tunnel/controller.json <<EOF
{
  "vpn_enabled": true,
  "active_server_key": "${SERVER_PUBLIC_KEY}",
  "ap_ssid": "${AP_SSID}",
  "servers": [
    {
      "name": "Primary VPS",
      "endpoint": "${SERVER_ENDPOINT}",
      "public_key": "${SERVER_PUBLIC_KEY}"
    }
  ]
}
EOF
  chmod 600 /etc/maxos-game-tunnel/controller.json
fi
printf '%s\n' "$SERVER_ENDPOINT" > /etc/maxos-game-tunnel/relay-endpoint
cat > /etc/maxos-game-tunnel/pi-vpn.nft <<EOF
table inet maxos_pi {
  chain input {
    type filter hook input priority filter; policy accept;
    iifname "wlan0" udp dport { 53, 67 } accept
    iifname "wlan0" tcp dport { 53, 80, 8080 } accept
    iifname "wlan0" ip protocol icmp accept
    iifname "wlan0" drop
  }
  chain forward {
    type filter hook forward priority filter; policy accept;
    iifname "${WG_INTERFACE}" oifname "wlan0" ct state established,related accept
    iifname "wlan0" oifname "${WG_INTERFACE}" accept
    iifname "wlan0" drop
    oifname "wlan0" drop
  }
}
EOF
cat > /etc/maxos-game-tunnel/pi-bypass.nft <<EOF
table inet maxos_pi {
  chain input {
    type filter hook input priority filter; policy accept;
    iifname "wlan0" udp dport { 53, 67 } accept
    iifname "wlan0" tcp dport { 53, 80, 8080 } accept
    iifname "wlan0" ip protocol icmp accept
    iifname "wlan0" drop
  }
  chain forward {
    type filter hook forward priority filter; policy accept;
    iifname "${UPLINK_INTERFACE}" oifname "wlan0" ct state established,related accept
    iifname "wlan0" oifname "${UPLINK_INTERFACE}" accept
    iifname "wlan0" drop
    oifname "wlan0" drop
  }

  chain postrouting {
    type nat hook postrouting priority srcnat; policy accept;
    oifname "${UPLINK_INTERFACE}" ip saddr ${GAME_SUBNET} masquerade
  }
}
EOF
cat > /usr/local/lib/maxos-game-mode <<EOF
#!/usr/bin/env bash
set -euo pipefail
case "\${1:-}" in
  vpn)
    while ip rule del pref 90 from ${GAME_GATEWAY_IP} table 100 2>/dev/null; do :; done
    while ip rule del pref 100 iif wlan0 table 100 2>/dev/null; do :; done
    ip rule add pref 90 from ${GAME_GATEWAY_IP} table 100
    ip rule add pref 100 iif wlan0 table 100
    nft delete table inet maxos_pi 2>/dev/null || true
    nft -f /etc/maxos-game-tunnel/pi-vpn.nft
    ;;
  bypass)
    while ip rule del pref 90 from ${GAME_GATEWAY_IP} table 100 2>/dev/null; do :; done
    while ip rule del pref 100 iif wlan0 table 100 2>/dev/null; do :; done
    nft delete table inet maxos_pi 2>/dev/null || true
    nft -f /etc/maxos-game-tunnel/pi-bypass.nft
    ;;
  *)
    printf 'usage: %s vpn|bypass\n' "\$0" >&2
    exit 2
    ;;
esac
EOF
chmod 755 /usr/local/lib/maxos-game-mode

cat > /usr/local/lib/maxos-game-network <<EOF
#!/usr/bin/env bash
set -euo pipefail
nmcli connection down maxos-game-ap >/dev/null 2>&1 || true
ip route replace ${GAME_SUBNET} dev wlan0 table 100
ip route replace unreachable default table 100 metric 32767
nft delete table inet maxos_pi 2>/dev/null || true
nft -f /etc/maxos-game-tunnel/pi-vpn.nft
nmcli connection up maxos-game-ap
/usr/local/lib/maxos-game-mode vpn
EOF
chmod 755 /usr/local/lib/maxos-game-network

cat > /etc/systemd/system/maxos-game-network.service <<EOF
[Unit]
Description=MaxOS gaming network routing and kill switch
After=NetworkManager.service nftables.service
Before=wg-quick@${WG_INTERFACE}.service

[Service]
Type=oneshot
RemainAfterExit=yes
ExecStart=/usr/local/lib/maxos-game-network

[Install]
WantedBy=multi-user.target
EOF

install -d -m 755 "/etc/systemd/system/wg-quick@${WG_INTERFACE}.service.d"
cat > "/etc/systemd/system/wg-quick@${WG_INTERFACE}.service.d/maxos.conf" <<EOF
[Unit]
Requires=maxos-game-network.service maxos-wg-relay-client.service
After=maxos-game-network.service maxos-wg-relay-client.service
EOF

install -d -m 755 /etc/systemd/system/dnsmasq.service.d
cat > /etc/systemd/system/dnsmasq.service.d/maxos.conf <<EOF
[Unit]
Wants=maxos-game-network.service
After=maxos-game-network.service
EOF

install -m 755 "$RELAY_BINARY" /usr/local/bin/wg-relay
cat > /etc/systemd/system/maxos-wg-relay-client.service <<EOF
[Unit]
Description=MaxOS obfuscated WireGuard transport
After=network-online.target
Wants=network-online.target
Before=wg-quick@${WG_INTERFACE}.service

[Service]
ExecStart=/usr/local/bin/wg-relay client
Restart=always
RestartSec=1

[Install]
WantedBy=multi-user.target
EOF

install -m 755 "$PI_AGENT_BINARY" /usr/local/bin/pi-agent
cat > /etc/systemd/system/pi-agent.service <<EOF
[Unit]
Description=MaxOS gaming tunnel controller
After=network-online.target wg-quick@${WG_INTERFACE}.service
Wants=network-online.target

[Service]
ExecStart=/usr/local/bin/pi-agent --listen ${GAME_GATEWAY_IP}:80 --interface ${WG_INTERFACE}
Restart=on-failure
RestartSec=2
KillSignal=SIGINT

[Install]
WantedBy=multi-user.target
EOF

UPDATER_BINARY="$UPDATER_BINARY" UPDATE_PUBLIC_KEY="$UPDATE_PUBLIC_KEY" \
  MIGRATION_SCRIPT="$MIGRATION_SCRIPT" STATUS_ADDRESS="$GAME_GATEWAY_IP" \
  WG_INTERFACE="$WG_INTERFACE" deploy/pi/install-updater.sh
systemctl daemon-reload
systemctl enable maxos-game-network.service maxos-wg-relay-client.service "wg-quick@${WG_INTERFACE}.service" pi-agent.service
systemctl restart maxos-game-network.service
systemctl restart maxos-wg-relay-client.service
systemctl restart "wg-quick@${WG_INTERFACE}.service"
systemctl restart dnsmasq.service
systemctl restart pi-agent.service

CLIENT_PUBLIC_KEY=$(< /etc/wireguard/client.pub)
printf '\nClient public key:\n%s\n\n' "$CLIENT_PUBLIC_KEY"
printf 'Run on the VPS:\n'
printf 'sudo maxos-server add-peer --public-key %q --tunnel-ip %q --subnet %q\n' \
  "$CLIENT_PUBLIC_KEY" "$WG_ADDRESS" "$GAME_SUBNET"
printf '\nConnect to Wi-Fi %q and open http://gofrowifi.net\n' "$AP_SSID"

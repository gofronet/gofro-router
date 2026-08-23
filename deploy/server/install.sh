#!/usr/bin/env bash
set -euo pipefail

WG_INTERFACE="${WG_INTERFACE:-gt0}"
WG_ADDRESS="${WG_ADDRESS:-10.202.0.1/24}"
WG_PORT="${WG_PORT:-51820}"
RELAY_PORT="${RELAY_PORT:-8443}"
GAME_POOL="${GAME_POOL:-10.203.0.0/16}"
SERVER_BINARY="${SERVER_BINARY:-target/release/server}"
RELAY_BINARY="${RELAY_BINARY:-target/release/wg-relay}"

die() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

[[ $EUID -eq 0 ]] || die "run as root"
[[ $WG_INTERFACE =~ ^[a-zA-Z0-9_.:-]+$ ]] || die "invalid WG_INTERFACE"
[[ $WG_ADDRESS =~ ^[0-9a-fA-F:./]+$ ]] || die "invalid WG_ADDRESS"
[[ $GAME_POOL =~ ^[0-9a-fA-F:./]+$ ]] || die "invalid GAME_POOL"
[[ $WG_PORT =~ ^[0-9]+$ ]] && (( WG_PORT > 0 && WG_PORT < 65536 )) || die "invalid WG_PORT"
[[ $RELAY_PORT =~ ^[0-9]+$ ]] && (( RELAY_PORT > 0 && RELAY_PORT < 65536 )) || die "invalid RELAY_PORT"
(( WG_PORT != RELAY_PORT )) || die "WG_PORT and RELAY_PORT must differ"

if [[ -z ${WAN_INTERFACE:-} ]]; then
  read -ra route <<< "$(ip -4 route get 1.1.1.1)"
  for ((i = 0; i < ${#route[@]} - 1; i++)); do
    if [[ ${route[i]} == dev ]]; then
      WAN_INTERFACE=${route[i + 1]}
      break
    fi
  done
fi
[[ ${WAN_INTERFACE:-} =~ ^[a-zA-Z0-9_.:-]+$ ]] || die "set WAN_INTERFACE explicitly"

apt-get update
apt-get install -y --no-install-recommends wireguard-tools nftables

install -d -m 700 /etc/wireguard /etc/maxos-game-tunnel
if [[ -e /etc/wireguard/${WG_INTERFACE}.conf && ! -s /etc/maxos-game-tunnel/server.key ]]; then
  die "/etc/wireguard/${WG_INTERFACE}.conf already exists and is not managed by this installer"
fi
if [[ ! -s /etc/maxos-game-tunnel/server.key ]]; then
  umask 077
  wg genkey > /etc/maxos-game-tunnel/server.key
fi
wg pubkey < /etc/maxos-game-tunnel/server.key > /etc/maxos-game-tunnel/server.pub

if [[ ! -e /etc/wireguard/${WG_INTERFACE}.conf ]]; then
  cat > "/etc/wireguard/${WG_INTERFACE}.conf" <<EOF
[Interface]
Address = ${WG_ADDRESS}
ListenPort = ${WG_PORT}
PrivateKey = $(< /etc/maxos-game-tunnel/server.key)
SaveConfig = true
EOF
  chmod 600 "/etc/wireguard/${WG_INTERFACE}.conf"
fi

cat > /etc/sysctl.d/99-maxos-game-tunnel.conf <<'EOF'
net.ipv4.ip_forward = 1
EOF
sysctl -w net.ipv4.ip_forward=1 >/dev/null

cat > /etc/maxos-game-tunnel/server.nft <<EOF
table inet maxos_server {
  chain input {
    type filter hook input priority filter; policy accept;
    iifname "${WG_INTERFACE}" drop
  }

  chain forward {
    type filter hook forward priority filter; policy accept;
    iifname "${WG_INTERFACE}" oifname "${WAN_INTERFACE}" ip saddr ${GAME_POOL} accept
    iifname "${WAN_INTERFACE}" oifname "${WG_INTERFACE}" ct state established,related accept
    iifname "${WG_INTERFACE}" drop
    oifname "${WG_INTERFACE}" drop
  }

  chain postrouting {
    type nat hook postrouting priority srcnat; policy accept;
    oifname "${WAN_INTERFACE}" ip saddr ${GAME_POOL} masquerade
  }
}
EOF

cat > /etc/systemd/system/maxos-server-firewall.service <<EOF
[Unit]
Description=MaxOS gaming tunnel server firewall
Before=wg-quick@${WG_INTERFACE}.service

[Service]
Type=oneshot
RemainAfterExit=yes
ExecStartPre=-/usr/sbin/nft delete table inet maxos_server
ExecStart=/usr/sbin/nft -f /etc/maxos-game-tunnel/server.nft
ExecStop=-/usr/sbin/nft delete table inet maxos_server

[Install]
WantedBy=multi-user.target
EOF

install -d -m 755 "/etc/systemd/system/wg-quick@${WG_INTERFACE}.service.d"
cat > "/etc/systemd/system/wg-quick@${WG_INTERFACE}.service.d/maxos.conf" <<EOF
[Unit]
Requires=maxos-server-firewall.service
After=maxos-server-firewall.service
EOF

if [[ -f $SERVER_BINARY ]]; then
  install -m 755 "$SERVER_BINARY" /usr/local/bin/maxos-server
else
  printf 'warning: %s not found; install the server CLI later\n' "$SERVER_BINARY" >&2
fi
if [[ -f $RELAY_BINARY ]]; then
  install -m 755 "$RELAY_BINARY" /usr/local/bin/wg-relay
else
  die "$RELAY_BINARY not found; build wg-relay first"
fi

cat > /etc/systemd/system/maxos-wg-relay-server.service <<EOF
[Unit]
Description=MaxOS obfuscated WireGuard transport
Requires=wg-quick@${WG_INTERFACE}.service
After=wg-quick@${WG_INTERFACE}.service

[Service]
ExecStart=/usr/local/bin/wg-relay server --listen 0.0.0.0:${RELAY_PORT} --wireguard 127.0.0.1:${WG_PORT}
Restart=always
RestartSec=1

[Install]
WantedBy=multi-user.target
EOF

systemctl daemon-reload
systemctl enable maxos-server-firewall.service "wg-quick@${WG_INTERFACE}.service" maxos-wg-relay-server.service
systemctl stop maxos-wg-relay-server.service 2>/dev/null || true
systemctl restart maxos-server-firewall.service
systemctl restart "wg-quick@${WG_INTERFACE}.service"
wg set "${WG_INTERFACE}" listen-port "${WG_PORT}"
wg-quick save "${WG_INTERFACE}"
systemctl restart maxos-wg-relay-server.service

printf '\nServer public key:\n%s\n' "$(< /etc/maxos-game-tunnel/server.pub)"
printf 'Allow UDP relay port %s in the VPS/cloud firewall.\n' "$RELAY_PORT"

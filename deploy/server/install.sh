#!/usr/bin/env bash
set -euo pipefail

WG_INTERFACE=${WG_INTERFACE:-gt0}
WG_ADDRESS=${WG_ADDRESS:-10.202.0.1/24}
WG_PORT=${WG_PORT:-51820}
WG_MTU=${WG_MTU:-1280}
RELAY_PORT=${RELAY_PORT:-8443}
SCRIPT_DIR=$(CDPATH='' cd "$(dirname "$0")" && pwd)

die() { printf 'error: %s\n' "$*" >&2; exit 1; }
install_atomic() {
  local source=$1 destination=$2 temporary="$2.tmp.$$"
  install -m 755 "$source" "$temporary" || return 1
  mv -f "$temporary" "$destination" || return 1
}
install_programs() {
  install -d -m 755 /usr/local/bin /usr/local/sbin /etc/gofro || return 1
  install_atomic "$SCRIPT_DIR/root/usr/local/bin/gofro-router-server" /usr/local/bin/gofro-router-server || return 1
  install_atomic "$SCRIPT_DIR/root/usr/local/bin/gofro-relay" /usr/local/bin/gofro-relay || return 1
  install_atomic "$SCRIPT_DIR/root/usr/local/sbin/gofro-server-install" /usr/local/sbin/gofro-server-install || return 1
  install_atomic "$SCRIPT_DIR/root/usr/local/sbin/gofro-managed" /usr/local/sbin/gofro-managed || return 1
}
verify_programs() {
  local version
  version=$(< "$SCRIPT_DIR/VERSION") || return 1
  [[ $(/usr/local/bin/gofro-router-server --version) == "gofro-server $version" ]] || return 1
  [[ $(/usr/local/bin/gofro-relay --version) == "gofro-relay $version" ]] || return 1
}
commit_release() {
  install -m 644 "$SCRIPT_DIR/update-public.pem" /etc/gofro/update-public.pem.tmp || return 1
  mv -f /etc/gofro/update-public.pem.tmp /etc/gofro/update-public.pem || return 1
  install -m 644 "$SCRIPT_DIR/VERSION" /etc/gofro/version.tmp || return 1
  mv -f /etc/gofro/version.tmp /etc/gofro/version || return 1
}

[[ $EUID == 0 ]] || die 'run as root'
case ${1:-} in
  --install) mode=install ;;
  --update) mode=update ;;
  *) die 'usage: install.sh --install|--update' ;;
esac
[[ $(< "$SCRIPT_DIR/TARGET") == x86_64-server-linux-musl ]] || die 'release target mismatch'
[[ -s $SCRIPT_DIR/VERSION && -s $SCRIPT_DIR/update-public.pem ]] || die 'release bundle is incomplete'
[[ -x $SCRIPT_DIR/root/usr/local/bin/gofro-router-server ]] || die 'server binary is missing'
[[ -x $SCRIPT_DIR/root/usr/local/bin/gofro-relay ]] || die 'relay binary is missing'
[[ -x $SCRIPT_DIR/root/usr/local/sbin/gofro-server-install ]] || die 'updater is missing'
[[ -x $SCRIPT_DIR/root/usr/local/sbin/gofro-managed ]] || die 'management command is missing'

if [[ $mode == update ]]; then
  [[ -e /etc/gofro/version ]] || die 'Gofro is not installed'
  backup=$(mktemp -d)
  trap 'rm -rf "$backup"' EXIT
  cp -a /usr/local/bin/gofro-router-server /usr/local/bin/gofro-relay \
    /usr/local/sbin/gofro-server-install "$backup/"
  [[ ! -e /usr/local/sbin/gofro-managed ]] || cp -a /usr/local/sbin/gofro-managed "$backup/"
  cp -a /etc/gofro/update-public.pem /etc/gofro/version "$backup/"
  if install_programs && verify_programs && systemctl restart gofro-relay.service && \
    systemctl is-active --quiet gofro-relay.service && commit_release; then
    exit 0
  fi
  install -m 755 "$backup/gofro-router-server" /usr/local/bin/gofro-router-server
  install -m 755 "$backup/gofro-relay" /usr/local/bin/gofro-relay
  install -m 755 "$backup/gofro-server-install" /usr/local/sbin/gofro-server-install
  if [[ -e $backup/gofro-managed ]]; then
    install -m 755 "$backup/gofro-managed" /usr/local/sbin/gofro-managed
  else
    rm -f /usr/local/sbin/gofro-managed
  fi
  install -m 644 "$backup/update-public.pem" /etc/gofro/update-public.pem
  install -m 644 "$backup/version" /etc/gofro/version
  systemctl restart gofro-relay.service || true
  die 'update failed and was rolled back'
fi

[[ $WG_INTERFACE =~ ^[a-zA-Z0-9_.:-]+$ ]] || die 'invalid WG_INTERFACE'
[[ $WG_ADDRESS =~ ^[0-9a-fA-F:./]+$ ]] || die 'invalid WG_ADDRESS'
if ! [[ $WG_PORT =~ ^[0-9]+$ ]] || ! (( WG_PORT > 0 && WG_PORT < 65536 )); then die 'invalid WG_PORT'; fi
if ! [[ $WG_MTU =~ ^[0-9]+$ ]] || ! (( WG_MTU >= 1280 && WG_MTU <= 65535 )); then die 'invalid WG_MTU'; fi
if ! [[ $RELAY_PORT =~ ^[0-9]+$ ]] || ! (( RELAY_PORT > 0 && RELAY_PORT < 65536 )); then die 'invalid RELAY_PORT'; fi
(( WG_PORT != RELAY_PORT )) || die 'WG_PORT and RELAY_PORT must differ'
if [[ -z ${WAN_INTERFACE:-} ]]; then
  read -r -a route <<< "$(ip -4 route get 1.1.1.1)"
  for ((i = 0; i < ${#route[@]} - 1; i++)); do
    if [[ ${route[i]} == dev ]]; then WAN_INTERFACE=${route[i + 1]}; break; fi
  done
fi
[[ ${WAN_INTERFACE:-} =~ ^[a-zA-Z0-9_.:-]+$ ]] || die 'set WAN_INTERFACE explicitly'

apt-get update
apt-get install -y --no-install-recommends wireguard-tools nftables
install -d -m 700 /etc/wireguard /etc/gofro
if [[ ! -s /etc/gofro/server.key && -s /etc/maxos-game-tunnel/server.key ]]; then
  install -m 600 /etc/maxos-game-tunnel/server.key /etc/gofro/server.key
fi
if [[ -e /etc/wireguard/$WG_INTERFACE.conf && ! -s /etc/gofro/server.key ]]; then
  die "/etc/wireguard/$WG_INTERFACE.conf already exists and is not managed by this installer"
fi
if [[ ! -s /etc/gofro/server.key ]]; then
  umask 077
  wg genkey > /etc/gofro/server.key
fi
wg pubkey < /etc/gofro/server.key > /etc/gofro/server.pub
if [[ ! -e /etc/wireguard/$WG_INTERFACE.conf ]]; then
  cat > "/etc/wireguard/$WG_INTERFACE.conf" <<EOF
[Interface]
Address = $WG_ADDRESS
ListenPort = $WG_PORT
PrivateKey = $(< /etc/gofro/server.key)
SaveConfig = true
EOF
  chmod 600 "/etc/wireguard/$WG_INTERFACE.conf"
fi
cat > /etc/sysctl.d/99-gofro.conf <<'EOF'
net.ipv4.ip_forward = 1
net.core.rmem_max = 4194304
net.core.wmem_max = 4194304
EOF
sysctl -w net.ipv4.ip_forward=1 net.core.rmem_max=4194304 net.core.wmem_max=4194304 >/dev/null
cat > /etc/gofro/server.nft <<EOF
table inet gofro_server {
  chain input { type filter hook input priority filter; policy accept; iifname "$WG_INTERFACE" drop; }
  chain forward {
    type filter hook forward priority filter; policy accept;
    iifname "$WG_INTERFACE" oifname "$WAN_INTERFACE" accept
    iifname "$WAN_INTERFACE" oifname "$WG_INTERFACE" ct state established,related accept
    iifname "$WG_INTERFACE" drop
    oifname "$WG_INTERFACE" drop
  }
  chain postrouting { type nat hook postrouting priority srcnat; policy accept; iifname "$WG_INTERFACE" oifname "$WAN_INTERFACE" masquerade; }
}
EOF
cat > /etc/systemd/system/gofro-firewall.service <<EOF
[Unit]
Description=Gofro Router server firewall
Before=wg-quick@$WG_INTERFACE.service
[Service]
Type=oneshot
RemainAfterExit=yes
ExecStartPre=-/usr/sbin/nft delete table inet gofro_server
ExecStart=/usr/sbin/nft -f /etc/gofro/server.nft
ExecStop=-/usr/sbin/nft delete table inet gofro_server
[Install]
WantedBy=multi-user.target
EOF
install -d -m 755 "/etc/systemd/system/wg-quick@$WG_INTERFACE.service.d"
cat > "/etc/systemd/system/wg-quick@$WG_INTERFACE.service.d/gofro.conf" <<EOF
[Unit]
Requires=gofro-firewall.service
After=gofro-firewall.service
[Service]
ExecStartPost=/usr/sbin/ip link set dev $WG_INTERFACE mtu $WG_MTU
EOF
cat > /etc/systemd/system/gofro-relay.service <<EOF
[Unit]
Description=Gofro Router obfuscated WireGuard transport
Requires=wg-quick@$WG_INTERFACE.service
After=wg-quick@$WG_INTERFACE.service
[Service]
ExecStart=/usr/local/bin/gofro-relay server --listen 0.0.0.0:$RELAY_PORT --wireguard 127.0.0.1:$WG_PORT
Restart=always
RestartSec=1
[Install]
WantedBy=multi-user.target
EOF
install_programs
verify_programs
systemctl disable --now maxos-wg-relay-server.service maxos-server-firewall.service 2>/dev/null || true
rm -f /etc/systemd/system/maxos-wg-relay-server.service /etc/systemd/system/maxos-server-firewall.service \
  "/etc/systemd/system/wg-quick@$WG_INTERFACE.service.d/maxos.conf"
nft delete table inet maxos_server 2>/dev/null || true
systemctl daemon-reload
systemctl enable gofro-firewall.service "wg-quick@$WG_INTERFACE.service" gofro-relay.service
systemctl stop gofro-relay.service 2>/dev/null || true
systemctl restart gofro-firewall.service
systemctl restart "wg-quick@$WG_INTERFACE.service"
wg set "$WG_INTERFACE" listen-port "$WG_PORT"
wg-quick save "$WG_INTERFACE"
systemctl restart gofro-relay.service
systemctl is-active --quiet gofro-relay.service || die 'relay failed to start'
commit_release
printf '\nServer public key:\n%s\n' "$(< /etc/gofro/server.pub)"
printf 'Allow UDP relay port %s in the VPS/cloud firewall.\n' "$RELAY_PORT"

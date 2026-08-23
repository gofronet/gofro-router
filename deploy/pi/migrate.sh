#!/usr/bin/env bash
set -euo pipefail

ACTION=${1:-}
OLD_VERSION=${2:-}
NEW_VERSION=${3:-}
[[ $OLD_VERSION =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || exit 2
[[ $NEW_VERSION =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || exit 2

# Keep `up` and `down` reversible until `commit`. Future releases add only
# version-gated, idempotent migrations here.
API_UNIT=/etc/systemd/system/gofro-updater-api.service
CHECK_UNIT=/etc/systemd/system/gofro-updater-check.service
LOCK_DROPIN=/etc/systemd/system/gofro-updater.service.d/lock.conf
DNSMASQ_CONFIG=/etc/dnsmasq.d/maxos-game-tunnel.conf
NETWORK_SCRIPT=/usr/local/lib/maxos-game-network
BYPASS_RULES=/etc/maxos-game-tunnel/pi-bypass.nft
DHCP_MARKER=/var/lib/maxos-game-tunnel/update/dhcp-pool-migration

crosses_updater_api() {
  dpkg --compare-versions "$OLD_VERSION" lt 0.3.0 &&
    dpkg --compare-versions "$NEW_VERSION" ge 0.3.0
}

crosses_larger_dhcp_pool() {
  dpkg --compare-versions "$OLD_VERSION" lt 0.3.1 &&
    dpkg --compare-versions "$NEW_VERSION" ge 0.3.1
}

preflight_dhcp_pool() {
  local gateway
  local base
  gateway=$(< /etc/maxos-game-tunnel/status-address)
  base=${gateway%.*}
  [[ $gateway == "${base}.1" ]]
  grep -Eq '^dhcp-range=[0-9.]+,[0-9.]+,255\.255\.255\.(248|0),12h$' "$DNSMASQ_CONFIG"
  grep -Eq "${base}.0/(29|24)" "$NETWORK_SCRIPT"
  grep -Eq "${base}.0/(29|24)" "$BYPASS_RULES"
  nmcli connection show maxos-game-ap >/dev/null
}

set_dhcp_pool() {
  local from_prefix=$1
  local to_prefix=$2
  local end=$3
  local netmask=$4
  local gateway
  local base
  preflight_dhcp_pool
  gateway=$(< /etc/maxos-game-tunnel/status-address)
  base=${gateway%.*}
  sed -i "s|^dhcp-range=.*|dhcp-range=${base}.2,${base}.${end},${netmask},12h|" "$DNSMASQ_CONFIG"
  sed -i "s|${base}.0/${from_prefix}|${base}.0/${to_prefix}|g" "$NETWORK_SCRIPT" "$BYPASS_RULES"
  grep -Fxq "dhcp-range=${base}.2,${base}.${end},${netmask},12h" "$DNSMASQ_CONFIG"
  grep -Fq "${base}.0/${to_prefix}" "$NETWORK_SCRIPT"
  grep -Fq "${base}.0/${to_prefix}" "$BYPASS_RULES"
  nmcli connection modify maxos-game-ap ipv4.addresses "${gateway}/${to_prefix}"
  systemctl restart maxos-game-network.service
  systemctl restart dnsmasq.service
}

write_updater_api() {
  local binary=$1
  local temporary
  temporary=$(mktemp "${API_UNIT}.XXXXXX")
  trap 'rm -f "$temporary"' RETURN
  cat > "$temporary" <<'EOF'
[Unit]
Description=Gofro Router updater API
Requires=maxos-game-network.service
After=network-online.target maxos-game-network.service

[Service]
EOF
  printf 'ExecStart=%s --serve\n' "$binary" >> "$temporary"
  cat >> "$temporary" <<'EOF'
Restart=always
RestartSec=2
PrivateTmp=yes
ProtectHome=yes

[Install]
WantedBy=multi-user.target
EOF
  chmod 644 "$temporary"
  mv -f "$temporary" "$API_UNIT"
  trap - RETURN
}

install_updater_api() {
  install -d -m 755 "${LOCK_DROPIN%/*}"
  cat > "$LOCK_DROPIN" <<'EOF'
[Service]
ExecStart=
ExecStart=/usr/bin/flock /run/gofro-updater.lock /usr/local/bin/gofro-updater
ExecStopPost=
ExecStopPost=/usr/bin/flock /run/gofro-updater.lock /usr/local/bin/gofro-updater --recover-runtime
EOF
  cat > "$CHECK_UNIT" <<'EOF'
[Unit]
Description=Check for Gofro Router updates
Requires=gofro-update-recovery.service
Wants=network-online.target
After=network-online.target gofro-update-recovery.service

[Service]
Type=oneshot
ExecStart=/usr/bin/flock /run/gofro-updater.lock /usr/local/bin/gofro-updater --check
ExecStopPost=/usr/bin/flock /run/gofro-updater.lock /usr/local/bin/gofro-updater --recover-runtime
TimeoutStartSec=15min
TimeoutStopSec=15min
PrivateTmp=yes
ProtectHome=yes
EOF
  write_updater_api "$PWD/gofro-updater"
  sed -i 's/tcp dport { 53, 80 }/tcp dport { 53, 80, 8080 }/' /etc/maxos-game-tunnel/pi-vpn.nft /etc/maxos-game-tunnel/pi-bypass.nft
  systemctl daemon-reload
  systemctl enable --now gofro-updater-api.service
  for _ in {1..20}; do
    curl --fail --silent --max-time 1 "http://$(< /etc/maxos-game-tunnel/status-address):8080/api/status" >/dev/null && return
    sleep 0.25
  done
  return 1
}

remove_updater_api() {
  systemctl stop gofro-updater-check.service || true
  systemctl disable --now gofro-updater-api.service || true
  rm -f "$API_UNIT" "$CHECK_UNIT" "$LOCK_DROPIN"
  sed -i 's/tcp dport { 53, 80, 8080 }/tcp dport { 53, 80 }/' /etc/maxos-game-tunnel/pi-vpn.nft /etc/maxos-game-tunnel/pi-bypass.nft
  systemctl daemon-reload
}

case $ACTION in
  up)
    if crosses_updater_api; then install_updater_api; fi
    if crosses_larger_dhcp_pool; then
      preflight_dhcp_pool
      install -m 600 /dev/null "$DHCP_MARKER"
      sync -f "$DHCP_MARKER"
      sync -f "${DHCP_MARKER%/*}"
      set_dhcp_pool 29 24 250 255.255.255.0
    fi
    ;;
  down)
    if crosses_larger_dhcp_pool && [[ -e $DHCP_MARKER ]]; then
      set_dhcp_pool 24 29 6 255.255.255.248
      rm -f "$DHCP_MARKER"
    fi
    if crosses_updater_api; then remove_updater_api; fi
    ;;
  commit)
    if crosses_updater_api; then
      write_updater_api /usr/local/lib/maxos-game-tunnel/current/gofro-updater
      systemctl daemon-reload
    fi
    if crosses_larger_dhcp_pool; then rm -f "$DHCP_MARKER"; fi
    ;;
  *) exit 2 ;;
esac

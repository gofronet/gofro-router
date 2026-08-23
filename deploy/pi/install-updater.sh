#!/usr/bin/env bash
set -euo pipefail

UPDATER_BINARY=${UPDATER_BINARY:-target/release/gofro-updater}
UPDATE_PUBLIC_KEY=${UPDATE_PUBLIC_KEY:-deploy/pi/update-public.pem}
MIGRATION_SCRIPT=${MIGRATION_SCRIPT:-deploy/pi/migrate.sh}
PI_AGENT=${PI_AGENT:-/usr/local/bin/pi-agent}
WG_RELAY=${WG_RELAY:-/usr/local/bin/wg-relay}
WG_INTERFACE=${WG_INTERFACE:-gt0}
STATUS_ADDRESS=${STATUS_ADDRESS:-10.203.1.1}
BOOTSTRAP_VERSION=${BOOTSTRAP_VERSION:-}
ROOT=/usr/local/lib/maxos-game-tunnel
RELEASES=$ROOT/releases
CURRENT=$ROOT/current
STATE=/var/lib/maxos-game-tunnel/update

die() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

binary_version() {
  local reported candidate
  reported=$($1 --version 2>/dev/null) || return 1
  candidate=${reported##* }
  [[ $candidate =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || return 1
  printf '%s\n' "$candidate"
}

[[ $EUID -eq 0 ]] || die "run as root"
[[ $(uname -m) == aarch64 ]] || die "updater requires aarch64 Linux"
[[ $WG_INTERFACE =~ ^[a-zA-Z0-9_.:-]+$ ]] || die "invalid WG_INTERFACE"
IFS=. read -r -a octets <<< "$STATUS_ADDRESS"
[[ ${#octets[@]} == 4 ]] || die "invalid STATUS_ADDRESS"
for octet in "${octets[@]}"; do
  [[ $octet =~ ^[0-9]+$ ]] && (( 10#$octet <= 255 )) || die "invalid STATUS_ADDRESS"
done
for file in "$UPDATER_BINARY" "$UPDATE_PUBLIC_KEY" "$MIGRATION_SCRIPT" "$PI_AGENT" "$WG_RELAY"; do
  [[ -f $file ]] || die "$file not found"
done
"$UPDATER_BINARY" --self-check >/dev/null
SOURCE_VERSION=$(binary_version "$PI_AGENT" || true)
if [[ -z $SOURCE_VERSION ]]; then
  [[ $BOOTSTRAP_VERSION =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "set BOOTSTRAP_VERSION for a legacy agent"
  SOURCE_VERSION=$BOOTSTRAP_VERSION
fi

install -d -m 755 "$RELEASES" "$STATE" /etc/maxos-game-tunnel
install -m 644 "$UPDATE_PUBLIC_KEY" /etc/maxos-game-tunnel/update-public.pem
printf '%s\n' "$STATUS_ADDRESS" > /etc/maxos-game-tunnel/status-address.new
chmod 644 /etc/maxos-game-tunnel/status-address.new
mv -f /etc/maxos-game-tunnel/status-address.new /etc/maxos-game-tunnel/status-address

if [[ ! -L $CURRENT ]]; then
  RELEASE=$RELEASES/$SOURCE_VERSION
  install -d -m 755 "$RELEASE"
  install -m 755 "$PI_AGENT" "$RELEASE/pi-agent"
  install -m 755 "$WG_RELAY" "$RELEASE/wg-relay"
  install -m 755 "$UPDATER_BINARY" "$RELEASE/gofro-updater"
  install -m 755 "$MIGRATION_SCRIPT" "$RELEASE/migrate.sh"
  sync -f "$RELEASE"
  TEMP_LINK=$CURRENT.new.$$
  trap 'rm -f "$TEMP_LINK"' EXIT
  ln -s "$RELEASE" "$TEMP_LINK"
  mv -Tf "$TEMP_LINK" "$CURRENT"
  trap - EXIT
fi

RESOLVED=$(readlink -f "$CURRENT")
[[ $RESOLVED == "$RELEASES/"* && ${RESOLVED#"$RELEASES/"} != */* ]] || die "invalid current release link"
[[ -x $CURRENT/pi-agent && -x $CURRENT/wg-relay ]] || die "current release is incomplete"
CURRENT_VERSION=$(binary_version "$CURRENT/pi-agent" || true)
if [[ -z $CURRENT_VERSION ]]; then
  [[ $BOOTSTRAP_VERSION =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "current agent does not report a version"
  CURRENT_VERSION=$BOOTSTRAP_VERSION
fi
[[ ${RESOLVED##*/} == "$CURRENT_VERSION" ]] || die "current release directory does not match its version"
if [[ $CURRENT_VERSION == 0.1.0 ]]; then
  install -m 755 "$UPDATER_BINARY" "$CURRENT/gofro-updater"
  install -m 755 "$MIGRATION_SCRIPT" "$CURRENT/migrate.sh"
else
  [[ -x $CURRENT/gofro-updater && -x $CURRENT/migrate.sh ]] || die "current release is incomplete"
fi
ln -sfn "$CURRENT/pi-agent" /usr/local/bin/pi-agent
ln -sfn "$CURRENT/wg-relay" /usr/local/bin/wg-relay
install -m 755 "$UPDATER_BINARY" /usr/local/bin/gofro-updater
if [[ ! -s $STATE/version ]]; then
  printf '%s\n' "$CURRENT_VERSION" > "$STATE/version.new"
  chmod 644 "$STATE/version.new"
  mv -f "$STATE/version.new" "$STATE/version"
else
  read -r INSTALLED_VERSION < "$STATE/version"
  [[ $INSTALLED_VERSION == "$CURRENT_VERSION" ]] || die "version marker does not match current release"
fi
sync -f "$ROOT"
sync -f "$STATE"
sync -f /usr/local/bin/gofro-updater
sync -f /etc/maxos-game-tunnel

cat > /etc/systemd/system/gofro-update-recovery.service <<EOF
[Unit]
Description=Recover an interrupted Gofro Router update
Before=pi-agent.service maxos-wg-relay-client.service wg-quick@${WG_INTERFACE}.service

[Service]
Type=oneshot
RemainAfterExit=yes
ExecStart=/usr/local/bin/gofro-updater --recover-only
EOF

cat > /etc/systemd/system/gofro-updater.service <<'EOF'
[Unit]
Description=Gofro Router signed updater
Requires=gofro-update-recovery.service
Wants=network-online.target
After=network-online.target gofro-update-recovery.service

[Service]
Type=oneshot
ExecStart=/usr/local/bin/gofro-updater
ExecStopPost=/usr/local/bin/gofro-updater --recover-runtime
TimeoutStartSec=15min
TimeoutStopSec=15min
Nice=10
IOSchedulingClass=idle
PrivateTmp=yes
ProtectHome=yes
EOF

cat > /etc/systemd/system/gofro-updater.timer <<'EOF'
[Unit]
Description=Check for Gofro Router updates

[Timer]
OnBootSec=15min
OnCalendar=*-*-* 00/6:00:00
RandomizedDelaySec=30min
Persistent=true

[Install]
WantedBy=timers.target
EOF

install -d -m 755 /etc/systemd/system/pi-agent.service.d
cat > /etc/systemd/system/pi-agent.service.d/updater.conf <<'EOF'
[Unit]
Requires=gofro-update-recovery.service
After=gofro-update-recovery.service

[Service]
KillSignal=SIGINT
EOF

install -d -m 755 /etc/systemd/system/maxos-wg-relay-client.service.d
cat > /etc/systemd/system/maxos-wg-relay-client.service.d/updater.conf <<'EOF'
[Unit]
Requires=gofro-update-recovery.service
After=gofro-update-recovery.service
EOF

install -d -m 755 "/etc/systemd/system/wg-quick@${WG_INTERFACE}.service.d"
cat > "/etc/systemd/system/wg-quick@${WG_INTERFACE}.service.d/updater.conf" <<'EOF'
[Unit]
Requires=gofro-update-recovery.service
After=gofro-update-recovery.service
EOF

sync -f /etc/systemd/system
systemctl daemon-reload
systemctl enable --now gofro-update-recovery.service gofro-updater.timer

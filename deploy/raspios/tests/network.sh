#!/bin/sh
set -eu

ROOT="$(CDPATH='' cd "$(dirname "$0")/../../.." && pwd)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

grep -Fq 'systemctl disable --now avahi-daemon.service avahi-daemon.socket' \
	"$ROOT/deploy/raspios/install.sh"
grep -Fq 'systemctl disable --now avahi-daemon.service avahi-daemon.socket' \
	"$ROOT/deploy/raspios/root/usr/sbin/gofro-setup"

cat > "$TMP/ip" <<'EOF'
#!/bin/sh
echo "ip $*" >> "$GOFRO_TEST_LOG"
EOF
cat > "$TMP/mode" <<'EOF'
#!/bin/sh
echo "mode $*" >> "$GOFRO_TEST_LOG"
EOF
cat > "$TMP/nmcli" <<'EOF'
#!/bin/sh
echo "nmcli $*" >> "$GOFRO_TEST_LOG"
EOF
cat > "$TMP/iw" <<'EOF'
#!/bin/sh
echo "iw $*" >> "$GOFRO_TEST_LOG"
EOF
chmod +x "$TMP/ip" "$TMP/mode" "$TMP/nmcli" "$TMP/iw"
printf '%s\n' DE > "$TMP/wifi-country"

export GOFRO_TEST_LOG="$TMP/log"
GOFRO_MODE_COMMAND=$TMP/mode GOFRO_NMCLI_COMMAND=$TMP/nmcli \
GOFRO_IW_COMMAND=$TMP/iw GOFRO_WIFI_COUNTRY_FILE=$TMP/wifi-country PATH="$TMP:$PATH" \
	sh "$ROOT/deploy/raspios/root/usr/libexec/gofro/network" start

[ "$(sed -n '1p' "$TMP/log")" = 'iw reg set DE' ]
[ "$(sed -n '2p' "$TMP/log")" = 'ip route replace 10.203.1.0/24 dev wlan0 table 100' ]
[ "$(sed -n '3p' "$TMP/log")" = 'ip route replace unreachable default table 100 metric 32767' ]
[ "$(sed -n '4p' "$TMP/log")" = 'mode vpn' ]
[ "$(sed -n '5p' "$TMP/log")" = 'nmcli connection up gofro-ap' ]

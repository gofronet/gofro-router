#!/bin/sh
set -eu

ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

cat > "$TMP/uci" <<'EOF'
#!/bin/sh
case "$3" in
	gofro.main.lan_interface) echo br-lan ;;
	gofro.main.lan_address) echo 10.203.1.1 ;;
	gofro.main.lan_subnet) echo 10.203.1.0/24 ;;
esac
EOF
cat > "$TMP/ip" <<'EOF'
#!/bin/sh
echo "$*" >> "$GOFRO_TEST_LOG"
case "$*" in rule\ del*) exit 1 ;; esac
EOF
chmod +x "$TMP/uci" "$TMP/ip"

export GOFRO_TEST_LOG="$TMP/log"
PATH="$TMP:$PATH" sh "$ROOT/deploy/openwrt/root/usr/libexec/gofro/mode" vpn
grep -q 'rule add pref 80 fwmark 0x10000/0x30000 lookup main' "$TMP/log"
grep -q 'rule add pref 81 fwmark 0x20000/0x30000 lookup 100' "$TMP/log"
grep -q 'route replace unreachable default table 100 metric 32767' "$TMP/log"

: > "$TMP/log"
PATH="$TMP:$PATH" sh "$ROOT/deploy/openwrt/root/usr/libexec/gofro/mode" bypass
! grep -q 'rule add' "$TMP/log"

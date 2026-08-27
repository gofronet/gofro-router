#!/bin/sh
set -eu

ROOT="$(CDPATH='' cd "$(dirname "$0")/../../.." && pwd)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

cat > "$TMP/ip" <<'EOF'
#!/bin/sh
echo "ip $*" >> "$GOFRO_TEST_LOG"
case "$*" in rule\ del*) exit 1 ;; esac
EOF
cat > "$TMP/nft" <<'EOF'
#!/bin/sh
echo "nft $*" >> "$GOFRO_TEST_LOG"
[ "$1" != -f ] || cat >> "$GOFRO_TEST_LOG"
EOF
chmod +x "$TMP/ip" "$TMP/nft"

export GOFRO_TEST_LOG="$TMP/log"
PATH="$TMP:$PATH" sh "$ROOT/deploy/raspios/root/usr/libexec/gofro/mode" vpn
grep -q 'ip rule add pref 80 fwmark 0x10000/0x30000 lookup main' "$TMP/log"
grep -q 'ip rule add pref 81 fwmark 0x20000/0x30000 lookup 100' "$TMP/log"
grep -q 'ip rule add pref 90 from 10.203.1.0/24 lookup 100' "$TMP/log"
grep -q 'oifname "eth0" ip saddr 10.203.1.0/24 masquerade' "$TMP/log"
grep -q 'tcp dport { 53, 80, 8080 } accept' "$TMP/log"

: > "$TMP/log"
PATH="$TMP:$PATH" sh "$ROOT/deploy/raspios/root/usr/libexec/gofro/mode" bypass
grep -q 'ip rule add' "$TMP/log" && exit 1
grep -q 'iifname "wlan0" oifname "eth0" accept' "$TMP/log"

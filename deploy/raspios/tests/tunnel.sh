#!/bin/sh
set -eu

ROOT="$(CDPATH='' cd "$(dirname "$0")/../../.." && pwd)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

cat > "$TMP/ip" <<'EOF'
#!/bin/sh
echo "ip $*" >> "$GOFRO_TEST_LOG"
case "$*" in 'link show gt0') exit 1 ;; esac
EOF
cat > "$TMP/sysctl" <<'EOF'
#!/bin/sh
echo "sysctl $*" >> "$GOFRO_TEST_LOG"
EOF
cat > "$TMP/wg" <<'EOF'
#!/bin/sh
echo "wg $*" >> "$GOFRO_TEST_LOG"
case "$1" in showconf) printf '[Interface]\nPrivateKey = test\n' ;; esac
EOF
chmod +x "$TMP/ip" "$TMP/sysctl" "$TMP/wg"
printf '[Interface]\nPrivateKey = test\n' > "$TMP/gt0.conf"

export GOFRO_TEST_LOG="$TMP/log"
GOFRO_WIREGUARD_DIR=$TMP PATH="$TMP:$PATH" \
	sh "$ROOT/deploy/raspios/root/usr/libexec/gofro/tunnel" start gt0
grep -q 'ip link add dev gt0 type wireguard' "$TMP/log"
grep -q 'sysctl -w net.ipv4.conf.gt0.rp_filter=2' "$TMP/log"
grep -q "wg setconf gt0 $TMP/gt0.conf" "$TMP/log"
grep -q 'ip link set mtu 1280 up dev gt0' "$TMP/log"
grep -q 'ip route replace default dev gt0 table 100 metric 10' "$TMP/log"

GOFRO_WIREGUARD_DIR=$TMP PATH="$TMP:$PATH" \
	sh "$ROOT/deploy/raspios/root/usr/libexec/gofro/tunnel" save gt0
grep -q 'wg showconf gt0' "$TMP/log"
grep -q 'PrivateKey = test' "$TMP/gt0.conf"

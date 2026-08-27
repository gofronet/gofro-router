#!/bin/sh
set -eu

ROOT="$(CDPATH='' cd "$(dirname "$0")/../../.." && pwd)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

cat > "$TMP/uci" <<'EOF'
#!/bin/sh
case "$3" in
	gofro.main.interface) echo gt0 ;;
	gofro.main.lan_interface) echo br-lan ;;
esac
EOF
cat > "$TMP/mode" <<'EOF'
#!/bin/sh
echo "mode $*" >> "$GOFRO_TEST_LOG"
EOF
cat > "$TMP/ip" <<'EOF'
#!/bin/sh
echo "ip $*" >> "$GOFRO_TEST_LOG"
EOF
cat > "$TMP/jsonfilter" <<'EOF'
#!/bin/sh
printf '%s\n' "$GOFRO_TEST_VPN"
EOF
chmod +x "$TMP/uci" "$TMP/mode" "$TMP/ip" "$TMP/jsonfilter"
export GOFRO_TEST_LOG="$TMP/log"

GOFRO_TEST_VPN=true ACTION=ifup INTERFACE=lan DEVICE=br-lan GOFRO_MODE_COMMAND=$TMP/mode PATH="$TMP:$PATH" \
	sh "$ROOT/deploy/openwrt/root/etc/hotplug.d/iface/90-gofro-route"
grep -q '^mode vpn$' "$TMP/log"

GOFRO_TEST_VPN=false ACTION=ifup INTERFACE=lan DEVICE=br-lan GOFRO_MODE_COMMAND=$TMP/mode PATH="$TMP:$PATH" \
	sh "$ROOT/deploy/openwrt/root/etc/hotplug.d/iface/90-gofro-route"
grep -q '^mode bypass$' "$TMP/log"

ACTION=ifup INTERFACE=gt0 DEVICE=gt0 GOFRO_MODE_COMMAND=$TMP/mode PATH="$TMP:$PATH" \
	sh "$ROOT/deploy/openwrt/root/etc/hotplug.d/iface/90-gofro-route"
grep -q '^ip route replace default dev gt0 table 100 metric 10$' "$TMP/log"

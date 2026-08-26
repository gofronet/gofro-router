#!/bin/sh
set -eu

ROOT="$(CDPATH='' cd "$(dirname "$0")/../../.." && pwd)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

cat > "$TMP/uci" <<'EOF'
#!/bin/sh
case "$*" in
	'show wireless')
		printf '%s\n' 'wireless.ap2=wifi-iface'
		[ "${GOFRO_TEST_SINGLE_BAND:-0}" = 1 ] || printf '%s\n' 'wireless.ap5=wifi-iface'
		printf '%s\n' 'wireless.station=wifi-iface'
		;;
	'-q get wireless.ap2.mode'|'-q get wireless.ap5.mode') echo ap ;;
	'-q get wireless.station.mode') echo sta ;;
	'-q get wireless.ap2.device') echo radio0 ;;
	'-q get wireless.ap5.device') echo radio1 ;;
	'-q get wireless.radio0.band') echo 2g ;;
	'-q get wireless.radio1.band') echo 5g ;;
	'-q get wireless.ap2.ssid') echo 'Old 2' ;;
	'-q get wireless.ap5.ssid') echo 'Old 5' ;;
	set*|'commit wireless') printf '%s\n' "$*" >> "$GOFRO_TEST_LOG" ;;
esac
EOF
cat > "$TMP/wifi" <<'EOF'
#!/bin/sh
:
EOF
cat > "$TMP/sleep" <<'EOF'
#!/bin/sh
:
EOF
chmod +x "$TMP/uci" "$TMP/wifi" "$TMP/sleep"

export GOFRO_TEST_LOG="$TMP/log"
output="$(PATH="$TMP:$PATH" sh "$ROOT/deploy/openwrt/root/usr/libexec/gofro/wifi" list)"
[ "$output" = "$(printf '2g\tOld 2\n5g\tOld 5')" ]
output="$(GOFRO_TEST_SINGLE_BAND=1 PATH="$TMP:$PATH" sh "$ROOT/deploy/openwrt/root/usr/libexec/gofro/wifi" list)"
[ "$output" = "$(printf '2g\tOld 2')" ]

PATH="$TMP:$PATH" sh "$ROOT/deploy/openwrt/root/usr/libexec/gofro/wifi" \
	set 5g 'GofroWIFI 5' 'secret123'
grep -q '^set wireless\.ap5\.ssid=GofroWIFI 5$' "$TMP/log"
grep -q '^set wireless\.ap5\.key=secret123$' "$TMP/log"
if grep -q 'wireless\.ap2' "$TMP/log"; then exit 1; fi

: > "$TMP/log"
PATH="$TMP:$PATH" sh "$ROOT/deploy/openwrt/root/usr/libexec/gofro/wifi" \
	set 2g 'GofroWIFI 2'
grep -q '^set wireless\.ap2\.ssid=GofroWIFI 2$' "$TMP/log"
if grep -q '\.key=' "$TMP/log"; then exit 1; fi
if grep -q 'wireless\.ap5' "$TMP/log"; then exit 1; fi

: > "$TMP/log"
if GOFRO_TEST_SINGLE_BAND=1 PATH="$TMP:$PATH" \
	sh "$ROOT/deploy/openwrt/root/usr/libexec/gofro/wifi" set 5g Missing 2>/dev/null; then
	exit 1
fi
[ ! -s "$TMP/log" ]

PATH="$TMP:$PATH" sh "$ROOT/deploy/openwrt/root/usr/libexec/gofro/wifi" \
	set all 'Legacy name'
grep -q '^set wireless\.ap2\.ssid=Legacy name$' "$TMP/log"
grep -q '^set wireless\.ap5\.ssid=Legacy name$' "$TMP/log"

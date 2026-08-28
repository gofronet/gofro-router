#!/bin/sh
set -eu

ROOT="$(CDPATH='' cd "$(dirname "$0")/../../.." && pwd)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

cat > "$TMP/uci" <<'EOF'
#!/bin/sh
[ "$*" = '-q get gofro.main.interface' ]
printf '%s\n' gt0
EOF
cat > "$TMP/ifup" <<'EOF'
#!/bin/sh
[ "$*" = gt0 ]
EOF
cat > "$TMP/ip" <<'EOF'
#!/bin/sh
case "$*" in
	'link show gt0') exit 0 ;;
	'-o -4 address show dev gt0 scope global')
		count=0
		[ ! -f "$GOFRO_TEST_COUNT" ] || IFS= read -r count < "$GOFRO_TEST_COUNT"
		count=$((count + 1))
		printf '%s\n' "$count" > "$GOFRO_TEST_COUNT"
		[ "$count" -lt 2 ] || printf '8: gt0 inet 10.202.0.4/32 scope global gt0\n'
		;;
	*) exit 1 ;;
esac
EOF
cat > "$TMP/wg" <<'EOF'
#!/bin/sh
[ "$*" = 'showconf gt0' ]
printf '%s\n' '[Interface]'
EOF
cat > "$TMP/sleep" <<'EOF'
#!/bin/sh
exit 0
EOF
chmod +x "$TMP/uci" "$TMP/ifup" "$TMP/ip" "$TMP/wg" "$TMP/sleep"

GOFRO_TEST_COUNT=$TMP/count GOFRO_WIREGUARD_DIR=$TMP/wireguard PATH="$TMP:$PATH" \
	sh "$ROOT/deploy/openwrt/root/usr/libexec/gofro/tunnel" start gt0
[ "$(cat "$TMP/count")" = 2 ]
grep -Fxq '[Interface]' "$TMP/wireguard/gt0.conf"

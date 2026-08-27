#!/bin/sh
set -eu

ROOT="$(CDPATH='' cd "$(dirname "$0")/../../.." && pwd)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

cat > "$TMP/nmcli" <<'EOF'
#!/bin/sh
echo "$*" >> "$GOFRO_TEST_LOG"
case "$*" in *--get-values*) printf 'GofroWIFI 5\n' ;; esac
EOF
cat > "$TMP/sleep" <<'EOF'
#!/bin/sh
exit 0
EOF
chmod +x "$TMP/nmcli" "$TMP/sleep"

export GOFRO_TEST_LOG="$TMP/log"
output="$(PATH="$TMP:$PATH" sh "$ROOT/deploy/raspios/root/usr/libexec/gofro/wifi" list)"
[ "$output" = "$(printf '5g\tGofroWIFI 5')" ]
PATH="$TMP:$PATH" sh "$ROOT/deploy/raspios/root/usr/libexec/gofro/wifi" set 5g Gaming password123
grep -q 'connection modify gofro-ap 802-11-wireless.ssid Gaming' "$TMP/log"
grep -q 'connection modify gofro-ap wifi-sec.psk password123' "$TMP/log"
PATH="$TMP:$PATH" sh "$ROOT/deploy/raspios/root/usr/libexec/gofro/wifi" set 2g Unsupported 2>/dev/null && exit 1

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
cat > "$TMP/onboarding" <<'EOF'
#!/bin/sh
printf '%s\n' "$*" >> "$GOFRO_TEST_LOG"
cat >> "$GOFRO_TEST_STDIN"
EOF
chmod +x "$TMP/nmcli" "$TMP/onboarding"

export GOFRO_TEST_LOG="$TMP/log"
export GOFRO_STATE_DIR="$TMP/state"
export GOFRO_ONBOARDING_COMMAND="$TMP/onboarding"
export GOFRO_TEST_STDIN="$TMP/stdin"
mkdir "$GOFRO_STATE_DIR"
output="$(PATH="$TMP:$PATH" sh "$ROOT/deploy/raspios/root/usr/libexec/gofro/wifi" list)"
[ "$output" = "$(printf '5g\tGofroWIFI 5')" ]
printf '%s\n' password123 | PATH="$TMP:$PATH" sh "$ROOT/deploy/raspios/root/usr/libexec/gofro/wifi" set 5g Gaming
grep -q 'set 5g Gaming' "$TMP/log"
[ "$(cat "$TMP/stdin")" = password123 ]
grep -q password123 "$TMP/log" && exit 1
if PATH="$TMP:$PATH" sh "$ROOT/deploy/raspios/root/usr/libexec/gofro/wifi" set 2g Unsupported 2>/dev/null; then
	exit 1
fi

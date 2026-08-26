#!/bin/sh
set -eu

ROOT="$(CDPATH='' cd "$(dirname "$0")/../../.." && pwd)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

sed -n \
	-e '/^write_result() {$/,/^}$/p' \
	-e '/^run_update() {$/,/^}$/p' \
	"$ROOT/deploy/openwrt/root/usr/libexec/gofro/update" > "$TMP/functions.sh"
# shellcheck disable=SC1091
. "$TMP/functions.sh"

STATE_DIR=$TMP/state
# Referenced by the sourced functions; ShellCheck cannot follow the generated file.
# shellcheck disable=SC2034
TRIGGER=$STATE_DIR/update-request
RESULT=$STATE_DIR/update-result
# shellcheck disable=SC2034
LOG=$STATE_DIR/update.log
VERSION_FILE=$TMP/version
UPDATE=$TMP/update
# shellcheck disable=SC2034
LOGGER=:
mkdir "$STATE_DIR"
printf '%s\n' 0.4.2 > "$VERSION_FILE"

cat > "$UPDATE" <<'EOF'
#!/bin/sh
exit 0
EOF
chmod +x "$UPDATE"
run_update
[ "$(cat "$RESULT")" = current ]

cat > "$UPDATE" <<EOF
#!/bin/sh
printf '%s\n' 0.4.3 > "$VERSION_FILE"
EOF
chmod +x "$UPDATE"
run_update
[ "$(cat "$RESULT")" = updated ]

cat > "$UPDATE" <<'EOF'
#!/bin/sh
exit 1
EOF
chmod +x "$UPDATE"
run_update
[ "$(cat "$RESULT")" = failed ]

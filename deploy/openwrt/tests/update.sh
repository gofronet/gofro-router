#!/bin/sh
set -eu

ROOT="$(CDPATH='' cd "$(dirname "$0")/../../.." && pwd)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

sed -n \
	-e '/^setup_pending() {$/,/^}$/p' \
	-e '/^write_result() {$/,/^}$/p' \
	-e '/^request_update() {$/,/^}$/p' \
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
SERVICE=$TMP/service
# Referenced by the sourced request helper.
# shellcheck disable=SC2034
LOCK=$TMP/lock
# shellcheck disable=SC2034
ONBOARDING=$TMP/onboarding-state
mkdir "$STATE_DIR"
printf '%s\n' 0.4.2 > "$VERSION_FILE"

cat > "$SERVICE" <<'EOF'
#!/bin/sh
exit 0
EOF
chmod +x "$SERVICE"
request_update
[ -e "$TRIGGER" ]
rm -f "$TRIGGER"
mkdir "$LOCK"
if request_update; then exit 1; fi
[ ! -e "$TRIGGER" ]
rmdir "$LOCK"

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

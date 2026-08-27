#!/bin/sh
set -eu

ROOT="$(CDPATH='' cd "$(dirname "$0")/../../.." && pwd)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

sed -n \
	-e '/^write_result() {$/,/^}$/p' \
	-e '/^request_update() {$/,/^}$/p' \
	-e '/^run_update() {$/,/^}$/p' \
	"$ROOT/deploy/raspios/root/usr/libexec/gofro/update" > "$TMP/functions.sh"
# shellcheck disable=SC1091
. "$TMP/functions.sh"

STATE_DIR=$TMP/state
TRIGGER=$STATE_DIR/update-request
RESULT=$STATE_DIR/update-result
# Referenced by the sourced functions; ShellCheck cannot follow the generated file.
# shellcheck disable=SC2034
LOG=$STATE_DIR/update.log
VERSION_FILE=$TMP/version
UPDATE=$TMP/update
SERVICE=$TMP/systemctl
# Referenced by the sourced request helper.
# shellcheck disable=SC2034
LOCK=$TMP/lock
# shellcheck disable=SC2034
LOGGER=:
mkdir "$STATE_DIR"
printf '%s\n' 0.4.7 > "$VERSION_FILE"

cat > "$SERVICE" <<'EOF'
#!/bin/sh
[ "$*" = 'start gofro-updater.service' ]
EOF
chmod +x "$SERVICE"
request_update
[ -e "$TRIGGER" ]
rm -f "$TRIGGER"

cat > "$UPDATE" <<'EOF'
#!/bin/sh
exit 0
EOF
chmod +x "$UPDATE"
run_update
[ "$(cat "$RESULT")" = current ]

cat > "$UPDATE" <<EOF
#!/bin/sh
printf '%s\n' 0.5.0 > "$VERSION_FILE"
EOF
chmod +x "$UPDATE"
run_update
[ "$(cat "$RESULT")" = updated ]

#!/bin/sh
set -eu

ROOT="$(CDPATH='' cd "$(dirname "$0")/../../.." && pwd)"
SERVICE="$ROOT/deploy/openwrt/root/usr/libexec/gofro/service"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

if "$SERVICE" reboot gofro-relay 2>/dev/null; then exit 1; fi
if "$SERVICE" restart router 2>/dev/null; then exit 1; fi
cat > "$TMP/reboot" <<'EOF'
#!/bin/sh
printf '%s\n' "$*" > "$GOFRO_TEST_REBOOT_LOG"
EOF
chmod +x "$TMP/reboot"
GOFRO_UPDATE_LOCK="$TMP/lock" GOFRO_REBOOT_COMMAND="$TMP/reboot" GOFRO_REBOOT_DELAY=0 GOFRO_TEST_REBOOT_LOG="$TMP/reboot.log" "$SERVICE" reboot router
[ -d "$TMP/lock" ]
while [ ! -e "$TMP/reboot.log" ]; do sleep 1; done
[ "$(cat "$TMP/reboot.log")" = '' ]
if GOFRO_UPDATE_LOCK="$TMP/lock" "$SERVICE" reboot router 2>/dev/null; then exit 1; fi
mkdir "$TMP/trigger"
if GOFRO_UPDATE_LOCK="$TMP/trigger-lock" GOFRO_UPDATE_TRIGGER="$TMP/trigger" "$SERVICE" reboot router 2>/dev/null; then exit 1; fi
[ ! -e "$TMP/trigger-lock" ]
cat > "$TMP/fail" <<'EOF'
#!/bin/sh
exit 1
EOF
chmod +x "$TMP/fail"
GOFRO_UPDATE_LOCK="$TMP/fail-lock" GOFRO_REBOOT_COMMAND="$TMP/fail" GOFRO_REBOOT_DELAY=0 "$SERVICE" reboot router
while [ -e "$TMP/fail-lock" ]; do sleep 1; done
cat > "$TMP/wait" <<'EOF'
#!/bin/sh
printf '%s\n' "$$" > "$GOFRO_TEST_REBOOT_PID"
trap 'exit 1' TERM
while :; do sleep 1; done
EOF
chmod +x "$TMP/wait"
GOFRO_UPDATE_LOCK="$TMP/term-lock" GOFRO_REBOOT_COMMAND="$TMP/wait" GOFRO_REBOOT_DELAY=0 GOFRO_TEST_REBOOT_PID="$TMP/reboot.pid" "$SERVICE" reboot router
while [ ! -e "$TMP/reboot.pid" ]; do sleep 1; done
kill -TERM "$(cat "$TMP/reboot.pid")"
while [ -e "$TMP/term-lock" ]; do sleep 1; done

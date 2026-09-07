#!/bin/sh
set -eu
ROOT="$(CDPATH='' cd "$(dirname "$0")/../../.." && pwd)"
TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT
mkdir "$TMP/state" "$TMP/keyfiles"
if ! command -v flock >/dev/null 2>&1; then
	cat > "$TMP/flock" <<'EOF'
#!/usr/bin/env python3
import fcntl, sys
fcntl.flock(int(sys.argv[-1]), fcntl.LOCK_EX | fcntl.LOCK_NB)
EOF
	chmod +x "$TMP/flock"
	export PATH="$TMP:$PATH"
fi
cat > "$TMP/nmcli" <<'EOF'
#!/bin/sh
printf '%s\n' "$*" >> "$GOFRO_TEST_LOG"
[ "${GOFRO_NMCLI_FAIL_UP:-}" != 1 ] || case "$*" in *'connection up'*) exit 1;; esac
[ "${GOFRO_NMCLI_FAIL_LOAD:-}" != 1 ] || case "$*" in *'connection load'*) exit 1;; esac
case "$*" in
  'connection up gofro-ap') touch "$GOFRO_STATE_DIR/test-active" ;;
  'connection down gofro-ap') rm -f "$GOFRO_STATE_DIR/test-active" ;;
  *'--fields NAME connection show --active'*) [ ! -f "$GOFRO_STATE_DIR/test-active" ] || printf '%s\n' gofro-ap ;;
  *'key-mgmt connection show gofro-ap'*) sed -n 's/^key-mgmt=//p' "$GOFRO_NM_KEYFILE_DIR/gofro-ap.nmconnection" ;;
  *'psk connection show gofro-ap'*) sed -n 's/^psk=//p' "$GOFRO_NM_KEYFILE_DIR/gofro-ap.nmconnection" ;;
esac
EOF
cat > "$TMP/nft" <<'EOF'
#!/bin/sh
printf '%s\n' "$*" >> "$GOFRO_TEST_LOG"
cat >/dev/null || true
EOF
chmod +x "$TMP/nmcli" "$TMP/nft"; export GOFRO_TEST_LOG="$TMP/log"
run() { at=$1; shift; GOFRO_STATE_DIR="$TMP/state" GOFRO_NM_KEYFILE_DIR="$TMP/keyfiles" GOFRO_NMCLI_COMMAND="$TMP/nmcli" GOFRO_NFT_COMMAND="$TMP/nft" GOFRO_APPLY_LOCK="$TMP/lock" GOFRO_PROFILE_UUID=00000000-0000-0000-0000-000000000001 GOFRO_BOOT_ID=00000000-0000-0000-0000-000000000000 GOFRO_UPTIME="$at" sh "$ROOT/deploy/raspios/root/usr/libexec/gofro/onboarding" "$@"; }
GOFRO_ONBOARDING_FRESH=1 run 10 begin
[ "$(cat "$TMP/state/onboarding-state")" = admin ]
grep -q 'ap-isolation=1' "$TMP/keyfiles/gofro-ap.nmconnection"
grep -q 'uuid=00000000-0000-0000-0000-000000000001' "$TMP/keyfiles/gofro-ap.nmconnection"
grep -q '\[wifi-security\]' "$TMP/keyfiles/gofro-ap.nmconnection" && exit 1
printf '%s\n' wifi > "$TMP/state/onboarding-state"
printf 'owner\n' > "$TMP/state/admin-password"
printf '%s\n' '{"networks":[{"password":"pass;\\\"word","ssid":" Office;\" WiFi ","band":"5g"}]}' | run 10 wifi
[ "$(cat "$TMP/state/onboarding-state")" = wifi_applying ]
run 12 watch
[ "$(cat "$TMP/state/onboarding-state")" = wifi_applying ]
GOFRO_NMCLI_FAIL_UP=1 run 13 watch || true
[ "$(cat "$TMP/state/onboarding-state")" = wifi ]
if grep -q '\[wifi-security\]' "$TMP/keyfiles/gofro-ap.nmconnection"; then exit 1; fi
printf '%s\n' '{"networks":[{"password":"pass;\\\"word","ssid":" Office;\" WiFi ","band":"5g"}]}' | run 14 wifi
GOFRO_NMCLI_FAIL_UP=0 run 17 watch
[ "$(cat "$TMP/state/onboarding-state")" = server ]
grep -q '\[wifi-security\]' "$TMP/keyfiles/gofro-ap.nmconnection"
grep -q 'uuid=00000000-0000-0000-0000-000000000001' "$TMP/keyfiles/gofro-ap.nmconnection"
grep -q '\\;' "$TMP/keyfiles/gofro-ap.nmconnection" && exit 1
grep -q 'pass;' "$TMP/log" && exit 1
cp "$TMP/keyfiles/gofro-ap.nmconnection" "$TMP/secured-profile"
printf '%s\n' admin > "$TMP/state/onboarding-state"; printf '%s\n' 'bad 999' > "$TMP/state/onboarding-window"
run 30 watch
grep -q 'connection down gofro-ap' "$TMP/log"
grep -q 'list table inet gofro_setup' "$TMP/log"
printf '%s\n' 1234567890 > "$TMP/state/onboarding-window"
run 30 watch
printf '%s\n' admin > "$TMP/state/onboarding-state"; rm -f "$TMP/state/onboarding-window"
touch "$TMP/state/admin-password"
GOFRO_ONBOARDING_FRESH=1 run 30 begin
[ "$(cat "$TMP/state/onboarding-state")" = wifi ]
rm -f "$TMP/state/onboarding-state"
GOFRO_ONBOARDING_FRESH=1 run 30 begin && exit 1
rm -f "$TMP/state/admin-password"
run 30 network
[ ! -e "$TMP/state/test-active" ]
cp "$TMP/secured-profile" "$TMP/keyfiles/gofro-ap.nmconnection"
run 30 network
[ -e "$TMP/state/test-active" ]
cp "$TMP/keyfiles/gofro-ap.nmconnection" "$TMP/profile.before"
printf '%s\n' changedpass | GOFRO_NMCLI_FAIL_LOAD=1 run 40 set 5g Changed && exit 1
cmp "$TMP/profile.before" "$TMP/keyfiles/gofro-ap.nmconnection"
grep -q changedpass "$TMP/log" && exit 1
grep -q 'iifname != "wlan0" ip daddr 10.203.1.1 drop' "$ROOT/deploy/raspios/root/usr/libexec/gofro/onboarding"
grep -q 'oifname "wlan0" drop' "$ROOT/deploy/raspios/root/usr/libexec/gofro/onboarding"
grep -Fq '[ ! -e /etc/gofro/onboarding-state ] || return 0' "$ROOT/deploy/raspios/install.sh"

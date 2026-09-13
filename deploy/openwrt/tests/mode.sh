#!/bin/sh
set -eu
ROOT="$(CDPATH='' cd "$(dirname "$0")/../../.." && pwd)"
TMP="$(mktemp -d)"; trap '[ -z "${holder:-}" ] || kill -KILL "$holder" 2>/dev/null; rm -rf "$TMP"' EXIT
# macOS has flock(2), but no flock CLI. Exercise the same descriptor lock there.
if ! command -v flock >/dev/null 2>&1; then
cat > "$TMP/flock" <<'EOF'
#!/usr/bin/perl
use Fcntl qw(LOCK_EX LOCK_NB);
open(my $lock, '>&=', 9) or die $!;
flock($lock, LOCK_EX | LOCK_NB) or exit 1;
EOF
chmod +x "$TMP/flock"
fi
cat > "$TMP/ip" <<'EOF'
#!/bin/sh
printf '%s\n' "$*" >> "$GOFRO_TEST_LOG"
case "$*" in
'-4 -N -o route show table 100')
	case "${READ_FAIL:-}" in
		missing) printf '%s\n' 'Error: ipv4: FIB table does not exist.' 'Dump terminated' >&2; exit 2 ;;
		error) echo 'Operation not permitted' >&2; exit 2 ;;
	esac
	printf '%s\n' "${ROUTES:-}" ;;
'-4 -N rule show') printf '%s\n' "${RULES:-}" ;;
esac
EOF
cat > "$TMP/jsonfilter" <<'EOF'
#!/bin/sh
case "$*" in *'@.version'*) echo 0.5.15 ;; *'@.device'*) echo "${OLD_DEVICE:-br-home}" ;; *'@.subnet'*) echo 10.203.1.0/24 ;; esac
EOF
cat > "$TMP/nft" <<'EOF'
#!/bin/sh
[ "${GUARDED:-0}" = 1 ] || exit 1
printf '%s\n' "$*" >> "$GOFRO_TEST_NFT_LOG"
cat <<GUARD
table inet gofro_guard {
 chain gofro_guard {
  type filter hook forward priority filter; policy accept;
  iifname "br-home" oifname != "${GUARD_OUTPUT_DEVICE:-br-home}" drop
 }
}
GUARD
EOF
chmod +x "$TMP/ip" "$TMP/jsonfilter" "$TMP/nft"
export GOFRO_TEST_LOG="$TMP/log" GOFRO_TEST_NFT_LOG="$TMP/nft.log" GOFRO_ROUTING_LEGACY="$TMP/history" GOFRO_MODE_LOCK="$TMP/lock"
PATH="$TMP:$PATH" sh "$ROOT/deploy/openwrt/root/usr/libexec/gofro/mode" vpn br-home 192.168.44.0/24
grep -Fxq 'rule add pref 80 fwmark 0x10000/0x30000 lookup 254' "$TMP/log"
grep -Fxq -- '-4 -N -o route show table 100' "$TMP/log"
grep -Fxq -- '-4 -N rule show' "$TMP/log"
grep -Fxq 'rule add pref 81 fwmark 0x20000/0x30000 lookup 100' "$TMP/log"
grep -Fxq 'route replace unreachable default table 100 metric 32767 proto 186' "$TMP/log"
if grep -q 'pref 90.*rule add' "$TMP/log"; then exit 1; fi
: > "$TMP/log"
PATH="$TMP:$PATH" sh "$ROOT/deploy/openwrt/root/usr/libexec/gofro/mode" bypass br-home 192.168.44.0/24
if grep -q 'rule del' "$TMP/log"; then exit 1; fi
if sh "$ROOT/deploy/openwrt/root/usr/libexec/gofro/mode" vpn 2>/dev/null; then exit 1; fi
mode() { PATH="$TMP:$PATH" sh "$ROOT/deploy/openwrt/root/usr/libexec/gofro/mode" "$@" br-home 192.168.44.0/24; }
reject() {
	: > "$TMP/log"
	if mode vpn 2>"$TMP/error"; then echo 'accepted conflicting ownership' >&2; exit 1; fi
	if grep -Eq '^(rule add|rule del|route replace|route del)' "$TMP/log"; then echo 'mutated before validation' >&2; exit 1; fi
}
for rule in \
	'81: from all fwmark 0x20000/0x30000 lookup 1000' \
	'81: from 192.168.44.0/24 fwmark 0x20000/0x30000 lookup 100' \
	'81: from all fwmark 0x20000/0x30000 lookup 100 suppress_prefixlength 0' \
	'81: from all fwmark 0x20000/0x20000 lookup 100' \
	'80: from all fwmark 0x10000/0x30000 lookup 254 iif eth0'; do
	export RULES="$rule"; reject
done
export RULES='80: from all fwmark 0x10000/0x30000 lookup 254
81: from all fwmark 0x20000/0x30000 lookup 100'
: > "$TMP/log"; mode vpn
if grep -q 'rule add' "$TMP/log"; then exit 1; fi
export RULES="$RULES
81: from all fwmark 0x20000/0x30000 lookup 100"
reject
export RULES=''
for route in 'default via 192.0.2.1 dev eth1 metric 10' '7 default proto 4 metric 32767'; do
	export ROUTES="$route"; reject
done
export ROUTES='default dev foreign proto 1869 scope 253 metric 10'; reject
export ROUTES='192.168.44.0/24 dev br-home proto 186 scope 253
7 default proto 186 metric 32767'
mode vpn; mode check
export ROUTES='10.203.1.0/24 dev br-home proto 3 scope 253
default dev gt0 proto 3 scope 253 metric 10
7 default proto 3 metric 32767'
reject # Matching legacy shapes are not proof of ownership.
: > "$TMP/history"
reject # History without a guard is not enough.
export GUARDED=1 RULES='90: from 10.203.1.0/24 lookup 100'
export GUARD_OUTPUT_DEVICE=gt0
reject
if mode check 2>"$TMP/error"; then exit 1; fi
unset GUARD_OUTPUT_DEVICE
export OLD_DEVICE=br-old
reject
grep -Fq 'requires maintenance' "$TMP/error"
if mode check 2>"$TMP/error"; then exit 1; fi
unset OLD_DEVICE
: > "$TMP/log"; mode check
grep -Fxq -- '-s list chain inet gofro_guard gofro_guard' "$TMP/nft.log"
if grep -Eq '^(rule add|rule del|route replace|route del)' "$TMP/log"; then exit 1; fi
: > "$TMP/log"; mode vpn
grep -Fxq 'route replace 10.203.1.0/24 dev br-home table 100 proto 186' "$TMP/log"
grep -Fxq 'route replace default dev gt0 table 100 metric 10 proto 186' "$TMP/log"
grep -Fxq 'rule del pref 90 from 10.203.1.0/24 lookup 100' "$TMP/log"
grep -Fxq 'route del 10.203.1.0/24 dev br-home table 100 proto 186' "$TMP/log"
grep -Fxq 'route replace 192.168.44.0/24 dev br-home table 100 proto 186' "$TMP/log"
export ROUTES='10.203.1.0/24 dev br-home scope 253
default dev gt0 scope 253 metric 10
7 default metric 32767'
mode vpn
for route in \
	'10.203.1.0/24 dev br-other proto 3 scope 253' \
	'default dev gt0 proto 3 scope 253 metric 11' \
	'default via 192.0.2.1 dev gt0 proto 3 metric 10' \
	'7 default proto 3 metric 32766' \
	'10.203.1.0/24 dev br-home proto 3 scope 253 src 10.203.1.1'; do
	export ROUTES="$route"; reject
done
export ROUTES=''
for rule in \
	'90: from 192.168.44.0/24 lookup 100' \
	'90: from 10.203.1.0/24 lookup 1000' \
	'90: from 10.203.1.0/24 to 192.0.2.0/24 lookup 100'; do
	export RULES="$rule"
	: > "$TMP/log"; mode vpn
	if grep -q 'rule del' "$TMP/log"; then exit 1; fi
done
echo 'PASS exact rule identities, foreign-route refusal and guarded v0.5.15 adoption'
export ROUTES='' RULES=''
export READ_FAIL=error; reject
export READ_FAIL=missing; mode vpn
unset READ_FAIL
# A live owner excludes mode; SIGKILL leaves the file but releases ownership.
mkfifo "$TMP/wait"
(
	exec 9>"$TMP/lock"
	PATH="$TMP:$PATH" flock -n 9
	: > "$TMP/locked"
	read -r _ignored < "$TMP/wait"
) &
holder=$!
while [ ! -e "$TMP/locked" ]; do sleep 0.01; done
reject
kill -KILL "$holder"
wait "$holder" 2>/dev/null || true
holder=
[ -f "$TMP/lock" ]
mode check
for subnet in 192.168.44.1/24 192.168.999.0/24 192.168.44.0/33 192.168.44.0 ''; do
	if PATH="$TMP:$PATH" sh "$ROOT/deploy/openwrt/root/usr/libexec/gofro/mode" vpn br-home "$subnet" 2>/dev/null; then exit 1; fi
done
echo 'PASS production guard, same-device renumbering, device-change refusal and SIGKILL lock release'

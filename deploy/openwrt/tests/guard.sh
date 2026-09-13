#!/bin/sh
set -eu

ROOT="$(CDPATH='' cd "$(dirname "$0")/../../.." && pwd)"
TMP="$(mktemp -d)"
TMP="$(CDPATH='' cd "$TMP" && pwd -P)"
trap 'rm -rf "$TMP"' EXIT
export TEST_ROOT="$TMP" GOFRO_HELPERS="$TMP/bin" GOFRO_GUARD_DIR="$TMP/state" GOFRO_MODE_LOCK="$TMP/lock"
env mkdir "$TMP/bin"
cp "$ROOT/deploy/openwrt/root/usr/libexec/gofro/guard" "$TMP/bin/guard"
cat > "$TMP/bin/stub" <<'EOF'
#!/bin/sh
set -eu
name=${0##*/}
printf '%s:%s\n' "$name" "$*" >> "$TEST_ROOT/log"
case "$name" in
id) echo 0 ;;
stat)
	case "$3" in
	*/guard-device) echo "${TEST_FILE_MODE:-0:600}" ;;
	*/apply.lock) echo "${TEST_APPLY_MODE:-0:600:1}" ;;
	"$TEST_ROOT") echo "${TEST_ANCESTOR_MODE:-0:700}" ;;
	*) echo 0:700 ;; esac ;;
chown|sync) : ;;
flock)
	[ "${TEST_LOCK_FAIL:-0}" = 0 ] || exit 1
	[ "$*" != 8 ] || [ "${TEST_FENCE_FAIL:-0}" = 0 ] || exit 1
	# macOS has flock(2), but no flock CLI. Exercise real inherited-FD locks.
	python3 - "$@" <<'PY'
import fcntl
import sys
args = sys.argv[1:]
flags = fcntl.LOCK_EX
if args[0] == '-n':
    flags |= fcntl.LOCK_NB
    args.pop(0)
assert len(args) == 1 and args[0] in ('7', '8', '9')
try:
    fcntl.flock(int(args[0]), flags)
except BlockingIOError:
    sys.exit(1)
PY
	printf 'flock-acquired:%s\n' "$*" >> "$TEST_ROOT/log" ;;
network)
	[ "${TEST_NETWORK_FAIL:-0}" = 0 ] || exit 1
	printf '%s\n' '{validated snapshot}' ;;
jsonfilter)
	cat >/dev/null
	[ "${TEST_JSON_FAIL:-0}" = 0 ] || exit 126
	case "$2" in
	'@') echo object ;;
	'@["gofro-agent"]') [ "${TEST_SERVICE_ABSENT:-0}" = 0 ] || exit 1; echo "${TEST_SERVICE_TYPE:-object}" ;;
	'@["gofro-agent"].instances') echo "${TEST_INSTANCES_TYPE:-object}" ;;
	'@["gofro-agent"].instances[*]')
		[ -n "${TEST_PIDS:-}" ] || [ "${TEST_REGISTERED:-0}" = 1 ] || exit 1
		echo '{}' ;;
	'@["gofro-agent"].instances[*].pid') printf '%s\n' "${TEST_PIDS:-}" ;;
	'@["gofro-agent"].instances[*].running')
		if [ "${TEST_MISSING_PID:-0}" = 1 ]; then echo true
		else for pid in ${TEST_PIDS:-}; do echo true; done; fi ;;
	'@["gofro-agent"].instances[*].term_timeout') printf '%s\n' "${TEST_TERM_TIMEOUT:-5}" ;;
	'@.device') echo "${TEST_DEVICE:-br-home}" ;;
	'@.subnet') echo "${TEST_SUBNET:-172.22.7.0/24}" ;;
	'@.address') echo "${TEST_ADDRESS:-172.22.7.1}" ;;
	esac ;;
mode)
	[ -s "$GOFRO_GUARD_DIR/guard-device" ]
	[ -s "$TEST_ROOT/armed" ]
	[ "$*" = "check ${TEST_DEVICE:-br-home} ${TEST_SUBNET:-172.22.7.0/24}" ]
	[ "${TEST_MODE_FAIL:-0}" = 0 ] ;;
nft)
	case "$*" in
	'-c -f -'|'-f -')
		cat > "$TEST_ROOT/batch"
		if grep -q '^flush table inet gofro_guard$' "$TEST_ROOT/batch"; then
			grep -Fxq 'add chain inet gofro_guard gofro_guard { type filter hook forward priority filter; policy accept; }' "$TEST_ROOT/batch"
			grep -Fxq 'add rule inet gofro_guard gofro_guard iifname "br-home" oifname != "br-home" drop' "$TEST_ROOT/batch"
			[ "${TEST_ARM_FAIL:-0}" = 0 ] || exit 1
			[ "$1" = -c ] || cp "$TEST_ROOT/batch" "$TEST_ROOT/armed"
		else
			[ "$(cat "$TEST_ROOT/batch")" = 'flush chain inet gofro_routing gofro_dns
delete chain inet gofro_routing gofro_dns' ]
			[ "${TEST_DNS_CHECK_FAIL:-0}" = 0 ] || exit 1
			[ "$1" = -c ] || rm "$TEST_ROOT/dns"
		fi
		cat "$TEST_ROOT/batch" >> "$TEST_ROOT/batches" ;;
	'list tables') [ ! -e "$TEST_ROOT/dns" ] || echo 'table inet gofro_routing' ;;
	'-s list table inet gofro_routing'|'-s -y list chain inet gofro_routing gofro_dns')
		cat "$TEST_ROOT/dns" ;;
	*) echo "unexpected nft command: $*" >&2; exit 1 ;;
	esac ;;
ubus)
	[ "$*" = 'call service list {"name":"gofro-agent"}' ] || exit 1
	[ "${TEST_UBUS_FAIL:-0}" = 0 ] || exit 1
	echo '{"gofro-agent":{"instances":{}}}' ;;
ip|uci|service) echo "forbidden command: $name" >&2; exit 1 ;;
*) exit 1 ;;
esac
EOF
chmod +x "$TMP/bin/stub" "$TMP/bin/guard"
for name in id stat chown sync flock network jsonfilter mode nft ubus ip uci service; do
	ln -s stub "$TMP/bin/$name"
done
export PATH="$TMP/bin:$PATH"
: > "$TMP/log"

reject() {
	if ("$@") > "$TMP/out" 2> "$TMP/error"; then
		echo "unexpected success: $*" >&2; exit 1
	fi
}
reject "$TMP/bin/guard" boot
[ ! -e "$TMP/armed" ]
reject env TEST_NETWORK_FAIL=1 "$TMP/bin/guard" prepare
[ ! -e "$GOFRO_GUARD_DIR/guard-device" ]
reject env TEST_DEVICE='bad";flush ruleset' "$TMP/bin/guard" prepare
reject env TEST_ARM_FAIL=1 "$TMP/bin/guard" prepare
[ ! -e "$GOFRO_GUARD_DIR/guard-device" ]

# Source the actual procd service, stubbing all runtime operations.
. "$ROOT/deploy/openwrt/root/etc/init.d/gofro-agent"
action=start
service_running() { procd_json=clobbered; [ "${TEST_RUNNING:-0}" = 1 ]; }
config_load() { printf 'config\n' >> "$TMP/log"; [ "${TEST_CONFIG_FAIL:-0}" = 0 ]; }
config_get() { interface=${TEST_INTERFACE:-gt0}; }
mkdir() {
	case "$*" in
	'-p /tmp/gofro') : ;;
	"-p $GOFRO_GUARD_DIR") command mkdir "$@" ;;
	*) exit 1 ;;
	esac
}
procd_open_instance() {
	[ -s "$TMP/armed" ] && [ -s "$GOFRO_GUARD_DIR/guard-device" ]
	printf 'launch\n' >> "$TMP/log"
}
procd_set_param() { printf 'procd:%s\n' "$*" >> "$TMP/log"; }
procd_append_param() { procd_set_param "$@"; }
procd_close_instance() {
	writer_blocked || exit 1
	[ "${TEST_LAUNCH_FAIL:-0}" = 0 ]
}
writer_blocked() {
	(
		exec 7>>"$GOFRO_GUARD_DIR/apply.lock"
		if flock -n 7; then echo 'unfenced writer could clear guard' >&2; exit 1; fi
	)
}
writer_available() {
	(exec 7>>"$GOFRO_GUARD_DIR/apply.lock"; flock -n 7)
}
rc_start() {
	start_service
	# rc_procd submits after this return, not after service_started. A fresh
	# controller's nonblocking lock must succeed even if it runs immediately.
	writer_available || exit 1
	printf 'submit\n' >> "$TMP/log"
	writer_available || exit 1
}
procd_kill() {
	writer_blocked || exit 1
	[ -s "$TMP/armed" ] || [ "${TEST_NO_IDENTITY:-0}" = 1 ] || exit 1
	printf 'procd-kill\n' >> "$TMP/log"
	if [ "${TEST_RESPAWN:-0}" = 1 ]; then
		TEST_PIDS=102; export TEST_PIDS
	fi
	if [ "${TEST_POST_ARM_FAIL:-0}" = 1 ]; then TEST_ARM_FAIL=1; export TEST_ARM_FAIL; fi
	if [ "${TEST_POST_QUERY_FAIL:-0}" = 1 ]; then TEST_UBUS_FAIL=1; export TEST_UBUS_FAIL; fi
}
kill() {
	if [ "$#" != 2 ] || [ "$1" != -0 ]; then exit 1; fi
	case "$2" in 101|102) ;; *) exit 1 ;; esac
	printf 'probe:%s\n' "$2" >> "$TMP/log"
	[ -e "$TMP/pid.$2" ]
}
sleep() {
	[ "$*" = 1 ] || exit 1
	writer_blocked || exit 1
	[ -s "$TMP/armed" ] || exit 1
	printf 'wait\n' >> "$TMP/log"
	[ "${TEST_STUCK:-0}" = 0 ] || return 0
	for pid in 101 102; do
		[ -e "$TMP/pid.$pid" ] || continue
		ticks="$(cat "$TMP/pid.$pid")"
		if [ "$ticks" = 1 ]; then rm "$TMP/pid.$pid"
		else printf '%s\n' "$((ticks - 1))" > "$TMP/pid.$pid"; fi
	done
}
rc_stop() {
	# Exact rc.common hook order, deliberately ignoring hook return codes.
	stop_service
	procd_kill
	service_stopped
	writer_available || exit 1
}
procd_json=instances
rc_start
[ "$procd_json" = instances ]
[ "$(cat "$GOFRO_GUARD_DIR/guard-device")" = br-home ]
awk '/^network:/ {network=NR} /^nft:-f -/ {arm=NR} /^mode:/ {mode=NR} /^launch$/ {exit !(network<arm && arm<mode && mode<NR)}' "$TMP/log"

for failure in TEST_CONFIG_FAIL TEST_MODE_FAIL TEST_NETWORK_FAIL; do
	: > "$TMP/log"
	rm "$TMP/armed"
	export "$failure=1"
	reject rc_start
	unset "$failure"
	[ -s "$TMP/armed" ]
	if grep -q '^launch$' "$TMP/log"; then exit 1; fi
done
TEST_LAUNCH_FAIL=1
reject rc_start
unset TEST_LAUNCH_FAIL
[ -s "$TMP/armed" ]
TEST_INTERFACE=foreign; export TEST_INTERFACE
reject rc_start
unset TEST_INTERFACE
reject env TEST_DEVICE=br-new "$TMP/bin/guard" prepare
grep -q 'requires coordinated maintenance' "$TMP/error"
[ "$(cat "$GOFRO_GUARD_DIR/guard-device")" = br-home ]
TEST_SUBNET=192.168.44.0/24 TEST_ADDRESS=192.168.44.7 rc_start
grep -Fq 'procd:command --dns-listen 192.168.44.7:5353' "$TMP/log"

# rc_procd always closes/submits after start_service returns. An explicit
# already-running start must exit the invocation, not submit empty instances.
: > "$TMP/log"
(
	TEST_RUNNING=1
	rc_start
	echo 'empty-submit' >> "$TMP/log"
)
[ ! -s "$TMP/log" ]
restart() {
	printf 'restart\n' >> "$TMP/log"
	rc_stop
	TEST_RUNNING=1 rc_start
}
action=reload
reload_service
grep -qx restart "$TMP/log"
grep -qx launch "$TMP/log"
action=restart
: > "$TMP/log"
restart
grep -qx launch "$TMP/log"
action=start

# The START=18 boot job works without network/ubus and stop only rearms.
. "$ROOT/deploy/openwrt/root/etc/init.d/gofro-guard"
[ "$START" = 18 ]
: > "$TMP/log"
TEST_NETWORK_FAIL=1 start
stop
if grep -Eq '^(network|ubus|ip|uci|service):' "$TMP/log"; then exit 1; fi
reject env TEST_FILE_MODE=1000:600 "$TMP/bin/guard" boot
reject env TEST_LOCK_FAIL=1 "$TMP/bin/guard" boot
printf 'bad;device\n' > "$GOFRO_GUARD_DIR/guard-device"
reject "$TMP/bin/guard" boot
printf 'br-home\n' > "$GOFRO_GUARD_DIR/guard-device"
mv "$GOFRO_GUARD_DIR/guard-device" "$TMP/device"
ln -s "$TMP/device" "$GOFRO_GUARD_DIR/guard-device"
reject "$TMP/bin/guard" boot
rm "$GOFRO_GUARD_DIR/guard-device"
mv "$TMP/device" "$GOFRO_GUARD_DIR/guard-device"

cat > "$TMP/dns" <<'EOF'
table inet gofro_routing {
 chain gofro_dns {
  type nat hook prerouting priority -101; policy accept;
  iifname "br-home" udp dport 53 redirect to :5353
  iifname "br-home" tcp dport 53 redirect to :5353
 }
}
EOF
cp "$TMP/dns" "$TMP/owned-dns"
reject env TEST_DNS_CHECK_FAIL=1 "$TMP/bin/guard" stop
cmp "$TMP/dns" "$TMP/owned-dns"
rc_stop 2> "$TMP/warning"
[ ! -e "$TMP/dns" ]
[ -s "$TMP/armed" ]
grep -q 'existing redirected NAT flows remain' "$TMP/warning"
rc_stop
sed 's/:5353/:9999/g' "$TMP/owned-dns" > "$TMP/dns"
cp "$TMP/dns" "$TMP/foreign-dns"
reject rc_stop
cmp "$TMP/dns" "$TMP/foreign-dns"
sed 's/ udp / meta nfproto ipv4 udp /; s/ tcp / meta nfproto ipv4 tcp /' "$TMP/owned-dns" > "$TMP/dns"
cp "$TMP/dns" "$TMP/foreign-dns"
reject rc_stop
cmp "$TMP/dns" "$TMP/foreign-dns"
sed '/iifname/d' "$TMP/owned-dns" > "$TMP/dns"
rc_stop 2>/dev/null
[ ! -e "$TMP/dns" ]
if grep -Eq '(delete|destroy).*gofro_guard|flush ruleset|conntrack' "$TMP/batches"; then exit 1; fi

# In-flight controller finishes (and clears its guard) before stop obtains 8.
# Both captured procd PIDs then outlive delete; 8 must fence every wait tick.
: > "$TMP/log"
printf '2\n' > "$TMP/pid.101"
printf '3\n' > "$TMP/pid.102"
(
	exec 7>>"$GOFRO_GUARD_DIR/apply.lock"
	flock 7
	: > "$TMP/controller-ready"
	i=0
	while [ ! -e "$TMP/controller-release" ]; do
		i=$((i + 1)); [ "$i" -lt 300 ] || exit 1
		command sleep 0.01
	done
	rm "$TMP/armed"
	printf 'old-transaction-finished\n' >> "$TMP/log"
) &
controller=$!
i=0
while [ ! -e "$TMP/controller-ready" ]; do
	i=$((i + 1)); [ "$i" -lt 300 ]; command sleep 0.01
done
(
	TEST_PIDS='101
102'; export TEST_PIDS
	rc_stop
	printf 'stop-complete\n' >> "$TMP/log"
) &
stopper=$!
i=0
while ! grep -qx 'flock:8' "$TMP/log"; do
	i=$((i + 1)); [ "$i" -lt 300 ]; command sleep 0.01
done
if grep -q '^procd-kill$\|^nft:' "$TMP/log"; then exit 1; fi
: > "$TMP/controller-release"
wait "$controller"
wait "$stopper"
[ -s "$TMP/armed" ]
[ ! -e "$TMP/pid.101" ] && [ ! -e "$TMP/pid.102" ]
awk '/old-transaction-finished/ {old=NR} /^flock-acquired:8$/ {fence=NR}
 /^nft:-f -$/ {arm=NR} /^procd-kill$/ {stopped=NR; if (!(old<fence && fence<arm && arm<NR)) exit 1}
 /^wait$/ {lastwait=NR} /^stop-complete$/ {if (!(stopped<lastwait && lastwait<arm && arm<NR)) exit 1}' "$TMP/log"

# exit, not return: no native kill or replacement submission after failure.
action=restart
for failure in TEST_FENCE_FAIL TEST_UBUS_FAIL TEST_JSON_FAIL TEST_ARM_FAIL TEST_MISSING_PID; do
	: > "$TMP/log"
	export "$failure=1"
	reject restart
	unset "$failure"
	if grep -Eq '^procd-kill$|^submit$' "$TMP/log"; then exit 1; fi
	writer_available
done
for bad in -1 0 1 01 2147483648 '101;kill'; do
	: > "$TMP/log"
	TEST_PIDS=$bad; export TEST_PIDS
	reject restart
	if grep -Eq '^procd-kill$|^submit$|^probe:' "$TMP/log"; then exit 1; fi
done
unset TEST_PIDS
for bad in -1 007 301 junk; do
	: > "$TMP/log"
	TEST_TERM_TIMEOUT=$bad; export TEST_TERM_TIMEOUT
	reject restart
	if grep -Eq '^procd-kill$|^submit$' "$TMP/log"; then exit 1; fi
done
unset TEST_TERM_TIMEOUT
for bad in 1000:600:1 0:600:2 0:666:1; do
	TEST_APPLY_MODE=$bad; export TEST_APPLY_MODE
	reject restart
done
unset TEST_APPLY_MODE
TEST_ANCESTOR_MODE=0:777; export TEST_ANCESTOR_MODE
reject restart
unset TEST_ANCESTOR_MODE
mv "$GOFRO_GUARD_DIR/apply.lock" "$TMP/apply.lock"
ln -s "$TMP/apply.lock" "$GOFRO_GUARD_DIR/apply.lock"
reject restart
rm "$GOFRO_GUARD_DIR/apply.lock"
mv "$TMP/apply.lock" "$GOFRO_GUARD_DIR/apply.lock"

: > "$TMP/log"
printf '1\n' > "$TMP/pid.101"
TEST_PIDS=101 TEST_STUCK=1 TEST_TERM_TIMEOUT=7
export TEST_PIDS TEST_STUCK TEST_TERM_TIMEOUT
reject restart
grep -q 'restart aborted' "$TMP/error"
[ "$(grep -c '^wait$' "$TMP/log")" = 12 ]
[ -s "$TMP/armed" ]
if grep -q '^submit$' "$TMP/log"; then exit 1; fi
unset TEST_PIDS TEST_STUCK TEST_TERM_TIMEOUT
rm "$TMP/pid.101"
: > "$TMP/log"
TEST_POST_ARM_FAIL=1; export TEST_POST_ARM_FAIL
reject restart
unset TEST_POST_ARM_FAIL
if grep -q '^submit$' "$TMP/log"; then exit 1; fi
writer_available
printf '2\n' > "$TMP/pid.102"
(
	TEST_PIDS=101 TEST_RESPAWN=1; export TEST_PIDS TEST_RESPAWN
	rc_stop
)
[ ! -e "$TMP/pid.102" ]
echo 'PASS boot/start/stop guard, dual-stack DNS cleanup, apply-lock fencing, delayed PID exit, timeout abort, repeated start and restart/reload'

mv "$GOFRO_GUARD_DIR/guard-device" "$TMP/device"
cp "$TMP/armed" "$TMP/retained-guard"
TEST_NO_IDENTITY=1; export TEST_NO_IDENTITY
for service in absent empty; do
	: > "$TMP/log"
	if [ "$service" = absent ]; then TEST_SERVICE_ABSENT=1; export TEST_SERVICE_ABSENT; fi
	rc_stop
	unset TEST_SERVICE_ABSENT
	[ "$(grep -c '^ubus:' "$TMP/log")" = 2 ]
	grep -qx procd-kill "$TMP/log"
	cmp "$TMP/retained-guard" "$TMP/armed"
	[ ! -e "$GOFRO_GUARD_DIR/guard-device" ]
	if grep -Eq '^(nft|network|mode):' "$TMP/log"; then exit 1; fi
	printf 'PASS identity-less stop with %s service proves quiescence twice and preserves retained guard\n' "$service"
done
rm "$TMP/armed"
rc_stop
[ ! -e "$TMP/armed" ]
for failure in TEST_UBUS_FAIL TEST_JSON_FAIL TEST_MISSING_PID TEST_REGISTERED; do
	: > "$TMP/log"
	export "$failure=1"
	reject rc_stop
	unset "$failure"
	if grep -q '^procd-kill$' "$TMP/log"; then exit 1; fi
	writer_available
	printf 'PASS identity-less uncertain stop refuses before native kill: %s\n' "$failure"
done
for field in TEST_SERVICE_TYPE TEST_INSTANCES_TYPE; do
	export "$field=array"
	reject rc_stop
	unset "$field"
done
TEST_PIDS=101; export TEST_PIDS
reject rc_stop
unset TEST_PIDS
ln -s "$TMP/missing" "$GOFRO_GUARD_DIR/guard-device"
reject rc_stop
rm "$GOFRO_GUARD_DIR/guard-device"
printf 'PASS identity-less live instance, malformed maps and dangling identity refuse\n'
for failure in TEST_RESPAWN TEST_POST_QUERY_FAIL; do
	: > "$TMP/log"
	export "$failure=1"
	reject rc_stop
	unset "$failure"
	grep -qx procd-kill "$TMP/log"
	[ "$(grep -c '^ubus:' "$TMP/log")" = 2 ]
	writer_available
	[ ! -e "$TMP/armed" ]
	printf 'PASS identity-less post-kill change refuses completion: %s\n' "$failure"
done

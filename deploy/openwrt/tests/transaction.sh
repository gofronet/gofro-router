#!/bin/sh
set -eu
ROOT="$(CDPATH='' cd "$(dirname "$0")/../../.." && pwd)"
SOURCE=$ROOT/deploy/openwrt
TMP="$(mktemp -d)"
TMP="$(CDPATH='' cd "$TMP" && pwd -P)"
cleanup() {
	status=$?
	if [ "$status" = 0 ]; then guard_untouched || status=1; fi
	if [ "$status" != 0 ]; then
		printf 'FAIL fixture: %s\n' "${FS:-initialization}" >&2
		if [ -f "${FS:-}/output" ]; then cat "$FS/output" >&2; fi
	fi
	rm -rf "$TMP"
	exit "$status"
}
trap cleanup EXIT
trap 'exit 1' HUP INT TERM
export FS

guard_untouched() {
	[ ! -e "$FS/early-tls" ] || return 1
	# Every fresh fixture starts like native OpenWrt: no stat until apk installs it.
	[ ! -e "$FS/early-stat" ] || return 1
	if [ -e "$FS/expected-guard" ]; then cmp "$FS/expected-guard" "$FS/runtime-guard" || return 1
	else [ ! -e "$FS/runtime-guard" ] || return 1; fi
	if grep '^nft:' "$FS/commands" | grep -Ev '^nft:(list tables|-s list chain inet gofro_guard gofro_guard|-c -f -|-f -)$'; then
		printf 'FAIL: installer/recovery attempted independent nft guard control\n' >&2
		return 1
	fi
}

# Run complete scripts, rewriting only absolute OS paths in disposable copies.
# No installer test switches or host /etc writes, and no real networking commands.
copy_script() {
	# Dollar expressions belong to the copied script, not this shell.
	# shellcheck disable=SC2016
	sed -E 's#(^|[[:space:]="<>(:-])/(etc|usr|tmp|proc|sys|var)/#\1@ROOT@/\2/#g; s#destination=/#destination=@ROOT@/#; s#"/\$path"#"@ROOT@/$path"#; s#"/\$\(dirname#"@ROOT@/$(dirname#' "$1" > "$2.new"
	# Check before expanding @ROOT@: Linux fixture paths themselves start in /tmp.
	if grep -Eq '(^|[[:space:]="<>(:-])/(etc|usr|tmp|proc|sys)/' "$2.new"; then
		printf 'FAIL unconfined OS path in %s\n' "$1" >&2; exit 1
	fi
	sed "1s|.*|#!/bin/sh|; s|@ROOT@|$FS|g" "$2.new" > "$2"
	rm "$2.new"
	chmod 755 "$2"
}

fresh() {
	[ -z "${FS:-}" ] || guard_untouched
	FS=$TMP/$1
	mkdir -p "$FS/bin" "$FS/uci" "$FS/etc/config" "$FS/etc/gofro" "$FS/etc/init.d" "$FS/etc/rc.d" \
		"$FS/proc/sys/kernel/random" "$FS/tmp" "$FS/usr/lib" "$FS/bundle/root/usr/bin" \
		"$FS/bundle/root/usr/sbin" "$FS/bundle/root/usr/libexec/gofro" "$FS/bundle/root/etc/init.d" \
		"$FS/bundle/root/etc/hotplug.d/iface" "$FS/bundle/root/usr/share/gofro" "$FS/var/lock"
	for cmd in uci id apk sync sleep logger nft ip wg openssl df mv jsonfilter uclient-fetch sha256sum tar chown stat; do
		ln -s "$SOURCE/tests/fixtures/commands" "$FS/bin/$cmd"
	done
	if ! PATH="$BASE_PATH" command -v flock >/dev/null 2>&1; then ln -s "$SOURCE/tests/fixtures/commands" "$FS/bin/flock"; fi
	PATH=$FS/bin:$BASE_PATH; export PATH
	printf 'DISTRIB_RELEASE=25.12.0\nDISTRIB_ARCH=x86_64\n' > "$FS/etc/openwrt_release"
	printf 'MemTotal: 262144 kB\n' > "$FS/proc/meminfo"
	printf 'boot-fixture\n' > "$FS/proc/sys/kernel/random/boot_id"
	printf '100.00 0.00\n' > "$FS/proc/uptime"
	printf '0.5.16\n' > "$FS/bundle/VERSION"
	printf 'x86_64-openwrt-linux-musl\n' > "$FS/bundle/TARGET"
	cp -R "$SOURCE/defaults" "$FS/bundle/defaults"
	cp "$SOURCE/update-public.pem" "$FS/bundle/update-public.pem"
	copy_script "$SOURCE/install.sh" "$FS/bundle/install.sh"
	for name in gofro-setup gofro-update; do copy_script "$SOURCE/root/usr/sbin/$name" "$FS/bundle/root/usr/sbin/$name"; done
	copy_script "$SOURCE/root/usr/libexec/gofro/transaction" "$FS/bundle/root/usr/libexec/gofro/transaction"
	copy_script "$SOURCE/root/usr/libexec/gofro/guard" "$FS/bundle/root/usr/libexec/gofro/guard"
	for name in gofro-finalize gofro-recover gofro-guard; do
		copy_script "$SOURCE/root/etc/init.d/$name" "$FS/bundle/root/etc/init.d/$name"
		if [ "$name" = gofro-guard ]; then
			# Model rc.common enable, but run the actual service body for boot/start/stop.
			# shellcheck disable=SC2016
			printf '\ncase "$1" in enable) printf "gofro-guard:enable\\n" >> "$FS/commands"; [ ! -e "$FS/guard-enable-fail" ] || exit 1; ln -sf "$FS/etc/init.d/gofro-guard" "$FS/etc/rc.d/S18gofro-guard"; exit ;; enabled) [ -L "$FS/etc/rc.d/S18gofro-guard" ]; exit ;; boot) start; exit ;; esac\n' >> "$FS/bundle/root/etc/init.d/$name"
		fi
		# shellcheck disable=SC2016
		printf '\ncase "$1" in enable|disable) exit 0 ;; esac\n"$@"\n' >> "$FS/bundle/root/etc/init.d/$name"
	done
	for path in usr/bin/gofro-agent usr/bin/gofro-relay usr/libexec/gofro/network \
		usr/libexec/gofro/mode usr/libexec/gofro/onboarding usr/libexec/gofro/service \
		usr/libexec/gofro/tunnel usr/libexec/gofro/update usr/libexec/gofro/wifi \
		usr/libexec/gofro/dns-flows \
		etc/init.d/gofro-agent etc/init.d/gofro-relay etc/init.d/gofro-onboarding etc/init.d/gofro-updater \
		etc/hotplug.d/iface/90-gofro-route; do
		cp "$SOURCE/tests/fixtures/commands" "$FS/bundle/root/$path"
		chmod 755 "$FS/bundle/root/$path"
	done
	for name in firewall dnsmasq; do ln -s "$SOURCE/tests/fixtures/commands" "$FS/etc/init.d/$name"; done
	for name in geosite.dat geoip.dat GEODATA-LICENSES.md; do : > "$FS/bundle/root/usr/share/gofro/$name"; done
	uci set gofro.main.interface=gt0
	uci set network.lan.ipaddr=192.168.8.1
	uci set network.wan.proto=pppoe
	uci set wireless.radio0.country=US
	uci set firewall.operator.name=operator
	uci set dhcp.@dnsmasq[0].port=53
	uci set uhttpd.main.listen_http=80
	: > "$FS/commands"
	: > "$FS/services"
	unset FAIL_MATCH FAIL_ALWAYS BAD_HEALTH KILL_MATCH FAIL_AFTER_MATCH DEGRADED
}

upgrade() {
	fresh "$1"
	old=$FS/usr/lib/gofro/releases/0.5.15
	mkdir -p "$old"
	cp -R "$FS/bundle/root/." "$old/"
	# Hash the actual shipped helper, but never execute its unsafe legacy mutations.
	cp "$SOURCE/tests/fixtures/v0.5.15-mode" "$old/usr/libexec/gofro/mode"
	# v15 has neither the transaction helper nor rollback finalization.
	rm "$old/usr/libexec/gofro/transaction" "$old/usr/libexec/gofro/guard" "$old/etc/init.d/gofro-guard"
	printf '#!/bin/sh\nexit 0\n' > "$old/etc/init.d/gofro-finalize"
	ln -s "$old" "$FS/usr/lib/gofro/current"
	# Match the symlinks an existing installation actually has.
	for path in usr/bin/gofro-agent usr/bin/gofro-relay usr/sbin/gofro-setup usr/sbin/gofro-update \
		usr/libexec/gofro/transaction usr/libexec/gofro/network usr/libexec/gofro/mode etc/init.d/gofro-agent etc/init.d/gofro-relay \
		etc/init.d/gofro-finalize etc/init.d/gofro-recover; do
		mkdir -p "$FS/$(dirname "$path")"
		ln -s "$FS/usr/lib/gofro/current/$path" "$FS/$path"
	done
	printf '0.5.15\n' > "$FS/etc/gofro/version"
	printf 'password-hash\n' > "$FS/etc/gofro/admin-password"
	printf 'server\n' > "$FS/etc/gofro/onboarding-state"
	printf 'old-cert\n' > "$FS/etc/gofro/tls-cert.pem"
	printf 'old-key\n' > "$FS/etc/gofro/tls-key.pem"
	mkdir -p "$FS/etc/wireguard"
	printf 'existing-vpn-key\n' > "$FS/etc/wireguard/client.key"
	uci set network.gt0=interface
	uci set gofro.main.lan_interface=br-old
	uci set gofro.main.lan_subnet=10.203.1.0/24
	uci set network.gt0.proto=wireguard
	uci set network.gt0.mtu=1420
	uci set firewall.gofro_vpn=zone
	uci set firewall.gofro_vpn.name=gofro_vpn
	uci set firewall.gofro_vpn.masq=0
	uci add_list dhcp.@dnsmasq[0].server='127.0.0.1#5353'
	uci add_list dhcp.@dnsmasq[0].address='/operator.test/192.0.2.1'
	uci add_list dhcp.@dnsmasq[0].address='/gofrowifi.net/10.203.1.1'
	uci set dhcp.@dnsmasq[0].noresolv=1
	uci set dhcp.@dnsmasq[0].localuse=0
	cp -R "$FS/uci" "$FS/before"
	: > "$FS/commands"
}

fails() {
	if "$@" > "$FS/output" 2>&1; then
		printf 'FAIL: unexpectedly succeeded: %s\n' "$*" >&2
		cat "$FS/output" >&2
		exit 1
	fi
}

restored() {
	guard_untouched
	diff -ru "$FS/before" "$FS/uci"
	diff -ru "$FS/before" "$FS/at-old-start"
	[ "$(cat "$FS/at-old-cert")" = old-cert ]
	[ "$(cat "$FS/at-old-key")" = old-key ]
	if [ -e "$FS/expected-guard" ]; then cmp "$FS/expected-guard" "$FS/at-old-guard"
	else [ ! -e "$FS/at-old-guard" ]; fi
	if [ -e "$FS/etc/gofro/guard-device" ]; then
		cmp "$FS/etc/gofro/guard-device" "$FS/at-old-boot-device"
		[ -L "$FS/etc/rc.d/S18gofro-guard" ]
		[ -x "$FS/usr/libexec/gofro/guard" ]
		[ -x "$FS/etc/init.d/gofro-guard" ]
	fi
	if [ -e "$FS/history-before" ]; then
		cmp "$FS/history-before" "$FS/etc/gofro/routing-legacy.json"
		cmp "$FS/history-before" "$FS/at-old-history"
	else
		[ ! -e "$FS/etc/gofro/routing-legacy.json" ]
		[ ! -e "$FS/at-old-history" ]
	fi
	[ "$(cat "$FS/etc/gofro/tls-cert.pem")" = old-cert ]
	[ "$(cat "$FS/etc/gofro/tls-key.pem")" = old-key ]
	[ "$(cat "$FS/etc/gofro/admin-password")" = password-hash ]
	[ "$(cat "$FS/etc/wireguard/client.key")" = existing-vpn-key ]
	[ "$(cat "$FS/etc/gofro/onboarding-state")" = server ]
	[ ! -e "$FS/etc/gofro/setup-code" ]
	[ "$(readlink "$FS/usr/lib/gofro/current")" = "$old" ]
	[ "$(cat "$FS/etc/gofro/version")" = 0.5.15 ]
	if [ -e "$FS/expected-guard" ]; then [ -s "$FS/etc/gofro/update-previous" ]
	else [ ! -e "$FS/etc/gofro/update-previous" ]; fi
}

BASE_PATH=$PATH
REAL_HASH="$(command -v sha256sum || command -v shasum)"; export REAL_HASH
REAL_STAT="$(command -v stat)"; export REAL_STAT
for install_mode in install update; do
	if [ "$install_mode" = install ]; then fresh nonexecutable-preflight-install; else upgrade nonexecutable-preflight-update; fi
	helper=$FS/bundle/root/usr/libexec/gofro/transaction
	# Model the signed archive AFTER copy_script's executable fixture default.
	chmod 644 "$helper"
	[ ! -x "$helper" ]
	if [ "$install_mode" = install ]; then sh "$FS/bundle/install.sh" > "$FS/output" 2>&1
	else sh "$FS/bundle/install.sh" --update > "$FS/output" 2>&1; fi
	[ "$(cat "$FS/etc/gofro/version")" = 0.5.16 ]
	[ ! -x "$helper" ]
	staged=$FS/usr/lib/gofro/releases/0.5.16/usr/libexec/gofro/transaction
	permissions="$("$REAL_STAT" -c '%a' "$staged" 2>/dev/null)" || permissions="$("$REAL_STAT" -f '%Lp' "$staged")"
	[ "$permissions" = 755 ]
	"$FS/usr/libexec/gofro/transaction" panel-check
	awk '/^uci:-X show dhcp$/ {preflight=1}
		/^apk:update$/ {if (!preflight) exit 1; checked=1}
		END {if (!checked) exit 1}' "$FS/commands"
	printf 'PASS %s: source transaction 0644 preflight succeeds before apk; staged helper is executable 0755\n' "$install_mode"
done
for install_mode in install update; do
	if [ "$install_mode" = install ]; then fresh missing-stat-install; else upgrade missing-stat-update; fi
	FAIL_MATCH='apk:update'; export FAIL_MATCH
	if [ "$install_mode" = install ]; then fails sh "$FS/bundle/install.sh"; else fails sh "$FS/bundle/install.sh" --update; fi
	[ ! -e "$FS/stat-installed" ]
	if grep -Eq '^stat:|^uci:(set|add_list|del_list|commit)|^gofro-agent:(stop|restart)' "$FS/commands"; then exit 1; fi
	printf 'PASS missing stat %s dependency failure refuses before security validation or mutation\n' "$install_mode"
done
upgrade interrupted-before-guard-prepare
copy_script "$SOURCE/root/etc/init.d/gofro-agent" "$FS/native-agent"
cp "$SOURCE/tests/fixtures/native-stop" "$FS/bundle/root/etc/init.d/gofro-agent"
: > "$FS/kill-after-current"
fails sh "$FS/bundle/install.sh" --update
[ "$(readlink "$FS/usr/lib/gofro/current")" = "$FS/usr/lib/gofro/releases/0.5.16" ]
[ ! -e "$FS/etc/gofro/guard-device" ]
[ ! -e "$FS/runtime-guard" ]
[ -s "$FS/etc/gofro/update-previous" ]
if grep -q '^gofro-agent:.*--init-security' "$FS/commands"; then exit 1; fi
rm "$FS/kill-after-current"
rmdir "$FS/tmp/gofro-install.lock"
: > "$FS/commands"
sh "$FS/etc/rc.d/S08gofro-recover" boot
[ "$(grep -c '^native:inspect$' "$FS/commands")" = 2 ]
grep -Fxq 'native:kill' "$FS/commands"
grep -Fxq 'native:stopped' "$FS/commands"
awk '/^native:stopped$/ {stopped=1} /^uci:set |^mv:-fT / {if (!stopped) exit 1}' "$FS/commands"
[ ! -e "$FS/etc/gofro/guard-device" ]
[ ! -e "$FS/runtime-guard" ]
[ "$(readlink "$FS/usr/lib/gofro/current")" = "$old" ]
sh "$FS/etc/rc.d/S99gofro-finalize" boot
restored
[ ! -e "$FS/etc/gofro/update-0.5.16" ]
printf 'PASS crash after current publication before guard prepare recovers on cold boot through real stop hooks\n'

upgrade mixed-dns
uci add_list dhcp.@dnsmasq[0].server=1.1.1.1
cp -R "$FS/uci" "$FS/mixed"
: > "$FS/commands"
fails sh "$FS/bundle/install.sh" --update
diff -ru "$FS/mixed" "$FS/uci"
if grep -Eq '^(apk:|uci:(set|add_list|del_list|commit)|gofro-agent:stop)' "$FS/commands"; then exit 1; fi
[ ! -d "$FS/usr/lib/gofro/releases/0.5.16" ]
printf 'PASS mixed legacy DNS refuses before mutation\n'

for service in agent relay; do
	upgrade "persistent-$service-stop"
	FAIL_MATCH="gofro-$service:stop" FAIL_ALWAYS=1; export FAIL_MATCH FAIL_ALWAYS
	fails sh "$FS/bundle/install.sh" --update
	# Both the forward attempt and EXIT rollback must honor the failed lifecycle fence.
	[ "$(grep -c "^gofro-$service:stop$" "$FS/commands")" = 2 ]
	cp "$FS/etc/gofro/update-previous" "$FS/pending-before"
	for attempt in initial retry; do
		if [ "$attempt" = retry ]; then fails sh "$FS/bundle/install.sh" --update; fi
		cmp "$FS/pending-before" "$FS/etc/gofro/update-previous"
		[ -x "$FS/etc/gofro/update-0.5.16/restore" ]
		[ "$(readlink "$FS/usr/lib/gofro/current")" = "$old" ]
		[ "$(cat "$FS/etc/gofro/version")" = 0.5.15 ]
		diff -ru "$FS/before" "$FS/uci"
		[ "$(cat "$FS/etc/gofro/tls-cert.pem")" = old-cert ]
		[ "$(cat "$FS/etc/gofro/tls-key.pem")" = old-key ]
		[ ! -e "$FS/etc/gofro/routing-legacy.json" ]
		if grep -Eq '^uci:(-q )?(set|add_list|del_list|delete|batch|commit)|^gofro-agent:.*--init-security|^gofro-(agent|relay):(start|restart|enable|disable)|^(firewall|dnsmasq):|^mv:-fT ' "$FS/commands"; then exit 1; fi
		if grep -q ':0.5.16$' "$FS/services"; then exit 1; fi
	done
	printf 'PASS persistent %s stop failure fences activation/TLS/UCI/start and retains journal through installer retry\n' "$service"

	upgrade "rollback-$service-stop"
	printf 'gofro-%s:stop\n' "$service" > "$FS/stop-fail-after-activation"
	BAD_HEALTH=0.5.16; export BAD_HEALTH
	fails sh "$FS/bundle/install.sh" --update
	[ "$(readlink "$FS/usr/lib/gofro/current")" = "$FS/usr/lib/gofro/releases/0.5.16" ]
	[ ! -e "$FS/at-old-start" ]
	[ "$(cat "$FS/etc/gofro/tls-cert.pem")" = new-cert ]
	[ "$(uci get network.gt0.mtu)" = 1280 ]
	grep -q 'update rollback failed' "$FS/output"
	cp "$FS/etc/gofro/update-previous" "$FS/pending-before"
	cp -R "$FS/uci" "$FS/failed-before"
	printf '%s\n' "$old" > "$FS/etc/gofro/update-restored"
	: > "$FS/commands"
	fails sh "$FS/bundle/install.sh" --update
	[ ! -e "$FS/etc/gofro/update-restored" ]
	[ "$(grep -c "^gofro-$service:stop$" "$FS/commands")" = 2 ]
	cmp "$FS/pending-before" "$FS/etc/gofro/update-previous"
	[ -x "$FS/etc/gofro/update-0.5.16/restore" ]
	diff -ru "$FS/failed-before" "$FS/uci"
	[ "$(cat "$FS/etc/gofro/tls-cert.pem")" = new-cert ]
	[ "$(cat "$FS/etc/gofro/tls-key.pem")" = new-key ]
	[ "$(readlink "$FS/usr/lib/gofro/current")" = "$FS/usr/lib/gofro/releases/0.5.16" ]
	if grep -Eq '^uci:(-q )?(set|add_list|del_list|delete|batch|commit)|^gofro-agent:.*--init-security|^gofro-(agent|relay):(start|restart|enable|disable)|^(firewall|dnsmasq):|^mv:-fT ' "$FS/commands"; then exit 1; fi
	printf 'PASS persistent %s stop failure fences EXIT rollback and pending recovery without restoring under a live writer\n' "$service"

	# Manually invoking the boot entrypoint while the failed update is still live.
	printf '%s\n' "$old" > "$FS/etc/gofro/update-restored"
	cp -R "$FS/etc/gofro/update-0.5.16" "$FS/backup-before"
	: > "$FS/commands"
	fails sh "$FS/etc/rc.d/S08gofro-recover" boot
	grep -Fxq "gofro-$service:stop" "$FS/commands"
	[ ! -e "$FS/etc/gofro/update-restored" ]
	fails sh "$FS/etc/rc.d/S99gofro-finalize" boot
	cmp "$FS/pending-before" "$FS/etc/gofro/update-previous"
	diff -ru "$FS/backup-before" "$FS/etc/gofro/update-0.5.16"
	diff -ru "$FS/failed-before" "$FS/uci"
	[ "$(cat "$FS/etc/gofro/tls-cert.pem")" = new-cert ]
	[ "$(cat "$FS/etc/gofro/tls-key.pem")" = new-key ]
	[ "$(readlink "$FS/usr/lib/gofro/current")" = "$FS/usr/lib/gofro/releases/0.5.16" ]
	if grep -Eq '^uci:(-q )?(set|add_list|del_list|delete|batch|commit)|^gofro-agent:.*--init-security|^gofro-(agent|relay):(start|restart|enable)|^mv:-fT ' "$FS/commands"; then exit 1; fi
	rm "$FS/stop-fail-after-activation"
	: > "$FS/commands"
	sh "$FS/etc/rc.d/S08gofro-recover" boot
	[ "$(cat "$FS/etc/gofro/update-restored")" = "$old" ]
	[ "$(readlink "$FS/usr/lib/gofro/current")" = "$old" ]
	diff -ru "$FS/before" "$FS/uci"
	[ "$(cat "$FS/etc/gofro/tls-cert.pem")" = old-cert ]
	[ "$(cat "$FS/etc/gofro/tls-key.pem")" = old-key ]
	awk '/^gofro-agent:stop$/ {agent=1} /^gofro-relay:stop$/ {relay=1}
		/^uci:set |^mv:-fT / {if (!agent || !relay) exit 1; restored=1}
		END {if (!restored) exit 1}' "$FS/commands"
	# v15 cannot retire the runtime guard: restoration succeeds, finalization stays pending.
	fails sh "$FS/etc/rc.d/S99gofro-finalize" boot
	restored
	printf 'PASS live recovery %s stop refusal preserves current/UCI/TLS/backup, invalidates stale witness, and retries safely\n' "$service"
done

for failure in dns-apply apply health; do
	upgrade "$failure"
	if [ "$failure" = dns-apply ]; then FAIL_MATCH='uci:commit dhcp'; export FAIL_MATCH
	elif [ "$failure" = apply ]; then FAIL_MATCH='uci:set firewall.gofro_vpn.masq=1'; export FAIL_MATCH
	else BAD_HEALTH=0.5.16; export BAD_HEALTH; fi
	fails sh "$FS/bundle/install.sh" --update
	restored
	grep -q 'gofro-agent:restart:0.5.15' "$FS/services"
	grep -q 'firewall:reload' "$FS/commands"
	grep -q 'dnsmasq:restart' "$FS/commands"
	printf 'PASS %s failure restores exact UCI/TLS/auth state and old release\n' "$failure"
done

# Recovery may run before the agent's normal boot start, with finalization later at S99.
fresh already-running-finalizer
sh "$FS/bundle/install.sh" > "$FS/output" 2>&1
previous="$(readlink "$FS/usr/lib/gofro/current")"
backup=$FS/etc/gofro/update-0.5.17
"$FS/usr/libexec/gofro/transaction" snapshot "$backup"
printf '%s\n%s\n' "$previous" "$backup" > "$FS/etc/gofro/update-previous"
sh "$FS/etc/init.d/gofro-recover" boot
"$FS/etc/init.d/gofro-agent" start
generation="$(cat "$FS/agent-generation")"
# Same parameters: the pre-init guard arms, but the existing healthy process stays alive.
"$FS/etc/init.d/gofro-agent" start
[ "$(cat "$FS/agent-generation")" = "$generation" ]
[ -e "$FS/runtime-guard" ]
[ "$(jsonfilter -i "$FS/status" -e '@.degraded')" = false ]
grep -Fxq 'agent:start-noop' "$FS/commands"
FAIL_MATCH='gofro-agent:restart'; export FAIL_MATCH
fails sh "$FS/etc/init.d/gofro-finalize" boot
[ -s "$FS/etc/gofro/update-previous" ]
[ -d "$backup" ]
[ -e "$FS/runtime-guard" ]
[ "$(cat "$FS/agent-generation")" = "$generation" ]
unset FAIL_MATCH
sh "$FS/etc/init.d/gofro-finalize" boot
[ "$(cat "$FS/agent-generation")" = "$((generation + 1))" ]
[ ! -e "$FS/runtime-guard" ]
[ ! -e "$FS/etc/gofro/update-previous" ]
[ ! -e "$backup" ]
printf 'PASS already-running finalizer requires a new agent generation; failed restart retains guard and journal\n'

for service in agent relay; do
	fresh "finalizer-$service-stop"
	sh "$FS/bundle/install.sh" > "$FS/output" 2>&1
	previous="$(readlink "$FS/usr/lib/gofro/current")"
	backup=$FS/etc/gofro/update-0.5.17
	"$FS/usr/libexec/gofro/transaction" snapshot "$backup"
	printf '%s\n%s\n' "$previous" "$backup" > "$FS/etc/gofro/update-previous"
	sh "$FS/etc/init.d/gofro-recover" boot
	"$FS/etc/init.d/gofro-agent" start
	cp -R "$FS/uci" "$FS/finalize-before"
	cp "$FS/etc/gofro/update-previous" "$FS/pending-before"
	cp -R "$backup" "$FS/backup-before"
	FAIL_MATCH="gofro-$service:stop" FAIL_ALWAYS=1; export FAIL_MATCH FAIL_ALWAYS
	: > "$FS/commands"
	fails sh "$FS/etc/init.d/gofro-finalize" boot
	cmp "$FS/pending-before" "$FS/etc/gofro/update-previous"
	diff -ru "$FS/backup-before" "$backup"
	diff -ru "$FS/finalize-before" "$FS/uci"
	[ "$(readlink "$FS/usr/lib/gofro/current")" = "$previous" ]
	if grep -Eq '^gofro-(agent|relay):(start|restart)|^gofro-agent:.*--init-security|^uci:(set|commit)' "$FS/commands"; then exit 1; fi
	unset FAIL_MATCH FAIL_ALWAYS
	sh "$FS/etc/init.d/gofro-finalize" boot
	[ ! -e "$FS/etc/gofro/update-previous" ]
	[ ! -e "$backup" ]
	[ ! -e "$FS/runtime-guard" ]
	printf 'PASS finalizer %s stop refusal prevents either service start; successful retry reconciles and clears journal\n' "$service"
done

upgrade absent-dns-aliases
uci delete dhcp.@dnsmasq[0].address
rm -rf "$FS/before"
cp -R "$FS/uci" "$FS/before"
BAD_HEALTH=0.5.16; export BAD_HEALTH
fails sh "$FS/bundle/install.sh" --update
restored
printf 'PASS rollback preserves absent DNS aliases instead of inventing entries\n'

upgrade failed-restore
printf 'failed-runtime-reconcile\n' > "$FS/expected-guard"
cp "$FS/expected-guard" "$FS/runtime-guard"
BAD_HEALTH=0.5.16 FAIL_MATCH='uci:set network.gt0.mtu=1420' FAIL_ALWAYS=1
export BAD_HEALTH FAIL_MATCH FAIL_ALWAYS
fails sh "$FS/bundle/install.sh" --update
[ -s "$FS/etc/gofro/update-previous" ]
[ "$(readlink "$FS/usr/lib/gofro/current")" != "$old" ]
if grep -q 'gofro-agent:restart:0.5.15' "$FS/services"; then exit 1; fi
fails sh "$FS/etc/rc.d/S08gofro-recover" boot
grep -q 'gofro-agent:disable' "$FS/commands"
fails sh "$FS/etc/rc.d/S99gofro-finalize" boot
guard_untouched
[ ! -e "$FS/etc/gofro/update-restored" ]
if grep -Eq 'gofro-agent:(start|restart):0.5.15' "$FS/services"; then exit 1; fi
unset FAIL_MATCH FAIL_ALWAYS
sh "$FS/etc/rc.d/S08gofro-recover" boot
# Recovery remains callable even after current switches back to a release without the helper.
sh "$FS/etc/rc.d/S08gofro-recover" boot
fails sh "$FS/etc/rc.d/S99gofro-finalize" boot
grep -q 'runtime guard retained; forwarding may be blocked' "$FS/output"
restored
printf 'PASS failed restore stops old restart; repeated boot recovery/finalize restore state without clearing runtime guard\n'

for recovery in boot installer updater; do
	upgrade "interrupted-update-$recovery"
	KILL_MATCH='uci:-q delete dhcp.@dnsmasq[0].localuse'; export KILL_MATCH
	fails sh "$FS/bundle/install.sh" --update
	[ -s "$FS/etc/gofro/update-previous" ]
	[ "$(readlink "$FS/usr/lib/gofro/current")" != "$old" ]
	unset KILL_MATCH
	rmdir "$FS/tmp/gofro-install.lock"
	if [ "$recovery" = boot ]; then
		sh "$FS/etc/rc.d/S08gofro-recover" boot
		fails sh "$FS/etc/rc.d/S99gofro-finalize" boot
	elif [ "$recovery" = installer ]; then
		# An interrupted installation may need to install stat before recovery validates files.
		rm "$FS/stat-installed"
		fails sh "$FS/bundle/install.sh" --update
		grep -q 'runtime guard retained; forwarding may be blocked' "$FS/output"
	else
		# Model a crash after version publication but before clearing the pending journal.
		printf '0.5.16\n' > "$FS/etc/gofro/version"
		GOFRO_RELEASE_URL=https://fixture.invalid; export GOFRO_RELEASE_URL
		fails sh "$FS/bundle/root/usr/sbin/gofro-update"
		unset GOFRO_RELEASE_URL
		grep -q 'runtime guard retained; forwarding may be blocked' "$FS/output"
	fi
	restored
	printf 'PASS interrupted DNS migration restores through %s path\n' "$recovery"
done

upgrade snapshot-refused
mkdir "$FS/etc/gofro/update-0.5.16"
fails sh "$FS/bundle/install.sh" --update
diff -ru "$FS/before" "$FS/uci"
[ ! -s "$FS/services" ]
[ "$(readlink "$FS/usr/lib/gofro/current")" = "$old" ]
printf 'PASS snapshot refusal does not arm rollback or restart services\n'

for collision in network.gt0 firewall.gofro_lan_dns key marker interface; do
	fresh "collision-$collision"
	case "$collision" in
		key) mkdir "$FS/etc/wireguard"; printf 'operator-key\n' > "$FS/etc/wireguard/client.key" ;;
		marker) printf 'wan\n' > "$FS/etc/gofro/openwrt-owned-v16" ;;
		interface) uci set gofro.main.interface=wan ;;
		*) uci set "$collision=operator" ;;
	esac
	cp -R "$FS/uci" "$FS/before"
	fails sh "$FS/bundle/install.sh"
	diff -ru "$FS/before" "$FS/uci"
	if [ "$collision" = key ]; then [ "$(cat "$FS/etc/wireguard/client.key")" = operator-key ]
	else [ ! -e "$FS/etc/wireguard/client.key" ]; fi
	if [ "$collision" = marker ]; then [ "$(cat "$FS/etc/gofro/openwrt-owned-v16")" = wan ]
	else [ ! -e "$FS/etc/gofro/openwrt-owned-v16" ]; fi
	printf 'PASS fresh setup collision: %s\n' "$collision"
done

for failure in partial-uci interrupted finalize finalize-write finalize-published; do
	fresh "setup-$failure"
	if [ "$failure" = partial-uci ]; then FAIL_MATCH='uci:set network.gt0.proto=wireguard'; export FAIL_MATCH
	elif [ "$failure" = interrupted ]; then KILL_MATCH='uci:set network.gt0.proto=wireguard'; export KILL_MATCH
	elif [ "$failure" = finalize-write ]; then FAIL_MATCH="mv:-f $FS/etc/gofro/version.new $FS/etc/gofro/version"; export FAIL_MATCH
	elif [ "$failure" = finalize-published ]; then FAIL_AFTER_MATCH="mv:-f $FS/etc/gofro/version.new $FS/etc/gofro/version"; export FAIL_AFTER_MATCH
	else BAD_HEALTH=0.5.16; export BAD_HEALTH; fi
	fails sh "$FS/bundle/install.sh"
	if [ "$failure" = finalize-published ]; then [ "$(cat "$FS/etc/gofro/version")" = 0.5.16 ]
	else [ ! -e "$FS/etc/gofro/version" ]; fi
	[ "$(cat "$FS/etc/gofro/openwrt-owned-v16")" = gt0 ]
	[ "$(cat "$FS/etc/wireguard/client.key")" = private-key ]
	if [ "$failure" = interrupted ]; then
		# A reboot clears /tmp, including the lock left by an untrappable SIGKILL.
		rmdir "$FS/tmp/gofro-setup.lock"
	else grep -q 'gofro-agent:stop' "$FS/commands"; fi
	unset FAIL_MATCH BAD_HEALTH KILL_MATCH FAIL_AFTER_MATCH
	sh "$FS/bundle/install.sh" > "$FS/retry-output" 2>&1
	[ "$(cat "$FS/etc/gofro/version")" = 0.5.16 ]
	[ ! -e "$FS/etc/gofro/install-pending" ]
	[ -L "$FS/etc/rc.d/S18gofro-guard" ]
	[ "$(cat "$FS/etc/gofro/guard-device")" = br-old ]
	[ "$(find "$FS/etc/gofro/guard-device" -type f -perm 0600)" = "$FS/etc/gofro/guard-device" ]
	[ ! -e "$FS/etc/gofro/routing-legacy.json" ]
	[ ! -e "$FS/at-new-history" ]
	[ "$(cat "$FS/etc/gofro/onboarding-state")" = admin ]
	[ "$(cat "$FS/etc/gofro/onboarding-window")" = 'boot-fixture 1000' ]
	[ "$(find "$FS/etc/gofro" -prune -type d -perm 0700)" = "$FS/etc/gofro" ]
	[ "$(find "$FS/etc/gofro/setup-code" -type f -perm 0600)" = "$FS/etc/gofro/setup-code" ]
	[ "$(find "$FS/etc/gofro/openwrt-owned-v16" -type f -perm 0600)" = "$FS/etc/gofro/openwrt-owned-v16" ]
	[ "$(grep -c '^wg:genkey$' "$FS/commands")" = 1 ]
	[ "$(uci get network.gt0.addresses)" = 10.202.0.2/32 ]
	[ "$(uci get firewall.gofro_vpn.network)" = gt0 ]
	[ "$(uci get network.wan.proto)" = pppoe ]
	[ "$(uci get dhcp.@dnsmasq[0].port)" = 53 ]
	[ "$(uci get uhttpd.main.listen_http)" = 80 ]
	grep -q 'Gofro setup URL: https://192.168.8.1:8443' "$FS/retry-output"
	printf 'PASS setup %s failure retains ownership/key and installer retry completes\n' "$failure"
done

upgrade successful-update
sh "$FS/bundle/install.sh" --update > "$FS/output" 2>&1
[ "$(cat "$FS/etc/gofro/version")" = 0.5.16 ]
[ "$(cat "$FS/etc/gofro/admin-password")" = password-hash ]
[ "$(cat "$FS/etc/wireguard/client.key")" = existing-vpn-key ]
[ "$(cat "$FS/etc/gofro/onboarding-state")" = server ]
[ ! -e "$FS/etc/gofro/setup-code" ]
[ ! -e "$FS/etc/gofro/onboarding-window" ]
[ ! -e "$FS/etc/gofro/update-previous" ]
[ ! -e "$FS/etc/gofro/update-0.5.16" ]
[ "$(uci get dhcp.@dnsmasq[0].address)" = /operator.test/192.0.2.1 ]
[ "$(uci get network.wan.proto)" = pppoe ]
if grep -q '^wg:' "$FS/commands"; then exit 1; fi
printf 'PASS successful update preserves authentication and does not reopen setup\n'

upgrade history-rollback
printf '{"version":"0.5.15","device":"br-old","subnet":"10.203.1.0/24","note":"preserve exact bytes"}\n' > "$FS/history-before"
cp "$FS/history-before" "$FS/etc/gofro/routing-legacy.json"
BAD_HEALTH=0.5.16; export BAD_HEALTH
fails sh "$FS/bundle/install.sh" --update
restored
printf 'PASS attestation snapshot restores original bytes before old-agent restart\n'

# Exercise the actual new mode script, not a substitute for its ownership checks.
for adoption in canonical foreign-route new-only-guard except-gt0 changed-device; do
	upgrade "legacy-routes-$adoption"
	copy_script "$SOURCE/root/usr/libexec/gofro/mode" "$FS/bundle/root/usr/libexec/gofro/mode"
	: > "$FS/real-mode"
	printf '90: from 10.203.1.0/24 lookup 100\n' > "$FS/rules"
	printf '10.203.1.0/24 dev br-old proto boot scope link\ndefault dev gt0 proto boot scope link metric 10\nunreachable default proto boot metric 32767\n' > "$FS/routes"
	case "$adoption" in
		foreign-route) printf 'default via 192.0.2.1 dev foreign metric 11\n' >> "$FS/routes" ;;
		new-only-guard) : > "$FS/new-only-guard" ;;
		except-gt0) printf 'gt0\n' > "$FS/guard-output-device" ;;
		changed-device) : > "$FS/changed-device" ;;
	esac
	cp "$FS/routes" "$FS/routes-before"
	cp "$FS/rules" "$FS/rules-before"
	if [ "$adoption" = canonical ]; then
		sh "$FS/bundle/install.sh" --update > "$FS/output" 2>&1
		[ "$(cat "$FS/guard-device")" = br-old ]
		[ "$(cat "$FS/at-new-history")" = '{"version":"0.5.15","device":"br-old","subnet":"10.203.1.0/24"}' ]
		[ "$(find "$FS/at-new-history" -type f -perm 0600)" = "$FS/at-new-history" ]
		[ ! -e "$FS/etc/gofro/routing-legacy.json" ]
		[ ! -e "$FS/runtime-guard" ]
		grep -Fxq '192.168.8.0/24 dev br-old proto 186 scope link' "$FS/routes"
		if grep -Eq 'proto boot|^10\.203\.1\.0/24 ' "$FS/routes" || grep -q '^90:' "$FS/rules"; then exit 1; fi
		grep -Fxq 'agent:full-reconcile-complete' "$FS/commands"
		cmp "$FS/routes-before" "$FS/at-tls-routes"
		cmp "$FS/rules-before" "$FS/at-tls-rules"
		awk '
			/^mv:-f .*routing-legacy.json.new .*routing-legacy.json$/ { published=1 }
			/^uci:-q del_list dhcp/ { if (!published) exit 1; seen=1 }
			END { if (!seen) exit 1 }' "$FS/commands"
	else
		fails sh "$FS/bundle/install.sh" --update
		cmp "$FS/routes-before" "$FS/routes"
		cmp "$FS/rules-before" "$FS/rules"
		if grep -Eq '^ip:(rule add|rule del|route replace|route del)' "$FS/commands"; then exit 1; fi
		if [ "$adoption" = changed-device ]; then
			grep -q 'requires maintenance; automatic migration is unsupported' "$FS/output"
			[ ! -s "$FS/services" ]
			[ ! -e "$FS/etc/gofro/guard-device" ]
			[ "$(readlink "$FS/usr/lib/gofro/current")" = "$old" ]
		else
			grep -q 'runtime guard retained; forwarding may be blocked' "$FS/output"
			restored
		fi
		if [ "$adoption" = except-gt0 ]; then grep -q 'requires an exact guard' "$FS/output"; fi
	fi
	printf 'PASS actual mode integration: %s\n' "$adoption"
done

for history in modified-helper missing-device missing-subnet invalid-subnet pending-uci mismatched-version; do
	upgrade "unproven-$history"
	case "$history" in
		modified-helper) printf '\n# locally changed\n' >> "$old/usr/libexec/gofro/mode" ;;
		missing-device) uci delete gofro.main.lan_interface ;;
		missing-subnet) uci delete gofro.main.lan_subnet ;;
		invalid-subnet) uci set 'gofro.main.lan_subnet=10.203.1.0/24
192.168.8.0/24' ;;
		pending-uci) printf 'gofro.main.lan_interface=other\n' > "$FS/uci-changes" ;;
		mismatched-version) printf '0.5.14\n' > "$FS/etc/gofro/version" ;;
	esac
	cp -R "$FS/uci" "$FS/unproven-before"
	: > "$FS/commands"
	fails sh "$FS/bundle/install.sh" --update
	diff -ru "$FS/unproven-before" "$FS/uci"
	[ ! -e "$FS/etc/gofro/routing-legacy.json" ]
	[ ! -s "$FS/services" ]
	if grep -Eq '^uci:(set|add_list|del_list|commit)|^apk:' "$FS/commands"; then exit 1; fi
	printf 'PASS unproven legacy history refuses before mutation: %s\n' "$history"
done

fresh unowned-history
printf '{"version":"0.5.15","device":"br-old","subnet":"10.203.1.0/24"}\n' > "$FS/etc/gofro/routing-legacy.json"
cp "$FS/etc/gofro/routing-legacy.json" "$FS/history-before"
fails sh "$FS/bundle/install.sh"
cmp "$FS/history-before" "$FS/etc/gofro/routing-legacy.json"
[ ! -s "$FS/services" ]
if grep -Eq '^uci:(set|add_list|del_list|commit)|^apk:' "$FS/commands"; then exit 1; fi
printf 'PASS fresh install refuses unowned history without inventing or replacing it\n'

upgrade retained-boot-guard
printf 'br-old\n' > "$FS/etc/gofro/guard-device"
chmod 700 "$FS/etc/gofro"; chmod 600 "$FS/etc/gofro/guard-device"
cp "$FS/etc/gofro/guard-device" "$FS/boot-before"
FAIL_MATCH='uci:set firewall.gofro_vpn.masq=1'; export FAIL_MATCH
fails sh "$FS/bundle/install.sh" --update
restored
cmp "$FS/boot-before" "$FS/etc/gofro/update-0.5.16/guard-device"
cmp "$FS/boot-before" "$FS/etc/gofro/guard-device"
[ ! -e "$old/usr/libexec/gofro/guard" ]
printf 'boot-checkpoint\n' >> "$FS/commands"
sh "$FS/etc/rc.d/S18gofro-guard" boot
awk '/^boot-checkpoint$/ { boot=1 } boot && /^(network|uci|ip):/ { exit 1 }' "$FS/commands"
[ "$(cat "$FS/guard-device")" = br-old ]
printf 'PASS exact boot identity retained; pinned START18 helper boots without network after v15 rollback\n'

# Even after journal finalization, the boot-only release must not be pruned or overwritten.
rm "$FS/etc/gofro/update-previous"
fails sh "$FS/bundle/install.sh" --update
grep -q 'release is pinned by the boot guard' "$FS/output"
[ -x "$FS/usr/libexec/gofro/guard" ]
[ -x "$FS/etc/init.d/gofro-guard" ]
sh "$FS/etc/rc.d/S18gofro-guard" boot
printf 'PASS installer pruning and same-version retry preserve the pinned boot-only release\n'

upgrade conflicting-boot-guard
printf 'br-old\n' > "$FS/etc/gofro/guard-device"
chmod 700 "$FS/etc/gofro"; chmod 600 "$FS/etc/gofro/guard-device"
printf 'br-home\n' > "$FS/replacement-guard"
FAIL_MATCH='uci:set firewall.gofro_vpn.masq=1'; export FAIL_MATCH
fails sh "$FS/bundle/install.sh" --update
[ "$(cat "$FS/etc/gofro/update-0.5.16/guard-device")" = br-old ]
[ "$(cat "$FS/etc/gofro/guard-device")" = br-home ]
[ "$(cat "$FS/guard-device")" = br-home ]
[ ! -e "$FS/at-old-start" ]
[ "$(readlink "$FS/usr/lib/gofro/current")" != "$old" ]
grep -q 'boot guard identity changed; refusing rollback' "$FS/output"
fails sh "$FS/etc/rc.d/S08gofro-recover" boot
[ "$(cat "$FS/etc/gofro/guard-device")" = br-home ]
[ ! -e "$FS/at-old-start" ]
printf 'PASS live and boot rollback refuse to replace a newer boot guard with the old LAN\n'

upgrade unsafe-boot-state
printf 'br-old\n' > "$FS/etc/gofro/guard-device"
chmod 700 "$FS/etc/gofro"; chmod 600 "$FS/etc/gofro/guard-device"
FAIL_MATCH='uci:set firewall.gofro_vpn.masq=1'; export FAIL_MATCH
fails sh "$FS/bundle/install.sh" --update
restored
chmod 644 "$FS/etc/gofro/guard-device"
rm -rf "$FS/at-old-start"
fails sh "$FS/etc/rc.d/S08gofro-recover" boot
[ ! -e "$FS/at-old-start" ]
[ ! -e "$FS/etc/gofro/update-restored" ]
grep -q 'gofro-agent:disable' "$FS/commands"
printf 'PASS boot recovery rejects unsafe guard-file permissions before enabling the old agent\n'

for failure in enable network arm persist mode tls; do
	fresh "setup-guard-$failure"
	case "$failure" in
		enable) : > "$FS/guard-enable-fail" ;;
		network) FAIL_MATCH='network:' ;;
		arm) FAIL_MATCH='nft:-c -f -' ;;
		persist) FAIL_MATCH="chown:0:0 $FS/etc/gofro" ;;
		mode) FAIL_MATCH='mode:check br-old 192.168.8.0/24' ;;
		tls) FAIL_MATCH='gofro-agent:--https-listen 192.168.8.1:8443 --init-security' ;;
	esac
	export FAIL_MATCH
	fails sh "$FS/bundle/install.sh"
	[ ! -e "$FS/etc/gofro/version" ]
	[ "$(cat "$FS/etc/gofro/openwrt-owned-v16")" = gt0 ]
	if [ "$failure" != tls ]; then
		[ ! -e "$FS/etc/wireguard/client.key" ]
		if grep -q '^gofro-agent:.*--init-security' "$FS/commands"; then exit 1; fi
	fi
	if [ "$failure" != enable ]; then [ -L "$FS/etc/rc.d/S18gofro-guard" ]; fi
	case "$failure" in mode|tls) [ "$(cat "$FS/etc/gofro/guard-device")" = br-old ] ;; esac
	if [ "$failure" = persist ]; then grep -q 'boot guard is not persisted' "$FS/output"; fi
	unset FAIL_MATCH
	rm -f "$FS/guard-enable-fail"
	sh "$FS/bundle/install.sh" > "$FS/output" 2>&1
	[ "$(cat "$FS/etc/gofro/version")" = 0.5.16 ]
	[ -L "$FS/etc/rc.d/S18gofro-guard" ]
	[ "$(cat "$FS/etc/gofro/guard-device")" = br-old ]
	[ ! -e "$FS/runtime-guard" ]
	printf 'PASS setup guard %s failure retains ownership and retries without early TLS\n' "$failure"
done

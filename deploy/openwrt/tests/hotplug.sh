#!/bin/sh
set -eu
ROOT="$(CDPATH='' cd "$(dirname "$0")/../../.." && pwd)"; TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT
if ! command -v flock >/dev/null 2>&1; then
cat > "$TMP/flock" <<'EOF'
#!/usr/bin/perl
use Fcntl qw(LOCK_EX LOCK_NB);
open(my $lock, '>&=', 9) or die $!;
flock($lock, LOCK_EX | LOCK_NB) or exit 1;
EOF
chmod +x "$TMP/flock"
fi
for command in uci ip; do
cat > "$TMP/$command" <<'EOF'
#!/bin/sh
case "$0" in
*/uci) [ "$3" = gofro.main.interface ] && echo gt0 ;;
*) echo "$*" >> "$GOFRO_TEST_LOG"; case "$*" in '-4 -N -o route show table 100') printf '%s\n' "${ROUTES:-}" ;; '-4 -N rule show') printf '%s\n' "${RULES:-}" ;; esac ;;
esac
EOF
chmod +x "$TMP/$command"
done
mkdir -p "$TMP/init.d"
cat > "$TMP/init.d/gofro-agent" <<'EOF'
#!/bin/sh
echo "$*" >> "$GOFRO_TEST_LOG"
EOF
chmod +x "$TMP/init.d/gofro-agent"; export GOFRO_TEST_LOG="$TMP/log"
cat > "$TMP/network" <<'EOF'
#!/bin/sh
[ "${NETWORK_FAIL:-0}" = 0 ] || exit 1
echo '{}'
EOF
cat > "$TMP/jsonfilter" <<'EOF'
#!/bin/sh
case "$*" in *'@.device'*) echo br-home ;; *'@.subnet'*) echo 192.168.44.0/24 ;; esac
EOF
chmod +x "$TMP/network" "$TMP/jsonfilter"
export GOFRO_NETWORK_COMMAND="$TMP/network" GOFRO_MODE_COMMAND="$ROOT/deploy/openwrt/root/usr/libexec/gofro/mode" GOFRO_ROUTING_LEGACY="$TMP/no-history" GOFRO_MODE_LOCK="$TMP/lock"
ACTION=ifupdate INTERFACE=lan DEVICE=br-home GOFRO_AGENT_INIT="$TMP/init.d/gofro-agent" PATH="$TMP:$PATH" sh "$ROOT/deploy/openwrt/root/etc/hotplug.d/iface/90-gofro-route"
grep -Fxq restart "$TMP/log"
ACTION=ifup INTERFACE=gt0 DEVICE=gt0 PATH="$TMP:$PATH" sh "$ROOT/deploy/openwrt/root/etc/hotplug.d/iface/90-gofro-route"
grep -Fxq 'route replace default dev gt0 table 100 metric 10 proto 186' "$TMP/log"
for action in ifup ifdown; do
	: > "$TMP/log"
	if ACTION="$action" INTERFACE=gt0 DEVICE=gt0 ROUTES='default dev foreign scope link metric 10' PATH="$TMP:$PATH" sh "$ROOT/deploy/openwrt/root/etc/hotplug.d/iface/90-gofro-route" 2>/dev/null; then exit 1; fi
	if grep -Eq '^(route replace|route del|rule add|rule del)' "$TMP/log"; then exit 1; fi
done
: > "$TMP/log"
if ACTION=ifup INTERFACE=gt0 DEVICE=gt0 NETWORK_FAIL=1 PATH="$TMP:$PATH" sh "$ROOT/deploy/openwrt/root/etc/hotplug.d/iface/90-gofro-route"; then exit 1; fi
[ ! -s "$TMP/log" ]
echo 'PASS hotplug uses shared ownership validation before mutation'

#!/bin/sh
set -eu

ROOT=$(CDPATH='' cd "$(dirname "$0")/../../.." && pwd)
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
umask 077
mkdir "$TMP/state" "$TMP/config" "$TMP/bin"
export GOFRO_STATE_DIR="$TMP/state" GOFRO_CONFIG_DIR="$TMP/config" GOFRO_INIT_DIR="$TMP/bin"
export GOFRO_BOOT_ID=01234567-89ab-cdef-0123-456789abcdef GOFRO_UPTIME=10
export GOFRO_TEST_LOG="$TMP/args" GOFRO_ONBOARDING_LOCK="$TMP/apply.lock"
export GOFRO_TEST_WIFI_HELPER="$ROOT/deploy/openwrt/root/usr/libexec/gofro/wifi"
ONBOARDING=$ROOT/deploy/openwrt/root/usr/libexec/gofro/onboarding

# Exercise real helper orchestration with an isolated UCI store, not the host network.
cat > "$TMP/bin/uci" <<'EOF'
#!/usr/bin/env python3
import json, os, pathlib, shlex, sys
root = pathlib.Path(os.environ['GOFRO_CONFIG_DIR'])
args = [arg for arg in sys.argv[1:] if arg != '-q']
with open(os.environ['GOFRO_TEST_LOG'], 'a') as log:
    log.write(json.dumps(args) + '\n')
def command(args):
    action = args[0]
    key, _, value = args[1].partition('=')
    path = root / key.split('.')[0]
    data = json.loads(path.read_text()) if path.exists() else {}
    if action == 'get':
        if key not in data: raise SystemExit(1)
        print(data[key])
    elif action == 'show':
        for name, item in data.items(): print(f'{name}={item}')
    elif action in ('commit', 'revert'): pass
    elif action == 'delete':
        data = {name: item for name, item in data.items() if name != key and not name.startswith(key + '.')}
    elif action == 'set': data[key] = value
    elif action == 'add_list': data[key] = value
    else: raise SystemExit(2)
    if action not in ('get', 'show'): path.write_text(json.dumps(data))
if args[0] == 'batch':
    for line in sys.stdin:
        if line.strip(): command(shlex.split(line))
else: command(args)
EOF
cat > "$TMP/bin/jsonfilter" <<'EOF'
#!/bin/sh
if [ "$1" = -i ]; then file=$2; shift 2; else file=-; fi
expression=$(printf %s "$2" | sed 's/^@//;s/\[\*\]/[]/g')
exec jq -er "$expression // empty" "$file"
EOF
cat > "$TMP/bin/wifi" <<'EOF'
#!/bin/sh
printf 'wifi %s\n' "$*" >> "$GOFRO_TEST_LOG"
if [ "$1" = status ] && [ "${GOFRO_TEST_RADIO_DELAY:-}" = 1 ]; then
	count=$(cat "$GOFRO_STATE_DIR/radio-checks" 2>/dev/null || printf 0)
	count=$((count + 1))
	printf '%s\n' "$count" > "$GOFRO_STATE_DIR/radio-checks"
	if [ "$count" -le 5 ]; then printf '%s\n' '{"radio0":{"up":false},"radio1":{"up":false}}'; exit 0; fi
fi
[ "$1" != status ] || printf '%s\n' '{"radio0":{"up":true},"radio1":{"up":true}}'
EOF
cat > "$TMP/bin/wifi-helper" <<'EOF'
#!/bin/sh
if [ "${GOFRO_TEST_FAIL_STAGE:-}" = 1 ] && [ "$*" = 'stage 5g Home 5G' ]; then exit 1; fi
exec sh "$GOFRO_TEST_WIFI_HELPER" "$@"
EOF
cat > "$TMP/bin/nft" <<'EOF'
#!/bin/sh
if [ "$1" = -f ]; then cat >> "$GOFRO_TEST_LOG"; fi
EOF
cat > "$TMP/bin/network" <<'EOF'
#!/bin/sh
printf 'service %s\n' "$*" >> "$GOFRO_TEST_LOG"
EOF
cp "$TMP/bin/network" "$TMP/bin/firewall"
cp "$TMP/bin/network" "$TMP/bin/dnsmasq"
cp "$TMP/bin/network" "$TMP/bin/gofro-agent"
cp "$TMP/bin/network" "$TMP/bin/gofro-onboarding"
cp "$TMP/bin/network" "$TMP/bin/ifdown"
chmod +x "$TMP/bin/"*
export PATH="$TMP/bin:$PATH" GOFRO_UCI_COMMAND="$TMP/bin/uci" GOFRO_NFT_COMMAND="$TMP/bin/nft"
export GOFRO_WIFI_COMMAND="$TMP/bin/wifi" GOFRO_WIFI_HELPER="$TMP/bin/wifi-helper" GOFRO_IFDOWN_COMMAND="$TMP/bin/ifdown"
printf '%s\n' '{"wireless.ap2":"wifi-iface","wireless.ap2.mode":"ap","wireless.ap2.device":"radio0","wireless.ap2.ssid":"Old 2","wireless.radio0.band":"2g","wireless.ap5":"wifi-iface","wireless.ap5.mode":"ap","wireless.ap5.device":"radio1","wireless.ap5.ssid":"Old 5","wireless.radio1.band":"5g"}' > "$TMP/config/wireless"
for config in network dhcp gofro firewall; do printf '{}\n' > "$TMP/config/$config"; done

if sh "$ONBOARDING" begin; then exit 1; fi
[ ! -e "$TMP/state/onboarding-state" ]
GOFRO_ONBOARDING_FRESH=1 sh "$ONBOARDING" begin
[ "$(cat "$TMP/state/onboarding-state")" = admin ]
[ "$(uci get wireless.gofro_setup.ssid)" = 'GofroNET Wi-Fi Setup' ]
[ "$(uci get wireless.ap2.disabled)" = 1 ]
[ "$(uci get network.gofro_setup_device.type)" = bridge ]
[ "$(uci get dhcp.gofro_setup.interface)" = gofro_setup ]
grep -Fq 'iifname != "br-gofro-setup" ip daddr 10.203.1.1 drop' "$TMP/args"
GOFRO_BOOT_ID=11111111-1111-1111-1111-111111111111 sh "$ONBOARDING" recover
[ "$(uci get wireless.gofro_setup.disabled)" = 1 ]
[ -e "$TMP/state/onboarding-window" ]
printf 'owner\n' > "$TMP/state/admin-password"
sh "$ONBOARDING" begin
[ "$(cat "$TMP/state/onboarding-state")" = wifi ]
printf '%s\n' '{"networks":[{"band":"2g","ssid":"Home 2G","password":"secret-two"}]}' > "$TMP/missing"
if sh "$ONBOARDING" wifi < "$TMP/missing"; then exit 1; fi
[ "$(cat "$TMP/state/onboarding-state")" = wifi ]
printf '%s\n' '{"networks":[{"band":"2g","ssid":"Home 2G","password":"secret-two"},{"band":"5g","ssid":"Home 5G","password":"secret-five"}]}' > "$TMP/payload"
sh "$ONBOARDING" wifi < "$TMP/payload"
[ "$(cat "$TMP/state/onboarding-state")" = wifi_applying ]
sh "$ONBOARDING" apply
[ "$(uci get wireless.ap2.ssid)" = 'Old 2' ]
if GOFRO_UPTIME=14 GOFRO_TEST_FAIL_STAGE=1 sh "$ONBOARDING" apply; then exit 1; fi
[ "$(cat "$TMP/state/onboarding-state")" = wifi ]
[ "$(uci get wireless.ap2.ssid)" = 'Old 2' ]
sh "$ONBOARDING" wifi < "$TMP/payload"
GOFRO_UPTIME=14 GOFRO_TEST_RADIO_DELAY=1 sh "$ONBOARDING" apply
[ "$(cat "$TMP/state/onboarding-state")" = server ]
[ ! -e "$TMP/state/onboarding-wifi.json" ]
[ "$(uci get wireless.ap2.ssid)" = 'Home 2G' ]
[ "$(uci get wireless.ap5.ssid)" = 'Home 5G' ]
[ "$(uci get network.lan.ipaddr)" = 10.203.1.1 ]
if uci -q get wireless.gofro_setup >/dev/null; then exit 1; fi
sh "$ONBOARDING" complete
[ ! -e "$TMP/state/onboarding-state" ]
if GOFRO_ONBOARDING_FRESH=1 sh "$ONBOARDING" begin; then exit 1; fi
if grep -q 'secret-' "$TMP/args"; then exit 1; fi
awk '
 /\/etc\/init.d\/uhttpd stop/ { stopped=1 }
 /\/usr\/libexec\/gofro\/onboarding begin/ { if (!stopped) exit 1; stopped=0; count++ }
 END { if (count != 2) exit 1 }
' "$ROOT/deploy/openwrt/root/usr/sbin/gofro-setup"

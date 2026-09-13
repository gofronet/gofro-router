#!/bin/sh
# Run only in a disposable Docker container with --network none --cap-add NET_ADMIN.
set -eu
[ "${1:-}" = --run ] && [ -e /.dockerenv ] || { echo 'requires disposable Docker --network none' >&2; exit 2; }
if [ "$(ip -o link show up | wc -l)" -ne 1 ] || ! ip link show lo up >/dev/null; then
	echo 'requires an empty network namespace' >&2; exit 2
fi
ROOT="$(CDPATH='' cd "$(dirname "$0")/../../.." && pwd)"
TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT
export GOFRO_MODE_LOCK="$TMP/lock" GOFRO_ROUTING_LEGACY="$TMP/history.json"
mode() { sh "$ROOT/deploy/openwrt/root/usr/libexec/gofro/mode" "$1" br-home 192.168.44.0/24; }
routes() { ip -4 -N -o route show table 100 | awk '{$1=$1; print}'; }
reject() {
	before_routes="$(routes)"; before_rules="$(ip -4 -N rule show)"
	if mode "$1" >"$TMP/out" 2>"$TMP/error"; then echo "unsafe $1 succeeded" >&2; exit 1; fi
	[ "$(routes)" = "$before_routes" ] && [ "$(ip -4 -N rule show)" = "$before_rules" ]
}

ip -Version
ip link add br-home type dummy
ip link set br-home up
ip link add gt0 type dummy
ip link set gt0 up
main="$(ip -4 -N route show table main)"; local_routes="$(ip -4 -N route show table local)"
mode vpn
ip -4 -o route show table 100 | grep -q 'proto bgp'
routes | grep -Fxq '192.168.44.0/24 dev br-home proto 186 scope 253'
routes | grep -Fxq '7 default proto 186 metric 32767'
first="$(routes)"
mode vpn
mode check
[ "$(routes)" = "$first" ]
echo 'PASS real iproute2 fresh apply, bgp rendering, second apply and check'

mode tunnel-up
routes | grep -Fxq 'default dev gt0 proto 186 scope 253 metric 10'
mode tunnel-up
mode tunnel-down
mode tunnel-down
[ "$(routes)" = "$first" ]
mode tunnel-up
mode bypass
[ "$(routes)" = '7 default proto 186 metric 32767' ]
mode bypass
mode check
mode vpn
[ "$(routes)" = "$first" ]
echo 'PASS real iproute2 on/off and tunnel up/down'

# Recreate the exact v0.5.15 routes and source rule, without flushing anything.
mode bypass
ip route replace 10.203.1.0/24 dev br-home table 100 proto boot
ip route replace default dev gt0 table 100 metric 10 proto boot
ip route replace unreachable default table 100 metric 32767 proto boot
ip rule add pref 90 from 10.203.1.0/24 lookup 100
reject vpn
cat > "$GOFRO_ROUTING_LEGACY" <<'EOF'
{"version":"0.5.15","device":"br-home","subnet":"10.203.1.0/24"}
EOF
cat > "$TMP/jsonfilter" <<'EOF'
#!/bin/sh
exec python3 -c 'import json,sys; print(json.load(open(sys.argv[2]))[sys.argv[-1][2:]])' "$@"
EOF
chmod +x "$TMP/jsonfilter"
export PATH="$TMP:$PATH"
nft -f - <<'EOF'
add table inet gofro_guard
add chain inet gofro_guard gofro_guard { type filter hook forward priority filter; policy accept; }
add rule inet gofro_guard gofro_guard iifname "br-home" oifname != "br-home" drop
EOF
legacy="$(routes)"
mode check
[ "$(routes)" = "$legacy" ]
mode vpn
mode vpn
mode check
routes | grep -Fxq 'default dev gt0 proto 186 scope 253 metric 10'
routes | grep -Fxq '192.168.44.0/24 dev br-home proto 186 scope 253'
routes | grep -Fxq '7 default proto 186 metric 32767'
[ "$(routes | wc -l)" -eq 3 ]
if ip -4 -N rule show | grep -q '^90:'; then exit 1; fi
echo 'PASS real iproute2 canonical guarded adoption and same-device renumbering'

ip route add 198.51.100.0/24 dev br-home table 100 proto static
reject vpn
reject check
reject tunnel-up
reject tunnel-down
ip route del 198.51.100.0/24 dev br-home table 100 proto static
ip route replace default dev gt0 table 100 metric 10 proto static
reject tunnel-up
ip route replace default dev gt0 table 100 metric 10 proto 186
ip rule del pref 81 fwmark 0x20000/0x30000 lookup 100
mkdir -p /etc/iproute2
printf '1000 gofro_foreign\n' >> /etc/iproute2/rt_tables
ip rule add pref 81 fwmark 0x20000/0x30000 lookup gofro_foreign
ip -4 rule show | grep -q 'lookup gofro_foreign'
reject vpn
grep -Fq 'foreign rule priority 81' "$TMP/error"
[ "$(ip -4 -N route show table main)" = "$main" ]
[ "$(ip -4 -N route show table local)" = "$local_routes" ]
echo 'PASS real iproute2 foreign routes/table-1000 alias preserved; main/local unchanged'

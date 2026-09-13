#!/bin/sh
set -eu
ROOT="$(CDPATH='' cd "$(dirname "$0")/../../.." && pwd)"; SNAPSHOT="$ROOT/deploy/openwrt/root/usr/libexec/gofro/network"
TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT
cat > "$TMP/ubus" <<'EOF'
#!/bin/sh
case "$2" in
network.interface.lan) echo lan ;;
network.device) case "$4" in "{\"name\":\"$GOFRO_TEST_DEVICE\"}") echo device ;; *) echo wan-device ;; esac ;;
network.interface.wan6) [ "${WAN_MISSING:-0}" = 0 ] || exit 1; echo wan6 ;;
*) [ "${WAN_MISSING:-0}" = 0 ] || exit 1; echo wan ;;
esac
EOF
cat > "$TMP/jsonfilter" <<'EOF'
#!/bin/sh
read -r status
if [ "$status" = device ]; then printf '%s\n' "${BRIDGE_MEMBERS:-}"; exit; fi
if [ "$status" = wan-device ]; then printf '%s\n' "${WAN_BRIDGE_MEMBERS:-}"; exit; fi
if [ "$status" = wan ]; then printf '%s\n' "${WAN_DEVICE:-eth1}"; exit; fi
if [ "$status" = wan6 ]; then printf '%s\n' "${WAN6_DEVICE:-eth1}"; exit; fi
case "$*" in *'.up'*) echo true ;; *'.proto'*) echo "${LAN_PROTO:-static}" ;; *'.mask'*) printf '%s\n' "$GOFRO_TEST_PREFIX" ;; *'ipv4-address'*) printf '%s\n' "$GOFRO_TEST_ADDRESS" ;; *'.l3_device'*) printf '%s\n' "$GOFRO_TEST_DEVICE" ;; esac
EOF
cat > "$TMP/uci" <<'EOF'
#!/bin/sh
case "$*" in
*'show firewall'*) printf '%s\n' firewall.defaults=defaults firewall.home=zone; [ "${DUPLICATE:-0}" = 0 ] || echo firewall.other=zone ;;
*'firewall.home.network'*) echo "${ZONE_MEMBERS:-lan}" ;;
*'firewall.home.name'*|*'firewall.other.name'*) echo "${ZONE_NAME:-home}" ;;
*'firewall.defaults.flow_offloading_hw'*) echo "${OFFLOAD_HW:-0}" ;;
*'firewall.defaults.flow_offloading'*) echo "${OFFLOAD:-0}" ;;
*'network.wan.device'*) echo "${WAN_CONFIG_DEVICE:-eth1}" ;;
*'network.wan6.device'*) echo "${WAN6_CONFIG_DEVICE:-@wan}" ;;
*'get network.wan'|*'get network.wan6') echo interface ;;
esac
exit 0
EOF
cat > "$TMP/ipcalc.sh" <<'EOF'
#!/bin/sh
case "$1" in 192.168.44.7/24) echo NETWORK=192.168.44.0 ;; 172.22.7.1/24) echo NETWORK=172.22.7.0 ;; 10.202.0.5/24) echo NETWORK=10.202.0.0 ;; esac
EOF
chmod +x "$TMP/ubus" "$TMP/jsonfilter" "$TMP/uci" "$TMP/ipcalc.sh"
export GOFRO_TEST_STATUS='{}' GOFRO_TEST_ADDRESS=192.168.44.7 GOFRO_TEST_PREFIX=24 GOFRO_TEST_DEVICE=br-home GOFRO_IPCALC="$TMP/ipcalc.sh"
PATH="$TMP:$PATH" sh "$SNAPSHOT" | grep -Fxq '{"device":"br-home","address":"192.168.44.7","subnet":"192.168.44.0/24","prefix":24,"zone":"home"}'
export GOFRO_TEST_ADDRESS=172.22.7.1 GOFRO_TEST_DEVICE=br-office
PATH="$TMP:$PATH" sh "$SNAPSHOT" | grep -Fq '"device":"br-office"'
export GOFRO_TEST_ADDRESS=10.202.0.5 GOFRO_TEST_DEVICE=br-lan
if PATH="$TMP:$PATH" sh "$SNAPSHOT" >/dev/null 2>&1; then exit 1; fi
export GOFRO_TEST_ADDRESS=192.168.44.7 GOFRO_TEST_DEVICE=br-home
reject() {
	if env PATH="$TMP:$PATH" "$@" sh "$SNAPSHOT" >"$TMP/out" 2>"$TMP/error"; then echo "accepted unsafe network: $*" >&2; exit 1; fi
	[ ! -s "$TMP/out" ]
}
reject LAN_PROTO=pppoe
reject LAN_PROTO=dhcp WAN_DEVICE=br-home
reject WAN_CONFIG_DEVICE=br-home
reject WAN6_DEVICE=br-home
reject WAN6_CONFIG_DEVICE=br-home
reject WAN_CONFIG_DEVICE=@lan
reject BRIDGE_MEMBERS=eth1
reject WAN_BRIDGE_MEMBERS=br-home
reject BRIDGE_MEMBERS=lan1 WAN_BRIDGE_MEMBERS=lan1
reject WAN_MISSING=1
reject ZONE_MEMBERS='lan wan'
reject ZONE_MEMBERS='lan wan6'
reject ZONE_NAME=wan
reject DUPLICATE=1
reject OFFLOAD=1
grep -Fq 'flow_offloading is incompatible' "$TMP/error"
reject OFFLOAD_HW=1
grep -Fq 'flow_offloading_hw is incompatible' "$TMP/error"
GOFRO_TEST_DEVICE=eth0 PATH="$TMP:$PATH" sh "$SNAPSHOT" | grep -Fq '"device":"eth0"'
BRIDGE_MEMBERS='lan1 lan2' PATH="$TMP:$PATH" sh "$SNAPSHOT" | grep -Fq '"device":"br-home"'
echo 'PASS network identity, zone and offloading checks'

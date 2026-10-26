#!/usr/bin/env bash
set -euo pipefail
# Run only inside a disposable root/user network+mount namespace, like warp-netns.
[[ $EUID == 0 ]] || exit 1
ROOT="$(CDPATH='' cd "$(dirname "$0")/../../.." && pwd)"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
sed '/^case ${1:-} in/,$d' "$ROOT/deploy/server/gofro-warp" > "$TMP/functions"
# shellcheck disable=SC1091
source "$TMP/functions"
STATE=$TMP/state
mkdir -m 700 "$STATE"
printf 'WG_INTERFACE=gt0\nWAN_INTERFACE=eth0\n' > "$STATE/network"
chmod 600 "$STATE/network"
ip link add gt0 type dummy
ip link set gt0 up
ip link add eth0 type dummy
ip address add 203.0.113.1/24 dev eth0
ip link set eth0 up
network_up
check_routes
ip -n gofro-warp link show dev gofro-warp1 >/dev/null
ip netns exec gofro-warp nft list table inet gofro_warp | grep -Fq 'policy drop'
[[ $(ip -4 -N rule show | grep '^10900:') == *'from 10.202.1.0/24 iif gt0 lookup 10900' ]]
# A failed start must not overwrite existing interfaces.
if (network_up >/dev/null 2>&1); then exit 1; fi
ip link show dev gofro-warp0 >/dev/null

# ExecStopPost can run after preflight failure. It must not replace foreign state.
ip rule delete pref 10900
ip rule add pref 10900 lookup 10901
before=$(ip -4 -N -o route show table 10900)
if (network_down >/dev/null 2>&1); then exit 1; fi
[[ $(ip -4 -N -o route show table 10900) == "$before" ]]
ip link show dev gofro-warp0 >/dev/null
ip rule delete pref 10900
ip rule add pref 10900 from 10.202.1.0/24 iif gt0 lookup 10900
network_down
check_routes
if ip link show dev gofro-warp0 >/dev/null 2>&1; then exit 1; fi
ip -4 -N rule show | grep -q '^10900:'
ip -4 -o route show table 10900 | grep -Fq 'unreachable default'
network_up
check_routes
network_down
printf 'PASS: production WARP network lifecycle, exact ownership, foreign-state refusal and persistent fail-closed route\n'

#!/usr/bin/env bash
set -euo pipefail
ROOT="$(CDPATH='' cd "$(dirname "$0")/../../.." && pwd)"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
HELPER=${GOFRO_WARP_TEST_HELPER:-$ROOT/deploy/server/gofro-warp}
bash "$HELPER" render-firewall gt0 eth0 > "$TMP/server.nft"
bash "$HELPER" render-namespace-firewall > "$TMP/namespace.nft"
bash "$HELPER" render-legacy-firewall gt0 eth0 > "$TMP/legacy.nft"
# shellcheck disable=SC2016
sed -n '/^table inet gofro_server {$/,/^}$/p' "$ROOT/deploy/server/install.sh" |
  sed 's/\$WG_INTERFACE/gt0/g; s/\$WAN_INTERFACE/eth0/g' > "$TMP/installer.nft"
cmp "$TMP/legacy.nft" "$TMP/installer.nft"
grep -Fq 'iifname "gt0" ip saddr 10.202.1.0/24 oifname != "gofro-warp0" drop' "$TMP/server.nft"
grep -Fq 'iifname "gofro-warp0" oifname "gt0" ct state established,related accept' "$TMP/server.nft"
grep -Fq 'ip saddr 169.254.203.2 oifname "eth0" masquerade' "$TMP/server.nft"
grep -Fq 'type filter hook forward priority filter; policy drop;' "$TMP/namespace.nft"
grep -Fq 'iifname "gofro-warp1" oifname "CloudflareWARP" ip saddr 10.202.1.0/24 accept' "$TMP/namespace.nft"
grep -Fq 'tcp option maxseg size set 1240' "$TMP/namespace.nft"
if bash "$HELPER" render-firewall 'gt0;id' eth0 >/dev/null 2>&1; then exit 1; fi
if bash "$HELPER" render-firewall gt0 'eth0" accept' >/dev/null 2>&1; then exit 1; fi
if bash "$HELPER" render-firewall gt0 >/dev/null 2>&1; then exit 1; fi

# Functions execute against stubs; no root, Cloudflare registration or networking.
for function in valid_interface check_routes status; do
  sed -n "/^$function() {/,/^}/p" "$HELPER" >> "$TMP/functions"
done
# shellcheck disable=SC1091
source "$TMP/functions"
die() { exit 1; }
# shellcheck disable=SC2034
SOURCES=10.202.1.0/24 TABLE=10900 PEER=169.254.203.2 VETH=gofro-warp0 WG_INTERFACE=gt0
ip() {
  case "$*" in
    '-4 -N rule show') printf '%s\n' "${RULES:-}" ;;
    '-4 -N -o route show table 10900') printf '%s\n' "${ROUTES:-}" ;;
    '-4 -N -o route show exact 10.202.1.0/24') printf '%s\n' "${RETURN_ROUTE:-}" ;;
    '-4 -o route show exact 169.254.203.0/30') return 0 ;;
    *) return 1 ;;
  esac
}
check_routes
RULES='10900: from 10.202.1.0/24 iif gt0 lookup 10900'
ROUTES=$'default via 169.254.203.2 dev gofro-warp0 proto 186 metric 10\nunreachable default proto 186 metric 32767'
RETURN_ROUTE='10.202.1.0/24 dev gt0 proto 186 scope 253'
check_routes
for foreign in '10900: from all lookup 10900' '10900: from 10.202.1.0/24 iif gt0 lookup 10901'; do
  RULES=$foreign
  if (check_routes); then exit 1; fi
done

# A quiet grep can kill a still-writing producer with SIGPIPE under pipefail.
# Drain the whole status stream, even after its first matching line.
STATE=$TMP/status
mkdir -p "$STATE" "$TMP/bin"
touch "$STATE/network" "$STATE/enabled"
printf '#!/bin/sh\nexit 0\n' > "$TMP/bin/warp-cli"
chmod 700 "$TMP/bin/warp-cli"
export PATH="$TMP/bin:$PATH"
read_network() { :; }
systemctl() { [[ $* == 'is-active --quiet warp-svc.service' ]]; }
cli() {
  awk -v status="$STATUS_TEST_REPLY" 'BEGIN {
    print "Status update: " status;
    for (i=0; i<20000; i++) print "additional daemon diagnostic output " i;
  }'
}
STATUS_TEST_REPLY=Connected
reply=$(status)
[[ $reply == *'"connected":true'* ]]
STATUS_TEST_REPLY=Disconnected
reply=$(status)
[[ $reply == *'"connected":false'* ]]
printf 'PASS: WARP firewall rendering, validation, foreign route refusal and SIGPIPE-safe status streams\n'

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
if grep -Fq 'ip saddr 169.254.203.2' "$TMP/server.nft"; then exit 1; fi
grep -Fq 'type filter hook forward priority filter; policy drop;' "$TMP/namespace.nft"
grep -Fq 'iifname "gofro-warp1" oifname "CloudflareWARP" ip saddr 10.202.1.0/24 accept' "$TMP/namespace.nft"
grep -Fq 'tcp option maxseg size set 1380' "$TMP/namespace.nft"
if bash "$HELPER" render-firewall 'gt0;id' eth0 >/dev/null 2>&1; then exit 1; fi
if bash "$HELPER" render-firewall gt0 'eth0" accept' >/dev/null 2>&1; then exit 1; fi
if bash "$HELPER" render-firewall gt0 >/dev/null 2>&1; then exit 1; fi

# Functions execute against stubs; no root, Cloudflare registration or networking.
for function in valid_interface valid_ipv4 read_profile register_profile check_routes owned_wireguard status; do
  sed -n "/^$function() {/,/^}/p" "$HELPER" >> "$TMP/functions"
done
# shellcheck disable=SC1091
source "$TMP/functions"
die() { exit 1; }
# shellcheck disable=SC2034
SOURCES=10.202.1.0/24 TABLE=10900 PEER=169.254.203.2 VETH=gofro-warp0 WG_INTERFACE=gt0 NS=gofro-warp WIREGUARD=CloudflareWARP
# Invoked by the sourced production functions, then replaced for status tests.
# shellcheck disable=SC2317,SC2329
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

# Profile text is data, never shell code or wg-quick hooks. Ownership is tested
# in the root-only kernel regression; these unprivileged tests stub only UID.
protected_file() { [[ -f $1 && ! -L $1 && $(stat -c '%a:%h' "$1") == 600:1 ]]; }
cat > "$TMP/profile" <<'EOF'
[Interface]
PrivateKey = AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=
Address = 172.16.0.2/32, 2606:4700:110::1/128
DNS = 1.1.1.1
MTU = 1280
[Peer]
PublicKey = BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB=
AllowedIPs = 0.0.0.0/0, ::/0
Endpoint = engage.cloudflareclient.com:2408
PersistentKeepalive = 25
EOF
chmod 600 "$TMP/profile"
read_profile "$TMP/profile"
[[ $PROFILE_ADDRESS == 172.16.0.2/32 && $PROFILE_PORT == 2408 && $PROFILE_HOST == engage.cloudflareclient.com ]]
for invalid in '0.0.0.0' '127.0.0.1' '224.0.0.1' '1.2.3.256' '1.2.3.4.' '1.2.3' '1.2.3.-1'; do
  if valid_ipv4 "$invalid"; then exit 1; fi
done
# shellcheck disable=SC2016
for change in 's/:2408/:65536/' 's@172.16.0.2/32@172.16.0.999/32@' 's@0.0.0.0/0,@192.0.2.0/24,@' 's/engage.cloudflareclient.com/$(id)/'; do
  sed "$change" "$TMP/profile" > "$TMP/invalid"
  chmod 600 "$TMP/invalid"
  if (read_profile "$TMP/invalid"); then exit 1; fi
done
for extra in 'PostUp = touch /tmp/never-execute-warp-profile' 'PublicKey = BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB=' '[Peer]'; do
  cp "$TMP/profile" "$TMP/invalid"
  printf '%s\n' "$extra" >> "$TMP/invalid"
  if (read_profile "$TMP/invalid"); then exit 1; fi
done
chmod 644 "$TMP/invalid"
if (read_profile "$TMP/invalid"); then exit 1; fi
ln -s "$TMP/profile" "$TMP/profile-link"
if (read_profile "$TMP/profile-link"); then exit 1; fi

# Registration is explicit, private, bounded, reusable, and never a payload
# daemon. The fake utility performs no network requests.
STATE=$TMP/registration
mkdir -m 700 "$STATE"
export WARP_TEST_PROFILE=$TMP/profile WARP_TEST_CALLS=$TMP/register-calls
WGCF=$TMP/fake-wgcf
cat > "$WGCF" <<'EOF'
#!/usr/bin/env bash
set -eu
printf '%s\n' "$*" >> "$WARP_TEST_CALLS"
[[ $1 == --config ]]
account=$2
shift 2
case "$*" in
  'register --accept-tos') printf 'private-registration\n' > "$account" ;;
  'generate --keepalive 25 --profile '*) cp "$WARP_TEST_PROFILE" "$5" ;;
  *) exit 1 ;;
esac
EOF
chmod 700 "$WGCF"
register_profile
[[ $(wc -l < "$WARP_TEST_CALLS") == 2 ]]
[[ $(stat -c '%a' "$STATE/account.toml") == 600 && $(stat -c '%a' "$STATE/profile.conf") == 600 ]]
register_profile
[[ $(wc -l < "$WARP_TEST_CALLS") == 2 ]]
rm "$STATE/profile.conf"
register_profile
[[ $(wc -l < "$WARP_TEST_CALLS") == 3 ]]

# Quiet grep can kill a still-writing producer under pipefail. Both ownership
# filters must drain large streams. A fresh kernel handshake, not a process or
# link's mere presence, is required for connected=true.
STATE=$TMP/status
mkdir -p "$STATE"
touch "$STATE/network" "$STATE/enabled" "$STATE/profile.conf"
chmod 600 "$STATE/profile.conf"
read_network() { :; }
systemctl() { [[ $* == 'is-active --quiet gofro-warp.service' ]]; }
ip() {
  case "$*" in
    '-d link show dev CloudflareWARP')
      awk 'BEGIN { print "alias gofro-warp-wireguard-v1 wireguard"; for(i=0;i<20000;i++) print "additional link output " i }' ;;
    'netns exec gofro-warp wg show CloudflareWARP latest-handshakes') printf 'peer\t%s\n' "$HANDSHAKE" ;;
    *) return 1 ;;
  esac
}
owned_wireguard
HANDSHAKE=$(date +%s)
reply=$(status)
[[ $reply == *'"connected":true'* ]]
for HANDSHAKE in 0 1 999999999999 invalid; do
  reply=$(status)
  [[ $reply == *'"connected":false'* ]]
done
HANDSHAKE=$(date +%s)
rm "$STATE/enabled"
reply=$(status)
[[ $reply == *'"enabled":false,"connected":false'* ]]
printf 'PASS: WARP firewall, strict WireGuard profiles, private reusable registration, fresh handshake status, foreign route refusal and SIGPIPE-safe ownership\n'

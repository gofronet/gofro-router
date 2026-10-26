#!/usr/bin/env bash
set -Eeuo pipefail
trap 'printf "FAIL: native WARP fixture line %s: %s\n" "$LINENO" "$BASH_COMMAND" >&2' ERR
# Only inside a disposable root/user network+mount namespace, never host netns.
[[ $EUID == 0 ]] || exit 1
ROOT="$(CDPATH='' cd "$(dirname "$0")/../../.." && pwd)"
TMP=$(mktemp -d)
PIDS=()
cleanup() {
  for pid in "${PIDS[@]}"; do kill "$pid" 2>/dev/null || true; wait "$pid" 2>/dev/null || true; done
  rm -rf "$TMP"
}
trap cleanup EXIT
sed '/^case ${1:-} in/,$d' "$ROOT/deploy/server/gofro-warp" > "$TMP/functions"
# shellcheck disable=SC1091
source "$TMP/functions"
STATE=$TMP/state
mkdir -m 700 "$STATE"
printf 'WG_INTERFACE=gt0\nWAN_INTERFACE=eth0\n' > "$STATE/network"
chmod 600 "$STATE/network"
ip netns add warp-test-router
ip netns add warp-test-cloud
ip link add gt0 type veth peer name router-lan
ip link set router-lan netns warp-test-router
ip address add 10.202.0.1/24 dev gt0
ip link set gt0 up
ip -n warp-test-router address add 10.202.0.6/24 dev router-lan
ip -n warp-test-router address add 10.202.1.6/32 dev router-lan
ip -n warp-test-router link set router-lan up
ip -n warp-test-router route add default via 10.202.0.1
ip link add eth0 type veth peer name cloud-wan
ip link set cloud-wan netns warp-test-cloud
ip address add 203.0.113.1/24 dev eth0
ip link set eth0 up
ip -n warp-test-cloud address add 203.0.113.2/24 dev cloud-wan
ip -n warp-test-cloud link set cloud-wan up
ip -n warp-test-cloud link set lo up
ip -n warp-test-cloud address add 198.51.100.1/32 dev lo
ip route add default via 203.0.113.2
sysctl -qw net.ipv4.ip_forward=1 net.ipv4.conf.all.rp_filter=0 net.ipv4.conf.default.rp_filter=0
umask 077
wg genkey > "$TMP/client.key"
wg genkey > "$TMP/cloud.key"
client_public=$(wg pubkey < "$TMP/client.key")
cloud_public=$(wg pubkey < "$TMP/cloud.key")
ip -n warp-test-cloud link add wg-cloud type wireguard
ip netns exec warp-test-cloud wg set wg-cloud private-key "$TMP/cloud.key" listen-port 2408 peer "$client_public" allowed-ips 172.16.0.2/32
ip -n warp-test-cloud address add 192.0.2.1/32 dev wg-cloud
ip -n warp-test-cloud link set wg-cloud mtu 1420 up
ip -n warp-test-cloud route add 172.16.0.2/32 dev wg-cloud
cat > "$STATE/profile.conf" <<EOF
[Interface]
PrivateKey = $(< "$TMP/client.key")
Address = 172.16.0.2/32, 2606:4700:110::1/128
DNS = 1.1.1.1
MTU = 1280
[Peer]
PublicKey = $cloud_public
AllowedIPs = 0.0.0.0/0, ::/0
Endpoint = 203.0.113.2:2408
PersistentKeepalive = 25
EOF
read_network
render_firewall | nft -f -
network_up
[[ -z $(ip -n gofro-warp route show default) ]]
default_before=$(ip -4 route show default)
resolver_before=$(sha256sum /etc/resolv.conf)

# The production connect path verifies a trace request. Here the endpoint is
# a local HTTP/UDP fixture reached through a real encrypted kernel WG tunnel,
# not Cloudflare. The wrapper changes only the verification URL for this test.
cat > "$TMP/server.py" <<'PY'
import http.server, socket, threading
class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        peer = self.client_address[0]
        body = f"ip={peer}\nwarp={'on' if peer == '172.16.0.2' else 'off'}\n".encode()
        self.send_response(200)
        self.end_headers()
        self.wfile.write(body)
    def log_message(self, *args): pass
def udp():
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as server:
        server.bind(('198.51.100.1', 8081))
        while True:
            data, peer = server.recvfrom(65535)
            server.sendto(f'{peer[0]} {len(data)}'.encode(), peer)
threading.Thread(target=udp, daemon=True).start()
http.server.ThreadingHTTPServer(('198.51.100.1', 8080), Handler).serve_forever()
PY
ip netns exec warp-test-cloud python3 "$TMP/server.py" &
PIDS+=("$!")
mkdir "$TMP/bin"
real_curl=$(command -v curl)
printf '#!/bin/sh\nexec "%s" -4 --noproxy "*" --fail --silent --max-time 2 http://198.51.100.1:8080/trace\n' "$real_curl" > "$TMP/bin/curl"
chmod 700 "$TMP/bin/curl"
export PATH="$TMP/bin:$PATH"
connect
[[ $(ip -4 route show default) == "$default_before" && $(sha256sum /etc/resolv.conf) == "$resolver_before" ]]
owned_wireguard -n gofro-warp
[[ $(ip -n gofro-warp -o link show CloudflareWARP) == *'mtu 1420'* ]]
[[ $(ip -n gofro-warp route show default) == 'default dev CloudflareWARP scope link'* ]]
touch "$STATE/enabled"
# Invoked by the sourced status function before the publication-test stub below.
# shellcheck disable=SC2317,SC2329
systemctl() { [[ $* == 'is-active --quiet gofro-warp.service' ]]; }
[[ $(status) == *'"connected":true'* ]]

cat > "$TMP/client.py" <<'PY'
import socket, sys
source, expected, mode = sys.argv[1:]
if mode == 'tcp':
    with socket.create_connection(('198.51.100.1', 8080), timeout=2, source_address=(source, 0)) as client:
        client.sendall(b'GET /trace HTTP/1.0\r\nHost: fixture\r\n\r\n')
        chunks = []
        while data := client.recv(4096): chunks.append(data)
        reply = b''.join(chunks).decode()
        assert f'ip={expected}\n' in reply, reply
else:
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as client:
        client.settimeout(2)
        client.bind((source, 0))
        client.sendto(b'x' * 1351, ('198.51.100.1', 8081))
        reply = client.recv(1024).decode()
        assert reply == f'{expected} 1351', reply
PY
probe() { ip netns exec warp-test-router python3 "$TMP/client.py" "$@"; }
for mode in tcp udp; do
  probe 10.202.1.6 172.16.0.2 "$mode"
  probe 10.202.0.6 203.0.113.1 "$mode"
done
# A missing policy rule must not send WARP identity through the normal WAN.
ip rule delete pref 10900
if probe 10.202.1.6 203.0.113.1 udp >/dev/null 2>&1; then exit 1; fi
probe 10.202.0.6 203.0.113.1 udp
ip rule add pref 10900 from 10.202.1.0/24 iif gt0 lookup 10900
# Teardown refuses a foreign host interface even when an owned one is in netns.
ip link add CloudflareWARP type dummy
if (wireguard_down >/dev/null 2>&1); then exit 1; fi
ip link show CloudflareWARP >/dev/null
owned_wireguard -n gofro-warp
ip link delete CloudflareWARP
wireguard_down
[[ $(status) == *'"connected":false'* ]]
for mode in tcp udp; do
  if probe 10.202.1.6 203.0.113.1 "$mode" >/dev/null 2>&1; then exit 1; fi
  probe 10.202.0.6 203.0.113.1 "$mode"
done
network_down
network_up
connect
probe 10.202.1.6 172.16.0.2 udp
network_down
probe 10.202.0.6 203.0.113.1 tcp
[[ $(ip -4 route show default) == "$default_before" && $(sha256sum /etc/resolv.conf) == "$resolver_before" ]]

# Unit/resolver publication runs only in this test's private mount namespace.
# Hiding /etc here must never touch the operator's systemd or DNS files.
mkdir -p "$TMP/etc/systemd/system" "$TMP/etc/netns"
mount --bind "$TMP/etc" /etc
systemctl() { [[ $* == daemon-reload ]]; }
write_units
write_units
grep -Fxq 'ExecStopPost=/usr/local/sbin/gofro-warp disconnect gt0' /etc/systemd/system/gofro-warp.service
cp /etc/systemd/system/gofro-warp.service "$TMP/unit-original"
printf '# operator customization\n' >> /etc/systemd/system/gofro-warp.service
custom=$(sha256sum /etc/systemd/system/gofro-warp.service)
if (write_units >/dev/null 2>&1); then exit 1; fi
[[ $(sha256sum /etc/systemd/system/gofro-warp.service) == "$custom" ]]
cp "$TMP/unit-original" /etc/systemd/system/gofro-warp.service
cp /etc/netns/gofro-warp/resolv.conf "$TMP/resolver-sentinel"
before=$(sha256sum "$TMP/resolver-sentinel")
rm /etc/netns/gofro-warp/resolv.conf
ln -s "$TMP/resolver-sentinel" /etc/netns/gofro-warp/resolv.conf
if (write_units >/dev/null 2>&1); then exit 1; fi
[[ -L /etc/netns/gofro-warp/resolv.conf && $(sha256sum "$TMP/resolver-sentinel") == "$before" ]]
rm /etc/netns/gofro-warp/resolv.conf
cp "$TMP/resolver-sentinel" /etc/netns/gofro-warp/resolv.conf
chmod 777 /etc/netns/gofro-warp
if (write_units >/dev/null 2>&1); then exit 1; fi
[[ $(stat -c '%a' /etc/netns/gofro-warp) == 777 ]]
printf 'PASS: production kernel WireGuard encryption, host socket isolation, TCP/1351-byte UDP, fresh handshake, restart, foreign interface/unit/resolver refusal and fail-closed teardown; no real Cloudflare registration\n'

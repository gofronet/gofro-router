#!/bin/sh
set -eu

# Isolated Ethernet/PPPoE-device simulation, NOT PPPoE negotiation.
# Fixtures come from emit_netns_fixtures, never a substitute classifier.
# Contract: routing.nft = VPN on/rules; all.nft = VPN on/all;
# off.nft = VPN off/rules; guard.nft = render_guard, all with lan0/5353
# and the emitter's existing direct fake-IP mapping. Missing coverage is fatal.
ROOT="$(CDPATH='' cd "$(dirname "$0")/../../.." && pwd)"
case "${1:-}" in
	--emit-only|'')
		command -v cargo >/dev/null
		if [ -n "${FIXTURES:-}" ]; then
			[ -d "$FIXTURES" ] || { printf '%s\n' 'FIXTURES must be an existing directory' >&2; exit 1; }
		else
			FIXTURES="$(mktemp -d)"
			trap 'rm -rf "$FIXTURES"' EXIT
		fi
		(
			cd "$ROOT"
			GOFRO_NFT_FIXTURE_DIR="$FIXTURES" cargo test --offline --locked -p gofro-agent \
				dataplane::tests::emit_netns_fixtures -- --exact --ignored
		)
		for fixture in routing all off guard; do
			[ -s "$FIXTURES/$fixture.nft" ] || {
				printf 'FAIL: renderer fixture missing: %s.nft\n' "$fixture" >&2; exit 1;
			}
		done
		if [ "${1:-}" = --emit-only ]; then
			printf '%s\n' 'PASS: fixture emission only; Linux kernel regression NOT RUN'
			exit 0
		fi
		[ "$(uname -s)" = Linux ] || {
			printf '%s\n' 'BLOCKED: kernel regression requires Linux (not a successful SKIP)' >&2; exit 1;
		}
		if [ "$(id -u)" = 0 ]; then
			env FIXTURES="$FIXTURES" sh "$0" --run
		else
			sudo -n env FIXTURES="$FIXTURES" sh "$0" --run
		fi
		exit 0 ;;
	--run) ;;
	*) printf 'usage: %s [--emit-only|--run]\n' "$0" >&2; exit 1 ;;
esac

[ "$(uname -s)" = Linux ] || { printf '%s\n' 'BLOCKED: --run requires Linux' >&2; exit 1; }
[ "$(id -u)" = 0 ] || { printf '%s\n' '--run requires root' >&2; exit 1; }
for command in ip nft python3 sysctl; do
	command -v "$command" >/dev/null || { printf 'missing required command: %s\n' "$command" >&2; exit 1; }
done
for fixture in routing all off guard; do
	[ -s "${FIXTURES:?}/$fixture.nft" ] || { printf 'missing fixture: %s.nft\n' "$fixture" >&2; exit 1; }
done

tag="gfr$$"
r="${tag}r" c="${tag}c" w="${tag}w" v="${tag}v"
owned=''
cleanup() {
	for ns in $owned; do
		for pid in $(ip netns pids "$ns"); do kill "$pid" 2>/dev/null || true; done
		ip netns del "$ns"
	done
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
for ns in "$r" "$c" "$w" "$v"; do
	ip netns add "$ns"
	owned="$owned $ns"
	ip -n "$ns" link set lo up
done
# Create both ends inside owned namespaces, never in the host network namespace.
ip -n "$r" link add lan0 type veth peer name client0 netns "$c"
ip -n "$r" link add pppoe-wan type veth peer name wan0 netns "$w"
ip -n "$r" link add gt0 type veth peer name vpn0 netns "$v"
ip -n "$r" link set lan0 up
ip -n "$c" link set client0 up
ip -n "$r" link set pppoe-wan mtu 1492 up
ip -n "$w" link set wan0 mtu 1492 up
ip -n "$r" link set gt0 mtu 1480 up
ip -n "$v" link set vpn0 mtu 1480 up
ip -n "$r" addr add 192.168.0.1/24 dev lan0
ip -n "$c" addr add 192.168.0.2/24 dev client0
ip -n "$c" route add default via 192.168.0.1
ip -n "$r" addr add 2001:db8:1::1/64 dev lan0 nodad
ip -n "$c" addr add 2001:db8:1::2/64 dev client0 nodad
ip -n "$c" route add default via 2001:db8:1::1
ip -n "$r" addr add 192.0.2.1 peer 192.0.2.2/32 dev pppoe-wan
ip -n "$w" addr add 192.0.2.2 peer 192.0.2.1/32 dev wan0
for address in 198.51.100.2 203.0.113.2 8.8.8.8 9.9.9.9 1.1.1.1; do
	ip -n "$w" addr add "$address/32" dev lo
done
ip -n "$w" route add default via 192.0.2.1 dev wan0
ip -n "$r" addr add 10.0.0.1/24 dev gt0
ip -n "$v" addr add 10.0.0.2/24 dev vpn0
ip -n "$v" addr add 8.8.8.8/32 dev lo
ip -n "$v" addr add 198.51.100.2/32 dev lo
ip -n "$r" addr add 2001:db8:2::1/64 dev gt0 nodad
ip -n "$v" addr add 2001:db8:2::2/64 dev vpn0 nodad
ip -n "$v" route add 192.168.0.0/24 via 10.0.0.1
ip -n "$v" route add 2001:db8:1::/64 via 2001:db8:2::1
ip netns exec "$r" sysctl -q -w net.ipv4.ip_forward=1 net.ipv6.conf.all.forwarding=1
# Strict reverse-path filtering would reject asymmetric policy-routed replies.
for device in all default lan0 pppoe-wan gt0; do
	ip netns exec "$r" sysctl -q -w "net.ipv4.conf.$device.rp_filter=0"
done

# A veth needs a next hop, unlike a negotiated point-to-point PPP link.
ip -n "$r" route replace default via 192.0.2.2 dev pppoe-wan mtu 1492
ip -n "$r" rule add pref 80 fwmark 0x10000/0x30000 lookup main
ip -n "$r" rule add pref 81 fwmark 0x20000/0x30000 lookup 100
ip -n "$r" route replace default dev gt0 table 100 metric 10 proto 186 mtu 1480
ip -n "$r" route replace unreachable default table 100 metric 32767 proto 186
ip -n "$r" route replace 192.168.0.0/24 dev lan0 table 100 proto 186 mtu 1500

python3 - "$r" "$c" "$w" "$v" "$FIXTURES" <<'PY'
import json
import pathlib
import select
import subprocess
import sys

r, c, w, v, fixtures = sys.argv[1:]
processes = []
traffic_checks = 0
udp_retries = 0

def run(ns, *args, **kwargs):
    return subprocess.run(["ip", "netns", "exec", ns, *args], check=True,
                          text=True, timeout=15, **kwargs)

def nft(script):
    run(r, "nft", "-f", "-", input=script)

def load(name):
    print("Loading rendered fixture:", name, flush=True)
    path = str(pathlib.Path(fixtures) / (name + ".nft"))
    run(r, "nft", "--check", "-f", path)
    run(r, "nft", "-f", path)

def counter(name):
    data = json.loads(run(r, "nft", "-j", "list", "counter", "inet", "observe", name,
                          capture_output=True).stdout)
    return next(item["counter"]["packets"] for item in data["nftables"] if "counter" in item)

def start(ns, code, *args):
    p = subprocess.Popen(["ip", "netns", "exec", ns, "python3", "-u", "-c", code, *args],
                         stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
    processes.append(p)
    assert select.select([p.stdout], [], [], 5)[0], "endpoint startup timed out"
    assert p.stdout.readline().strip() == "ready", "endpoint failed to start"
    return p

# Identity-tagged echo endpoints test DNS transport redirection, not the resolver implementation.
server = r'''
import json, socket, sys, threading
def tcp_client(c, label):
    with c:
        try:
            with c.makefile("rb") as stream:
                for data in stream: c.sendall(label + data)
        except OSError: pass
def serve(s, label, udp):
    while True:
        if udp:
            data, peer = s.recvfrom(4096); s.sendto(label + data, peer)
        else:
            c, _ = s.accept()
            threading.Thread(target=tcp_client, args=(c, label), daemon=True).start()
for address, port, label in json.loads(sys.argv[1]):
    for kind in (socket.SOCK_STREAM, socket.SOCK_DGRAM):
        s = socket.socket(socket.AF_INET6 if ":" in address else socket.AF_INET, kind)
        s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        if s.family == socket.AF_INET6:
            s.setsockopt(socket.IPPROTO_IPV6, socket.IPV6_V6ONLY, 0)
        if len(sys.argv) > 2:
            s.setsockopt(socket.SOL_SOCKET, socket.SO_BINDTODEVICE, sys.argv[2].encode() + b"\0")
        s.bind((address, port))
        if kind == socket.SOCK_STREAM: s.listen()
        threading.Thread(target=serve, args=(s, label.encode(), kind == socket.SOCK_DGRAM), daemon=True).start()
print("ready")
threading.Event().wait()
'''
client = r'''
import socket, sys
address, port, proto, expected, source_port = sys.argv[1:]
s = socket.socket(socket.AF_INET6 if ":" in address else socket.AF_INET,
                  socket.SOCK_DGRAM if proto == "udp" else socket.SOCK_STREAM)
s.settimeout(1 if expected == "blocked" else 3)
s.bind(("::" if ":" in address else "0.0.0.0", int(source_port)))
try:
    s.connect((address, int(port)))
    s.sendall(b"probe\n")
    data = s.recv(4096)
    if proto == "tcp":
        while data and not data.endswith(b"\n"):
            chunk = s.recv(4096)
            if not chunk: break
            data += chunk
except OSError:
    if expected != "blocked": raise
else:
    assert expected != "blocked", (address, proto, "unexpected response", data)
    assert data == expected.encode() + b"probe\n", (address, proto, data, expected)
finally:
    s.close()
'''

source_port = 30000

def probe(ns, address, expected, port=8080):
    global source_port, traffic_checks
    for proto in ("tcp", "udp"):
        source_port += 1
        run(ns, "python3", "-c", client, address, str(port), proto, expected, str(source_port))
        traffic_checks += 1

hold = r'''
import socket, sys
address, port, source = sys.argv[1:4]
udp = sys.argv[4:5] == ["udp"]
s = socket.socket(type=socket.SOCK_DGRAM if udp else socket.SOCK_STREAM); s.settimeout(2)
if udp: s.bind(("192.168.0.2", int(sys.argv[5])))
if source: s.bind((source, 0))
s.connect((address, int(port)))
assert s.getpeername() == (address, int(port))
print("ready")
for line in sys.stdin:
    try:
        s.sendall(line.encode())
        data = b""
        while not data.endswith(b"\n"):
            chunk = s.recv(4096)
            if not chunk: raise ConnectionResetError()
            data += chunk
        print(data.decode().strip())
    except OSError: print("blocked")
'''

def exchange(p, expected, reclassified=None, forbidden=None):
    global traffic_checks, udp_retries
    if reclassified:
        assert "udp" in p.args and forbidden, "NAT-switch retry is UDP-only and requires a leak check"
        leaked_before = counter(forbidden)
    for attempt in range(2 if reclassified else 1):
        if reclassified:
            marked_before = counter(reclassified)
        token = f"same-connection-{traffic_checks}-{attempt}"
        p.stdin.write(token + "\n"); p.stdin.flush()
        assert select.select([p.stdout], [], [], 5)[0], "persistent socket exchange timed out"
        actual = p.stdout.readline().strip()
        if reclassified:
            assert counter(reclassified) > marked_before, "packet did not receive revised meta/ct marks"
            assert counter(forbidden) == leaked_before, "reclassified UDP leaked to the wrong interface"
            assert counter("bad_meta") == counter("bad_ct") == 0, "reclassification clobbered foreign marks"
        # nf_nat_oif_changed kills a masqueraded conntrack and drops the first
        # interface-switch packet. Retry the same UDP socket once, never TCP.
        if reclassified and actual == "blocked" and attempt == 0:
            udp_retries += 1
            print("Retrying one UDP NAT-switch drop:", reclassified, flush=True)
            continue
        assert actual == expected.replace("same-connection", token), ("persistent socket", actual, expected)
        break
    traffic_checks += 1

try:
    # Simulated fw4 environment only: WAN port forwarding and WAN masquerading.
    # No test-written routing classification, DNS interception, or fail-closed rule.
    nft('''
add table ip fw4_simulation
add chain ip fw4_simulation dstnat { type nat hook prerouting priority dstnat; policy accept; }
add rule ip fw4_simulation dstnat iifname "pppoe-wan" ip daddr 192.0.2.1 tcp dport 18080 dnat to 192.168.0.2:8080
add chain ip fw4_simulation srcnat { type nat hook postrouting priority srcnat; policy accept; }
add rule ip fw4_simulation srcnat oifname "pppoe-wan" masquerade
add table inet observe
add counter inet observe bad_meta
add counter inet observe bad_ct
add counter inet observe direct
add counter inet observe vpn
add counter inet observe port_forward
add counter inet observe wan_leak
add counter inet observe blocked_leak
add counter inet observe ipv6_leak
add counter inet observe forwarded
add counter inet observe mode_direct
add counter inet observe mode_vpn
add counter inet observe original_direct
add counter inet observe revised_vpn
add counter inet observe original_wan_egress
add counter inet observe original_vpn_egress
add counter inet observe mode_wan_egress
add counter inet observe mode_vpn_egress
add chain inet observe seed { type filter hook prerouting priority -151; policy accept; }
add rule inet observe seed iifname "lan0" meta mark set meta mark | 0x4
add rule inet observe seed ct mark set ct mark | 0x8
add chain inet observe marks { type filter hook prerouting priority -149; policy accept; }
add rule inet observe marks iifname "lan0" meta nfproto ipv4 meta mark & 0xfffcffff != 0x4 counter name bad_meta
add rule inet observe marks iifname "lan0" meta nfproto ipv4 ct mark & 0xfffcffff != 0x8 counter name bad_ct
add rule inet observe marks iifname "lan0" meta mark 0x10004 ct mark 0x10008 counter name direct
add rule inet observe marks iifname "lan0" meta mark 0x20004 ct mark 0x20008 counter name vpn
add rule inet observe marks iifname "lan0" ip daddr 8.8.8.8 udp sport 31000 meta mark 0x10004 ct mark 0x10008 counter name mode_direct
add rule inet observe marks iifname "lan0" ip daddr 8.8.8.8 udp sport 31000 meta mark 0x20004 ct mark 0x20008 counter name mode_vpn
add rule inet observe marks iifname "lan0" ip daddr 198.51.100.2 udp sport 31001 meta mark 0x10004 ct mark 0x10008 counter name original_direct
add rule inet observe marks iifname "lan0" ip daddr 198.51.100.2 udp sport 31001 meta mark 0x20004 ct mark 0x20008 counter name revised_vpn
add chain inet observe egress { type filter hook postrouting priority 110; policy accept; }
add rule inet observe egress oifname "pppoe-wan" ip daddr 9.9.9.9 tcp sport 18080 ct direction reply ct status dnat meta mark 0x10004 ct mark 0x10008 counter name port_forward
add rule inet observe egress oifname "pppoe-wan" ip daddr 8.8.8.8 counter name wan_leak
add rule inet observe egress oifname != "lan0" ip daddr 203.0.113.2 counter name blocked_leak
add rule inet observe egress iifname "lan0" meta nfproto ipv6 oifname "gt0" counter name ipv6_leak
add rule inet observe egress iifname "lan0" oifname != "lan0" counter name forwarded
add rule inet observe egress oifname "pppoe-wan" ip daddr 198.51.100.2 udp sport 31001 counter name original_wan_egress
add rule inet observe egress oifname "gt0" ip daddr 198.51.100.2 udp sport 31001 counter name original_vpn_egress
add rule inet observe egress oifname "pppoe-wan" ip daddr 8.8.8.8 udp sport 31000 counter name mode_wan_egress
add rule inet observe egress oifname "gt0" ip daddr 8.8.8.8 udp sport 31000 counter name mode_vpn_egress
''')
    start(w, server, json.dumps([[address, 8080, "wan:"] for address in
                                ("198.51.100.2", "203.0.113.2", "8.8.8.8")]
                               + [["1.1.1.1", 53, "upstream:"]]))
    start(v, server, json.dumps([["8.8.8.8", 8080, "vpn:"], ["198.51.100.2", 8080, "vpn:"],
                                ["2001:db8:2::2", 8080, "ipv6:"], ["2001:db8:2::2", 53, "upstream6:"]]))
    start(c, server, json.dumps([["192.168.0.2", 8080, "lan:"]]))
    # Transport double with the real helper's dual-stack/device-bound shape.
    # The Rust Linux-only socket test separately verifies bind_lan_sockets itself.
    start(r, server, json.dumps([["::", 5353, "fake-dns:"], ["2001:db8:1::1", 546, "lan6:"]]), "lan0")
    start(r, server, json.dumps([["192.168.0.1", 8081, "router-control:"],
                                ["2001:db8:1::1", 8081, "router-control:"]]))

    for address, mark, device, mtu in (("198.51.100.2", "0x10004", "pppoe-wan", 1492),
                                        ("8.8.8.8", "0x20004", "gt0", 1480),
                                        ("192.168.0.2", "0x20004", "lan0", 1500)):
        route = run(r, "ip", "route", "get", address, "mark", mark, capture_output=True).stdout
        assert "dev " + device in route and "mtu " + str(mtu) in route, route

    # Positive controls: negative tests below have reachable, responding endpoints.
    probe(c, "203.0.113.2", "wan:")
    probe(c, "8.8.8.8", "wan:")
    probe(c, "2001:db8:2::2", "ipv6:")
    probe(c, "2001:db8:2::2", "upstream6:", 53)
    probe(c, "192.168.0.1", "fake-dns:", 5353)
    probe(c, "2001:db8:1::1", "fake-dns:", 5353)
    probe(w, "192.168.0.1", "router-control:", 8081)
    probe(v, "2001:db8:1::1", "router-control:", 8081)
    print("PASS: live WAN TCP/UDP, IPv6 forwarding, masked policy routes and MTUs", flush=True)
    load("routing")
    # Same destination, wrong ingress device: neither direct access nor DNS
    # interception may expose the LAN-bound wildcard listener to WAN/VPN peers.
    for port in (53, 5353):
        probe(w, "192.168.0.1", "blocked", port)
        probe(v, "2001:db8:1::1", "blocked", port)
    blocked_before = counter("blocked_leak")
    ipv6_before = counter("ipv6_leak")
    probe(c, "198.51.100.2", "wan:")
    probe(c, "198.18.0.1", "wan:")
    probe(c, "8.8.8.8", "vpn:")
    probe(c, "203.0.113.2", "blocked")
    assert counter("blocked_leak") == blocked_before, "blocked TCP/UDP was forwarded"
    probe(c, "1.1.1.1", "fake-dns:", 53)
    probe(r, "1.1.1.1", "upstream:", 53)
    probe(c, "2001:db8:2::2", "fake-dns:", 53)
    probe(r, "2001:db8:2::2", "upstream6:", 53)
    probe(c, "2001:db8:1::1", "lan6:", 546)
    probe(c, "2001:db8:2::2", "blocked")
    assert counter("ipv6_leak") == ipv6_before, "VPN-on IPv6 forwarding leak"
    print("PASS: rules direct/block/fake-IP/DNS TCP+UDP, no local DNS recursion, IPv6 input/forward split", flush=True)

    # Inbound WAN->LAN DNAT, NOT outbound fake-IP DNAT. The public WAN source
    # is outside direct/local rules: misclassifying the LAN SYN-ACK as a new VPN
    # flow sends it to gt0 and breaks this very same WAN socket.
    inbound = start(w, hold, "192.0.2.1", "18080", "9.9.9.9")
    exchange(inbound, "lan:same-connection")
    assert counter("port_forward") > 0, "DNAT reply tuple/marks did not return through WAN"
    direct_flow = start(c, hold, "198.51.100.2", "8080", "", "udp", "31001")
    exchange(direct_flow, "wan:same-connection")
    assert counter("original_direct") > 0, "original UDP tuple was not direct"
    load("all")
    before = counter("port_forward")
    exchange(inbound, "lan:same-connection")
    fresh_inbound = start(w, hold, "192.0.2.1", "18080", "9.9.9.9")
    exchange(fresh_inbound, "lan:same-connection")
    assert counter("port_forward") > before, "VPN=on/all broke WAN DNAT replies"
    exchange(direct_flow, "vpn:same-connection", "revised_vpn", "original_wan_egress")
    assert counter("revised_vpn") > 0, "rules->all did not reclassify the original direct UDP tuple"
    print("PASS: persistent and new WAN->LAN TCP DNAT replies remain WAN in VPN=on/all", flush=True)
    probe(c, "198.51.100.2", "vpn:")
    probe(c, "8.8.8.8", "vpn:")
    probe(c, "1.1.1.1", "fake-dns:", 53)
    probe(r, "1.1.1.1", "upstream:", 53)
    probe(c, "2001:db8:2::2", "fake-dns:", 53)
    probe(r, "2001:db8:2::2", "upstream6:", 53)
    probe(c, "2001:db8:1::1", "lan6:", 546)
    probe(c, "2001:db8:2::2", "blocked")

    established = start(c, hold, "8.8.8.8", "8080", "")
    assert counter("ipv6_leak") == ipv6_before, "VPN-on/all IPv6 forwarding leak"
    exchange(established, "vpn:same-connection")
    before = counter("wan_leak")
    run(r, "ip", "link", "set", "gt0", "down")
    # Some kernels remove this route on link-down; others leave it linkdown.
    # Model netifd withdrawal in either case without removing the sentinel.
    run(r, "ip", "route", "flush", "table", "100", "dev", "gt0", "exact", "0.0.0.0/0")
    result = subprocess.run(["ip", "-n", r, "route", "get", "8.8.8.8", "mark", "0x20004"],
                            capture_output=True, text=True, timeout=5)
    assert result.returncode != 0, result.stdout
    exchange(established, "blocked")
    probe(c, "8.8.8.8", "blocked")
    assert counter("wan_leak") == before, "established/new VPN packets leaked through WAN"
    probe(c, "198.18.0.1", "wan:")
    exchange(inbound, "lan:same-connection")
    run(r, "ip", "link", "set", "gt0", "up")
    # Link-down can remove IPv6 addresses; restore the simulated netifd topology.
    run(r, "ip", "-6", "addr", "replace", "2001:db8:2::1/64", "dev", "gt0", "nodad")
    run(r, "ip", "route", "replace", "default", "dev", "gt0", "table", "100",
        "metric", "10", "proto", "186", "mtu", "1480")
    probe(c, "8.8.8.8", "vpn:")

    # Guard persists across classifier replacement and loss of the VPN rule.
    # Test new AND established traffic, plus working local DNS/input.
    print("PASS: gt0-down established/new VPN traffic cannot leak WAN; direct and WAN DNAT still work", flush=True)
    mode_flow = start(c, hold, "8.8.8.8", "8080", "", "udp", "31000")
    exchange(mode_flow, "vpn:same-connection")
    for next_mode in ("off", "routing"):
        transition = start(w, hold, "192.0.2.1", "18080", "9.9.9.9")
        exchange(transition, "lan:same-connection")
        load("guard")
        before = counter("forwarded")
        probe(c, "8.8.8.8", "blocked")
        run(r, "ip", "rule", "del", "pref", "81")
        probe(c, "8.8.8.8", "blocked")
        exchange(transition, "blocked")
        exchange(mode_flow, "blocked")
        load(next_mode)
        exchange(mode_flow, "blocked")
        probe(c, "198.51.100.2", "blocked")
        probe(c, "198.18.0.1", "blocked")
        probe(c, "2001:db8:2::2", "blocked")
        probe(c, "1.1.1.1", "fake-dns:", 53)
        probe(r, "1.1.1.1", "upstream:", 53)
        probe(c, "2001:db8:2::2", "fake-dns:", 53)
        probe(r, "2001:db8:2::2", "upstream6:", 53)
        probe(c, "2001:db8:1::1", "lan6:", 546)
        assert counter("forwarded") == before, "transition guard allowed LAN egress"
        run(r, "ip", "rule", "add", "pref", "81", "fwmark", "0x20000/0x30000", "lookup", "100")
        nft("destroy table inet gofro_guard\n")
        mode_counter = "mode_direct" if next_mode == "off" else "mode_vpn"
        before = counter(mode_counter)
        exchange(mode_flow, ("wan:" if next_mode == "off" else "vpn:") + "same-connection",
                 mode_counter, "mode_vpn_egress" if next_mode == "off" else "mode_wan_egress")
        assert counter(mode_counter) > before, "same conntrack tuple was not reclassified"
        # No traffic from this originally direct tuple during the guard/config
        # window: its first packet after the change must not reuse stale ct marks.
        before = counter("original_direct")
        exchange(direct_flow, "wan:same-connection", "original_direct", "original_vpn_egress")
        assert counter("original_direct") > before, "idle original direct UDP tuple kept its VPN mark"
        exchange(inbound, "lan:same-connection")
        probe(c, "198.51.100.2", "wan:")
        probe(c, "8.8.8.8", "wan:" if next_mode == "off" else "vpn:")
        before = counter("blocked_leak")
        probe(c, "203.0.113.2", "blocked")
        assert counter("blocked_leak") == before, "mode transition bypassed the block rule"
        before = counter("ipv6_leak")
        probe(c, "2001:db8:2::2", "ipv6:" if next_mode == "off" else "blocked")
        assert (counter("ipv6_leak") > before) == (next_mode == "off"), "IPv6 mode transition leak"
        print("PASS: guarded transition to", next_mode, "with two persistent UDP tuples (one idle) and original WAN TCP connection", flush=True)

    assert counter("direct") > 0 and counter("vpn") > 0, "mark assertions were not exercised"
    assert counter("bad_meta") == 0, "unrelated packet mark bits were clobbered"
    assert counter("bad_ct") == 0, "unrelated conntrack mark bits were clobbered"
    assert all(p.poll() is None for p in processes), "endpoint died during regression"
    print(f"PASS: kernel namespace regression, {traffic_checks} socket checks, {udp_retries} verified UDP NAT-switch retries (simulated pppoe-wan; no PPPoE negotiation)")
except BaseException:
    print(f"FAIL: kernel regression after {traffic_checks} completed socket checks", flush=True)
    run(r, "nft", "list", "ruleset")
    run(r, "ip", "rule", "show")
    run(r, "ip", "route", "show", "table", "100")
    for ns in (r, c, v):
        run(ns, "ip", "-6", "route", "show")
        run(ns, "ip", "-6", "neigh", "show")
    raise
finally:
    for p in processes:
        if p.poll() is None: p.terminate()
    for p in processes:
        try: p.wait(timeout=3)
        except subprocess.TimeoutExpired: p.kill(); p.wait()
PY

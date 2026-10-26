#!/usr/bin/env bash
set -euo pipefail
# No real WARP registration: exercise production nft renderers against a fake
# CloudflareWARP link, entirely within disposable namespaces and synthetic IPs.
[[ $EUID == 0 ]] || { printf 'Requires root in an isolated test environment\n' >&2; exit 1; }
ROOT="$(CDPATH='' cd "$(dirname "$0")/../../.." && pwd)"
FIXTURES=${FIXTURES:?Emit dataplane::tests::emit_warp_fixtures first}
for name in warp warp-off warp-before; do [[ -s $FIXTURES/$name.nft ]]; done
tag="gfw$$"
c=${tag}c r=${tag}r s=${tag}s w=${tag}w t=${tag}t
owned=()
server_pid=
cleanup() {
  [[ -z $server_pid ]] || kill "$server_pid" 2>/dev/null || true
  for ns in "${owned[@]}"; do
    for pid in $(ip netns pids "$ns"); do kill "$pid" 2>/dev/null || true; done
    ip netns delete "$ns"
  done
}
trap cleanup EXIT
for ns in "$c" "$r" "$s" "$w" "$t"; do
  ip netns add "$ns"; owned+=("$ns")
  ip -n "$ns" link set lo up
  ip netns exec "$ns" sysctl -qw net.ipv4.ip_forward=1 net.ipv4.conf.all.rp_filter=0 net.ipv4.conf.default.rp_filter=0
done
pair() {
  local left=$1 device=$2 address=$3 right=$4 peer=$5 peer_address=$6
  ip -n "$left" link add "$device" type veth peer name "$peer" netns "$right"
  ip -n "$left" address add "$address" dev "$device"
  ip -n "$right" address add "$peer_address" dev "$peer"
  ip -n "$left" link set "$device" up
  ip -n "$right" link set "$peer" up
}
pair "$r" lan0 192.168.50.1/24 "$c" client0 192.168.50.2/24
pair "$r" gt0 10.202.0.2/24 "$s" gt0 10.202.0.1/24
pair "$r" wan0 192.0.2.1/30 "$t" router-wan 192.0.2.2/30
pair "$s" wan0 203.0.113.1/30 "$t" server-wan 203.0.113.2/30
pair "$s" gofro-warp0 169.254.203.1/30 "$w" gofro-warp1 169.254.203.2/30
pair "$w" CloudflareWARP 203.0.113.5/30 "$t" warp-wan 203.0.113.6/30
ip -n "$t" address add 198.51.100.10/32 dev lo
ip -n "$c" route add default via 192.168.50.1
ip -n "$r" route add default via 192.0.2.2
ip -n "$r" rule add pref 80 fwmark 0x10000/0x30000 lookup main
ip -n "$r" rule add pref 81 fwmark 0x20000/0x30000 lookup 100
ip -n "$r" route add unreachable default table 100 metric 32767
ip -n "$r" route add default via 10.202.0.1 dev gt0 table 100 metric 10
ip -n "$s" route add default via 203.0.113.2
# The fake WireGuard link is Ethernet and needs a next-hop; real WireGuard is L3.
ip -n "$s" route add 10.202.1.0/24 via 10.202.0.2 dev gt0
ip -n "$s" rule add pref 10900 from 10.202.1.0/24 iif gt0 lookup 10900
ip -n "$s" route add unreachable default table 10900 metric 32767
ip -n "$s" route add default via 169.254.203.2 table 10900 metric 10
ip -n "$w" route add 10.202.1.0/24 via 169.254.203.1
ip -n "$w" route add default via 203.0.113.6 dev CloudflareWARP
bash "$ROOT/deploy/server/gofro-warp" render-firewall gt0 wan0 | ip netns exec "$s" nft -f -
bash "$ROOT/deploy/server/gofro-warp" render-namespace-firewall | ip netns exec "$w" nft -f -
ip netns exec "$r" nft -f - <<'EOF'
table ip fw4_simulation {
 chain postrouting { type nat hook postrouting priority srcnat; policy accept; oifname { "wan0", "gt0" } masquerade; }
}
EOF
ip netns exec "$r" nft -f "$FIXTURES/warp.nft"

ip netns exec "$t" python3 -u - <<'PY' &
import json, socket, threading
def udp():
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    s.bind(("198.51.100.10", 18080))
    while True:
        data, peer = s.recvfrom(65535)
        if data != b"no-reply": s.sendto(json.dumps(peer[0]).encode(), peer)
def handle(s, peer):
    with s:
        while s.recv(1024): s.sendall(json.dumps(peer[0]).encode())
threading.Thread(target=udp, daemon=True).start()
s = socket.socket()
s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
s.bind(("198.51.100.10", 18080))
s.listen()
while True:
    stream, peer = s.accept()
    threading.Thread(target=handle, args=(stream, peer), daemon=True).start()
PY
server_pid=$!

python3 - "$c" "$r" "$s" "$w" "$FIXTURES" <<'PY'
import atexit, json, os, pathlib, socket, subprocess, sys, time
c, r, s, w, fixtures = sys.argv[1:]
def run(ns, *args): return subprocess.check_output(["ip", "netns", "exec", ns, *args], text=True)
if os.environ.get("GOFRO_WARP_TRACE"):
    logs=[]
    for ns in (c,r,s,w):
        run(ns,"nft","add","table","inet","warp_debug")
        run(ns,"nft","add","chain","inet","warp_debug","pre","{ type filter hook prerouting priority -310; policy accept; }")
        run(ns,"nft","add","rule","inet","warp_debug","pre","meta","nftrace","set","1")
        f=open(pathlib.Path(fixtures)/(ns+".trace"),"w+")
        p=subprocess.Popen(["ip","netns","exec",ns,"nft","monitor","trace"],stdout=f,text=True)
        logs.append((ns,p,f))
    def dump():
        for ns,p,f in logs:
            p.terminate(); p.wait(timeout=3); f.seek(0)
            print(ns,run(ns,"ip","-4","route","show","table","all"),f.read(),file=sys.stderr)
            f.close()
    atexit.register(dump)
    time.sleep(.1)
client = '''import json,socket,sys
s=socket.socket(socket.AF_INET,socket.SOCK_DGRAM if sys.argv[2]=="udp" else socket.SOCK_STREAM)
s.settimeout(.5)
try:
 s.connect((sys.argv[1],18080)); s.send(b"probe"); print(json.loads(s.recv(1024)))
except (OSError,ValueError): print("blocked")
'''
def probe(ns, ip, protocol="udp"): return run(ns, "python3", "-c", client, ip, protocol).strip()
for _ in range(40):
    if probe(c, "198.18.0.4") == "192.0.2.1": break
    time.sleep(.05)
else: raise AssertionError("target did not start or Direct is broken")
def check(warp, vpn="203.0.113.1", direct="192.0.2.1"):
    for protocol in ("udp", "tcp"):
        for ip, expected in (("198.18.0.2",warp),("198.18.0.3",vpn),("198.18.0.4",direct)):
            actual = probe(c,ip,protocol)
            assert actual == expected, (protocol,ip,expected,actual)
check("203.0.113.5")

# Both domains point to one real IP; only the WARP fake lease changes egress.
# Keep an established TCP/UDP socket, and also an unreplied UDP conntrack entry.
session = '''import json,socket,sys
s=socket.socket(socket.AF_INET,socket.SOCK_DGRAM if sys.argv[1]=="udp" else socket.SOCK_STREAM)
s.settimeout(.5); s.connect(("198.18.0.2",18080))
for line in sys.stdin:
 try: s.send(line.strip().encode()); print(json.loads(s.recv(1024)),flush=True)
 except OSError: print("blocked",flush=True)
'''
for protocol, first in (("tcp","probe"),("udp","probe"),("udp","no-reply")):
    run(r,"nft","-f",str(pathlib.Path(fixtures)/"warp-before.nft"))
    p=subprocess.Popen(["ip","netns","exec",c,"python3","-u","-c",session,protocol],stdin=subprocess.PIPE,stdout=subprocess.PIPE,text=True)
    try:
        p.stdin.write(first+"\n"); p.stdin.flush()
        assert p.stdout.readline().strip() == ("blocked" if first=="no-reply" else "203.0.113.1")
        run(r,"nft","-f",str(pathlib.Path(fixtures)/"warp.nft"))
        p.stdin.write("probe\n"); p.stdin.flush()
        assert p.stdout.readline().strip()=="blocked", "old NAT leaked WARP intent through the ordinary VPS exit"
    finally:
        p.stdin.close(); p.wait(timeout=3)
check("203.0.113.5")

# Even a missing host policy rule must not fall through to the VPS default route.
run(s,"ip","rule","del","pref","10900")
check("blocked")
run(s,"ip","rule","add","pref","10900","from","10.202.1.0/24","iif","gt0","lookup","10900")
check("203.0.113.5")

# Simulate disconnected WARP with a fully working native namespace/VPS uplink.
run(w,"ip","link","set","CloudflareWARP","down")
run(w,"ip","route","replace","default","via","169.254.203.1","dev","gofro-warp1")
assert probe(w,"198.51.100.10") == "203.0.113.1", "native registration uplink is broken; failure test would be meaningless"
check("blocked")
run(r,"nft","-f",str(pathlib.Path(fixtures)/"warp-off.nft"))
check("blocked",vpn="192.0.2.1")
print("PASS: selective WARP TCP/UDP, shared IPs, NAT transitions, missing policy rule, disconnected WARP and VPN-off fail-closed")
PY

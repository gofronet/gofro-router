#!/bin/sh
set -eu

# Run only inside a disposable Docker --network none container. Mount the real
# agent as AGENT_BIN and emit_netns_fixtures output as FIXTURES, both read-only.
# Mount the repository read-only too, for the real guard helper.
ROOT="$(CDPATH='' cd "$(dirname "$0")/../../.." && pwd)"
if [ "$(uname -s)" != Linux ] || [ "$(id -u)" != 0 ] || [ ! -f /.dockerenv ]; then
	printf '%s\n' 'BLOCKED: requires a disposable root Linux Docker container' >&2; exit 1;
fi
for command in ip nft python3 mount conntrack jq jsonfilter dnsmasq; do
    command -v "$command" >/dev/null || { printf 'missing required command: %s\n' "$command" >&2; exit 1; }
done
[ -x "${AGENT_BIN:?mount the production agent read-only}" ]
[ -s "${FIXTURES:?mount real rendered fixtures read-only}/routing.nft" ]
[ -f "$ROOT/deploy/openwrt/root/usr/libexec/gofro/guard" ]
# Loaded kernel tunnel modules may create down, unaddressed template devices.
ip -j address show | python3 -c 'import json,sys; links=json.load(sys.stdin); assert [x["ifname"] for x in links if "UP" in x["flags"]] == ["lo"] and all(x["ifname"] == "lo" or not x.get("addr_info") for x in links), "requires --network none; refusing existing networking"'

tag="gdn$$" owned=''
r="${tag}r" c="${tag}c" w="${tag}w" v="${tag}v"
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
ip -n "$r" link add lan0 type veth peer name client0 netns "$c"
ip -n "$r" link add wan0 type veth peer name peer0 netns "$w"
ip -n "$r" link add gt0 type veth peer name vpn0 netns "$v"
for pair in "$r lan0" "$c client0" "$r wan0" "$w peer0" "$r gt0" "$v vpn0"; do
	ns=${pair% *} device=${pair#* }
	ip -n "$ns" link set "$device" addrgenmode none
	ip -n "$ns" link set "$device" up
done
ip -n "$r" addr add 192.168.0.1/24 dev lan0
ip -n "$c" addr add 192.168.0.2/24 dev client0
ip -n "$c" link set client0 address 02:00:00:00:00:01
ip -n "$r" addr add 192.0.2.1/24 dev wan0
ip -n "$w" addr add 192.0.2.2/24 dev peer0
ip -n "$c" route add default via 192.168.0.1
ip -n "$w" route add default via 192.0.2.1
ip -n "$r" addr add 10.0.0.1/24 dev gt0
ip -n "$v" addr add 10.0.0.2/24 dev vpn0
ip -n "$v" route add default via 10.0.0.1
ip -n "$r" route add default via 192.0.2.2
ip -n "$r" rule add pref 80 fwmark 0x10000/0x30000 lookup main
ip -n "$r" rule add pref 81 fwmark 0x20000/0x30000 lookup 100
ip -n "$r" route add default dev gt0 table 100 metric 10 proto 186
ip -n "$r" route add unreachable default table 100 metric 32767 proto 186
ip -n "$r" route add 192.168.0.0/24 dev lan0 table 100 proto 186
# CI's minimal image has no procps/sysctl, and Docker masks /proc/sys read-only.
# ip netns exec creates a slave mount namespace: this fresh proc mount and all
# sysctl writes are confined to this invocation and our owned router netns.
# shellcheck disable=SC2016 # device is expanded by the namespace's child shell.
ip netns exec "$r" sh -eu -c '
    mount -t proc -o nosuid,nodev,noexec proc /proc
    printf "1\n" > /proc/sys/net/ipv4/ip_forward
    for device in all default lan0 wan0 gt0; do
        printf "0\n" > "/proc/sys/net/ipv4/conf/$device/rp_filter"
    done
'
# Deliberately install ULA before GUA, with two addresses in each global-scope prefix.
for address in fe80::1 fd12:3456::1 fd12:3456::11 2001:db8:1::1 2001:db8:1::11; do
	ip -n "$r" -6 addr add "$address/64" dev lan0 nodad
done
for address in fe80::2 fd12:3456::2 2001:db8:1::2; do
	ip -n "$c" -6 addr add "$address/64" dev client0 nodad
done
ip -n "$r" -6 addr add 2001:db8:ff::1/64 dev wan0 nodad
ip -n "$w" -6 addr add 2001:db8:ff::2/64 dev peer0 nodad
ip -n "$c" -6 route add default via 2001:db8:1::1
ip -n "$w" -6 route add default via 2001:db8:ff::1

python3 - "$r" "$c" "$w" "$v" "$AGENT_BIN" "$FIXTURES" "$ROOT/deploy/openwrt/root/usr/libexec/gofro/guard" <<'PY'
import hashlib
import json
import pathlib
import select
import subprocess
import sys
import tempfile
import time

r, c, w, v, agent_bin, fixtures, guard_helper = sys.argv[1:]

def run(ns, *args, **kwargs):
    return subprocess.run(["ip", "netns", "exec", ns, *args], text=True,
                          timeout=15, check=True, **kwargs)

query = r'''
import ipaddress, socket, struct, sys
source, address, port, transport, identity, blocked = sys.argv[1:]
port, identity = int(port), int(identity)
family = socket.AF_INET6 if ":" in address else socket.AF_INET
device = socket.if_nametoindex("client0" if not blocked else "peer0")
def endpoint(ip, port):
    if family == socket.AF_INET: return (ip, port)
    return (ip, port, 0, device if ipaddress.ip_address(ip).is_link_local else 0)
name = b"\x04wifi\x05gofro\x03net\0"
packet = struct.pack("!6H", identity, 0x100, 1, 0, 0, 0) + name + struct.pack("!HH", 1, 1)
def dns_name(data, offset):
    labels, end, seen = [], None, set()
    while True:
        assert offset not in seen and offset < len(data), "invalid DNS name"
        seen.add(offset)
        length = data[offset]
        if length & 0xc0 == 0xc0:
            if end is None: end = offset + 2
            offset = struct.unpack_from("!H", data, offset)[0] & 0x3fff
        elif length == 0:
            return b".".join(labels), end if end is not None else offset + 1
        else:
            assert length <= 63 and offset + 1 + length <= len(data)
            labels.append(data[offset + 1:offset + 1 + length])
            offset += 1 + length
s = socket.socket(family, socket.SOCK_DGRAM if transport == "udp" else socket.SOCK_STREAM)
s.settimeout(2)
s.bind(endpoint(source, 0))
local = s.getsockname()
try:
    s.connect(endpoint(address, port))
    if transport == "udp":
        s.send(packet)
        # Connected UDP rejects mismatched source IP/port instead of accepting
        # an otherwise correct answer from the wildcard listener's chosen source.
        response, peer = s.recvfrom(4096)
    else:
        s.sendall(struct.pack("!H", len(packet)) + packet)
        with s.makefile("rb") as stream:
            prefix = stream.read(2)
            assert len(prefix) == 2, "short DNS TCP length"
            size = struct.unpack("!H", prefix)[0]
            response = stream.read(size)
            assert len(response) == size, "short DNS TCP frame"
        peer = s.getpeername()
    assert not blocked, "WAN reached the actual DNS listener"
    assert ipaddress.ip_address(peer[0].split("%", 1)[0]) == ipaddress.ip_address(address) and peer[1] == port, peer
    ident, flags, questions, answers, authority, additional = struct.unpack_from("!6H", response)
    assert ident == identity and flags & 0x820f == 0x8000, (ident, flags)
    assert (questions, answers, authority, additional) == (1, 1, 0, 0)
    question, offset = dns_name(response, 12)
    assert question == b"wifi.gofro.net" and struct.unpack_from("!HH", response, offset) == (1, 1)
    answer, offset = dns_name(response, offset + 4)
    kind, klass, ttl, size = struct.unpack_from("!HHIH", response, offset)
    assert answer == question and (kind, klass, ttl, size) == (1, 1, 30, 4)
    assert response[offset + 10:] == socket.inet_aton("198.18.0.0"), response.hex()
except (TimeoutError, ConnectionRefusedError) as error:
    if blocked:
        # Native SO_BINDTODEVICE can produce refusal rather than a silent drop.
        print("PASS: wrong-device DNS rejected:", transport, type(error).__name__)
        sys.exit(0)
    print("FAIL tuple:", local, "->", (address, port), transport, "DNS ID", identity, type(error).__name__, flush=True)
    if transport == "udp":
        # Diagnostic only; never convert a failed connected/source check to PASS.
        s.close()
        with socket.socket(family, socket.SOCK_DGRAM) as diagnostic:
            diagnostic.settimeout(2)
            diagnostic.bind(endpoint(source, local[1]))
            diagnostic.sendto(packet, endpoint(address, port))
            try:
                data, peer = diagnostic.recvfrom(4096)
                print("Unconnected diagnostic reply:", peer, "->", diagnostic.getsockname(), "DNS ID", data[:2].hex(), flush=True)
            except OSError as diagnostic_error:
                print("No diagnostic reply:", diagnostic_error, flush=True)
    raise
finally:
    s.close()
print("PASS:", source, "->", address, port, transport, "source/ID/question/A verified")
'''

failures, completed, logs = [], 0, []
with tempfile.TemporaryDirectory(prefix="gofro-real-dns-", dir="/run") as temporary:
    root = pathlib.Path(temporary)
    key = "A" * 43 + "="  # Syntactically valid fixture, never used for a connection.
    (root / "config.json").write_text(json.dumps({
        "vpn_enabled": True, "active_server_key": key,
        "servers": [{"name": "Never contacted", "endpoint": "192.0.2.254:51820", "public_key": key}],
        "routing": {"domain_rules": [], "ip_rules": [], "default_target": "vpn", "mode": "rules"},
    }))
    (root / "empty.dat").write_bytes(b"")
    # Disposable known credential in the production record format; no user auth files.
    password = "netns-only-password"
    salt = bytes(range(32))
    digest = hashlib.scrypt(password.encode(), salt=salt, n=16384, r=8, p=1, dklen=32)
    credential = root / "password"
    credential.touch(mode=0o600)
    credential.write_text(f"gofro-scrypt-v1$16384$8$1${salt.hex()}${digest.hex()}")
    args = [agent_bin, "--lan-interface", "lan0", "--lan-subnet", "192.168.0.0/24",
            "--listen", "127.0.0.1:8080", "--http-listen", "192.168.0.1:8081",
            "--https-listen", "192.168.0.1:8443", "--dns-listen", "192.168.0.1:5353",
            "--dns-upstream", "127.0.0.1:53", "--mode-command", "/bin/false"]
    for flag, name in (("config", "config.json"), ("geosite", "empty.dat"), ("geoip", "empty.dat"),
                       ("tls-cert", "cert.pem"), ("tls-key", "key.pem"), ("admin-password", "password"),
                       ("setup-code", "setup-code"), ("management-dir", "management"), ("routing-state", "routing.sqlite")):
        args += ["--" + flag, str(root / name)]
    run(r, agent_bin, "--version")
    agent = subprocess.Popen(["ip", "netns", "exec", r, *args], stdout=subprocess.PIPE,
                              stderr=subprocess.STDOUT, bufsize=0)
    luci = None
    try:
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline:
            assert agent.poll() is None, "actual agent exited during startup"
            if select.select([agent.stdout], [], [], 0.2)[0]:
                line = agent.stdout.readline().decode()
                logs.append(line)
                print(line, end="", flush=True)
                if "Gofro agent started" in line: break
        else:
            raise AssertionError("actual agent startup timed out")
        assert any("network recovery failed" in line and "/bin/false" in line for line in logs), "reconcile did not fail at the intended no-op command"
        run(r, "nft", "list", "table", "inet", "gofro_guard")
        # Check cold degraded startup BEFORE rendered fixtures can mask missing
        # production VIP setup. Keep running the existing cases even if this fails.
        run(c, "python3", "-c", query, "192.168.0.2", "192.168.0.1", "5353", "udp", "900", "")
        cold = subprocess.run(["ip", "netns", "exec", c, "python3", "-c", r'''
import http.client, json, socket, ssl, sys
context = ssl.create_default_context(cafile=sys.argv[1])
def status(address, port, host):
    connection = http.client.HTTPConnection(address, port, timeout=2)
    connection.connect()
    connection.sock = context.wrap_socket(connection.sock, server_hostname="wifi.gofro.net")
    connection.request("GET", "/api/auth/status", headers={"Host": host})
    response = connection.getresponse()
    assert response.status == 200 and json.loads(response.read())["state"] == "login"
    connection.close()
status("192.168.0.1", 8443, "192.168.0.1:8443")
print("PASS: cold reconcile failure retains actual direct-LAN TLS/auth recovery", flush=True)
status("198.18.0.0", 443, "wifi.gofro.net")
print("PASS: cold reconcile failure retains canonical VIP TLS/auth", flush=True)
''', str(root / "cert.pem")], text=True, capture_output=True, timeout=10)
        print(cold.stdout, end="", flush=True)
        if cold.returncode:
            print("FAIL: cold degraded startup has no working canonical VIP before fixture preload", flush=True)
            print(cold.stderr, end="", flush=True)
        run(r, "nft", "--check", "-f", str(pathlib.Path(fixtures) / "routing.nft"))
        run(r, "nft", "-f", str(pathlib.Path(fixtures) / "routing.nft"))
        # Count the actual post-DNAT IPv6 destination and any accidental upstream query.
        observe = "add table inet dns_probe\nadd chain inet dns_probe input { type filter hook input priority 10; }\n"
        for address in ("fe80::1", "fd12:3456::1", "fd12:3456::11", "2001:db8:1::1", "2001:db8:1::11"):
            observe += f"add rule inet dns_probe input ip6 daddr {address} udp dport 5353 counter\n"
        observe += "add chain inet dns_probe output { type filter hook output priority 10; }\nadd rule inet dns_probe output meta l4proto { tcp, udp } th dport 53 counter\n"
        run(r, "nft", "-f", "-", input=observe)

        cases = [(c, "192.168.0.2", dest, 53, "") for dest in ("192.168.0.1", "198.51.100.53")]
        for source in ("fe80::2", "fd12:3456::2", "2001:db8:1::2"):
            for destination in ("fe80::1", "fd12:3456::1", "fd12:3456::11", "2001:db8:1::1", "2001:db8:1::11", "2001:db8:53::53"):
                cases.append((c, source, destination, 53, ""))
        # Verify WAN can reach the panel socket at the same LAN address before
        # requiring native DNS device isolation, with no test firewall hiding it.
        run(w, "python3", "-c", 'import socket; socket.create_connection(("192.168.0.1", 8081), 2).close()')
        for source, destination in (("192.0.2.2", "192.168.0.1"), ("2001:db8:ff::2", "2001:db8:1::1")):
            cases.append((w, source, destination, 5353, "blocked"))
        for ns, source, destination, port, blocked in cases:
            for transport in ("udp", "tcp"):
                result = subprocess.run(["ip", "netns", "exec", ns, "python3", "-c", query,
                                         source, destination, str(port), transport, str(1000 + completed), blocked],
                                        text=True, capture_output=True, timeout=10)
                completed += 1
                print(result.stdout, end="", flush=True)
                if result.returncode:
                    failures.append((source, destination, port, transport))
                    print(result.stderr, end="", flush=True)
        assert agent.poll() is None, "actual agent stopped during queries"
        run(r, "nft", "list", "table", "inet", "dns_probe")
        table = json.loads(run(r, "nft", "-j", "list", "table", "inet", "dns_probe", capture_output=True).stdout)
        upstream = [expression["counter"]["packets"] for item in table["nftables"]
                    if "rule" in item and item["rule"]["chain"] == "output"
                    for expression in item["rule"]["expr"] if "counter" in expression]
        assert upstream == [0], "local panel DNS unexpectedly queried an upstream"
        print(f"Actual production DNS: {completed - len(failures)} passed, {len(failures)} failed, {completed} total", flush=True)
        assert completed == 44, f"existing DNS coverage changed: {completed}/44"
        assert not failures, f"actual production DNS failed {len(failures)}/{completed} cases: {failures}"
        print(f"PASS: actual production Rust DNS, {completed} cases; connected UDP and TCP source/ID/A checks with LL+ULA+GUA aliases")

        # Bind real LAN default ports after agent startup: any frontend occupation
        # fails bind. The identities below must survive every rendered mode change.
        luci_code = r'''
import http.server, socketserver, ssl, sys, threading
class Server(http.server.ThreadingHTTPServer):
    def server_bind(self):
        # HTTPServer's getfqdn() would consult Docker DNS outside this netns.
        socketserver.TCPServer.server_bind(self)
        self.server_name, self.server_port = "LuCI", self.server_address[1]
class LuCI(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        body = f"simulated-LuCI:{self.server.server_port}".encode()
        self.send_response(200); self.send_header("Content-Length", str(len(body)))
        self.end_headers(); self.wfile.write(body)
    def log_message(self, *args): pass
for port in (80, 443):
    server = Server(("192.168.0.1", port), LuCI)
    if port == 443:
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        context.load_cert_chain(sys.argv[1], sys.argv[2])
        server.socket = context.wrap_socket(server.socket, server_side=True)
    threading.Thread(target=server.serve_forever, daemon=True).start()
print("ready", flush=True)
threading.Event().wait()
'''
        luci = subprocess.Popen(["ip", "netns", "exec", r, "python3", "-u", "-c", luci_code,
                                 str(root / "cert.pem"), str(root / "key.pem")], stdout=subprocess.PIPE, text=True)
        assert select.select([luci.stdout], [], [], 5)[0] and luci.stdout.readline().strip() == "ready", "LAN 80/443 occupied by frontend or LuCI failed"

        panel = r'''
import http.client, json, socket, ssl, sys
cert, password = sys.argv[1:]
context = ssl.create_default_context(cafile=cert)
def request(address, port, host, secure, path, method="GET", headers=None, body=None):
    connection = http.client.HTTPConnection(address, port, timeout=3)
    connection.connect()
    if secure:
        # Verify the actual generated certificate and send canonical TLS SNI,
        # independently of the destination IP and explicit HTTP Host header.
        connection.sock = context.wrap_socket(connection.sock, server_hostname="wifi.gofro.net")
        assert connection.sock.version() in ("TLSv1.2", "TLSv1.3")
    connection.request(method, path, body=body, headers={"Host": host, **(headers or {})})
    response = connection.getresponse()
    data = response.read()
    result = response.status, response.getheaders(), data
    connection.close()
    return result
path = "/api/auth/status?next=%2Fhome&x=1"
checks = 0
for address, port, host, destination in (
    ("198.18.0.0", 80, "wifi.gofro.net", "https://wifi.gofro.net"),
    ("198.18.0.0", 8081, "wifi.gofro.net:8081", "https://wifi.gofro.net"),
    ("192.168.0.1", 8081, "192.168.0.1:8081", "https://192.168.0.1:8443"),
):
    status, headers, body = request(address, port, host, False, path)
    assert status == 307 and dict(headers).get("location") == destination + path, (status, headers, body)
    checks += 1
for address, port, host in (("198.18.0.0", 443, "wifi.gofro.net"),
                            ("198.18.0.0", 8443, "wifi.gofro.net:8443"),
                            ("192.168.0.1", 8443, "192.168.0.1:8443")):
    status, headers, body = request(address, port, host, True, path)
    reply = json.loads(body)
    assert status == 200 and reply["state"] == "login" and len(reply["csrf_token"]) == 64, (status, body)
    cookies = "; ".join(value.split(";", 1)[0] for name, value in headers if name.lower() == "set-cookie")
    status, headers, body = request(address, port, host, True, "/api/auth/login", "POST",
        {"Origin": "https://" + host, "Cookie": cookies, "X-CSRF-Token": reply["csrf_token"],
         "Content-Type": "application/json"}, json.dumps({"password": password}))
    assert status == 200 and json.loads(body)["state"] == "authenticated", (status, body)
    cookies = "; ".join(value.split(";", 1)[0] for name, value in headers if name.lower() == "set-cookie")
    assert "__Host-gofro-session=" in cookies
    status, _, body = request(address, port, host, True, path, headers={"Cookie": cookies})
    assert status == 200 and json.loads(body)["state"] == "authenticated", (status, body)
    checks += 3
for port in (80, 443):
    status, _, body = request("192.168.0.1", port, "192.168.0.1", port == 443, "/")
    assert status == 200 and body == f"simulated-LuCI:{port}".encode(), (status, body)
    checks += 1
assert checks == 14
print("PASS: 14 actual HTTP/TLS/auth and LuCI identity checks", flush=True)
'''
        # Observe both sides of the production drop hook: a timeout alone could
        # be an absent route/listener or the forward guard hiding a VIP WAN leak.
        run(r, "nft", "-f", "-", input='''
add table inet panel_probe
add counter inet panel_probe arrived
add counter inet panel_probe survived
add counter inet panel_probe leak
add counter inet panel_probe ingress_dnat
add counter inet panel_probe forward_before
add counter inet panel_probe forward_after
add chain inet panel_probe before { type filter hook prerouting priority -151; }
add rule inet panel_probe before ip daddr 198.18.0.0 counter name arrived
add chain inet panel_probe after { type filter hook prerouting priority -149; }
add rule inet panel_probe after ip daddr 198.18.0.0 counter name survived
add chain inet panel_probe input { type filter hook input priority 10; }
add rule inet panel_probe input iifname != "lan0" ct status dnat ip daddr 192.168.0.1 tcp dport { 8081, 8443 } counter name ingress_dnat
add chain inet panel_probe egress { type filter hook postrouting priority 110; }
add rule inet panel_probe egress oifname { "wan0", "gt0" } ct original ip daddr 198.18.0.0 counter name leak
add chain inet panel_probe forward_before { type filter hook forward priority -1; }
add rule inet panel_probe forward_before iifname "lan0" ip daddr 198.51.100.2 udp dport 2080 counter name forward_before
add chain inet panel_probe forward_after { type filter hook forward priority 1; }
add rule inet panel_probe forward_after iifname "lan0" ip daddr 198.51.100.2 udp dport 2080 counter name forward_after
''')
        def counts():
            table = json.loads(run(r, "nft", "-j", "list", "table", "inet", "panel_probe", capture_output=True).stdout)
            return {item["counter"]["name"]: item["counter"]["packets"] for item in table["nftables"] if "counter" in item}

        blocked_probe = r'''
import socket, sys
port, transport = sys.argv[1:]
with socket.socket(type=socket.SOCK_STREAM if transport == "tcp" else socket.SOCK_DGRAM) as s:
    s.settimeout(0.3)
    try:
        s.connect(("198.18.0.0", int(port)))
        s.sendall(b"VIP must drop\n")
        s.recv(4096)
    except TimeoutError: pass
    else: raise AssertionError("unsupported/wrong-ingress VIP did not silently drop")
'''
        panel_checks = alias_checks = drop_checks = 0
        for mode, guarded, tunnel_down in (("routing", False, False), ("off", False, False),
                                           ("all", False, False), ("all", False, True),
                                           ("off", True, False), ("routing", True, False),
                                           ("all", True, False)):
            run(r, "nft", "-f", str(pathlib.Path(fixtures) / (mode + ".nft")))
            if guarded:
                run(r, "nft", "-f", str(pathlib.Path(fixtures) / "guard.nft"))
                run(r, "nft", "list", "table", "inet", "gofro_guard", capture_output=True)
            else:
                run(r, "nft", "-f", "-", input="destroy table inet gofro_guard\n")
            run(r, "ip", "link", "set", "gt0", "down" if tunnel_down else "up")
            if tunnel_down:
                run(r, "ip", "route", "flush", "table", "100", "dev", "gt0", "exact", "0.0.0.0/0")
                route = subprocess.run(["ip", "-n", r, "route", "get", "8.8.8.8", "mark", "0x20000"], capture_output=True)
                assert route.returncode != 0, "missing tunnel did not fail closed"
            else:
                run(r, "ip", "route", "replace", "default", "dev", "gt0", "table", "100", "metric", "10", "proto", "186")
            for transport in ("tcp", "udp"):
                run(c, "python3", "-c", query, "192.168.0.2", "192.168.0.1", "53", transport, str(2000 + alias_checks), "")
                alias_checks += 1
            run(c, "python3", "-c", panel, str(root / "cert.pem"), password)
            panel_checks += 14
            if not tunnel_down:
                before_forward = counts()
                run(c, "python3", "-c", 'import socket; s=socket.socket(type=socket.SOCK_DGRAM); s.sendto(b"guard probe", ("198.51.100.2", 2080)); s.close()')
                after_forward = counts()
                assert after_forward["forward_before"] > before_forward["forward_before"], "forward guard probe never arrived"
                assert (after_forward["forward_after"] == before_forward["forward_after"]) == guarded, "forward guard inactive or unguarded positive control failed"
            forbidden = [(c, port, proto) for port, proto in ((443, "udp"), (53, "tcp"), (53, "udp"),
                         (22, "tcp"), (22, "udp"), (80, "udp"), (8443, "udp"))]
            forbidden += [(ns, port, "tcp") for ns in ((w,) if tunnel_down else (w, v)) for port in (80, 443, 8081, 8443)]
            for ns, port, transport in forbidden:
                before_drop = counts()
                run(ns, "python3", "-c", blocked_probe, str(port), transport)
                after_drop = counts()
                assert after_drop["arrived"] > before_drop["arrived"], (ns, port, transport, "probe never arrived")
                assert after_drop["survived"] == before_drop["survived"], (ns, port, transport, "production VIP drop bypassed")
                assert after_drop["leak"] == after_drop["ingress_dnat"] == 0, after_drop
                drop_checks += 1
            assert luci.poll() is None and agent.poll() is None, "management process died"
            print(f"PASS: VIP alias/management/drop isolation: {mode}, guard={guarded}, tunnel_down={tunnel_down}", flush=True)
        assert (panel_checks, alias_checks, drop_checks) == (98, 14, 101)
        print(f"PASS: {panel_checks} real panel/LuCI checks, {alias_checks} mode DNS aliases, {drop_checks} observed VIP drops", flush=True)

        # Native OpenWrt host-record contract, including a real AAAA positive
        # control which FakeDNS filters under the ordinary VPN policy.
        native = subprocess.Popen(["ip", "netns", "exec", r, "dnsmasq", "--keep-in-foreground",
            "--no-resolv", "--no-hosts", "--bind-interfaces", "--interface=lan0", "--interface=lo",
            "--port=53", "--user=root", f"--pid-file={root / 'dnsmasq.pid'}", "--local-ttl=30",
            "--host-record=wifi.gofro.net,198.18.0.0",
            "--host-record=full-direct.test,2001:db8:99::1"], stderr=subprocess.PIPE)
        aaaa = r'''
import socket,struct,sys
address,transport,expected=sys.argv[1:]
question=b"\x0bfull-direct\x04test\0"+struct.pack("!HH",28,1)
packet=struct.pack("!6H",789,256,1,0,0,0)+question
with socket.socket(socket.AF_INET6 if ":" in address else socket.AF_INET,
                   socket.SOCK_DGRAM if transport=="udp" else socket.SOCK_STREAM) as s:
    s.settimeout(3); s.connect((address,53))
    if transport=="udp":
        s.send(packet); data=s.recv(4096)
    else:
        s.sendall(struct.pack("!H",len(packet))+packet)
        with s.makefile("rb") as stream:
            prefix=stream.read(2); assert len(prefix)==2
            size=struct.unpack("!H",prefix)[0]; data=stream.read(size); assert len(data)==size
    ident,flags,qd,an,_,_=struct.unpack_from("!6H",data)
    assert ident==789 and flags & 0x820f==0x8000 and qd==1 and an==int(expected),(address,transport,data.hex())
    if an: assert data[-16:]==socket.inet_pton(socket.AF_INET6,"2001:db8:99::1"),data.hex()
'''
        native_checks = 0
        try:
            time.sleep(.15)
            assert native.poll() is None, native.stderr.read().decode()
            run(r, "nft", "-f", str(pathlib.Path(fixtures) / "excluded.nft"))
            run(r, "nft", "-f", str(pathlib.Path(fixtures) / "guard-excluded.nft"))
            for excluded in (False, True, False):
                run(c, "ip", "link", "set", "client0", "address", "02:00:00:00:00:02" if excluded else "02:00:00:00:00:01")
                run(r, "sh", str(pathlib.Path(guard_helper).with_name("dns-flows")), "cleanup", "lan0", "5353")
                for source, address in (("192.168.0.2", "192.168.0.1"), ("2001:db8:1::2", "2001:db8:1::1")):
                    for transport in ("udp", "tcp"):
                        run(c, "python3", "-c", query, source, address, "53", transport, str(3000 + native_checks), "")
                        run(c, "python3", "-c", aaaa, address, transport, "1" if excluded else "0")
                        native_checks += 2
            print(f"PASS: {native_checks} native dnsmasq/FakeDNS checks: excluded panel VIP and unfiltered AAAA, ordinary MAC filters AAAA", flush=True)
        finally:
            native.terminate()
            try: native.wait(timeout=5)
            except subprocess.TimeoutExpired: native.kill(); native.wait()
        # Restore the old helper ownership controls' original empty guard.
        run(r, "nft", "-f", str(pathlib.Path(fixtures) / "all.nft"))
        run(r, "nft", "-f", str(pathlib.Path(fixtures) / "guard.nft"))
    finally:
        if luci is not None:
            luci.terminate()
            try: luci.wait(timeout=5)
            except subprocess.TimeoutExpired: luci.kill(); luci.wait()
        agent.terminate()
        try: agent.wait(timeout=5)
        except subprocess.TimeoutExpired: agent.kill(); agent.wait()

    # Exercise the real helper's strict ownership check against nft's formatter,
    # after the writer has exited, without discovery or replacement test chains.
    guard_state = root / "guard-state"
    guard_state.mkdir(mode=0o700)
    (guard_state / "guard-device").write_text("lan0\n")
    (guard_state / "guard-device").chmod(0o600)
    def routing_objects():
        result = json.loads(run(r, "nft", "-j", "-s", "list", "table", "inet", "gofro_routing", capture_output=True).stdout)
        # Full Direct additionally synchronizes this owned set, including adoption of
        # older tables without it. Every other object must remain byte-identical.
        result["nftables"] = [x for x in result["nftables"] if x.get("set", {}).get("name") != "device_exclusions"]
        return result
    before = routing_objects()
    assert any(item.get("chain", {}).get("name") == "gofro_dns" for item in before["nftables"])
    without_dns = {"nftables": [item for item in before["nftables"]
                               if item.get("chain", {}).get("name") != "gofro_dns"
                               and item.get("rule", {}).get("chain") != "gofro_dns"]}
    forward_guard = run(r, "nft", "-s", "-y", "list", "chain", "inet", "gofro_guard", "gofro_guard", capture_output=True).stdout
    run(r, "nft", "-s", "-y", "list", "chain", "inet", "gofro_routing", "gofro_dns")
    for action in ("boot", "boot", "stop", "stop"):
        run(r, "env", f"GOFRO_GUARD_DIR={guard_state}", f"GOFRO_MODE_LOCK={root / 'mode.lock'}",
            f"GOFRO_HELPERS={pathlib.Path(guard_helper).parent}",
            "sh", guard_helper, action)
        after = routing_objects()
        assert after == (before if action == "boot" else without_dns), f"guard {action} changed unexpected dataplane objects or retained DNS"
        assert run(r, "nft", "-s", "-y", "list", "chain", "inet", "gofro_guard", "gofro_guard", capture_output=True).stdout == forward_guard, f"guard {action} altered forwarding protection"
        for table_name in ("gofro_guard", "gofro_routing"):
            listing = run(r, "nft", "-s", "list", "set", "inet", table_name, "device_exclusions", capture_output=True).stdout
            assert "type ether_addr" in listing and "elements" not in listing
        print(f"PASS: actual guard {action}; expected DNS state, other dataplane objects and forwarding guard preserved", flush=True)
    print(f"PASS: {completed} actual DNS cases and 4 real guard boot/stop checks")

    # Real startup and shell lifecycle synchronize the committed JSON before
    # fallible TLS setup. No rendered guard may mask a stale exemption here.
    config = json.loads((root / "config.json").read_text())
    config["device_exclusions"] = ["02:00:00:00:00:02"]
    committed = guard_state / "controller.json"
    committed.write_text(json.dumps(config)); committed.chmod(0o600)
    (guard_state / "controller.json.new").write_text('{"device_exclusions": []}')
    failed_args = list(args)
    failed_args[failed_args.index("--config") + 1] = str(committed)
    failed_args[failed_args.index("--tls-cert") + 1] = str(root)  # directory, deterministic TLS failure
    failed = subprocess.run(["ip", "netns", "exec", r, *failed_args], text=True, capture_output=True, timeout=15)
    print("FAILED TLS STARTUP STDOUT:", failed.stdout, "STDERR:", failed.stderr, flush=True)
    assert failed.returncode != 0 and "Is a directory (os error 21)" in failed.stderr, "startup did not fail reading the TLS certificate directory"
    for action in (None, "boot", "stop"):
        if action:
            run(r, "nft", "-f", str(pathlib.Path(fixtures) / "publish-empty.nft"))
            run(r, "env", f"GOFRO_GUARD_DIR={guard_state}", f"GOFRO_MODE_LOCK={root / 'mode.lock'}",
                f"GOFRO_HELPERS={pathlib.Path(guard_helper).parent}", "sh", guard_helper, action)
        for table_name in ("gofro_guard", "gofro_routing"):
            listing = json.loads(run(r, "nft", "-j", "list", "set", "inet", table_name, "device_exclusions", capture_output=True).stdout)
            actual = next(item["set"].get("elem", []) for item in listing["nftables"] if "set" in item)
            assert actual == ["02:00:00:00:00:02"], (action, table_name, actual)
        for excluded in (True, False):
            run(c, "ip", "link", "set", "client0", "address", "02:00:00:00:00:02" if excluded else "02:00:00:00:00:01")
            before_forward = counts()
            run(c, "python3", "-c", 'import socket; s=socket.socket(type=socket.SOCK_DGRAM); s.sendto(b"committed guard", ("198.51.100.2",2080)); s.close()')
            after_forward = counts()
            assert after_forward["forward_before"] == before_forward["forward_before"] + 1, (action, before_forward, after_forward)
            assert after_forward["forward_after"] == before_forward["forward_after"] + int(excluded), (action, excluded, before_forward, after_forward)
        print("PASS: actual nft packet enforcement after", action or "failed TLS startup", "committed exclusion wins over candidate JSON", flush=True)
    print("PACKET COUNTS:", json.dumps(counts()), flush=True)
    assert cold.returncode == 0, "PRODUCTION BUG: cold reconcile failure answers VIP DNS but canonical TLS/auth is unreachable; rendered fixture preload masks it"
PY

#!/bin/sh
set -eu

# Run only inside a disposable Docker --network none container. Mount the real
# agent as AGENT_BIN and emit_netns_fixtures output as FIXTURES, both read-only.
# Mount the repository read-only too, for the real guard helper.
ROOT="$(CDPATH='' cd "$(dirname "$0")/../../.." && pwd)"
if [ "$(uname -s)" != Linux ] || [ "$(id -u)" != 0 ] || [ ! -f /.dockerenv ]; then
	printf '%s\n' 'BLOCKED: requires a disposable root Linux Docker container' >&2; exit 1;
fi
for command in ip nft python3; do command -v "$command" >/dev/null; done
[ -x "${AGENT_BIN:?mount the production agent read-only}" ]
[ -s "${FIXTURES:?mount real rendered fixtures read-only}/routing.nft" ]
[ -f "$ROOT/deploy/openwrt/root/usr/libexec/gofro/guard" ]
# Loaded kernel tunnel modules may create down, unaddressed template devices.
ip -j address show | python3 -c 'import json,sys; links=json.load(sys.stdin); assert [x["ifname"] for x in links if "UP" in x["flags"]] == ["lo"] and all(x["ifname"] == "lo" or not x.get("addr_info") for x in links), "requires --network none; refusing existing networking"'

tag="gdn$$" owned=''
r="${tag}r" c="${tag}c" w="${tag}w"
cleanup() {
	for ns in $owned; do
		for pid in $(ip netns pids "$ns"); do kill "$pid" 2>/dev/null || true; done
		ip netns del "$ns"
	done
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
for ns in "$r" "$c" "$w"; do
	ip netns add "$ns"
	owned="$owned $ns"
	ip -n "$ns" link set lo up
done
ip -n "$r" link add lan0 type veth peer name client0 netns "$c"
ip -n "$r" link add wan0 type veth peer name peer0 netns "$w"
for pair in "$r lan0" "$c client0" "$r wan0" "$w peer0"; do
	ns=${pair% *} device=${pair#* }
	ip -n "$ns" link set "$device" addrgenmode none
	ip -n "$ns" link set "$device" up
done
ip -n "$r" addr add 192.168.0.1/24 dev lan0
ip -n "$c" addr add 192.168.0.2/24 dev client0
ip -n "$r" addr add 192.0.2.1/24 dev wan0
ip -n "$w" addr add 192.0.2.2/24 dev peer0
ip -n "$c" route add default via 192.168.0.1
ip -n "$w" route add default via 192.0.2.1
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

python3 - "$r" "$c" "$w" "$AGENT_BIN" "$FIXTURES" "$ROOT/deploy/openwrt/root/usr/libexec/gofro/guard" <<'PY'
import json
import pathlib
import select
import subprocess
import sys
import tempfile
import time

r, c, w, agent_bin, fixtures, guard_helper = sys.argv[1:]

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
    assert response[offset + 10:] == socket.inet_aton("192.168.0.1"), response.hex()
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
        assert not failures, f"actual production DNS failed {len(failures)}/{completed} cases: {failures}"
        print(f"PASS: actual production Rust DNS, {completed} cases; connected UDP and TCP source/ID/A checks with LL+ULA+GUA aliases")
    finally:
        agent.terminate()
        try: agent.wait(timeout=5)
        except subprocess.TimeoutExpired: agent.kill(); agent.wait()

    # Exercise the real helper's strict ownership check against nft's formatter,
    # after the writer has exited, without discovery or replacement test chains.
    guard_state = root / "guard-state"
    guard_state.mkdir(mode=0o700)
    (guard_state / "guard-device").write_text("lan0\n")
    (guard_state / "guard-device").chmod(0o600)
    before = json.loads(run(r, "nft", "-j", "-s", "list", "table", "inet", "gofro_routing", capture_output=True).stdout)
    assert any(item.get("chain", {}).get("name") == "gofro_dns" for item in before["nftables"])
    without_dns = {"nftables": [item for item in before["nftables"]
                               if item.get("chain", {}).get("name") != "gofro_dns"
                               and item.get("rule", {}).get("chain") != "gofro_dns"]}
    forward_guard = run(r, "nft", "-s", "-y", "list", "table", "inet", "gofro_guard", capture_output=True).stdout
    run(r, "nft", "-s", "-y", "list", "chain", "inet", "gofro_routing", "gofro_dns")
    for action in ("boot", "boot", "stop", "stop"):
        run(r, "env", f"GOFRO_GUARD_DIR={guard_state}", f"GOFRO_MODE_LOCK={root / 'mode.lock'}",
            "sh", guard_helper, action)
        after = json.loads(run(r, "nft", "-j", "-s", "list", "table", "inet", "gofro_routing", capture_output=True).stdout)
        assert after == (before if action == "boot" else without_dns), f"guard {action} changed unexpected dataplane objects or retained DNS"
        assert run(r, "nft", "-s", "-y", "list", "table", "inet", "gofro_guard", capture_output=True).stdout == forward_guard, f"guard {action} altered forwarding protection"
        print(f"PASS: actual guard {action}; expected DNS state, other dataplane objects and forwarding guard preserved", flush=True)
    print(f"PASS: {completed} actual DNS cases and 4 real guard boot/stop checks")
PY

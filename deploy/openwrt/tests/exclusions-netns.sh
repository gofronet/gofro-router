#!/bin/sh
set -eu
# Disposable Docker only: repository is read-only, all networking is in an owned netns.
[ "$(uname -s)" = Linux ] && [ "$(id -u)" = 0 ] && [ -f /.dockerenv ] || exit 1
ROOT="$(CDPATH='' cd "$(dirname "$0")/../../.." && pwd)"
ip -j address show | python3 -c 'import json,sys; assert all(x["ifname"]=="lo" or not x.get("addr_info") for x in json.load(sys.stdin)), "requires --network none"'
ns="gex$$"; client="${ns}c"; wan="${ns}w"
trap 'ip netns del "$client"; ip netns del "$wan"; ip netns del "$ns"' EXIT
for name in "$ns" "$client" "$wan"; do ip netns add "$name"; ip -n "$name" link set lo up; done
ip -n "$ns" link add lan0 type veth peer name client0 netns "$client"
ip -n "$ns" link add wan0 type veth peer name peer0 netns "$wan"
for pair in "$ns lan0" "$ns wan0" "$client client0" "$wan peer0"; do
	ip -n "${pair% *}" link set "${pair#* }" addrgenmode none
	ip -n "${pair% *}" link set "${pair#* }" up
done
ip -n "$client" link set client0 address 02:00:00:00:00:01
ip -n "$ns" address add 192.168.7.1/24 dev lan0
ip -n "$client" address add 192.168.7.2/24 dev client0
ip -n "$ns" address add 192.0.2.1/24 dev wan0
ip -n "$wan" address add 192.0.2.2/24 dev peer0
ip -n "$client" route add default via 192.168.7.1
ip -n "$wan" route add default via 192.0.2.1
for address in fe80::1 fd12::1 2001:db8::1; do ip -n "$ns" -6 address add "$address/64" dev lan0 nodad; done
ip -n "$client" -6 address add fd12::2/64 dev client0 nodad
ip -n "$ns" -6 address add 2001:db8:2::1/64 dev wan0 nodad
ip -n "$wan" -6 address add 2001:db8:2::2/64 dev peer0 nodad
ip -n "$client" -6 route add default via fd12::1
ip -n "$wan" -6 route add default via 2001:db8:2::1
# Deterministic test-only WAN link; device enforcement never reads neighbors.
ip -n "$ns" link set wan0 address 02:00:00:00:00:10
ip -n "$wan" link set peer0 address 02:00:00:00:00:11
ip -n "$ns" -6 neigh add 2001:db8:2::2 lladdr 02:00:00:00:00:11 nud permanent dev wan0
ip -n "$wan" -6 neigh add 2001:db8:2::1 lladdr 02:00:00:00:00:10 nud permanent dev peer0
ip netns exec "$ns" sh -eu -c 'mount -t proc -o nosuid,nodev,noexec proc /proc; printf "1\n" > /proc/sys/net/ipv4/ip_forward; printf "1\n" > /proc/sys/net/ipv6/conf/all/forwarding'
ip netns exec "$ns" python3 - "$ROOT" "$client" "$wan" <<'PY'
import fcntl
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import time

helpers = pathlib.Path(sys.argv[1]) / "deploy/openwrt/root/usr/libexec/gofro"
client, wan = sys.argv[2:]

def run(*args, ok=True, **kwargs):
    result = subprocess.run(args, text=True, capture_output=True, timeout=15, **kwargs)
    if ok and result.returncode:
        for name in (None, client, wan):
            prefix = ["ip"] + (["-n", name] if name else [])
            subprocess.run(prefix + ["-6", "route", "show"])
            subprocess.run(prefix + ["-6", "neigh", "show"])
            subprocess.run(prefix + ["-6", "address", "show"])
    assert (result.returncode == 0) == ok, (args, result.stdout, result.stderr)
    return result

def nft(script):
    run("nft", "-f", "-", input=script)

def table(name):
    return run("nft", "-s", "-y", "list", "table", "inet", name).stdout

with tempfile.TemporaryDirectory(dir="/root") as tmp:
    root = pathlib.Path(tmp)
    state = root / "state"
    state.mkdir(mode=0o700)
    config = state / "controller.json"
    identity = state / "guard-device"
    identity.write_text("lan0\n")
    identity.chmod(0o600)
    env = dict(os.environ, GOFRO_GUARD_DIR=str(state), GOFRO_HELPERS=str(helpers), GOFRO_MODE_LOCK=str(root / "mode.lock"))

    def commit(value):
        config.write_text(json.dumps(value))
        config.chmod(0o600)

    def guard(action="boot", ok=True, **extra):
        return run("sh", str(helpers / "guard"), action, env=env | extra, ok=ok)

    def sets(expected):
        for name in ("gofro_guard", "gofro_routing"):
            listing = json.loads(run("nft", "-j", "list", "set", "inet", name, "device_exclusions").stdout)
            s = next(x["set"] for x in listing["nftables"] if "set" in x)
            assert s["type"] == "ether_addr" and set(s.get("elem", [])) == set(expected), s
        assert "198.18.1.1 : 203.0.113.7" in table("gofro_routing")

    nft('''add table inet gofro_routing
add set inet gofro_routing device_exclusions { type ether_addr; }
add element inet gofro_routing device_exclusions { 02:00:00:00:00:99 }
add map inet gofro_routing fake_map { type ipv4_addr : ipv4_addr; }
add element inet gofro_routing fake_map { 198.18.1.1 : 203.0.113.7 }
''')
    guard()
    sets([])
    old_chain = run("nft", "-s", "-y", "list", "chain", "inet", "gofro_guard", "gofro_guard").stdout
    assert 'iifname "lan0" oifname != "lan0" drop' in old_chain and "ether" not in old_chain
    macs = ["02:00:00:00:00:01", "aa:bb:cc:dd:ee:02"]
    commit({"device_exclusions": macs})
    guard()
    sets(macs)
    assert 'ether saddr != @device_exclusions drop' in table("gofro_guard")
    server = subprocess.Popen(["ip", "netns", "exec", wan, "python3", "-u", "-c", '''
import selectors,socket
sel=selectors.DefaultSelector()
for family,address in ((socket.AF_INET,"192.0.2.2"),(socket.AF_INET6,"2001:db8:2::2")):
 for port in (53,23456):
  s=socket.socket(family,socket.SOCK_DGRAM); s.bind((address,port)); sel.register(s,selectors.EVENT_READ)
print("ready",flush=True)
while True:
 for key,_ in sel.select():
  data,peer=key.fileobj.recvfrom(1024); key.fileobj.sendto(data,peer)
'''], stdout=subprocess.PIPE, text=True)
    try:
        assert server.stdout.readline().strip() == "ready"
        probe = '''
import socket,sys
address,port,allowed=sys.argv[1:]
s=socket.socket(socket.AF_INET6 if ":" in address else socket.AF_INET,socket.SOCK_DGRAM)
s.bind(("::" if ":" in address else "0.0.0.0",50001))
s.settimeout(2 if allowed=="yes" else .3); s.sendto(b"native",(address,int(port)))
try: received=s.recv(1024)==b"native"
except TimeoutError: received=False
assert received==(allowed=="yes"),(address,port,allowed,received)
'''
        for excluded in (True, False, True):
            commit({"device_exclusions": macs if excluded else []})
            guard()
            for address in ("192.0.2.2", "2001:db8:2::2"):
                for port in (53, 23456):
                    run("ip", "netns", "exec", client, "python3", "-c", probe, address, str(port), "yes" if excluded else "no")
        # Same addresses, different MAC: enforcement does not consult IP/ARP inventory.
        run("ip", "-n", client, "link", "set", "client0", "address", "02:00:00:00:00:09")
        run("ip", "netns", "exec", client, "python3", "-c", probe, "192.0.2.2", "53", "no")
        run("ip", "-n", client, "link", "set", "client0", "address", "02:00:00:00:00:01")
        print("PASS actual dual-stack forwarding/native port53: excluded allowed, removal blocked, same-IP different-MAC blocked", flush=True)
    finally:
        server.terminate()
        server.wait(timeout=5)
    # Native resolver path used by fully excluded devices, with the exact
    # OpenWrt hostrecord rendering (no global server/address options).
    dns = subprocess.Popen(["dnsmasq", "--keep-in-foreground", "--no-resolv", "--no-hosts", "--bind-interfaces",
                            "--interface=lan0", "--port=53", "--user=root", f"--pid-file={root / 'dns.pid'}",
                            "--host-record=wifi.gofro.net,198.18.0.0"], stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
    try:
        time.sleep(.15)
        assert dns.poll() is None
        run("ip", "netns", "exec", client, "python3", "-c", '''
import socket,struct
question=b"".join(bytes([len(x)])+x for x in (b"wifi",b"gofro",b"net"))+b"\\0"+struct.pack("!HH",1,1)
packet=struct.pack("!HHHHHH",4321,256,1,0,0,0)+question
for family,address in ((socket.AF_INET,"192.168.7.1"),(socket.AF_INET6,"fd12::1")):
 for transport in (socket.SOCK_DGRAM,socket.SOCK_STREAM):
  with socket.socket(family,transport) as s:
   s.settimeout(2); s.connect((address,53))
   if transport==socket.SOCK_STREAM:
    s.sendall(struct.pack("!H",len(packet))+packet); size=struct.unpack("!H",s.recv(2))[0]; data=s.recv(size)
   else:
    s.send(packet); data=s.recv(4096)
   assert data[:2]==packet[:2] and data[-4:]==socket.inet_aton("198.18.0.0"),data
''')
        print("PASS excluded native dnsmasq panel record over IPv4/IPv6 UDP/TCP", flush=True)
    finally:
        dns.terminate()
        dns.wait(timeout=5)
    # A crash candidate never overrides the one committed configuration.
    (state / "controller.json.new").write_text('{"device_exclusions": ["02:00:00:00:00:99"]}')
    guard()
    sets(macs)
    failed_helpers = root / "failed-helpers"
    failed_helpers.mkdir()
    (failed_helpers / "network").write_text("#!/bin/sh\nexit 1\n")
    (failed_helpers / "network").chmod(0o755)
    (failed_helpers / "dns-flows").symlink_to(helpers / "dns-flows")
    guard("prepare", ok=False, GOFRO_HELPERS=str(failed_helpers))
    sets(macs)  # Enforcement sync preceded the deliberately fallible discovery.
    for value in ({}, {"routing": {"device_exclusions": macs}}, {"device_exclusions": []}):
        commit(value)
        guard()
        sets([])
        assert run("nft", "-s", "-y", "list", "chain", "inet", "gofro_guard", "gofro_guard").stdout == old_chain
    maximum = [f"02:00:00:00:{i // 256:02x}:{i % 256:02x}" for i in range(256)]
    commit({"device_exclusions": maximum})
    guard()
    sets(maximum)
    bad = [None, False, 2, "02:00:00:00:00:01", {}, [None], [2], [{}], [[]],
           ["00:00:00:00:00:00"], ["01:00:00:00:00:01"], ["ff:ff:ff:ff:ff:ff"],
           ["AA:BB:CC:DD:EE:02"], ["2:00:00:00:00:01"], ["02-00-00-00-00-01"],
           ["02:00:00:00:00:01\n"], ["02:00:00:00:00:01\u0000"],
           ["02:00:00:00:00:01; flush ruleset"], maximum + ["02:00:00:01:00:00"]]
    for value in bad:
        commit({"device_exclusions": value})
        guard(ok=False)
        sets([])
        assert "ether saddr" not in table("gofro_guard")
    for text in ('[]', '{} {}', '{"device_exclusions": [],}', '{"device_exclusions": []} junk', '{',
                 '{"device_exclusions":null,"device_exclusions":["02:00:00:00:00:01"]}'):
        config.write_text(text)
        guard(ok=False)
        sets([])
    commit({"device_exclusions": macs})
    for mode in (0o644, 0o666):
        config.chmod(mode)
        guard(ok=False)
        sets([])
    config.chmod(0o600)
    config.rename(state / "saved")
    config.symlink_to(state / "saved")
    guard(ok=False)
    sets([])
    config.unlink()
    (state / "saved").rename(config)
    os.link(config, state / "hardlink")
    guard(ok=False)
    (state / "hardlink").unlink()
    os.chown(config, 1234, 1234)
    guard(ok=False)
    sets([])
    os.chown(config, 0, 0)
    guard()
    sets(macs)
    guard(ok=False, GOFRO_APPLY_LOCK_FD="8")
    # An unrelated open FD8 is never accepted as the lifecycle fence.
    run("sh", "-c", 'exec 8>"$1"; GOFRO_APPLY_LOCK_FD=8 sh "$2" boot', "sh", str(root / "wrong.lock"), str(helpers / "guard"), env=env, ok=False)
    # Lock contention fences the CONFIG READ, not just nft publication.
    with (state / "apply.lock").open("a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        proc = subprocess.Popen(["sh", str(helpers / "guard"), "boot"], env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        time.sleep(0.15)
        assert proc.poll() is None
        commit({"device_exclusions": []})
        fcntl.flock(lock, fcntl.LOCK_UN)
        out, err = proc.communicate(timeout=15)
        assert proc.returncode == 0, (out, err)
    sets([])
    run("sh", "-c", 'exec 8>>"$1/apply.lock"; flock 8; GOFRO_APPLY_LOCK_FD=8 sh "$2" boot', "sh", str(state), str(helpers / "guard"), env=env)
    root.chmod(0o777)
    guard(ok=False)
    root.chmod(0o700)
    print("PASS strict config, 256/private MACs, crash candidate, trusted files, both atomic sets, fake-map retention, and inherited/standalone locks", flush=True)

    # Stop owns exactly two historical chain shapes, plus its empty chain.
    base = 'add chain inet gofro_routing gofro_dns { type nat hook prerouting priority -101; policy accept; }\n'
    rules = ''.join(f'add rule inet gofro_routing gofro_dns iifname "lan0" {p} dport 53 redirect to :5353\n' for p in ("udp", "tcp"))
    bypass = 'add rule inet gofro_routing gofro_dns iifname "lan0" ether saddr @device_exclusions return\n'
    for owned in (rules, bypass + rules, ""):
        nft(base + owned)
        guard("stop")
        assert "chain gofro_dns" not in table("gofro_routing")
    for foreign in (rules + bypass, bypass, rules.replace(":5353", ":9999"), rules.replace('"lan0"', '"wan0"'), rules + "add rule inet gofro_routing gofro_dns counter\n"):
        nft(base + foreign)
        before = table("gofro_routing")
        guard("stop", ok=False)
        assert table("gofro_routing") == before
        nft("flush chain inet gofro_routing gofro_dns\ndelete chain inet gofro_routing gofro_dns\n")
    print("PASS exact old/new/empty DNS chain ownership and foreign-chain rejection", flush=True)

    # Native conntrack netlink, including legacy redirects to LL/ULA/GUA.
    serial = 30000
    targets = []
    controls = []

    def flow(version, proto, *, mark=0, dport=53, reply=None, rport=None, target=False, dnat=True):
        global serial
        serial += 1
        src, dst = ("192.168.7.2", "198.51.100.1") if version == 4 else ("fd12::2", "2001:db8:ff::1")
        args = ["conntrack", "-I", "-p", proto, "-s", src, "-d", dst,
                "--sport", str(serial), "--dport", str(dport), "-r", reply or dst, "-q", src,
                "--reply-port-src", str(rport or dport), "--reply-port-dst", str(serial), "-t", "600", "--mark", str(mark)]
        if proto == "tcp": args += ["--state", "ESTABLISHED"]
        if reply and dnat: args += ["--dst-nat", reply]
        run(*args)
        (targets if target else controls).append((version, proto, serial))

    for version in (4, 6):
        for proto in ("udp", "tcp"):
            flow(version, proto, mark=0x40000000, target=True)
            flow(version, proto, mark=0x40020000, target=True)
            flow(version, proto, mark=0x40000000, dport=443)
            flow(version, proto, mark=0x20000)  # mutable VPN bit alone is NOT ownership
            flow(version, proto)
            addresses = ["192.168.7.1"] if version == 4 else ["fe80::1", "fd12::1", "2001:db8::1"]
            for address in addresses:
                flow(version, proto, reply=address, rport=5353, target=True)
                flow(version, proto, reply=address, rport=5353, dnat=False)
                flow(version, proto, reply=address, rport=5354)
                flow(version, proto, reply=address, rport=5353, dport=54)
            flow(version, proto, reply="192.168.8.1" if version == 4 else "fd99::1", rport=5353)
    run("sh", str(helpers / "dns-flows"), "cleanup", "lan0", "5353")
    for version, proto, sport in targets + controls:
        entries = run("conntrack", "-L", "-f", f"ipv{version}", "-p", proto, "--sport", str(sport)).stdout
        assert bool(entries.strip()) == ((version, proto, sport) in controls), (version, proto, sport, entries)
    run("sh", str(helpers / "dns-flows"), "cleanup", "lan0", "5353")
    print(f"PASS native conntrack cleanup: {len(targets)} DNS entries removed, {len(controls)} unrelated entries retained; idempotent zero-match", flush=True)

    stubdir = root / "bin"
    stubdir.mkdir()
    stub = stubdir / "conntrack"
    for diagnostic, status in (("netlink permission denied", 1), ("", 1), ("conntrack v1.4.8 (conntrack-tools): 0 flow entries have been deleted.", 2)):
        stub.write_text(f"#!/bin/sh\nprintf '%s\\n' '{diagnostic}' >&2\nexit {status}\n")
        stub.chmod(0o755)
        failed = guard(ok=False, PATH=f"{stubdir}:{os.environ['PATH']}")
        assert "conntrack deletion failed" in failed.stderr
        assert "drop" in table("gofro_guard")
    print("PASS cleanup distinguishes zero-match from genuine errors and leaves startup guard armed on failure", flush=True)
PY

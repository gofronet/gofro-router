#!/bin/sh
set -eu
# Exact committed v17 init + current helpers, in a cold disposable namespace.
[ "$(uname -s)" = Linux ] && [ "$(id -u)" = 0 ] && [ -f /.dockerenv ] || exit 1
ROOT="$(CDPATH='' cd "$(dirname "$0")/../../.." && pwd)"
ip -j address show | python3 -c 'import json,sys; assert all(x["ifname"]=="lo" or not x.get("addr_info") for x in json.load(sys.stdin)), "requires --network none"'
ns="glife$$"
trap 'ip netns del "$ns"' EXIT
ip netns add "$ns"
ip -n "$ns" link set lo up
ip netns exec "$ns" python3 - "$ROOT" <<'PY'
import json
import os
import pathlib
import shutil
import socket
import struct
import subprocess
import sys
import tempfile

repo = pathlib.Path(sys.argv[1])
source = repo / "deploy/openwrt/root"

def run(*args, ok=True, **kwargs):
    result = subprocess.run(args, capture_output=True, text=True, timeout=10, **kwargs)
    assert (result.returncode == 0) == ok, (args, result.stdout, result.stderr)
    return result.stdout

with tempfile.TemporaryDirectory(dir="/root") as tmp:
    root = pathlib.Path(tmp)
    state = root / "state"
    for path in (state, root / "bin", root / "helpers", root / "config", root / "delta"):
        path.mkdir(mode=0o700)

    def script(name, text):
        path = root / name
        path.write_text(text)
        path.chmod(0o755)
        return path

    def record(path, text):
        path.write_text(text)
        path.chmod(0o600)

    record(state / "guard-device", "br-lan\n")
    record(state / "controller.json", '{"device_exclusions":[]}\n')
    for name in ("guard", "dns-flows"):
        (root / "helpers" / name).symlink_to(source / "usr/libexec/gofro" / name)
    script("helpers/network", '#!/bin/sh\nprintf \'{"device":"br-lan","address":"192.168.7.1","subnet":"192.168.7.0/24"}\\n\'\n')
    script("helpers/mode", '#!/bin/sh\n[ "$*" = "check br-lan 192.168.7.0/24" ]\n')
    script("bin/ubus", '#!/bin/sh\n[ "$*" = \'call service list {"name":"gofro-agent"}\' ] || exit 1\nprintf \'{"gofro-agent":{"instances":{}}}\\n\'\n')
    native_uci = shutil.which("uci")
    script("bin/uci", f'#!/bin/sh\nexec "{native_uci}" -c "{root / "config"}" -t "{root / "delta"}" "$@"\n')
    env = dict(os.environ, GOFRO_GUARD_DIR=str(state), STATE_DIR=str(state),
               GOFRO_HELPERS=str(root / "helpers"), GOFRO_MODE_LOCK=str(root / "mode.lock"),
               PATH=f"{root / 'bin'}:{os.environ['PATH']}")
    env.pop("GOFRO_APPLY_LOCK_FD", None)
    # Read the immutable historical artifact, never a hand-recreated old init.
    old = run("git", "-c", f"safe.directory={repo}", "-C", str(repo), "show",
              "5132a7a:deploy/openwrt/root/etc/init.d/gofro-agent")
    assert "GOFRO_APPLY_LOCK_FD" not in old
    legacy = root / "v17-init"
    legacy.write_text(old)
    agent = script("agent", f'''#!/bin/sh
set -eu
. "{legacy}"
[ -z "${{GOFRO_APPLY_LOCK_FD:-}}" ] || exit 1
service_running() {{ return 1; }}
config_load() {{ :; }}
config_get() {{ interface=gt0; }}
procd_open_instance() {{ :; }}
procd_set_param() {{ :; }}
procd_append_param() {{ :; }}
fenced() {{ (exec 7>>"$GOFRO_GUARD_DIR/apply.lock"; ! flock -n 7); }}
procd_close_instance() {{ fenced; }}
case "$1" in
 start) action=start; start_service ;;
 stop) stop_service; fenced; echo kill >> "{root / 'service-log'}"; service_stopped ;;
 enable|disable) echo "$1" >> "{root / 'service-log'}" ;;
 *) exit 2 ;;
esac
(exec 7>>"$GOFRO_GUARD_DIR/apply.lock"; flock -n 7)
''')
    for action in ("start", "stop", "stop"):
        run("sh", str(agent), action, env=env)
    chain = run("nft", "-s", "list", "chain", "inet", "gofro_guard", "gofro_guard")
    assert 'iifname "br-lan" oifname != "br-lan" drop' in chain and "ether" not in chain
    print("PASS exact 5132a7a v17 start/stop + new guard: no env contract, no deadlock, parent fence held until release", flush=True)

    # Wrong descriptors are refused; same-inode unlocked descriptors acquire
    # the lock, and never bypass a competing writer's separate description.
    guard = str(root / "helpers/guard")
    record(state / "controller.json", '{"device_exclusions":["02:00:00:00:00:01"]}\n')
    run("sh", "-c", 'exec 8>"$1"; sh "$2" stop', "sh", str(root / "wrong"), guard, env=env, ok=False)
    assert run("nft", "-s", "list", "chain", "inet", "gofro_guard", "gofro_guard") == chain
    record(state / "controller.json", '{"device_exclusions":[]}\n')
    run("sh", "-c", 'exec 8>>"$1/apply.lock"; sh "$2" stop; (exec 7>>"$1/apply.lock"; ! flock -n 7)',
        "sh", str(state), guard, env=env)
    record(state / "controller.json", '{"device_exclusions":["02:00:00:00:00:01"]}\n')
    run("sh", "-c", 'exec 7>>"$1/apply.lock"; flock 7; exec 8>>"$1/apply.lock"; sh "$2" stop',
        "sh", str(state), guard, env=env, ok=False)
    assert run("nft", "-s", "list", "chain", "inet", "gofro_guard", "gofro_guard") == chain
    record(state / "controller.json", '{"device_exclusions":[]}\n')
    print("PASS wrong FD8 refused; unlocked same inode fenced; competing writer cannot be bypassed", flush=True)

    # Real START=8 recovery entrypoint, real old stop hook, current guard and
    # transaction restore. Only procd/ubus/reload boundaries are fixtures.
    (root / "config/dhcp").write_text("config dnsmasq\nconfig hostrecord 'operator'\n option name 'operator.test'\n option ip '192.0.2.9'\n")
    for package in ("network", "firewall"):
        (root / "config" / package).touch()
    reload = script("reload", f'''#!/bin/sh
set -eu
[ "$*" = reload ]
echo reload >> "{root / 'reload-log'}"
[ ! -e "{root / 'reload-fail'}" ]
name="$(uci get dhcp.gofro_panel.name)"
address="$(uci get dhcp.gofro_panel.ip)"
exec dnsmasq --no-resolv --no-hosts --bind-interfaces --listen-address=127.0.0.1 --port=1053 --user=root --pid-file="{root / 'dns.pid'}" --host-record="$name,$address" --host-record=operator.test,192.0.2.9
''')
    transaction = script("transaction", (source / "usr/libexec/gofro/transaction").read_text().replace("/etc/init.d/dnsmasq", str(reload)))
    backup = state / "backup"
    run("sh", str(transaction), "snapshot", str(backup), env=env)
    previous = root / "app/releases/0.5.17"
    candidate = root / "app/releases/0.5.18"
    previous.mkdir(parents=True)
    candidate.mkdir()
    (root / "app/current").symlink_to(candidate)
    record(state / "update-previous", f"{previous}\n{backup}\n")
    relay = script("relay", '#!/bin/sh\ncase "$1" in stop|enable) exit 0;; *) exit 1;; esac\n')
    recovery = script("recover", (source / "etc/init.d/gofro-recover").read_text()
                      .replace("/etc/init.d/gofro-agent", str(agent)).replace("/etc/init.d/gofro-relay", str(relay)))
    # No br-lan yet: marked DNS still goes, address-based legacy DNS stays.
    assert json.loads(run("ip", "-j", "link", "show"))[0]["ifname"] == "lo"
    for port, mark, sport in ((53, "0x40000000", 31001), (443, "0x40000000", 31002), (53, "0", 31003)):
        run("conntrack", "-I", "-p", "udp", "-s", "192.168.7.2", "-d", "198.51.100.1", "--sport", str(sport),
            "--dport", str(port), "-r", "192.168.7.1", "-q", "192.168.7.2", "--reply-port-src", "5353",
            "--reply-port-dst", str(sport), "--dst-nat", "192.168.7.1", "-t", "600", "--mark", mark)
    run("sh", "-c", '. "$1"; [ "$START" = 8 ]; boot', "sh", str(recovery), env=env | {"APP_ROOT": str(root / "app")})
    assert (root / "app/current").resolve() == previous
    assert (state / "update-restored").read_text().strip() == str(previous)
    assert (state / "version").read_text().strip() == "0.5.17"
    assert "disable" not in (root / "service-log").read_text()
    assert run("nft", "-s", "list", "chain", "inet", "gofro_guard", "gofro_guard") == chain
    for sport, present in ((31001, False), (31002, True), (31003, True)):
        assert bool(run("conntrack", "-L", "-p", "udp", "--sport", str(sport)).strip()) == present
    run("ip", "link", "add", "br-lan", "type", "dummy")
    run("ip", "address", "add", "192.168.7.1/24", "dev", "br-lan")
    run("sh", guard, "boot", env=env)
    assert not run("conntrack", "-L", "-p", "udp", "--sport", "31003").strip()
    print("PASS pending START8 recovery before LAN creation: old stop completes, guard retained, marked cleanup runs, legacy cleanup deferred until LAN exists", flush=True)

    ip = script("bin/ip", "#!/bin/sh\necho 'netlink lookup failed' >&2\nexit 1\n")
    before_log = (root / "service-log").read_text()
    run("sh", str(agent), "stop", env=env, ok=False)
    assert (root / "service-log").read_text() == before_log  # no procd kill after failed cleanup
    ip.write_text('#!/bin/sh\nprintf \'[{"ifname":null}]\\n\'\n')
    run("sh", guard, "stop", env=env, ok=False)
    ip.write_text('#!/bin/sh\nif [ "$*" = "-j link show" ]; then printf \'[{"ifname":"br-lan"}]\\n\'; else exit 1; fi\n')
    run("sh", guard, "stop", env=env, ok=False)
    ip.unlink()
    print("PASS link lookup failure, malformed dump and present-device address failure remain fatal", flush=True)

    # Commit succeeds, first activation fails. Retry must reload matching UCI
    # and expose real native DNS while retaining every unrelated config byte.
    (root / "reload-fail").touch()
    run("sh", str(transaction), "panel-apply", env=env, ok=False)
    committed = (root / "config/dhcp").read_bytes()
    assert run("uci", "changes", "dhcp", env=env) == ""
    assert not (root / "dns.pid").exists()
    (root / "reload-fail").unlink()
    run("sh", str(transaction), "panel-apply", env=env)
    try:
        assert (root / "config/dhcp").read_bytes() == committed
        assert (root / "reload-log").read_text() == "reload\nreload\n"
        for name, address in (("wifi.gofro.net", "198.18.0.0"), ("operator.test", "192.0.2.9")):
            question = b"".join(bytes([len(x)]) + x.encode() for x in name.split(".")) + b"\0" + struct.pack("!HH", 1, 1)
            packet = struct.pack("!HHHHHH", 4321, 256, 1, 0, 0, 0) + question
            with socket.socket(type=socket.SOCK_DGRAM) as s:
                s.settimeout(2)
                s.sendto(packet, ("127.0.0.1", 1053))
                data = s.recv(4096)
            assert data[:2] == packet[:2] and data[-4:] == socket.inet_aton(address)
    finally:
        os.kill(int((root / "dns.pid").read_text()), 15)
    print("PASS commit-good/reload-failed retry activates real native panel DNS, preserves operator DNS and exact UCI bytes", flush=True)
PY

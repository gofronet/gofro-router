#!/bin/sh
set -eu
# Native UCI against temporary files only; no service or network operations.
ROOT="$(CDPATH='' cd "$(dirname "$0")/../../.." && pwd)"
command -v uci >/dev/null
python3 - "$ROOT" "$(command -v uci)" <<'PY'
import os
import pathlib
import subprocess
import sys
import tempfile

source = pathlib.Path(sys.argv[1]) / "deploy/openwrt/root/usr/libexec/gofro/transaction"
native_uci = sys.argv[2]
with tempfile.TemporaryDirectory() as tmp:
    root = pathlib.Path(tmp)
    for name in ("config", "delta", "bin", "state"):
        (root / name).mkdir()
    env = dict(os.environ, PATH=f"{root / 'bin'}:{os.environ['PATH']}", STATE_DIR=str(root / "state"))
    uci = root / "bin/uci"
    uci.write_text(f'#!/bin/sh\nexec "{native_uci}" -c "{root / "config"}" -t "{root / "delta"}" "$@"\n')
    uci.chmod(0o755)
    reload = root / "reload"
    reload.write_text(f'#!/bin/sh\n[ "$*" = reload ] || exit 1\necho reload >> "{root / "reloads"}"\n[ ! -e "{root / "reload-fail"}" ]\n')
    reload.chmod(0o755)
    helper = root / "transaction"
    helper.write_text(source.read_text().replace("/etc/init.d/dnsmasq", str(reload)))
    helper.chmod(0o755)
    for package in ("network", "firewall"):
        (root / "config" / package).touch()

    def run(*args, ok=True):
        result = subprocess.run(args, env=env, text=True, capture_output=True, timeout=10)
        assert (result.returncode == 0) == ok, (args, result.stdout, result.stderr)
        return result.stdout

    def tx(*args, ok=True):
        return run("sh", str(helper), *map(str, args), ok=ok)

    def setup(extra="", instances="config dnsmasq\n"):
        (root / "config/dhcp").write_text(instances + '''\nconfig dhcp 'lan'
 option interface 'lan'
config host 'operator'
 option name 'preserve-me'
 option ip '192.0.2.9'
''' + extra)
        run("uci", "revert", "dhcp")
        (root / "reloads").write_text("")

    def exported():
        return run("uci", "export", "dhcp")

    setup()
    before = exported()
    tx("panel-check")
    assert exported() == before and not (root / "reloads").read_text()
    tx("snapshot", root / "absent")
    tx("panel-apply")
    assert run("uci", "get", "dhcp.gofro_panel.name").strip() == "wifi.gofro.net"
    assert run("uci", "get", "dhcp.gofro_panel.ip").strip() == "198.18.0.0"
    instance = run("uci", "get", "dhcp.gofro_panel.instance").strip()
    assert instance.startswith("cfg") and run("uci", "get", f"dhcp.{instance}").strip() == "dnsmasq"
    tx("panel-apply")
    assert (root / "reloads").read_text() == "reload\nreload\n"
    tx("restore", root / "absent")
    assert exported() == before, (before, exported())
    assert (root / "reloads").read_text() == "reload\nreload\nreload\n"
    tx("restore", root / "absent")
    assert (root / "reloads").read_text() == "reload\nreload\nreload\n"

    # A named LAN instance, with a separate guest and disabled DNS instance.
    instances = '''config dnsmasq 'guest'
 list interface 'guest'
config dnsmasq 'lan_dns'
 list interface 'lan'
config dnsmasq 'disabled'
 option disabled '1'
'''
    owned = '''config hostrecord 'gofro_panel'
 list name 'wifi.gofro.net'
 list ip '198.18.0.0'
 option instance 'lan_dns'
'''
    setup(owned, instances)
    before = exported()
    tx("snapshot", root / "present")
    tx("panel-apply")
    assert exported() == before and (root / "reloads").read_text() == "reload\n"
    run("uci", "delete", "dhcp.gofro_panel")
    run("uci", "commit", "dhcp")
    tx("restore", root / "present")
    assert exported() == before  # includes exact list/option types and order
    assert (root / "reloads").read_text() == "reload\nreload\n"
    (root / "reload-fail").touch()
    tx("restore", root / "present", ok=False)
    (root / "reload-fail").unlink()
    tx("restore", root / "present")
    assert exported() == before
    assert (root / "reloads").read_text() == "reload\n" * 4

    for section in (owned.replace("198.18.0.0", "192.0.2.7"), owned.replace("hostrecord", "host"),
                    owned + " option ttl '300'\n", owned.replace("lan_dns", "guest")):
        setup(section, instances)
        before = exported()
        tx("panel-check", ok=False)
        tx("panel-apply", ok=False)
        tx("snapshot", root / "refused", ok=False)
        assert exported() == before and not (root / "reloads").read_text()
        assert not (root / "refused").exists()
    for instances in ("config dnsmasq 'a'\nconfig dnsmasq 'b'\n", "config dnsmasq 'a'\n option port '0'\n",
                      "config dnsmasq 'a'\n list notinterface 'lan'\n"):
        setup(instances=instances)
        tx("panel-check", ok=False)
    setup()
    run("uci", "set", "dhcp.operator.ip=192.0.2.10")
    tx("panel-check", ok=False)
    assert run("uci", "changes", "dhcp").strip() == "dhcp.operator.ip='192.0.2.10'"
    setup()
    (root / "reload-fail").touch()
    tx("panel-apply", ok=False)
    committed = (root / "config/dhcp").read_bytes()
    assert run("uci", "changes", "dhcp") == ""
    (root / "reload-fail").unlink()
    tx("panel-apply")
    assert (root / "reloads").read_text() == "reload\nreload\n"
    assert (root / "config/dhcp").read_bytes() == committed
    print("PASS native UCI: read-only preflight, exact rollback, collision/pending-change isolation, committed-record reload failure retried without rewriting config")
PY

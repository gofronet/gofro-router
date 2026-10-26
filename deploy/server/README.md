# Selective WARP egress

This is development/candidate functionality, not a claim that a published release
already contains it. Upgrade the router and server together before use.

## Management

The managed-server panel exposes WARP installation, reconnect, disable and live
status. Installation requires explicit acceptance of Cloudflare's application
terms. The authenticated, CSRF-protected API is:

```json
POST /api/servers/warp
{"public_key":"<server key>","enabled":true,"accept_terms":true}
```

Disable with `enabled:false`. Restricted SSH accepts only the literal commands
`enable-warp` and `disable-warp`, not arbitrary options or shell expressions.
The corresponding root CLI is `gofro-router-server enable-warp --accept-terms`.

The signed server bundle contains a checksum-pinned **wgcf v2.3.1**, an unofficial
registration-only utility. It obtains a WireGuard profile after explicit terms
acceptance; **payload uses the Linux kernel WireGuard driver**, not `warp-svc`,
MASQUE, `wireguard-go` or a userspace proxy. The VPS downloads no executable for
registration. See [dependency provenance](WGCF.md). Supported hosts are Debian
bookworm/trixie and Ubuntu jammy/noble/resolute with kernel WireGuard support.

The Cloudflare device API is not an officially supported WireGuard provisioning
API. Registration and endpoint availability require real VPS qualification.
Existing unmanaged WARP installations, old MASQUE candidate state, and customized
Gofro server firewalls are refused, not adopted or overwritten. Failed setup may
leave a prepared, disconnected exit; inspect it before retrying. There is no
automatic MASQUE or ordinary-exit fallback.

Registration/account tokens and profiles remain root-only (`0700` directory,
`0600` files). Diagnostics from wgcf are private, never API response bodies.
Profiles are parsed as data: hooks/extra peers/duplicate settings are refused,
and profile text is never sourced or executed by `wg-quick`. DNS/MTU/IPv6 fields
from its generic profile do not change host settings.

## Data plane

The router's existing FakeDNS identifies domains, including exact, suffix and
GeoSite rules. `warp` is a fourth route target. Distinct fake IPs preserve different
egress decisions even if multiple domains share one real CDN IP.

* Ordinary router source: `10.202.0.N`.
* WARP source: `10.202.1.N` for the same router, N=2..254.
* WARP mark: `0x60000`, matching the existing VPN rule `0x20000/0x30000`.
* Router-owned mark mask: `0x70000` (the additional `0x40000` bit is reserved).
* Router WARP SNAT: priority 99, before OpenWrt fw4 masquerading.

The source identity is an inner IP header, not an skb mark expected to survive
WireGuard. The router does not need a second WireGuard peer or another relay.
Only managed profiles with an explicit Gofro tunnel address can produce WARP
traffic. Old/unprepared servers do not authorize the second source IP, so WireGuard
rejects it. Friend peers retain their original single-source AllowedIPs.

Server preparation atomically replaces only an exactly recognized Gofro firewall
and persists its WARP source guard **before** authorizing second source addresses.
All existing owner routes are preserved; collisions abort before installation.
WireGuard authorization/save failure rolls back the exact previous configuration.
New router owners on a prepared server receive both identities. Disabled WARP keeps
these identities reserved; revocation removes the peer, including both sources.

The VPS uses `gofro-warp-network.service`, namespace `gofro-warp`, host veth
`gofro-warp0`, namespace veth `gofro-warp1`, link `169.254.203.0/30`, and routing
table/priority `10900` (routes protocol 186). Foreign policy rules, table routes,
source return routes and existing namespace/interface names are refused.

`gofro-warp.service` is a oneshot setup/teardown service, not a payload daemon.
It creates kernel `CloudflareWARP` in the host namespace and then moves the
plaintext interface into `gofro-warp`. As documented by
[WireGuard](https://www.wireguard.com/netns/), the encrypted UDP socket stays in
its birthplace, so transport leaves directly via the normal VPS WAN without
an extra userspace hop or namespace-to-host NAT. Only that namespace gets a
default route to WARP. Setup verifies `warp=on` or `warp=plus` through the actual
interface; live status requires a fresh kernel handshake, not just an active
unit/link. WireGuard restarts propagate to the network and egress units.

The host default route and resolver are never changed. Registration is control
plane only, over the host's ordinary connection. The namespace has no default
route via its host veth, even before WARP connects or after it is disabled.
Its forward policy is DROP and permits user payload
it only towards `CloudflareWARP`, with reverse established/related traffic allowed.
The host independently drops WARP-source traffic going anywhere except its veth.
The WARP policy rule and unreachable default survive shutdown. Stopping WARP does
not restart WireGuard or relay, nor change ordinary VPN traffic.

The native WARP interface uses MTU 1420 and caps TCP MSS at 1380. The router's
existing conservative MSS cap is preserved. UDP/QUIC is not blocked, and a
1351-byte UDP payload fits both managed tunnels without fragmentation.
Path MTU discovery/ICMP remains necessary for larger datagrams. The main Gofro
tunnel's configured MTU is not reduced. IPv6 remains unsupported by the managed
data plane; explicit Direct device exclusions retain native OpenWrt IPv6.

Existing conntrack/NAT bindings are never silently reused across WARP/non-WARP
policy or server changes. Confirmed flows with the wrong translated source are
dropped until the application reconnects, including unreplied UDP/SYN flows.

## Limitations and maintenance

* No country selector or promise of access to a particular service. Verify the
  observed Cloudflare egress geolocation on the actual VPS.
* No HTTPS interception, SNI sniffing, proxy listener, or inbound public WARP port.
* Domain selection still depends on router DNS. DoH and cached real-IP connections
  have the same domain-identification limits as existing Gofro routing.
* `mode:all` ignores configured rules, as before; use `mode:rules` for selective WARP.
* Disabling the router VPN blocks WARP intent instead of converting it to Direct.
  Active WARP policies also keep managed IPv6 forwarding blocked while VPN is off.
* Don't manually flush the server/namespace nft ruleset or remove policy rules.
  Such operator actions are outside fail-closed guarantees. Uninstalling WARP,
  custom firewall integration and removing leftover namespaces require maintenance.
* Old agents cannot parse saved `warp` rules or FakeDNS target code 4. Back up
  controller state before development deployment; do not downgrade with WARP rules
  or leases still present. Published update signing remains unchanged.

Kernel WireGuard removes a userspace data path, but no transport is guaranteed
fastest for every endpoint or route. For hardware qualification, record WARP
egress/colo, handshake/RTT, repeated throughput samples and VPS CPU under the same
load; compare direct VPS and router-to-VPS-to-WARP paths. Do not describe local
namespace tests or one HTTP download as proof of maximum Internet performance.

Diagnostics on the VPS (never share `account.toml`, `profile.conf`, WireGuard or management keys):

```sh
gofro-router-server managed-status
systemctl status gofro-warp-network gofro-warp
journalctl -u gofro-warp-network -u gofro-warp
ip -4 rule show
ip -4 route show table 10900
ip netns exec gofro-warp wg show CloudflareWARP
ip netns exec gofro-warp nft list table inet gofro_warp
```

## Tests

`cargo test --workspace --locked` covers policy precedence, source mapping, lease
persistence, server owner/friend isolation and rollback. `bash deploy/server/tests/warp.sh`
checks production firewalls, strict profile validation, private reusable
registration, handshake freshness and foreign-route refusal without privileges.
The kernel regression `warp-netns.sh` uses only disposable namespaces and a fake
WARP interface, not a real Cloudflare account. `warp-wireguard.sh` exercises the
production setup/teardown through **real kernel WireGuard encryption** between
disposable local namespaces, including TCP, 1351-byte UDP, birth-socket isolation,
foreign interface refusal, restart and fail-closed teardown. CI runs both.
Neither test proves that Cloudflare registration or a target site is reachable
from your actual VPS.

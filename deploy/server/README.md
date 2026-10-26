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

The official `cloudflare-warp` package is installed from Cloudflare's signed APT
repository (Debian bookworm/trixie; Ubuntu jammy/noble/resolute). Existing unmanaged
WARP installations and customized Gofro server firewalls are refused, not adopted
or overwritten. Failed installation can leave a prepared, disconnected exit;
inspect its state before retrying. No user payload is sent through a fallback exit.

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

`warp-svc.service` runs inside that namespace using a systemd drop-in. Its resolver
file is bind-mounted only into the service's mount namespace. It uses the official
MASQUE tunnel in traffic-only (`warp`) mode. `gofro-warp-connect.service` connects
on startup and verifies `warp=on` or `warp=plus` through the WARP interface.
WireGuard restarts propagate to the isolated network, daemon and connection units.

The host default route and resolver are never changed. Namespace-local native
traffic (registration and encrypted Cloudflare transport) can use the VPS WAN.
Forwarded user traffic cannot: the namespace's forward policy is DROP and permits
it only towards `CloudflareWARP`, with reverse established/related traffic allowed.
The host independently drops WARP-source traffic going anywhere except its veth.
The WARP policy rule and unreachable default survive shutdown. Stopping WARP does
not restart WireGuard or relay, nor change ordinary VPN traffic.

TCP MSS is capped at 1240 for WARP's 1280-byte minimum MTU; UDP/QUIC is not blocked.
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

Diagnostics on the VPS (never share `reg.json`, WireGuard or management keys):

```sh
gofro-router-server managed-status
systemctl status gofro-warp-network warp-svc gofro-warp-connect
journalctl -u gofro-warp-network -u warp-svc -u gofro-warp-connect
ip -4 rule show
ip -4 route show table 10900
ip netns exec gofro-warp warp-cli status
ip netns exec gofro-warp nft list table inet gofro_warp
```

## Tests

`cargo test --workspace --locked` covers policy precedence, source mapping, lease
persistence, server owner/friend isolation and rollback. `bash deploy/server/tests/warp.sh`
checks production firewall templates and foreign-route refusal without privileges.
The kernel regression `warp-netns.sh` uses only disposable namespaces and a fake
WARP interface, not a real Cloudflare account. It does not prove that the official
client or any particular target site is reachable from your VPS.

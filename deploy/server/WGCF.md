# Registration-only dependency

The signed x86_64 server bundle includes **wgcf v2.3.1** as
`/usr/local/libexec/gofro-wgcf`. It is an unofficial WARP registration utility,
not a traffic transport. Gofro does not install `warp-svc`, `wireguard-go`, a
userspace proxy, or MASQUE for WARP egress.

- Source: <https://github.com/ViRb3/wgcf>
- Reviewed upstream revision: `de6bba7147dc965a1b8e4de113a709750697ccea`
- Pinned asset: `https://github.com/ViRb3/wgcf/releases/download/v2.3.1/wgcf_2.3.1_linux_amd64`
- SHA-256: `4fb7a136aa470ece11d17949c7db07b040ec9c604742dc8680bcf1889f85d0c8`
- Upstream license: [MIT](WGCF-LICENSE); upstream dependency versions and
  notices are available at the pinned source revision.

CI checks the pinned asset checksum before including it in the signed server
archive. The installed helper checks that same checksum before registration.
No executable is downloaded by the VPS's WARP enable operation. Register and
generate run with a timeout, explicit terms acceptance, root-only configuration
and private diagnostics. Existing registrations are reused; this is not a
license/referral generator or a WARP+ entitlement bypass.

The Cloudflare device API is not a supported public WireGuard provisioning API.
Its availability and WARP endpoint compatibility must be checked on the actual
VPS. Failure does not switch to MASQUE or leak traffic via the VPS's normal exit.
Only the Linux kernel WireGuard implementation carries payload after provisioning.

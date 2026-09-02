#!/bin/sh
set -eu

ROOT="$(CDPATH='' cd "$(dirname "$0")/../../.." && pwd)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

sed -n \
	-e '/^valid_version() {$/,/^}$/p' \
	-e '/^version_newer() {$/,/^}$/p' \
	-e '/^write_public_key() {$/,/^}$/p' \
	-e '/^route_release_assets() {$/,/^}$/p' \
	-e '/^platform_for() {$/,/^}$/p' \
	-e '/^enough_memory() {$/,/^}$/p' \
	"$ROOT/deploy/openwrt/root/usr/sbin/gofro-update" > "$TMP/version.sh"
sed -n -e '/^platform_for() {$/,/^}$/p' -e '/^enough_memory() {$/,/^}$/p' \
	"$ROOT/deploy/openwrt/install.sh" > "$TMP/install-platform.sh"
sed -n -e '/^platform_for() {$/,/^}$/p' -e '/^enough_memory() {$/,/^}$/p' \
	"$TMP/version.sh" > "$TMP/update-platform.sh"
cmp "$TMP/install-platform.sh" "$TMP/update-platform.sh"
# shellcheck disable=SC1091
. "$TMP/version.sh"

valid_version 0.4.0
valid_version 0.4 && exit 1
version_newer 0.4.0 0.3.9
version_newer 0.4.1 0.4.0
version_newer 1.0.0 0.99.99
version_newer 0.4.0 0.4.0 && exit 1
version_newer 0.3.9 0.4.0 && exit 1

[ "$(platform_for aarch64_cortex-a53)" = aarch64-openwrt-linux-musl ]
[ "$(platform_for aarch64_generic)" = aarch64-openwrt-linux-musl ]
[ "$(platform_for arm_arm926ej-s)" = armv5te-openwrt-linux-musleabi ]
[ "$(platform_for arm_xscale)" = armv5te-openwrt-linux-musleabi ]
[ "$(platform_for arm_arm1176jzf-s_vfp)" = armv6-openwrt-linux-musleabihf ]
[ "$(platform_for arm_cortex-a7)" = armv7-openwrt-linux-musleabi ]
[ "$(platform_for arm_cortex-a7_neon-vfpv4)" = armv7-openwrt-linux-musleabihf ]
[ "$(platform_for i386_pentium-mmx)" = i586-openwrt-linux-musl ]
[ "$(platform_for riscv64_generic)" = riscv64-openwrt-linux-musl ]
[ "$(platform_for x86_64)" = x86_64-openwrt-linux-musl ]
for unsupported in arm_fa526 armeb_xscale loongarch64_generic mips_24kc \
	mipsel_24kc powerpc64_e5500 powerpc_8548 unknown; do
	platform_for "$unsupported" && exit 1
done

enough_memory 196608
enough_memory 196607 && exit 1
enough_memory unknown && exit 1

write_public_key "$TMP/update-public.pem"
cmp "$TMP/update-public.pem" "$ROOT/deploy/openwrt/update-public.pem"

ip() { printf '%s\n' "$*" >> "$TMP/ip.log"; }
uci() { printf '%s\n' gt0; }
export mode=update
export DEFAULT_BASE_URL=https://github.com/gofronet/gofro-router/releases/latest/download
export BASE_URL=$DEFAULT_BASE_URL
export GITHUB_ASSETS_CIDR=185.199.108.0/22
ASSET_ROUTE=
route_release_assets
[ "$ASSET_ROUTE" = gt0 ]
grep -Fxq 'link show gt0' "$TMP/ip.log"
grep -Fxq 'route add 185.199.108.0/22 dev gt0' "$TMP/ip.log"

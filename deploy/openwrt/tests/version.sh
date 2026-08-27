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
	"$ROOT/deploy/openwrt/root/usr/sbin/gofro-update" > "$TMP/version.sh"
# shellcheck disable=SC1091
. "$TMP/version.sh"

valid_version 0.4.0
valid_version 0.4 && exit 1
version_newer 0.4.0 0.3.9
version_newer 0.4.1 0.4.0
version_newer 1.0.0 0.99.99
version_newer 0.4.0 0.4.0 && exit 1
version_newer 0.3.9 0.4.0 && exit 1

[ "$(platform_for mediatek/filogic cudy,tr3000-256mb-v1)" = aarch64-openwrt-linux-musl ]
platform_for mediatek/filogic cudy,unknown && exit 1

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

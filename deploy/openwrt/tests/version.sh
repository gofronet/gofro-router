#!/bin/sh
set -eu

ROOT="$(CDPATH='' cd "$(dirname "$0")/../../.." && pwd)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

sed -n \
	-e '/^valid_version() {$/,/^}$/p' \
	-e '/^version_newer() {$/,/^}$/p' \
	-e '/^write_public_key() {$/,/^}$/p' \
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

write_public_key "$TMP/update-public.pem"
cmp "$TMP/update-public.pem" "$ROOT/deploy/openwrt/update-public.pem"

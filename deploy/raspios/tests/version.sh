#!/bin/sh
set -eu

ROOT="$(CDPATH='' cd "$(dirname "$0")/../../.." && pwd)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

sed -n \
	-e '/^valid_version() {$/,/^}$/p' \
	-e '/^version_newer() {$/,/^}$/p' \
	-e '/^write_public_key() {$/,/^}$/p' \
	-e '/^platform_for() {$/,/^}$/p' \
	"$ROOT/deploy/raspios/root/usr/sbin/gofro-update" > "$TMP/version.sh"
TARGET=aarch64-raspios-linux-musl
# shellcheck disable=SC1091
. "$TMP/version.sh"

valid_version 0.5.0
valid_version 0.5 && exit 1
version_newer 0.5.0 0.4.9
version_newer 0.5.0 0.5.0 && exit 1
[ "$(platform_for aarch64 debian trixie 'Raspberry Pi 5 Model B Rev 1.0')" = "$TARGET" ]
[ "$(platform_for aarch64 raspbian trixie 'Raspberry Pi 5 Model B Rev 1.0')" = "$TARGET" ]
platform_for aarch64 debian bookworm 'Raspberry Pi 5 Model B Rev 1.0' && exit 1
platform_for aarch64 debian trixie 'Raspberry Pi 4 Model B Rev 1.5' && exit 1

write_public_key "$TMP/update-public.pem"
cmp "$TMP/update-public.pem" "$ROOT/deploy/raspios/update-public.pem"

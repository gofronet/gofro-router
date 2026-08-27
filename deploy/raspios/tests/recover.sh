#!/bin/sh
set -eu

# GNU mv supports the atomic -T switch used on Raspberry Pi OS; macOS mv does not.
[ "$(uname -s)" != Darwin ] || exit 0

ROOT="$(CDPATH='' cd "$(dirname "$0")/../../.." && pwd)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

mkdir -p "$TMP/app/releases/0.4.7" "$TMP/app/releases/0.5.0" "$TMP/etc"
ln -s "$TMP/app/releases/0.5.0" "$TMP/app/current"
printf '%s\n' "$TMP/app/releases/0.4.7" > "$TMP/etc/update-previous"
printf '%s\n' 0.4.7 > "$TMP/etc/version"

GOFRO_APP_ROOT=$TMP/app \
GOFRO_UPDATE_PREVIOUS=$TMP/etc/update-previous \
GOFRO_VERSION_FILE=$TMP/etc/version \
	sh "$ROOT/deploy/raspios/root/usr/libexec/gofro/recover"

[ "$(readlink "$TMP/app/current")" = "$TMP/app/releases/0.4.7" ]
[ "$(cat "$TMP/etc/version")" = 0.4.7 ]
[ ! -e "$TMP/etc/update-previous" ]

#!/bin/sh
set -eu

# GNU mv supports the atomic -T switch used on Raspberry Pi OS; macOS mv does not.
[ "$(uname -s)" != Darwin ] || exit 0

ROOT="$(CDPATH='' cd "$(dirname "$0")/../../.." && pwd)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

APP_ROOT=$TMP/app
RELEASES=$APP_ROOT/releases
CURRENT=$APP_ROOT/current
# Referenced by the sourced switch helper.
# shellcheck disable=SC2034
CURRENT_TMP=
mkdir -p "$RELEASES/0.4.7" "$RELEASES/0.5.0" "$RELEASES/0.6.0"

sed -n \
	-e '/^valid_version() {$/,/^}$/p' \
	-e '/^valid_release() {$/,/^}$/p' \
	-e '/^switch_current() {$/,/^}$/p' \
	-e '/^prune_releases() {$/,/^}$/p' \
	"$ROOT/deploy/raspios/install.sh" > "$TMP/functions.sh"
# shellcheck disable=SC1091
. "$TMP/functions.sh"

ln -s "$RELEASES/0.4.7" "$CURRENT"
switch_current "$RELEASES/0.5.0"
[ "$(readlink "$CURRENT")" = "$RELEASES/0.5.0" ]
valid_release "$RELEASES/0.4.7"
valid_release "$TMP/other/0.4.7" && exit 1

prune_releases "$RELEASES/0.5.0" "$RELEASES/0.4.7"
[ -d "$RELEASES/0.4.7" ]
[ -d "$RELEASES/0.5.0" ]
[ ! -e "$RELEASES/0.6.0" ]

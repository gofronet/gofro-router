#!/bin/sh
set -eu

# BusyBox and GNU mv support -T; macOS mv does not.
[ "$(uname -s)" != Darwin ] || exit 0

ROOT="$(CDPATH='' cd "$(dirname "$0")/../../.." && pwd)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

APP_ROOT=$TMP/app
RELEASES=$APP_ROOT/releases
CURRENT=$APP_ROOT/current
# shellcheck disable=SC2034
CURRENT_TMP=
mkdir -p "$RELEASES/0.3.0" "$RELEASES/0.4.0"

sed -n '/^switch_current() {$/,/^}$/p' "$ROOT/deploy/openwrt/install.sh" > "$TMP/switch-current.sh"
# shellcheck disable=SC1091
. "$TMP/switch-current.sh"

ln -s "$RELEASES/0.3.0" "$CURRENT"
previous="$(readlink "$CURRENT")"
switch_current "$RELEASES/0.4.0"
[ "$(readlink "$CURRENT")" = "$RELEASES/0.4.0" ]

# A failed health check restores the exact previous release.
switch_current "$previous"
[ "$(readlink "$CURRENT")" = "$RELEASES/0.3.0" ]

# A failed switch must report failure and keep the active release unchanged.
(
	# shellcheck disable=SC2329
	ln() { return 1; }
	switch_current "$RELEASES/0.4.0" && exit 1
)
[ "$(readlink "$CURRENT")" = "$RELEASES/0.3.0" ]

# A reboot during an update also restores the committed release and version.
STATE_DIR=$TMP/state
mkdir "$STATE_DIR"
printf '%s\n' 0.4.0 > "$STATE_DIR/version"
printf '%s\n' "$previous" > "$STATE_DIR/update-previous"
switch_current "$RELEASES/0.4.0"
sed -n '/^recover_update() {$/,/^}$/p' \
	"$ROOT/deploy/openwrt/root/etc/init.d/gofro-recover" > "$TMP/recover-update.sh"
# shellcheck disable=SC1091
. "$TMP/recover-update.sh"
recover_update
[ "$(readlink "$CURRENT")" = "$RELEASES/0.3.0" ]
[ "$(cat "$STATE_DIR/version")" = 0.3.0 ]
[ ! -e "$STATE_DIR/update-previous" ]

# Recovery keeps its marker when the atomic switch fails, so boot can retry.
printf '%s\n' "$previous" > "$STATE_DIR/update-previous"
switch_current "$RELEASES/0.4.0"
(
	# shellcheck disable=SC2329
	mv() { return 1; }
	recover_update && exit 1
)
[ -e "$STATE_DIR/update-previous" ]

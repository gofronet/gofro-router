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
	# shellcheck disable=SC2317,SC2329
	ln() { return 1; }
	set +e
	switch_current "$RELEASES/0.4.0"
	status=$?
	set -e
	[ "$status" -ne 0 ]
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
	# shellcheck disable=SC2317,SC2329
	mv() { return 1; }
	set +e
	recover_update
	status=$?
	set -e
	[ "$status" -ne 0 ]
)
[ -e "$STATE_DIR/update-previous" ]

# Installation refuses to fill overlay without one release plus a small margin.
sed -n '/^enough_space() {$/,/^}$/p' "$ROOT/deploy/openwrt/install.sh" > "$TMP/enough-space.sh"
# shellcheck disable=SC1091
. "$TMP/enough-space.sh"
ROOTFS=$TMP/rootfs
mkdir "$ROOTFS"
du() { printf '100\t%s\n' "$ROOTFS"; }
df() { printf 'Filesystem 1024-blocks Used Available Capacity Mounted on\noverlay 1000 388 %s 39%% /overlay\n' "$TEST_AVAILABLE"; }
TEST_AVAILABLE=612
enough_space
TEST_AVAILABLE=611
enough_space && exit 1

# Pruning preserves both sides of an interrupted transaction until recovery.
sed -n '/^prune_releases() {$/,/^}$/p' "$ROOT/deploy/openwrt/install.sh" > "$TMP/prune-releases.sh"
# shellcheck disable=SC1091
. "$TMP/prune-releases.sh"
mkdir "$RELEASES/0.5.0"
prune_releases "$RELEASES/0.4.0" "$RELEASES/0.3.0"
[ -d "$RELEASES/0.3.0" ]
[ -d "$RELEASES/0.4.0" ]
[ ! -e "$RELEASES/0.5.0" ]
prune_releases "$RELEASES/0.4.0" ''
[ ! -e "$RELEASES/0.3.0" ]
[ -d "$RELEASES/0.4.0" ]

#!/bin/sh
set -eu

ROOT="$(CDPATH='' cd "$(dirname "$0")/../../.." && pwd)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

sed -n '/^status_healthy() {$/,/^}$/p' "$ROOT/deploy/openwrt/install.sh" > "$TMP/health.sh"
# shellcheck disable=SC1091
. "$TMP/health.sh"

jsonfilter() {
	while [ "$#" -gt 0 ]; do
		if [ "$1" = -e ]; then
			expression=$2
			break
		fi
		shift
	done
	case "$expression" in
		'@.version') printf '%s\n' "$TEST_VERSION" ;;
		'@.routing.dns_active') printf '%s\n' "$TEST_DNS" ;;
		'@.routing.dataplane_active') printf '%s\n' "$TEST_DATAPLANE" ;;
		'@.vpn_enabled') printf '%s\n' "$TEST_VPN" ;;
		'@.tunnel_active') printf '%s\n' "$TEST_TUNNEL" ;;
		'@.peer.handshake_age_seconds') printf '%s\n' "$TEST_HANDSHAKE_AGE" ;;
		*) return 1 ;;
	esac
}

uci() {
	[ "$*" = '-q get gofro.main.interface' ]
	printf '%s\n' gt0
}

ip() {
	[ "$*" = 'link show gt0' ]
	printf '7: gt0: <POINTOPOINT,UP> mtu %s state UNKNOWN\n' "$TEST_MTU"
}

# Referenced by the sourced function; ShellCheck cannot follow the generated file.
# shellcheck disable=SC2034
STATUS_FILE=$TMP/status
# shellcheck disable=SC2034
VERSION=0.4.0
TEST_VERSION=0.4.0
TEST_DNS=true
TEST_DATAPLANE=true
TEST_VPN=false
TEST_TUNNEL=false
TEST_HANDSHAKE_AGE=
TEST_MTU=1280
status_healthy

sed -n '/^install_status_healthy() {$/,/^}$/p' \
	"$ROOT/deploy/openwrt/root/etc/init.d/gofro-finalize" > "$TMP/install-health.sh"
# shellcheck disable=SC1091
. "$TMP/install-health.sh"
# Referenced by the sourced function; ShellCheck cannot follow the generated file.
# shellcheck disable=SC2034
status_file=$STATUS_FILE
install_status_healthy 0.4.0

TEST_DATAPLANE=false
set +e
install_status_healthy 0.4.0
status=$?
set -e
[ "$status" -ne 0 ]
TEST_DATAPLANE=true

TEST_VPN=true
TEST_TUNNEL=true
TEST_HANDSHAKE_AGE=30
status_healthy

TEST_HANDSHAKE_AGE=181
set +e
status_healthy
status=$?
set -e
[ "$status" -ne 0 ]

TEST_HANDSHAKE_AGE=30
TEST_TUNNEL=false
set +e
status_healthy
status=$?
set -e
[ "$status" -ne 0 ]

TEST_TUNNEL=true
TEST_MTU=1360
set +e
status_healthy
status=$?
set -e
[ "$status" -ne 0 ]

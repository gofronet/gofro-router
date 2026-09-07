#!/bin/sh
set -eu

ROOT="$(CDPATH='' cd "$(dirname "$0")/../../.." && pwd)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

sed -n '/^status_healthy() {$/,/^}$/p' "$ROOT/deploy/raspios/install.sh" > "$TMP/health.sh"
# shellcheck disable=SC1091
. "$TMP/health.sh"

jq() {
	case "$2" in
		.version) printf '%s\n' "$TEST_VERSION" ;;
		.dns_active) printf '%s\n' "$TEST_DNS" ;;
		.dataplane_active) printf '%s\n' "$TEST_DATAPLANE" ;;
		.vpn_enabled) printf '%s\n' "$TEST_VPN" ;;
		.tunnel_active) printf '%s\n' "$TEST_TUNNEL" ;;
		.handshake_age_seconds) printf '%s\n' "$TEST_HANDSHAKE_AGE" ;;
		*) return 1 ;;
	esac
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

TEST_VPN=true
TEST_TUNNEL=true
TEST_HANDSHAKE_AGE=30
status_healthy

TEST_HANDSHAKE_AGE=181
if status_healthy; then
	exit 1
fi

TEST_HANDSHAKE_AGE=30
TEST_TUNNEL=false
if status_healthy; then
	exit 1
fi

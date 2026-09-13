#!/bin/sh
set -eu
ROOT="$(CDPATH='' cd "$(dirname "$0")/../../.." && pwd)"
SETUP="$ROOT/deploy/openwrt/root/usr/sbin/gofro-setup"
grep -Fq 'usage: gofro-setup [COUNTRY]' "$SETUP"
grep -Fq 'openssl rand -hex 16' "$SETUP"
grep -Fq 'https://%s:8443' "$SETUP"
if grep -Eiq 'wifi reload|/etc/init.d/network|uhttpd|dhcp\.|wireless\.' "$SETUP"; then exit 1; fi

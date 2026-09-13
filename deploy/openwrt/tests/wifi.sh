#!/bin/sh
set -eu
ROOT="$(CDPATH='' cd "$(dirname "$0")/../../.." && pwd)"
if sh "$ROOT/deploy/openwrt/root/usr/libexec/gofro/wifi" list >/dev/null 2>&1; then exit 1; fi
if grep -Eiq 'uci |wifi reload|wireless\.' "$ROOT/deploy/openwrt/root/usr/libexec/gofro/wifi"; then exit 1; fi

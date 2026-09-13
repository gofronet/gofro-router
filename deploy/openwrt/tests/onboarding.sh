#!/bin/sh
set -eu
ROOT="$(CDPATH='' cd "$(dirname "$0")/../../.." && pwd)"
for action in guard recover watch begin complete; do sh "$ROOT/deploy/openwrt/root/usr/libexec/gofro/onboarding" "$action"; done
if grep -Eiq 'wireless|wifi reload|br-gofro-setup' "$ROOT/deploy/openwrt/root/usr/libexec/gofro/onboarding"; then exit 1; fi

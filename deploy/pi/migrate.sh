#!/usr/bin/env bash
set -euo pipefail

ACTION=${1:-}
OLD_VERSION=${2:-}
NEW_VERSION=${3:-}
[[ $OLD_VERSION =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || exit 2
[[ $NEW_VERSION =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || exit 2

# Keep `up` and `down` reversible until `commit`. Future releases add only
# version-gated, idempotent migrations here.
case $ACTION in
  up|down|commit) ;;
  *) exit 2 ;;
esac

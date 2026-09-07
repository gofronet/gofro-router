#!/usr/bin/env bash
set -euo pipefail

ROOT="$(CDPATH='' cd "$(dirname "$0")/../../.." && pwd)"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
sed -n -e '/^valid_version()/,/^}/p' -e '/^version_newer()/,/^}/p' -e '/^write_public_key()/,/^}/p' -e '/^manifest_valid()/,/^}/p' "$ROOT/deploy/server/gofro-server-install" > "$TMP/functions.sh"
# shellcheck disable=SC1091
source "$TMP/functions.sh"
valid_version 1.2.3
valid_version 1.2 && exit 1
version_newer 1.2.4 1.2.3
version_newer 1.2.3 1.2.3 && exit 1
write_public_key "$TMP/key"
cmp "$TMP/key" "$ROOT/deploy/server/update-public.pem"

# Referenced by the sourced manifest helper.
# shellcheck disable=SC2034
TARGET=x86_64-server-linux-musl
# shellcheck disable=SC2034
ARCHIVE=gofro-router-x86_64-server-linux-musl.tar.gz
printf '%s\n' '{"schema":1,"version":"1.2.3","target":"x86_64-server-linux-musl","archive":"gofro-router-x86_64-server-linux-musl.tar.gz","sha256":"0123456789012345678901234567890123456789012345678901234567890123"}' > "$TMP/manifest"
manifest_valid "$TMP/manifest"
printf '%s\n' '{"schema":2}' > "$TMP/manifest"
manifest_valid "$TMP/manifest" && exit 1
bash "$ROOT/deploy/server/install.sh" invalid >/dev/null 2>&1 && exit 1
grep -Fq 'drop; }' "$ROOT/deploy/server/install.sh"
grep -Fq 'masquerade; }' "$ROOT/deploy/server/install.sh"

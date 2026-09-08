#!/usr/bin/env bash
set -euo pipefail

ROOT="$(CDPATH='' cd "$(dirname "$0")/../../.." && pwd)"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
sed "s|/usr/local/bin/gofro-router-server|$TMP/server|g; s|/usr/local/sbin/gofro-server-install|$TMP/install|g" \
  "$ROOT/deploy/server/gofro-managed" > "$TMP/gofro-managed"
printf '%s\n' '#!/usr/bin/env bash' "printf '%s\\n' \"\$*\" >> '$TMP/argv'" > "$TMP/server"
printf '%s\n' '#!/usr/bin/env bash' 'exit 0' > "$TMP/install"
chmod 700 "$TMP/server" "$TMP/install"

# The forced-command wrapper must reject shell syntax before it reaches a binary.
if SSH_ORIGINAL_COMMAND='managed-status; touch /tmp/gofro-managed-injection' \
  bash "$ROOT/deploy/server/gofro-managed" >/dev/null 2>&1; then
  exit 1
else
  [[ $? == 126 ]]
fi
[[ ! -e /tmp/gofro-managed-injection ]]

if SSH_ORIGINAL_COMMAND='create-friend 198.51.100.1:8443; id' \
  bash "$ROOT/deploy/server/gofro-managed" >/dev/null 2>&1; then
  exit 1
else
  [[ $? == 126 ]]
fi

key='Aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa='
for command in \
  'managed-status' \
  'create-friend 198.51.100.1:8443' \
  "rename-friend $key" \
  "revoke-friend $key" \
  "friend-profile $key [2001:db8::1]:8443" \
  'restart-vpn'; do
  SSH_ORIGINAL_COMMAND=$command bash "$TMP/gofro-managed" >/dev/null 2>&1
done
grep -Fx 'managed-status' "$TMP/argv"
grep -Fx 'create-friend 198.51.100.1:8443' "$TMP/argv"
grep -Fx "rename-friend $key" "$TMP/argv"
grep -Fx "revoke-friend $key" "$TMP/argv"
grep -Fx "friend-profile $key [2001:db8::1]:8443" "$TMP/argv"
grep -Fx 'restart-vpn' "$TMP/argv"

if SSH_ORIGINAL_COMMAND='create-friend 999.51.100.1:8443' \
  bash "$ROOT/deploy/server/gofro-managed" >/dev/null 2>&1; then
  exit 1
else
  [[ $? == 126 ]]
fi

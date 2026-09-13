#!/usr/bin/env bash
set -euo pipefail

ROOT="$(CDPATH='' cd "$(dirname "$0")/../../.." && pwd)"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
sed "s|/usr/local/bin/gofro-router-server|$TMP/server|g; s|/usr/local/sbin/gofro-server-install|$TMP/install|g" \
  "$ROOT/deploy/server/gofro-managed" > "$TMP/gofro-managed"
printf '%s\n' '#!/usr/bin/env bash' "printf '%s\\n' \"\$*\" >> '$TMP/argv'" "printf '%s:%s\\n' \"\$#\" \"\$*\" >> '$TMP/argc'" > "$TMP/server"
printf '%s\n' '#!/usr/bin/env bash' 'exit 0' > "$TMP/install"
printf '%s\n' '#!/usr/bin/env bash' 'shift' 'exec "$@"' > "$TMP/timeout"
chmod 700 "$TMP/server" "$TMP/install" "$TMP/timeout"
export PATH="$TMP:$PATH"

# The forced-command wrapper must reject shell syntax before it reaches a binary.
if SSH_ORIGINAL_COMMAND="managed-status; touch $TMP/injection" \
  bash "$TMP/gofro-managed" >/dev/null 2>&1; then
  exit 1
else
  [[ $? == 126 ]]
fi
[[ ! -e $TMP/injection ]]

if SSH_ORIGINAL_COMMAND='create-friend 198.51.100.1:8443; id' \
  bash "$TMP/gofro-managed" >/dev/null 2>&1; then
  exit 1
else
  [[ $? == 126 ]]
fi

key='Aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa='
for command in \
  'managed-status' \
  'capabilities' \
  'create-friend 198.51.100.1:8443' \
  "rename-friend $key" \
  "revoke-friend $key" \
  "friend-profile $key 198.51.100.1:8443" \
  "friend-profile $key [2001:db8::1]:8443" \
  'create-profile 198.51.100.1:8443' \
  'create-router-profile 198.51.100.1:8443' \
  'create-router-profile [2001:db8::1]:8443' \
  "remove-router-peer $key" \
  'restart-vpn'; do
  SSH_ORIGINAL_COMMAND=$command bash "$TMP/gofro-managed" >/dev/null 2>&1
done
grep -Fx 'managed-status' "$TMP/argv"
grep -Fx 'capabilities' "$TMP/argv"
grep -Fx 'create-friend 198.51.100.1:8443' "$TMP/argv"
grep -Fx "rename-friend $key" "$TMP/argv"
grep -Fx "revoke-friend $key" "$TMP/argv"
grep -Fx "friend-profile $key 198.51.100.1:8443" "$TMP/argv"
grep -Fx "friend-profile $key [2001:db8::1]:8443" "$TMP/argv"
grep -Fx 'create-profile --endpoint 198.51.100.1:8443' "$TMP/argv"
grep -Fx 'create-router-profile 198.51.100.1:8443' "$TMP/argv"
grep -Fx '2:create-router-profile [2001:db8::1]:8443' "$TMP/argc"
grep -Fx '3:friend-profile Aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa= 198.51.100.1:8443' "$TMP/argc"
grep -Fx '3:friend-profile Aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa= [2001:db8::1]:8443' "$TMP/argc"
grep -Fx "remove-router-peer $key" "$TMP/argv"
if grep -F '10.203' "$TMP/argv"; then exit 1; fi
grep -Fx 'restart-vpn' "$TMP/argv"

if SSH_ORIGINAL_COMMAND='create-friend 999.51.100.1:8443' \
  bash "$TMP/gofro-managed" >/dev/null 2>&1; then
  exit 1
else
  [[ $? == 126 ]]
fi

before=$(wc -l < "$TMP/argv")
for command in \
  'create-router-profile 198.051.100.1:8443' \
  'create-router-profile 198.51.100.1:51820' \
  'create-router-profile vpn.test:8443' \
  'capabilities extra' \
  "remove-router-peer $key extra" \
  "friend-profile $key 198.51.100.1:8443; id"; do
  if SSH_ORIGINAL_COMMAND=$command bash "$TMP/gofro-managed" >/dev/null 2>&1; then
    exit 1
  else
    [[ $? == 126 ]]
  fi
done
[[ $(wc -l < "$TMP/argv") == "$before" ]]

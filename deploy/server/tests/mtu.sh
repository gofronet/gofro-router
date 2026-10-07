#!/usr/bin/env bash
set -euo pipefail

ROOT="$(CDPATH='' cd "$(dirname "$0")/../../.." && pwd)"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
for function in valid_mtu server_interface configured_mtu updated_mtu write_mtu live_mtu activate_mtu restore_mtu; do
  sed -n "/^$function() {$/,/^}/p" "$ROOT/deploy/server/install.sh" >> "$TMP/functions.sh"
done
# shellcheck disable=SC1091
source "$TMP/functions.sh"

cat > "$TMP/service" <<'EOF'
[Unit]
Requires=wg-quick@vpn-test.service
After=wg-quick@vpn-test.service
EOF
[[ $(server_interface "$TMP/service") == vpn-test ]]
printf '\nRequires=wg-quick@other.service\n' >> "$TMP/service"
if server_interface "$TMP/service"; then exit 1; fi
printf 'Requires=wg-quick@bad;name.service\n' > "$TMP/service"
if server_interface "$TMP/service"; then exit 1; fi

cat > "$TMP/initial" <<'EOF'
[Unit]
Requires=gofro-firewall.service
After=gofro-firewall.service
[Service]
ExecStartPost=/usr/sbin/ip link set dev vpn-test mtu 1280
# operator comment must survive update and rollback
EOF
[[ $(configured_mtu "$TMP/initial" vpn-test) == 1280 ]]
if configured_mtu "$TMP/initial" gt0; then exit 1; fi
[[ $(updated_mtu 1280 1379) == 1379 ]]
[[ $(updated_mtu 1360 1379) == 1360 ]]
[[ $(updated_mtu 1379 1379) == 1379 ]]
[[ $(updated_mtu 1280 1379 1280) == 1280 ]]
for value in 1279 65536 01379 18446744073709552995 '+1379' '1379; id'; do
  if updated_mtu 1280 1379 "$value"; then exit 1; fi
done

ip() {
  printf 'ip:%s\n' "$*" >> "$TMP/commands"
  case "$*" in
    '-o link show dev vpn-test') printf '7: vpn-test: <UP> mtu %s state UNKNOWN\n' "$(cat "$TMP/live")" ;;
    'link set dev vpn-test mtu '*)
      [[ ${FAIL_STEP:-} != ip ]] || return 1
      printf '%s\n' "$6" > "$TMP/live" ;;
    *) return 1 ;;
  esac
}

systemctl() {
  printf 'systemctl:%s\n' "$*" >> "$TMP/commands"
  case "$*" in
    daemon-reload) [[ ${FAIL_STEP:-} != reload ]] ;;
    'restart gofro-relay.service') [[ ${FAIL_STEP:-} != restart ]] ;;
    *) return 1 ;;
  esac
}

for failure in none reload restart ip; do
  cp "$TMP/initial" "$TMP/dropin"
  printf '1280\n' > "$TMP/live"
  : > "$TMP/commands"
  before=$(live_mtu vpn-test)
  FAIL_STEP=$failure
  if [[ $failure == none ]]; then
    activate_mtu "$TMP/dropin" vpn-test 1379
    [[ $(cat "$TMP/live") == 1379 ]]
    [[ $(configured_mtu "$TMP/dropin" vpn-test) == 1379 ]]
    grep -Fqx '# operator comment must survive update and rollback' "$TMP/dropin"
    [[ $(cat "$TMP/commands") == $'ip:-o link show dev vpn-test\nsystemctl:daemon-reload\nsystemctl:restart gofro-relay.service\nip:link set dev vpn-test mtu 1379' ]]
  else
    if activate_mtu "$TMP/dropin" vpn-test 1379; then exit 1; fi
    FAIL_STEP=
    restore_mtu "$TMP/initial" "$TMP/dropin" vpn-test "$before"
    cmp "$TMP/initial" "$TMP/dropin"
    [[ $(cat "$TMP/live") == 1280 ]]
  fi
done
printf 'PASS: server MTU migration, custom settings and rollback\n'

#!/bin/sh
set -eu

ROOT=$(CDPATH='' cd "$(dirname "$0")/../../.." && pwd)
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
mkdir "$TMP/bin"
cat > "$TMP/bin/mktemp" <<'EOF'
#!/bin/sh
: > "$GOFRO_TEST_LOG"
printf '%s\n' "$GOFRO_TEST_LOG"
EOF
chmod +x "$TMP/bin/mktemp"
export GOFRO_TEST_LOG="$TMP/install.log"

for platform in openwrt raspios; do
	for source in install.sh root/usr/sbin/gofro-setup root/usr/sbin/gofro-update; do
		{
			printf '%s\n' '#!/bin/sh' 'set -eu' 'mode=install' 'die() { printf "%s\n" "$*" >&2; exit 1; }'
			sed -n '/^if .*GOFRO_INSTALL_QUIET/,/^fi$/p' "$ROOT/deploy/$platform/$source"
			# shellcheck disable=SC2016
			printf '%s\n' 'printf "fixture-password\n"' 'printf "package-output\n" >&2' 'exit "${GOFRO_TEST_EXIT:-0}"'
		} > "$TMP/install.sh"
		output=$(PATH="$TMP/bin:$PATH" GOFRO_INSTALL_QUIET='' sh "$TMP/install.sh" 2> "$TMP/stderr")
		[ "$output" = "$(printf '%s\n' 'GofroNET Wi-Fi Setup' 'https://wifi.gofro.net')" ]
		[ ! -s "$TMP/stderr" ]
		grep -q fixture-password "$TMP/install.log"
		if PATH="$TMP/bin:$PATH" GOFRO_INSTALL_QUIET='' GOFRO_TEST_EXIT=7 sh "$TMP/install.sh" > "$TMP/stdout" 2> "$TMP/stderr"; then exit 1; fi
		[ ! -s "$TMP/stdout" ]
		if grep -q fixture-password "$TMP/stderr"; then exit 1; fi
		grep -q 'installation failed' "$TMP/stderr"
	done
done

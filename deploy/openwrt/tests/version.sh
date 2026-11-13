#!/bin/sh
set -eu

ROOT="$(CDPATH='' cd "$(dirname "$0")/../../.." && pwd)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

sed -n \
	-e '/^die() {$/,/^}$/p' \
	-e '/^valid_version() {$/,/^}$/p' \
	-e '/^version_newer() {$/,/^}$/p' \
	-e '/^write_public_key() {$/,/^}$/p' \
	-e '/^route_release_assets() {$/,/^}$/p' \
	-e '/^release_metadata() {$/,/^}$/p' \
	-e '/^download_asset() {$/,/^}$/p' \
	-e '/^platform_for() {$/,/^}$/p' \
	-e '/^enough_memory() {$/,/^}$/p' \
	"$ROOT/deploy/openwrt/root/usr/sbin/gofro-update" > "$TMP/version.sh"
sed -n -e '/^platform_for() {$/,/^}$/p' -e '/^enough_memory() {$/,/^}$/p' \
	"$ROOT/deploy/openwrt/install.sh" > "$TMP/install-platform.sh"
sed -n -e '/^platform_for() {$/,/^}$/p' -e '/^enough_memory() {$/,/^}$/p' \
	"$TMP/version.sh" > "$TMP/update-platform.sh"
cmp "$TMP/install-platform.sh" "$TMP/update-platform.sh"
# shellcheck disable=SC1091
. "$TMP/version.sh"

valid_version 0.4.0
valid_version 0.4 && exit 1
version_newer 0.4.0 0.3.9
version_newer 0.4.1 0.4.0
version_newer 1.0.0 0.99.99
version_newer 0.4.0 0.4.0 && exit 1
version_newer 0.3.9 0.4.0 && exit 1

[ "$(platform_for aarch64_cortex-a53)" = aarch64-openwrt-linux-musl ]
[ "$(platform_for aarch64_generic)" = aarch64-openwrt-linux-musl ]
[ "$(platform_for arm_arm926ej-s)" = armv5te-openwrt-linux-musleabi ]
[ "$(platform_for arm_xscale)" = armv5te-openwrt-linux-musleabi ]
[ "$(platform_for arm_arm1176jzf-s_vfp)" = armv6-openwrt-linux-musleabihf ]
[ "$(platform_for arm_cortex-a7)" = armv7-openwrt-linux-musleabi ]
[ "$(platform_for arm_cortex-a7_neon-vfpv4)" = armv7-openwrt-linux-musleabihf ]
[ "$(platform_for i386_pentium-mmx)" = i586-openwrt-linux-musl ]
[ "$(platform_for riscv64_generic)" = riscv64-openwrt-linux-musl ]
[ "$(platform_for x86_64)" = x86_64-openwrt-linux-musl ]
for unsupported in arm_fa526 armeb_xscale loongarch64_generic mips_24kc \
	mipsel_24kc powerpc64_e5500 powerpc_8548 unknown; do
	platform_for "$unsupported" && exit 1
done

enough_memory 49152
enough_memory 49151 && exit 1
enough_memory unknown && exit 1

write_public_key "$TMP/update-public.pem"
cmp "$TMP/update-public.pem" "$ROOT/deploy/openwrt/update-public.pem"

ip() { printf '%s\n' "$*" >> "$TMP/ip.log"; }
uci() { printf '%s\n' gt0; }
export mode=update
export DEFAULT_BASE_URL=https://github.com/gofronet/gofro-router/releases/latest/download
export BASE_URL=$DEFAULT_BASE_URL
export GITHUB_ASSETS_CIDR=185.199.108.0/22
ASSET_ROUTE=
route_release_assets
[ "$ASSET_ROUTE" = gt0 ]
grep -Fxq 'link show gt0' "$TMP/ip.log"
grep -Fxq 'route add 185.199.108.0/22 dev gt0' "$TMP/ip.log"

# Public downloads use the API, not the blocked github.com redirect. Fixture
# parsing mirrors native jsonfilter; the same selector is exercised on OpenWrt.
GITHUB_API=https://api.github.com/repos/gofronet/gofro-router
cat > "$TMP/release.fixture" <<EOF
{"tag_name":"v0.5.28","assets":[
 {"name":"manifest.json","url":"$GITHUB_API/releases/assets/101"},
 {"name":"manifest.json.sig","url":"$GITHUB_API/releases/assets/102"},
 {"name":"bundle.tar.gz","url":"$GITHUB_API/releases/assets/103"}]}
EOF
jsonfilter() {
	python3 - "$2" "$4" <<'PY'
import json, re, sys
data = json.load(open(sys.argv[1]))
expression = sys.argv[2]
if expression == '@.tag_name':
    print(data.get('tag_name', ''))
else:
    match = re.fullmatch(r'@\.assets\[@\.name="([^"]+)"\]\.url', expression)
    assert match, expression
    for asset in data.get('assets', []):
        if asset.get('name') == match[1]:
            print(asset.get('url', ''))
PY
}
# Mock the executable's hyphenated name in the fixture's supported shells.
# shellcheck disable=SC3033
uclient-fetch() {
	fetch_output='' fetch_header=''
	while [ "$#" -gt 0 ]; do
		case "$1" in
			-T) shift 2 ;;
			-O) fetch_output=$2; shift 2 ;;
			--header=*) fetch_header=${1#--header=}; shift ;;
			*) fetch_url=$1; shift ;;
		esac
	done
	printf '%s %s\n' "$fetch_url" "$fetch_header" >> "$TMP/fetch.log"
	[ "${FETCH_FAIL:-0}" = 0 ] || return 4
	case "$fetch_url" in
		"$GITHUB_API/releases/latest") cp "$TMP/release.fixture" "$fetch_output" ;;
		*) printf '%s\n' asset-bytes > "$fetch_output" ;;
	esac
}
release_metadata
[ "$RELEASE_TAG" = v0.5.28 ]
for asset in manifest.json manifest.json.sig bundle.tar.gz; do
	download_asset "$asset" 60 "$TMP/download"
	[ "$(cat "$TMP/download")" = asset-bytes ]
done
grep -Fxq "$GITHUB_API/releases/assets/101 Accept: application/octet-stream" "$TMP/fetch.log"
grep -Fxq "$GITHUB_API/releases/assets/103 Accept: application/octet-stream" "$TMP/fetch.log"
if grep -F 'https://github.com/' "$TMP/fetch.log"; then exit 1; fi
if (download_asset absent.json 60 "$TMP/download" 2> "$TMP/error"); then exit 1; fi
grep -Fq 'release asset is missing or invalid' "$TMP/error"

cp "$TMP/release.json" "$TMP/good-release.json"
printf '%s\n' '{"tag_name":"bad","assets":[]}' > "$TMP/release.fixture"
if (release_metadata 2> "$TMP/error"); then exit 1; fi
grep -Fq 'GitHub release tag is invalid' "$TMP/error"
cp "$TMP/good-release.json" "$TMP/release.json"
FETCH_FAIL=1
if (download_asset manifest.json 60 "$TMP/download" 2> "$TMP/error"); then exit 1; fi
grep -Fq 'cannot download release asset: manifest.json' "$TMP/error"
if (release_metadata 2> "$TMP/error"); then exit 1; fi
grep -Fq 'cannot fetch GitHub release metadata' "$TMP/error"
FETCH_FAIL=0

BASE_URL=https://mirror.example/releases
RELEASE_TAG=
release_metadata
[ -z "$RELEASE_TAG" ]
download_asset manifest.json 60 "$TMP/download"
grep -Fxq "$BASE_URL/manifest.json " "$TMP/fetch.log"
printf 'PASS: version checks and bootstrap-independent GitHub API asset downloads with useful errors\n'

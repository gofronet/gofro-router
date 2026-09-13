#!/bin/sh
set -eu

APP_ROOT=/usr/lib/gofro
RELEASES=$APP_ROOT/releases
CURRENT=$APP_ROOT/current
BUNDLE="$(CDPATH='' cd "$(dirname "$0")" && pwd)"
ROOTFS=$BUNDLE/root
DEFAULTS=$BUNDLE/defaults
STAGING=
CURRENT_TMP=
STATUS_FILE=
LOCK=/tmp/gofro-install.lock
LOCKED=
PENDING=/etc/gofro/update-previous
ROLLBACK=
BACKUP=
PLATFORM=
RECOVER_INIT=${RECOVER_INIT:-/etc/init.d/gofro-recover}
RC_D=${RC_D:-/etc/rc.d}

die() {
	echo "error: $*" >&2
	exit 1
}

platform_for() {
	case "$1" in
		aarch64_*) echo aarch64-openwrt-linux-musl ;;
		arm_arm926ej-s|arm_xscale) echo armv5te-openwrt-linux-musleabi ;;
		arm_arm1176jzf-s_vfp) echo armv6-openwrt-linux-musleabihf ;;
		arm_cortex-*_neon*|arm_cortex-*_vfp*) echo armv7-openwrt-linux-musleabihf ;;
		arm_cortex-*) echo armv7-openwrt-linux-musleabi ;;
		i386_pentium-mmx|i386_pentium4) echo i586-openwrt-linux-musl ;;
		riscv64_generic) echo riscv64-openwrt-linux-musl ;;
		x86_64) echo x86_64-openwrt-linux-musl ;;
		*) return 1 ;;
	esac
}

enough_memory() {
	case "$1" in ''|*[!0-9]*) return 1 ;; esac
	[ "$1" -ge 196608 ]
}

# shellcheck disable=SC2317,SC2329
cleanup() {
	status=$?
	trap - EXIT HUP INT TERM
	set +e
	[ -z "$ROLLBACK" ] || rollback_update || echo 'error: update rollback failed; do not restart Gofro until recovery succeeds' >&2
	[ -z "$STAGING" ] || rm -rf "$STAGING"
	[ -z "$CURRENT_TMP" ] || rm -f "$CURRENT_TMP"
	rm -f "$STATUS_FILE"
	[ -z "$LOCKED" ] || rmdir "$LOCK"
	exit "$status"
}
trap cleanup EXIT
trap 'exit 1' HUP INT TERM

valid_version() {
	case "$1" in ''|*[!0-9.]*) return 1 ;; esac
	old_ifs=$IFS
	IFS=.
	# shellcheck disable=SC2086
	set -- $1
	IFS=$old_ifs
	[ "$#" = 3 ] && [ -n "$1" ] && [ -n "$2" ] && [ -n "$3" ]
}

valid_release() {
	case "$1" in "$RELEASES/"*) ;; *) return 1 ;; esac
	version=${1#"$RELEASES/"}
	valid_version "$version" && [ "$1" = "$RELEASES/$version" ] && [ -d "$1" ]
}

binary_version() {
	reported="$($1 --version 2>/dev/null)" || return 1
	reported=${reported##* }
	valid_version "$reported" || return 1
	echo "$reported"
}

switch_current() {
	CURRENT_TMP=$APP_ROOT/current.new.$$
	rm -f "$CURRENT_TMP"
	ln -s "$1" "$CURRENT_TMP" || return 1
	mv -fT "$CURRENT_TMP" "$CURRENT" || return 1
	sync || return 1
	CURRENT_TMP=
}

link_runtime() {
	for path in \
		usr/bin/gofro-agent \
		usr/bin/gofro-relay \
		usr/libexec/gofro/mode \
		usr/libexec/gofro/network \
		usr/libexec/gofro/guard \
		usr/libexec/gofro/onboarding \
		usr/libexec/gofro/service \
		usr/libexec/gofro/tunnel \
		usr/libexec/gofro/transaction \
		usr/libexec/gofro/update \
		usr/libexec/gofro/wifi \
		usr/sbin/gofro-setup \
		usr/sbin/gofro-update \
		usr/share/gofro/geosite.dat \
		usr/share/gofro/geoip.dat \
		usr/share/gofro/GEODATA-LICENSES.md \
		etc/init.d/gofro-recover \
		etc/init.d/gofro-guard \
		etc/init.d/gofro-onboarding \
		etc/init.d/gofro-agent \
		etc/init.d/gofro-relay \
		etc/init.d/gofro-updater \
		etc/init.d/gofro-finalize \
		etc/hotplug.d/iface/90-gofro-route
	do
		destination=/$path
		mkdir -p "/$(dirname "$path")"
		target=$CURRENT/$path
		case "$path" in usr/libexec/gofro/guard|etc/init.d/gofro-guard)
			# These must survive rollback to v15, which has neither guard file.
			target=$release/$path
			if [ -L "$destination" ]; then
				existing="$(readlink "$destination")"
				existing_release=${existing%/"$path"}
				if [ "$existing" = "$existing_release/$path" ] && valid_release "$existing_release"; then ln -sf "$target" "$destination"; continue; fi
			fi ;;
		esac
		if [ -L "$destination" ] && [ "$(readlink "$destination")" = "$target" ]; then
			continue
		fi
		if [ -e "$destination" ] || [ -L "$destination" ]; then
			die "$destination already exists"
		fi
		ln -s "$target" "$destination"
	done
}

copy_default() {
	[ -e "$2" ] && return 0
	mkdir -p "$(dirname "$2")"
	cp "$DEFAULTS/$1" "$2"
	chmod "$3" "$2"
}

write_version() {
	printf '%s\n' "$1" > /etc/gofro/version.new || return 1
	chmod 644 /etc/gofro/version.new || return 1
	mv -f /etc/gofro/version.new /etc/gofro/version || return 1
	sync
}

write_pending() {
	rm -f /etc/gofro/update-restored || return 1
	printf '%s\n%s\n' "$1" "$2" > "$PENDING.new" || return 1
	chmod 600 "$PENDING.new" || return 1
	mv -f "$PENDING.new" "$PENDING" || return 1
	sync
}

clear_pending() {
	rm -f "$PENDING" || return 1
	sync
}

read_pending() {
	{ IFS= read -r pending; IFS= read -r pending_backup; } < "$PENDING" || return 1
}

migrate_legacy_dns() {
	[ "${GOFRO_LEGACY_DNS:-0}" = 1 ] || return 0
	uci -q del_list dhcp.@dnsmasq[0].server='127.0.0.1#5353' || return 1
	for alias in /gofrowifi.net/10.203.1.1 /wifi.gofro.net/10.203.1.1; do
		case " $(uci -q get dhcp.@dnsmasq[0].address || true) " in
			*" $alias "*) uci -q del_list "dhcp.@dnsmasq[0].address=$alias" || return 1 ;;
		esac
	done
	uci -q delete dhcp.@dnsmasq[0].noresolv || return 1
	uci -q delete dhcp.@dnsmasq[0].localuse || return 1
	uci commit dhcp || return 1
	/etc/init.d/dnsmasq restart
}

preflight_legacy_dns() {
	[ "$(uci -q get gofro.main.interface || echo gt0)" = gt0 ] || die 'Gofro only owns the gt0 interface'
	server="$(uci -q get dhcp.@dnsmasq[0].server 2>/dev/null || true)"
	case "$server" in *127.0.0.1#5353*) ;; *) GOFRO_LEGACY_DNS=0; return 0 ;; esac
	[ "$server" = '127.0.0.1#5353' ] || die 'legacy DNS is ambiguous; repair it in OpenWrt before updating'
	# Only the complete v15 tuple is attributable to Gofro; custom DNS is operator-owned.
	[ "$(uci -q get dhcp.@dnsmasq[0].noresolv 2>/dev/null || true)" = 1 ] || die 'legacy DNS is ambiguous; repair it in OpenWrt before updating'
	[ "$(uci -q get dhcp.@dnsmasq[0].localuse 2>/dev/null || true)" = 0 ] || die 'legacy DNS is ambiguous; repair it in OpenWrt before updating'
	GOFRO_LEGACY_DNS=1
}

preflight_routing_history() {
	unset GOFRO_LEGACY_DEVICE GOFRO_LEGACY_SUBNET
	if [ "$previous" != "$RELEASES/0.5.15" ]; then
		[ ! -e /etc/gofro/routing-legacy.json ] || die 'legacy routing history has no proven v0.5.15 installation'
		return 0
	fi
	[ "$(cat /etc/gofro/version)" = 0.5.15 ] || die 'legacy installed version is not proven'
	for path in usr/bin/gofro-agent usr/bin/gofro-relay usr/libexec/gofro/mode; do
		[ "$(readlink "/$path")" = "$CURRENT/$path" ] || die 'legacy runtime ownership is not proven'
	done
	[ "$(binary_version "$previous/usr/bin/gofro-agent")" = 0.5.15 ] || die 'legacy agent version is not proven'
	[ "$(binary_version "$previous/usr/bin/gofro-relay")" = 0.5.15 ] || die 'legacy relay version is not proven'
	# Exact shipped v0.5.15 mode helper: its two UCI options are the old route context.
	digest="$(sha256sum "$previous/usr/libexec/gofro/mode")" || die 'cannot verify legacy routing helper'
	[ "${digest%% *}" = be3b5a3ab0d36ac77dabe531a4cc62090b5b7de30d53fa3d883681d88f629125 ] || die 'legacy routing helper is not the shipped v0.5.15 helper'
	changes="$(uci changes gofro)" || die 'cannot inspect pending Gofro UCI changes'
	[ -z "$changes" ] || die 'commit or revert pending Gofro UCI changes before updating'
	GOFRO_LEGACY_DEVICE="$(uci -q get gofro.main.lan_interface)" || die 'recorded legacy LAN device is missing'
	GOFRO_LEGACY_SUBNET="$(uci -q get gofro.main.lan_subnet)" || die 'recorded legacy LAN subnet is missing'
	case "$GOFRO_LEGACY_DEVICE" in ''|*[!A-Za-z0-9_.-]*|????????????????*) die 'recorded legacy LAN device is invalid' ;; esac
	case "$GOFRO_LEGACY_SUBNET" in ''|*[!0-9./]*|*/*/*|/*|*/) die 'recorded legacy LAN subnet is invalid' ;; esac
	printf '%s\n' "$GOFRO_LEGACY_SUBNET" | awk -F '[./]' '
		NF != 5 { exit 1 }
		{ for (i=1; i<=5; i++) if ($i !~ /^(0|[1-9][0-9]*)$/ || $i > (i==5 ? 32 : 255)) exit 1
		  ip=$1*16777216+$2*65536+$3*256+$4; if (ip % (2^(32-$5))) exit 1 }' || die 'recorded legacy LAN subnet is invalid'
	snapshot="$("$ROOTFS/usr/libexec/gofro/network")" || die 'cannot validate current LAN before migration'
	[ "$(printf '%s' "$snapshot" | jsonfilter -e '@.device')" = "$GOFRO_LEGACY_DEVICE" ] || die 'LAN device change requires maintenance; automatic migration is unsupported'
	export GOFRO_LEGACY_DEVICE GOFRO_LEGACY_SUBNET
}

rollback_update() {
	[ -n "$BACKUP" ] || return 1
	rm -f /etc/gofro/update-restored || return 1
	/etc/init.d/gofro-agent stop || return 1
	/etc/init.d/gofro-relay stop || return 1
	transaction restore "$BACKUP" || return 1
	/etc/init.d/firewall reload || return 1
	[ "$(cat "$BACKUP/legacy-dns")" != 1 ] || /etc/init.d/dnsmasq restart || return 1
	switch_current "$ROLLBACK" || return 1
	VERSION=${ROLLBACK##*/}
	write_version "$VERSION" || return 1
	restart_services || return 1; healthy || return 1
	# Health is not a runtime reconciliation witness, including after rollback to v15.
	# Only the current agent's successful full reconcile may clear gofro_guard.
	clear_pending || return 1; rm -rf "$BACKUP" || return 1
	ln -sf /etc/init.d/gofro-finalize "$RC_D/S99gofro-finalize" || return 1
	rm -f "$RC_D/S08gofro-recover" || return 1; "$RECOVER_INIT" disable || return 1; "$RECOVER_INIT" enable || return 1; sync
}

transaction() {
	if [ -n "${release:-}" ] && [ -x "$release/usr/libexec/gofro/transaction" ]; then
		"$release/usr/libexec/gofro/transaction" "$@"
	else
		/usr/libexec/gofro/transaction "$@"
	fi
}

configure_vpn_zone() {
	interface="$(uci -q get gofro.main.interface || echo gt0)"
	[ "$interface" = gt0 ] || return 1
	[ "$(uci -q get network.gt0.proto)" = wireguard ] || return 1
	[ "$(uci -q get firewall.gofro_vpn.name)" = gofro_vpn ] || return 1
	uci set "network.$interface.mtu=1280" || return 1
	uci set firewall.gofro_vpn.mtu_fix='1' || return 1
	uci set firewall.gofro_vpn.masq='1' || return 1
	uci commit network || return 1
	uci commit firewall || return 1
	/etc/init.d/firewall reload
}

restart_services() {
	/etc/init.d/gofro-guard enable || return 1
	/etc/init.d/gofro-agent disable || return 1
	/etc/init.d/gofro-agent enable || return 1
	/etc/init.d/gofro-relay restart || return 1
	/etc/init.d/gofro-agent restart
}

init_security() {
	chmod 700 /etc/gofro || return 1
	/etc/init.d/gofro-guard enable || return 1
	snapshot="$(/usr/libexec/gofro/guard prepare)" || return 1
	device="$(printf '%s' "$snapshot" | jsonfilter -e '@.device')" || return 1
	subnet="$(printf '%s' "$snapshot" | jsonfilter -e '@.subnet')" || return 1
	/usr/libexec/gofro/mode check "$device" "$subnet" || return 1
	address="$(printf '%s' "$snapshot" | jsonfilter -e '@.address')" || return 1
	fingerprint="$(/usr/bin/gofro-agent --https-listen "$address:8443" --init-security)" || return 1
	logger -t gofro "Gofro HTTPS certificate fingerprint: $fingerprint"
}

status_healthy() {
	[ "$(jsonfilter -i "$STATUS_FILE" -e '@.version' 2>/dev/null)" = "$VERSION" ] || return 1
	[ "$(jsonfilter -i "$STATUS_FILE" -e '@.dns_active' 2>/dev/null)" = true ] || return 1
	[ "$(jsonfilter -i "$STATUS_FILE" -e '@.dataplane_active' 2>/dev/null)" = true ] || return 1
	if [ "$VERSION" != 0.5.15 ]; then
		[ "$(jsonfilter -i "$STATUS_FILE" -e '@.degraded' 2>/dev/null)" = false ] || return 1
		[ ! -e /etc/gofro/routing-legacy.json ] || return 1
	fi
	tables="$(nft list tables)" || return 1
	if printf '%s\n' "$tables" | grep -Eq '^table inet gofro_guard[[:space:]]*$'; then
		echo 'warning: runtime guard retained; forwarding may be blocked even after restoring the old release' >&2
		return 1
	fi
	vpn_enabled="$(jsonfilter -i "$STATUS_FILE" -e '@.vpn_enabled' 2>/dev/null)"
	[ "$vpn_enabled" = false ] && return 0
	[ "$vpn_enabled" = true ] || return 1
	[ "$(jsonfilter -i "$STATUS_FILE" -e '@.tunnel_active' 2>/dev/null)" = true ] || return 1
	interface="$(uci -q get gofro.main.interface || echo gt0)"
	ip link show "$interface" | grep -q ' mtu 1280 ' || return 1
	handshake_age="$(jsonfilter -i "$STATUS_FILE" -e '@.handshake_age_seconds' 2>/dev/null)"
	case "$handshake_age" in ''|*[!0-9]*) return 1 ;; esac
	[ "$handshake_age" -le 180 ]
}

healthy() {
	count=0
	while [ "$count" -lt 30 ]; do
		if uclient-fetch -q -T 2 -O "$STATUS_FILE" 'http://127.0.0.1:8080/healthz' 2>/dev/null &&
			status_healthy &&
			{ [ ! -s /etc/gofro/relay-endpoint ] || /etc/init.d/gofro-relay running; }; then
			return 0
		fi
		count=$((count + 1))
		sleep 1
	done
	return 1
}

enough_space() {
	required="$(du -sk "$ROOTFS" | awk 'NR == 1 { print $1 }')"
	available="$(df -Pk /usr/lib | awk 'END { print $4 }')"
	case "$required:$available" in *[!0-9:]*) return 1 ;; esac
	[ "$available" -ge "$((required + 512))" ]
}

prune_releases() {
	[ -d "$RELEASES" ] || return 0
	guard_release="$(readlink /usr/libexec/gofro/guard 2>/dev/null || true)"
	guard_release=${guard_release%/usr/libexec/gofro/guard}
	for old_release in "$RELEASES"/*; do
		if [ "$old_release" = "$guard_release" ] || [ "$old_release" = "$1" ] || { [ -n "$2" ] && [ "$old_release" = "$2" ]; }; then
			continue
		fi
		rm -rf "$old_release"
	done
}

[ "$(id -u)" = 0 ] || die 'run as root'
[ -r /etc/openwrt_release ] || die 'OpenWrt is required'
# shellcheck disable=SC1091
. /etc/openwrt_release
case "${DISTRIB_RELEASE:-}" in 25.12.*) ;; *) die 'OpenWrt 25.12 is required' ;; esac
PLATFORM="$(platform_for "${DISTRIB_ARCH:-}")" || die "unsupported OpenWrt architecture: ${DISTRIB_ARCH:-unknown}"
MEMORY_KIB="$(awk '$1 == "MemTotal:" { print $2; exit }' /proc/meminfo)" || die 'router memory is unavailable'
enough_memory "$MEMORY_KIB" || die 'at least 192 MiB RAM must be visible to OpenWrt'

case "$#:${1:-}" in
	1:--update) mode=update ;;
	0:) mode=install; country= ;;
	1:[A-Z][A-Z]) mode=install; country=$1 ;;
	*) die 'usage: install.sh [COUNTRY] | install.sh --update' ;;
esac
if [ -e /etc/gofro/onboarding-state ]; then
	onboarding_state="$(cat /etc/gofro/onboarding-state)"
	case "$onboarding_state" in
		server) ;;
		admin)
			if [ "$mode" != install ] || { [ -e /etc/gofro/version ] && [ ! -s /etc/gofro/install-pending ]; }; then
				die 'setup is already open; use the existing console setup code'
			fi ;;
		*) die 'finish legacy onboarding manually before updating';;
	esac
fi
[ "$mode" != install ] || [ ! -e /etc/gofro/version ] || [ -s /etc/gofro/install-pending ] || die 'Gofro is already installed; run gofro-update'

IFS= read -r VERSION < "$BUNDLE/VERSION" || die 'bundle has no VERSION'
valid_version "$VERSION" || die 'bundle version is invalid'
IFS= read -r TARGET < "$BUNDLE/TARGET" || die 'bundle has no TARGET'
[ "$TARGET" = "$PLATFORM" ] || die 'bundle target does not match this router'
[ "$(binary_version "$ROOTFS/usr/bin/gofro-agent")" = "$VERSION" ] || die 'gofro-agent version mismatch'
[ "$(binary_version "$ROOTFS/usr/bin/gofro-relay")" = "$VERSION" ] || die 'gofro-relay version mismatch'
for path in \
	usr/sbin/gofro-update \
	usr/libexec/gofro/update \
	usr/libexec/gofro/network \
	usr/libexec/gofro/guard \
	usr/libexec/gofro/mode \
	usr/libexec/gofro/onboarding \
	usr/share/gofro/geosite.dat \
	usr/share/gofro/geoip.dat \
	etc/init.d/gofro-recover \
	etc/init.d/gofro-guard \
	etc/init.d/gofro-onboarding \
	etc/init.d/gofro-agent \
	etc/init.d/gofro-relay \
	etc/init.d/gofro-updater \
	etc/init.d/gofro-finalize \
	usr/libexec/gofro/transaction
do
	[ -f "$ROOTFS/$path" ] || die "bundle is missing $path"
done

mkdir "$LOCK" 2>/dev/null || die 'another installation or update is running'
LOCKED=1
STATUS_FILE="$(mktemp /tmp/gofro-status.XXXXXX)"
[ "$mode" != update ] || [ -s "$PENDING" ] || preflight_legacy_dns
rm -rf "$RELEASES"/.[0-9]* "$APP_ROOT"/current.new.*

previous=
if [ -L "$CURRENT" ]; then
	previous="$(readlink "$CURRENT")"
	valid_release "$previous" || die 'invalid current release link'
elif [ -e "$CURRENT" ]; then
	die 'current release is not a symlink'
fi

release=$RELEASES/$VERSION
pending=
if [ -s "$PENDING" ]; then
	[ "$mode" = update ] || die 'a pending update must be recovered before installing'
	read_pending || die 'pending update is invalid'
	valid_release "$pending" || die 'pending update is invalid'
	[ -d "$pending_backup" ] || die 'pending update is invalid'
	ROLLBACK=$pending
	BACKUP=$pending_backup
	rollback_update || die 'pending update restoration failed; do not restart Gofro until recovery succeeds'
	ROLLBACK=
	die 'pending update was rolled back; retry the update'
fi
[ "$mode" != update ] || preflight_routing_history
[ "$mode" != install ] || [ ! -e /etc/gofro/routing-legacy.json ] || die 'fresh installation cannot adopt legacy routing history'
prune_releases "$previous" "$pending"

if [ "$mode" = install ]; then
	apk update
	apk add ca-bundle dnsmasq firewall4 ip-full jsonfilter kmod-wireguard \
		openssl-util openssh-client openssh-client-utils openssh-keygen sshpass uclient-fetch uhttpd wireguard-tools
else
	[ -n "$previous" ] || die 'Gofro is not installed'
	apk update
	apk add openssh-client openssh-client-utils openssh-keygen sshpass
fi

[ "$previous" = "$release" ] || enough_space || die 'not enough persistent space for this release'

if [ "$previous" = "$release" ]; then
	if [ "$mode" = install ] && { [ ! -e /etc/gofro/version ] || [ -s /etc/gofro/install-pending ]; }; then
		link_runtime
		"$RECOVER_INIT" enable
		/etc/init.d/gofro-onboarding enable
		/etc/init.d/gofro-relay enable
		/etc/init.d/gofro-agent enable
		/etc/init.d/gofro-updater enable
		/etc/init.d/gofro-finalize enable
		/etc/init.d/gofro-updater start
		if [ "${onboarding_state:-}" = server ] || [ -e /etc/gofro/admin-password ]; then
			if [ ! -s /etc/gofro/install-pending ]; then
				printf '%s\n' "$VERSION" > /etc/gofro/install-pending.new
				chmod 600 /etc/gofro/install-pending.new
				mv -f /etc/gofro/install-pending.new /etc/gofro/install-pending
				sync
			fi
			if ! restart_services || ! healthy || ! /etc/init.d/gofro-finalize boot; then
				die "Gofro $VERSION failed its health check"
			fi
		else
		GOFRO_INSTALL_VERSION=$VERSION /usr/sbin/gofro-setup ${country:+"$country"}
		fi
		echo "Gofro $VERSION installation resumed"
		exit 0
	fi
	echo "Gofro $VERSION is already installed"
	exit 0
fi
[ "$release" != "${guard_release:-}" ] || die 'this release is pinned by the boot guard; recover it instead of replacing its files'
[ "$mode" != install ] || [ ! -s /etc/gofro/install-pending ] || die 'resume the pending installation using the same release'
if [ "$mode" = install ] && [ -n "$previous" ] && [ -e /etc/gofro/version ]; then
	die 'Gofro is already installed; run gofro-update'
fi

mkdir -p "$RELEASES" /etc/gofro
chmod 700 /etc/gofro
STAGING=$RELEASES/.$VERSION.$$
rm -rf "$STAGING"
mkdir "$STAGING"
cp -R "$ROOTFS/." "$STAGING/"
chmod 755 "$STAGING/usr/bin/gofro-agent" "$STAGING/usr/bin/gofro-relay" \
	"$STAGING/usr/sbin/gofro-setup" "$STAGING/usr/sbin/gofro-update" \
	"$STAGING/usr/libexec/gofro/"* "$STAGING/etc/init.d/"* \
	"$STAGING/etc/hotplug.d/iface/"*
rm -rf "$release"
mv "$STAGING" "$release"
STAGING=

copy_default etc/config/gofro /etc/config/gofro 600
copy_default etc/gofro/controller.json /etc/gofro/controller.json 600
if [ ! -e /etc/gofro/update-public.pem ]; then
	cp "$BUNDLE/update-public.pem" /etc/gofro/update-public.pem
	chmod 644 /etc/gofro/update-public.pem
fi

if [ "$mode" = install ]; then
	switch_current "$release"
	link_runtime
	"$RECOVER_INIT" enable
	/etc/init.d/gofro-onboarding enable
	/etc/init.d/gofro-relay enable
	/etc/init.d/gofro-agent enable
	/etc/init.d/gofro-updater enable
	/etc/init.d/gofro-finalize enable
	/etc/init.d/gofro-updater start
	GOFRO_INSTALL_VERSION=$VERSION /usr/sbin/gofro-setup ${country:+"$country"}
	exit 0
fi

BACKUP=/etc/gofro/update-$VERSION
GOFRO_INTERFACE="$(uci -q get gofro.main.interface || echo gt0)" GOFRO_LEGACY_DNS=${GOFRO_LEGACY_DNS:-0} \
	transaction snapshot "$BACKUP" || die 'cannot snapshot update state'
ROLLBACK=$previous
ln -sf "$release/etc/init.d/gofro-recover" "$RC_D/S08gofro-recover"
ln -sf "$release/etc/init.d/gofro-finalize" "$RC_D/S99gofro-finalize"
sync
rm -f "$RC_D/S89gofro-recover"
write_pending "$previous" "$BACKUP"
link_runtime
/etc/init.d/gofro-agent stop || die 'cannot stop Gofro agent; update activation aborted'
/etc/init.d/gofro-relay stop || die 'cannot stop Gofro relay; update activation aborted'
# Pre-init must guard the attested OLD device; only full agent reconcile retires history.
transaction attest "$BACKUP" || die 'cannot publish proven legacy routing history'
switch_current "$release"
if init_security && migrate_legacy_dns && configure_vpn_zone && restart_services && healthy; then
	write_version "$VERSION"
	clear_pending
	ROLLBACK=
	rm -rf "$BACKUP"
	prune_releases "$release" ''
	echo "Gofro updated to $VERSION"
	exit 0
fi

die "Gofro $VERSION failed its health check"

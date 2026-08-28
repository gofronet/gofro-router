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
PANEL_BACKUP=/etc/gofro/update-uhttpd
ROLLBACK=
PLATFORM=

die() {
	echo "error: $*" >&2
	exit 1
}

platform_for() {
	case "$1:$2" in
		mediatek/filogic:cudy,tr3000-256mb-v1) echo aarch64-openwrt-linux-musl ;;
		*) return 1 ;;
	esac
}

# shellcheck disable=SC2317,SC2329
cleanup() {
	status=$?
	trap - EXIT HUP INT TERM
	set +e
	if [ -n "$ROLLBACK" ] && restore_panel && switch_current "$ROLLBACK"; then
		if restart_services && write_version "${ROLLBACK##*/}"; then
			clear_pending
			[ -z "${release:-}" ] || [ "$release" = "$ROLLBACK" ] || rm -rf "$release"
		fi
	fi
	[ -z "$STAGING" ] || rm -rf "$STAGING"
	[ -z "$CURRENT_TMP" ] || rm -f "$CURRENT_TMP"
	rm -f "$PANEL_BACKUP.new"
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
		usr/libexec/gofro/service \
		usr/libexec/gofro/tunnel \
		usr/libexec/gofro/update \
		usr/libexec/gofro/wifi \
		usr/sbin/gofro-setup \
		usr/sbin/gofro-update \
		usr/share/gofro/geosite.dat \
		usr/share/gofro/geoip.dat \
		usr/share/gofro/GEODATA-LICENSES.md \
		etc/init.d/gofro-recover \
		etc/init.d/gofro-agent \
		etc/init.d/gofro-relay \
		etc/init.d/gofro-updater \
		etc/init.d/gofro-finalize \
		etc/hotplug.d/iface/90-gofro-route
	do
		destination=/$path
		mkdir -p "/$(dirname "$path")"
		if [ -L "$destination" ] && [ "$(readlink "$destination")" = "$CURRENT/$path" ]; then
			continue
		fi
		if [ -e "$destination" ] || [ -L "$destination" ]; then
			die "$destination already exists"
		fi
		ln -s "$CURRENT/$path" "$destination"
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
	printf '%s\n' "$1" > "$PENDING.new" || return 1
	chmod 600 "$PENDING.new" || return 1
	mv -f "$PENDING.new" "$PENDING" || return 1
	sync
}

clear_pending() {
	rm -f "$PENDING" || return 1
	sync
}

backup_panel() {
	[ ! -s "$PANEL_BACKUP" ] || return 0
	uci export uhttpd > "$PANEL_BACKUP.new" || return 1
	chmod 600 "$PANEL_BACKUP.new" || return 1
	mv -f "$PANEL_BACKUP.new" "$PANEL_BACKUP" || return 1
	sync
}

# shellcheck disable=SC2317,SC2329
restore_panel() {
	[ -s "$PANEL_BACKUP" ] || return 0
	uci import uhttpd < "$PANEL_BACKUP" || return 1
	uci commit uhttpd || return 1
	/etc/init.d/uhttpd restart || return 1
	rm -f "$PANEL_BACKUP" || return 1
	sync
}

clear_panel_backup() {
	rm -f "$PANEL_BACKUP" || return 1
	sync
}

configure_panel() {
	uci -q del_list dhcp.@dnsmasq[0].address='/wifi.gofro.net/10.203.1.1' || true
	uci add_list dhcp.@dnsmasq[0].address='/wifi.gofro.net/10.203.1.1' || return 1
	uci -q delete uhttpd.main.listen_http || true
	uci add_list uhttpd.main.listen_http='10.203.1.1:81' || return 1
	uci -q delete uhttpd.main.listen_https || true
	uci add_list uhttpd.main.listen_https='10.203.1.1:444' || return 1
	uci commit uhttpd || return 1
	/etc/init.d/uhttpd restart || return 1
	uci commit dhcp || return 1
	/etc/init.d/dnsmasq restart
}

restart_services() {
	/etc/init.d/gofro-relay restart || return 1
	/etc/init.d/gofro-agent restart
}

status_healthy() {
	[ "$(jsonfilter -i "$STATUS_FILE" -e '@.version' 2>/dev/null)" = "$VERSION" ] || return 1
	[ "$(jsonfilter -i "$STATUS_FILE" -e '@.routing.dns_active' 2>/dev/null)" = true ] || return 1
	[ "$(jsonfilter -i "$STATUS_FILE" -e '@.routing.dataplane_active' 2>/dev/null)" = true ] || return 1
	vpn_enabled="$(jsonfilter -i "$STATUS_FILE" -e '@.vpn_enabled' 2>/dev/null)"
	[ "$vpn_enabled" = false ] && return 0
	[ "$vpn_enabled" = true ] || return 1
	[ "$(jsonfilter -i "$STATUS_FILE" -e '@.tunnel_active' 2>/dev/null)" = true ] || return 1
	interface="$(uci -q get gofro.main.interface || echo gt0)"
	ip link show "$interface" | grep -q ' mtu 1280 ' || return 1
	handshake_age="$(jsonfilter -i "$STATUS_FILE" -e '@.peer.handshake_age_seconds' 2>/dev/null)"
	case "$handshake_age" in ''|*[!0-9]*) return 1 ;; esac
	[ "$handshake_age" -le 180 ]
}

healthy() {
	listen="$(uci -q get gofro.main.listen || echo 10.203.1.1:8080)"
	count=0
	while [ "$count" -lt 30 ]; do
		if uclient-fetch -q -T 2 -O "$STATUS_FILE" "http://$listen/api/status" 2>/dev/null &&
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
	available="$(df -Pk /overlay | awk 'END { print $4 }')"
	case "$required:$available" in *[!0-9:]*) return 1 ;; esac
	[ "$available" -ge "$((required + 512))" ]
}

prune_releases() {
	[ -d "$RELEASES" ] || return 0
	for old_release in "$RELEASES"/*; do
		if [ "$old_release" = "$1" ] || { [ -n "$2" ] && [ "$old_release" = "$2" ]; }; then
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
IFS= read -r board < /tmp/sysinfo/board_name || die 'router model is unavailable'
PLATFORM="$(platform_for "${DISTRIB_TARGET:-}" "$board")" || die "unsupported router: $board"

case "${1:-}" in
	--update) mode=update ;;
	[A-Z][A-Z]) mode=install; country=$1 ;;
	*) die 'usage: install.sh COUNTRY | install.sh --update' ;;
esac

IFS= read -r VERSION < "$BUNDLE/VERSION" || die 'bundle has no VERSION'
valid_version "$VERSION" || die 'bundle version is invalid'
IFS= read -r TARGET < "$BUNDLE/TARGET" || die 'bundle has no TARGET'
[ "$TARGET" = "$PLATFORM" ] || die 'bundle target does not match this router'
[ "$(binary_version "$ROOTFS/usr/bin/gofro-agent")" = "$VERSION" ] || die 'gofro-agent version mismatch'
[ "$(binary_version "$ROOTFS/usr/bin/gofro-relay")" = "$VERSION" ] || die 'gofro-relay version mismatch'
for path in \
	usr/sbin/gofro-update \
	usr/libexec/gofro/update \
	usr/share/gofro/geosite.dat \
	usr/share/gofro/geoip.dat \
	etc/init.d/gofro-recover \
	etc/init.d/gofro-agent \
	etc/init.d/gofro-relay \
	etc/init.d/gofro-updater \
	etc/init.d/gofro-finalize
do
	[ -f "$ROOTFS/$path" ] || die "bundle is missing $path"
done

mkdir "$LOCK" 2>/dev/null || die 'another installation or update is running'
LOCKED=1
STATUS_FILE="$(mktemp /tmp/gofro-status.XXXXXX)"
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
	IFS= read -r pending < "$PENDING" || die 'pending update is invalid'
	valid_release "$pending" || die 'pending update is invalid'
	[ "$previous" = "$release" ] || die 'a pending update must be recovered before installing another version'
fi
prune_releases "$previous" "$pending"

if [ "$mode" = install ]; then
	apk update
	apk add ca-bundle dnsmasq firewall4 ip-full iw jsonfilter kmod-wireguard \
		openssl-util uclient-fetch wireguard-tools
else
	[ -n "$previous" ] || die 'Gofro is not installed'
fi

[ "$previous" = "$release" ] || enough_space || die 'not enough free overlay space for this release'

if [ "$previous" = "$release" ]; then
	if [ "$mode" = update ] && [ -n "$pending" ]; then
		ROLLBACK=$pending
		if backup_panel && configure_panel && restart_services && healthy; then
			write_version "$VERSION"
			clear_pending
			ROLLBACK=
			clear_panel_backup
			echo "Gofro recovered update to $VERSION"
			exit 0
		fi
		die "Gofro $VERSION failed its health check"
	fi
	if [ "$mode" = install ] && [ ! -e /etc/gofro/version ]; then
		link_runtime
		/etc/init.d/gofro-recover enable
		/etc/init.d/gofro-relay enable
		/etc/init.d/gofro-agent enable
		/etc/init.d/gofro-updater enable
		/etc/init.d/gofro-finalize enable
		/etc/init.d/gofro-updater start
		GOFRO_INSTALL_VERSION=$VERSION /usr/sbin/gofro-setup "$country"
		echo "Gofro $VERSION installation resumed"
		exit 0
	fi
	echo "Gofro $VERSION is already installed"
	exit 0
fi
if [ "$mode" = install ] && [ -n "$previous" ] && [ -e /etc/gofro/version ]; then
	die 'Gofro is already installed; run gofro-update'
fi

mkdir -p "$RELEASES" /etc/gofro
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
	/etc/init.d/gofro-recover enable
	/etc/init.d/gofro-relay enable
	/etc/init.d/gofro-agent enable
	/etc/init.d/gofro-updater enable
	/etc/init.d/gofro-finalize enable
	/etc/init.d/gofro-updater start
	GOFRO_INSTALL_VERSION=$VERSION /usr/sbin/gofro-setup "$country"
	exit 0
fi

ROLLBACK=$previous
write_pending "$previous"
backup_panel || die 'failed to back up panel configuration'
configure_panel || die 'failed to configure panel address'
link_runtime
/etc/init.d/gofro-agent stop || true
/etc/init.d/gofro-relay stop || true
switch_current "$release"
interface="$(uci -q get gofro.main.interface || echo gt0)"
uci set "network.$interface.mtu=1280"
uci commit network
if restart_services && healthy; then
	write_version "$VERSION"
	clear_pending
	ROLLBACK=
	clear_panel_backup
	prune_releases "$release" ''
	echo "Gofro updated to $VERSION"
	exit 0
fi

die "Gofro $VERSION failed its health check"

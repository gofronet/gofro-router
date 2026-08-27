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
TARGET=aarch64-raspios-linux-musl

die() {
	echo "error: $*" >&2
	exit 1
}

# shellcheck disable=SC2317,SC2329
cleanup() {
	status=$?
	trap - EXIT HUP INT TERM
	set +e
	if [ -n "$ROLLBACK" ] && switch_current "$ROLLBACK"; then
		if restart_services && write_version "${ROLLBACK##*/}"; then
			clear_pending
			[ -z "${release:-}" ] || [ "$release" = "$ROLLBACK" ] || rm -rf "$release"
		fi
	fi
	[ -z "$STAGING" ] || rm -rf "$STAGING"
	[ -z "$CURRENT_TMP" ] || rm -f "$CURRENT_TMP"
	[ -z "$STATUS_FILE" ] || rm -f "$STATUS_FILE"
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

platform_for() {
	case "$1:$2:$3:$4" in
		aarch64:debian:trixie:Raspberry\ Pi\ 5\ Model*|aarch64:raspbian:trixie:Raspberry\ Pi\ 5\ Model*) echo "$TARGET" ;;
		*) return 1 ;;
	esac
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
		usr/libexec/gofro/recover \
		usr/libexec/gofro/service \
		usr/libexec/gofro/tunnel \
		usr/libexec/gofro/update \
		usr/libexec/gofro/wifi \
		usr/sbin/gofro-setup \
		usr/sbin/gofro-update \
		usr/share/gofro/geosite.dat \
		usr/share/gofro/geoip.dat \
		usr/share/gofro/GEODATA-LICENSES.md
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

install_units() {
	for unit in "$ROOTFS"/etc/systemd/system/gofro-*.service; do
		install -m 644 "$unit" "/etc/systemd/system/${unit##*/}"
	done
	systemctl daemon-reload
}

verify_units() {
	for unit in "$ROOTFS"/etc/systemd/system/gofro-*.service; do
		installed=/etc/systemd/system/${unit##*/}
		if [ ! -f "$installed" ] || ! cmp "$unit" "$installed"; then
			die "systemd unit ${unit##*/} requires a fresh install"
		fi
	done
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

configure_panel_domain() {
	cat > /etc/dnsmasq.d/gofro-domain.conf.new <<'EOF'
address=/wifi.gofro.net/10.203.1.1
local=/wifi.gofro.net/
EOF
	mv -f /etc/dnsmasq.d/gofro-domain.conf.new /etc/dnsmasq.d/gofro-domain.conf || return 1
	systemctl restart dnsmasq.service
}

restart_services() {
	systemctl restart gofro-network.service || return 1
	systemctl restart dnsmasq.service || return 1
	systemctl restart gofro-relay.service || return 1
	systemctl restart gofro-agent.service
}

status_healthy() {
	[ "$(jq -r '.version' "$STATUS_FILE" 2>/dev/null)" = "$VERSION" ] || return 1
	[ "$(jq -r '.routing.dns_active' "$STATUS_FILE" 2>/dev/null)" = true ] || return 1
	[ "$(jq -r '.routing.dataplane_active' "$STATUS_FILE" 2>/dev/null)" = true ] || return 1
	vpn_enabled="$(jq -r '.vpn_enabled' "$STATUS_FILE" 2>/dev/null)"
	[ "$vpn_enabled" = false ] && return 0
	[ "$vpn_enabled" = true ] || return 1
	[ "$(jq -r '.tunnel_active' "$STATUS_FILE" 2>/dev/null)" = true ] || return 1
	handshake_age="$(jq -r '.peer.handshake_age_seconds' "$STATUS_FILE" 2>/dev/null)"
	case "$handshake_age" in ''|*[!0-9]*) return 1 ;; esac
	[ "$handshake_age" -le 180 ]
}

healthy() {
	count=0
	while [ "$count" -lt 30 ]; do
		if systemctl is-active --quiet dnsmasq.service && \
			nmcli --terse --fields NAME connection show --active | grep -Fxq gofro-ap && \
			[ "$(dig +short +time=1 +tries=1 @127.0.0.1 gofrowifi.net A)" = 10.203.1.1 ] && \
			curl --fail --silent --show-error --max-time 2 -o "$STATUS_FILE" http://10.203.1.1/api/status 2>/dev/null && status_healthy && \
			{ [ ! -s /etc/gofro/relay-endpoint ] || systemctl is-active --quiet gofro-relay.service; }; then
			return 0
		fi
		count=$((count + 1))
		sleep 1
	done
	return 1
}

enough_space() {
	required="$(du -sk "$ROOTFS" | awk 'NR == 1 { print $1 }')"
	available="$(df -Pk / | awk 'END { print $4 }')"
	case "$required:$available" in *[!0-9:]*) return 1 ;; esac
	[ "$available" -ge "$((required + 1024))" ]
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
[ -r /etc/os-release ] || die 'Raspberry Pi OS is required'
# shellcheck disable=SC1091
. /etc/os-release
model="$(tr -d '\000' < /proc/device-tree/model 2>/dev/null || true)"
PLATFORM="$(platform_for "$(uname -m)" "${ID:-}" "${VERSION_CODENAME:-}" "$model")" || die 'Raspberry Pi 5 with 64-bit Raspberry Pi OS Trixie is required'

case "${1:-}" in
	--update) mode=update ;;
	[A-Z][A-Z]) mode=install; country=$1 ;;
	*) die 'usage: install.sh COUNTRY | install.sh --update' ;;
esac

IFS= read -r VERSION < "$BUNDLE/VERSION" || die 'bundle has no VERSION'
valid_version "$VERSION" || die 'bundle version is invalid'
IFS= read -r bundle_target < "$BUNDLE/TARGET" || die 'bundle has no TARGET'
[ "$bundle_target" = "$PLATFORM" ] || die 'bundle target does not match this Raspberry Pi'
[ "$(binary_version "$ROOTFS/usr/bin/gofro-agent")" = "$VERSION" ] || die 'gofro-agent version mismatch'
[ "$(binary_version "$ROOTFS/usr/bin/gofro-relay")" = "$VERSION" ] || die 'gofro-relay version mismatch'
for path in \
	usr/sbin/gofro-setup \
	usr/sbin/gofro-update \
	usr/libexec/gofro/mode \
	usr/libexec/gofro/network \
	usr/libexec/gofro/recover \
	usr/libexec/gofro/service \
	usr/libexec/gofro/tunnel \
	usr/libexec/gofro/update \
	usr/libexec/gofro/wifi \
	usr/share/gofro/geosite.dat \
	usr/share/gofro/geoip.dat \
	etc/systemd/system/gofro-agent.service \
	etc/systemd/system/gofro-network.service \
	etc/systemd/system/gofro-recover.service \
	etc/systemd/system/gofro-relay.service \
	etc/systemd/system/gofro-updater.service
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

pending=
if [ -s "$PENDING" ]; then
	[ "$mode" = update ] || die 'a pending update must be recovered before installing'
	IFS= read -r pending < "$PENDING" || die 'pending update is invalid'
	valid_release "$pending" || die 'pending update is invalid'
fi

if [ "$mode" = install ]; then
	apt-get update
	DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
		ca-certificates curl dnsmasq dnsutils iproute2 iw jq network-manager nftables \
		openssl rfkill wireguard-tools
	systemctl is-active --quiet NetworkManager.service || die 'NetworkManager is not active'
	default_interface="$(ip -4 route get 1.1.1.1 | sed -n 's/.* dev \([^ ]*\).*/\1/p' | head -n 1)"
	[ "$default_interface" = eth0 ] || die 'Ethernet eth0 must be the active uplink'
else
	[ -n "$previous" ] || die 'Gofro is not installed'
	verify_units
fi

release=$RELEASES/$VERSION
[ -z "$pending" ] || [ "$previous" = "$release" ] || die 'a pending update must be recovered before installing another version'
prune_releases "$previous" "$pending"
[ "$previous" = "$release" ] || enough_space || die 'not enough free space for this release'

if [ "$previous" = "$release" ]; then
	if [ "$mode" = update ] && [ -n "$pending" ]; then
		ROLLBACK=$pending
		if restart_services && healthy && configure_panel_domain; then
			write_version "$VERSION"
			clear_pending
			ROLLBACK=
			prune_releases "$release" ''
			echo "Gofro recovered update to $VERSION"
			exit 0
		fi
		die "Gofro $VERSION failed its health check"
	fi
	if [ "$mode" = install ] && [ ! -e /etc/gofro/version ]; then
		link_runtime
		install_units
		/usr/sbin/gofro-setup "$country"
		healthy || die "Gofro $VERSION failed its health check"
		write_version "$VERSION"
		echo "Gofro $VERSION installation resumed"
		exit 0
	fi
	echo "Gofro $VERSION is already installed"
	exit 0
fi
if [ "$mode" = install ] && [ -n "$previous" ] && [ -e /etc/gofro/version ]; then
	die 'Gofro is already installed; run gofro-update'
fi

mkdir -p "$RELEASES" /etc/gofro /var/lib/gofro
STAGING=$RELEASES/.$VERSION.$$
rm -rf "$STAGING"
mkdir "$STAGING"
cp -R "$ROOTFS/." "$STAGING/"
chmod 755 "$STAGING/usr/bin/gofro-agent" "$STAGING/usr/bin/gofro-relay" \
	"$STAGING/usr/sbin/"* "$STAGING/usr/libexec/gofro/"*
rm -rf "$release"
mv "$STAGING" "$release"
STAGING=

copy_default etc/gofro/controller.json /etc/gofro/controller.json 600
if [ ! -e /etc/gofro/update-public.pem ]; then
	cp "$BUNDLE/update-public.pem" /etc/gofro/update-public.pem
	chmod 644 /etc/gofro/update-public.pem
fi

if [ "$mode" = install ]; then
	switch_current "$release"
	link_runtime
	install_units
	/usr/sbin/gofro-setup "$country"
	healthy || die "Gofro $VERSION failed its health check"
	write_version "$VERSION"
	echo "Gofro $VERSION installed"
	exit 0
fi

ROLLBACK=$previous
write_pending "$previous"
systemctl stop gofro-agent.service || true
systemctl stop gofro-relay.service || true
switch_current "$release"
if restart_services && healthy && configure_panel_domain; then
	write_version "$VERSION"
	clear_pending
	ROLLBACK=
	prune_releases "$release" ''
	echo "Gofro updated to $VERSION"
	exit 0
fi

die "Gofro $VERSION failed its health check"

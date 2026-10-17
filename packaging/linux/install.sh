#!/bin/sh
# Install (or upgrade) LoupeCam as a systemd service on Linux, e.g. a Raspberry Pi.
#
#   curl -fsSL https://raw.githubusercontent.com/cinderblock/loupecam/master/packaging/linux/install.sh | sudo sh
#   sudo sh install.sh [--version v0.2.0] [--binary ./loupecam] [--listen 0.0.0.0:8080]
#                      [--kiosk [--rotate 180] [--no-browser]]
#
# Downloads the release binary for this machine (checked against the release's
# SHA256SUMS), installs it to /opt/loupecam, adds the camera udev rule, a `loupecam`
# system user and the loupecam.service unit, and starts it. Re-running upgrades the
# binary and unit; /etc/loupecam/loupecam.env (listen address, token) is kept.
#
# --kiosk also runs the touchscreen UI on the attached display (loupecam-kiosk.service,
# on tty1 instead of the console login), installing cage and Chromium for casts it
# can't show natively unless --no-browser. Its settings: /etc/loupecam/kiosk.env.
set -eu

REPO=cinderblock/loupecam
RAW="https://raw.githubusercontent.com/$REPO/master/packaging/linux"
PREFIX=/opt/loupecam
ENV_FILE=/etc/loupecam/loupecam.env
KIOSK_ENV=/etc/loupecam/kiosk.env

version=latest
binary=
listen=0.0.0.0:80
kiosk=0
rotate=0
browser=1

usage() { sed -n '2,15s/^# \{0,1\}//p' "$0"; exit "${1:-0}"; }

while [ $# -gt 0 ]; do
	case "$1" in
	--version) version=$2; shift 2 ;;
	--binary) binary=$2; shift 2 ;;
	--listen) listen=$2; shift 2 ;;
	--kiosk) kiosk=1; shift ;;
	--rotate) rotate=$2; shift 2 ;;
	--no-browser) browser=0; shift ;;
	-h | --help) usage ;;
	*) echo "unknown option: $1" >&2; usage 2 >&2 ;;
	esac
done

[ "$(id -u)" -eq 0 ] || { echo "run as root (sudo)" >&2; exit 1; }
command -v systemctl >/dev/null || { echo "systemd is required" >&2; exit 1; }

case "$(uname -m)" in
aarch64 | arm64) target=aarch64-unknown-linux-musl ;;
x86_64 | amd64) target=x86_64-unknown-linux-musl ;;
*) echo "no LoupeCam build for $(uname -m)" >&2; exit 1 ;;
esac

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

fetch() { curl -fsSL --retry 3 -o "$2" "$1"; }

# Packaging files: next to this script when run from a checkout, else from GitHub.
case "$0" in
*install.sh) here=$(dirname "$0") ;;
*) here=/nonexistent ;; # piped into sh
esac
packaged() {
	if [ -f "$here/$1" ]; then cp "$here/$1" "$tmp/$1"; else fetch "$RAW/$1" "$tmp/$1"; fi
}
packaged loupecam.service
packaged 99-loupecam.rules
packaged 99-loupecam-usb.rules
if [ "$kiosk" = 1 ]; then
	packaged loupecam-kiosk.service
	packaged 99-loupecam-kiosk.rules
fi

if [ -n "$binary" ]; then
	cp "$binary" "$tmp/loupecam"
else
	if [ "$version" = latest ]; then
		base="https://github.com/$REPO/releases/latest/download"
	else
		base="https://github.com/$REPO/releases/download/$version"
	fi
	asset="loupecam-$target"
	echo "Downloading $asset ($version)"
	fetch "$base/$asset" "$tmp/$asset"
	fetch "$base/SHA256SUMS" "$tmp/SHA256SUMS"
	(cd "$tmp" && grep " \*\{0,1\}$asset\$" SHA256SUMS | sha256sum -c -) || {
		echo "checksum mismatch for $asset" >&2
		exit 1
	}
	mv "$tmp/$asset" "$tmp/loupecam"
fi
chmod 755 "$tmp/loupecam"
"$tmp/loupecam" --version

# The camera is reached through the plugdev group (see 99-loupecam.rules).
getent group plugdev >/dev/null || groupadd --system plugdev
if ! id loupecam >/dev/null 2>&1; then
	useradd --system --user-group --groups plugdev --home-dir /var/lib/loupecam \
		--no-create-home --shell /usr/sbin/nologin --comment LoupeCam loupecam
fi

# The service owns its install directory so it can update itself in place.
install -d -o loupecam -g loupecam -m 755 "$PREFIX"
install -o loupecam -g loupecam -m 755 "$tmp/loupecam" "$PREFIX/loupecam.new"
mv -f "$PREFIX/loupecam.new" "$PREFIX/loupecam"
ln -sf "$PREFIX/loupecam" /usr/local/bin/loupecam

install -m 644 "$tmp/99-loupecam.rules" /etc/udev/rules.d/99-loupecam.rules
# USB sticks for captures (see 99-loupecam-usb.rules).
install -d -m 755 /media/loupecam /media/loupecam/usb
install -m 644 "$tmp/99-loupecam-usb.rules" /etc/udev/rules.d/99-loupecam-usb.rules
udevadm control --reload
udevadm trigger --subsystem-match=usb --attr-match=idVendor=0547
udevadm trigger --subsystem-match=block --property-match=ID_BUS=usb --action=add

install -m 644 "$tmp/loupecam.service" /etc/systemd/system/loupecam.service

if [ ! -f "$ENV_FILE" ]; then
	install -d -m 755 /etc/loupecam
	token=$(od -An -tx1 -N16 /dev/urandom | tr -d ' \n')
	umask 027
	cat >"$ENV_FILE" <<CONF
# LoupeCam service settings. Apply with: sudo systemctl restart loupecam

# Address and port to serve on.
LOUPECAM_LISTEN=$listen

# Access token for the API, stream and web UI. Empty means anyone on the network
# can use the camera.
LOUPECAM_TOKEN=$token

# Extra arguments to \`loupecam serve\`, e.g. --mdns-name "Bench scope"
LOUPECAM_ARGS=
CONF
	chgrp loupecam "$ENV_FILE"
	echo "Created $ENV_FILE"
fi

systemctl daemon-reload
systemctl enable loupecam.service >/dev/null
systemctl restart loupecam.service

. "$ENV_FILE"
port=${LOUPECAM_LISTEN##*:}

if [ "$kiosk" = 1 ]; then
	if [ "$browser" = 1 ] && command -v apt-get >/dev/null; then
		echo "Installing cage and Chromium (for casting PDFs and web pages to the display)"
		apt-get install -y --no-install-recommends cage >/dev/null
		apt-get install -y chromium-browser >/dev/null 2>&1 || apt-get install -y chromium >/dev/null
	fi
	# The display (video, render) and the touchscreen (input).
	groups=
	for g in video render input; do
		getent group "$g" >/dev/null && groups="$groups${groups:+,}$g"
	done
	if ! id loupecam-kiosk >/dev/null 2>&1; then
		useradd --system --user-group ${groups:+--groups "$groups"} --home-dir /var/lib/loupecam-kiosk \
			--no-create-home --shell /usr/sbin/nologin --comment "LoupeCam kiosk" loupecam-kiosk
	fi
	install -m 644 "$tmp/99-loupecam-kiosk.rules" /etc/udev/rules.d/99-loupecam-kiosk.rules
	udevadm control --reload
	udevadm trigger --subsystem-match=backlight
	install -m 644 "$tmp/loupecam-kiosk.service" /etc/systemd/system/loupecam-kiosk.service
	if [ ! -f "$KIOSK_ENV" ]; then
		cat >"$KIOSK_ENV" <<CONF
# LoupeCam kiosk settings. Apply with: sudo systemctl restart loupecam-kiosk

# The server to show (this machine's).
LOUPECAM_SERVER=http://127.0.0.1:$port

# Rotate the UI for how the display is mounted: 0, 90, 180 or 270 (clockwise).
LOUPECAM_KIOSK_ROTATE=$rotate

# Turn the backlight off after this many minutes without a touch (0 = never).
LOUPECAM_KIOSK_SLEEP=0

# The resting screen: native (the fast touch UI) or browser (the full web UI).
LOUPECAM_KIOSK_HOME=native
CONF
		echo "Created $KIOSK_ENV"
	fi
	# The kiosk replaces the console login on tty1.
	systemctl disable getty@tty1.service >/dev/null 2>&1 || true
	systemctl daemon-reload
	systemctl enable loupecam-kiosk.service >/dev/null
	systemctl restart loupecam-kiosk.service
	echo "Kiosk running on the attached display. Logs: journalctl -u loupecam-kiosk -f"
fi

suffix=$([ "$port" = 80 ] && echo "" || echo ":$port")
echo
echo "LoupeCam is running: http://$(hostname).local$suffix/"
[ -z "${LOUPECAM_TOKEN:-}" ] || echo "Token: $LOUPECAM_TOKEN   (or open http://$(hostname).local$suffix/?token=$LOUPECAM_TOKEN)"
echo "Logs: journalctl -u loupecam -f    Settings: $ENV_FILE"

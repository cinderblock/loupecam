#!/bin/sh
# Install (or upgrade) LoupeCam as a systemd service on Linux, e.g. a Raspberry Pi.
#
#   curl -fsSL https://raw.githubusercontent.com/cinderblock/loupecam/master/packaging/linux/install.sh | sudo sh
#   sudo sh install.sh [--version v0.2.0] [--binary ./loupecam] [--listen 0.0.0.0:8080]
#
# Downloads the release binary for this machine (checked against the release's
# SHA256SUMS), installs it to /opt/loupecam, adds the camera udev rule, a `loupecam`
# system user and the loupecam.service unit, and starts it. Re-running upgrades the
# binary and unit; /etc/loupecam/loupecam.env (listen address, token) is kept.
set -eu

REPO=cinderblock/loupecam
RAW="https://raw.githubusercontent.com/$REPO/master/packaging/linux"
PREFIX=/opt/loupecam
ENV_FILE=/etc/loupecam/loupecam.env

version=latest
binary=
listen=0.0.0.0:80

usage() { sed -n '2,10s/^# \{0,1\}//p' "$0"; exit "${1:-0}"; }

while [ $# -gt 0 ]; do
	case "$1" in
	--version) version=$2; shift 2 ;;
	--binary) binary=$2; shift 2 ;;
	--listen) listen=$2; shift 2 ;;
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
udevadm control --reload
udevadm trigger --subsystem-match=usb --attr-match=idVendor=0547

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
suffix=$([ "$port" = 80 ] && echo "" || echo ":$port")
echo
echo "LoupeCam is running: http://$(hostname).local$suffix/"
[ -z "${LOUPECAM_TOKEN:-}" ] || echo "Token: $LOUPECAM_TOKEN   (or open http://$(hostname).local$suffix/?token=$LOUPECAM_TOKEN)"
echo "Logs: journalctl -u loupecam -f    Settings: $ENV_FILE"

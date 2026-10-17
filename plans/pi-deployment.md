# Raspberry Pi deployment

## Goal

Run `loupecam serve` permanently on a dedicated Raspberry Pi, so the microscope camera
is a network device. The install must be reproducible: the final Pi isn't set up yet, so
everything goes in the repo (`packaging/linux/`) and is tested first on a stand-in Pi.

## Environment / context

- Stand-in Pi: `ssh odriver2` (user `pi`, passwordless sudo). Pi 5, 4 GB, Debian 12
  bookworm, aarch64, kernel 6.12, NVMe root. Camera on USB3 bus 4 (`0547:1142`).
- odriver2 already runs `motion` on :8080/:8081 (Logitech StreamCam, unrelated),
  `odrive-websocket` (python, :8765) and avahi-daemon. **Port 80 is free.** Leave all
  of those alone.
- Release used: v0.2.0, asset `loupecam-aarch64-unknown-linux-musl` (built with
  `embed-ui`, so `--web-ui embedded` works).

## Decisions already made (don't re-ask)

- Deploy method is up to us ("deploy as you like"); port 80, or a random port if 80 is
  taken.
- Install from GitHub Releases rather than building on the Pi: it's the same binary the
  self-updater installs, and SHA256SUMS can verify it.

## Design

- System user `loupecam` (no login), in `plugdev` for the camera's udev rule.
- Binary at `/opt/loupecam/loupecam`, owned by `loupecam`, so self-update
  (`self_replace` renames within that directory, then `exec`s in place, same PID) works
  under systemd. `/usr/local/bin/loupecam` symlinks to it.
- State in `/var/lib/loupecam` (systemd `StateDirectory`): `XDG_CONFIG_HOME` points at
  `/var/lib/loupecam/config` (settings, calibration); captures in
  `/var/lib/loupecam/captures`.
- `/etc/loupecam/loupecam.env`: `LOUPECAM_LISTEN`, `LOUPECAM_TOKEN`, extra args. The
  installer creates it once and never overwrites it.
- Port 80 via `AmbientCapabilities=CAP_NET_BIND_SERVICE`; otherwise hardened unit.
- `Restart=always`. The server itself reconnects on camera unplug/replug.

## Plan / steps

1. Write `packaging/linux/loupecam.service` and `packaging/linux/install.sh`.
2. Run the installer on odriver2; verify service, web UI, stream, snapshot, mDNS
   (coexisting with avahi), reboot survival, replug.
3. Docs: README "Raspberry Pi / Linux service" section.
4. Commit.

## Findings / gotchas

- **USB power on the Pi 5 is the blocker.** With the service streaming, the camera
  trips USB over-current within ~10 s, repeatedly. All four ports reset at once
  (ODrive, StreamCam, mouse and camera all re-enumerate), from `dmesg`:
  `usb usb4-port1: over-current change #1` ... `usb 4-1: USB disconnect`. odriver2's
  supply doesn't do USB-PD (`/proc/device-tree/chosen/power/usbpd_power_data_objects`
  all zero, `max_current` = 0x384 = 900 mA), so the firmware keeps the 600 mA total USB
  limit (`usb_max_current_enable=0`). Claimed draw: AmScope `bMaxPower` 800 mA,
  StreamCam 896 mA (and `motion` streams from it continuously). Not throttled
  (`get_throttled=0x0`, EXT5V 5.20 V). The supply itself is fine; the port current
  limit is what trips.
  Fixes: the official 27 W (5 V 5 A) USB-C PSU, which lifts the limit to 1.6 A
  automatically; or `usb_max_current_enable=1` in `/boot/firmware/config.txt` if the
  supply really can deliver it; or a powered USB3 hub.
- The service otherwise works: started as `loupecam` on :80 via
  `CAP_NET_BIND_SERVICE`, camera opened inside the sandbox, token enforced (401 without
  it), `/api/stats` reported frames, and the server re-opened the camera on its own
  after each USB reset.
- **Fix applied on odriver2 (user-authorized 2026-10-16):** `usb_max_current_enable=1`
  appended to `/boot/firmware/config.txt` (backup `config.txt.bak-loupecam`), rebooted.
  Device tree then showed `usb_max_current_enable` = 1. Zero over-current events after
  that, both with `motion` stopped and with it running alongside (90 s).
- **But the supply can't carry full CPU load on top of that.** A 4-core `cargo build`
  on odriver2 (camera streaming, `motion` running) browned it out about 40 s in. The
  journal for that boot ends at 2026-10-16 23:13:49 with no shutdown, and the Pi
  rebooted by itself. For the final Pi, use the official 27 W supply rather than
  `usb_max_current_enable=1` on an unknown one. Build LoupeCam elsewhere: it
  cross-compiles from Windows (recipe in `plans/access-setup.md` on branch
  `access-setup`).
- `/var/lib/loupecam` is empty until the first settings change; a PATCH then wrote
  `/var/lib/loupecam/config/loupecam/settings.json` (owned by `loupecam`).
- Verified after the fix: starts at boot; `/snapshot.jpg`, `/stream.mjpg` (~1.1 MB/s);
  full-resolution capture, 4912x3684 PNG (48 MB) in 2.4 s, saved to
  `/var/lib/loupecam/captures`; `loupecam discover` finds it alongside avahi-daemon;
  inside the sandbox `/opt/loupecam` is writable and `/etc` read-only (so self-update
  can replace the binary); a simulated replug (`/sys/bus/usb/devices/4-1/authorized`
  0 then 1) recovered in ~4 s. While the camera is absent, nusb logs
  `ERROR nusb::error: interface not found (errno 2)` once a second: noisy but harmless.
- Web UI serves and shows its token prompt. Logging in with the token was left to the
  user (the agent doesn't type credentials into a non-localhost page).
- `loupecam discover` also lists `http://127.0.0.1:80/` among the URLs for a remote
  server. Cosmetic; not chased.

## Progress log

- [x] Unit + installer (`packaging/linux/loupecam.service`, `install.sh`)
- [x] Installed on odriver2 (v0.2.0, token generated). Running and enabled.
- [x] USB power fixed on odriver2 (`usb_max_current_enable=1`, rebooted)
- [x] Verified (stream, capture, settings, mDNS, reboot, replug, coexisting with motion)
- [x] README
- [x] Committed
- [ ] Final dedicated Pi: run the installer there (use a 27 W PSU or set
      `usb_max_current_enable=1`). Needs the commits pushed for the `curl | sh` form;
      otherwise copy `packaging/linux/` over and run `sudo sh install.sh`.

## Open questions for the user

1. Push to `origin/master`? Needed before the README's `curl | sh` one-liner works.

## Things not to do

- Don't touch `motion`, `odrive-websocket` or anything else on odriver2 — odriver2 is an
  idle ODrive rig lent for testing. The USB resets above already knocked its ODrive and
  StreamCam off the bus several times, which is why the USB current limit had to be raised before streaming.

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
- `/var/lib/loupecam` was still empty after the first run: settings are written only
  when something changes. Still to check: a PATCH lands in
  `/var/lib/loupecam/config/loupecam/settings.json`.

## Progress log

- [x] Unit + installer (`packaging/linux/loupecam.service`, `install.sh`)
- [x] Installed on odriver2 (v0.2.0, token generated). **Service stopped** (still
      enabled) because of the over-current resets, so it won't disrupt the ODrive rig.
- [ ] Verified (UI, stream, capture, mDNS, reboot, replug): blocked on USB power
- [ ] README
- [ ] Committed

## Open questions for the user

1. How to power the camera on odriver2 for testing: (a) set `usb_max_current_enable=1`
   and reboot odriver2, (b) stop `motion` while testing, or (c) a powered hub or a 5 A
   PSU. Recommendation: (c) if one is to hand, otherwise (a). For the final Pi, use the
   official 27 W PSU.

## Things not to do

- Don't touch `motion`, `odrive-websocket` or anything else on odriver2 — odriver2 is an
  idle ODrive rig lent for testing. The USB resets above already knocked its ODrive and
  StreamCam off the bus several times, so don't stream on it until power is sorted.

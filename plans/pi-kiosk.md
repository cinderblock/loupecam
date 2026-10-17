# Pi touchscreen kiosk

## Goal

A small fullscreen touch UI on the Raspberry Pi that runs the camera, using the
official 7" Raspberry Pi touch display. It shows the live camera view, gives a few
simple controls, and has buttons to take a snapshot and to start/stop recording.
Snapshots and recordings land in the server's normal captures directory, so any other
device can fetch them later through the web UI.

It should be a "desktopless" deployment, with no desktop environment and full control
of the screen, booting fast straight into one fullscreen app. Sometimes the display
should show other things: anyone on the network (a person, or an agent) can "cast" a
PDF, image or wiring diagram to it, Chromecast-style. Touch pan and pinch-zoom should
work in both the camera view and cast content.

## Environment / context

- Final hardware: **Raspberry Pi 4 + the original 7" touch display** (800×480 DSI,
  FT5406 touch controller on I²C, `rpi_backlight`). The Pi 4 has a hardware H.264
  encoder (`bcm2835-codec`, V4L2 mem2mem, `/dev/video11`, up to 1920×1080). The Pi 5
  has none.
- Builds on [pi-deployment.md](pi-deployment.md): `loupecam serve` runs as the
  `loupecam` systemd service (`/opt/loupecam`, state in `/var/lib/loupecam`, port 80,
  optional token in `/etc/loupecam/loupecam.env`). That plan is blocked on USB power on
  the stand-in Pi (odriver2, a **Pi 5**, borrowed ODrive rig, see its "Things not to
  do"). So odriver2 can't test the HW encoder or the display.
- Dev machine: Windows; `ffmpeg`/`ffprobe` on PATH (for checking MP4 output); camera
  not currently plugged in (`loupecam list`: none).
- Server today: MJPEG `/stream.mjpg`, WebSocket `/api/ws` (state, stats, binary JPEG
  preview frames), `POST /api/capture`, `/api/captures` (list/get/delete). Preview
  default 1280 px wide, ≤30 fps; stream default size index 2. No video recording.
- Release binaries: `aarch64-unknown-linux-musl` built natively on `ubuntu-24.04-arm`
  with `musl-tools` (C only: there's **no musl C++ toolchain**, which matters for
  openh264, a C++ library).

## Decisions already made (don't re-ask)

- **Pi 4 + original 7" display** (800×480 landscape).
- **Native Rust kiosk, fast boot**, drawing straight to DRM/KMS with no X and no
  Wayland. **Chromium override:** the kiosk can hand the screen to `cage` + Chromium
  (on demand for content it can't render natively, like PDFs and web pages, or
  permanently if configured) and take it back when that exits.
- **Recording is chosen at runtime:** high frame rate, high quality, or timelapse.
- **Casting is open to the network by default** (Chromecast-like). An optional
  separate *display token* locks it down. The display token never grants camera
  control, and the camera token keeps protecting the rest of the API. (The user first
  picked "separate display token", then said casting could just be open. Both are
  supported: open unless a display token is set.)

## Design

### Processes

- `loupecam serve` (existing service, hardened unit) owns the camera, recording and
  the cast state.
- `loupecam kiosk` (new subcommand, same binary, so self-update covers it) is a client
  of the local server over HTTP + WebSocket. It runs as a separate unit
  (`loupecam-kiosk.service`) as user `loupecam-kiosk` with a logind session on tty1
  (`PAMName=login`, `TTYPath=/dev/tty1`), in groups `video`, `render`, `input`. A
  separate process keeps the server sandbox intact: the server needs no `/dev/dri` or
  `/dev/input`, and a kiosk crash never takes down the camera.

### Native kiosk (`crates/loupecam-kiosk`)

- Pure Rust, so the static musl binary still works: `drm` (KMS: DSI connector, dumb
  buffers, double-buffered page flips), `evdev` (multitouch protocol B), software
  rendering with `tiny-skia`, text with `fontdue` (an embedded OFL font), `zune-jpeg`
  for preview frames, `resvg` for icons and SVG casts, `png`/`zune-jpeg` for image
  casts.
- Dev backend: `winit` + `softbuffer` window (mouse and touch events), so the same UI
  runs and is debuggable on Windows. Selected with `--window`.
- Not Slint: its royalty-free license excludes embedded devices, so it would mean GPLv3
  for a dual MIT/Apache project. egui and iced have no KMS backend.
- UI: full-screen viewer (pan, pinch-zoom, double-tap to fit), auto-hiding side bar:
  Snap, Record (with mode picker), AE, AWB, exposure/gain, overlay toggles; a status
  line (fps, recording time, IP/hostname so people know where to browse). Configurable
  rotation for upside-down mounting (render and touch are both transformed). Backlight
  dims after idle; tap to wake.

### Casting ("show on display")

- Server state: the current cast (`none` | `image` | `svg` | `pdf` | `url`), with the
  file stored under `/var/lib/loupecam/cast/` (only the latest is kept).
  `PUT /api/display` (a file body, or JSON `{"url": …}`), `DELETE /api/display` (back
  to camera), `GET /api/display` (metadata), `GET /api/display/content`. Broadcast
  on the WebSocket as `{"type":"display",…}`.
- Auth: the `/api/display*` routes are exempt from the camera token. If
  `--display-token` / `LOUPECAM_DISPLAY_TOKEN` is set, it (or the camera token) is
  required. A size limit applies (e.g. 64 MiB).
- The kiosk renders images and SVG natively, with pinch-zoom. PDFs and URLs use the
  Chromium override: the kiosk closes its DRM and evdev fds, runs
  `cage -- chromium --kiosk http://127.0.0.1/cast` (a web UI page that shows the cast
  plus a floating "Back to camera" button), and reclaims the screen when the cast is
  cleared or cage exits.
- Optional later step: announce `_loupecam-display._tcp` over mDNS so casting tools can
  find it.

### Recording (server, `crates/loupecam-server/src/record/`)

- `POST /api/record/start` (body: a mode + overrides), `POST /api/record/stop`; status
  goes in state/WebSocket. Output lands in the captures dir, and the Gallery lists and
  plays `.mp4` files.
- Modes (presets the UI offers; each one is just a recording-settings object):
  - **Smooth**: the current stream size, up to the sensor fps, ≤1280 px.
  - **Detailed**: switches the stream to a larger size, ≤1920×1080, lower fps.
  - **Timelapse**: one frame every N s, encoded at a playback fps (and optionally
    full-resolution stills as well).
- Container: **fragmented MP4**, written by our own small muxer, so a recording cut
  off by a power loss or crash is still playable up to the last fragment.
- Encoders behind a trait:
  - **V4L2 mem2mem** (`bcm2835-codec`, Pi 4): pure ioctls, no C deps, hardware.
  - **openh264** behind a cargo feature, for dev and testing on Windows. It is not in
    the musl release builds: it's C++ (no musl C++ toolchain in CI), and the H.264
    patent license only covers Cisco's own prebuilt binaries.
  - Later, if wanted: Media Foundation (Windows) / VideoToolbox (macOS) for the desktop
    app.
  - With no encoder available, timelapse still works as a still sequence; video modes
    report "no H.264 encoder on this host".

### USB stick storage (added 2026-10-17)

- `storage.usb` setting: `off` (internal), `whenAvailable` (stick if inserted, else
  internal), `always` (stick only; capture/record fail with "No USB stick…").
- The server looks for the stick at `--usb-dir` (default on Linux `/media/loupecam/usb`;
  `none` disables). Presence comes from `/proc/self/mountinfo` (an autofs trap counts),
  so polling doesn't keep the automount busy. Files go in `LoupeCam/` on the stick.
  Elsewhere (Windows dev) presence = the directory exists.
- Captures list both locations with `location`; get/delete take `?location=`. A
  recording picks its location at start.
- With `always` and no stick, the kiosk greys out Snap/Record under a "No USB stick" box
  with **Save on this device**; the web UI does the same in its toolbar. That choice
  sends `location=internal` per request (the setting isn't changed) and lasts until a
  stick is inserted. A running recording is never covered (Stop stays reachable).
- Packaging: `99-loupecam-usb.rules` runs `systemd-mount --automount=yes
  --timeout-idle-sec=2s --owner=loupecam` for FAT32/exFAT sticks;
  `loupecam.service` gets `ReadWritePaths=-/media/loupecam`.

## Plan / steps

1. ✅ Casting API + display token (server) and the `/cast` page (web).
2. ✅ Recording core: fMP4 muxer (tests check it with ffprobe), encoder trait, openh264
   dev backend, recorder actor, API, timelapse.
3. ✅ Web UI: record button and mode picker, Gallery video playback, touch gestures in
   Viewer.
4. ✅ Native kiosk crate: the window backend first (develop on Windows), then DRM + evdev.
5. ✅ Chromium override (cage handoff).
6. ✅ V4L2 M2M encoder (written; needs the Pi 4 to test).
7. ✅ Packaging: `loupecam-kiosk.service`, `install.sh --kiosk` (installs `cage`,
   `chromium`, the user, config.txt notes), README.
8. **← current (blocked: needs the Pi 4 + display)** Hardware test on the Pi 4 + display:
   build an aarch64 binary with `--features embed-ui,kiosk` (CI artifact or on the Pi),
   `install.sh --binary ./loupecam --kiosk`, then check: KMS output and touch (incl.
   pinch), rotation, backlight sleep/wake, hardware encoder (all three presets, file
   plays in a browser), cage/Chromium hand-off and back (PDF, URL, three-finger tap),
   `--home browser`, CPU load while recording + kiosk (see the develop-twice note).

## Findings / gotchas

- The exFAT kernel driver has no `flush` mount option (vfat does), so the udev rule
  mounts them with separate rules. ext4 sticks aren't automounted: the files' owner is
  on disk, so the server's user couldn't write without chowning the stick.
- `serve --simulate-camera` (hidden flag; `loupecam::sim::SimCamera`) streams a
  synthetic specimen through the real pipeline, so the kiosk, recording and web UI can
  be developed with no camera attached.
- On a Pi 4 `/dev/dri/card0` may be V3D (render only): the kiosk scans `card*` for one
  with a connected connector.
- To cross-check everything for the Pi from Windows, use LLVM's clang as the C compiler
  for `ring`: `CC_aarch64_unknown_linux_musl=clang
  CFLAGS_aarch64_unknown_linux_musl="--target=aarch64-linux-musl -ffreestanding"
  AR_aarch64_unknown_linux_musl=llvm-ar cargo clippy --target aarch64-unknown-linux-musl
  -p loupecam-cli --features kiosk,embed-ui`. (Type-check only; linking needs a musl
  sysroot.)
- Slint was rejected (license); the kiosk draws with tiny-skia + fontdue (DejaVu Sans via
  the `dejavu` crate), resvg for SVG casts.
- The embedded test browser (T3 preview) has no PDF viewer: PDF casts show blank
  there. Real Chrome renders them. Chromium on the Pi includes the viewer (verify).
- Chrome's PDF viewer toolbar runs along the top, so the cast page's back button sits
  bottom-right.
- The recorder develops frames itself, independently of the preview, so recording and
  watching both develop each frame. Fine on a desktop; on the Pi 4 measure it, and if
  it's CPU-bound, share the preview's developed image when the sizes match.
- Captures are now served with `tower_http::services::ServeFile` (range requests), so
  `<video>` can seek and big files aren't read into memory.
- Cast HTML/SVG is served with `Content-Security-Policy: sandbox allow-scripts`, so its
  scripts get an opaque origin and can't use the camera-token cookie. PDFs aren't
  sandboxed (Chromium's PDF viewer refuses to load under a CSP sandbox).
- The `/cast` page reads `?displayToken=`, never the stored camera token.
- URL casts are shown in an iframe; sites that forbid framing show blank, so the
  kiosk also needs a native "back to camera" gesture while Chromium runs (planned:
  read evdev without grabbing it, so cage still gets the touches).
- openh264 is C++, and the release musl toolchain is C only (`musl-tools`). It also
  carries the H.264 patent caveat above. So it's dev-only, and the Pi relies on the
  hardware encoder.

## Progress log

- [x] Questions answered (hardware, stack, recording modes, cast auth)
- [x] Casting API + display token + `/cast` page: `PUT/DELETE/GET /api/display`,
      `/content`, `/events` (SSE), WebSocket `display` message, mDNS `cast` TXT key,
      `--display-token`. Checked end to end with curl in both auth modes.
- [x] Recording: muxer, encoder trait, openh264, recorder, API, timelapse. Tests: an
      openh264 → fMP4 round trip checked by ffprobe/ffmpeg (including a file truncated
      mid-way), and the full recorder fed synthetic raw frames (video, timelapse with
      stills).
- [x] Web UI: record button + Recording panel (presets and knobs), Cast panel, ▶ videos
      in Gallery, pinch/two-finger pan/double-tap in Viewer. Checked in a browser: panels,
      preset switch, `/cast` live-switching via SSE, PDF in real Chrome, our fMP4 plays and
      seeks in Chromium. Pinch not yet tried on a real touchscreen (no frames without a
      camera, and no multi-touch in the test browser).
- [x] Native kiosk: window backend. Checked on Windows against `serve --simulate-camera`:
      live view ~30 fps, snap, record/stop with timer, settings sheet, wheel zoom, drag
      pan, SVG cast with text, PDF cast handed to the browser path. (Test by posting
      WM_* mouse messages to the window: doesn't move the user's cursor.)
- [x] Native kiosk: DRM + evdev written; clippy-clean for aarch64-musl and x86_64-gnu.
      **Not yet run on hardware** (needs the Pi 4 + display).
- [x] Chromium override: screen state machine in the KMS loop (native / cast browser /
      home browser), throwaway profile without the camera token for casts, three-finger
      tap back to the camera, `--home browser` for the full web UI. Untested on hardware.
- [x] V4L2 M2M encoder: raw ioctls (no bindgen), struct sizes asserted at compile time
      for 64-bit; type-checks for aarch64-musl. Untested on hardware.
- [x] Packaging + README: `loupecam-kiosk.service`, backlight udev rule,
      `install.sh --kiosk [--rotate N] [--no-browser]`, release/CI features, README
      sections (kiosk, casting, recording).
- [x] USB stick storage: server, web UI, kiosk, packaging. Checked on Windows with a
      folder standing in for the stick (all three modes, fallback, listing, fetch by
      location; kiosk shows "No USB stick!" and the capture error).
- [ ] Tested on Pi 4 + display (now including: a stick automounts, the sandboxed
      service can write to it through `ReadWritePaths=-/media/loupecam` (mount
      propagation into the service's namespace is the unverified part), idle unmount,
      pulling the stick mid-recording fails the recording cleanly)

## Open questions for the user

1. Is a Pi 4 with the display available to test on, and how is it reached (SSH host)?
   Step 8 needs it (odriver2 is a Pi 5: no hardware encoder, no display).
2. Push the local commits to `origin/master`? The README's `curl | sh` install (with
   `--kiosk`) only works once they're pushed and released (pi-deployment.md asks the
   same). Recommendation: push, then cut a release so the Pi can install it.

## Things not to do

- Don't stream from the camera on odriver2 until USB power is sorted (see
  pi-deployment.md).
- Never give the camera token to the Chromium instance that shows casts: a cast URL is
  arbitrary web content, it could reach the local API (CORS is permissive), and casts
  are open to the network.
- Don't use Slint (license, see above).

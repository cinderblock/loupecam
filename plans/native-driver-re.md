# LoupeCam: native AmScope/ToupTek driver, reverse engineering + library + apps

## Goal

A platform-agnostic, open-source library that talks to AmScope (ToupTek OEM) USB
cameras **natively** (no vendor SDK binaries), reads frames and every camera setting,
and changes them. On top of it: a simple desktop app with all the features one might
need, plus a "headless" mode that exposes the stream and controls on the network with
an optionally-enabled web UI.

Long-term (out of scope for now): run headless on a Linux ARM board with USB3 to make
the camera a network appliance, maybe with a local screen and shutter buttons.

## Environment / context

- Machine: Windows 11 (`NOOOK`), shell is elevated (needed for USBPcap).
- Camera: **AmScope MU1803-HS**, USB `0547:1142`, bcdUSB 3.10, serial
  `TP2306120849281D444FDD27A249155`, FW `3.5.5.20210621`, HW `3.0`, FPGA `5.2`,
  production date 2023-06-12. Sensor ID reads `0x1820` (Aptina/onsemi **AR1820HS**,
  18 MP, 1.25 µm). Resolutions 4912×3684, 2456×1842, 1228×922. Max bit depth 12.
  SDK model flags `0x81042469`.
- USB: a single interface (class `ff`), one endpoint **0x81 bulk IN**, 1024-byte
  max packet, SS companion bMaxBurst 15. The firmware reports the `MS_COMP_WINUSB` compat
  ID, so Windows binds WinUSB automatically (the INF on this machine came from libusbx
  via a Formlabs install). **No custom/signed driver is needed.**
- Vendor app: AmScope 4.12.24446 at `C:\Program Files\AmScope\AmScope`. The SDK is
  `x64\amcam.dll` v55.24446.20240114, a rebranded ToupTek `toupcam`.
  `uvcsam.dll` is for UVC models. `hcam\glavcam.sys` is for a different camera family and
  is irrelevant here.
- Tools: USBPcap (`C:\Program Files\USBPcap\USBPcapCMD.exe`), Wireshark/tshark
  (`C:\Program Files\Wireshark\tshark.exe`, not on PATH), Rust (with aarch64 linux-musl
  and windows targets), Bun, gh, Python 3.12. `~/tools/ghidra-mcp` exists (not yet
  examined).
- The USBPcap device address changes across replugs/reboots (27 → 22). `re/capture.ps1`
  looks it up each run.

## Decisions already made (don't re-ask)

- **Native backend only.** No vendor SDK in the shipped library. The SDK is used
  solely as an RE oracle (driving it while capturing, and decompiling it).
- **Stack:** a Rust core library (USB via pure-Rust `nusb`, so no native builds), a Rust
  headless server (HTTP/WebSocket controls and the stream), and a TypeScript (Bun) web UI.
  The same UI runs inside a **Tauri** desktop app and is served by headless mode.
- **Public GitHub repo** (MIT/Apache-2.0 dual license planned). No vendor binaries
  committed.
- Primary branch `master`.
- **Name: LoupeCam** (2026-10-08). Crates `loupecam`, `loupecam-protocol`, `loupecam-isp`,
  `loupecam-server`, binary `loupecam`. Vendor-neutral on purpose: the camera is a
  rebranded ToupTek, and "AmScope" is the vendor's trademark. "MyScope" and "OpenScope" were
  rejected as already in use in microscopy. "AmScope" stays only where it names the hardware
  or the vendor app.
- The user explicitly OK'd closing the AmScope app and doing whatever is needed for
  captures, including downloading/decompiling the Linux SDK.

## Plan / steps

1. **[current] Protocol RE**
   1. Capture scenarios with `re/capture.ps1` + `re/sdk_probe.py` (open/close, stream
      at each resolution, raw vs RGB, every control).
   2. Crack the session obfuscation (seed → XOR key; request-number mapping).
   3. Work out the challenge-response (`0x5e/0x79`-style). First test whether the device
      *requires* it, by skipping it from our own code.
   4. Decode the flash blob (bzip2) read via `req 0x20`.
   5. Frame format on bulk 0x81: headers/trailers, packing (RAW8/RAW12), sync.
   6. Static RE of `amcam.dll` (or Linux `libtoupcam.so`) in Ghidra where captures
      aren't enough.
2. Python proof-of-concept that opens the camera without the SDK and grabs a frame.
3. Rust workspace: `loupecam-protocol` (protocol), `loupecam-isp` (demosaic/WB/etc.),
   `loupecam-server` (headless), `apps/web` (UI), `apps/desktop` (Tauri).
4. Desktop + headless features, packaging, CI (Windows/Linux x64/arm64, macOS).

## Architecture (decided, implementing)

```
crates/loupecam-protocol pure protocol: Session<T: Transport>, frames, ISP regs   [done]
crates/loupecam          nusb transport, Camera, bulk reader thread           [done]
crates/loupecam-isp      demosaic/colour/tone/stats/AE/AWB/encoders           [done]
crates/loupecam-server   camera actor + preview pipeline + axum HTTP/WS API   [current]
crates/loupecam-cli      `loupecam` binary: list/info/stream/snap/serve
apps/web                 React + Vite + TS (Bun) UI, served by the server
apps/desktop             Tauri v2 shell: embeds the server on 127.0.0.1, same UI
```

- **One API path.** The desktop app embeds the same server and the webview talks
  HTTP/WS to it, so headless mode and desktop exercise identical code. "Share on network"
  in the desktop app just rebinds the listener.
- **Camera actor**: a std thread owns `Camera` (blocking USB). Commands arrive over a
  channel with oneshot replies. A frame thread runs AE/AWB and publishes the latest raw
  frame plus a preview JPEG (encoded only while someone is watching, to save CPU on ARM).
- **HTTP API**: `GET /api/info`, `GET /api/state`, `PATCH /api/settings`,
  `POST /api/capture`, `GET /api/captures[/name]`, `POST /api/awb`, `WS /api/ws`
  (state + stats + histogram pushes), `GET /stream.mjpg` (MJPEG multipart: works in
  `<img>`, VLC, Home Assistant, OctoPrint), `GET /snapshot.jpg`. The web UI is served at
  `/` only when enabled.
- **Security**: bind 127.0.0.1 by default. Network exposure is opt-in (`--listen`), with
  an optional bearer token / password.
- **Full-res stills while previewing small**: reconfigure to full size, skip settle
  frames, capture, restore the preview (same approach as the vendor SDK's `Snap`).

## Findings / gotchas

### Control-transfer protocol (open sequence, `captures/01_open_close.*`)

All vendor requests use `bmRequestType` `0xc0` (IN) / `0x40` (OUT).

| # | Req | Dir | Notes |
|---|-----|-----|-------|
| 1 | `0x16` | IN, wL=2 → 1 byte `08` | `wValue` = per-session random **seed** (0x3d97, 0xee4e seen) |
| 2 | varies (`0x5e`, `0x4d`) | OUT 16 bytes | random-looking **challenge** |
| 3 | varies (`0x79`, `0x67`) | IN 16 bytes | **response** |
| 4 | `0x01` | OUT wV=3 wI=0x0f | unknown, not scrambled? |
| 5 | `0x0a` | IN 2 bytes, issued twice | register read; returns `1820` (sensor chip ID) |
| 6 | `0x10` | IN 1 byte `33` | unknown |
| 7 | `0x20` | IN: 4 bytes len (`0x18f6` = 6390 LE), then 4096 @wV=0, 2326 @wV=0x1000 | flash blob: `f6180000 32c71900 00 "BZh91AY&SY"…` bzip2 |
| 8 | `0x1e` | IN 16 → "3.5.5.20210621\0" | firmware version |
| 9 | `0x1f` | IN 16 → "3.0\0" | hardware version |
| 10 | `0x0a` | IN 2 → `0502` | FPGA version 5.2 |
| 11 | `0x0d` | IN 1 → `08` | unknown |
| 12 | `0x0c` | IN 3, wI=0x30b2 → `000008` | unknown; wIndex NOT scrambled |

**Session scrambling:** `K = rotr16(seed, 4)`. Each later `wValue`/`wIndex` (on at least
`0x0a` and `0x0d`) = plain XOR K. Verified across two sessions:
- chip ID read: plain wV=0x0000 wI=0xffff (both sessions)
- FPGA read: plain wV=0x0000 wI=0xfeff
- `0x0d`: plain wV=0x0011 wI=0x30b4
- `0x0c`'s wI=0x30b2 is identical across sessions, so it is not scrambled.

The **challenge/response request numbers** also change per session (0x5e/0x79 vs
0x4d/0x67), and the rule for that is not found yet.

### Request semantics (from `captures/02_stream_controls.*`, descrambled)

Scrambled requests (wV/wI XOR session key): `0x0a` (read), `0x0b` (FPGA write),
`0x0d` (sensor write). All the others (`0x01`, `0x0c`, `0x10`, `0x17`, `0x1e`, `0x1f`,
`0x20`, `0xd9`, `0xda`) are sent plain.

- `0x0d` **sensor register write**, sent as an IN request: wV = 16-bit value, wI = AR1820
  register address, response 1 byte `08` (ack). Standard SMIA/AR1820 registers seen:
  `0x0100` mode_select (1 = stream), `0x0104` grouped_parameter_hold, `0x0300-0x030a` PLL
  (vt_pix 6, vt_sys 1, pre_pll 6, mult 100, op_pix 12, op_sys 1), `0x0112` data format
  `0x0c0c`, `0x0344-0x034e` crop/output (x 0..4921, y 0..3713, out 1232×930 for the
  1228×922 mode), `0x0342` line_length_pck, `0x0340` frame_length_lines, `0x3012`
  coarse_integration_time (exposure), `0x305e` global gain, `0x3040` read_mode (`0x61c7` =
  binning/skip for the small mode), `0x301a` reset_register, `0x30b4`, `0x31xx`/`0x3exx`
  analog tuning.
- `0x0b` **FPGA register write**, IN: wV = value, wI = `addr << 8`. Seen: `0xa2` = width/4
  (0x133 → 1228), `0xa4` = height (0x39a = 922), `0xa6`, `0xa8`; `0x60-0x70` = 3×3 color
  matrix (identity, 0x3ff = 1.0); `0xd4/0xd6/0xd8` = 0x100 (WB gains = 1.0?);
  `0xf2-0xfa`, `0xfc/0xf8`, `0x02`, `0x20`.
- `0x0a` read (2 bytes): plain wI `0xffff` → sensor chip ID `0x1820`, `0xfeff` → FPGA
  version `0x0502`.
- `0x0c` read 3 bytes, wI = some address (`0x30b2`, `0x0000`, `0x3064`). It's a sensor
  register *read*? Unclear.
- `0xd9` OUT 2048 bytes × 4 at wI `0x2200/0x2400/0x2600/0x2800`: a 4096-entry u16 LUT
  (identity ramp in RAW mode).
- `0xda` OUT 16 bytes ×2: unknown (`00727a00 00747101 00763d00 0078b800` then the same
  with 0x3x in place of 0x7x).
- `0x01` OUT, wI=0x0f: wV=3 = stream/transfer enable (also sent right after auth), wV=0 =
  stop.
- `0x17` IN 2 bytes after stop: `0000`.
- SDK `Stop` + `Start` **re-runs the whole session** (new seed, auth, full init).
- SDK H/V flip generate **no USB traffic** (they're done in software on the host).
  `put_Speed` changes line_length_pck (0 → 0x6400, 2 → 0x2ee0, default 0x2580) and then
  re-writes exposure lines.
- Exposure (µs → 0x3012 lines): 10000 → 0x22c, 5000 → 0x116, 20000 → 0x459,
  50000 → 0xadd at default speed. Gain 100/200/400 → 0x305e = 0x1609/0x2809/0x258a.

### Sweeps (`re/sweeps/gen.py` → `sw_*` captures; `re/correlate.py`, `re/diff_modes.py`)

**Sensor controls** (`sw_sensor`, RAW8 1228×922):
- Exposure: `0x3012` = round(µs / line_time). line_time = line_length_pck / ≈533.9 MHz
  (e.g. 9600 pck → 17.98 µs). `0x0340` frame_length_lines is rewritten each time but
  unchanged (1022). Exposure longer than the frame just stretches it (500 ms → 27813
  lines).
- Speed 0..3 → line_length_pck 25600 / 16800 / 12000 / 9600 at 1228×922. Exposure is
  re-sent in lines.
- HZ (anti-flicker): 0 = 60 Hz (20 ms became 927 lines = 16.67 ms), 1 = 50 Hz, 2 = DC.
  It's host-side rounding of exposure.
- Gain (`0x305e`, wrapped in group hold `0x0104`): 100 → 0x1609, 150 → 0x1f09, 200 →
  0x2809, 300 → 0x3989, 400 → 0x258a, 500 → 0x2e0a. Nonlinear (analog coarse/fine +
  digital fields). **Needs a dense 100..500 sweep or datasheet math.** Full-res mode uses
  a different base (100 → 0x2c89).
- put_Mode (bin vs skip): `0x3040` 0x69c7 vs 0x61c7, plus FPGA `0x20` = 2.
- ROI: sensor crop (x/y_addr_start/end, output size, rounded and padded) + FPGA
  `0xa2` = w/4, `0xa4` = h, then line_length/frame_length/exposure.
- Auto-exposure = a host-side loop writing exposure/gain. The trailer stats likely feed
  it.
- **No USB traffic** (host-side or unsupported here): black level, CG, low noise, high
  fullwell, framerate limit, precise framerate, bandwidth, defect pixel on/off, zero
  offset, H/V flip, the get_Option queries for uptime/line time, software trigger mode +
  Trigger(), brightness, contrast, LevelRange, chrome, negative, WhiteBalanceGain (RGB-gain
  mode).

**ISP controls** (`sw_isp_controls`, RGB24): the FPGA has a hardware ISP, enabled by
FPGA `0xf2` = 1 in RGB mode.
- White balance (TempTint) → FPGA `0xd4/0xd6/0xd8` = R/G/B gains, 8.8 fixed (0x100 =
  1.0). Defaults: 0x188, 0x100, 0x1b6.
- Hue and Saturation → recompute the 3×3 color matrix FPGA `0x60..0x70` (signed 16-bit,
  0x3ff ≈ 1.0, row-major). Default CCM (hue 0, sat 128): `[1219 -242 46; -461 1533 -49;
  -164 -527 1714]`. Option COLORMATIX 0 → identity.
- Gamma → re-upload the 4096-entry 12-bit LUT via `0xd9` (4 × 2048-byte chunks, wI
  0x2200/2400/2600/2800).
- Test pattern (option 0x28) → FPGA `0x1c` = pattern number. In the saved SDK RAW frames
  the pattern was not visible. Re-check natively.
- Even in RGB mode the bulk stream is **8 bits/pixel** (same byte count as RAW8). The
  FPGA ISP output is still mosaic-domain, and the host demosaics.

**Modes** (`sw_modes`, 3 sizes × raw8/raw16/rgb24):
- FPGA `0x02`: 0 = 8-bit transport, 1 = 16-bit (12-bit samples, LSB-aligned in u16 LE).
- FPGA `0x20`: 0/1/2 = size index (sensor binning/skip level).
- Sensor per size: `0x3040` read_mode 0x4041 / 0x60c3 / 0x61c7; output 4916×3692 /
  2460×1850 / 1232×930 (sensor outputs a few extra px, FPGA crops to the model size);
  frame_length 0x0ec8 / 0x0796 / 0x03fe; line_length 0x2a30 (0x36b0 for 16-bit
  full-res) / 0x1f40 / 0x2580.
- `0xda` 16-byte payload differs by size only (`0032eb01…`/`f500`/`7a00`); meaning
  unknown.
- Frame sizes on the wire = w×h×(1|2) + 52. Measured rates: full 12.7 fps (8-bit),
  10 fps (16-bit); 2456×1842 ~34 fps; 1228×922 ~50 fps.

**Stills / trigger** (`sw_still_trigger`): `Snap(n)` = a full re-init to the still size
(starting with sensor `0x0103` = 0x100, software reset), grab one frame, then re-init
back to the preview size. Software trigger is purely host-side (no traffic).

### Auth is not required

Seed → reads/writes/stream all work from `native_probe.py` **without** the 16-byte
challenge-response. The SDK presumably uses it to check that the camera is genuine. The
challenge/response request numbers are random per session in ranges ~`0x43-0x5e` and
`0x61-0x80`. Seed values are random too.

### Bulk / frame format (RAW8, 1228×922)

- The SDK reads 512 KiB URBs, and each frame ends with a **short transfer**. Frame =
  1228×922 bytes of pixels + **52-byte trailer**: 6 × u64 (they look like per-channel
  statistic sums, two copies of 3 values) + u32 frame counter.
- Right after stream start there are some 4-byte reads (`00000000`, `01000000`) plus
  stale partial frames. Discard until the first full-size frame.
- The test image was very dark (values 0-3), so the lens is probably capped or unlit.

### Flash blob (`req 0x20`)

`[u32 len=6390][u32 ?=0x0019c732][u8 0][bzip2 stream][32-byte ASCII serial + NUL]`. It
decompresses to 6599 bytes of records `[u16 row?][u16 count][count sorted bytes][packed
high bits…]`, which looks like a factory **defect pixel map**. Not decoded yet.

### Native driver results (Rust, `target/debug/loupecam.exe`)

- Every mode streams at the sensor-limited rate: full RAW8 13.1 fps, full RAW12 10.1 fps
  (386 MB/s), 2456×1842 34 fps, 1228×922 50 fps (capped by the 20 ms exposure).
  ROI, binning and speed all work.
- Generated control sequences match the SDK's transfer for transfer
  (`crates/loupecam-protocol/tests/sequences.rs`).
- **Bayer = RGGB**, verified with the sensor colour bars (`--sensor-reg 0x0600=2`) at
  full res. The bars come out mirrored left-to-right (the readout is mirrored).
- RAW12 is LSB-aligned 12-bit in u16 LE. Beware: PIL rescales 16-bit PGMs on read, so
  use `re/pgm_peek.py`.
- The first ~3 frames after start are dark (the SDK-style 1 ms startup exposure is
  still in the pipeline). `snap` skips 6 by default.
- FPGA test pattern (reg `0x1c`) has **no visible effect** in RAW or processed mode.
  The sensor's CCS test pattern (`0x0600`) works.
- **The lens is covered with blue tape** (per the user), which explains the dim, blurry,
  blue scenes. The user's reference image from the vendor app:
  `~/Downloads/0001.bmp` (4912×3684, shop lights on).
- **Orientation**: our frames are bottom-up relative to the vendor image (vertical-flip
  correlation 0.999 vs 0.25 unflipped) → `Model::rows_bottom_up`, applied when
  developing.
- **White balance**: the vendor uses a fixed daylight preset (R 1.53, G 1.0, B 1.71).
  Grey-world AWB turned the blue tape grey, so it's opt-in. With the preset, our colours
  match the vendor image closely (they add a little extra saturation/contrast).
- **Corrupt last row** in the 1228×922 mode (in the vendor SDK's frames too) →
  `Model::bad_last_row`, patched in the driver from the row two above.
- Gotcha: a stray `/tmp/enum.py` from the first session shadowed Python's `enum` module.
  Keep RE scripts in `re/`.

- **Restart needs re-seed + sensor power-up wait**: after a stop, sensor I²C (`0x0c`,
  `0x0d`) answers `09` until re-seeded *and* ~140 ms has passed since the `0x01` enable
  (the SDK waits ~138 ms). The first open only worked because the flash read took
  ~200 ms. Fixed with `Session::reseed` and a 150 ms `SENSOR_POWER_UP` delay.
- Server measurements: AE converges on the blue tape at ~120 ms; a full-res 12-bit still
  while previewing at 1228×922 takes 1.9 s end to end (including the 26 MB PNG).

### Gotchas

- USBPcapCMD **silently fails to overwrite** an existing output file; the old pcap
  stays put. `capture.ps1` should delete it first (TODO).
- A snap length of 4096 truncated the 4096-byte flash read. Use 65535 for control
  captures.
- The AmScope app keeps streaming in the background and holds the device. Close it
  before capturing (`capture.ps1` checks).
- Most SDK getters (serial, versions, ranges) are served from cached open-time data and
  generate no USB traffic.

## Progress log

- [x] Identify camera, SDK, driver situation
- [x] Capture tooling: `re/usbpcap.py` (pcap parser), `re/sdk_probe.py` (SDK driver),
      `re/capture.ps1`
- [x] Open/close capture decoded, XOR session key found
- [x] 12 open/close sessions: challenge req numbers are random, not seed-derived
- [x] Challenge-response is NOT required (native stream works without it)
- [x] Decompress flash blob (bzip2); [ ] parse it (defect map?)
- [x] Native RAW8 1228×922 streaming via replay (`native_probe.py stream`)
- [ ] Trailer stats semantics; RAW12 packing; RGB (non-RAW) mode; other resolutions
- [ ] Derive register values from first principles (PLL, timing, exposure formula)
      rather than replaying blobs
- [x] Controls capture sweeps: sensor, ISP, modes, test patterns, still/trigger
- [ ] Dense gain sweep (100..500) → gain encoding
- [ ] Meaning of `0xda` payload, `0x0c` reads, FPGA `0xa6/0xa8/0xf4-0xfc`, trailer stats
- [ ] Bayer order + color verification (needs a lit, focused, colorful target)
- [ ] Ghidra on amcam.dll (not needed so far)
- [x] Python PoC without SDK (`re/native_probe.py`)
- [x] Rust workspace: protocol + driver + CLI, verified on hardware
- [x] loupecam-isp: demosaic (superpixel/bilinear/MHC), colour, tone, stats, AE/AWB,
      PNG/TIFF/JPEG. Full-res MHC 66 ms
- [x] loupecam-server (actor, preview, REST/WS/MJPEG, captures, ROI/WB from display regions,
      token auth, settings persistence) + `loupecam serve`. Verified on hardware
- [ ] apps/web UI
- [ ] apps/desktop (Tauri)
- [ ] README, docs/protocol.md, udev rule, CI (Win/Linux x64+arm64/macOS), license files
- [ ] Hotplug / reconnect
- [ ] Video recording (format TBD, see open questions)
- [ ] Microscope extras: scale bar/calibration per objective, crosshair/grid overlays
- [ ] mDNS advertisement for the headless appliance
- [ ] Optical calibration of analog gain stages; black level; decode defect map

## Open questions for the user

1. License: MIT/Apache-2.0 dual (Rust convention)? *Recommendation: yes.*
2. ~~Repo name on GitHub~~ Answered 2026-10-08: the project is **LoupeCam**,
   repo `cinderblock/loupecam` (not created yet).
3. Video recording: (a) MJPEG-in-AVI/MKV (pure Rust, big files, every frame
   lossless-ish), (b) H.264 via openh264 (a native build, small files), or (c) a
   browser-side MediaRecorder for desktop use only. *Recommendation: (a) first, (b)
   later as an option.*
4. Need a lit, focused, colourful target under the microscope for colour/sharpness
   validation and gain calibration.

## Things not to do

- Don't commit vendor binaries or decompiled vendor code to the public repo. Commit
  only protocol knowledge written in our own words, plus our own code.
- Don't commit raw pcaps wholesale (they're large and contain the device serial).
  Curate small fixtures deliberately.
- Don't leave the AmScope app running during captures.

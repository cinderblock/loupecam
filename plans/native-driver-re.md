# AmScope native driver: reverse engineering + library + apps

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
3. Rust workspace: `amscope-core` (protocol), `amscope-isp` (demosaic/WB/etc.),
   `amscope-server` (headless), `apps/web` (UI), `apps/desktop` (Tauri).
4. Desktop + headless features, packaging, CI (Windows/Linux x64/arm64, macOS).

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
- [ ] More open/close sessions to pin down the request-number mapping
- [ ] Does the device require the challenge-response?
- [ ] Decompress/parse flash blob
- [ ] Streaming capture + frame format
- [ ] Controls capture (exposure, gain, flip, speed, resolution, ROI, …)
- [ ] Ghidra on amcam.dll
- [ ] Python PoC without SDK
- [ ] Rust workspace

## Open questions for the user

1. License: MIT/Apache-2.0 dual (Rust convention)? *Recommendation: yes.*
2. Repo name on GitHub (e.g. `cinderblock/amscope-reader`)?

## Things not to do

- Don't commit vendor binaries or decompiled vendor code to the public repo. Commit
  only protocol knowledge written in our own words, plus our own code.
- Don't commit raw pcaps wholesale (they're large and contain the device serial).
  Curate small fixtures deliberately.
- Don't leave the AmScope app running during captures.

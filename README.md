# LoupeCam

An open, cross-platform driver and app for AmScope / ToupTek USB3 microscope
cameras. It needs **no vendor SDK and no driver installation**: it talks to the camera
directly over USB, so it runs anywhere Rust and USB do, including ARM Linux boards.

- **`loupecam` library** (Rust): open the camera, stream raw frames, and control every
  setting.
- **Image pipeline**: demosaic (superpixel, bilinear, Malvar-He-Cutler), white balance,
  colour correction, tone curves, auto exposure and white balance, and PNG/TIFF/JPEG
  output.
- **Headless server** (`loupecam serve`): live MJPEG stream, a JSON + WebSocket API for
  all controls and captures, and an optional web UI. Run it on a small board and the
  camera becomes a network device.
- **Desktop app** (Tauri): the same UI in a native window, backed by the same server.

> Status: early. Developed and verified against an **AmScope MU1803-HS** (18 MP,
> onsemi AR1820HS). Other ToupTek models use the same protocol family but need their
> sensor programming captured (see [Adding a camera](#adding-a-camera)).

## Supported cameras

| Model | USB ID | Sensor | Status |
| --- | --- | --- | --- |
| AmScope MU1803-HS | `0547:1142` | AR1820HS, 4912×3684, 1.25 µm | Working: all sizes, 8/12-bit, ROI, binning, speeds |

## Install

Download from [Releases](https://github.com/cinderblock/loupecam/releases/latest):

- **Desktop app**: the Windows installer (`…-setup.exe` or `.msi`), the macOS `.dmg`
  (Apple silicon or Intel), or a Linux `.AppImage`/`.deb`/`.rpm`.
- **Command line / headless**: one self-contained binary per platform:
  `loupecam-x86_64-pc-windows-msvc.exe`, `loupecam-x86_64-unknown-linux-musl`,
  `loupecam-aarch64-unknown-linux-musl` (Raspberry Pi and other ARM64 boards),
  `loupecam-aarch64-apple-darwin`, `loupecam-x86_64-apple-darwin`. The web UI is
  built in (`loupecam serve --web-ui embedded`).

Every release includes `SHA256SUMS` and GitHub build-provenance attestations. Windows
and macOS code signing are wired up but not yet active (no certificates yet), so
SmartScreen and Gatekeeper warn on first launch. See [docs/releasing.md](docs/releasing.md).

## Updates

Every build can update itself. Downloads are verified against LoupeCam's release
signing key, which is compiled into the app, before anything is installed.

- **CLI**: `loupecam update` (or `--check` to only look).
- **Headless server**: checks for new releases every 6 hours (`updates.checkIntervalHours`)
  and shows a banner in the web UI with **Update and restart**. Set
  `updates.autoInstall` (web UI → About & updates) to install automatically; the
  server then releases the camera and restarts itself. `--no-update-check` disables
  checking.
- **Desktop app**: checks at launch and every 6 hours and asks before installing.
  File → **Automatically Install Updates** makes it silent; File → **Check for Updates…**
  checks now.

## Building from source

### Quick start

Requirements: [Rust](https://rustup.rs) (stable) and, for the web UI and desktop app,
[Bun](https://bun.sh).

```sh
# Build the web UI once (served by `loupecam serve --web-ui`)
cd apps/web && bun install && bun run build && cd ../..

cargo run --release -p loupecam-cli -- list
cargo run --release -p loupecam-cli -- info
cargo run --release -p loupecam-cli -- snap --size 0 --mode raw12 -o photo.png
cargo run --release -p loupecam-cli -- serve --web-ui apps/web/dist
# then open http://127.0.0.1:8080
```

Close the AmScope app first: only one program can use the camera at a time.

### Windows

Nothing to install. The camera's firmware tells Windows to use its built-in WinUSB
driver.

### Linux

Allow non-root access with the udev rule:

```sh
sudo cp packaging/linux/99-loupecam.rules /etc/udev/rules.d/
sudo udevadm control --reload && sudo udevadm trigger
```

## The `loupecam` command

| Command | |
| --- | --- |
| `loupecam list` | Connected cameras |
| `loupecam info` | Model, serial, production date, firmware/hardware/FPGA versions |
| `loupecam stream` | Stream for a while and report frame rate, drops and throughput |
| `loupecam snap -o file.{png,tif,jpg,pgm}` | Capture: `.pgm` and `--raw` are undeveloped sensor data; the rest are developed images |
| `loupecam serve` | The network service (below) |
| `loupecam update` | Install the latest release (signature-verified) |

Stream options: `--size` (0 = largest), `--mode raw8|raw12`, `-e/--exposure` µs,
`-g/--gain` (multiplier), `--speed 0..3`, `--binning`, `--roi x,y,w,h`. Develop options:
`--demosaic`, `--wb r,g,b` / `--awb`, `--sixteen`, `--no-ccm`. For experiments,
`--sensor-reg ADDR=VAL` and `--fpga-reg ADDR=VAL`.

## Headless server

```sh
loupecam serve --listen 0.0.0.0:8080 --token "$(openssl rand -hex 16)" --web-ui apps/web/dist
```

The server listens on `127.0.0.1` by default. When exposing it, set a token. Clients send
it as `Authorization: Bearer …` (the token can also come from `LOUPECAM_TOKEN`), as `?token=…` (for `<img>` and WebSocket), or in an
`loupecam_token` cookie.

| Endpoint | |
| --- | --- |
| `GET /stream.mjpg` | Live MJPEG (browsers, VLC, Home Assistant, OctoPrint, …) |
| `GET /snapshot.jpg` | Latest preview frame |
| `GET /api/state` | Status, device, settings |
| `GET /api/stats` | fps, exposure, gain, channel means, histogram |
| `GET`/`PATCH /api/settings` | Read / change settings (JSON merge patch, e.g. `{"exposureUs": 50000, "autoExposure": false}`) |
| `POST /api/capture[?fullResolution=bool]` | Develop and save a still; returns its name |
| `GET /api/captures`, `GET`/`DELETE /api/captures/{name}` | Saved captures |
| `POST /api/white-balance` `{"region": {x,y,width,height} \| null}` | One-shot white balance over a region of the displayed image (normalised 0..1) |
| `POST /api/roi` `{"region": … \| null}` | Set or clear the sensor ROI from a region of the displayed image |
| `WS /api/ws` | Pushes `state` and `stats` messages; send `{"type":"preview","enabled":true}` to also receive JPEG frames as binary messages |

Settings persist across restarts (in the OS config directory, e.g.
`%APPDATA%\loupecam\settings.json`). Captures go to `Pictures/LoupeCam` unless
`--captures` says otherwise.

## Desktop app

```sh
cd apps/desktop && bun install && bun run dev     # or: bun run build for an installer
```

For UI work, `cd apps/web && bun run dev` serves the UI with hot reload and proxies
the API to a running `loupecam serve` (override with `LOUPECAM_SERVER=http://host:port`).

## Releasing

Push a tag: `git tag -a v1.2.3 -m "LoupeCam 1.2.3" && git push origin v1.2.3`. The
version comes from the tag. Signing (update signatures, checksums, provenance, and
Windows/macOS code signing once certificates are configured) is described in
[docs/releasing.md](docs/releasing.md).

## Layout

```text
crates/loupecam-protocol  wire protocol, no I/O (session, registers, frame decoding)
crates/loupecam           USB driver over nusb (pure Rust)
crates/loupecam-isp       image pipeline and encoders
crates/loupecam-server    headless service: camera actor, preview, HTTP/WS API
crates/loupecam-cli       the `loupecam` binary
crates/loupecam-update    signed self-update from GitHub Releases
apps/web                 web UI (React, Vite, Bun)
apps/desktop             Tauri desktop shell
docs/protocol.md         the USB protocol, documented
re/                      reverse-engineering tools (capture, decode, correlate)
```

## How it was built

The protocol was worked out by driving the vendor SDK from a script while recording
USB traffic with USBPcap, then attributing each transfer to the SDK call that caused
it ([`re/`](re)). The camera turned out to use a lightly scrambled vendor protocol in
front of standard AR1820 sensor registers and a small FPGA image pipeline. The full
write-up is in [docs/protocol.md](docs/protocol.md). The library's tests check that it
produces the same control sequences as the vendor SDK.

No vendor code or binaries are included in this repository.

### Adding a camera

1. Install the vendor software, plug the camera in, and close the vendor app.
2. Run the sweeps: `re/capture.ps1 <name> re/sweeps/modes.json` (and the others in
   `re/sweeps`). This needs Windows, USBPcap, Python and an elevated shell.
3. Decode with `re/correlate.py` and `re/diff_modes.py`, add a `Model` (and a sensor
   module if it uses a new sensor) to `crates/loupecam-protocol`, and add fixtures with
   `re/make_fixture.py`.

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.

AmScope and ToupTek are trademarks of their respective owners. This project is not
affiliated with either.

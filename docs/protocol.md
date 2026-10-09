# AmScope / ToupTek USB3 camera protocol

How the MU1803-HS (and, very likely, other ToupTek-made USB3 cameras sold under the
AmScope and other brands) is driven over USB. Everything here was learned by
observing traffic between the vendor SDK and the camera (USBPcap), and by
experimenting from our own code. The tools are in [`re/`](../re).

The implementation is [`crates/loupecam-protocol`](../crates/loupecam-protocol). Its
tests replay control sequences captured from the vendor SDK and require ours to match
them transfer for transfer.

## Device

| | |
| --- | --- |
| USB IDs | `0547:1142` (MU1803-HS) |
| Speed | SuperSpeed (bcdUSB 3.10) |
| Interface | one, class `ff` (vendor) |
| Endpoints | `0x81` bulk IN (1024-byte packets, burst 15) for image data |
| Windows driver | the firmware reports the `MS_COMP_WINUSB` compatible ID, so Windows binds the inbox WinUSB driver automatically. No vendor driver is involved. |
| Linux | plain usbfs. Needs a udev rule for non-root access ([`packaging/linux`](../packaging/linux)). |

Inside are a Cypress FX3 bridge, an FPGA (version 5.2 on our unit), and an onsemi AR1820HS
18 MP sensor reached over I²C through the bridge.

## Control requests

All vendor requests are addressed to the device with `bmRequestType` `0xc0` (IN) or
`0x40` (OUT). Several "write" operations are sent as IN requests and return a
one-byte status: `08` = OK, `09` = failed (e.g. the sensor did not answer on I²C).

| `bRequest` | Dir | wValue | wIndex | Data | Meaning |
| --- | --- | --- | --- | --- | --- |
| `0x16` | IN 2 | seed | 0 | `08` | Start a session; sets the scrambling key (below) |
| `0x01` | OUT | 3 / 0 | `0x0f` | — | Enable / disable. Gates the sensor: I²C fails for ~140 ms after enabling |
| `0x0a` | IN 2 | *scr* | *scr* | big-endian u16 | Read. `(0, 0xffff)` = sensor chip ID (`0x1820`), `(0, 0xfeff)` = FPGA version (`0x0502` = 5.2) |
| `0x0b` | IN 1 | *scr* value | *scr* `reg << 8` | status | FPGA register write |
| `0x0d` | IN 1 | *scr* value | *scr* register | status | Sensor register write (16-bit address and value) |
| `0x0c` | IN 3 | 0 | address | `[?, ?, status]` | Probably a sensor register read; the SDK issues a few at open/start |
| `0x10` | IN 1 | 0 | 0 | `33` | Status, meaning unknown |
| `0x17` | IN 2 | 0 | 0 | `0000` | Read after stopping, meaning unknown |
| `0x1e` | IN 16 | 0 | 0 | ASCII | Firmware version, e.g. `3.5.5.20210621` |
| `0x1f` | IN 16 | 0 | 0 | ASCII | Hardware version, e.g. `3.0` |
| `0x20` | IN ≤4096 | offset | 0 | bytes | Factory data (below) |
| `0xd9` | OUT 2048 | 0 | `0x2200 + 0x200·n` | 1024 × u16 LE | Tone LUT chunk *n* of 4 (4096 entries, 12-bit in and out) |
| `0xda` | OUT 16 | 0 | 0 | 4 × `[00, reg, u16 LE]` | Statistics window (two banks: regs `0x72..0x78` and `0x32..0x38`) |

### Session scrambling

The `wValue` and `wIndex` of `0x0a`, `0x0b` and `0x0d` are XORed with a 16-bit key derived
from the seed sent in `0x16`:

```text
key = rotate_right_16(seed, 4)
on the wire: wValue ^ key, wIndex ^ key
```

The seed can be any value. A new session (new seed) is needed after every stream stop
before the sensor accepts writes again.

### Challenge-response (not needed)

Right after the seed, the vendor SDK sends a 16-byte random block with an OUT request
whose number varies (seen `0x43`–`0x5e`) and reads 16 bytes back with an IN request
(`0x61`–`0x80`). The camera works without it. It appears to be the SDK checking that
the camera is genuine, so this driver skips it.

## Factory data

Read with `0x20`: first 4 bytes (`u32 LE len`), then `len + 32` bytes total from offset 0,
in chunks of up to 4096 (`wValue` = offset).

```text
[u32 len][5-byte header][bzip2 stream …][32-byte NUL-padded ASCII serial]
└──────────────── len bytes ────────────────┘
```

The serial looks like `TP230612…` and encodes the production date (2023-06-12). The
bzip2 payload (6.6 kB on our unit) looks like a factory defect-pixel map and is not
decoded yet.

## Sequences

**Open:** seed → `0x01` enable → wait 150 ms → read sensor ID → `0x10` → factory data →
versions → read FPGA version → sensor `0x30b4 = 0x0011` → `0x0c(0x30b2)`.

**Start streaming** (all writes through the scrambled requests):

1. FPGA `0xfc = 1`, `0xf8 = 1`. `0x0c` reads of `0x0000` and `0x3064`.
2. Sensor standby (`0x0100 = 0`), then PLL (`0x0304 = 6`, `0x0306 = 100`, `0x0302 = 1`,
   `0x0300 = 6`, `0x030a = 1`, `0x0308 = 12`) inside a grouped-parameter hold
   (`0x0104 = 0x100 … 0`).
3. FPGA `0x02`: 0 = 8-bit transport, 1 = 16-bit.
4. Sensor analog tuning (`0x31be`, `0x31ae`, `0x31c6`, `0x31c0`, `0x0112 = 0x0c0c`,
   `0x3f3c`, `0x3ed2`, `0x31e0`, `0x31e6`, `0x3ede`, `0x30b4`; values in
   `sensor/ar1820.rs`).
5. FPGA `0x20` = size index; window (`0x0344`–`0x034e`) and FPGA `0xa2 = width/4`,
   `0xa4 = height`, `0xa6 = 0`, `0xa8 = 2`, in a hold.
6. Line length (`0x0342`, in a hold), frame length (`0x0340 = output height + 92`), and
   integration time (`0x3012`).
7. `0x3040` read mode (subsampling; `| 0x0800` for binning), `0x301a = 0x10`,
   `0x301e = 0`, a 1 ms startup exposure, then `0x301a = 0x1e` to start streaming.
8. FPGA ISP: `0xf2` (1 = on), `0xfa = 1`, `0xf4 = 4`, `0xf6 = 4`; white balance
   `0xd4/0xd6/0xd8` (8.8 fixed point); colour matrix `0x60..0x70` (row-major, signed,
   1.0 = 1023); LUT upload. In RAW modes these are identity.
9. Real exposure, gain (`0x305e`, in a hold), the two statistics windows, `0x01` enable.

**Stop:** sensor `0x0100 = 0`, `0x01` disable twice, `0x17`.

## Modes (MU1803-HS)

| Size | Read mode | Sensor output | FPGA `0x20` | Line length, speed 0…3 |
| --- | --- | --- | --- | --- |
| 4912 × 3684 | `0x4041` | 4916 × 3692 | 0 | 38400, 21600, 14400, 10800 (≥14000 in 16-bit) |
| 2456 × 1842 | `0x60c3` | 2460 × 1850 | 1 | 32000, 16000, 10400, 8000 |
| 1228 × 922 | `0x61c7` | 1232 × 930 | 2 | 25600, 16800, 12000, 9600 |

- **Exposure**: `0x3012 = round(µs × 534 MHz / line_length)`. The vendor uses 534 MHz;
  the PLL math gives 533.3 MHz. Exposures longer than a frame just stretch the frame.
- **Gain** (`0x305e`): bits 6:0 select an analog stage (`0x09` ≈ 1×, `0x0a` ≈ 2×,
  `0x0e` ≈ 3×, `0x7a` ≈ 4×); bits 15:7 are digital gain in 1/64 units.
- **ROI**: the array is read mirrored on both axes, so windows are addressed back from
  the full-frame end coordinates; `end − start = n·f − (2f − 1)` for `n` output pixels
  at subsampling factor `f`.
- Stills at a higher resolution than the preview are a full reconfiguration, done the
  same way as the vendor SDK.

## Image stream

Frames arrive on bulk `0x81`, read in 512 KiB transfers. Each frame is
`width × height × (1 | 2)` bytes of Bayer data (RGGB; 16-bit samples are 12-bit,
little-endian, LSB-aligned), followed by a 52-byte trailer, and is terminated by a
short transfer.

```text
trailer: u64 R, G, B sums over stats window 0; u64 R, G, B over window 1; u32 frame counter
```

The first few frames after start still carry the startup exposure. Rows arrive
bottom-up relative to the vendor software's upright image. In the 1228 × 922 mode the
last row is corrupt (the vendor SDK's frames show it too).

With the FPGA ISP enabled (the vendor's "RGB" modes) the stream is still 8-bit Bayer:
the FPGA applies white balance, the colour matrix and the tone LUT in mosaic domain,
and the host demosaics.

## Unknowns

- `0x0c` responses, `0x10`, `0x17`, FPGA registers `0xa6`, `0xa8`, `0xf4`–`0xfc`.
- The FPGA test-pattern register (`0x1c`, set by the SDK's test-pattern option) has no
  visible effect. The sensor's own test pattern (`0x0600`) works.
- Factory defect-map format.
- Optical calibration of the analog gain stages.

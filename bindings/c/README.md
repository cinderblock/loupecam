# LoupeCam C API

A C interface to [LoupeCam](https://github.com/cinderblock/loupecam), the open driver
for AmScope / ToupTek USB3 microscope cameras. Anything that can call C can use it:
C and C++, C# (P/Invoke), Go (cgo), Julia, MATLAB, LabVIEW, Zig and others. Like the
rest of LoupeCam, it talks to the camera directly over USB, so it needs no vendor SDK
and no driver installation.

## Getting it

Each [release](https://github.com/cinderblock/loupecam/releases/latest) has a
`loupecam-c-<target>.zip` per platform. Each one contains:

- `include/loupecam.h`
- `lib/`: the shared library and the static library
- `native-static-libs.txt`: the system libraries a static link needs

| Platform | Shared library | Link with |
| --- | --- | --- |
| Windows | `loupecam_c.dll` | `loupecam_c.dll.lib` (import library) |
| Linux | `libloupecam_c.so` | `-lloupecam_c` |
| macOS | `libloupecam_c.dylib` | `-lloupecam_c` |

To build it yourself, run `cargo build --release -p loupecam-capi`. The libraries land
in `target/release`, and the header is [`include/loupecam.h`](include/loupecam.h).

## Example

```c
#include <stdio.h>
#include "loupecam.h"

int main(void) {
  LoupecamCamera *cam;
  if (loupecam_camera_open(0, &cam) != LOUPECAM_STATUS_OK) {  /* or _open_simulated */
    fprintf(stderr, "%s\n", loupecam_last_error());
    return 1;
  }
  LoupecamStartOptions start = loupecam_start_options_default();
  start.pixel_mode = LOUPECAM_PIXEL_MODE_RAW12;
  LoupecamStream *stream;
  LoupecamFrame *frame;
  loupecam_camera_start(cam, &start, &stream);
  loupecam_stream_read(stream, 2000, &frame);

  LoupecamDevelopOptions dev = loupecam_develop_options_default();
  dev.demosaic = LOUPECAM_DEMOSAIC_MHC;
  loupecam_frame_save(frame, "photo.png", 90, false, &dev);

  loupecam_frame_free(frame);
  loupecam_stream_free(stream);
  loupecam_camera_free(cam);
  return 0;
}
```

[`examples/snap.c`](examples/snap.c) is a complete program that checks every result.
It's also this API's test: [`test.sh`](test.sh) compiles it with warnings as errors and
runs it against the simulated camera.

## Conventions

- **Results:** every call returns a `LoupecamStatus` (`LOUPECAM_STATUS_OK` is 0). After
  a failure, `loupecam_last_error()` describes it, per thread.
- **Handles** (`LoupecamCamera`, `LoupecamStream`, `LoupecamFrame`,
  `LoupecamImage`, `LoupecamBytes`) are opaque and freed with their `_free` function.
  Freeing `NULL` does nothing.
- **Data pointers** (`loupecam_frame_data`, `loupecam_image_data`,
  `loupecam_bytes_data`) stay valid until their handle is freed.
- **Enum-valued fields** are plain integers set from the `LOUPECAM_…` constants. An
  out-of-range value returns `LOUPECAM_STATUS_INVALID_ARGUMENT` rather than causing
  undefined behaviour.
- **Threads:** any function can be called from any thread. A camera's calls are
  serialized internally, and a blocking `loupecam_stream_read` doesn't hold up other
  cameras or streams.
- **Panics** never cross into C: they become `LOUPECAM_STATUS_ERROR` with a message.
- **Samples:** a frame's raw samples are `uint8_t`, or little-endian `uint16_t` in RAW12
  (that's native on every supported platform). Rows come as the sensor sends them,
  which is bottom-up on the MU1803. Developing turns them upright.
- **Strings** are UTF-8 (paths too).

## API

| | |
| --- | --- |
| `loupecam_camera_count`, `loupecam_camera_open(index)`, `loupecam_camera_open_simulated` | Find and open cameras |
| `loupecam_camera_info`, `loupecam_camera_resolution` | Model, serial, versions and sizes |
| `loupecam_camera_start(options)` | Stream (size, pixel mode, exposure, gain, speed, binning, ROI) |
| `loupecam_camera_set_exposure / _gain / _speed / _binning / _roi`, `loupecam_camera_stop` | Change settings while streaming |
| `loupecam_camera_geometry` | The stream's size, bit depth, exposure and frame-rate limit |
| `loupecam_stream_read(timeout)`, `loupecam_stream_latest`, `loupecam_stream_stats` | Frames and counters |
| `loupecam_frame_info`, `loupecam_frame_data` | The raw mosaic |
| `loupecam_frame_develop(options)` | An RGB (or monochrome) image, 8- or 16-bit |
| `loupecam_frame_encode`, `loupecam_frame_save` | PNG, TIFF or JPEG, or the raw mosaic as TIFF |
| `loupecam_frame_white_balance`, `loupecam_frame_stats` | Grey-world gains, means and the histogram |

The header documents every function and field. `LoupecamDevelopOptions` has the same
options, with the same defaults, as the Python and Node.js packages.

## ABI stability

The ABI may change before 1.0. Build against the header that ships with the library.
`loupecam_version()` reports the version of the library you loaded.

## Setup

On Linux, non-root users need the udev rule
([`99-loupecam.rules`](../../packaging/linux/99-loupecam.rules)). Windows and macOS need
nothing. Only one program can use the camera at a time.

## License

MIT OR Apache-2.0.

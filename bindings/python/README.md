# loupecam (Python)

Python bindings for [LoupeCam](https://github.com/cinderblock/loupecam), the open
driver for AmScope / ToupTek USB3 microscope cameras. They talk to the camera directly
over USB, so they need no vendor SDK and no driver installation. Frames come back as
NumPy arrays.

```sh
pip install loupecam
```

The wheels need CPython 3.9 or later, and are built for Windows (x64), Linux (x64,
arm64) and macOS (arm64, x64).

## Example

```python
import loupecam

with loupecam.open() as cam:  # the first camera found
    print(cam.info)  # <CameraInfo MU1803-HS serial … firmware …>

    # Size 0 is the largest (4912×3684 on the MU1803); raw12 gives 12-bit samples.
    stream = cam.start(size=0, mode="raw12", exposure_us=30_000, gain=1.0)
    frame = stream.read(timeout=2)

    mosaic = frame.raw         # (3684, 4912) uint16: the sensor's Bayer data
    rgb = frame.develop()      # (3684, 4912, 3) uint8, colour-corrected and upright
    frame.save("photo.png", white_balance="auto", demosaic="mhc")
    frame.save("photo16.tif", depth=16)
    frame.save("mosaic.tif", raw=True)  # undeveloped, for raw tools
```

Reading frames continuously:

```python
stream = cam.start(size=2)  # 1228×922, the fastest
for frame in stream:  # until the stream ends
    show(frame.develop(demosaic="superpixel"))
```

`stream.latest()` skips frames that queued up while you were busy, which suits a live
view. Waiting for a frame, developing and encoding all release the GIL, so other
threads keep running. Ctrl-C interrupts a wait.

### Without a camera

`loupecam.open_simulated()` gives a simulated MU1803-HS that produces synthetic
frames. It has the same API, so it's useful for tests and for writing code before the
hardware arrives.

## API

The complete, documented API is in the type stubs
([`_native.pyi`](python/loupecam/_native.pyi)). Editors and type checkers pick them up.

| | |
| --- | --- |
| `list_cameras()` | Connected cameras (`CameraDevice`: model, id, bus, port chain) |
| `open(device=None)` | The first camera, or a `CameraDevice`, an index, or an id |
| `open_simulated()` | The simulator |
| `Camera.start(size, mode, *, exposure_us, gain, speed, binning, roi)` | Start streaming and return a `Stream` |
| `Camera.set_exposure / set_gain / set_speed / set_binning / set_roi` | Change settings while streaming |
| `Camera.info`, `.model`, `.geometry`, `.stats` | Identity, sizes, the stream's timing and its counters |
| `Stream.read(timeout)`, `.latest(timeout)`, `.try_read()` | Frames |
| `Frame.raw` | The mosaic as a NumPy array (`numpy.asarray(frame)` also works) |
| `Frame.develop(**options)` | An RGB (or monochrome) array |
| `Frame.encode(format, **options)`, `Frame.save(path, **options)` | PNG, TIFF (8/16-bit) or JPEG |
| `Frame.white_balance(region)`, `Frame.stats(region)` | Grey-world gains, means and the histogram |

The develop options all default to the camera model's own settings:

| Option | Values |
| --- | --- |
| `demosaic` | `"superpixel"`, `"bilinear"` (default) or `"mhc"` |
| `white_balance` | `"daylight"` (default), `"auto"`, `"none"` or `(r, g, b)` |
| `color_matrix` | the model's (default), `False`, or a 3×3 matrix |
| `tone` | `"srgb"` (default), `"linear"` or a gamma value |
| `black_point`, `white_point`, `brightness`, `contrast` | levels |
| `black_level` | in sample units |
| `monochrome`, `negative` | |
| `flip_horizontal`, `flip_vertical`, `rotation` | orientation |
| `depth` | `8` or `16` |

Errors raise `loupecam.LoupecamError`. A stream that has ended (stopped, restarted or
closed) raises `loupecam.StreamStopped`, a subclass. A timed-out wait raises
`TimeoutError`.

## Setup

- **Windows:** nothing. The camera uses the built-in WinUSB driver.
- **Linux:** install the udev rule so non-root users can open the camera:

  ```sh
  sudo curl -o /etc/udev/rules.d/99-loupecam.rules \
    https://raw.githubusercontent.com/cinderblock/loupecam/master/packaging/linux/99-loupecam.rules
  sudo udevadm control --reload && sudo udevadm trigger
  ```

- **macOS:** nothing.

Only one program can use the camera at a time, so close the AmScope app (or a running
`loupecam serve`) first.

## Building from source

You need [Rust](https://rustup.rs) and [maturin](https://www.maturin.rs). From
`bindings/python`:

```sh
pip install maturin numpy pytest
maturin develop --release
pytest
```

## License

MIT OR Apache-2.0.

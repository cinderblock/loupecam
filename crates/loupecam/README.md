# loupecam

A native driver for AmScope (ToupTek OEM) USB3 microscope cameras.

It talks to the camera directly over USB with [`nusb`](https://crates.io/crates/nusb),
so it needs no vendor SDK and no kernel driver:

- **Windows:** the camera binds to the built-in WinUSB driver.
- **Linux:** it needs only a udev rule granting access
  ([`99-loupecam.rules`](https://github.com/cinderblock/loupecam/blob/master/packaging/linux/99-loupecam.rules)).
- **macOS:** no setup needed.

It's tested with the AmScope MU1803-HS (18 MP, onsemi AR1820HS).

```rust,no_run
use loupecam::{Camera, PixelMode, StreamConfig};

let mut cam = Camera::open_first()?;
println!("{} serial {}", cam.info().model.name, cam.info().serial);

// Size index 0 is the largest; Raw12 gives 12-bit samples in 16-bit words.
let frames = cam.start(&StreamConfig::new(0, PixelMode::Raw12))?;
let frame = frames.recv()?;
println!("{}x{} frame #{}", frame.width, frame.height, frame.trailer.sequence);
# Ok::<(), loupecam::Error>(())
```

Frames are the raw Bayer mosaic.
[`loupecam-isp`](https://crates.io/crates/loupecam-isp) turns them into RGB images
and PNG/TIFF/JPEG files.

## Without a camera

`loupecam::AnyCamera::simulated()` gives a simulated MU1803-HS with synthetic frames.
That's useful for tests and for writing code before the hardware arrives.
`AnyCamera` runs the same code against either one.

## Related crates

| Crate | |
| --- | --- |
| [`loupecam-protocol`](https://crates.io/crates/loupecam-protocol) | The wire protocol, with no I/O. |
| [`loupecam-isp`](https://crates.io/crates/loupecam-isp) | The image pipeline and encoders. |

There are also Python, Node.js and C bindings, and a server and desktop app. See
[the repository](https://github.com/cinderblock/loupecam).

## License

MIT OR Apache-2.0.

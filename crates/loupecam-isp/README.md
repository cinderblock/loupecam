# loupecam-isp

The host-side image pipeline for AmScope / ToupTek microscope cameras. It turns raw
Bayer frames from [`loupecam`](https://crates.io/crates/loupecam) into images:

- black level and white balance
- demosaicing: superpixel, bilinear, or Malvar–He–Cutler
- colour matrix, tone curves, monochrome and negative
- flips and rotation
- flat-field and defective-pixel correction
- statistics, plus auto exposure and white balance controllers
- PNG, TIFF and JPEG encoding (8- and 16-bit), and raw TIFF

```rust,no_run
use loupecam::{Camera, PixelMode, StreamConfig};
use loupecam_isp::{DevelopParams, Demosaic, develop, encode};

let mut cam = Camera::open_first()?;
let frame = cam.start(&StreamConfig::new(0, PixelMode::Raw12))?.recv()?;

// The model's colour matrix and daylight white balance, rows upright.
let mut params = DevelopParams::for_model(cam.model());
params.demosaic = Demosaic::Mhc;
let image = develop(&frame, &params);
std::fs::write("photo.png", encode::png8(&image)?)?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

Developing is parallelised with rayon and done row by row, so a full 18 MP frame needs
no full-size intermediate buffer.

## License

MIT OR Apache-2.0.

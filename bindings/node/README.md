# loupecam (Node.js)

Node.js bindings for [LoupeCam](https://github.com/cinderblock/loupecam), the open
driver for AmScope / ToupTek USB3 microscope cameras. They talk to the camera directly
over USB, so they need no vendor SDK and no driver installation.

```sh
npm install loupecam    # or: bun add loupecam
```

The prebuilt addons cover Windows (x64, arm64), Linux (x64 and arm64, glibc) and
macOS (arm64, x64). Node.js 22 or later is supported, and Bun works too. TypeScript
typings are included.

## Example

```js
import { open } from 'loupecam'

const cam = await open() // the first camera found
console.log(cam.info) // { model: { name: 'MU1803-HS', … }, serial: '…', … }

// Size 0 is the largest (4912×3684 on the MU1803); raw12 gives 12-bit samples.
const stream = await cam.start({ size: 0, mode: 'raw12', exposureUs: 30_000 })
const frame = await stream.read(2000)

const mosaic = frame.raw() // Uint16Array: the sensor's Bayer data
const image = await frame.develop() // { width, height, channels: 3, depth: 8, data: Uint8Array }
await frame.save('photo.png', { whiteBalance: 'auto', demosaic: 'mhc' })
await frame.save('photo16.tif', { depth: 16 })
await frame.save('mosaic.tif', { raw: true }) // undeveloped, for raw tools

await cam.close()
```

Reading frames continuously:

```js
await using cam = await open() // closed at the end of the scope (Node 24+)
const stream = await cam.start({ size: 2 }) // 1228×922, the fastest
for await (const frame of stream) {
  // runs until the stream ends
  show(await frame.develop({ demosaic: 'superpixel' }))
}
```

`stream.latest()` skips frames that queued up while you were busy, which suits a live
view.

Anything that blocks runs on libuv's thread pool and returns a Promise. That includes
opening the camera, starting, stopping and changing settings, waiting for frames,
developing, encoding and saving. A `read()` with no timeout holds a pool thread until a
frame arrives. Node's pool has 4 threads by default (`UV_THREADPOOL_SIZE`), so prefer
timeouts if you run many streams at once.

### Without a camera

`openSimulated()` gives a simulated MU1803-HS that produces synthetic frames. It has
the same API, so it's useful for tests and for writing code before the hardware
arrives.

## API

The typings ([`native.d.ts`](native.d.ts), [`index.d.ts`](index.d.ts)) document every
call and option.

| | |
| --- | --- |
| `listCameras()` | Connected cameras (`CameraDevice`: model, id, bus, port chain) |
| `open(device?)` | The first camera, or a `CameraDevice`, an index, or an id |
| `openSimulated()` | The simulator |
| `camera.start({ size, mode, exposureUs, gain, speed, binning, roi })` | Start streaming and resolve with a `Stream` |
| `camera.setExposure / setGain / setSpeed / setBinning / setRoi` | Change settings while streaming |
| `camera.info`, `.model`, `.geometry`, `.stats` | Identity, sizes, the stream's timing and its counters |
| `stream.read(timeoutMs?)`, `.latest(timeoutMs?)`, `.tryRead()` | Frames, and `for await` |
| `frame.raw()` | The mosaic: a `Uint8Array`, or a `Uint16Array` for 12-bit |
| `frame.develop(options?)` | An RGB (or monochrome) `Image` |
| `frame.encode(format?, options?)`, `frame.save(path, options?)` | PNG, TIFF (8/16-bit) or JPEG |
| `frame.whiteBalance(region?)`, `frame.stats(region?)` | Grey-world gains, means and the histogram |

The develop options all default to the camera model's own settings:

| Option | Values |
| --- | --- |
| `demosaic` | `'superpixel'`, `'bilinear'` (default) or `'mhc'` |
| `whiteBalance` | `'daylight'` (default), `'auto'`, `'none'` or `[r, g, b]` |
| `colorMatrix` | `true` (default: the model's), `false`, or a 3×3 matrix |
| `tone` | `'srgb'` (default), `'linear'` or a gamma value |
| `blackPoint`, `whitePoint`, `brightness`, `contrast` | levels |
| `blackLevel` | in sample units |
| `monochrome`, `negative` | |
| `flipHorizontal`, `flipVertical`, `rotation` | orientation |
| `depth` | `8` or `16` |

`encode()` and `save()` also take `quality` (JPEG) and `raw`.

Errors carry a `code`:

| Code | Meaning |
| --- | --- |
| `ERR_LOUPECAM_STREAM_STOPPED` | The stream ended (stopped, restarted or closed) |
| `ERR_LOUPECAM_TIMEOUT` | No frame within the timeout |
| `ERR_INVALID_ARG_VALUE` | A bad option value |
| `ERR_LOUPECAM` | Anything else (USB, camera, encoding, closed camera) |

Methods that return Promises reject on bad values rather than throwing.

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

You need [Rust](https://rustup.rs) and [Bun](https://bun.sh). From `bindings/node`:

```sh
bun install
bun run build      # native.js, native.d.ts and loupecam.<platform>.node
bun run test       # on Node, against the simulated camera
bun run typecheck
```

## License

MIT OR Apache-2.0.

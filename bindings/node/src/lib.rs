//! Node.js bindings (napi-rs): the native half of the `loupecam` npm package.
//!
//! `index.js` re-exports this module (loaded through the generated `native.js`) and
//! adds `Stream[Symbol.asyncIterator]` and `Camera[Symbol.asyncDispose]`. Anything
//! that blocks (USB I/O, waiting for frames, developing, encoding, writing files) runs
//! on the libuv thread pool and returns a Promise.
//!
//! Errors carry a `code`: `ERR_LOUPECAM_STREAM_STOPPED`, `ERR_LOUPECAM_TIMEOUT`,
//! `ERR_INVALID_ARG_VALUE` for bad options, and `ERR_LOUPECAM` for everything else.

use loupecam::{AnyCamera, Gain, Model, RawFrame, RecvError, Roi, SampleFormat, StreamConfig};
use loupecam_bindings::{self as common, ColorMatrix, Developed, DevelopOptions as Options, WhiteBalance};
use loupecam_isp::stats::Rect;
use loupecam_isp::tone::Transfer;
use napi::bindgen_prelude::*;
use napi_derive::napi;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const ERR_LOUPECAM: &str = "ERR_LOUPECAM";
const ERR_STOPPED: &str = "ERR_LOUPECAM_STREAM_STOPPED";
const ERR_TIMEOUT: &str = "ERR_LOUPECAM_TIMEOUT";
const ERR_ARG: &str = "ERR_INVALID_ARG_VALUE";

/// An error `code`, for synchronous calls (they return `Result<T, Code>`).
pub struct Code(&'static str);

impl AsRef<str> for Code {
    fn as_ref(&self) -> &str {
        self.0
    }
}

/// What the worker-thread side of a call returns.
type Outcome<T> = std::result::Result<T, Fail>;

/// A failure, carried out of a worker thread and thrown as a JS error with a `code`.
pub struct Fail {
    code: &'static str,
    message: String,
}

impl Fail {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Fail { code, message: message.into() }
    }
}

impl From<RecvError> for Fail {
    fn from(e: RecvError) -> Self {
        match e {
            RecvError::Stopped => Fail::new(ERR_STOPPED, "the stream stopped"),
            RecvError::Timeout => Fail::new(ERR_TIMEOUT, "timed out waiting for a frame"),
        }
    }
}

impl From<loupecam::Error> for Fail {
    fn from(e: loupecam::Error) -> Self {
        match e {
            loupecam::Error::Recv(r) => r.into(),
            e => Fail::new(ERR_LOUPECAM, e.to_string()),
        }
    }
}

impl From<common::Error> for Fail {
    fn from(e: common::Error) -> Self {
        match e {
            common::Error::InvalidArgument(m) => Fail::new(ERR_ARG, m),
            e => Fail::new(ERR_LOUPECAM, e.to_string()),
        }
    }
}

impl From<std::io::Error> for Fail {
    fn from(e: std::io::Error) -> Self {
        Fail::new(ERR_LOUPECAM, e.to_string())
    }
}

impl From<Fail> for napi::Error<Code> {
    fn from(f: Fail) -> Self {
        napi::Error::new(Code(f.code), f.message)
    }
}

/// Throw `f` from a Promise: a JS `Error` with `code` set.
fn reject(env: &Env, f: Fail) -> napi::Error {
    let make = || -> napi::Result<napi::Error> {
        let mut e = env.create_error(napi::Error::from_reason(f.message.clone()))?;
        e.set_named_property("code", f.code)?;
        Ok(napi::Error::from(e.to_unknown()))
    };
    make().unwrap_or_else(|e| e)
}

/// Blocking work for the thread pool: `job` runs there, and `to_js` converts its
/// result back on the JS thread.
pub struct Work<T, J> {
    job: Option<Box<dyn FnOnce() -> Outcome<T> + Send>>,
    to_js: fn(T) -> J,
}

impl<T: Send + 'static, J: ToNapiValue + TypeName> Task for Work<T, J> {
    type Output = Outcome<T>;
    type JsValue = J;

    fn compute(&mut self) -> napi::Result<Self::Output> {
        Ok(self.job.take().expect("a task runs once")())
    }

    fn resolve(&mut self, env: Env, out: Self::Output) -> napi::Result<J> {
        out.map(self.to_js).map_err(|f| reject(&env, f))
    }
}

fn work<T: Send + 'static, J: ToNapiValue + TypeName>(job: impl FnOnce() -> Outcome<T> + Send + 'static, to_js: fn(T) -> J) -> AsyncTask<Work<T, J>> {
    AsyncTask::new(Work { job: Some(Box::new(job)), to_js })
}

/// A Promise that rejects with `f`. Methods that return Promises reject on bad
/// arguments rather than throwing, so callers have one error path.
fn failed<T: Send + 'static, J: ToNapiValue + TypeName>(f: Fail) -> AsyncTask<Work<T, J>> {
    work(move || Err(f), |_| unreachable!())
}

/// Validate synchronously, then run the returned task, or reject with the failure.
fn promise<T: Send + 'static, J: ToNapiValue + TypeName>(setup: impl FnOnce() -> Outcome<AsyncTask<Work<T, J>>>) -> AsyncTask<Work<T, J>> {
    setup().unwrap_or_else(failed)
}

fn same<T>(v: T) -> T {
    v
}

// --- Plain objects --------------------------------------------------------------------

#[napi(object)]
pub struct Size {
    pub width: u32,
    pub height: u32,
}

/// A rectangle in pixels.
#[napi(object)]
#[derive(Clone, Copy)]
pub struct Region {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl From<Region> for Rect {
    fn from(r: Region) -> Self {
        Rect { x: r.x, y: r.y, width: r.width, height: r.height }
    }
}

/// A camera model: its sensor, sizes and colour filter.
#[napi(object)]
pub struct ModelInfo {
    pub name: String,
    pub vendor_id: u32,
    pub product_id: u32,
    /// Streamable sizes, largest first. The index is `StartOptions.size`.
    pub resolutions: Vec<Size>,
    /// Pixel pitch in micrometres.
    pub pixel_size_um: f64,
    /// The mosaic's top-left 2×2 cell, e.g. `"RGGB"`; absent for monochrome sensors.
    pub bayer_pattern: Option<String>,
    /// Significant bits per sample in the `raw12` mode.
    pub max_bit_depth: u32,
}

fn model_info(m: &Model) -> ModelInfo {
    ModelInfo {
        name: m.name.into(),
        vendor_id: m.vendor_id.into(),
        product_id: m.product_id.into(),
        resolutions: m.resolutions.iter().map(|r| Size { width: r.width, height: r.height }).collect(),
        pixel_size_um: m.pixel_size_um.into(),
        bayer_pattern: common::pattern_name(m).map(Into::into),
        max_bit_depth: m.max_bit_depth.into(),
    }
}

/// A connected camera that isn't open yet. Pass it (or its `id`) to `open()`.
#[napi(object)]
pub struct CameraDevice {
    pub model: ModelInfo,
    /// Platform-specific identifier, stable while the camera stays plugged in.
    pub id: String,
    pub bus: String,
    pub port_chain: Vec<u32>,
}

/// Identity and versions read from an open camera.
#[napi(object)]
pub struct CameraInfo {
    pub model: ModelInfo,
    pub serial: String,
    pub firmware_version: String,
    pub hardware_version: String,
    /// `"major.minor"`.
    pub fpga_version: String,
    pub sensor_id: u32,
    /// `YYYY-MM-DD`, when the camera records it.
    pub production_date: Option<String>,
}

/// Geometry and timing of a running stream.
#[napi(object)]
pub struct Geometry {
    pub width: u32,
    pub height: u32,
    /// Significant bits per sample (8 or 12).
    pub bits: u32,
    /// The exposure actually programmed, in µs.
    pub exposure_us: f64,
    /// The highest frame rate this geometry allows at the current exposure.
    pub max_fps: f64,
}

fn geometry(g: loupecam::StreamGeometry) -> Geometry {
    let g = common::Geometry::from(g);
    Geometry { width: g.width, height: g.height, bits: g.bits.into(), exposure_us: g.exposure_us, max_fps: g.max_fps }
}

/// Counters for a running stream.
#[napi(object)]
pub struct StreamStats {
    pub frames: f64,
    /// Frames dropped because the reader didn't keep up (the queue holds 3).
    pub dropped: f64,
    /// Transfers or partial frames discarded while (re)synchronising.
    pub discarded: f64,
    pub bytes: f64,
    pub transfer_errors: f64,
    /// Frame rate over the last second.
    pub fps: f64,
    /// Frames missing according to the camera's sequence counter.
    pub sequence_gaps: f64,
}

fn stream_stats(s: loupecam::StreamStats) -> StreamStats {
    StreamStats {
        frames: s.frames as f64,
        dropped: s.dropped as f64,
        discarded: s.discarded as f64,
        bytes: s.bytes as f64,
        transfer_errors: s.transfer_errors as f64,
        fps: s.fps,
        sequence_gaps: s.sequence_gaps as f64,
    }
}

/// Statistics of a frame's raw mosaic.
#[napi(object)]
pub struct FrameStats {
    /// Mean linear level of R, G and B, 0 to 1.
    pub mean: Vec<f64>,
    /// Mean luma (Rec. 709) after white balance, 0 to 1.
    pub luma: f64,
    /// Fraction of 2×2 cells with a channel at or near full scale.
    pub clipped: f64,
    /// 256-bin histogram of per-cell linear luma.
    pub histogram: Vec<u32>,
}

/// A developed image: interleaved RGB (or one channel with `monochrome`), rows top
/// to bottom.
#[napi(object)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    /// 3, or 1 with `monochrome`.
    pub channels: u32,
    /// Bits per channel: 8 (`Uint8Array`) or 16 (`Uint16Array`).
    pub depth: u32,
    pub data: Either<Uint8Array, Uint16Array>,
}

fn image(d: Developed) -> Image {
    let (width, height, channels) = d.shape();
    let (depth, data) = match d {
        Developed::Eight(i) => (8, Either::A(Uint8Array::new(i.data))),
        Developed::Sixteen(i) => (16, Either::B(Uint16Array::new(i.data))),
    };
    Image { width, height, channels: channels.into(), depth, data }
}

/// Options for `Camera.start()`.
#[napi(object)]
pub struct StartOptions {
    /// Index into `model.resolutions` (0, the default, is the largest).
    pub size: Option<u32>,
    /// `"raw8"` (default), `"raw12"` (12-bit samples) or `"processed8"` (the camera's
    /// own ISP).
    #[napi(ts_type = "'raw8' | 'raw12' | 'processed8'")]
    pub mode: Option<String>,
    /// Exposure in µs (default 20000).
    pub exposure_us: Option<u32>,
    /// Gain multiplier (default 1).
    pub gain: Option<f64>,
    /// Readout speed, 0 (slowest) to 3 (fastest, the default).
    pub speed: Option<u32>,
    pub binning: Option<bool>,
    /// Stream only this region of the size.
    pub roi: Option<Region>,
}

/// The develop options, shared by `develop()` and (with `quality` and `raw`)
/// `encode()` and `save()`.
macro_rules! develop_options {
    ($($doc:literal)* $name:ident { $($($fdoc:literal)* $field:ident: Option<$ty:ty>,)* }) => {
        $(#[doc = $doc])*
        #[napi(object)]
        pub struct $name {
            /// `"superpixel"` (half size, fastest), `"bilinear"` (default) or `"mhc"`
            /// (Malvar-He-Cutler: sharpest, about twice the cost).
            #[napi(ts_type = "'superpixel' | 'bilinear' | 'mhc'")]
            pub demosaic: Option<String>,
            /// `"daylight"` (the model's default), `"auto"` (grey world), `"none"`, or
            /// `[r, g, b]` gains.
            #[napi(ts_type = "'daylight' | 'auto' | 'none' | [number, number, number]")]
            pub white_balance: Option<Either<String, Vec<f64>>>,
            /// `true` (default) for the model's matrix, `false` for none, or a 3×3
            /// row-major matrix applied to linear camera RGB.
            #[napi(ts_type = "boolean | number[][]")]
            pub color_matrix: Option<Either<bool, Vec<Vec<f64>>>>,
            /// `"srgb"` (default), `"linear"`, or a gamma value.
            #[napi(ts_type = "'srgb' | 'linear' | number")]
            pub tone: Option<Either<String, f64>>,
            /// Input level (0-1) mapped to black.
            pub black_point: Option<f64>,
            /// Input level (0-1) mapped to white.
            pub white_point: Option<f64>,
            /// -1 to 1.
            pub brightness: Option<f64>,
            /// -1 to 1.
            pub contrast: Option<f64>,
            /// Sensor black level to subtract, in the frame's sample units.
            pub black_level: Option<u32>,
            /// A single luminance channel.
            pub monochrome: Option<bool>,
            pub negative: Option<bool>,
            pub flip_horizontal: Option<bool>,
            /// Relative to upright (the MU1803's bottom-up readout is already corrected).
            pub flip_vertical: Option<bool>,
            /// Clockwise degrees, applied after flips.
            #[napi(ts_type = "0 | 90 | 180 | 270")]
            pub rotation: Option<u32>,
            /// Output bits per channel (default 8).
            #[napi(ts_type = "8 | 16")]
            pub depth: Option<u32>,
            // (Spelled out: napi-derive only sees `Option` in the tokens it's given.)
            $($(#[doc = $fdoc])* pub $field: Option<$ty>,)*
        }

        impl $name {
            fn options(&self) -> Outcome<Options> {
                let mut o = Options::default();
                if let Some(d) = &self.demosaic {
                    o.demosaic = common::demosaic_named(d)?;
                }
                match &self.white_balance {
                    None => {}
                    Some(Either::A(name)) => o.white_balance = name.parse()?,
                    Some(Either::B(g)) => o.white_balance = WhiteBalance::Gains(triple(g, "whiteBalance")?),
                }
                match &self.color_matrix {
                    None | Some(Either::A(true)) => {}
                    Some(Either::A(false)) => o.color_matrix = ColorMatrix::Identity,
                    Some(Either::B(rows)) => o.color_matrix = ColorMatrix::Custom(matrix(rows)?),
                }
                match &self.tone {
                    None => {}
                    Some(Either::A(name)) => o.transfer = common::transfer_named(name)?,
                    Some(Either::B(g)) => o.transfer = Transfer::Gamma(*g as f32),
                }
                let f = |v: Option<f64>, d: f32| v.map_or(d, |v| v as f32);
                o.black_point = f(self.black_point, o.black_point);
                o.white_point = f(self.white_point, o.white_point);
                o.brightness = f(self.brightness, o.brightness);
                o.contrast = f(self.contrast, o.contrast);
                o.black_level = self.black_level.unwrap_or(0);
                o.monochrome = self.monochrome.unwrap_or(false);
                o.negative = self.negative.unwrap_or(false);
                o.flip_horizontal = self.flip_horizontal.unwrap_or(false);
                o.flip_vertical = self.flip_vertical.unwrap_or(false);
                o.rotation = self.rotation.unwrap_or(0);
                o.depth = self.depth.unwrap_or(8).min(255) as u8;
                o.validate()?;
                Ok(o)
            }
        }
    };
}

develop_options! {
    " Options for `Frame.develop()`. Every option defaults to the camera model's own"
    " settings: its colour matrix and daylight white balance, bilinear demosaicing,"
    " the sRGB curve, upright."
    DevelopOptions {}
}

develop_options! {
    " Options for `Frame.encode()` and `Frame.save()`: the develop options, plus"
    " `quality` and `raw`."
    EncodeOptions {
        " JPEG quality, 1-100 (default 90)."
        quality: Option<u32>,
        " Save the undeveloped mosaic, scaled to 16 bits (TIFF only); the develop"
        " options are ignored."
        raw: Option<bool>,
    }
}

impl EncodeOptions {
    fn quality(&self) -> u8 {
        self.quality.unwrap_or(90).clamp(1, 100) as u8
    }
}

fn triple(v: &[f64], what: &str) -> Outcome<[f32; 3]> {
    match v {
        [a, b, c] => Ok([*a as f32, *b as f32, *c as f32]),
        _ => Err(Fail::new(ERR_ARG, format!("{what} needs 3 numbers"))),
    }
}

fn matrix(rows: &[Vec<f64>]) -> Outcome<[[f32; 3]; 3]> {
    match rows {
        [a, b, c] => Ok([triple(a, "colorMatrix")?, triple(b, "colorMatrix")?, triple(c, "colorMatrix")?]),
        _ => Err(Fail::new(ERR_ARG, "colorMatrix needs 3 rows")),
    }
}

// --- Cameras --------------------------------------------------------------------------

type Shared = Arc<Mutex<Option<AnyCamera>>>;

/// Run `f` on the camera (on the thread pool, where its USB I/O can block).
fn on_camera<T: Send + 'static>(cam: &Shared, f: impl FnOnce(&mut AnyCamera) -> Outcome<T> + Send + 'static) -> impl FnOnce() -> Outcome<T> + Send + 'static {
    let cam = cam.clone();
    move || {
        let mut g = cam.lock().unwrap_or_else(|e| e.into_inner());
        f(g.as_mut().ok_or_else(|| Fail::new(ERR_LOUPECAM, "the camera is closed"))?)
    }
}

/// An open camera. `close()` it when done (or use `await using`).
#[napi]
pub struct Camera {
    cam: Shared,
    model: &'static Model,
}

impl Camera {
    fn new(cam: AnyCamera) -> Self {
        Camera { model: cam.model(), cam: Arc::new(Mutex::new(Some(cam))) }
    }

    /// A quick read of the open camera (these don't do I/O).
    fn peek<T>(&self, f: impl FnOnce(&AnyCamera) -> T) -> Result<T, Code> {
        let g = self.cam.lock().unwrap_or_else(|e| e.into_inner());
        g.as_ref().map(f).ok_or_else(|| Fail::new(ERR_LOUPECAM, "the camera is closed").into())
    }
}

#[napi]
impl Camera {
    #[napi(getter)]
    pub fn info(&self) -> Result<CameraInfo, Code> {
        self.peek(|c| {
            let i = c.info();
            CameraInfo {
                model: model_info(i.model),
                serial: i.serial.clone(),
                firmware_version: i.firmware_version.clone(),
                hardware_version: i.hardware_version.clone(),
                fpga_version: format!("{}.{}", i.fpga_version.0, i.fpga_version.1),
                sensor_id: i.sensor_id.into(),
                production_date: i.production_date.map(|(y, m, d)| format!("{y:04}-{m:02}-{d:02}")),
            }
        })
    }

    #[napi(getter)]
    pub fn model(&self) -> ModelInfo {
        model_info(self.model)
    }

    /// Whether this is the simulated camera.
    #[napi(getter)]
    pub fn simulated(&self) -> Result<bool, Code> {
        self.peek(|c| c.is_simulated())
    }

    #[napi(getter)]
    pub fn streaming(&self) -> Result<bool, Code> {
        self.peek(|c| c.is_streaming())
    }

    #[napi(getter)]
    pub fn closed(&self) -> bool {
        self.cam.lock().unwrap_or_else(|e| e.into_inner()).is_none()
    }

    /// The running stream's geometry, or `null` when not streaming.
    #[napi(getter)]
    pub fn geometry(&self) -> Result<Option<Geometry>, Code> {
        self.peek(|c| c.geometry().map(geometry))
    }

    #[napi(getter)]
    pub fn stats(&self) -> Result<Option<StreamStats>, Code> {
        self.peek(|c| c.stats().map(stream_stats))
    }

    /// The requested exposure in µs, while streaming.
    #[napi(getter)]
    pub fn exposure_us(&self) -> Result<Option<u32>, Code> {
        self.peek(|c| c.config().map(|c| c.exposure_us))
    }

    /// The gain multiplier, while streaming.
    #[napi(getter)]
    pub fn gain(&self) -> Result<Option<f64>, Code> {
        self.peek(|c| c.config().map(|c| c.gain.multiplier().into()))
    }

    /// The largest multiplier `setGain()` accepts.
    #[napi]
    pub fn max_gain() -> f64 {
        Gain::max_multiplier().into()
    }

    /// Start streaming. Starting again reconfigures the camera, and earlier streams
    /// end.
    #[napi(ts_return_type = "Promise<Stream>")]
    pub fn start(&self, options: Option<StartOptions>) -> AsyncTask<Work<Stream, Stream>> {
        promise(|| {
            let o = options.unwrap_or(StartOptions { size: None, mode: None, exposure_us: None, gain: None, speed: None, binning: None, roi: None });
            let mode = o.mode.as_deref().map(common::pixel_mode_named).transpose()?;
            let mut cfg = StreamConfig::new(o.size.unwrap_or(0) as usize, mode.unwrap_or(loupecam::PixelMode::Raw8));
            cfg.exposure_us = o.exposure_us.unwrap_or(cfg.exposure_us);
            if let Some(g) = o.gain {
                cfg.gain = Gain::for_multiplier(g as f32).map_err(|e| Fail::new(ERR_ARG, e.to_string()))?;
            }
            cfg.speed = o.speed.map_or(cfg.speed, |s| s.min(255) as u8);
            cfg.binning = o.binning.unwrap_or(false);
            cfg.roi = o.roi.map(roi).transpose()?;
            let model = self.model;
            let job = on_camera(&self.cam, move |c| Ok(c.start(&cfg)?));
            Ok(work(move || job().map(|rx| Stream { rx: Arc::new(rx), model }), same))
        })
    }

    /// Stop streaming (the camera stays open).
    #[napi(ts_return_type = "Promise<void>")]
    pub fn stop(&self) -> AsyncTask<Work<(), ()>> {
        work(on_camera(&self.cam, |c| Ok(c.stop()?)), same)
    }

    /// Stop streaming and release the camera. Closing twice is harmless.
    #[napi(ts_return_type = "Promise<void>")]
    pub fn close(&self) -> AsyncTask<Work<(), ()>> {
        let cam = self.cam.clone();
        work(
            move || {
                drop(cam.lock().unwrap_or_else(|e| e.into_inner()).take());
                Ok(())
            },
            same,
        )
    }

    /// Set the exposure in µs; resolves with the exposure actually programmed.
    #[napi(ts_return_type = "Promise<number>")]
    pub fn set_exposure(&self, exposure_us: u32) -> AsyncTask<Work<f64, f64>> {
        work(on_camera(&self.cam, move |c| Ok(c.set_exposure(exposure_us)?)), same)
    }

    /// Set the gain as a multiplier (1 is unity, up to `Camera.maxGain()`).
    #[napi(ts_return_type = "Promise<void>")]
    pub fn set_gain(&self, gain: f64) -> AsyncTask<Work<(), ()>> {
        promise(|| {
            let g = Gain::for_multiplier(gain as f32).map_err(|e| Fail::new(ERR_ARG, e.to_string()))?;
            Ok(work(on_camera(&self.cam, move |c| Ok(c.set_gain(g)?)), same))
        })
    }

    /// Readout speed, 0 (slowest) to 3 (fastest).
    #[napi(ts_return_type = "Promise<Geometry>")]
    pub fn set_speed(&self, speed: u32) -> AsyncTask<Work<loupecam::StreamGeometry, Geometry>> {
        let speed = speed.min(255) as u8;
        work(on_camera(&self.cam, move |c| Ok(c.set_speed(speed)?)), geometry)
    }

    #[napi(ts_return_type = "Promise<void>")]
    pub fn set_binning(&self, binning: bool) -> AsyncTask<Work<(), ()>> {
        work(on_camera(&self.cam, move |c| Ok(c.set_binning(binning)?)), same)
    }

    /// Stream only a region of the current size, or the whole frame with `null`.
    #[napi(ts_return_type = "Promise<Geometry>")]
    pub fn set_roi(&self, roi_: Option<Region>) -> AsyncTask<Work<loupecam::StreamGeometry, Geometry>> {
        promise(|| {
            let r = roi_.map(roi).transpose()?;
            Ok(work(on_camera(&self.cam, move |c| Ok(c.set_roi(r)?)), geometry))
        })
    }
}

fn roi(r: Region) -> Outcome<Roi> {
    let n = |v: u32| u16::try_from(v).map_err(|_| Fail::new(ERR_ARG, format!("roi value {v} is too large")));
    Ok(Roi { x: n(r.x)?, y: n(r.y)?, width: n(r.width)?, height: n(r.height)? })
}

/// Connected cameras of supported models.
#[napi]
pub fn list_cameras() -> Result<Vec<CameraDevice>, Code> {
    let devices = loupecam::list().map_err(Fail::from)?;
    Ok(devices
        .into_iter()
        .map(|d| CameraDevice { model: model_info(d.model), id: d.id, bus: d.bus, port_chain: d.port_chain.into_iter().map(Into::into).collect() })
        .collect())
}

/// Open a camera: the first one found, or one from `listCameras()` (the device, its
/// index, or its `id`).
#[napi(ts_args_type = "device?: CameraDevice | number | string", ts_return_type = "Promise<Camera>")]
pub fn open(device: Option<Either3<u32, String, CameraDevice>>) -> AsyncTask<Work<AnyCamera, Camera>> {
    work(
        move || {
            let Some(device) = device else { return Ok(AnyCamera::open_first()?) };
            let devices = loupecam::list()?;
            let found = match &device {
                Either3::A(i) => devices.into_iter().nth(*i as usize),
                Either3::B(id) | Either3::C(CameraDevice { id, .. }) => devices.into_iter().find(|d| &d.id == id),
            };
            let dev = found.ok_or_else(|| Fail::new(ERR_LOUPECAM, "no such camera"))?;
            Ok(AnyCamera::open(&dev)?)
        },
        Camera::new,
    )
}

/// A simulated MU1803-HS: synthetic frames, no hardware needed.
#[napi]
pub fn open_simulated() -> Camera {
    Camera::new(AnyCamera::simulated())
}

// --- Streams and frames ---------------------------------------------------------------

/// Frames from a running camera. `for await (const frame of stream)` reads until the
/// stream ends.
#[napi]
pub struct Stream {
    rx: Arc<loupecam::FrameReceiver>,
    model: &'static Model,
}

#[napi]
impl Stream {
    /// The next frame, waiting up to `timeoutMs` (forever if omitted). Rejects with
    /// code `ERR_LOUPECAM_TIMEOUT`, or `ERR_LOUPECAM_STREAM_STOPPED` once the stream
    /// has ended.
    #[napi(ts_return_type = "Promise<Frame>")]
    pub fn read(&self, timeout_ms: Option<u32>) -> AsyncTask<Work<Frame, Frame>> {
        let (rx, model) = (self.rx.clone(), self.model);
        work(
            move || {
                let raw = match timeout_ms {
                    Some(t) => rx.recv_timeout(Duration::from_millis(t.into()))?,
                    None => rx.recv()?,
                };
                Ok(Frame { raw: Arc::new(raw), model })
            },
            same,
        )
    }

    /// The newest frame, discarding older queued ones (for live views). Waits up to
    /// `timeoutMs` (default 1000).
    #[napi(ts_return_type = "Promise<Frame>")]
    pub fn latest(&self, timeout_ms: Option<u32>) -> AsyncTask<Work<Frame, Frame>> {
        let (rx, model) = (self.rx.clone(), self.model);
        let t = Duration::from_millis(timeout_ms.unwrap_or(1000).into());
        work(move || Ok(Frame { raw: Arc::new(rx.recv_latest(t)?), model }), same)
    }

    /// A frame if one is ready, else `null`.
    #[napi]
    pub fn try_read(&self) -> Option<Frame> {
        self.rx.try_recv().map(|raw| Frame { raw: Arc::new(raw), model: self.model })
    }

    #[napi(getter)]
    pub fn stats(&self) -> StreamStats {
        stream_stats(self.rx.stats())
    }
}

/// One frame: the raw Bayer mosaic as the sensor delivered it.
#[napi]
pub struct Frame {
    raw: Arc<RawFrame>,
    model: &'static Model,
}

#[napi]
impl Frame {
    #[napi(getter)]
    pub fn width(&self) -> u32 {
        self.raw.width
    }

    #[napi(getter)]
    pub fn height(&self) -> u32 {
        self.raw.height
    }

    /// Significant bits per sample: 8, or 12 in the `raw12` mode.
    #[napi(getter)]
    pub fn bits(&self) -> u32 {
        self.raw.format.bits().into()
    }

    /// The camera's frame counter.
    #[napi(getter)]
    pub fn sequence(&self) -> u32 {
        self.raw.trailer.sequence
    }

    /// The mosaic's top-left 2×2 cell, e.g. `"RGGB"`.
    #[napi(getter)]
    pub fn bayer_pattern(&self) -> Option<String> {
        common::pattern_name(self.model).map(Into::into)
    }

    #[napi(getter)]
    pub fn model(&self) -> ModelInfo {
        model_info(self.model)
    }

    /// The raw mosaic, row-major, `width × height` samples: a `Uint8Array`, or a
    /// `Uint16Array` holding `bits` significant bits. Rows are as the sensor sends them
    /// (bottom-up on the MU1803; `develop()` turns them upright). Each call copies.
    #[napi]
    pub fn raw(&self) -> Either<Uint8Array, Uint16Array> {
        match self.raw.format {
            SampleFormat::U8 => Either::A(Uint8Array::new(self.raw.data.clone())),
            SampleFormat::U16 { .. } => Either::B(Uint16Array::new(common::samples_u16(&self.raw))),
        }
    }

    /// Develop into an image (RGB unless `monochrome`).
    #[napi(ts_return_type = "Promise<Image>")]
    pub fn develop(&self, options: Option<DevelopOptions>) -> AsyncTask<Work<Developed, Image>> {
        promise(|| {
            let o = options.map(|o| o.options()).transpose()?.unwrap_or_default();
            let (raw, model) = (self.raw.clone(), self.model);
            Ok(work(move || Ok(o.develop(&raw, model)?), image))
        })
    }

    /// Develop and encode as `"png"` (default), `"tiff"` or `"jpeg"`.
    #[napi(ts_args_type = "format?: 'png' | 'tiff' | 'tif' | 'jpeg' | 'jpg', options?: EncodeOptions", ts_return_type = "Promise<Buffer>")]
    pub fn encode(&self, format: Option<String>, options: Option<EncodeOptions>) -> AsyncTask<Work<Vec<u8>, Buffer>> {
        promise(|| {
            let (o, quality, raw_) = encode_options(options.as_ref())?;
            let format = common::format_named(format.as_deref().unwrap_or("png"), quality)?;
            let (raw, model) = (self.raw.clone(), self.model);
            Ok(work(move || Ok(o.encode(&raw, model, format, raw_)?), Buffer::from))
        })
    }

    /// Develop and write a file; the format comes from the extension (`.png`,
    /// `.tif`/`.tiff`, `.jpg`/`.jpeg`).
    #[napi(ts_return_type = "Promise<void>")]
    pub fn save(&self, path: String, options: Option<EncodeOptions>) -> AsyncTask<Work<(), ()>> {
        promise(|| {
            let (o, quality, raw_) = encode_options(options.as_ref())?;
            let path = std::path::PathBuf::from(path);
            let format = common::format_for_path(&path, quality)?;
            let (raw, model) = (self.raw.clone(), self.model);
            Ok(work(move || Ok(std::fs::write(&path, o.encode(&raw, model, format, raw_)?)?), same))
        })
    }

    /// Grey-world white balance `[r, g, b]` gains. Over a neutral `region`, this is
    /// "click to white balance".
    #[napi(ts_return_type = "[number, number, number]")]
    pub fn white_balance(&self, region: Option<Region>) -> Vec<f64> {
        common::grey_world(&self.raw, self.model, region.map(Into::into)).map(Into::into).to_vec()
    }

    /// Raw statistics. `whiteBalance` weights the luma estimate (default: the model's
    /// daylight balance).
    #[napi(ts_args_type = "region?: Region | null, whiteBalance?: [number, number, number]")]
    pub fn stats(&self, region: Option<Region>, white_balance: Option<Vec<f64>>) -> Result<FrameStats, Code> {
        let wb = white_balance.as_deref().map(|g| triple(g, "whiteBalance")).transpose()?;
        let s = common::frame_stats(&self.raw, self.model, region.map(Into::into), wb);
        Ok(FrameStats {
            mean: s.mean.map(Into::into).to_vec(),
            luma: s.luma.into(),
            clipped: s.clipped.into(),
            histogram: s.histogram,
        })
    }
}

fn encode_options(o: Option<&EncodeOptions>) -> Outcome<(Options, u8, bool)> {
    Ok(match o {
        None => (Options::default(), 90, false),
        Some(o) => (o.options()?, o.quality(), o.raw.unwrap_or(false)),
    })
}

/// The package version (the Rust crate's, which follows the release tag).
#[napi]
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

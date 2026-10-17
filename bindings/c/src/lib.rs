//! The C API: `include/loupecam.h` is generated from this file by cbindgen.
//!
//! Conventions, for every function:
//! - The return value is a [`LoupecamStatus`]; on failure, `loupecam_last_error()`
//!   describes it (per thread).
//! - Handles are opaque, created by the library and freed with their `_free`
//!   function. Freeing `NULL` is a no-op. Data pointers a handle hands out stay valid
//!   until that handle is freed.
//! - Enum-valued fields and arguments are plain integers (so an out-of-range value from
//!   C is reported, not undefined behaviour); use the `LOUPECAM_…` constants.
//! - Strings are UTF-8 and NUL-terminated.
//! - Panics never cross the boundary: they become `LOUPECAM_STATUS_ERROR`.

#![allow(clippy::missing_safety_doc)] // documented once, above: pointers must be valid

use loupecam::{AnyCamera, Gain, Model, RawFrame, RecvError, Roi, StreamConfig};
use loupecam_bindings::{self as common, ColorMatrix, Developed, DevelopOptions, WhiteBalance};
use loupecam_isp::encode::Format;
use loupecam_isp::stats::Rect;
use loupecam_isp::tone::Transfer;
use loupecam_isp::Demosaic;
use std::cell::RefCell;
use std::ffi::{CStr, CString, c_char, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Mutex;
use std::time::Duration;

/// Result of every call.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoupecamStatus {
    Ok = 0,
    /// Anything not covered below (USB, camera, encoding, file I/O, internal).
    Error = 1,
    /// A NULL pointer where one isn't allowed, or a value out of range.
    InvalidArgument = 2,
    /// No frame within the timeout.
    Timeout = 3,
    /// The stream ended: the camera was stopped, restarted or freed.
    StreamStopped = 4,
    /// No camera (at that index).
    NotFound = 5,
    /// The call needs a running stream.
    NotStreaming = 6,
    /// Not available on this camera (e.g. the simulator).
    Unsupported = 7,
}

/// Pixel delivery modes (`LoupecamStartOptions.pixel_mode`).
#[repr(C)]
pub enum LoupecamPixelMode {
    /// Raw sensor data, 8 bits per sample.
    Raw8 = 0,
    /// Raw sensor data, 12 significant bits in 16-bit samples.
    Raw12 = 1,
    /// The camera's own ISP (white balance, colour matrix, tone), 8 bits, still a mosaic.
    Processed8 = 2,
}

/// Demosaic algorithms (`LoupecamDevelopOptions.demosaic`).
#[repr(C)]
pub enum LoupecamDemosaic {
    /// Bilinear interpolation (the default).
    Bilinear = 0,
    /// Each 2×2 cell becomes one pixel: half size, fastest.
    Superpixel = 1,
    /// Malvar-He-Cutler: sharpest, about twice the cost of bilinear.
    Mhc = 2,
}

/// White balance (`LoupecamDevelopOptions.white_balance`).
#[repr(C)]
pub enum LoupecamWhiteBalance {
    /// The model's daylight gains (the default).
    Daylight = 0,
    /// Grey world over the whole frame.
    Auto = 1,
    /// Unity gains.
    None = 2,
    /// `LoupecamDevelopOptions.white_balance_gains`.
    Gains = 3,
}

/// Colour matrix (`LoupecamDevelopOptions.color_matrix`).
#[repr(C)]
pub enum LoupecamColorMatrix {
    /// The model's own (the default).
    Model = 0,
    /// None.
    Identity = 1,
    /// `LoupecamDevelopOptions.color_matrix_values`, row-major.
    Custom = 2,
}

/// Tone curves (`LoupecamDevelopOptions.tone`).
#[repr(C)]
pub enum LoupecamTone {
    /// sRGB (the default).
    Srgb = 0,
    Linear = 1,
    /// A power law with `LoupecamDevelopOptions.gamma`.
    Gamma = 2,
}

/// Image file formats.
#[repr(C)]
pub enum LoupecamFormat {
    Png = 0,
    Tiff = 1,
    Jpeg = 2,
}

/// A rectangle in pixels. A zero width or height means "none" where a region is
/// optional.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct LoupecamRegion {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// Identity of an open camera and its model.
#[repr(C)]
pub struct LoupecamCameraInfo {
    pub model: [c_char; 32],
    pub serial: [c_char; 64],
    pub firmware_version: [c_char; 32],
    pub hardware_version: [c_char; 32],
    pub fpga_version_major: u8,
    pub fpga_version_minor: u8,
    pub sensor_id: u16,
    pub vendor_id: u16,
    pub product_id: u16,
    /// Number of sizes; see `loupecam_camera_resolution`.
    pub resolution_count: u32,
    /// Pixel pitch in micrometres.
    pub pixel_size_um: f32,
    /// Significant bits per sample in the RAW12 mode.
    pub max_bit_depth: u32,
    /// The mosaic's top-left 2×2 cell, e.g. "RGGB"; empty for monochrome sensors.
    pub bayer_pattern: [c_char; 8],
    pub simulated: bool,
}

/// Options for `loupecam_camera_start`; start from `loupecam_start_options_default()`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct LoupecamStartOptions {
    /// Index into the model's sizes (0, the default, is the largest).
    pub size: u32,
    /// A `LoupecamPixelMode` (default RAW8).
    pub pixel_mode: u32,
    /// Exposure in µs (default 20000).
    pub exposure_us: u32,
    /// Gain multiplier (default 1).
    pub gain: f64,
    /// Readout speed, 0 (slowest) to 3 (fastest, the default).
    pub speed: u32,
    pub binning: bool,
    /// Stream only this region of the size (zero width: the whole frame).
    pub roi: LoupecamRegion,
}

/// Develop options; start from `loupecam_develop_options_default()`, which gives the
/// camera model's own settings: its colour matrix and daylight white balance, bilinear
/// demosaicing, the sRGB curve, upright, 8 bits.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct LoupecamDevelopOptions {
    /// A `LoupecamDemosaic`.
    pub demosaic: u32,
    /// A `LoupecamWhiteBalance`.
    pub white_balance: u32,
    /// R, G, B gains for `LOUPECAM_WHITE_BALANCE_GAINS`.
    pub white_balance_gains: [f32; 3],
    /// A `LoupecamColorMatrix`.
    pub color_matrix: u32,
    /// Row-major 3×3 matrix for `LOUPECAM_COLOR_MATRIX_CUSTOM`, applied to linear
    /// camera RGB.
    pub color_matrix_values: [f32; 9],
    /// A `LoupecamTone`.
    pub tone: u32,
    /// The exponent for `LOUPECAM_TONE_GAMMA`.
    pub gamma: f32,
    /// Input level (0-1) mapped to black.
    pub black_point: f32,
    /// Input level (0-1) mapped to white.
    pub white_point: f32,
    /// -1 to 1.
    pub brightness: f32,
    /// -1 to 1.
    pub contrast: f32,
    /// Sensor black level to subtract, in the frame's sample units.
    pub black_level: u32,
    /// A single luminance channel.
    pub monochrome: bool,
    pub negative: bool,
    pub flip_horizontal: bool,
    /// Relative to upright (a bottom-up sensor readout is corrected regardless).
    pub flip_vertical: bool,
    /// Clockwise degrees: 0, 90, 180 or 270.
    pub rotation: u32,
    /// Output bits per channel: 8 or 16.
    pub depth: u32,
}

/// Geometry and timing of a running stream.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct LoupecamGeometry {
    pub width: u32,
    pub height: u32,
    /// Significant bits per sample (8 or 12).
    pub bits: u32,
    /// The exposure actually programmed, in µs.
    pub exposure_us: f64,
    /// The highest frame rate this geometry allows at that exposure.
    pub max_fps: f64,
}

/// Counters for a running stream.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct LoupecamStreamStats {
    pub frames: u64,
    /// Frames dropped because the reader didn't keep up (the queue holds 3).
    pub dropped: u64,
    /// Transfers or partial frames discarded while (re)synchronising.
    pub discarded: u64,
    pub bytes: u64,
    pub transfer_errors: u64,
    /// Frames missing according to the camera's sequence counter.
    pub sequence_gaps: u64,
    /// Frame rate over the last second.
    pub fps: f64,
}

/// A frame's layout.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct LoupecamFrameInfo {
    pub width: u32,
    pub height: u32,
    /// Significant bits per sample: 8, or 12 in RAW12.
    pub bits: u32,
    /// 1 (uint8_t samples) or 2 (little-endian uint16_t samples).
    pub bytes_per_sample: u32,
    /// The camera's frame counter.
    pub sequence: u32,
}

/// A developed image's layout. Pixels are interleaved, rows top to bottom, no padding.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct LoupecamImageInfo {
    pub width: u32,
    pub height: u32,
    /// 3 (RGB), or 1 with `monochrome`.
    pub channels: u32,
    /// Bits per channel: 8 (uint8_t) or 16 (uint16_t, native byte order).
    pub depth: u32,
}

/// Statistics of a frame's raw mosaic.
#[repr(C)]
pub struct LoupecamFrameStats {
    /// Mean linear level of R, G and B, 0 to 1.
    pub mean: [f32; 3],
    /// Mean luma (Rec. 709) after white balance, 0 to 1.
    pub luma: f32,
    /// Fraction of 2×2 cells with a channel at or near full scale.
    pub clipped: f32,
    /// 256-bin histogram of per-cell linear luma.
    pub histogram: [u32; 256],
}

/// An open camera.
pub struct LoupecamCamera {
    cam: Mutex<AnyCamera>,
    model: &'static Model,
}

/// Frames from a running camera.
pub struct LoupecamStream {
    rx: loupecam::FrameReceiver,
    model: &'static Model,
}

/// One frame: the raw Bayer mosaic as the sensor delivered it.
pub struct LoupecamFrame {
    raw: RawFrame,
    model: &'static Model,
}

/// A developed image.
pub struct LoupecamImage(Developed);

/// An encoded file in memory.
pub struct LoupecamBytes(Vec<u8>);

// --- Errors ---------------------------------------------------------------------------

struct Fail(LoupecamStatus, String);

impl Fail {
    fn arg(msg: impl Into<String>) -> Self {
        Fail(LoupecamStatus::InvalidArgument, msg.into())
    }
}

impl From<loupecam::Error> for Fail {
    fn from(e: loupecam::Error) -> Self {
        match e {
            loupecam::Error::Recv(r) => r.into(),
            loupecam::Error::NotFound => Fail(LoupecamStatus::NotFound, e.to_string()),
            loupecam::Error::Unsupported(_) => Fail(LoupecamStatus::Unsupported, e.to_string()),
            e => Fail(LoupecamStatus::Error, e.to_string()),
        }
    }
}

impl From<RecvError> for Fail {
    fn from(e: RecvError) -> Self {
        match e {
            RecvError::Stopped => Fail(LoupecamStatus::StreamStopped, "the stream stopped".into()),
            RecvError::Timeout => Fail(LoupecamStatus::Timeout, "timed out waiting for a frame".into()),
        }
    }
}

impl From<common::Error> for Fail {
    fn from(e: common::Error) -> Self {
        match e {
            common::Error::InvalidArgument(m) => Fail::arg(m),
            e => Fail(LoupecamStatus::Error, e.to_string()),
        }
    }
}

impl From<std::io::Error> for Fail {
    fn from(e: std::io::Error) -> Self {
        Fail(LoupecamStatus::Error, e.to_string())
    }
}

thread_local! {
    static LAST_ERROR: RefCell<CString> = RefCell::new(CString::default());
}

fn set_error(msg: &str) {
    let c = CString::new(msg.replace('\0', " ")).unwrap_or_default();
    LAST_ERROR.with(|e| *e.borrow_mut() = c);
}

/// Run `f`, turning failures and panics into a status (and the thread's last error).
fn guard(f: impl FnOnce() -> Result<(), Fail>) -> LoupecamStatus {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(Ok(())) => LoupecamStatus::Ok,
        Ok(Err(Fail(status, msg))) => {
            set_error(&msg);
            status
        }
        Err(panic) => {
            let msg = panic.downcast_ref::<&str>().copied().or_else(|| panic.downcast_ref::<String>().map(String::as_str)).unwrap_or("unknown");
            set_error(&format!("internal error: {msg}"));
            LoupecamStatus::Error
        }
    }
}

/// A required pointer argument.
unsafe fn arg<'a, T>(p: *const T, name: &str) -> Result<&'a T, Fail> {
    unsafe { p.as_ref() }.ok_or_else(|| Fail::arg(format!("{name} is NULL")))
}

/// A required output pointer.
unsafe fn out<'a, T>(p: *mut T, name: &str) -> Result<&'a mut T, Fail> {
    unsafe { p.as_mut() }.ok_or_else(|| Fail::arg(format!("{name} is NULL")))
}

fn handle<T>(v: T) -> *mut T {
    Box::into_raw(Box::new(v))
}

unsafe fn free<T>(p: *mut T) {
    if !p.is_null() {
        // Dropping can run USB I/O (stopping a stream); never unwind into C.
        let _ = catch_unwind(AssertUnwindSafe(|| drop(unsafe { Box::from_raw(p) })));
    }
}

/// Copy `s` into a fixed C string field, truncating at a character boundary.
fn copy_str(dst: &mut [c_char], s: &str) {
    let max = dst.len() - 1;
    let mut n = s.len().min(max);
    while !s.is_char_boundary(n) {
        n -= 1;
    }
    for (d, b) in dst.iter_mut().zip(s.as_bytes()[..n].iter()) {
        *d = *b as c_char;
    }
    dst[n] = 0;
}

// --- Library --------------------------------------------------------------------------

/// The library version, e.g. "0.3.0". Static; never NULL.
#[unsafe(no_mangle)]
pub extern "C" fn loupecam_version() -> *const c_char {
    static VERSION: &CStr = match CStr::from_bytes_with_nul(concat!(env!("CARGO_PKG_VERSION"), "\0").as_bytes()) {
        Ok(v) => v,
        Err(_) => c"unknown",
    };
    VERSION.as_ptr()
}

/// A description of the last failure on this thread. Valid until the next failing
/// call on this thread; never NULL (empty if nothing has failed).
#[unsafe(no_mangle)]
pub extern "C" fn loupecam_last_error() -> *const c_char {
    LAST_ERROR.with(|e| e.borrow().as_ptr())
}

/// The largest gain multiplier the camera accepts.
#[unsafe(no_mangle)]
pub extern "C" fn loupecam_max_gain() -> f64 {
    Gain::max_multiplier().into()
}

/// The defaults for `loupecam_camera_start`.
#[unsafe(no_mangle)]
pub extern "C" fn loupecam_start_options_default() -> LoupecamStartOptions {
    LoupecamStartOptions {
        size: 0,
        pixel_mode: LoupecamPixelMode::Raw8 as u32,
        exposure_us: 20_000,
        gain: 1.0,
        speed: 3,
        binning: false,
        roi: LoupecamRegion::default(),
    }
}

/// The defaults for developing: the camera model's own settings.
#[unsafe(no_mangle)]
pub extern "C" fn loupecam_develop_options_default() -> LoupecamDevelopOptions {
    let d = DevelopOptions::default();
    LoupecamDevelopOptions {
        demosaic: LoupecamDemosaic::Bilinear as u32,
        white_balance: LoupecamWhiteBalance::Daylight as u32,
        white_balance_gains: [1.0; 3],
        color_matrix: LoupecamColorMatrix::Model as u32,
        color_matrix_values: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
        tone: LoupecamTone::Srgb as u32,
        gamma: 2.2,
        black_point: d.black_point,
        white_point: d.white_point,
        brightness: d.brightness,
        contrast: d.contrast,
        black_level: 0,
        monochrome: false,
        negative: false,
        flip_horizontal: false,
        flip_vertical: false,
        rotation: 0,
        depth: 8,
    }
}

fn develop_options(o: Option<&LoupecamDevelopOptions>) -> Result<DevelopOptions, Fail> {
    let Some(o) = o else { return Ok(DevelopOptions::default()) };
    let m = &o.color_matrix_values;
    let options = DevelopOptions {
        demosaic: match o.demosaic {
            0 => Demosaic::Bilinear,
            1 => Demosaic::Superpixel,
            2 => Demosaic::Mhc,
            v => return Err(Fail::arg(format!("demosaic {v} isn't a LoupecamDemosaic"))),
        },
        white_balance: match o.white_balance {
            0 => WhiteBalance::Daylight,
            1 => WhiteBalance::Auto,
            2 => WhiteBalance::None,
            3 => WhiteBalance::Gains(o.white_balance_gains),
            v => return Err(Fail::arg(format!("white_balance {v} isn't a LoupecamWhiteBalance"))),
        },
        color_matrix: match o.color_matrix {
            0 => ColorMatrix::Model,
            1 => ColorMatrix::Identity,
            2 => ColorMatrix::Custom([[m[0], m[1], m[2]], [m[3], m[4], m[5]], [m[6], m[7], m[8]]]),
            v => return Err(Fail::arg(format!("color_matrix {v} isn't a LoupecamColorMatrix"))),
        },
        transfer: match o.tone {
            0 => Transfer::Srgb,
            1 => Transfer::Linear,
            2 => Transfer::Gamma(o.gamma),
            v => return Err(Fail::arg(format!("tone {v} isn't a LoupecamTone"))),
        },
        black_point: o.black_point,
        white_point: o.white_point,
        brightness: o.brightness,
        contrast: o.contrast,
        black_level: o.black_level,
        monochrome: o.monochrome,
        negative: o.negative,
        flip_horizontal: o.flip_horizontal,
        flip_vertical: o.flip_vertical,
        rotation: o.rotation,
        depth: u8::try_from(o.depth).unwrap_or(0),
    };
    options.validate()?;
    Ok(options)
}

fn to_rect(r: Option<&LoupecamRegion>) -> Option<Rect> {
    r.filter(|r| r.width > 0 && r.height > 0).map(|r| Rect { x: r.x, y: r.y, width: r.width, height: r.height })
}

fn to_roi(r: &LoupecamRegion) -> Result<Option<Roi>, Fail> {
    if r.width == 0 || r.height == 0 {
        return Ok(None);
    }
    let n = |v: u32| u16::try_from(v).map_err(|_| Fail::arg(format!("roi value {v} is too large")));
    Ok(Some(Roi { x: n(r.x)?, y: n(r.y)?, width: n(r.width)?, height: n(r.height)? }))
}

// --- Cameras --------------------------------------------------------------------------

/// The number of connected cameras of supported models.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn loupecam_camera_count(count: *mut usize) -> LoupecamStatus {
    guard(|| {
        *unsafe { out(count, "count") }? = loupecam::list()?.len();
        Ok(())
    })
}

fn opened(cam: AnyCamera, camera: &mut *mut LoupecamCamera) {
    *camera = handle(LoupecamCamera { model: cam.model(), cam: Mutex::new(cam) });
}

/// Open the connected camera at `index` (0: the first one found). Free it with
/// `loupecam_camera_free`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn loupecam_camera_open(index: usize, camera: *mut *mut LoupecamCamera) -> LoupecamStatus {
    guard(|| {
        let camera = unsafe { out(camera, "camera") }?;
        let dev = loupecam::list()?.into_iter().nth(index).ok_or_else(|| Fail(LoupecamStatus::NotFound, format!("no camera at index {index}")))?;
        opened(AnyCamera::open(&dev)?, camera);
        Ok(())
    })
}

/// Open a simulated MU1803-HS: synthetic frames, no hardware needed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn loupecam_camera_open_simulated(camera: *mut *mut LoupecamCamera) -> LoupecamStatus {
    guard(|| {
        opened(AnyCamera::simulated(), unsafe { out(camera, "camera") }?);
        Ok(())
    })
}

/// Stop streaming and release the camera. Streams from it end; frames already read
/// stay valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn loupecam_camera_free(camera: *mut LoupecamCamera) {
    unsafe { free(camera) }
}

impl LoupecamCamera {
    fn with<T>(&self, f: impl FnOnce(&mut AnyCamera) -> loupecam::Result<T>) -> Result<T, Fail> {
        Ok(f(&mut self.cam.lock().unwrap_or_else(|e| e.into_inner()))?)
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn loupecam_camera_info(camera: *const LoupecamCamera, info: *mut LoupecamCameraInfo) -> LoupecamStatus {
    guard(|| {
        let (camera, info) = (unsafe { arg(camera, "camera") }?, unsafe { out(info, "info") }?);
        let cam = camera.cam.lock().unwrap_or_else(|e| e.into_inner());
        let (i, m) = (cam.info(), camera.model);
        copy_str(&mut info.model, m.name);
        copy_str(&mut info.serial, &i.serial);
        copy_str(&mut info.firmware_version, &i.firmware_version);
        copy_str(&mut info.hardware_version, &i.hardware_version);
        (info.fpga_version_major, info.fpga_version_minor) = i.fpga_version;
        info.sensor_id = i.sensor_id;
        info.vendor_id = m.vendor_id;
        info.product_id = m.product_id;
        info.resolution_count = m.resolutions.len() as u32;
        info.pixel_size_um = m.pixel_size_um;
        info.max_bit_depth = m.max_bit_depth.into();
        copy_str(&mut info.bayer_pattern, common::pattern_name(m).unwrap_or(""));
        info.simulated = cam.is_simulated();
        Ok(())
    })
}

/// The model's size number `index` (0 is the largest).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn loupecam_camera_resolution(camera: *const LoupecamCamera, index: u32, width: *mut u32, height: *mut u32) -> LoupecamStatus {
    guard(|| {
        let camera = unsafe { arg(camera, "camera") }?;
        let r = camera.model.resolutions.get(index as usize).ok_or_else(|| Fail::arg(format!("no size {index}")))?;
        *unsafe { out(width, "width") }? = r.width;
        *unsafe { out(height, "height") }? = r.height;
        Ok(())
    })
}

/// Start streaming (`options` NULL: the defaults). Starting again reconfigures the
/// camera, and earlier streams end. Free the stream with `loupecam_stream_free`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn loupecam_camera_start(camera: *mut LoupecamCamera, options: *const LoupecamStartOptions, stream: *mut *mut LoupecamStream) -> LoupecamStatus {
    guard(|| {
        let (camera, stream) = (unsafe { arg(camera, "camera") }?, unsafe { out(stream, "stream") }?);
        let o = unsafe { options.as_ref() }.copied().unwrap_or_else(|| loupecam_start_options_default());
        let mode = match o.pixel_mode {
            0 => loupecam::PixelMode::Raw8,
            1 => loupecam::PixelMode::Raw12,
            2 => loupecam::PixelMode::Processed8,
            v => return Err(Fail::arg(format!("pixel_mode {v} isn't a LoupecamPixelMode"))),
        };
        let mut cfg = StreamConfig::new(o.size as usize, mode);
        cfg.exposure_us = o.exposure_us;
        cfg.gain = Gain::for_multiplier(o.gain as f32).map_err(|e| Fail::arg(e.to_string()))?;
        cfg.speed = u8::try_from(o.speed).map_err(|_| Fail::arg(format!("speed {}", o.speed)))?;
        cfg.binning = o.binning;
        cfg.roi = to_roi(&o.roi)?;
        let rx = camera.with(|c| c.start(&cfg))?;
        *stream = handle(LoupecamStream { rx, model: camera.model });
        Ok(())
    })
}

/// Stop streaming (the camera stays open).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn loupecam_camera_stop(camera: *mut LoupecamCamera) -> LoupecamStatus {
    guard(|| unsafe { arg(camera, "camera") }?.with(|c| c.stop()))
}

/// Set the exposure in µs; `actual_us` (may be NULL) receives the exposure actually
/// programmed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn loupecam_camera_set_exposure(camera: *mut LoupecamCamera, exposure_us: u32, actual_us: *mut f64) -> LoupecamStatus {
    guard(|| {
        let actual = unsafe { arg(camera, "camera") }?.with(|c| c.set_exposure(exposure_us))?;
        if let Some(a) = unsafe { actual_us.as_mut() } {
            *a = actual;
        }
        Ok(())
    })
}

/// Set the gain as a multiplier (1 is unity, up to `loupecam_max_gain()`).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn loupecam_camera_set_gain(camera: *mut LoupecamCamera, gain: f64) -> LoupecamStatus {
    guard(|| {
        let g = Gain::for_multiplier(gain as f32).map_err(|e| Fail::arg(e.to_string()))?;
        unsafe { arg(camera, "camera") }?.with(|c| c.set_gain(g))
    })
}

/// Readout speed, 0 (slowest) to 3 (fastest).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn loupecam_camera_set_speed(camera: *mut LoupecamCamera, speed: u32) -> LoupecamStatus {
    guard(|| {
        let s = u8::try_from(speed).map_err(|_| Fail::arg(format!("speed {speed}")))?;
        unsafe { arg(camera, "camera") }?.with(|c| c.set_speed(s)).map(drop)
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn loupecam_camera_set_binning(camera: *mut LoupecamCamera, binning: bool) -> LoupecamStatus {
    guard(|| unsafe { arg(camera, "camera") }?.with(|c| c.set_binning(binning)))
}

/// Stream only a region of the current size (NULL or zero width: the whole frame).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn loupecam_camera_set_roi(camera: *mut LoupecamCamera, roi: *const LoupecamRegion) -> LoupecamStatus {
    guard(|| {
        let r = match unsafe { roi.as_ref() } {
            Some(r) => to_roi(r)?,
            None => None,
        };
        unsafe { arg(camera, "camera") }?.with(|c| c.set_roi(r)).map(drop)
    })
}

/// The running stream's geometry (`LOUPECAM_STATUS_NOT_STREAMING` when stopped).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn loupecam_camera_geometry(camera: *const LoupecamCamera, geometry: *mut LoupecamGeometry) -> LoupecamStatus {
    guard(|| {
        let (camera, geometry) = (unsafe { arg(camera, "camera") }?, unsafe { out(geometry, "geometry") }?);
        let g = camera.with(|c| Ok(c.geometry()))?.ok_or_else(|| Fail(LoupecamStatus::NotStreaming, "not streaming".into()))?;
        let g = common::Geometry::from(g);
        *geometry = LoupecamGeometry { width: g.width, height: g.height, bits: g.bits.into(), exposure_us: g.exposure_us, max_fps: g.max_fps };
        Ok(())
    })
}

// --- Streams --------------------------------------------------------------------------

fn read(stream: *const LoupecamStream, frame: *mut *mut LoupecamFrame, f: impl FnOnce(&loupecam::FrameReceiver) -> Result<RawFrame, RecvError>) -> LoupecamStatus {
    guard(|| {
        let (stream, frame) = (unsafe { arg(stream, "stream") }?, unsafe { out(frame, "frame") }?);
        *frame = handle(LoupecamFrame { raw: f(&stream.rx)?, model: stream.model });
        Ok(())
    })
}

/// The next frame, waiting up to `timeout_ms` (negative: forever). Free it with
/// `loupecam_frame_free`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn loupecam_stream_read(stream: *const LoupecamStream, timeout_ms: i32, frame: *mut *mut LoupecamFrame) -> LoupecamStatus {
    read(stream, frame, |rx| match u64::try_from(timeout_ms) {
        Ok(t) => rx.recv_timeout(Duration::from_millis(t)),
        Err(_) => rx.recv(),
    })
}

/// The newest frame, discarding older queued ones (for live views), waiting up to
/// `timeout_ms` (negative: 1 s).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn loupecam_stream_latest(stream: *const LoupecamStream, timeout_ms: i32, frame: *mut *mut LoupecamFrame) -> LoupecamStatus {
    let t = Duration::from_millis(u64::try_from(timeout_ms).unwrap_or(1000));
    read(stream, frame, |rx| rx.recv_latest(t))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn loupecam_stream_stats(stream: *const LoupecamStream, stats: *mut LoupecamStreamStats) -> LoupecamStatus {
    guard(|| {
        let s = unsafe { arg(stream, "stream") }?.rx.stats();
        *unsafe { out(stats, "stats") }? = LoupecamStreamStats {
            frames: s.frames,
            dropped: s.dropped,
            discarded: s.discarded,
            bytes: s.bytes,
            transfer_errors: s.transfer_errors,
            sequence_gaps: s.sequence_gaps,
            fps: s.fps,
        };
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn loupecam_stream_free(stream: *mut LoupecamStream) {
    unsafe { free(stream) }
}

// --- Frames ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub unsafe extern "C" fn loupecam_frame_info(frame: *const LoupecamFrame, info: *mut LoupecamFrameInfo) -> LoupecamStatus {
    guard(|| {
        let f = &unsafe { arg(frame, "frame") }?.raw;
        *unsafe { out(info, "info") }? = LoupecamFrameInfo {
            width: f.width,
            height: f.height,
            bits: f.format.bits().into(),
            bytes_per_sample: f.format.bytes_per_sample() as u32,
            sequence: f.trailer.sequence,
        };
        Ok(())
    })
}

/// The raw mosaic: `width × height` samples, row-major, as the sensor sends them
/// (bottom-up on the MU1803; developing turns them upright). Valid until the frame is
/// freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn loupecam_frame_data(frame: *const LoupecamFrame, data: *mut *const c_void, len: *mut usize) -> LoupecamStatus {
    guard(|| {
        let f = &unsafe { arg(frame, "frame") }?.raw;
        *unsafe { out(data, "data") }? = f.data.as_ptr().cast();
        *unsafe { out(len, "len") }? = f.data.len();
        Ok(())
    })
}

/// Develop into an image (`options` NULL: the defaults). Free it with
/// `loupecam_image_free`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn loupecam_frame_develop(frame: *const LoupecamFrame, options: *const LoupecamDevelopOptions, image: *mut *mut LoupecamImage) -> LoupecamStatus {
    guard(|| {
        let (frame, image) = (unsafe { arg(frame, "frame") }?, unsafe { out(image, "image") }?);
        let o = develop_options(unsafe { options.as_ref() })?;
        *image = handle(LoupecamImage(o.develop(&frame.raw, frame.model)?));
        Ok(())
    })
}

fn to_format(f: u32, quality: u32) -> Result<Format, Fail> {
    Ok(match f {
        0 => Format::Png,
        1 => Format::Tiff,
        2 => Format::Jpeg { quality: quality.clamp(1, 100) as u8 },
        v => return Err(Fail::arg(format!("format {v} isn't a LoupecamFormat"))),
    })
}

/// Develop and encode (`format` a `LoupecamFormat`; `quality` 1-100 for JPEG). With
/// `raw`, the undeveloped mosaic scaled to 16 bits, as TIFF only. Free the result with
/// `loupecam_bytes_free`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn loupecam_frame_encode(
    frame: *const LoupecamFrame,
    format: u32,
    quality: u32,
    raw: bool,
    options: *const LoupecamDevelopOptions,
    bytes: *mut *mut LoupecamBytes,
) -> LoupecamStatus {
    guard(|| {
        let (frame, bytes) = (unsafe { arg(frame, "frame") }?, unsafe { out(bytes, "bytes") }?);
        let o = develop_options(unsafe { options.as_ref() })?;
        *bytes = handle(LoupecamBytes(o.encode(&frame.raw, frame.model, to_format(format, quality)?, raw)?));
        Ok(())
    })
}

/// Develop and write a file at `path` (UTF-8); the format comes from the extension
/// (.png, .tif/.tiff, .jpg/.jpeg). `quality` and `raw` as for `loupecam_frame_encode`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn loupecam_frame_save(frame: *const LoupecamFrame, path: *const c_char, quality: u32, raw: bool, options: *const LoupecamDevelopOptions) -> LoupecamStatus {
    guard(|| {
        let frame = unsafe { arg(frame, "frame") }?;
        let path = unsafe { CStr::from_ptr(arg(path, "path")?) }.to_str().map_err(|_| Fail::arg("path isn't UTF-8"))?;
        let path = std::path::Path::new(path);
        let o = develop_options(unsafe { options.as_ref() })?;
        let format = common::format_for_path(path, quality.clamp(1, 100) as u8)?;
        std::fs::write(path, o.encode(&frame.raw, frame.model, format, raw)?)?;
        Ok(())
    })
}

/// Grey-world white balance: `gains` (3 floats) receives R, G, B. Over a neutral
/// `region` (NULL: the whole frame), this is "click to white balance".
#[unsafe(no_mangle)]
pub unsafe extern "C" fn loupecam_frame_white_balance(frame: *const LoupecamFrame, region: *const LoupecamRegion, gains: *mut f32) -> LoupecamStatus {
    guard(|| {
        let frame = unsafe { arg(frame, "frame") }?;
        let g = common::grey_world(&frame.raw, frame.model, to_rect(unsafe { region.as_ref() }));
        unsafe { out(gains.cast::<[f32; 3]>(), "gains") }?.copy_from_slice(&g);
        Ok(())
    })
}

/// Raw statistics over `region` (NULL: the whole frame). `white_balance` (3 floats,
/// NULL: the model's daylight gains) weights the luma estimate.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn loupecam_frame_stats(frame: *const LoupecamFrame, region: *const LoupecamRegion, white_balance: *const f32, stats: *mut LoupecamFrameStats) -> LoupecamStatus {
    guard(|| {
        let (frame, stats) = (unsafe { arg(frame, "frame") }?, unsafe { out(stats, "stats") }?);
        let wb = unsafe { white_balance.cast::<[f32; 3]>().as_ref() }.copied();
        let s = common::frame_stats(&frame.raw, frame.model, to_rect(unsafe { region.as_ref() }), wb);
        stats.mean = s.mean;
        stats.luma = s.luma;
        stats.clipped = s.clipped;
        stats.histogram = [0; 256];
        for (d, v) in stats.histogram.iter_mut().zip(s.histogram) {
            *d = v;
        }
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn loupecam_frame_free(frame: *mut LoupecamFrame) {
    unsafe { free(frame) }
}

// --- Images and bytes -----------------------------------------------------------------

#[unsafe(no_mangle)]
pub unsafe extern "C" fn loupecam_image_info(image: *const LoupecamImage, info: *mut LoupecamImageInfo) -> LoupecamStatus {
    guard(|| {
        let img = &unsafe { arg(image, "image") }?.0;
        let (width, height, channels) = img.shape();
        let depth = match img {
            Developed::Eight(_) => 8,
            Developed::Sixteen(_) => 16,
        };
        *unsafe { out(info, "info") }? = LoupecamImageInfo { width, height, channels: channels.into(), depth };
        Ok(())
    })
}

/// The pixels (`len` in bytes). Valid until the image is freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn loupecam_image_data(image: *const LoupecamImage, data: *mut *const c_void, len: *mut usize) -> LoupecamStatus {
    guard(|| {
        let (ptr, n): (*const c_void, usize) = match &unsafe { arg(image, "image") }?.0 {
            Developed::Eight(i) => (i.data.as_ptr().cast(), i.data.len()),
            Developed::Sixteen(i) => (i.data.as_ptr().cast(), i.data.len() * 2),
        };
        *unsafe { out(data, "data") }? = ptr;
        *unsafe { out(len, "len") }? = n;
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn loupecam_image_free(image: *mut LoupecamImage) {
    unsafe { free(image) }
}

/// The encoded file. Valid until the bytes are freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn loupecam_bytes_data(bytes: *const LoupecamBytes, data: *mut *const u8, len: *mut usize) -> LoupecamStatus {
    guard(|| {
        let b = &unsafe { arg(bytes, "bytes") }?.0;
        *unsafe { out(data, "data") }? = b.as_ptr();
        *unsafe { out(len, "len") }? = b.len();
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn loupecam_bytes_free(bytes: *mut LoupecamBytes) {
    unsafe { free(bytes) }
}

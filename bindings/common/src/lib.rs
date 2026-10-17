//! Shared by the language bindings (Python, Node.js, C), so every language has the
//! same develop options with the same names and defaults, and the same conversions.
//! Each binding maps its own argument style onto [`DevelopOptions`].

use loupecam::{BayerPattern, ColorFilter, Model, RawFrame, StreamGeometry};
use loupecam_isp::encode::{self, EncodeError, Format};
use loupecam_isp::stats::{self, Rect, Stats};
use loupecam_isp::tone::Transfer;
use loupecam_isp::{Demosaic, DevelopParams, Image8, Image16, Rotation};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A bad option value: bindings report it as their language's argument error.
    #[error("{0}")]
    InvalidArgument(String),
    #[error(transparent)]
    Encode(#[from] EncodeError),
}

fn invalid(msg: impl Into<String>) -> Error {
    Error::InvalidArgument(msg.into())
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// How to white-balance a developed image.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum WhiteBalance {
    /// The model's daylight gains.
    #[default]
    Daylight,
    /// Grey world over the whole frame.
    Auto,
    /// Unity gains.
    None,
    /// R, G, B gains.
    Gains([f32; 3]),
}

impl std::str::FromStr for WhiteBalance {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        match s.to_ascii_lowercase().as_str() {
            "daylight" => Ok(WhiteBalance::Daylight),
            "auto" => Ok(WhiteBalance::Auto),
            "none" => Ok(WhiteBalance::None),
            _ => Err(invalid(format!("white balance {s:?}: use \"daylight\", \"auto\", \"none\" or [r, g, b] gains"))),
        }
    }
}

/// Which colour matrix to apply to linear camera RGB.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum ColorMatrix {
    /// The model's own.
    #[default]
    Model,
    /// None (identity).
    Identity,
    /// Row-major.
    Custom([[f32; 3]; 3]),
}

/// Parse a tone curve name: `srgb` or `linear`. (A number is a gamma; bindings pass
/// that as [`Transfer::Gamma`] directly.)
pub fn transfer_named(s: &str) -> Result<Transfer> {
    match s.to_ascii_lowercase().as_str() {
        "srgb" => Ok(Transfer::Srgb),
        "linear" => Ok(Transfer::Linear),
        _ => Err(invalid(format!("tone {s:?}: use \"srgb\", \"linear\" or a gamma value"))),
    }
}

pub fn demosaic_named(s: &str) -> Result<Demosaic> {
    s.parse().map_err(|e: loupecam_isp::UnknownName| invalid(e.to_string()))
}

pub fn pixel_mode_named(s: &str) -> Result<loupecam::PixelMode> {
    s.parse().map_err(|_| invalid(format!("pixel mode {s:?}: use \"raw8\", \"raw12\" or \"processed8\"")))
}

/// A format by name (`png`, `tiff`/`tif`, `jpeg`/`jpg`).
pub fn format_named(s: &str, jpeg_quality: u8) -> Result<Format> {
    Format::from_extension(s, jpeg_quality).ok_or_else(|| invalid(format!("format {s:?}: use \"png\", \"tiff\" or \"jpeg\"")))
}

/// A format from a file path's extension.
pub fn format_for_path(path: &std::path::Path, jpeg_quality: u8) -> Result<Format> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    Format::from_extension(ext, jpeg_quality)
        .ok_or_else(|| invalid(format!("unknown image extension {ext:?} (use .png, .tif or .jpg)")))
}

/// Develop options. Every field defaults to the camera model's own settings: its
/// colour matrix and daylight white balance, bilinear demosaicing, the sRGB curve,
/// upright, 8 bits.
#[derive(Debug, Clone, PartialEq)]
pub struct DevelopOptions {
    pub demosaic: Demosaic,
    pub white_balance: WhiteBalance,
    pub color_matrix: ColorMatrix,
    pub transfer: Transfer,
    /// Input level (0–1) mapped to black.
    pub black_point: f32,
    /// Input level (0–1) mapped to white.
    pub white_point: f32,
    /// −1 to 1.
    pub brightness: f32,
    /// −1 to 1.
    pub contrast: f32,
    /// Sensor black level to subtract, in the frame's own sample units.
    pub black_level: u32,
    pub monochrome: bool,
    pub negative: bool,
    pub flip_horizontal: bool,
    /// Relative to upright: a bottom-up sensor readout is corrected regardless.
    pub flip_vertical: bool,
    /// Clockwise degrees: 0, 90, 180 or 270, applied after flips.
    pub rotation: u32,
    /// Output bits per channel: 8 or 16.
    pub depth: u8,
}

impl Default for DevelopOptions {
    fn default() -> Self {
        DevelopOptions {
            demosaic: Demosaic::Bilinear,
            white_balance: WhiteBalance::Daylight,
            color_matrix: ColorMatrix::Model,
            transfer: Transfer::Srgb,
            black_point: 0.0,
            white_point: 1.0,
            brightness: 0.0,
            contrast: 0.0,
            black_level: 0,
            monochrome: false,
            negative: false,
            flip_horizontal: false,
            flip_vertical: false,
            rotation: 0,
            depth: 8,
        }
    }
}

/// A developed image, 8 or 16 bits per channel.
pub enum Developed {
    Eight(Image8),
    Sixteen(Image16),
}

impl Developed {
    /// `(width, height, channels)`.
    pub fn shape(&self) -> (u32, u32, u8) {
        match self {
            Developed::Eight(i) => (i.width, i.height, i.channels),
            Developed::Sixteen(i) => (i.width, i.height, i.channels),
        }
    }
}

impl DevelopOptions {
    /// Check the values that have a fixed set (bindings call this before starting
    /// work, so a bad option fails fast).
    pub fn validate(&self) -> Result<()> {
        rotation(self.rotation)?;
        if self.depth != 8 && self.depth != 16 {
            return Err(invalid(format!("depth {}: use 8 or 16", self.depth)));
        }
        Ok(())
    }

    /// The pipeline parameters for `frame` from `model`.
    pub fn params(&self, frame: &RawFrame, model: &Model) -> Result<DevelopParams> {
        self.validate()?;
        let mut p = DevelopParams::for_model(model);
        p.demosaic = self.demosaic;
        p.wb = match self.white_balance {
            WhiteBalance::Daylight => p.wb,
            WhiteBalance::Auto => grey_world(frame, model, None),
            WhiteBalance::None => [1.0; 3],
            WhiteBalance::Gains(g) => g,
        };
        match self.color_matrix {
            ColorMatrix::Model => {}
            ColorMatrix::Identity => p.ccm = loupecam_isp::IDENTITY,
            ColorMatrix::Custom(m) => p.ccm = m,
        }
        p.tone.transfer = self.transfer;
        p.tone.black_point = self.black_point;
        p.tone.white_point = self.white_point;
        p.tone.brightness = self.brightness;
        p.tone.contrast = self.contrast;
        // The pipeline works at 16-bit scale.
        let shift = 16 - frame.format.bits() as u32;
        p.black_level = (self.black_level.min(u16::MAX as u32) << shift).min(u16::MAX as u32) as u16;
        p.monochrome = self.monochrome;
        p.negative = self.negative;
        p.orientation.flip_h = self.flip_horizontal;
        p.orientation.flip_v = self.flip_vertical ^ model.rows_bottom_up;
        p.orientation.rotation = rotation(self.rotation)?;
        Ok(p)
    }

    pub fn develop(&self, frame: &RawFrame, model: &Model) -> Result<Developed> {
        let p = self.params(frame, model)?;
        Ok(if self.depth == 16 {
            Developed::Sixteen(loupecam_isp::develop16(frame, &p))
        } else {
            Developed::Eight(loupecam_isp::develop(frame, &p))
        })
    }

    /// Develop and encode. With `raw`, the undeveloped mosaic as TIFF (the options
    /// are ignored).
    pub fn encode(&self, frame: &RawFrame, model: &Model, format: Format, raw: bool) -> Result<Vec<u8>> {
        if raw {
            if format != Format::Tiff {
                return Err(invalid("raw frames are saved as TIFF"));
            }
            return Ok(encode::raw_tiff(frame)?);
        }
        Ok(match self.develop(frame, model)? {
            Developed::Eight(i) => encode::encode8(&i, format)?,
            Developed::Sixteen(i) => encode::encode16(&i, format)?,
        })
    }
}

fn rotation(degrees: u32) -> Result<Rotation> {
    match degrees {
        0 => Ok(Rotation::None),
        90 => Ok(Rotation::Cw90),
        180 => Ok(Rotation::Cw180),
        270 => Ok(Rotation::Cw270),
        r => Err(invalid(format!("rotation {r}: use 0, 90, 180 or 270"))),
    }
}

/// The model's Bayer pattern (RGGB for the unsupported monochrome case).
pub fn pattern(model: &Model) -> BayerPattern {
    match model.color {
        ColorFilter::Bayer(p) => p,
        ColorFilter::Mono => BayerPattern::Rggb,
    }
}

/// The model's Bayer pattern name, e.g. `"RGGB"`, or `None` for monochrome.
pub fn pattern_name(model: &Model) -> Option<&'static str> {
    match model.color {
        ColorFilter::Bayer(p) => Some(p.name()),
        ColorFilter::Mono => None,
    }
}

/// Grey-world white balance gains, over `region` or the whole frame.
pub fn grey_world(frame: &RawFrame, model: &Model, region: Option<Rect>) -> [f32; 3] {
    loupecam_isp::auto::grey_world(&stats::compute(frame, pattern(model), region, 2, [1.0; 3]))
}

/// Raw statistics; `wb` (default: the model's daylight gains) weights the luma.
pub fn frame_stats(frame: &RawFrame, model: &Model, region: Option<Rect>, wb: Option<[f32; 3]>) -> Stats {
    let d = model.daylight_wb;
    stats::compute(frame, pattern(model), region, 1, wb.unwrap_or([d.r, d.g, d.b]))
}

/// The raw samples widened to `u16` (for 16-bit frames: just decoded from LE bytes).
pub fn samples_u16(frame: &RawFrame) -> Vec<u16> {
    match frame.format {
        loupecam::SampleFormat::U8 => frame.data.iter().map(|&b| b as u16).collect(),
        loupecam::SampleFormat::U16 { .. } => frame.data.as_chunks::<2>().0.iter().map(|b| u16::from_le_bytes(*b)).collect(),
    }
}

/// A stream's geometry, flattened for bindings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Geometry {
    pub width: u32,
    pub height: u32,
    pub bits: u8,
    /// The exposure actually programmed, in µs.
    pub exposure_us: f64,
    /// The highest frame rate this geometry allows at that exposure.
    pub max_fps: f64,
}

impl From<StreamGeometry> for Geometry {
    fn from(g: StreamGeometry) -> Self {
        let lines = loupecam::protocol::sensor::ar1820::exposure_lines(g.exposure_us.round() as u32, g.line_length_pck);
        Geometry {
            width: g.resolution.width,
            height: g.resolution.height,
            bits: g.format.bits(),
            exposure_us: g.exposure_us,
            max_fps: g.max_fps(lines),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame() -> (RawFrame, &'static Model) {
        let mut cam = loupecam::AnyCamera::simulated();
        let rx = cam.start(&loupecam::StreamConfig::new(2, loupecam::PixelMode::Raw12)).unwrap();
        (rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap(), cam.model())
    }

    #[test]
    fn defaults_and_options() {
        let (f, m) = frame();
        let o = DevelopOptions::default();
        assert_eq!(o.params(&f, m).unwrap(), DevelopParams::for_model(m));
        let img = DevelopOptions { demosaic: Demosaic::Superpixel, rotation: 90, depth: 16, ..o.clone() }.develop(&f, m).unwrap();
        assert!(matches!(img, Developed::Sixteen(_)));
        assert_eq!(img.shape(), (461, 614, 3));
        assert!(DevelopOptions { rotation: 45, ..o.clone() }.validate().is_err());
        assert!(DevelopOptions { depth: 12, ..o.clone() }.validate().is_err());
        assert!(o.encode(&f, m, Format::Png, true).is_err());
        assert!(o.encode(&f, m, Format::Tiff, true).is_ok());
    }

    #[test]
    fn names() {
        assert_eq!("AUTO".parse::<WhiteBalance>().unwrap(), WhiteBalance::Auto);
        assert!("bright".parse::<WhiteBalance>().is_err());
        assert_eq!(demosaic_named("mhc").unwrap(), Demosaic::Mhc);
        assert!(format_for_path("a.bmp".as_ref(), 90).is_err());
        assert_eq!(format_for_path("a.JPG".as_ref(), 70).unwrap(), Format::Jpeg { quality: 70 });
    }
}

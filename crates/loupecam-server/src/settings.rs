//! User-facing camera and image settings, as exchanged with clients (JSON, camelCase).
//!
//! Clients change settings with JSON merge patches (RFC 7396): send only the fields to
//! change, `null` to clear an optional one.

use loupecam::{Gain, PixelMode, Roi, StreamConfig};
use loupecam_isp::tone::{ToneCurve, Transfer};
use loupecam_isp::{Demosaic, DevelopParams, Orientation, Rotation};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Settings {
    /// Index into the model's resolutions (0 = largest).
    pub size_index: usize,
    /// 8 or 12 bits per sample from the sensor.
    pub bit_depth: u8,
    pub exposure_us: u32,
    pub auto_exposure: bool,
    /// Auto-exposure target, mean linear luma 0..1 (0.18 = mid grey).
    pub ae_target: f32,
    /// Longest exposure auto-exposure may choose.
    pub ae_max_exposure_us: u32,
    /// Highest gain auto-exposure may choose.
    pub ae_max_gain: f32,
    /// Sensor gain multiplier (1.0 = unity).
    pub gain: f32,
    /// Readout speed 0 (slowest, least noise) ..= 3 (fastest).
    pub speed: u8,
    /// Sum pixels instead of skipping when subsampling.
    pub binning: bool,
    pub roi: Option<RoiSetting>,
    /// Round exposure to multiples of the mains flicker period.
    pub anti_flicker: AntiFlicker,
    pub white_balance: WhiteBalance,
    pub color: ColorSettings,
    pub tone: ToneSettings,
    pub orientation: OrientationSettings,
    pub preview: PreviewSettings,
    pub capture: CaptureSettings,
    pub record: RecordSettings,
    pub updates: UpdateSettings,
    pub calibration: CalibrationSettings,
}

/// Which calibration results to apply. Each only has an effect once measured (see the
/// calibration wizard); with no calibration the defaults are used.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CalibrationSettings {
    /// Subtract the measured black level.
    pub black_level: bool,
    /// Replace measured defective pixels.
    pub defects: bool,
    /// Use the measured analog gain stages.
    pub gain_stages: bool,
    /// Apply the flat field. Off by default: it is tied to the lighting it was measured
    /// with (a moved ring light changes it).
    pub flat_field: bool,
    /// Use the measured colour matrix instead of the camera's default.
    pub color: bool,
    /// Scale preset (by name) for the scale bar and measurements.
    pub scale_preset: Option<String>,
}

impl Default for CalibrationSettings {
    fn default() -> Self {
        CalibrationSettings { black_level: true, defects: true, gain_stages: true, flat_field: false, color: false, scale_preset: None }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateSettings {
    /// Install new releases automatically (and restart). Off unless the user opts in.
    pub auto_install: bool,
    /// How often to look for a new release.
    pub check_interval_hours: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RoiSetting {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AntiFlicker {
    Off,
    #[serde(rename = "50hz")]
    Hz50,
    #[serde(rename = "60hz")]
    Hz60,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WhiteBalance {
    pub mode: WbMode,
    /// R, G, B gains used in manual mode and updated by auto modes.
    pub gains: [f32; 3],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WbMode {
    Manual,
    /// Continuous grey-world.
    Auto,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ColorSettings {
    /// Apply the camera's colour-correction matrix.
    pub correction: bool,
    /// 0 = greyscale, 1 = unchanged, 2 = double.
    pub saturation: f32,
    /// Hue rotation in degrees.
    pub hue: f32,
    pub monochrome: bool,
    pub negative: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToneSettings {
    /// "srgb", "linear", or a gamma value such as 2.2.
    pub curve: CurveSetting,
    pub black_point: f32,
    pub white_point: f32,
    pub brightness: f32,
    pub contrast: f32,
    /// Black level subtracted from the sensor data, in 12-bit units.
    pub black_level: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CurveSetting {
    Named(NamedCurve),
    Gamma(f32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NamedCurve {
    Srgb,
    Linear,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OrientationSettings {
    pub flip_horizontal: bool,
    pub flip_vertical: bool,
    /// 0, 90, 180 or 270 (clockwise).
    pub rotation: u16,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreviewSettings {
    /// Previews larger than this are downscaled.
    pub max_width: u32,
    pub jpeg_quality: u8,
    pub max_fps: f32,
    pub demosaic: DemosaicSetting,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CaptureSettings {
    pub format: CaptureFormat,
    pub jpeg_quality: u8,
    /// 16 bits per channel (PNG/TIFF only).
    pub sixteen_bit: bool,
    /// Capture at the largest resolution even when previewing smaller.
    pub full_resolution: bool,
    pub demosaic: DemosaicSetting,
    /// Also save the undeveloped mosaic as a 16-bit TIFF.
    pub save_raw: bool,
}

/// How recordings are made. The presets fill these in; any field can be changed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecordSettings {
    /// The preset these settings came from (informational; `custom` once edited).
    pub preset: RecordPreset,
    pub mode: RecordMode,
    /// The video fits within this size (keeping the frame's aspect ratio).
    pub max_width: u32,
    pub max_height: u32,
    /// Most frames per second to record (video mode).
    pub max_fps: f32,
    pub bitrate_kbps: u32,
    /// Stream at this size index while recording (then switch back), e.g. a larger
    /// size for detail. `None` = record the stream as it is.
    pub stream_size_index: Option<usize>,
    /// Timelapse: seconds between frames.
    pub timelapse_interval_s: f32,
    /// Timelapse: playback frame rate of the video.
    pub timelapse_fps: f32,
    /// Timelapse: also save every frame as a still (with the capture settings).
    pub timelapse_stills: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RecordPreset {
    /// High frame rate: the stream as it is, up to 1280×960 at 30 fps.
    Smooth,
    /// High quality: a larger stream size, up to 1920×1080 (the Pi 4 encoder's limit) at
    /// a lower frame rate.
    Detailed,
    /// One frame every 10 s, played back at 30 fps.
    Timelapse,
    Custom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RecordMode {
    /// Real time.
    Video,
    /// A frame every `timelapse_interval_s`.
    Timelapse,
}

impl RecordPreset {
    pub const ALL: [RecordPreset; 3] = [RecordPreset::Smooth, RecordPreset::Detailed, RecordPreset::Timelapse];

    /// The settings for a preset (`Custom` keeps `current`).
    pub fn settings(self, current: &RecordSettings) -> RecordSettings {
        let base = RecordSettings { preset: self, ..RecordSettings::default() };
        match self {
            RecordPreset::Smooth => base,
            RecordPreset::Detailed => RecordSettings {
                max_width: 1920,
                max_height: 1080,
                max_fps: 15.0,
                bitrate_kbps: 12_000,
                stream_size_index: Some(1),
                ..base
            },
            RecordPreset::Timelapse => {
                RecordSettings { mode: RecordMode::Timelapse, max_width: 1920, max_height: 1080, bitrate_kbps: 12_000, ..base }
            }
            RecordPreset::Custom => RecordSettings { preset: self, ..current.clone() },
        }
    }
}

/// The `smooth` preset.
impl Default for RecordSettings {
    fn default() -> Self {
        RecordSettings {
            preset: RecordPreset::Smooth,
            mode: RecordMode::Video,
            max_width: 1280,
            max_height: 960,
            max_fps: 30.0,
            bitrate_kbps: 8_000,
            stream_size_index: None,
            timelapse_interval_s: 10.0,
            timelapse_fps: 30.0,
            timelapse_stills: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CaptureFormat {
    Png,
    Tiff,
    Jpeg,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DemosaicSetting {
    Superpixel,
    Bilinear,
    Mhc,
}

impl From<DemosaicSetting> for Demosaic {
    fn from(d: DemosaicSetting) -> Self {
        match d {
            DemosaicSetting::Superpixel => Demosaic::Superpixel,
            DemosaicSetting::Bilinear => Demosaic::Bilinear,
            DemosaicSetting::Mhc => Demosaic::Mhc,
        }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            size_index: 2,
            bit_depth: 8,
            exposure_us: 20_000,
            auto_exposure: true,
            ae_target: 0.18,
            ae_max_exposure_us: 200_000,
            ae_max_gain: 8.0,
            gain: 1.0,
            speed: 3,
            binning: false,
            roi: None,
            anti_flicker: AntiFlicker::Off,
            // The vendor's daylight default (FPGA gains 0x188/0x100/0x1b6). Grey-world AWB
            // would neutralise genuinely coloured subjects, so it is opt-in.
            white_balance: WhiteBalance { mode: WbMode::Manual, gains: [1.53, 1.0, 1.71] },
            color: ColorSettings { correction: true, saturation: 1.0, hue: 0.0, monochrome: false, negative: false },
            tone: ToneSettings {
                curve: CurveSetting::Named(NamedCurve::Srgb),
                black_point: 0.0,
                white_point: 1.0,
                brightness: 0.0,
                contrast: 0.0,
                black_level: 0,
            },
            orientation: OrientationSettings { flip_horizontal: false, flip_vertical: false, rotation: 0 },
            preview: PreviewSettings { max_width: 1280, jpeg_quality: 80, max_fps: 30.0, demosaic: DemosaicSetting::Bilinear },
            capture: CaptureSettings {
                format: CaptureFormat::Png,
                jpeg_quality: 95,
                sixteen_bit: false,
                full_resolution: true,
                demosaic: DemosaicSetting::Mhc,
                save_raw: false,
            },
            record: RecordSettings::default(),
            updates: UpdateSettings { auto_install: false, check_interval_hours: 6.0 },
            calibration: CalibrationSettings::default(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    #[error("invalid settings: {0}")]
    Invalid(String),
}

impl Settings {
    /// Apply an RFC 7396 JSON merge patch, returning the validated result.
    pub fn patched(&self, patch: &serde_json::Value) -> Result<Settings, SettingsError> {
        let mut doc = serde_json::to_value(self).expect("settings serialise");
        merge(&mut doc, patch);
        let s: Settings = serde_json::from_value(doc).map_err(|e| SettingsError::Invalid(e.to_string()))?;
        s.validate()?;
        Ok(s)
    }

    pub fn validate(&self) -> Result<(), SettingsError> {
        let bad = |m: String| Err(SettingsError::Invalid(m));
        if ![8, 12].contains(&self.bit_depth) {
            return bad(format!("bitDepth must be 8 or 12, not {}", self.bit_depth));
        }
        if self.speed > 3 {
            return bad("speed must be 0..=3".into());
        }
        if !(1.0..=Gain::max_multiplier()).contains(&self.gain) {
            return bad(format!("gain must be 1.0..={}", Gain::max_multiplier()));
        }
        if self.exposure_us == 0 || self.exposure_us > 10_000_000 {
            return bad("exposureUs must be 1..=10000000".into());
        }
        if ![0, 90, 180, 270].contains(&self.orientation.rotation) {
            return bad("rotation must be 0, 90, 180 or 270".into());
        }
        if self.white_balance.gains.iter().any(|g| !(0.05..=16.0).contains(g)) {
            return bad("white balance gains must be 0.05..=16".into());
        }
        let r = &self.record;
        if !(16..=8192).contains(&r.max_width) || !(16..=8192).contains(&r.max_height) {
            return bad("record maxWidth/maxHeight must be 16..=8192".into());
        }
        if !(0.1..=120.0).contains(&r.max_fps) || !(0.1..=120.0).contains(&r.timelapse_fps) {
            return bad("record frame rates must be 0.1..=120".into());
        }
        if !(100..=100_000).contains(&r.bitrate_kbps) {
            return bad("record bitrateKbps must be 100..=100000".into());
        }
        if !(0.1..=86_400.0).contains(&r.timelapse_interval_s) {
            return bad("record timelapseIntervalS must be 0.1..=86400".into());
        }
        Ok(())
    }

    /// Whether changing from `self` to `next` requires restarting the stream.
    pub fn needs_restart(&self, next: &Settings) -> bool {
        self.size_index != next.size_index || self.bit_depth != next.bit_depth
    }

    /// Exposure after anti-flicker rounding (whole flicker periods, when longer than one).
    pub fn effective_exposure_us(&self, us: u32) -> u32 {
        let period = match self.anti_flicker {
            AntiFlicker::Off => return us,
            AntiFlicker::Hz50 => 10_000,
            AntiFlicker::Hz60 => 8_333,
        };
        if us < period { us } else { us / period * period }
    }

    /// Stream configuration, choosing gain registers from `gains` (the measured stages
    /// when calibrated, else the built-in estimates).
    pub fn stream_config(&self, gains: &loupecam::protocol::sensor::ar1820::GainTable) -> StreamConfig {
        let mode = if self.bit_depth == 12 { PixelMode::Raw12 } else { PixelMode::Raw8 };
        let mut c = StreamConfig::new(self.size_index, mode);
        c.exposure_us = self.effective_exposure_us(self.exposure_us);
        let gain = self.gain.min(gains.max_multiplier());
        c.gain = gains.for_multiplier(gain).unwrap_or(Gain::UNITY);
        c.speed = self.speed;
        c.binning = self.binning;
        c.roi = self.roi.map(|r| Roi { x: r.x, y: r.y, width: r.width, height: r.height });
        c
    }

    /// Host-side develop parameters for the current settings.
    /// Host-side develop parameters for a frame, including any enabled calibration.
    pub fn develop_params(&self, env: &crate::service::FrameEnvelope, demosaic: DemosaicSetting, cal: &crate::calibration::Calibration) -> DevelopParams {
        let mut p = self.develop_params_uncalibrated(env.model, demosaic);
        let c = &self.calibration;
        p.black_level = ((cal.black_level(c) + self.tone.black_level as f32) * 16.0).round().clamp(0.0, 65535.0) as u16;
        if let Some(col) = cal.color(c)
            && self.color.correction
        {
            let m = loupecam::ColorMatrix::hue(self.color.hue)
                .mul(&loupecam::ColorMatrix::saturation(self.color.saturation))
                .mul(&loupecam::ColorMatrix(col.ccm));
            p.ccm = m.0;
        }
        p.corrections = cal.corrections(c, env.size_index, env.origin, (env.raw.width, env.raw.height));
        p
    }

    /// Develop parameters without calibration (also used for orientation/tone only).
    pub fn develop_params_uncalibrated(&self, model: &loupecam::Model, demosaic: DemosaicSetting) -> DevelopParams {
        let pattern = match model.color {
            loupecam::ColorFilter::Bayer(p) => p,
            loupecam::ColorFilter::Mono => loupecam::BayerPattern::Rggb,
        };
        let mut p = DevelopParams::new(pattern);
        p.demosaic = demosaic.into();
        p.black_level = self.tone.black_level.saturating_mul(16);
        p.wb = self.white_balance.gains;
        let base = if self.color.correction { loupecam::ColorMatrix::mu1803_default() } else { loupecam::ColorMatrix::IDENTITY };
        let m = loupecam::ColorMatrix::hue(self.color.hue)
            .mul(&loupecam::ColorMatrix::saturation(self.color.saturation))
            .mul(&base);
        p.ccm = m.0;
        p.tone = ToneCurve {
            transfer: match self.tone.curve {
                CurveSetting::Named(NamedCurve::Srgb) => Transfer::Srgb,
                CurveSetting::Named(NamedCurve::Linear) => Transfer::Linear,
                CurveSetting::Gamma(g) => Transfer::Gamma(g),
            },
            black_point: self.tone.black_point,
            white_point: self.tone.white_point,
            brightness: self.tone.brightness,
            contrast: self.tone.contrast,
        };
        p.monochrome = self.color.monochrome;
        p.negative = self.color.negative;
        p.orientation = Orientation {
            flip_h: self.orientation.flip_horizontal,
            flip_v: self.orientation.flip_vertical ^ model.rows_bottom_up,
            rotation: match self.orientation.rotation {
                90 => Rotation::Cw90,
                180 => Rotation::Cw180,
                270 => Rotation::Cw270,
                _ => Rotation::None,
            },
        };
        p
    }
}

/// RFC 7396 JSON merge patch.
fn merge(target: &mut serde_json::Value, patch: &serde_json::Value) {
    use serde_json::Value;
    match patch {
        Value::Object(p) => {
            if !target.is_object() {
                *target = Value::Object(Default::default());
            }
            let t = target.as_object_mut().unwrap();
            for (k, v) in p {
                if v.is_null() {
                    t.insert(k.clone(), Value::Null);
                } else {
                    merge(t.entry(k.clone()).or_insert(Value::Null), v);
                }
            }
        }
        _ => *target = patch.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn patch_roundtrip() {
        let s = Settings::default();
        let n = s.patched(&json!({"exposureUs": 5000, "tone": {"curve": 2.2}, "roi": {"x": 0, "y": 0, "width": 100, "height": 100}})).unwrap();
        assert_eq!(n.exposure_us, 5000);
        assert_eq!(n.tone.curve, CurveSetting::Gamma(2.2));
        assert!(n.roi.is_some());
        let n = n.patched(&json!({"roi": null})).unwrap();
        assert!(n.roi.is_none());
        assert!(s.patched(&json!({"bitDepth": 10})).is_err());
        assert!(s.patched(&json!({"nope": 1})).is_err());
        let j = serde_json::to_value(&s).unwrap();
        assert_eq!(j["antiFlicker"], "off");
        assert_eq!(j["tone"]["curve"], "srgb");
    }
}

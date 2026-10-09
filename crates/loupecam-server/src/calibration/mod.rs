//! Optional per-camera calibration: black level, defective pixels, measured analog gain
//! stages, flat field, scale presets and a colour matrix.
//!
//! Nothing here is required: without a profile (or with a section switched off in
//! [`crate::settings::CalibrationSettings`]) the app uses its built-in defaults. Each
//! section is measured by a wizard step (see [`runner`]) and stored in
//! `<config>/loupecam/calibration/<serial>.json`.

pub mod analysis;
pub mod runner;
pub mod target;

use loupecam::protocol::sensor::ar1820::GainTable;
use loupecam_isp::correction::{Corrections, FlatField};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Profile {
    pub serial: String,
    pub dark: Option<Dark>,
    pub gain: Option<GainStages>,
    /// Flat fields by size index.
    pub flat_field: BTreeMap<usize, FlatFieldData>,
    pub scale: Vec<ScalePreset>,
    pub color: Option<ColorCalibration>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Dark {
    pub measured_at: String,
    /// Black level per channel R, G, B in 12-bit sensor units.
    pub black_level: [f32; 3],
    /// Defective pixels by size index, in full-frame pixels of that size.
    pub defects: BTreeMap<usize, Vec<(u32, u32)>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GainStages {
    pub measured_at: String,
    /// `(analog code, multiplier relative to the first stage)`.
    pub stages: Vec<(u8, f32)>,
    /// Worst deviation from a straight line in the exposure sweep (fraction).
    pub linearity_error: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FlatFieldData {
    pub measured_at: String,
    pub width: u32,
    pub height: u32,
    pub cols: u32,
    pub rows: u32,
    pub gains: Vec<[f32; 3]>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScalePreset {
    /// User's name for the optical setting, e.g. "2×" or "10× objective".
    pub name: String,
    /// Micrometres per *full-resolution* sensor pixel.
    pub um_per_sensor_pixel: f64,
    pub measured_at: String,
    /// How it was measured, for display (e.g. "ruler, 1 mm lines").
    pub method: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ColorCalibration {
    pub measured_at: String,
    /// White balance at calibration time (R, G, B).
    pub wb: [f32; 3],
    /// Colour matrix (camera RGB after white balance → linear sRGB), row-major.
    pub ccm: [[f32; 3]; 3],
    /// Mean colour error over the chart patches (ΔE-like, in linear RGB × 100).
    pub error: f32,
}

/// The loaded profile plus ready-to-use correction objects.
#[derive(Debug, Default)]
pub struct Calibration {
    pub profile: Profile,
    flat: BTreeMap<usize, Arc<FlatField>>,
    defects: BTreeMap<usize, Arc<Vec<(u32, u32)>>>,
}

impl Calibration {
    pub fn new(profile: Profile) -> Self {
        let flat = profile
            .flat_field
            .iter()
            .map(|(k, f)| (*k, Arc::new(FlatField { width: f.width, height: f.height, cols: f.cols, rows: f.rows, gains: f.gains.clone() })))
            .collect();
        let defects = profile
            .dark
            .as_ref()
            .map(|d| d.defects.iter().map(|(k, v)| (*k, Arc::new(v.clone()))).collect())
            .unwrap_or_default();
        Calibration { profile, flat, defects }
    }

    /// Corrections for a frame at `size_index` whose top-left is at `origin` within the
    /// full frame of that size.
    pub fn corrections(&self, s: &crate::settings::CalibrationSettings, size_index: usize, origin: (u32, u32), frame: (u32, u32)) -> Corrections {
        let flat_field = s
            .flat_field
            .then(|| self.flat.get(&size_index).cloned())
            .flatten()
            // Only if the frame lies within the map's frame (it was measured at this size).
            .filter(|f| origin.0 + frame.0 <= f.width && origin.1 + frame.1 <= f.height);
        let defects = s.defects.then(|| self.defects.get(&size_index).cloned()).flatten();
        Corrections { flat_field, defects, origin }
    }

    /// Analog gain stages to use for gain settings.
    pub fn gain_table(&self, s: &crate::settings::CalibrationSettings) -> GainTable {
        match (&self.profile.gain, s.gain_stages) {
            (Some(g), true) => GainTable::new(g.stages.clone()),
            _ => GainTable::default(),
        }
    }

    /// Black level (12-bit units, mean of channels) to subtract, if measured and enabled.
    pub fn black_level(&self, s: &crate::settings::CalibrationSettings) -> f32 {
        match (&self.profile.dark, s.black_level) {
            (Some(d), true) => d.black_level.iter().sum::<f32>() / 3.0,
            _ => 0.0,
        }
    }

    pub fn color(&self, s: &crate::settings::CalibrationSettings) -> Option<&ColorCalibration> {
        self.profile.color.as_ref().filter(|_| s.color)
    }

    /// µm per full-resolution sensor pixel for the selected scale preset.
    pub fn scale(&self, s: &crate::settings::CalibrationSettings) -> Option<&ScalePreset> {
        let name = s.scale_preset.as_deref()?;
        self.profile.scale.iter().find(|p| p.name == name)
    }
}

/// Where profiles live.
pub fn dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("loupecam").join("calibration"))
}

fn path_for(serial: &str) -> Option<PathBuf> {
    let safe: String = serial.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect();
    (!safe.is_empty()).then(|| dir().map(|d| d.join(format!("{safe}.json")))).flatten()
}

pub fn load(serial: &str) -> Profile {
    let Some(p) = path_for(serial) else { return Profile { serial: serial.into(), ..Default::default() } };
    match std::fs::read(&p).map(|b| serde_json::from_slice::<Profile>(&b)) {
        Ok(Ok(mut prof)) => {
            prof.serial = serial.into();
            prof
        }
        Ok(Err(e)) => {
            tracing::warn!("ignoring calibration {}: {e}", p.display());
            Profile { serial: serial.into(), ..Default::default() }
        }
        Err(_) => Profile { serial: serial.into(), ..Default::default() },
    }
}

pub fn save(profile: &Profile) -> std::io::Result<()> {
    let p = path_for(&profile.serial).ok_or_else(|| std::io::Error::other("no config directory or serial"))?;
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = p.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(profile).unwrap())?;
    std::fs::rename(tmp, p)
}

pub fn now() -> String {
    jiff::Timestamp::now().to_string()
}

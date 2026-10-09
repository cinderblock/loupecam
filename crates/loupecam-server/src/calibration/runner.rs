//! Runs calibration steps: drives the camera through [`Service::measure`] (and the
//! screen target where chosen), analyses, saves into the profile, and reports progress.

use super::analysis::{self, FULL_SCALE, Measurement};
use super::target::{Pattern, TargetHub};
use super::{ColorCalibration, Dark, FlatFieldData, GainStages, ScalePreset, now};
use crate::geometry::{NormRect, display_to_frame};
use crate::service::{MeasureSpec, Service};
use loupecam::Gain;
use loupecam::protocol::sensor::ar1820::ANALOG_STAGES;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::watch;

/// What lights the field for gain / flat-field steps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Light {
    /// The microscope's illuminator on a plain surface (white paper, blank slide).
    Physical,
    /// The screen target showing white.
    Screen,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase", tag = "source")]
pub enum ScaleSource {
    /// Evenly spaced lines (ruler, stage micrometer) `spacing_um` apart.
    Lines { spacing_um: f64 },
    /// The screen target's grid; `pixel_pitch_um` is the screen's pixel pitch.
    Screen { pixel_pitch_um: f64, period_px: u32 },
}

/// A step to run. JSON: `{"step": "scale", "name": "2×", "source": "lines", "spacingUm": 1000}`
/// (the scale source's fields sit alongside the step's).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase", tag = "step")]
pub enum StepRequest {
    Dark,
    Gain { light: Light },
    FlatField { light: Light, size_index: Option<usize> },
    Scale {
        name: String,
        #[serde(flatten)]
        source: ScaleSource,
    },
    Color { region: NormRect },
}

impl StepRequest {
    fn name(&self) -> &'static str {
        match self {
            StepRequest::Dark => "dark",
            StepRequest::Gain { .. } => "gain",
            StepRequest::FlatField { .. } => "flatField",
            StepRequest::Scale { .. } => "scale",
            StepRequest::Color { .. } => "color",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunStatus {
    pub running: Option<String>,
    pub progress: f32,
    pub message: String,
    pub last: Option<Outcome>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Outcome {
    pub step: String,
    pub ok: bool,
    pub summary: String,
    pub at: String,
}

pub struct Runner {
    pub status: watch::Sender<RunStatus>,
    cancel: AtomicBool,
    service: Arc<Service>,
    pub target: Arc<TargetHub>,
}

type StepResult = Result<String, String>;

impl Runner {
    pub fn new(service: Arc<Service>, target: Arc<TargetHub>) -> Arc<Self> {
        Arc::new(Runner { status: watch::Sender::new(RunStatus::default()), cancel: AtomicBool::new(false), service, target })
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
    }

    /// Start a step in the background. Fails if one is already running.
    pub fn start(self: &Arc<Self>, req: StepRequest) -> Result<(), String> {
        let mut started = false;
        self.status.send_if_modified(|s| {
            if s.running.is_none() {
                s.running = Some(req.name().into());
                s.progress = 0.0;
                s.message = "starting".into();
                started = true;
            }
            started
        });
        if !started {
            return Err("a calibration step is already running".into());
        }
        self.cancel.store(false, Ordering::Release);
        let me = self.clone();
        tokio::spawn(async move {
            let name = req.name().to_string();
            let r = me.run(req).await;
            me.target.idle();
            let (ok, summary) = match r {
                Ok(s) => (true, s),
                Err(e) => (false, e),
            };
            if ok {
                me.service.reload_calibration();
            }
            me.status.send_modify(|s| {
                s.running = None;
                s.progress = 1.0;
                s.message = String::new();
                s.last = Some(Outcome { step: name, ok, summary, at: now() });
            });
        });
        Ok(())
    }

    fn progress(&self, p: f32, msg: impl Into<String>) -> Result<(), String> {
        if self.cancel.load(Ordering::Acquire) {
            return Err("cancelled".into());
        }
        let msg = msg.into();
        self.status.send_modify(|s| {
            s.progress = p;
            s.message = msg;
        });
        Ok(())
    }

    async fn run(&self, req: StepRequest) -> StepResult {
        match req {
            StepRequest::Dark => self.dark().await,
            StepRequest::Gain { light } => self.gain(light).await,
            StepRequest::FlatField { light, size_index } => self.flat_field(light, size_index).await,
            StepRequest::Scale { name, source } => self.scale(name, source).await,
            StepRequest::Color { region } => self.color(region).await,
        }
    }

    fn serial(&self) -> Result<String, String> {
        self.service.shared.state.borrow().device.as_ref().map(|d| d.serial.clone()).ok_or_else(|| "no camera connected".into())
    }

    fn sizes(&self) -> usize {
        self.service.shared.state.borrow().device.as_ref().map_or(1, |d| d.resolutions.len())
    }

    /// Modify the stored profile and save it.
    fn update_profile(&self, f: impl FnOnce(&mut super::Profile)) -> Result<(), String> {
        let mut p = super::load(&self.serial()?);
        f(&mut p);
        super::save(&p).map_err(|e| format!("saving calibration: {e}"))
    }

    fn black(&self) -> [f32; 3] {
        self.service.shared.calibration.borrow().profile.dark.as_ref().map_or([0.0; 3], |d| d.black_level)
    }

    async fn measure(&self, size_index: usize, exposure_us: u32, gain: Gain, frames: u32) -> Result<Measurement, String> {
        if self.cancel.load(Ordering::Acquire) {
            return Err("cancelled".into());
        }
        self.service.measure(MeasureSpec { size_index, exposure_us, gain, frames }).await
    }

    /// Find an exposure (at unity gain) giving a green level near `target` (fraction of
    /// full scale) at `size_index`.
    async fn find_exposure(&self, size_index: usize, target: f32) -> Result<(u32, Measurement), String> {
        let s = self.service.shared.state.borrow().settings.clone();
        let black = self.black();
        // Start from what auto exposure is doing now.
        let mut exp = ((s.exposure_us as f32 * s.gain).clamp(50.0, 2_000_000.0)) as u32;
        let mut last = None;
        for _ in 0..6 {
            let m = self.measure(size_index, exp, Gain::UNITY, 2).await?;
            let lvl = analysis::level(&m, black).max(1.0) / FULL_SCALE;
            let clipped = m.clipped() > 0.01;
            if (lvl / target - 1.0).abs() < 0.15 && !clipped {
                return Ok((exp, m));
            }
            let ratio = if clipped { 0.3 } else { (target / lvl).clamp(0.05, 20.0) };
            let next = ((exp as f32 * ratio).clamp(50.0, 2_000_000.0)) as u32;
            if next == exp {
                last = Some(m);
                break;
            }
            exp = next;
            last = Some(m);
        }
        match last {
            Some(m) if analysis::level(&m, black) / FULL_SCALE > target * 0.3 => Ok((exp, m)),
            _ => Err("could not reach a usable brightness: add light (or check the lens cap is off)".into()),
        }
    }

    async fn light(&self, light: Light) -> Result<(), String> {
        if light == Light::Screen {
            self.target.show(Pattern::Solid { level: 1.0 }).await?;
        }
        Ok(())
    }

    async fn dark(&self) -> StepResult {
        self.progress(0.05, "measuring black level")?;
        let short = self.measure(0, 1000, Gain::UNITY, 8).await?;
        let black = analysis::black_level(&short);
        let mut defects = std::collections::BTreeMap::new();
        let n = self.sizes();
        let hot_gain = Gain::for_multiplier(4.0).unwrap_or(Gain::UNITY);
        let mut counts = Vec::new();
        for size in 0..n {
            self.progress(0.2 + 0.75 * size as f32 / n as f32, format!("finding defective pixels ({}/{n})", size + 1))?;
            let long = self.measure(size, 400_000, hot_gain, 3).await?;
            let d = analysis::find_defects(&long, black)?;
            counts.push(d.len());
            defects.insert(size, d);
        }
        self.update_profile(|p| p.dark = Some(Dark { measured_at: now(), black_level: black, defects }))?;
        Ok(format!(
            "black level {:.1} / {:.1} / {:.1} (R/G/B, 12-bit); defective pixels per resolution: {counts:?}",
            black[0], black[1], black[2]
        ))
    }

    async fn gain(&self, light: Light) -> StepResult {
        self.light(light).await?;
        let size = self.sizes().saturating_sub(1); // the smallest size: fastest
        let black = self.black();
        self.progress(0.05, "finding exposure")?;
        let (mut exp, base) = self.find_exposure(size, 0.08).await?;
        let base_level = analysis::level(&base, black);
        // Mains-powered lights flicker at 100 or 120 Hz; 50 ms spans whole cycles of
        // both, so every frame integrates the same light. Use a multiple of it when the
        // highest stage (~4×) still stays well clear of clipping.
        const FLICKER_SAFE_US: u32 = 50_000;
        let flicker_exp = exp.div_ceil(FLICKER_SAFE_US) * FLICKER_SAFE_US;
        let projected = base_level * flicker_exp as f32 / exp.max(1) as f32;
        if flicker_exp != exp && projected * 4.5 < 0.85 * FULL_SCALE {
            exp = flicker_exp;
        }
        let flicker_safe = exp % FLICKER_SAFE_US == 0;
        let frames = if flicker_safe { 4 } else { 8 };
        // Interleave unity-gain reference measurements between the stages, so slow
        // drift in the light cancels: ratio = stage / mean(reference before, after).
        let unity = |m: &Measurement| analysis::level(m, black);
        let mut reference = unity(&self.measure(size, exp, Gain::UNITY, frames).await?);
        let mut stages = vec![(ANALOG_STAGES[0].0, 1.0f32)];
        let mut drift = 0f32;
        for (i, &(code, _)) in ANALOG_STAGES.iter().enumerate().skip(1) {
            self.progress(0.15 + 0.5 * i as f32 / ANALOG_STAGES.len() as f32, format!("measuring analog stage {code:#04x}"))?;
            let m = self.measure(size, exp, Gain { analog_code: code, digital: 64 }, frames).await?;
            if m.clipped() > 0.01 {
                return Err(format!("stage {code:#04x} clipped; reduce the light"));
            }
            let after = unity(&self.measure(size, exp, Gain::UNITY, frames).await?);
            drift = drift.max((after / reference.max(1.0) - 1.0).abs());
            stages.push((code, analysis::level(&m, black) / ((reference + after) / 2.0).max(1.0)));
            reference = after;
        }
        // Linearity: levels across an exposure sweep at unity gain.
        let mut points = Vec::new();
        for (i, f) in [0.5f32, 1.0, 2.0, 4.0].into_iter().enumerate() {
            self.progress(0.65 + 0.3 * i as f32 / 4.0, format!("checking linearity ({}×)", f))?;
            let m = self.measure(size, (exp as f32 * f) as u32, Gain::UNITY, 3).await?;
            if m.clipped() < 0.001 {
                points.push((m.exposure_us, analysis::level(&m, black) as f64));
            }
        }
        let k = points.iter().map(|(t, l)| t * l).sum::<f64>() / points.iter().map(|(t, _)| t * t).sum::<f64>().max(1e-9);
        let linearity = points.iter().map(|(t, l)| ((l - k * t) / (k * t)).abs()).fold(0.0, f64::max) as f32;
        self.update_profile(|p| p.gain = Some(GainStages { measured_at: now(), stages: stages.clone(), linearity_error: linearity }))?;
        let list: Vec<String> = stages.iter().map(|(c, m)| format!("{c:#04x}: {m:.2}×")).collect();
        let mut s = format!("analog stages {}; linearity within {:.1} %", list.join(", "), linearity * 100.0);
        if !flicker_safe {
            s.push_str(&format!(" (exposure {:.1} ms is too short to average out light flicker; frames were averaged instead)", exp as f32 / 1000.0));
        }
        if drift > 0.03 {
            s.push_str(&format!(" (the light drifted {:.0} % between measurements; results may be less accurate)", drift * 100.0));
        }
        Ok(s)
    }

    async fn flat_field(&self, light: Light, size_index: Option<usize>) -> StepResult {
        self.light(light).await?;
        let size = size_index.unwrap_or_else(|| self.service.shared.state.borrow().settings.size_index);
        self.progress(0.1, "finding exposure")?;
        let (exp, _) = self.find_exposure(size, 0.5).await?;
        self.progress(0.4, "averaging frames")?;
        let m = self.measure(size, exp, Gain::UNITY, 8).await?;
        self.progress(0.9, "computing flat field")?;
        let ff = analysis::flat_field(&m, self.black(), 48)?;
        let worst = ff.gains.iter().flatten().fold(1f32, |a, &g| a.max(g));
        self.update_profile(|p| {
            p.flat_field.insert(
                size,
                FlatFieldData { measured_at: now(), width: ff.width, height: ff.height, cols: ff.cols, rows: ff.rows, gains: ff.gains.clone() },
            );
        })?;
        Ok(format!("flat field for {}×{} saved (strongest correction {worst:.2}×). Enable it in Calibration to use it.", ff.width, ff.height))
    }

    async fn scale(&self, name: String, source: ScaleSource) -> StepResult {
        let name = name.trim().to_string();
        if name.is_empty() {
            return Err("give the preset a name (e.g. the zoom setting)".into());
        }
        let spacing_um = match source {
            ScaleSource::Lines { spacing_um } => spacing_um,
            ScaleSource::Screen { pixel_pitch_um, period_px } => {
                self.target.show(Pattern::Grid { period: period_px, line: (period_px / 6).max(1) }).await?;
                pixel_pitch_um * period_px as f64
            }
        };
        if !spacing_um.is_finite() || spacing_um <= 0.0 {
            return Err("line spacing must be positive".into());
        }
        let (dev, s) = {
            let st = self.service.shared.state.borrow();
            (st.device.clone().ok_or("no camera connected")?, st.settings.clone())
        };
        // A mid size: enough pixels for sub-pixel accuracy, quick to analyse.
        let size = 1.min(dev.resolutions.len() - 1);
        self.progress(0.2, "capturing")?;
        let gains = self.service.shared.calibration.borrow().gain_table(&s.calibration);
        let m = self.measure(size, s.exposure_us, gains.for_multiplier(s.gain.min(gains.max_multiplier())).unwrap_or(Gain::UNITY), 2).await?;
        self.progress(0.8, "finding line spacing")?;
        let (px, py) = analysis::line_periods(&m);
        let period = match (px, py) {
            (Some(a), Some(b)) => (a.pixels * a.strength as f64 + b.pixels * b.strength as f64) / (a.strength + b.strength) as f64,
            (Some(a), None) | (None, Some(a)) => a.pixels,
            (None, None) => return Err("no regular lines found: focus on the ruler/grid, fill the view with several lines, and try again".into()),
        };
        let factor = dev.resolutions[0][0] as f64 / dev.resolutions[size][0] as f64;
        let um_per_sensor_pixel = spacing_um / period / factor;
        let method = match source {
            ScaleSource::Lines { spacing_um } => format!("lines {spacing_um} µm apart"),
            ScaleSource::Screen { pixel_pitch_um, period_px } => format!("screen grid, {period_px} px × {pixel_pitch_um:.1} µm"),
        };
        let preset = ScalePreset { name: name.clone(), um_per_sensor_pixel, measured_at: now(), method };
        self.update_profile(|p| {
            p.scale.retain(|q| q.name != name);
            p.scale.push(preset);
        })?;
        self.service
            .patch(serde_json::json!({ "calibration": { "scalePreset": name } }))
            .await?;
        let field_mm = dev.resolutions[0][0] as f64 * um_per_sensor_pixel / 1000.0;
        Ok(format!("{um_per_sensor_pixel:.3} µm per sensor pixel (field of view {field_mm:.1} mm wide); selected as the scale preset"))
    }

    async fn color(&self, region: NormRect) -> StepResult {
        let (s, env) = {
            let st = self.service.shared.state.borrow().settings.clone();
            (st, self.service.shared.frame.borrow().clone().ok_or("no frame yet")?)
        };
        let o = s.develop_params_uncalibrated(env.model, s.preview.demosaic).orientation;
        let r = display_to_frame(region, o, env.raw.width, env.raw.height);
        self.progress(0.2, "capturing the chart")?;
        let gains = self.service.shared.calibration.borrow().gain_table(&s.calibration);
        let m = self
            .measure(env.size_index, s.exposure_us, gains.for_multiplier(s.gain.min(gains.max_multiplier())).unwrap_or(Gain::UNITY), 4)
            .await?;
        // `r` is relative to the (possibly ROI) preview frame; the measurement is the full frame.
        let (ox, oy) = env.origin;
        let rect = (r.x + ox, r.y + oy, r.width, r.height);
        self.progress(0.7, "fitting the colour matrix")?;
        let black = self.black();
        let fit = fit_chart(&m, rect, black, s.white_balance.gains).ok_or("could not fit: is the whole chart inside the box?")?;
        let (ccm, err) = fit;
        if err > 8.0 {
            return Err(format!("the chart did not match well (error {err:.1}); draw the box tightly around the 24 patches"));
        }
        self.update_profile(|p| p.color = Some(ColorCalibration { measured_at: now(), wb: s.white_balance.gains, ccm, error: err }))?;
        self.service.patch(serde_json::json!({ "calibration": { "color": true } })).await?;
        Ok(format!("colour matrix fitted (mean error {err:.1}); enabled"))
    }
}

/// X-Rite ColorChecker Classic, sRGB (D65) 8-bit values, row by row (4 × 6).
const CHART_SRGB: [[u8; 3]; 24] = [
    [115, 82, 68], [194, 150, 130], [98, 122, 157], [87, 108, 67], [133, 128, 177], [103, 189, 170],
    [214, 126, 44], [80, 91, 166], [193, 90, 99], [94, 60, 108], [157, 188, 64], [224, 163, 46],
    [56, 61, 150], [70, 148, 73], [175, 54, 60], [231, 199, 31], [187, 86, 149], [8, 133, 161],
    [243, 243, 242], [200, 200, 200], [160, 160, 160], [122, 122, 121], [85, 85, 85], [52, 52, 52],
];

fn srgb_to_linear(v: u8) -> f32 {
    let c = v as f32 / 255.0;
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

/// Sample the 24 patches inside `rect` (x, y, w, h in measurement pixels), trying the
/// chart's possible orientations, and fit a colour matrix. Returns the matrix and the
/// mean patch error (linear RGB × 100) of the best orientation.
fn fit_chart(m: &Measurement, rect: (u32, u32, u32, u32), black: [f32; 3], wb: [f32; 3]) -> Option<([[f32; 3]; 3], f32)> {
    let (x0, y0, w, h) = rect;
    if w < 24 || h < 16 || x0 + w > m.width || y0 + h > m.height {
        return None;
    }
    let reference: Vec<[f32; 3]> = CHART_SRGB.iter().map(|p| p.map(srgb_to_linear)).collect();
    // Patch grid as seen in the box: 6×4 landscape or 4×6 portrait.
    let (gc, gr) = if w >= h { (6u32, 4u32) } else { (4, 6) };
    let mut cells = Vec::new();
    for r in 0..gr {
        for c in 0..gc {
            // The central 40 % of each cell, avoiding the black gaps between patches.
            let (cx0, cx1) = (x0 + (w * (5 * c + 1)) / (5 * gc) + w / (5 * gc), x0 + (w * (5 * c + 4)) / (5 * gc) - w / (5 * gc));
            let (cy0, cy1) = (y0 + (h * (5 * r + 1)) / (5 * gr) + h / (5 * gr), y0 + (h * (5 * r + 4)) / (5 * gr) - h / (5 * gr));
            cells.push(mean_rgb(m, (cx0, cy0, cx1.max(cx0 + 2), cy1.max(cy0 + 2)), black, wb));
        }
    }
    // Map chart patch index (row-major, 4×6) to the cell it lands in, for each
    // orientation the chart can lie in the box.
    type CellOf = fn(usize, usize) -> (u32, u32);
    let orientations: [CellOf; 2] = if gc == 6 {
        [|r, c| (c as u32, r as u32), |r, c| (5 - c as u32, 3 - r as u32)]
    } else {
        [|r, c| (3 - r as u32, c as u32), |r, c| (r as u32, 5 - c as u32)]
    };
    let mut best: Option<([[f32; 3]; 3], f32)> = None;
    for map in orientations {
        let cam: Vec<[f32; 3]> = (0..24)
            .map(|i| {
                let (cx, cy) = map(i / 6, i % 6);
                cells[(cy * gc + cx) as usize]
            })
            .collect();
        // Scale the camera values so the neutral patches match in brightness.
        let k = (18..24).map(|i| reference[i][1]).sum::<f32>() / (18..24).map(|i| cam[i][1]).sum::<f32>().max(1e-6);
        let cam: Vec<[f32; 3]> = cam.iter().map(|c| c.map(|v| v * k)).collect();
        let Some(ccm) = analysis::fit_ccm(&cam, &reference) else { continue };
        let err = cam
            .iter()
            .zip(&reference)
            .map(|(c, r)| {
                let out: [f32; 3] = std::array::from_fn(|i| (0..3).map(|j| ccm[i][j] * c[j]).sum());
                ((out[0] - r[0]).powi(2) + (out[1] - r[1]).powi(2) + (out[2] - r[2]).powi(2)).sqrt() * 100.0
            })
            .sum::<f32>()
            / 24.0;
        if best.as_ref().is_none_or(|b| err < b.1) {
            best = Some((ccm, err));
        }
    }
    best
}

/// Mean linear R, G, B (black-subtracted, white-balanced, 0..1) over a rectangle.
fn mean_rgb(m: &Measurement, (x0, y0, x1, y1): (u32, u32, u32, u32), black: [f32; 3], wb: [f32; 3]) -> [f32; 3] {
    let (rx, ry) = match m.pattern {
        loupecam::BayerPattern::Rggb => (0, 0),
        loupecam::BayerPattern::Grbg => (1, 0),
        loupecam::BayerPattern::Gbrg => (0, 1),
        loupecam::BayerPattern::Bggr => (1, 1),
    };
    let mut sum = [0f64; 3];
    let mut n = [0u32; 3];
    for y in y0..y1.min(m.height) {
        for x in x0..x1.min(m.width) {
            let c = match ((x & 1) == rx, (y & 1) == ry) {
                (true, true) => 0,
                (false, false) => 2,
                _ => 1,
            };
            sum[c] += (m.mean[(y * m.width + x) as usize] - black[c]).max(0.0) as f64;
            n[c] += 1;
        }
    }
    std::array::from_fn(|c| (sum[c] / n[c].max(1) as f64) as f32 / FULL_SCALE * wb[c])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_requests_parse_like_the_ui_sends_them() {
        let r: StepRequest = serde_json::from_str(r#"{"step":"scale","name":"2x","source":"screen","pixelPitchUm":276.9,"periodPx":16}"#).unwrap();
        assert_eq!(r, StepRequest::Scale { name: "2x".into(), source: ScaleSource::Screen { pixel_pitch_um: 276.9, period_px: 16 } });
        let r: StepRequest = serde_json::from_str(r#"{"step":"flatField","light":"screen","sizeIndex":1}"#).unwrap();
        assert_eq!(r, StepRequest::FlatField { light: Light::Screen, size_index: Some(1) });
        let r: StepRequest = serde_json::from_str(r#"{"step":"color","region":{"x":0.1,"y":0.1,"width":0.5,"height":0.4}}"#).unwrap();
        assert!(matches!(r, StepRequest::Color { .. }));
    }

    /// Render a chart through a known "camera" matrix and check the fit inverts it.
    #[test]
    fn chart_fit_recovers_colour() {
        let reference: Vec<[f32; 3]> = CHART_SRGB.iter().map(|p| p.map(srgb_to_linear)).collect();
        // Camera = inverse of a typical CCM (desaturated, channel crosstalk), then WB.
        let cam_of = |r: [f32; 3]| [0.7 * r[0] + 0.2 * r[1] + 0.1 * r[2], 0.15 * r[0] + 0.7 * r[1] + 0.15 * r[2], 0.1 * r[0] + 0.25 * r[1] + 0.65 * r[2]];
        let (w, h) = (600u32, 400u32);
        // `rot`: the chart lies upside down (patch order reversed), as when placed rotated.
        let render = |rot: bool| {
            let mut mean = vec![0f32; (w * h) as usize];
            for y in 0..h {
                for x in 0..w {
                    let (mut c, mut r) = ((x * 6 / w) as usize, (y * 4 / h) as usize);
                    if rot {
                        (c, r) = (5 - c, 3 - r);
                    }
                    let rgb = cam_of(reference[r * 6 + c]);
                    let ch = match (x & 1, y & 1) {
                        (0, 0) => 0,
                        (1, 1) => 2,
                        _ => 1,
                    };
                    mean[(y * w + x) as usize] = 64.0 + rgb[ch] * 3000.0;
                }
            }
            Measurement { width: w, height: h, pattern: loupecam::BayerPattern::Rggb, mean, exposure_us: 1.0, frames: 1 }
        };
        let (ccm, err) = fit_chart(&render(false), (0, 0, w, h), [64.0; 3], [1.0; 3]).unwrap();
        assert!(err < 1.0, "error {err}, ccm {ccm:?}");
        let (_, err2) = fit_chart(&render(true), (0, 0, w, h), [64.0; 3], [1.0; 3]).unwrap();
        assert!(err2 < 2.0, "flipped error {err2}");
    }
}

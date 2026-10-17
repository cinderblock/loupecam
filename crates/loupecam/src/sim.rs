//! A simulated camera, for developing and testing without hardware.
//!
//! [`SimCamera`] mirrors the parts of [`Camera`](crate::Camera) that applications use. It
//! renders a synthetic specimen (a slowly drifting grid, discs and colour patches) as
//! a Bayer mosaic, at the configured size, ROI, bit depth and frame rate, with
//! brightness following exposure and gain, so auto exposure and white balance behave.

use crate::stream::{Shared, StreamStats};
use crate::{FrameReceiver, Result};
use loupecam_protocol::Error as ProtocolError;
use loupecam_protocol::flash::FactoryData;
use loupecam_protocol::frame::{RawFrame, SampleFormat, Trailer};
use loupecam_protocol::model::{BayerPattern, ColorFilter, MU1803_HS, Model, Resolution};
use loupecam_protocol::sensor::ar1820::{self, Gain};
use loupecam_protocol::session::{DeviceInfo, Roi, StreamConfig, StreamGeometry};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// What the generator thread renders.
#[derive(Debug, Clone, Copy)]
struct Params {
    width: u32,
    height: u32,
    format: SampleFormat,
    /// Full-sensor pixels per output pixel.
    factor: u32,
    /// ROI origin in output pixels.
    origin: (u32, u32),
    exposure_us: f64,
    gain: f32,
    frame_interval: Duration,
}

struct Running {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    shared: Arc<Shared>,
}

impl Drop for Running {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        self.shared.close();
    }
}

pub struct SimCamera {
    info: DeviceInfo,
    config: Option<StreamConfig>,
    params: Arc<Mutex<Params>>,
    running: Option<Running>,
}

impl SimCamera {
    /// A simulated MU1803-HS.
    pub fn open() -> Self {
        let model = &MU1803_HS;
        let serial = "SIMULATED".to_string();
        let info = DeviceInfo {
            model,
            serial: serial.clone(),
            firmware_version: "sim".into(),
            hardware_version: "sim".into(),
            fpga_version: (0, 0),
            sensor_id: 0,
            production_date: None,
            factory: FactoryData { serial, header: [0; 5], payload: Vec::new() },
        };
        let params = Params {
            width: 0,
            height: 0,
            format: SampleFormat::U8,
            factor: 1,
            origin: (0, 0),
            exposure_us: 20_000.0,
            gain: 1.0,
            frame_interval: Duration::from_millis(33),
        };
        SimCamera { info, config: None, params: Arc::new(Mutex::new(params)), running: None }
    }

    pub fn info(&self) -> &DeviceInfo {
        &self.info
    }

    pub fn model(&self) -> &'static Model {
        self.info.model
    }

    pub fn config(&self) -> Option<&StreamConfig> {
        self.config.as_ref()
    }

    pub fn geometry(&self) -> Option<StreamGeometry> {
        let cfg = self.config.as_ref()?;
        let size = ar1820::SIZES.get(cfg.size_index)?;
        let llp = size.line_length(cfg.speed, cfg.pixel_mode.sample_format() != SampleFormat::U8);
        let lines = ar1820::exposure_lines(cfg.exposure_us, llp);
        let p = self.params.lock().unwrap();
        Some(StreamGeometry {
            resolution: Resolution::new(p.width, p.height),
            format: p.format,
            line_length_pck: llp,
            frame_length_lines: size.frame_length_lines,
            exposure_us: ar1820::lines_to_us(lines, llp),
        })
    }

    pub fn is_streaming(&self) -> bool {
        self.running.is_some()
    }

    pub fn start(&mut self, cfg: &StreamConfig) -> Result<FrameReceiver> {
        self.stop()?;
        self.config = Some(cfg.clone());
        self.update()?;
        let shared = Arc::new(Shared::new());
        let stop = Arc::new(AtomicBool::new(false));
        let (s, st, params) = (shared.clone(), stop.clone(), self.params.clone());
        let thread = std::thread::Builder::new()
            .name("loupecam-sim".into())
            .spawn(move || generate(params, s, st))
            .expect("spawn simulator thread");
        self.running = Some(Running { stop, thread: Some(thread), shared: shared.clone() });
        Ok(FrameReceiver { shared })
    }

    pub fn stop(&mut self) -> Result<()> {
        self.running = None;
        Ok(())
    }

    pub fn set_exposure(&mut self, exposure_us: u32) -> Result<f64> {
        self.with_config(|c| c.exposure_us = exposure_us)?;
        Ok(self.geometry().map_or(exposure_us as f64, |g| g.exposure_us))
    }

    pub fn set_gain(&mut self, gain: Gain) -> Result<()> {
        self.with_config(|c| c.gain = gain)
    }

    pub fn set_speed(&mut self, speed: u8) -> Result<StreamGeometry> {
        self.with_config(|c| c.speed = speed)?;
        self.geometry().ok_or_else(not_streaming)
    }

    pub fn set_binning(&mut self, binning: bool) -> Result<()> {
        self.with_config(|c| c.binning = binning)
    }

    pub fn set_roi(&mut self, roi: Option<Roi>) -> Result<StreamGeometry> {
        self.with_config(|c| c.roi = roi)?;
        self.geometry().ok_or_else(not_streaming)
    }

    pub fn stats(&self) -> Option<StreamStats> {
        self.running.as_ref().map(|r| r.shared.stats.lock().unwrap().clone())
    }

    fn with_config(&mut self, f: impl FnOnce(&mut StreamConfig)) -> Result<()> {
        let c = self.config.as_mut().ok_or_else(not_streaming)?;
        f(c);
        self.update()
    }

    /// Recompute the generator's parameters from the configuration.
    fn update(&mut self) -> Result<()> {
        let cfg = self.config.as_ref().ok_or_else(not_streaming)?;
        let size = ar1820::SIZES.get(cfg.size_index).ok_or_else(|| ProtocolError::OutOfRange(format!("size index {}", cfg.size_index)))?;
        let (width, height, origin) = match cfg.roi {
            Some(r) => (r.width as u32 & !3, r.height as u32 & !1, ((r.x & !3) as u32, (r.y & !1) as u32)),
            None => (size.width as u32, size.height as u32, (0, 0)),
        };
        let geom = self.geometry().expect("configured");
        let lines = ar1820::exposure_lines(cfg.exposure_us, geom.line_length_pck);
        let fps = geom.max_fps(lines);
        let mut p = self.params.lock().unwrap();
        *p = Params {
            width,
            height,
            format: cfg.pixel_mode.sample_format(),
            factor: size.factor as u32,
            origin,
            exposure_us: geom.exposure_us,
            gain: cfg.gain.multiplier(),
            frame_interval: Duration::from_secs_f64(1.0 / fps),
        };
        Ok(())
    }
}

fn not_streaming() -> crate::Error {
    ProtocolError::OutOfRange("not streaming".into()).into()
}

fn generate(params: Arc<Mutex<Params>>, shared: Arc<Shared>, stop: Arc<AtomicBool>) {
    let t0 = Instant::now();
    let mut next = Instant::now();
    let mut sequence = 0u32;
    let mut window = (Instant::now(), 0u32);
    while !stop.load(Ordering::Acquire) {
        let p = *params.lock().unwrap();
        next += p.frame_interval;
        let frame = render(&p, t0.elapsed().as_secs_f32(), sequence);
        shared.push(frame);
        sequence = sequence.wrapping_add(1);
        {
            let mut st = shared.stats.lock().unwrap();
            st.frames += 1;
            st.last_sequence = Some(sequence);
            window.1 += 1;
            let dt = window.0.elapsed().as_secs_f64();
            if dt >= 1.0 {
                st.fps = window.1 as f64 / dt;
                window = (Instant::now(), 0);
            }
        }
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        } else {
            // Rendering can't keep up: don't try to catch up.
            next = now;
        }
    }
}

/// Reflectance (linear RGB, 0..1) of the simulated specimen at full-sensor coordinates.
fn scene(x: f32, y: f32, t: f32) -> [f32; 3] {
    // The specimen drifts slowly, as on a stage.
    let (x, y) = (x + 150.0 * (t * 0.15).sin(), y + 100.0 * (t * 0.11).cos());
    let mut c = [0.62, 0.58, 0.50];
    // A grid of dark lines every 200 px (graph paper), wide enough not to alias at the
    // subsampled sizes.
    if x.rem_euclid(200.0) < 16.0 || y.rem_euclid(200.0) < 16.0 {
        c = [0.15, 0.15, 0.18];
    }
    // Coloured discs.
    let discs: [(f32, f32, f32, [f32; 3]); 5] = [
        (1400.0, 1200.0, 420.0, [0.55, 0.12, 0.10]),
        (3300.0, 1000.0, 300.0, [0.10, 0.40, 0.15]),
        (2500.0, 2500.0, 520.0, [0.12, 0.18, 0.55]),
        (900.0, 2800.0, 260.0, [0.70, 0.62, 0.12]),
        (4000.0, 2900.0, 350.0, [0.40, 0.12, 0.45]),
    ];
    for (cx, cy, r, col) in discs {
        let d = ((x - cx).powi(2) + (y - cy).powi(2)).sqrt();
        if d < r {
            // A soft rim so there are gradients to focus on.
            let k = ((r - d) / 40.0).min(1.0);
            for i in 0..3 {
                c[i] = c[i] * (1.0 - k) + col[i] * k;
            }
        }
    }
    c
}

/// Relative channel sensitivity, like the real sensor's: its red and blue respond
/// weakly, which the vendor's daylight white balance (1.53, 1, 1.71) compensates.
const RESPONSE: [f32; 3] = [1.0 / 1.53, 1.0, 1.0 / 1.71];

fn render(p: &Params, t: f32, sequence: u32) -> RawFrame {
    let (w, h) = (p.width as usize, p.height as usize);
    let max = match p.format {
        SampleFormat::U8 => 255.0,
        SampleFormat::U16 { bits } => ((1u32 << bits) - 1) as f32,
    };
    // 20 ms at unity gain puts the background near 60 % of full scale.
    let scale = (p.exposure_us as f32 * p.gain / 20_000.0) * max;
    let pattern = match MU1803_HS.color {
        ColorFilter::Bayer(b) => b,
        ColorFilter::Mono => BayerPattern::Rggb,
    };
    let bytes = p.format.bytes_per_sample();
    let mut data = vec![0u8; w * h * bytes];
    let f = p.factor as f32;
    let mut noise = sequence.wrapping_mul(2_654_435_761);
    for y in 0..h {
        for x in 0..w {
            let (sx, sy) = ((x as u32 + p.origin.0) as f32 * f, (y as u32 + p.origin.1) as f32 * f);
            let refl = scene(sx, sy, t);
            let ch = channel(pattern, x, y);
            // A little sensor noise.
            noise ^= noise << 13;
            noise ^= noise >> 17;
            noise ^= noise << 5;
            let n = (noise & 0xff) as f32 / 255.0 - 0.5;
            let v = (refl[ch] * RESPONSE[ch] * scale + n * max * 0.01).clamp(0.0, max);
            let i = y * w + x;
            if bytes == 1 {
                data[i] = v as u8;
            } else {
                data[2 * i..2 * i + 2].copy_from_slice(&(v as u16).to_le_bytes());
            }
        }
    }
    RawFrame { width: p.width, height: p.height, format: p.format, data, trailer: Trailer { stats: [[0; 3]; 2], sequence } }
}

/// Colour channel (0 R, 1 G, 2 B) of the mosaic at (x, y).
fn channel(pattern: BayerPattern, x: usize, y: usize) -> usize {
    let (odd_x, odd_y) = (x & 1 == 1, y & 1 == 1);
    let (r_x, r_y) = match pattern {
        BayerPattern::Rggb => (false, false),
        BayerPattern::Grbg => (true, false),
        BayerPattern::Gbrg => (false, true),
        BayerPattern::Bggr => (true, true),
    };
    match (odd_x == r_x, odd_y == r_y) {
        (true, true) => 0,
        (false, false) => 2,
        _ => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use loupecam_protocol::session::PixelMode;

    #[test]
    fn streams_and_follows_settings() {
        let mut cam = SimCamera::open();
        let rx = cam.start(&StreamConfig::new(2, PixelMode::Raw8)).unwrap();
        let f = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!((f.width, f.height, f.format), (1228, 922, SampleFormat::U8));
        let mean = |f: &RawFrame| f.data.iter().map(|&v| v as f64).sum::<f64>() / f.data.len() as f64;
        let before = mean(&f);
        cam.set_exposure(5_000).unwrap();
        // Skip a frame that may have been rendered before the change.
        rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let after = mean(&rx.recv_timeout(Duration::from_secs(5)).unwrap());
        assert!(after < before * 0.5, "{before} -> {after}");
        let g = cam.set_roi(Some(Roi { x: 100, y: 50, width: 403, height: 301 })).unwrap();
        assert_eq!((g.resolution.width, g.resolution.height), (400, 300));
        cam.stop().unwrap();
        while rx.try_recv().is_some() {}
        assert!(rx.recv_timeout(Duration::from_secs(1)).is_err());
    }
}

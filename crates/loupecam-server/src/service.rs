//! The camera actor: one thread owns the USB camera, applies settings, runs auto
//! exposure / white balance, and publishes frames, stats and state.
//!
//! Everything else talks to it through [`Service`]: commands go in over a channel,
//! and state comes out through `tokio::sync::watch` channels, which any number of
//! HTTP/WebSocket clients can observe.

use crate::settings::{Settings, WbMode};
use loupecam::{Camera, FrameReceiver, Gain, Model, RawFrame, RecvError};
use loupecam_isp::auto::{AutoExposure, Exposure, grey_world};
use loupecam_isp::stats::{self, Rect};
use serde::Serialize;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{oneshot, watch};

/// Connection/streaming status.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "state", content = "detail")]
pub enum Status {
    /// No camera found yet (detail: last error).
    Searching(Option<String>),
    Streaming,
    /// Reconfiguring (e.g. switching to full resolution for a capture).
    Busy(String),
}

/// What clients need to know about the connected camera.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceSummary {
    pub model: String,
    pub serial: String,
    pub firmware_version: String,
    pub hardware_version: String,
    pub fpga_version: String,
    pub production_date: Option<String>,
    pub resolutions: Vec<[u32; 2]>,
    pub pixel_size_um: f32,
    pub max_bit_depth: u8,
    pub max_gain: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct State {
    pub status: Status,
    pub device: Option<DeviceSummary>,
    pub settings: Settings,
}

/// Live measurements, published a few times per second.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveStats {
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub max_fps: f64,
    pub frames: u64,
    pub dropped: u64,
    pub sequence_gaps: u64,
    /// Exposure actually programmed (after line quantisation), µs.
    pub exposure_us: f64,
    pub gain: f32,
    /// Mean linear level per channel (before white balance), 0..1.
    pub mean: [f32; 3],
    /// Fraction of the frame clipped.
    pub clipped: f32,
    /// 64-bin luma histogram in display (tone-mapped) space.
    pub histogram: Vec<u32>,
}

/// A frame plus what is needed to interpret it.
#[derive(Clone)]
pub struct FrameEnvelope {
    pub raw: Arc<RawFrame>,
    pub model: &'static Model,
}

/// A frame captured for saving.
pub struct Captured {
    pub frame: FrameEnvelope,
    pub settings: Settings,
    pub exposure_us: f64,
    pub gain: f32,
}

type Reply<T> = oneshot::Sender<Result<T, String>>;

enum Command {
    Patch(serde_json::Value, Reply<Settings>),
    Capture { full_resolution: bool, reply: Reply<Captured> },
    WhiteBalance { region: Option<Rect>, reply: Reply<[f32; 3]> },
    Shutdown,
}

/// Channels shared between the actor and its observers.
pub struct Shared {
    pub state: watch::Sender<State>,
    pub stats: watch::Sender<LiveStats>,
    pub frame: watch::Sender<Option<FrameEnvelope>>,
}

/// Handle to the camera actor.
pub struct Service {
    tx: Mutex<mpsc::Sender<Command>>,
    pub shared: Arc<Shared>,
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl Service {
    pub fn spawn(settings: Settings) -> Arc<Self> {
        let (tx, rx) = mpsc::channel();
        let shared = Arc::new(Shared {
            state: watch::Sender::new(State { status: Status::Searching(None), device: None, settings: settings.clone() }),
            stats: watch::Sender::new(LiveStats::default()),
            frame: watch::Sender::new(None),
        });
        let actor = Actor::new(shared.clone(), rx, settings);
        let thread = std::thread::Builder::new()
            .name("loupecam-camera".into())
            .spawn(move || actor.run())
            .expect("spawn camera thread");
        Arc::new(Service { tx: Mutex::new(tx), shared, thread: Mutex::new(Some(thread)) })
    }

    async fn call<T>(&self, make: impl FnOnce(Reply<T>) -> Command) -> Result<T, String> {
        let (reply, rx) = oneshot::channel();
        self.tx.lock().unwrap().send(make(reply)).map_err(|_| "camera service stopped".to_string())?;
        rx.await.map_err(|_| "camera service stopped".to_string())?
    }

    /// Apply a JSON merge patch to the settings. Returns the new settings.
    pub async fn patch(&self, patch: serde_json::Value) -> Result<Settings, String> {
        self.call(|r| Command::Patch(patch, r)).await
    }

    /// Grab a frame for saving, switching to full resolution if asked.
    pub async fn capture(&self, full_resolution: bool) -> Result<Captured, String> {
        self.call(|reply| Command::Capture { full_resolution, reply }).await
    }

    /// One-shot white balance over a region (frame pixels) assumed neutral, or the whole
    /// frame (grey world) when `None`.
    pub async fn white_balance(&self, region: Option<Rect>) -> Result<[f32; 3], String> {
        self.call(|reply| Command::WhiteBalance { region, reply }).await
    }

    pub fn shutdown(&self) {
        let _ = self.tx.lock().unwrap().send(Command::Shutdown);
        if let Some(t) = self.thread.lock().unwrap().take() {
            let _ = t.join();
        }
    }
}

struct Live {
    cam: Camera,
    frames: FrameReceiver,
    last_frame: Instant,
}

struct Actor {
    shared: Arc<Shared>,
    rx: mpsc::Receiver<Command>,
    settings: Settings,
    live: Option<Live>,
    status: Status,
    device: Option<DeviceSummary>,
    /// Frames to wait after an exposure change before judging it.
    ae_cooldown: u32,
    last_stats: Instant,
    last_state: Instant,
    state_dirty: bool,
    latest: Option<FrameEnvelope>,
}

/// Frames to discard after a reconfiguration before trusting exposure.
const SETTLE_FRAMES: u32 = 4;
/// No frames for this long means the stream is dead; reconnect.
const STALL: Duration = Duration::from_secs(5);

impl Actor {
    fn new(shared: Arc<Shared>, rx: mpsc::Receiver<Command>, settings: Settings) -> Self {
        Actor {
            shared,
            rx,
            settings,
            live: None,
            status: Status::Searching(None),
            device: None,
            ae_cooldown: SETTLE_FRAMES,
            last_stats: Instant::now(),
            last_state: Instant::now(),
            state_dirty: true,
            latest: None,
        }
    }

    fn run(mut self) {
        loop {
            if self.live.is_none()
                && let Err(e) = self.connect()
            {
                self.set_status(Status::Searching(Some(e)));
                // Wait for a command or retry in a second.
                match self.rx.recv_timeout(Duration::from_secs(1)) {
                    Ok(Command::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => return,
                    Ok(cmd) => self.handle(cmd),
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
                self.publish_state(false);
                continue;
            }
            let live = self.live.as_mut().unwrap();
            match live.frames.recv_timeout(Duration::from_millis(50)) {
                Ok(f) => {
                    live.last_frame = Instant::now();
                    self.on_frame(f);
                }
                Err(RecvError::Timeout) => {
                    if live.last_frame.elapsed() > STALL {
                        self.disconnect("no frames from camera".into());
                    }
                }
                Err(RecvError::Stopped) => self.disconnect("stream stopped".into()),
            }
            loop {
                match self.rx.try_recv() {
                    Ok(Command::Shutdown) | Err(mpsc::TryRecvError::Disconnected) => {
                        if let Some(l) = self.live.as_mut() {
                            let _ = l.cam.stop();
                        }
                        return;
                    }
                    Ok(cmd) => self.handle(cmd),
                    Err(mpsc::TryRecvError::Empty) => break,
                }
            }
            self.publish_state(false);
        }
    }

    fn connect(&mut self) -> Result<(), String> {
        let mut cam = Camera::open_first().map_err(|e| e.to_string())?;
        let frames = cam.start(&self.settings.stream_config()).map_err(|e| e.to_string())?;
        let i = cam.info();
        self.device = Some(DeviceSummary {
            model: i.model.name.into(),
            serial: i.serial.clone(),
            firmware_version: i.firmware_version.clone(),
            hardware_version: i.hardware_version.clone(),
            fpga_version: format!("{}.{}", i.fpga_version.0, i.fpga_version.1),
            production_date: i.production_date.map(|(y, m, d)| format!("{y:04}-{m:02}-{d:02}")),
            resolutions: i.model.resolutions.iter().map(|r| [r.width, r.height]).collect(),
            pixel_size_um: i.model.pixel_size_um,
            max_bit_depth: i.model.max_bit_depth,
            max_gain: Gain::max_multiplier(),
        });
        tracing::info!(serial = %i.serial, "camera connected and streaming");
        self.live = Some(Live { cam, frames, last_frame: Instant::now() });
        self.ae_cooldown = SETTLE_FRAMES;
        self.set_status(Status::Streaming);
        Ok(())
    }

    fn disconnect(&mut self, why: String) {
        tracing::warn!("camera lost: {why}");
        self.live = None;
        self.device = None;
        self.latest = None;
        self.shared.frame.send_replace(None);
        self.set_status(Status::Searching(Some(why)));
    }

    fn set_status(&mut self, s: Status) {
        if self.status != s {
            self.status = s;
            self.publish_state(true);
        }
    }

    /// Publish state now if `force`, else at most twice a second while dirty.
    fn publish_state(&mut self, force: bool) {
        if !force && !(self.state_dirty && self.last_state.elapsed() > Duration::from_millis(500)) {
            return;
        }
        self.shared.state.send_replace(State {
            status: self.status.clone(),
            device: self.device.clone(),
            settings: self.settings.clone(),
        });
        self.last_state = Instant::now();
        self.state_dirty = false;
    }

    fn on_frame(&mut self, f: RawFrame) {
        let Some(live) = self.live.as_ref() else { return };
        let model = live.cam.model();
        let env = FrameEnvelope { raw: Arc::new(f), model };
        let pattern = match model.color {
            loupecam::ColorFilter::Bayer(p) => p,
            loupecam::ColorFilter::Mono => loupecam::BayerPattern::Rggb,
        };
        let f = &env.raw;
        let cells = (f.width as u64 * f.height as u64 / 4).max(1);
        let step = ((cells as f64 / 40_000.0).sqrt().ceil() as u32).max(1);
        let st = stats::compute(f, pattern, None, step, self.settings.white_balance.gains);

        if self.ae_cooldown > 0 {
            self.ae_cooldown -= 1;
        } else if self.settings.auto_exposure {
            self.auto_expose(&st);
        }
        if self.settings.white_balance.mode == WbMode::Auto {
            let target = grey_world(&st);
            let g = &mut self.settings.white_balance.gains;
            for k in 0..3 {
                g[k] += 0.15 * (target[k] - g[k]);
            }
            self.state_dirty = true;
        }
        if self.last_stats.elapsed() > Duration::from_millis(200) {
            self.publish_stats(&env, &st);
        }
        self.latest = Some(env.clone());
        self.shared.frame.send_replace(Some(env));
    }

    fn auto_expose(&mut self, st: &stats::Stats) {
        let ae = AutoExposure {
            target: self.settings.ae_target,
            max_exposure_us: self.settings.ae_max_exposure_us,
            max_gain: self.settings.ae_max_gain.min(Gain::max_multiplier()),
            ..Default::default()
        };
        let cur = Exposure { exposure_us: self.settings.exposure_us, gain: self.settings.gain };
        if let Some(next) = ae.update(st, cur) {
            let mut n = self.settings.clone();
            n.exposure_us = next.exposure_us.max(1);
            n.gain = next.gain;
            if let Err(e) = self.apply(n) {
                tracing::warn!("auto exposure: {e}");
            }
            self.ae_cooldown = 2;
        }
    }

    fn publish_stats(&mut self, env: &FrameEnvelope, st: &stats::Stats) {
        let Some(live) = self.live.as_ref() else { return };
        let s = live.frames.stats();
        let geom = live.cam.geometry();
        let tone = self.settings.develop_params(env.model, self.settings.preview.demosaic).tone;
        let mut hist = vec![0u32; 64];
        for (i, &n) in st.histogram.iter().enumerate() {
            let v = tone.eval((i as f32 + 0.5) / 256.0);
            hist[((v * 64.0) as usize).min(63)] += n;
        }
        self.shared.stats.send_replace(LiveStats {
            width: env.raw.width,
            height: env.raw.height,
            fps: s.fps,
            max_fps: geom.map_or(0.0, |g| {
                g.max_fps(loupecam::protocol::sensor::ar1820::exposure_lines(self.settings.exposure_us, g.line_length_pck))
            }),
            frames: s.frames,
            dropped: s.dropped,
            sequence_gaps: s.sequence_gaps,
            exposure_us: geom.map_or(0.0, |g| g.exposure_us),
            gain: self.settings.gain,
            mean: st.mean,
            clipped: st.clipped,
            histogram: hist,
        });
        self.last_stats = Instant::now();
    }

    fn handle(&mut self, cmd: Command) {
        match cmd {
            Command::Patch(patch, reply) => {
                let r = self
                    .settings
                    .patched(&patch)
                    .map_err(|e| e.to_string())
                    .and_then(|n| self.apply(n).map(|()| self.settings.clone()));
                self.publish_state(true);
                let _ = reply.send(r);
            }
            Command::Capture { full_resolution, reply } => {
                let r = self.capture(full_resolution);
                let _ = reply.send(r);
            }
            Command::WhiteBalance { region, reply } => {
                let r = self.white_balance(region);
                self.publish_state(true);
                let _ = reply.send(r);
            }
            Command::Shutdown => unreachable!("handled by the run loop"),
        }
    }

    /// Make `next` the active settings, reprogramming the camera as needed.
    fn apply(&mut self, next: Settings) -> Result<(), String> {
        let prev = std::mem::replace(&mut self.settings, next.clone());
        self.state_dirty = true;
        let Some(live) = self.live.as_mut() else { return Ok(()) };
        let r = (|| -> Result<(), loupecam::Error> {
            if prev.needs_restart(&next) {
                live.frames = live.cam.start(&next.stream_config())?;
                self.ae_cooldown = SETTLE_FRAMES;
                return Ok(());
            }
            let (pc, nc) = (prev.stream_config(), next.stream_config());
            if pc.speed != nc.speed {
                live.cam.set_speed(nc.speed)?;
            }
            if pc.exposure_us != nc.exposure_us || pc.speed != nc.speed {
                live.cam.set_exposure(nc.exposure_us)?;
            }
            if pc.gain != nc.gain {
                live.cam.set_gain(nc.gain)?;
            }
            if pc.binning != nc.binning {
                live.cam.set_binning(nc.binning)?;
            }
            if pc.roi != nc.roi {
                live.cam.set_roi(nc.roi)?;
                self.ae_cooldown = SETTLE_FRAMES;
            }
            Ok(())
        })();
        r.map_err(|e| {
            let msg = e.to_string();
            self.disconnect(msg.clone());
            msg
        })
    }

    fn capture(&mut self, full_resolution: bool) -> Result<Captured, String> {
        let live = self.live.as_mut().ok_or("no camera connected")?;
        let model = live.cam.model();
        let grab = |frames: &FrameReceiver, skip: u32| -> Result<RawFrame, String> {
            for _ in 0..skip {
                frames.recv_timeout(Duration::from_secs(5)).map_err(|e| e.to_string())?;
            }
            frames.recv_timeout(Duration::from_secs(5)).map_err(|e| e.to_string())
        };
        let switch = full_resolution && (self.settings.size_index != 0 || self.settings.roi.is_some() || self.settings.bit_depth != 12);
        let raw = if switch {
            self.status = Status::Busy("capturing at full resolution".into());
            self.shared.state.send_modify(|s| s.status = self.status.clone());
            // Stills always use 12-bit samples.
            let mut cfg = self.settings.stream_config();
            cfg.size_index = 0;
            cfg.roi = None;
            cfg.pixel_mode = loupecam::PixelMode::Raw12;
            let r = live
                .cam
                .start(&cfg)
                .map_err(|e| e.to_string())
                .and_then(|frames| grab(&frames, SETTLE_FRAMES));
            let restore = live.cam.start(&self.settings.stream_config()).map_err(|e| e.to_string());
            match restore {
                Ok(frames) => {
                    live.frames = frames;
                    live.last_frame = Instant::now();
                    self.ae_cooldown = SETTLE_FRAMES;
                    self.set_status(Status::Streaming);
                }
                Err(e) => self.disconnect(e),
            }
            r?
        } else {
            // A fresh frame, not one that may predate the request.
            grab(&live.frames, 0)?
        };
        let geom = self.live.as_ref().and_then(|l| l.cam.geometry());
        Ok(Captured {
            frame: FrameEnvelope { raw: Arc::new(raw), model },
            settings: self.settings.clone(),
            exposure_us: geom.map_or(self.settings.exposure_us as f64, |g| g.exposure_us),
            gain: self.settings.gain,
        })
    }

    fn white_balance(&mut self, region: Option<Rect>) -> Result<[f32; 3], String> {
        let env = self.latest.as_ref().ok_or("no frame yet")?;
        let pattern = match env.model.color {
            loupecam::ColorFilter::Bayer(p) => p,
            loupecam::ColorFilter::Mono => return Err("monochrome camera".into()),
        };
        let st = stats::compute(&env.raw, pattern, region, 1, [1.0; 3]);
        if st.cells == 0 {
            return Err("empty region".into());
        }
        let g = grey_world(&st);
        self.settings.white_balance.gains = g;
        self.settings.white_balance.mode = WbMode::Manual;
        self.state_dirty = true;
        Ok(g)
    }
}

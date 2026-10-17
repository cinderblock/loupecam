//! Video and timelapse recording to H.264 MP4 in the captures directory.
//!
//! The recorder develops raw frames itself (independently of the preview, at its own
//! size and rate), converts them to YUV and feeds an [`encoder`]. The result is
//! written as fragmented MP4 ([`mp4`]), so an interrupted recording stays playable.

pub mod encoder;
pub mod mp4;
#[cfg(target_os = "linux")]
mod v4l2;

use crate::calibration::Calibration;
use crate::captures;
use crate::service::{Captured, FrameEnvelope, Service};
use crate::settings::{DemosaicSetting, RecordMode, RecordSettings, Settings};
use encoder::{EncoderConfig, VideoEncoder};
use loupecam_isp::{Image8, develop, resize, yuv};
use mp4::Fmp4Writer;
use serde::Serialize;
use std::fs::File;
use std::io::BufWriter;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{oneshot, watch};

/// The recording in progress.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Recording {
    /// The video file in the captures directory (`None` for a timelapse of stills only,
    /// when no encoder is available).
    pub name: Option<String>,
    pub mode: RecordMode,
    pub encoder: Option<&'static str>,
    pub width: u32,
    pub height: u32,
    pub started: String,
    /// Wall-clock time recording.
    pub seconds: f64,
    pub frames: u64,
    pub bytes: u64,
    /// Stills saved (timelapse with `timelapseStills`).
    pub stills: u32,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordStatus {
    pub active: Option<Recording>,
    /// The last finished recording.
    pub last: Option<Recording>,
    /// Why the last recording failed or stopped early.
    pub error: Option<String>,
    /// H.264 encoders this build can use, in order of preference.
    pub encoders: Vec<&'static str>,
}

enum Slot {
    Idle,
    Starting,
    Running { stop: oneshot::Sender<()>, task: tokio::task::JoinHandle<()> },
}

pub struct Recorder {
    pub status: watch::Sender<RecordStatus>,
    service: Arc<Service>,
    captures_dir: PathBuf,
    running: Mutex<Slot>,
}

impl Recorder {
    pub fn new(service: Arc<Service>, captures_dir: PathBuf) -> Arc<Self> {
        let status = RecordStatus { encoders: encoder::backends(), ..Default::default() };
        Arc::new(Recorder { status: watch::Sender::new(status), service, captures_dir, running: Mutex::new(Slot::Idle) })
    }

    /// Start recording with the current `record` settings.
    pub async fn start(self: &Arc<Self>) -> Result<Recording, String> {
        // Claim the slot first so concurrent starts can't both proceed.
        {
            let mut slot = self.running.lock().unwrap();
            match &*slot {
                Slot::Starting => return Err("already starting a recording".into()),
                Slot::Running { task, .. } if !task.is_finished() => return Err("already recording".into()),
                _ => *slot = Slot::Starting,
            }
        }
        match self.prepare().await {
            Ok(job) => {
                let rec = job.recording.clone();
                self.status.send_modify(|s| {
                    s.active = Some(rec.clone());
                    s.error = None;
                });
                let me = self.clone();
                let (stop, stop_rx) = oneshot::channel();
                let task = tokio::spawn(async move { me.run(job, stop_rx).await });
                *self.running.lock().unwrap() = Slot::Running { stop, task };
                tracing::info!(file = ?rec.name, mode = ?rec.mode, "recording {}x{}", rec.width, rec.height);
                Ok(rec)
            }
            Err(e) => {
                *self.running.lock().unwrap() = Slot::Idle;
                self.status.send_modify(|s| s.error = Some(e.clone()));
                Err(e)
            }
        }
    }

    /// Stop recording and wait for the file to be finished. Returns the recording, or
    /// `None` if nothing was recording.
    pub async fn stop(&self) -> Option<Recording> {
        let (stop, task) = {
            let mut slot = self.running.lock().unwrap();
            match std::mem::replace(&mut *slot, Slot::Idle) {
                Slot::Running { stop, task } => (stop, task),
                other => {
                    *slot = other;
                    return None;
                }
            }
        };
        let _ = stop.send(());
        let _ = task.await;
        self.status.borrow().last.clone()
    }

    async fn prepare(&self) -> Result<Job, String> {
        let shared = &self.service.shared;
        let settings = shared.state.borrow().settings.clone();
        let rs = settings.record.clone();
        // Switch the stream size for the recording if asked.
        let mut restore_size = None;
        if let Some(i) = rs.stream_size_index
            && i != settings.size_index
        {
            self.service.patch(serde_json::json!({ "sizeIndex": i })).await?;
            restore_size = Some((settings.size_index, i));
        }
        let first = self.first_frame(restore_size.map(|(_, i)| i)).await;
        let fail = |e: String| async {
            if let Some((prev, _)) = restore_size {
                let _ = self.service.patch(serde_json::json!({ "sizeIndex": prev })).await;
            }
            Err::<Job, _>(e)
        };
        let Some(first) = first else { return fail("no frames from camera".into()).await };
        let (w, h) = output_size(&first, &settings, &rs);
        let fps = match rs.mode {
            RecordMode::Video => rs.max_fps,
            RecordMode::Timelapse => rs.timelapse_fps,
        };
        let cfg = EncoderConfig {
            width: w,
            height: h,
            fps,
            bitrate_bps: rs.bitrate_kbps.saturating_mul(1000),
            keyframe_interval: (2.0 * fps).round().max(1.0) as u32,
        };
        let enc = tokio::task::spawn_blocking(move || encoder::open(&cfg)).await.map_err(|e| e.to_string())?;
        let enc = match enc {
            Ok(e) => Some(e),
            Err(_) if rs.mode == RecordMode::Timelapse && rs.timelapse_stills => None,
            Err(e) => return fail(format!("{e:#}")).await,
        };
        let now = jiff::Zoned::now();
        let (name, writer) = match &enc {
            None => (None, None),
            Some(_) => {
                let dir = self.captures_dir.clone();
                let made = tokio::task::spawn_blocking(move || -> std::io::Result<(String, File)> {
                    std::fs::create_dir_all(&dir)?;
                    let stem = captures::unique_stem(&dir, &now.strftime("%Y%m%d-%H%M%S").to_string(), "mp4");
                    let name = format!("{stem}.mp4");
                    Ok((name.clone(), File::create(dir.join(&name))?))
                })
                .await
                .map_err(|e| e.to_string())?;
                match made {
                    Ok((name, file)) => (Some(name), Some(Fmp4Writer::new(BufWriter::new(file), w, h))),
                    Err(e) => return fail(format!("creating the video file: {e}")).await,
                }
            }
        };
        let recording = Recording {
            name,
            mode: rs.mode,
            encoder: enc.as_ref().map(|e| e.name()),
            width: w,
            height: h,
            started: jiff::Timestamp::now().to_string(),
            seconds: 0.0,
            frames: 0,
            bytes: 0,
            stills: 0,
        };
        Ok(Job { recording, settings: rs, enc, writer, restore_size })
    }

    /// The next frame (at stream size `size`, if given), waiting up to 10 s.
    async fn first_frame(&self, size: Option<usize>) -> Option<FrameEnvelope> {
        let mut rx = self.service.shared.frame.subscribe();
        let wait = async {
            loop {
                rx.changed().await.ok()?;
                let f = rx.borrow_and_update().clone();
                if let Some(f) = f
                    && size.is_none_or(|s| f.size_index == s)
                {
                    return Some(f);
                }
            }
        };
        tokio::time::timeout(Duration::from_secs(10), wait).await.ok().flatten()
    }

    async fn run(self: Arc<Self>, mut job: Job, mut stop: oneshot::Receiver<()>) {
        let shared = self.service.shared.clone();
        let mut frames = shared.frame.subscribe();
        let t0 = Instant::now();
        let rs = job.settings.clone();
        let min_interval = Duration::from_secs_f32(1.0 / rs.max_fps);
        let interval = Duration::from_secs_f32(rs.timelapse_interval_s);
        let mut next_tick = tokio::time::Instant::now();
        let mut last_frame: Option<Instant> = None;
        let mut last_status = Instant::now();
        let mut last_sync = Instant::now();
        let mut error = None;
        loop {
            // Wait for the next frame to record.
            let woke = match rs.mode {
                RecordMode::Video => {
                    let wait = async {
                        frames.changed().await.ok()?;
                        if let Some(l) = last_frame {
                            let since = l.elapsed();
                            if since < min_interval {
                                tokio::time::sleep(min_interval - since).await;
                            }
                        }
                        Some(())
                    };
                    tokio::select! { _ = &mut stop => None, r = wait => r }
                }
                RecordMode::Timelapse => {
                    tokio::select! { _ = &mut stop => None, () = tokio::time::sleep_until(next_tick) => Some(()) }
                }
            };
            if woke.is_none() {
                break;
            }
            let Some(env) = frames.borrow_and_update().clone() else { continue };
            let time = match rs.mode {
                RecordMode::Video => t0.elapsed(),
                RecordMode::Timelapse => {
                    // Keep the cadence, but don't try to catch up on ticks missed while
                    // a slow frame was being processed.
                    next_tick = (next_tick + interval).max(tokio::time::Instant::now());
                    Duration::from_secs_f64(job.recording.frames as f64 / rs.timelapse_fps as f64)
                }
            };
            last_frame = Some(Instant::now());
            let settings = shared.state.borrow().settings.clone();
            let calibration = shared.calibration.borrow().clone();
            let exposure_us = shared.stats.borrow().exposure_us;
            let dir = self.captures_dir.clone();
            let sync = last_sync.elapsed() > Duration::from_secs(5);
            let (enc, writer) = (job.enc.take(), job.writer.take());
            let (w, h) = (job.recording.width, job.recording.height);
            let r = tokio::task::spawn_blocking(move || {
                let mut enc = enc;
                let mut writer = writer;
                let r = (|| -> anyhow::Result<bool> {
                    if let (Some(enc), Some(writer)) = (enc.as_mut(), writer.as_mut()) {
                        let img = develop_video(&env, &settings, &calibration, w, h);
                        let out = enc.encode(&yuv::rgb_to_i420(&img), time)?;
                        write(writer, &out, rs.mode == RecordMode::Timelapse)?;
                        if sync {
                            writer.get_ref().get_ref().sync_data()?;
                        }
                    }
                    if rs.mode == RecordMode::Timelapse && rs.timelapse_stills {
                        let gain = settings.gain;
                        let c = Captured { frame: env, calibration, settings, exposure_us, gain };
                        captures::save(&c, &dir)?;
                        return Ok(true);
                    }
                    Ok(false)
                })();
                (enc, writer, r)
            })
            .await;
            let Ok((enc, writer, r)) = r else {
                error = Some("recording task panicked".to_string());
                break;
            };
            (job.enc, job.writer) = (enc, writer);
            if sync {
                last_sync = Instant::now();
            }
            match r {
                Ok(still) => {
                    job.recording.frames += 1;
                    job.recording.stills += still as u32;
                }
                Err(e) => {
                    error = Some(format!("{e:#}"));
                    break;
                }
            }
            if last_status.elapsed() > Duration::from_millis(500) {
                job.recording.seconds = t0.elapsed().as_secs_f64();
                job.recording.bytes = job.writer.as_ref().map_or(0, |w| w.bytes());
                let rec = job.recording.clone();
                self.status.send_modify(|s| s.active = Some(rec));
                last_status = Instant::now();
            }
        }
        // Finish the file.
        job.recording.seconds = t0.elapsed().as_secs_f64();
        let (enc, writer) = (job.enc.take(), job.writer.take());
        let timelapse = rs.mode == RecordMode::Timelapse;
        let finished = tokio::task::spawn_blocking(move || -> anyhow::Result<u64> {
            let (Some(mut enc), Some(mut writer)) = (enc, writer) else { return Ok(0) };
            write(&mut writer, &enc.finish()?, timelapse)?;
            let file = writer.finish()?.into_inner().map_err(|e| e.into_error())?;
            file.sync_all()?;
            Ok(file.metadata()?.len())
        })
        .await;
        match finished {
            Ok(Ok(bytes)) => job.recording.bytes = bytes,
            Ok(Err(e)) => error = error.or(Some(format!("finishing the video: {e:#}"))),
            Err(e) => error = error.or(Some(e.to_string())),
        }
        if let Some((prev, ours)) = job.restore_size
            && self.service.shared.state.borrow().settings.size_index == ours
        {
            let _ = self.service.patch(serde_json::json!({ "sizeIndex": prev })).await;
        }
        match &error {
            Some(e) => tracing::warn!(file = ?job.recording.name, "recording stopped: {e}"),
            None => tracing::info!(file = ?job.recording.name, frames = job.recording.frames, "recording finished"),
        }
        self.status.send_modify(|s| {
            s.active = None;
            s.last = Some(job.recording.clone());
            s.error = error;
        });
    }
}

struct Job {
    recording: Recording,
    settings: RecordSettings,
    enc: Option<Box<dyn VideoEncoder>>,
    writer: Option<Fmp4Writer<BufWriter<File>>>,
    /// (previous, recording) stream size index, when recording switched it.
    restore_size: Option<(usize, usize)>,
}

/// Mux encoder output. Timelapse frames each get their own fragment, so little is lost
/// if a long timelapse is interrupted.
fn write(writer: &mut Fmp4Writer<BufWriter<File>>, out: &[encoder::Encoded], fragment_each: bool) -> std::io::Result<()> {
    for e in out {
        if fragment_each {
            writer.request_flush();
        }
        let pts = (e.time.as_secs_f64() * mp4::TIMESCALE as f64).round() as u64;
        writer.push(&mp4::parse_annex_b(&e.annex_b), pts)?;
    }
    Ok(())
}

/// Video size for frames like `f`: their displayed (oriented) size fitted within the
/// limits, never enlarged, rounded down to even numbers (for 4:2:0).
fn output_size(f: &FrameEnvelope, s: &Settings, rs: &RecordSettings) -> (u32, u32) {
    let (w, h) = oriented_size(f.raw.width, f.raw.height, s);
    let (fw, fh) = fit(w, h, rs.max_width, rs.max_height);
    ((fw & !1).max(2), (fh & !1).max(2))
}

fn oriented_size(w: u32, h: u32, s: &Settings) -> (u32, u32) {
    if s.orientation.rotation % 180 == 90 { (h, w) } else { (w, h) }
}

/// `w × h` scaled to fit within `max_w × max_h`, never enlarged.
fn fit(w: u32, h: u32, max_w: u32, max_h: u32) -> (u32, u32) {
    let scale = (max_w as f64 / w as f64).min(max_h as f64 / h as f64).min(1.0);
    (((w as f64 * scale).round() as u32).max(1), ((h as f64 * scale).round() as u32).max(1))
}

/// Develop a frame to exactly `w × h`, letterboxed if its aspect ratio differs (the
/// stream changed shape mid-recording, e.g. a new ROI).
fn develop_video(env: &FrameEnvelope, s: &Settings, cal: &Calibration, w: u32, h: u32) -> Image8 {
    let (sw, _) = oriented_size(env.raw.width, env.raw.height, s);
    // Superpixel halves the size and is cheaper and alias-free when shrinking anyway.
    let demosaic = if sw / w >= 2 { DemosaicSetting::Superpixel } else { DemosaicSetting::Bilinear };
    let img = develop(&env.raw, &s.develop_params(env, demosaic, cal));
    let (fw, fh) = fit(img.width, img.height, w, h);
    let (fw, fh) = if fw.abs_diff(w) <= 2 && fh.abs_diff(h) <= 2 { (w, h) } else { (fw, fh) };
    let img = resize(&img, fw, fh);
    if (fw, fh) == (w, h) {
        return img;
    }
    let mut out = Image8::new(w, h, 3);
    let (ox, oy) = (((w - fw) / 2) as usize, ((h - fh) / 2) as usize);
    for y in 0..fh as usize {
        let src = &img.data[y * fw as usize * 3..(y + 1) * fw as usize * 3];
        let o = ((oy + y) * w as usize + ox) * 3;
        out.data[o..o + src.len()].copy_from_slice(src);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(fit(1228, 922, 1280, 960), (1228, 922));
        assert_eq!(fit(2456, 1842, 1920, 1080), (1440, 1080));
        assert_eq!(fit(4912, 3684, 1920, 1080), (1440, 1080));
        assert_eq!(fit(1000, 2000, 1920, 1080), (540, 1080));
    }

    /// Encode a moving test pattern, mux it, and have ffmpeg check the file (skipped when
    /// ffmpeg isn't on PATH). Also checks that a file cut off mid-fragment still plays.
    #[cfg(feature = "openh264")]
    #[test]
    fn encode_and_mux_roundtrip() {
        use std::process::Command;
        let (w, h, n) = (320u32, 240u32, 90u32);
        let cfg = EncoderConfig { width: w, height: h, fps: 30.0, bitrate_bps: 1_000_000, keyframe_interval: 30 };
        let mut enc = encoder::open(&cfg).unwrap();
        let path = std::env::temp_dir().join(format!("loupecam-test-{}.mp4", std::process::id()));
        let mut writer = Fmp4Writer::new(BufWriter::new(File::create(&path).unwrap()), w, h);
        for i in 0..n {
            let mut img = Image8::new(w, h, 3);
            for (k, px) in img.data.chunks_mut(3).enumerate() {
                let (x, y) = (k as u32 % w, k as u32 / w);
                px.copy_from_slice(&[((x + i * 4) % 256) as u8, (y % 256) as u8, 128]);
            }
            let out = enc.encode(&yuv::rgb_to_i420(&img), Duration::from_secs_f64(i as f64 / 30.0)).unwrap();
            write(&mut writer, &out, false).unwrap();
        }
        write(&mut writer, &enc.finish().unwrap(), false).unwrap();
        writer.finish().unwrap();

        let probe = |p: &std::path::Path| {
            Command::new("ffprobe")
                .args(["-v", "error", "-count_frames", "-select_streams", "v:0", "-show_entries"])
                .args(["stream=codec_name,width,height,nb_read_frames,color_space", "-of", "csv=p=0"])
                .arg(p)
                .output()
        };
        let Ok(out) = probe(&path) else {
            eprintln!("ffprobe not found; skipping the container check");
            return;
        };
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let text = String::from_utf8_lossy(&out.stdout);
        assert_eq!(text.trim(), format!("h264,{w},{h},bt709,{n}"), "ffprobe: {text}");
        // Decoding the whole file reports no errors.
        let d = Command::new("ffmpeg").args(["-v", "error", "-i"]).arg(&path).args(["-f", "null", "-"]).output().unwrap();
        assert!(d.stderr.is_empty(), "{}", String::from_utf8_lossy(&d.stderr));

        // Truncate mid-file, as a power cut would: the complete fragments still decode.
        let bytes = std::fs::read(&path).unwrap();
        let cut = path.with_extension("cut.mp4");
        std::fs::write(&cut, &bytes[..bytes.len() * 2 / 3]).unwrap();
        let out = probe(&cut).unwrap();
        let frames: u32 = String::from_utf8_lossy(&out.stdout).trim().rsplit(',').next().unwrap().parse().unwrap();
        assert!(frames >= 30 && frames < n, "{frames} frames survive truncation");
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&cut);
    }

    #[cfg(feature = "openh264")]
    #[tokio::test(flavor = "multi_thread")]
    async fn records_video_and_timelapse() {
        use crate::settings::RecordPreset;
        let dir = std::env::temp_dir().join(format!("loupecam-rec-{}", std::process::id()));
        let service = Service::spawn_with(Settings::default(), crate::service::Source::Simulated);
        let rec = Recorder::new(service.clone(), dir.clone());

        // Smooth: the 1228×922 stream fits 1280×960, so it's recorded as is.
        let r = rec.start().await.unwrap();
        assert_eq!((r.width, r.height, r.mode), (1228, 922, RecordMode::Video));
        assert!(rec.start().await.is_err(), "a second start must fail");
        tokio::time::sleep(Duration::from_millis(1500)).await;
        let done = rec.stop().await.unwrap();
        // Debug builds simulate and develop frames slowly.
        let least = if cfg!(debug_assertions) { 1 } else { 10 };
        assert!(done.frames >= least && done.bytes > 10_000, "{done:?}");
        assert!(dir.join(done.name.as_ref().unwrap()).is_file());
        assert!(rec.stop().await.is_none());

        // Timelapse with stills, fast.
        let ts = RecordSettings { timelapse_interval_s: 0.25, timelapse_stills: true, ..RecordPreset::Timelapse.settings(&Default::default()) };
        service.patch(serde_json::json!({ "record": ts, "capture": { "format": "jpeg" } })).await.unwrap();
        let r = rec.start().await.unwrap();
        assert_eq!(r.mode, RecordMode::Timelapse);
        tokio::time::sleep(Duration::from_millis(1100)).await;
        let done = rec.stop().await.unwrap();
        let least = if cfg!(debug_assertions) { 1 } else { 3 };
        assert!((least..=6).contains(&done.frames) && done.stills as u64 == done.frames, "{done:?}");
        let stills = std::fs::read_dir(&dir).unwrap().filter(|e| e.as_ref().unwrap().path().extension().is_some_and(|x| x == "jpg")).count();
        assert_eq!(stills as u32, done.stills);
        assert!(rec.status.borrow().error.is_none());

        let s = service.clone();
        tokio::task::spawn_blocking(move || s.shutdown()).await.unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}

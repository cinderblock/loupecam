//! H.264 encoders. openh264 (software) where it's compiled in (the `openh264` feature).
//! The Raspberry Pi 4's hardware encoder (V4L2 mem2mem) is next.

use loupecam_isp::yuv::I420;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EncoderConfig {
    pub width: u32,
    pub height: u32,
    /// Nominal frame rate (rate control and keyframe spacing).
    pub fps: f32,
    pub bitrate_bps: u32,
    /// Frames between keyframes.
    pub keyframe_interval: u32,
}

/// An encoded access unit (Annex-B byte stream) and the time of the frame it encodes.
#[derive(Debug, Clone)]
pub struct Encoded {
    pub annex_b: Vec<u8>,
    pub time: Duration,
}

pub trait VideoEncoder: Send {
    /// Backend name, for status and logs.
    fn name(&self) -> &'static str;
    /// Encode a frame. Hardware encoders work asynchronously, so this returns whatever
    /// output is ready (possibly from earlier frames).
    fn encode(&mut self, frame: &I420, time: Duration) -> anyhow::Result<Vec<Encoded>>;
    /// Drain frames still in flight.
    fn finish(&mut self) -> anyhow::Result<Vec<Encoded>>;
}

/// Names of the encoders this build can try, in order of preference.
pub fn backends() -> Vec<&'static str> {
    vec![
        #[cfg(feature = "openh264")]
        "openh264",
    ]
}

/// Open the first encoder that works for `cfg`.
#[cfg_attr(not(feature = "openh264"), allow(unused_variables, unused_mut))]
pub fn open(cfg: &EncoderConfig) -> anyhow::Result<Box<dyn VideoEncoder>> {
    let mut errors: Vec<String> = Vec::new();
    #[cfg(feature = "openh264")]
    match software::Openh264::open(cfg) {
        Ok(e) => return Ok(Box::new(e)),
        Err(e) => errors.push(format!("openh264: {e:#}")),
    }
    if errors.is_empty() {
        anyhow::bail!("no H.264 encoder in this build");
    }
    anyhow::bail!("no usable H.264 encoder ({})", errors.join("; "))
}

#[cfg(feature = "openh264")]
mod software {
    use super::*;
    use openh264::OpenH264API;
    use openh264::encoder::{BitRate, Encoder, FrameRate, IntraFramePeriod, RateControlMode, UsageType, VuiConfig};
    use openh264::formats::YUVSlices;

    pub struct Openh264 {
        enc: Encoder,
    }

    impl Openh264 {
        pub fn open(cfg: &EncoderConfig) -> anyhow::Result<Self> {
            let config = openh264::encoder::EncoderConfig::new()
                .bitrate(BitRate::from_bps(cfg.bitrate_bps))
                .max_frame_rate(FrameRate::from_hz(cfg.fps))
                .usage_type(UsageType::CameraVideoRealTime)
                .rate_control_mode(RateControlMode::Bitrate)
                .intra_frame_period(IntraFramePeriod::from_num_frames(cfg.keyframe_interval.max(1)))
                .vui(VuiConfig::bt709());
            Ok(Openh264 { enc: Encoder::with_api_config(OpenH264API::from_source(), config)? })
        }
    }

    impl VideoEncoder for Openh264 {
        fn name(&self) -> &'static str {
            "openh264"
        }

        fn encode(&mut self, f: &I420, time: Duration) -> anyhow::Result<Vec<Encoded>> {
            let (w, h) = (f.width as usize, f.height as usize);
            let yuv = YUVSlices::new((f.y(), f.u(), f.v()), (w, h), (w, w / 2, w / 2));
            let ts = openh264::Timestamp::from_millis(time.as_millis() as u64);
            let bits = self.enc.encode_at(&yuv, ts)?.to_vec();
            // Rate control may skip a frame entirely.
            Ok(if bits.is_empty() { vec![] } else { vec![Encoded { annex_b: bits, time }] })
        }

        fn finish(&mut self) -> anyhow::Result<Vec<Encoded>> {
            Ok(vec![])
        }
    }
}

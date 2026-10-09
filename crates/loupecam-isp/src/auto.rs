//! Auto exposure and auto white balance controllers.
//!
//! These are host-side loops, exactly as the vendor SDK does it: they read frame
//! [`Stats`](crate::stats::Stats) and decide new exposure/gain or white-balance gains.

use crate::stats::Stats;

/// Auto exposure: drives mean luma towards a target, preferring exposure time over
/// gain (less noise), within limits.
#[derive(Debug, Clone, PartialEq)]
pub struct AutoExposure {
    /// Target mean luma in linear light, 0..1. 0.18 is photographic mid-grey.
    pub target: f32,
    /// Relative error tolerated before adjusting (hysteresis), e.g. 0.05 = ±5 %.
    pub tolerance: f32,
    /// Fraction of the correction applied per step (0..1]. Lower = smoother.
    pub damping: f32,
    pub min_exposure_us: u32,
    pub max_exposure_us: u32,
    pub max_gain: f32,
    /// Reduce the target when this fraction of the image is clipped.
    pub clip_limit: f32,
}

impl Default for AutoExposure {
    fn default() -> Self {
        AutoExposure {
            target: 0.18,
            tolerance: 0.06,
            damping: 0.6,
            min_exposure_us: 100,
            max_exposure_us: 200_000,
            max_gain: 8.0,
            clip_limit: 0.02,
        }
    }
}

/// Exposure settings chosen by [`AutoExposure`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Exposure {
    pub exposure_us: u32,
    pub gain: f32,
}

impl AutoExposure {
    /// Given the stats of a frame taken with `current`, return new settings if a change
    /// is warranted.
    pub fn update(&self, stats: &Stats, current: Exposure) -> Option<Exposure> {
        let measured = stats.luma.max(1e-5);
        let mut ratio = self.target / measured;
        if stats.clipped > self.clip_limit && ratio > 0.8 {
            ratio = 0.8;
        }
        if (ratio - 1.0).abs() <= self.tolerance {
            return None;
        }
        let ratio = ratio.powf(self.damping).clamp(0.125, 8.0);
        let total = current.exposure_us as f32 * current.gain * ratio;
        let max_t = self.max_exposure_us as f32;
        let (t, g) = if total <= max_t {
            (total.max(self.min_exposure_us as f32), 1.0)
        } else {
            (max_t, (total / max_t).min(self.max_gain))
        };
        let next = Exposure { exposure_us: t.round() as u32, gain: g.max(1.0) };
        (next != current).then_some(next)
    }
}

/// Grey-world white balance: gains that make the mean R and B match G. Computed over
/// a neutral (white/grey) region, this is "click to white balance".
pub fn grey_world(stats: &Stats) -> [f32; 3] {
    let [r, g, b] = stats.mean.map(|v| v.max(1e-6));
    [(g / r).clamp(0.25, 8.0), 1.0, (g / b).clamp(0.25, 8.0)]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stats(luma: f32) -> Stats {
        Stats { mean: [luma; 3], luma, histogram: vec![], clipped: 0.0, cells: 1 }
    }

    #[test]
    fn converges() {
        let ae = AutoExposure { damping: 1.0, ..Default::default() };
        let cur = Exposure { exposure_us: 10_000, gain: 1.0 };
        // Twice as bright as wanted: halve exposure.
        let n = ae.update(&stats(0.36), cur).unwrap();
        assert_eq!(n.exposure_us, 5000);
        // Within tolerance: no change.
        assert!(ae.update(&stats(0.185), cur).is_none());
        // Very dark: exposure saturates at max, then gain.
        let n = ae.update(&stats(0.001), Exposure { exposure_us: 200_000, gain: 1.0 }).unwrap();
        assert_eq!(n.exposure_us, 200_000);
        assert!(n.gain > 1.0);
    }
}

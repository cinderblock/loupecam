//! Frame statistics for auto exposure, white balance, and histograms.

use crate::{LUMA, scale_shift};
use loupecam_protocol::frame::{RawFrame, SampleFormat};
use loupecam_protocol::model::BayerPattern;

/// Statistics computed from the raw mosaic (before white balance), on 2×2 cells.
#[derive(Debug, Clone, PartialEq)]
pub struct Stats {
    /// Mean linear level per channel R, G, B in 0..1.
    pub mean: [f32; 3],
    /// Mean luma (Rec. 709 weights) of the white-balanced means, 0..1.
    pub luma: f32,
    /// 256-bin histogram of per-cell luma (linear).
    pub histogram: Vec<u32>,
    /// Fraction of cells with any channel at or near full scale.
    pub clipped: f32,
    pub cells: u32,
}

/// A rectangle in frame pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// Compute statistics over `region` (whole frame if `None`), sampling every `step`-th
/// 2×2 cell in each direction (1 = every cell). `wb` weights the luma estimate.
pub fn compute(frame: &RawFrame, pattern: BayerPattern, region: Option<Rect>, step: u32, wb: [f32; 3]) -> Stats {
    let r = region.unwrap_or(Rect { x: 0, y: 0, width: frame.width, height: frame.height });
    let (x0, y0) = (r.x & !1, r.y & !1);
    let (x1, y1) = ((r.x + r.width).min(frame.width) & !1, (r.y + r.height).min(frame.height) & !1);
    let step = step.max(1) as usize * 2;
    let shift = scale_shift(frame.format);
    let w = frame.width as usize;
    let sample = |x: usize, y: usize| -> u32 {
        let v = match frame.format {
            SampleFormat::U8 => frame.data[y * w + x] as u32,
            SampleFormat::U16 { .. } => {
                let i = (y * w + x) * 2;
                u16::from_le_bytes([frame.data[i], frame.data[i + 1]]) as u32
            }
        };
        v << shift
    };
    let (rx, ry) = match pattern {
        BayerPattern::Rggb => (0, 0),
        BayerPattern::Grbg => (1, 0),
        BayerPattern::Gbrg => (0, 1),
        BayerPattern::Bggr => (1, 1),
    };
    let mut sum = [0u64; 3];
    let mut hist = vec![0u32; 256];
    let (mut cells, mut clipped) = (0u32, 0u32);
    const CLIP: u32 = 65535 - 512;
    for y in (y0 as usize..y1 as usize).step_by(step) {
        for x in (x0 as usize..x1 as usize).step_by(step) {
            let red = sample(x + rx, y + ry);
            let blue = sample(x + 1 - rx, y + 1 - ry);
            let g1 = sample(x + 1 - rx, y + ry);
            let g2 = sample(x + rx, y + 1 - ry);
            let green = (g1 + g2) / 2;
            sum[0] += red as u64;
            sum[1] += green as u64;
            sum[2] += blue as u64;
            if red.max(blue).max(g1).max(g2) >= CLIP {
                clipped += 1;
            }
            let l = LUMA[0] * red as f32 * wb[0] + LUMA[1] * green as f32 * wb[1] + LUMA[2] * blue as f32 * wb[2];
            hist[((l / 65536.0 * 256.0) as usize).min(255)] += 1;
            cells += 1;
        }
    }
    let n = cells.max(1) as f32;
    let mean = sum.map(|s| s as f32 / n / 65535.0);
    let luma = LUMA[0] * mean[0] * wb[0] + LUMA[1] * mean[1] * wb[1] + LUMA[2] * mean[2] * wb[2];
    Stats { mean, luma, histogram: hist, clipped: clipped as f32 / n, cells }
}

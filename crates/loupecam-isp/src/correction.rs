//! Optional per-camera corrections from calibration: flat field (vignetting, dust,
//! colour shading) and defective-pixel replacement. Both are applied to the mosaic in
//! the same pass that normalises and white-balances it.

use loupecam_protocol::model::BayerPattern;
use std::sync::Arc;

/// Per-channel gain map over the frame, on a coarse grid, interpolated bilinearly.
///
/// Measured on an evenly lit, defocused field: `gain = reference / local level`, where
/// the reference is the bright end of the field, so it stays unchanged and darker
/// regions are lifted.
#[derive(Debug, Clone, PartialEq)]
pub struct FlatField {
    /// Frame size the map was measured at (it applies to that resolution).
    pub width: u32,
    pub height: u32,
    pub cols: u32,
    pub rows: u32,
    /// `rows × cols` gains, row-major, each `[R, G, B]`.
    pub gains: Vec<[f32; 3]>,
}

/// Largest correction a flat field may apply (darker corners than this are left
/// partly uncorrected rather than amplifying noise without bound).
pub const MAX_FLAT_GAIN: f32 = 4.0;

impl FlatField {
    /// Build a flat field from a mean mosaic (any linear scale, black already
    /// subtracted) of an evenly lit field.
    pub fn from_mosaic(mosaic: &[f32], width: u32, height: u32, pattern: BayerPattern, cols: u32) -> Self {
        let (w, h) = (width as usize, height as usize);
        let cols = cols.clamp(4, (width / 8).max(4));
        let rows = ((cols as f32 * height as f32 / width as f32).round() as u32).max(3);
        let (cw, ch) = (cols as usize, rows as usize);
        let mut sum = vec![[0f64; 3]; cw * ch];
        let mut n = vec![[0u32; 3]; cw * ch];
        let (rx, ry) = red_site(pattern);
        for y in 0..h {
            let by = (y * ch / h).min(ch - 1);
            for x in 0..w {
                let bx = (x * cw / w).min(cw - 1);
                let c = color(x, y, rx, ry);
                sum[by * cw + bx][c] += mosaic[y * w + x] as f64;
                n[by * cw + bx][c] += 1;
            }
        }
        // No smoothing: each block averages hundreds of samples per channel at real
        // resolutions, and dust shadows are part of what the flat field should correct.
        let level: Vec<[f32; 3]> = sum
            .iter()
            .zip(&n)
            .map(|(s, n)| std::array::from_fn(|c| if n[c] > 0 { (s[c] / n[c] as f64) as f32 } else { 0.0 }))
            .collect();
        // Reference: the 95th percentile of block levels, so the brightest part of the
        // field is left as it is and only darker regions are lifted (a percentile, not
        // the maximum, so one bright block can't set it).
        let reference: [f32; 3] = std::array::from_fn(|c| {
            let mut v: Vec<f32> = level.iter().map(|l| l[c]).collect();
            v.sort_by(f32::total_cmp);
            v[((v.len() - 1) as f32 * 0.95) as usize]
        });
        let gains = level
            .iter()
            .map(|l| std::array::from_fn(|c| if l[c] > 0.0 { (reference[c] / l[c]).clamp(0.5, MAX_FLAT_GAIN) } else { 1.0 }))
            .collect();
        FlatField { width, height, cols, rows, gains }
    }

    /// Gains for one frame row (`y` in full-frame pixels), interpolated between grid
    /// rows: one `[R,G,B]` per grid column.
    pub(crate) fn row(&self, y: f32, out: &mut Vec<[f32; 3]>) {
        let (cw, ch) = (self.cols as usize, self.rows as usize);
        let (r0, r1, t) = lerp_index((y + 0.5) / self.height as f32 * ch as f32 - 0.5, ch);
        out.clear();
        out.extend((0..cw).map(|x| {
            let (a, b) = (self.gains[r0 * cw + x], self.gains[r1 * cw + x]);
            std::array::from_fn(|c| a[c] + (b[c] - a[c]) * t)
        }));
    }

    /// Gain for channel `c` at full-frame column `x`, given a row from [`row`](Self::row).
    #[inline]
    pub(crate) fn at(&self, row: &[[f32; 3]], x: f32, c: usize) -> f32 {
        let cw = self.cols as usize;
        let (c0, c1, t) = lerp_index((x + 0.5) / self.width as f32 * cw as f32 - 0.5, cw);
        (row[c0][c] + (row[c1][c] - row[c0][c]) * t).clamp(0.5, MAX_FLAT_GAIN)
    }
}

/// Neighbouring grid indices and weight for position `u` (in grid units, block centres
/// at integers). Outside the outer block centres (the last half-block at each edge) the
/// edge segment is extrapolated linearly instead of held flat.
fn lerp_index(u: f32, n: usize) -> (usize, usize, f32) {
    if n < 2 {
        return (0, 0, 0.0);
    }
    let u = u.clamp(-0.5, n as f32 - 0.5);
    let i0 = (u.floor().max(0.0) as usize).min(n - 2);
    (i0, i0 + 1, u - i0 as f32)
}

/// Calibration corrections for one develop. Coordinates are full-frame pixels of the
/// frame's resolution; `origin` is where the frame starts within that (ROI offset).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Corrections {
    pub flat_field: Option<Arc<FlatField>>,
    /// Defective pixels (full-frame coordinates), replaced by the mean of their
    /// same-colour neighbours.
    pub defects: Option<Arc<Vec<(u32, u32)>>>,
    pub origin: (u32, u32),
}

impl Corrections {
    pub fn is_empty(&self) -> bool {
        self.flat_field.is_none() && self.defects.is_none()
    }
}

pub(crate) fn red_site(p: BayerPattern) -> (usize, usize) {
    match p {
        BayerPattern::Rggb => (0, 0),
        BayerPattern::Grbg => (1, 0),
        BayerPattern::Gbrg => (0, 1),
        BayerPattern::Bggr => (1, 1),
    }
}

/// 0 = R, 1 = G, 2 = B.
#[inline]
pub(crate) fn color(x: usize, y: usize, rx: usize, ry: usize) -> usize {
    match ((x & 1) == rx, (y & 1) == ry) {
        (true, true) => 0,
        (false, false) => 2,
        _ => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_field_inverts_vignetting() {
        let (w, h) = (256u32, 192u32);
        // Radial falloff to 50 % in the corners.
        let mut m = vec![0f32; (w * h) as usize];
        for y in 0..h {
            for x in 0..w {
                let (dx, dy) = (x as f32 / w as f32 - 0.5, y as f32 / h as f32 - 0.5);
                let r2 = (dx * dx + dy * dy) / 0.5;
                m[(y * w + x) as usize] = 1000.0 * (1.0 - 0.5 * r2);
            }
        }
        let ff = FlatField::from_mosaic(&m, w, h, BayerPattern::Rggb, 32);
        let mut row = Vec::new();
        ff.row(h as f32 / 2.0, &mut row);
        let centre = ff.at(&row, w as f32 / 2.0, 1);
        assert!((centre - 1.0).abs() < 0.02, "centre gain {centre}");
        ff.row(4.0, &mut row);
        let corner = ff.at(&row, 4.0, 1);
        assert!(corner > 1.6, "corner gain {corner}");
    }
}

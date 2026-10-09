//! Calibration maths on averaged raw mosaics. No camera I/O, so it is unit-tested with
//! synthetic frames.

use loupecam::BayerPattern;
use loupecam_isp::correction::FlatField;

/// The per-pixel mean of several raw frames.
#[derive(Debug, Clone)]
pub struct Measurement {
    pub width: u32,
    pub height: u32,
    pub pattern: BayerPattern,
    /// Mean sample value per pixel, in 12-bit sensor units (0..4095).
    pub mean: Vec<f32>,
    /// Exposure actually programmed, µs.
    pub exposure_us: f64,
    pub frames: u32,
}

/// Full scale of a 12-bit sample.
pub const FULL_SCALE: f32 = 4095.0;

fn red_site(p: BayerPattern) -> (u32, u32) {
    match p {
        BayerPattern::Rggb => (0, 0),
        BayerPattern::Grbg => (1, 0),
        BayerPattern::Gbrg => (0, 1),
        BayerPattern::Bggr => (1, 1),
    }
}

fn color(x: u32, y: u32, (rx, ry): (u32, u32)) -> usize {
    match ((x & 1) == rx, (y & 1) == ry) {
        (true, true) => 0,
        (false, false) => 2,
        _ => 1,
    }
}

fn median(v: &mut [f32]) -> f32 {
    if v.is_empty() {
        return 0.0;
    }
    let mid = v.len() / 2;
    *v.select_nth_unstable_by(mid, f32::total_cmp).1
}

impl Measurement {
    /// Samples of channel `c` (0 R, 1 G, 2 B) within the central `fraction` of the frame
    /// (1.0 = whole frame), subsampled to at most ~200k values.
    fn channel(&self, c: usize, fraction: f32) -> Vec<f32> {
        let (w, h) = (self.width, self.height);
        let (mx, my) = (((1.0 - fraction) / 2.0 * w as f32) as u32, ((1.0 - fraction) / 2.0 * h as f32) as u32);
        let n = ((w - 2 * mx) as u64 * (h - 2 * my) as u64 / 4).max(1);
        let step = (((n as f64) / 200_000.0).sqrt().ceil() as u32).max(1) * 2;
        let rs = red_site(self.pattern);
        let mut out = Vec::new();
        for y in (my..h - my).step_by(step as usize) {
            for x in (mx..w - mx).step_by(step as usize) {
                // Visit the whole 2×2 cell so every channel is represented.
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let (xx, yy) = (x + dx, y + dy);
                    if xx < w && yy < h && color(xx, yy, rs) == c {
                        out.push(self.mean[(yy * w + xx) as usize]);
                    }
                }
            }
        }
        out
    }

    /// Median per channel over the central `fraction` of the frame.
    pub fn medians(&self, fraction: f32) -> [f32; 3] {
        std::array::from_fn(|c| median(&mut self.channel(c, fraction)))
    }

    /// Fraction of (sampled) pixels at or near full scale.
    pub fn clipped(&self) -> f32 {
        let v: Vec<f32> = (0..3).flat_map(|c| self.channel(c, 1.0)).collect();
        v.iter().filter(|&&s| s >= FULL_SCALE - 16.0).count() as f32 / v.len().max(1) as f32
    }
}

/// Black level per channel from a dark, short exposure.
pub fn black_level(dark: &Measurement) -> [f32; 3] {
    dark.medians(1.0)
}

/// How far a dark frame's median may sit above black before we conclude light is
/// leaking in (12-bit units).
const DARK_LEAK: f32 = 64.0;

/// Find hot/stuck pixels in a dark, long exposure: values far above their channel's
/// robust spread. Errors if the frame isn't actually dark.
pub fn find_defects(dark_long: &Measurement, black: [f32; 3]) -> Result<Vec<(u32, u32)>, String> {
    let med = dark_long.medians(1.0);
    for c in 0..3 {
        if med[c] - black[c] > DARK_LEAK {
            return Err(format!(
                "the frame is not dark (median {:.0} vs black {:.0}). Cover the lens or turn the light off and try again.",
                med[c], black[c]
            ));
        }
    }
    let spread: [f32; 3] = std::array::from_fn(|c| {
        let mut dev: Vec<f32> = dark_long.channel(c, 1.0).iter().map(|v| (v - med[c]).abs()).collect();
        1.4826 * median(&mut dev)
    });
    let rs = red_site(dark_long.pattern);
    let (w, h) = (dark_long.width, dark_long.height);
    let mut out = Vec::new();
    for y in 0..h {
        for x in 0..w {
            let c = color(x, y, rs);
            let v = dark_long.mean[(y * w + x) as usize];
            if v > med[c] + (12.0 * spread[c]).max(48.0) {
                out.push((x, y));
            }
        }
    }
    let limit = (w as usize * h as usize) / 500; // 0.2 %
    if out.len() > limit {
        return Err(format!("found {} defective pixels (more than 0.2 %): the frame is probably not dark enough", out.len()));
    }
    Ok(out)
}

/// Signal level above black: median of green over the central half of the frame.
pub fn level(m: &Measurement, black: [f32; 3]) -> f32 {
    m.medians(0.5)[1] - black[1]
}

/// Flat field from an evenly lit, defocused measurement.
pub fn flat_field(m: &Measurement, black: [f32; 3], cols: u32) -> Result<FlatField, String> {
    let lvl = level(m, black);
    if lvl < 0.1 * FULL_SCALE {
        return Err(format!("the field is too dark ({:.0}% of full scale); add light or exposure", lvl / FULL_SCALE * 100.0));
    }
    if m.clipped() > 0.001 {
        return Err("parts of the field are clipped; reduce the light or exposure".into());
    }
    let rs = red_site(m.pattern);
    let w = m.width;
    let mosaic: Vec<f32> = m
        .mean
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let (x, y) = (i as u32 % w, i as u32 / w);
            (v - black[color(x, y, rs)]).max(0.0)
        })
        .collect();
    let ff = FlatField::from_mosaic(&mosaic, m.width, m.height, m.pattern, cols);
    // An even field needs only gentle correction; strong structure means a textured
    // target (or the focus was not off), which would get baked into every image.
    let worst = ff.gains.iter().flatten().fold(1f32, |a, &g| a.max(g));
    if worst > 3.5 {
        return Err(format!("the field is very uneven (corner correction {worst:.1}×). Defocus fully on a plain, evenly lit surface."));
    }
    Ok(ff)
}

/// A detected repetition period.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Period {
    /// In full-resolution frame pixels.
    pub pixels: f64,
    /// Normalised autocorrelation at the period, 0..1 (higher = clearer).
    pub strength: f32,
}

/// Dominant repetition period of a line pattern (ruler graduations, a micrometer, or the
/// on-screen grid) along x and y, from the autocorrelation of averaged luminance
/// profiles. `None` where there is no clear periodicity.
pub fn line_periods(m: &Measurement) -> (Option<Period>, Option<Period>) {
    let (w, h) = (m.width as usize, m.height as usize);
    // Sum 2×2 cells into a half-resolution luminance image (colour-pattern free).
    let (hw, hh) = (w / 2, h / 2);
    let lum = |x: usize, y: usize| {
        let i = |xx: usize, yy: usize| m.mean[yy * w + xx];
        i(2 * x, 2 * y) + i(2 * x + 1, 2 * y) + i(2 * x, 2 * y + 1) + i(2 * x + 1, 2 * y + 1)
    };
    let mut cols = vec![0f64; hw];
    let mut rows = vec![0f64; hh];
    for (y, row) in rows.iter_mut().enumerate() {
        for (x, col) in cols.iter_mut().enumerate() {
            let v = lum(x, y) as f64;
            *col += v;
            *row += v;
        }
    }
    // Profiles are in half-resolution pixels; report full-resolution frame pixels.
    let full = |(p, strength): (f64, f32)| Period { pixels: p * 2.0, strength };
    (dominant_period(&cols).map(full), dominant_period(&rows).map(full))
}

/// Period of a 1-D profile: the first strong autocorrelation peak (sub-sample refined).
fn dominant_period(profile: &[f64]) -> Option<(f64, f32)> {
    let n = profile.len();
    if n < 32 {
        return None;
    }
    // Remove the mean and a linear trend (uneven lighting).
    let xm = (n - 1) as f64 / 2.0;
    let mean = profile.iter().sum::<f64>() / n as f64;
    let slope = profile.iter().enumerate().map(|(i, v)| (i as f64 - xm) * (v - mean)).sum::<f64>()
        / profile.iter().enumerate().map(|(i, _)| (i as f64 - xm).powi(2)).sum::<f64>();
    let d: Vec<f64> = profile.iter().enumerate().map(|(i, v)| v - mean - slope * (i as f64 - xm)).collect();
    let max_lag = n / 3;
    let ac: Vec<f64> = (0..=max_lag).map(|lag| (0..n - lag).map(|i| d[i] * d[i + lag]).sum::<f64>() / (n - lag) as f64).collect();
    if ac[0] <= 0.0 {
        return None;
    }
    // Skip the central lobe, then take the first local maximum above 30 % of zero-lag.
    let mut lag = 1;
    while lag < max_lag && ac[lag] > 0.0 {
        lag += 1;
    }
    let mut best: Option<usize> = None;
    for l in lag.max(2)..max_lag {
        if ac[l] > ac[l - 1] && ac[l] >= ac[l + 1] && ac[l] / ac[0] > 0.3 {
            best = Some(l);
            break;
        }
    }
    let l = best?;
    // Parabolic refinement.
    let (a, b, c) = (ac[l - 1], ac[l], ac[l + 1]);
    let denom = a - 2.0 * b + c;
    let off = if denom.abs() > 1e-12 { 0.5 * (a - c) / denom } else { 0.0 };
    Some((l as f64 + off, (b / ac[0]) as f32))
}

/// Least-squares 3×3 matrix `M` with `M · cam[i] ≈ reference[i]`, normalised so that
/// each row sums to 1 (white stays white).
pub fn fit_ccm(cam: &[[f32; 3]], reference: &[[f32; 3]]) -> Option<[[f32; 3]; 3]> {
    if cam.len() != reference.len() || cam.len() < 3 {
        return None;
    }
    // Normal equations: (AᵀA) Mᵀ = AᵀB, with A = cam (n×3), B = reference (n×3).
    let mut ata = [[0f64; 3]; 3];
    let mut atb = [[0f64; 3]; 3];
    for (a, b) in cam.iter().zip(reference) {
        for i in 0..3 {
            for j in 0..3 {
                ata[i][j] += a[i] as f64 * a[j] as f64;
                atb[i][j] += a[i] as f64 * b[j] as f64;
            }
        }
    }
    let inv = invert3(ata)?;
    let mut m = [[0f32; 3]; 3];
    for (r, row) in m.iter_mut().enumerate() {
        for (c, cell) in row.iter_mut().enumerate() {
            // M[r][c] = Σ_k inv[c][k] · atb[k][r]
            *cell = (0..3).map(|k| inv[c][k] * atb[k][r]).sum::<f64>() as f32;
        }
        let s: f32 = row.iter().sum();
        if s.abs() > 1e-6 {
            row.iter_mut().for_each(|v| *v /= s);
        }
    }
    Some(m)
}

fn invert3(m: [[f64; 3]; 3]) -> Option<[[f64; 3]; 3]> {
    let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    if det.abs() < 1e-12 {
        return None;
    }
    let c = |r: usize, k: usize| {
        let (r1, r2) = ((r + 1) % 3, (r + 2) % 3);
        let (k1, k2) = ((k + 1) % 3, (k + 2) % 3);
        m[r1][k1] * m[r2][k2] - m[r1][k2] * m[r2][k1]
    };
    Some(std::array::from_fn(|i| std::array::from_fn(|j| c(j, i) / det)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meas(w: u32, h: u32, f: impl Fn(u32, u32) -> f32) -> Measurement {
        Measurement {
            width: w,
            height: h,
            pattern: BayerPattern::Rggb,
            mean: (0..h).flat_map(|y| (0..w).map(move |x| (x, y))).map(|(x, y)| f(x, y)).collect(),
            exposure_us: 1000.0,
            frames: 1,
        }
    }

    /// Deterministic pseudo-noise in ±1.
    fn noise(x: u32, y: u32) -> f32 {
        let v = (x.wrapping_mul(73856093) ^ y.wrapping_mul(19349663)) % 1000;
        v as f32 / 500.0 - 1.0
    }

    #[test]
    fn dark_frame_black_and_defects() {
        let dark = meas(200, 150, |x, y| 168.0 + 3.0 * noise(x, y) + if (x, y) == (40, 50) || (x, y) == (101, 7) { 900.0 } else { 0.0 });
        let black = black_level(&dark);
        assert!(black.iter().all(|b| (b - 168.0).abs() < 2.0), "{black:?}");
        let d = find_defects(&dark, black).unwrap();
        assert_eq!(d, vec![(101, 7), (40, 50)]);
        // A lit frame is rejected.
        let lit = meas(200, 150, |x, y| 900.0 + 3.0 * noise(x, y));
        assert!(find_defects(&lit, black).is_err());
    }

    #[test]
    fn periods_of_a_ruler() {
        // Vertical lines every 37.5 px (a ruler imaged across x), plus lighting slope.
        let m = meas(640, 480, |x, _| 1000.0 + x as f32 + 400.0 * (2.0 * std::f32::consts::PI * x as f32 / 37.5).cos().max(0.0).powi(4));
        let (px, py) = line_periods(&m);
        let p = px.expect("x period");
        assert!((p.pixels - 37.5).abs() < 0.3, "period {p:?}");
        assert!(p.strength > 0.3);
        assert!(py.is_none(), "{py:?}");
    }

    #[test]
    fn ccm_recovers_a_matrix() {
        let truth = [[1.6f32, -0.4, -0.2], [-0.3, 1.5, -0.2], [-0.1, -0.5, 1.6]];
        let cam: Vec<[f32; 3]> = (0..24).map(|i| [((i * 7) % 11) as f32 / 10.0 + 0.05, ((i * 5) % 13) as f32 / 12.0 + 0.05, ((i * 3) % 7) as f32 / 6.0 + 0.05]).collect();
        let reference: Vec<[f32; 3]> = cam.iter().map(|c| std::array::from_fn(|r| (0..3).map(|k| truth[r][k] * c[k]).sum())).collect();
        let m = fit_ccm(&cam, &reference).unwrap();
        for r in 0..3 {
            for c in 0..3 {
                assert!((m[r][c] - truth[r][c]).abs() < 1e-3, "{m:?}");
            }
        }
    }
}

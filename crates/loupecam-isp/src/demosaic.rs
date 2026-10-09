//! Bayer demosaicing over a reflect-padded, white-balanced mosaic.

use crate::{DevelopParams, scale_shift};
use loupecam_protocol::frame::{RawFrame, SampleFormat};
use loupecam_protocol::model::BayerPattern;
use rayon::prelude::*;

/// Demosaic algorithm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Demosaic {
    /// Each 2×2 cell becomes one RGB pixel (half resolution). Fastest, no
    /// interpolation artefacts; good for previews.
    Superpixel,
    /// Bilinear interpolation. Fast, slightly soft, some colour fringing.
    #[default]
    Bilinear,
    /// Malvar–He–Cutler gradient-corrected linear interpolation (2004). Sharper with
    /// far less fringing, at roughly 2× the cost of bilinear.
    Mhc,
}

impl Demosaic {
    pub fn output_size(self, w: u32, h: u32) -> (u32, u32) {
        match self {
            Demosaic::Superpixel => (w / 2, h / 2),
            _ => (w, h),
        }
    }

    /// Fill `out` (interleaved RGB, 16-bit scale) with output row `y`.
    pub(crate) fn row(self, m: &Padded, y: u32, out: &mut [u16]) {
        match self {
            Demosaic::Superpixel => superpixel_row(m, y as isize, out),
            Demosaic::Bilinear => interp_row(m, y as isize, out, bilinear),
            Demosaic::Mhc => interp_row(m, y as isize, out, mhc),
        }
    }
}

const PAD: usize = 2;

/// Mosaic normalised to 16-bit scale, black-subtracted and white-balanced, with a
/// 2-pixel mirror border so 5×5 kernels need no bounds checks. Mirroring about the
/// edge pixel preserves Bayer parity.
pub(crate) struct Padded {
    pub w: usize,
    pub h: usize,
    stride: usize,
    data: Vec<u16>,
    /// Position of the red site within the 2×2 cell.
    rx: usize,
    ry: usize,
}

impl Padded {
    pub fn prepare(frame: &RawFrame, p: &DevelopParams) -> Self {
        let (w, h) = (frame.width as usize, frame.height as usize);
        let stride = w + 2 * PAD;
        let (rx, ry) = red_site(p.pattern);
        let shift = scale_shift(frame.format);
        let black = p.black_level as u32;
        // Gains in 8.8 fixed point, rescaled so white stays white after black removal.
        let range = 65535u32.saturating_sub(black).max(1);
        let gain = p.wb.map(|g| ((g.max(0.0) * 65535.0 / range as f32) * 256.0).round() as u32);
        let mut data = vec![0u16; stride * (h + 2 * PAD)];
        data.par_chunks_mut(stride).skip(PAD).take(h).enumerate().for_each(|(y, row)| {
            let gy = (y & 1) == ry;
            let (g_even, g_odd) = match ((rx == 0), gy) {
                (true, true) => (gain[0], gain[1]),
                (true, false) => (gain[1], gain[2]),
                (false, true) => (gain[1], gain[0]),
                (false, false) => (gain[2], gain[1]),
            };
            let dst = &mut row[PAD..PAD + w];
            if let Some(ff) = &p.corrections.flat_field {
                // Slower path: per-pixel flat-field gain on top of white balance.
                let (ox, oy) = p.corrections.origin;
                let mut grow = Vec::new();
                ff.row((y as u32 + oy) as f32, &mut grow);
                let ch = |x: usize| crate::correction::color(x, y, rx, ry);
                let gf = |x: usize| [gain[0], gain[1], gain[2]][ch(x)] as f32 * ff.at(&grow, (x as u32 + ox) as f32, ch(x));
                let normf = |v: u32, x: usize| ((((v << shift).saturating_sub(black)) as f32 * gf(x)) as u32 >> 8).min(65535) as u16;
                match frame.format {
                    SampleFormat::U8 => {
                        for (x, (d, &s)) in dst.iter_mut().zip(&frame.data[y * w..(y + 1) * w]).enumerate() {
                            *d = normf(s as u32, x);
                        }
                    }
                    SampleFormat::U16 { .. } => {
                        let src = &frame.data[y * w * 2..(y + 1) * w * 2];
                        for (x, (d, s)) in dst.iter_mut().zip(src.as_chunks::<2>().0).enumerate() {
                            *d = normf(u16::from_le_bytes(*s) as u32, x);
                        }
                    }
                }
                return;
            }
            let norm = |v: u32, x: usize| {
                let g = if x & 1 == 0 { g_even } else { g_odd };
                (((v << shift).saturating_sub(black) * g) >> 8).min(65535) as u16
            };
            match frame.format {
                SampleFormat::U8 => {
                    let src = &frame.data[y * w..(y + 1) * w];
                    for (x, (d, &s)) in dst.iter_mut().zip(src).enumerate() {
                        *d = norm(s as u32, x);
                    }
                }
                SampleFormat::U16 { .. } => {
                    let src = &frame.data[y * w * 2..(y + 1) * w * 2];
                    for (x, (d, s)) in dst.iter_mut().zip(src.as_chunks::<2>().0).enumerate() {
                        *d = norm(u16::from_le_bytes(*s) as u32, x);
                    }
                }
            }
        });
        let mut m = Padded { w, h, stride, data, rx, ry };
        if let Some(defects) = &p.corrections.defects {
            m.fix_defects(defects, p.corrections.origin);
        }
        m.reflect_borders();
        m
    }

    /// Replace each defective pixel with the mean of its same-colour neighbours (two
    /// pixels away horizontally and vertically) that lie inside the frame.
    fn fix_defects(&mut self, defects: &[(u32, u32)], (ox, oy): (u32, u32)) {
        let (w, h, s) = (self.w as i64, self.h as i64, self.stride);
        let at = |x: i64, y: i64| (y as usize + PAD) * s + x as usize + PAD;
        for &(dx, dy) in defects {
            let (x, y) = (dx as i64 - ox as i64, dy as i64 - oy as i64);
            if x < 0 || y < 0 || x >= w || y >= h {
                continue;
            }
            let (mut sum, mut n) = (0u32, 0u32);
            for (nx, ny) in [(x - 2, y), (x + 2, y), (x, y - 2), (x, y + 2)] {
                if nx >= 0 && ny >= 0 && nx < w && ny < h {
                    sum += self.data[at(nx, ny)] as u32;
                    n += 1;
                }
            }
            if let Some(mean) = sum.checked_div(n) {
                self.data[at(x, y)] = mean as u16;
            }
        }
    }

    fn reflect_borders(&mut self) {
        let (w, h, s) = (self.w, self.h, self.stride);
        for y in PAD..PAD + h {
            let r = &mut self.data[y * s..(y + 1) * s];
            for k in 1..=PAD {
                r[PAD - k] = r[PAD + k.min(w - 1)];
                r[PAD + w - 1 + k] = r[PAD + w - 1 - k.min(w - 1)];
            }
        }
        for k in 1..=PAD {
            let src = (PAD + k.min(h - 1)) * s;
            self.data.copy_within(src..src + s, (PAD - k) * s);
            let src = (PAD + h - 1 - k.min(h - 1)) * s;
            self.data.copy_within(src..src + s, (PAD + h - 1 + k) * s);
        }
    }

    #[inline(always)]
    fn at(&self, x: isize, y: isize) -> i32 {
        self.data[(y + PAD as isize) as usize * self.stride + (x + PAD as isize) as usize] as i32
    }

    /// Colour at (x, y): 0 = R, 1 = G, 2 = B.
    #[inline(always)]
    fn color(&self, x: isize, y: isize) -> u8 {
        let (cx, cy) = (((x & 1) as usize) == self.rx, ((y & 1) as usize) == self.ry);
        match (cx, cy) {
            (true, true) => 0,
            (false, false) => 2,
            _ => 1,
        }
    }
}

fn red_site(p: BayerPattern) -> (usize, usize) {
    match p {
        BayerPattern::Rggb => (0, 0),
        BayerPattern::Grbg => (1, 0),
        BayerPattern::Gbrg => (0, 1),
        BayerPattern::Bggr => (1, 1),
    }
}

fn superpixel_row(m: &Padded, y: isize, out: &mut [u16]) {
    let (y0, ow) = (2 * y, m.w / 2);
    for (ox, px) in out.as_chunks_mut::<3>().0.iter_mut().take(ow).enumerate() {
        let x0 = 2 * ox as isize;
        let mut acc = [0i32; 3];
        for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
            acc[m.color(x0 + dx, y0 + dy) as usize] += m.at(x0 + dx, y0 + dy);
        }
        px[0] = acc[0] as u16;
        px[1] = (acc[1] / 2) as u16;
        px[2] = acc[2] as u16;
    }
}

fn interp_row(m: &Padded, y: isize, out: &mut [u16], f: fn(&Padded, isize, isize) -> [i32; 3]) {
    for (x, px) in out.as_chunks_mut::<3>().0.iter_mut().take(m.w).enumerate() {
        let [r, g, b] = f(m, x as isize, y);
        px[0] = r.clamp(0, 65535) as u16;
        px[1] = g.clamp(0, 65535) as u16;
        px[2] = b.clamp(0, 65535) as u16;
    }
}

#[inline(always)]
fn bilinear(m: &Padded, x: isize, y: isize) -> [i32; 3] {
    let c = m.at(x, y);
    let cross = || (m.at(x - 1, y) + m.at(x + 1, y) + m.at(x, y - 1) + m.at(x, y + 1) + 2) / 4;
    let diag = || (m.at(x - 1, y - 1) + m.at(x + 1, y - 1) + m.at(x - 1, y + 1) + m.at(x + 1, y + 1) + 2) / 4;
    let horiz = || (m.at(x - 1, y) + m.at(x + 1, y) + 1) / 2;
    let vert = || (m.at(x, y - 1) + m.at(x, y + 1) + 1) / 2;
    match m.color(x, y) {
        0 => [c, cross(), diag()],
        2 => [diag(), cross(), c],
        _ => {
            // Green site: is red to the left/right (red row) or above/below?
            if m.color(x + 1, y) == 0 { [horiz(), c, vert()] } else { [vert(), c, horiz()] }
        }
    }
}

/// Malvar, He, Cutler: "High-quality linear interpolation for demosaicing of
/// Bayer-patterned color images", ICASSP 2004. Kernels scaled by 16 (halves kept exact).
#[inline(always)]
fn mhc(m: &Padded, x: isize, y: isize) -> [i32; 3] {
    let p = |dx: isize, dy: isize| m.at(x + dx, y + dy);
    let c = p(0, 0);
    let axial2 = p(-2, 0) + p(2, 0) + p(0, -2) + p(0, 2);
    let cross1 = p(-1, 0) + p(1, 0) + p(0, -1) + p(0, 1);
    let diag1 = p(-1, -1) + p(1, -1) + p(-1, 1) + p(1, 1);
    // G at R/B sites: [4c + 2·cross1 − axial2] / 8
    let g_at_rb = || (8 * c + 4 * cross1 - 2 * axial2 + 8) / 16;
    // Opposite chroma at R/B sites: [6c + 2·diag1 − 1.5·axial2] / 8
    let rb_at_br = || (12 * c + 4 * diag1 - 3 * axial2 + 8) / 16;
    // Chroma at G sites, neighbours horizontal: [5c + 4·(l+r) − (diag1) − (h2) + 0.5·(v2)] / 8
    let h2 = p(-2, 0) + p(2, 0);
    let v2 = p(0, -2) + p(0, 2);
    let at_g_horiz = || (10 * c + 8 * (p(-1, 0) + p(1, 0)) - 2 * diag1 - 2 * h2 + v2 + 8) / 16;
    let at_g_vert = || (10 * c + 8 * (p(0, -1) + p(0, 1)) - 2 * diag1 - 2 * v2 + h2 + 8) / 16;
    match m.color(x, y) {
        0 => [c, g_at_rb(), rb_at_br()],
        2 => [rb_at_br(), g_at_rb(), c],
        _ => {
            if m.color(x + 1, y) == 0 {
                [at_g_horiz(), c, at_g_vert()]
            } else {
                [at_g_vert(), c, at_g_horiz()]
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DevelopParams, IDENTITY, ToneCurve};
    use loupecam_protocol::frame::Trailer;

    /// Mosaic a flat colour and check every algorithm reconstructs it exactly.
    #[test]
    fn flat_colour_is_exact() {
        let (w, h) = (16u32, 12u32);
        let rgb = [3000u16, 2000, 1000];
        for pattern in [BayerPattern::Rggb, BayerPattern::Grbg, BayerPattern::Gbrg, BayerPattern::Bggr] {
            let (rx, ry) = red_site(pattern);
            let mut data = Vec::new();
            for y in 0..h as usize {
                for x in 0..w as usize {
                    let c = match ((x & 1) == rx, (y & 1) == ry) {
                        (true, true) => 0,
                        (false, false) => 2,
                        _ => 1,
                    };
                    data.extend_from_slice(&rgb[c].to_le_bytes());
                }
            }
            let frame = RawFrame { width: w, height: h, format: SampleFormat::U16 { bits: 12 }, data, trailer: Trailer::default() };
            for alg in [Demosaic::Superpixel, Demosaic::Bilinear, Demosaic::Mhc] {
                let mut p = DevelopParams::new(pattern);
                p.demosaic = alg;
                p.ccm = IDENTITY;
                p.tone = ToneCurve::linear();
                let img = crate::develop16(&frame, &p);
                let want = rgb.map(|v| v << 4);
                for px in img.data.as_chunks::<3>().0 {
                    for k in 0..3 {
                        assert!((px[k] as i32 - want[k] as i32).abs() <= 16, "{pattern:?} {alg:?} {px:?} != {want:?}");
                    }
                }
            }
        }
    }

    fn flat_frame(w: u32, h: u32, f: impl Fn(u32, u32) -> u16) -> RawFrame {
        let mut data = Vec::new();
        for y in 0..h {
            for x in 0..w {
                data.extend_from_slice(&f(x, y).to_le_bytes());
            }
        }
        RawFrame { width: w, height: h, format: SampleFormat::U16 { bits: 12 }, data, trailer: Trailer::default() }
    }

    fn linear(alg: Demosaic) -> DevelopParams {
        let mut p = DevelopParams::new(BayerPattern::Rggb);
        p.demosaic = alg;
        p.tone = ToneCurve::linear();
        p
    }

    #[test]
    fn defect_is_replaced() {
        let frame = flat_frame(32, 32, |x, y| if (x, y) == (10, 12) { 4095 } else { 1000 });
        let mut p = linear(Demosaic::Bilinear);
        let hot = crate::develop16(&frame, &p);
        p.corrections.defects = Some(std::sync::Arc::new(vec![(10, 12)]));
        let fixed = crate::develop16(&frame, &p);
        let px = |img: &crate::Image16| img.data[(12 * 32 + 10) * 3];
        assert!(px(&hot) > 30000);
        assert!((px(&fixed) as i32 - (1000 << 4)).abs() < 64, "{}", px(&fixed));
    }

    #[test]
    fn flat_field_evens_out_a_vignetted_frame() {
        let (w, h) = (128u32, 96u32);
        let level = |x: u32, y: u32| {
            let (dx, dy) = (x as f32 / w as f32 - 0.5, y as f32 / h as f32 - 0.5);
            (2000.0 * (1.0 - 0.8 * (dx * dx + dy * dy))) as u16
        };
        let frame = flat_frame(w, h, level);
        let mosaic: Vec<f32> = (0..h).flat_map(|y| (0..w).map(move |x| level(x, y) as f32)).collect();
        let ff = crate::correction::FlatField::from_mosaic(&mosaic, w, h, BayerPattern::Rggb, 16);
        let mut p = linear(Demosaic::Bilinear);
        p.corrections.flat_field = Some(std::sync::Arc::new(ff));
        let img = crate::develop16(&frame, &p);
        let g = |x: usize, y: usize| img.data[(y * w as usize + x) * 3 + 1] as f32;
        let (centre, corner) = (g(64, 48), g(6, 6));
        assert!((corner / centre - 1.0).abs() < 0.04, "corner {corner} vs centre {centre}");
    }
}

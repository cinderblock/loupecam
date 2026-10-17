//! RGB to YUV 4:2:0 for video encoders.

use crate::Image8;
use rayon::prelude::*;

/// Planar YUV 4:2:0 (I420): the Y plane, then U, then V, each tightly packed. BT.709
/// coefficients, limited ("TV") range, the usual encoding for HD video.
#[derive(Clone, PartialEq)]
pub struct I420 {
    /// Even.
    pub width: u32,
    /// Even.
    pub height: u32,
    pub data: Vec<u8>,
}

impl std::fmt::Debug for I420 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "I420({}x{})", self.width, self.height)
    }
}

impl I420 {
    pub fn y(&self) -> &[u8] {
        &self.data[..(self.width * self.height) as usize]
    }

    pub fn u(&self) -> &[u8] {
        let (y, c) = self.plane_sizes();
        &self.data[y..y + c]
    }

    pub fn v(&self) -> &[u8] {
        let (y, c) = self.plane_sizes();
        &self.data[y + c..]
    }

    fn plane_sizes(&self) -> (usize, usize) {
        let y = (self.width * self.height) as usize;
        (y, y / 4)
    }
}

// BT.709 limited range from full-range R'G'B', in 1/256ths.
const KY: [i32; 3] = [47, 157, 16];
const KU: [i32; 3] = [-26, -87, 113];
const KV: [i32; 3] = [112, -102, -10];

/// Convert an RGB image. Odd widths and heights lose their last column or row.
pub fn rgb_to_i420(img: &Image8) -> I420 {
    assert_eq!(img.channels, 3, "rgb_to_i420 needs an RGB image");
    let (w, h) = ((img.width & !1) as usize, (img.height & !1) as usize);
    let sw = img.width as usize * 3;
    let mut data = vec![0u8; w * h * 3 / 2];
    let (yp, uv) = data.split_at_mut(w * h);
    let (up, vp) = uv.split_at_mut(w * h / 4);
    let dot = |k: &[i32; 3], p: &[u8]| (k[0] * p[0] as i32 + k[1] * p[1] as i32 + k[2] * p[2] as i32 + 128) >> 8;
    // Two luma rows and one chroma row per job.
    yp.par_chunks_mut(2 * w)
        .zip(up.par_chunks_mut(w / 2).zip(vp.par_chunks_mut(w / 2)))
        .enumerate()
        .for_each(|(cy, (ys, (us, vs)))| {
            let r0 = &img.data[2 * cy * sw..];
            let r1 = &img.data[(2 * cy + 1) * sw..];
            for x in 0..w {
                ys[x] = (16 + dot(&KY, &r0[x * 3..])) as u8;
                ys[w + x] = (16 + dot(&KY, &r1[x * 3..])) as u8;
            }
            for cx in 0..w / 2 {
                let mut avg = [0u8; 3];
                for (k, a) in avg.iter_mut().enumerate() {
                    let o = cx * 6 + k;
                    *a = ((r0[o] as u32 + r0[o + 3] as u32 + r1[o] as u32 + r1[o + 3] as u32 + 2) / 4) as u8;
                }
                us[cx] = (128 + dot(&KU, &avg)).clamp(16, 240) as u8;
                vs[cx] = (128 + dot(&KV, &avg)).clamp(16, 240) as u8;
            }
        });
    I420 { width: w as u32, height: h as u32, data }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: u32, h: u32, p: [u8; 3]) -> I420 {
        let mut img = Image8::new(w, h, 3);
        img.data.chunks_mut(3).for_each(|c| c.copy_from_slice(&p));
        rgb_to_i420(&img)
    }

    #[test]
    fn reference_colours() {
        // BT.709 limited range: black 16/128/128, white 235/128/128, and the primaries.
        let px = |p| {
            let f = solid(4, 2, p);
            (f.y()[0], f.u()[0], f.v()[0])
        };
        assert_eq!(px([0, 0, 0]), (16, 128, 128));
        assert_eq!(px([255, 255, 255]), (235, 128, 128));
        let (y, u, v) = px([255, 0, 0]);
        assert!((62..=64).contains(&y) && (101..=103).contains(&u) && v == 240, "red {y} {u} {v}");
        let (y, u, v) = px([0, 0, 255]);
        assert!((31..=33).contains(&y) && u == 240 && (117..=119).contains(&v), "blue {y} {u} {v}");
    }

    #[test]
    fn odd_sizes_and_layout() {
        let f = solid(7, 5, [10, 20, 30]);
        assert_eq!((f.width, f.height, f.data.len()), (6, 4, 6 * 4 * 3 / 2));
        assert_eq!((f.y().len(), f.u().len(), f.v().len()), (24, 6, 6));
    }
}

//! Resizing to an exact size.

use crate::Image8;
use rayon::prelude::*;

/// Source pixels contributing to one output pixel along an axis, with weights in
/// 1/65536ths that sum to 65536.
struct Taps {
    start: usize,
    weights: Vec<u32>,
}

/// Area-average taps mapping `src` samples onto `dst` (shrinking), or nearest-neighbour
/// when enlarging.
fn taps(src: usize, dst: usize) -> Vec<Taps> {
    let scale = src as f64 / dst as f64;
    (0..dst)
        .map(|i| {
            if scale <= 1.0 {
                let s = (((i as f64 + 0.5) * scale) as usize).min(src - 1);
                return Taps { start: s, weights: vec![65536] };
            }
            let (a, b) = (i as f64 * scale, ((i + 1) as f64 * scale).min(src as f64));
            let start = a.floor() as usize;
            let end = (b.ceil() as usize).min(src);
            let mut w: Vec<f64> = (start..end).map(|s| (b.min(s as f64 + 1.0) - a.max(s as f64)).max(0.0)).collect();
            let total: f64 = w.iter().sum();
            w.iter_mut().for_each(|v| *v /= total);
            let mut weights: Vec<u32> = w.iter().map(|v| (v * 65536.0).round() as u32).collect();
            // Make the weights sum exactly to 1.0 so flat areas stay exact.
            let sum: u32 = weights.iter().sum();
            let big = weights.iter().enumerate().max_by_key(|&(_, v)| *v).map_or(0, |(k, _)| k);
            weights[big] = (weights[big] + 65536).saturating_sub(sum);
            Taps { start, weights }
        })
        .collect()
}

/// Resize to exactly `width × height` by area averaging. This is meant for shrinking
/// (video frames from sensor frames): every source pixel counts, so there is no
/// aliasing. Enlarging repeats pixels.
pub fn resize(src: &Image8, width: u32, height: u32) -> Image8 {
    let (width, height) = (width.max(1), height.max(1));
    if (src.width, src.height) == (width, height) {
        return src.clone();
    }
    let c = src.channels as usize;
    let (sw, sh, dw, dh) = (src.width as usize, src.height as usize, width as usize, height as usize);
    let xt = taps(sw, dw);
    let yt = taps(sh, dh);
    // Horizontal pass into 16.16 fixed point, then vertical.
    let mut tmp = vec![0u32; sh * dw * c];
    tmp.par_chunks_mut(dw * c).enumerate().for_each(|(y, row)| {
        let s = &src.data[y * sw * c..(y + 1) * sw * c];
        for (x, t) in xt.iter().enumerate() {
            for k in 0..c {
                let mut acc = 0u32;
                for (j, w) in t.weights.iter().enumerate() {
                    acc += s[(t.start + j) * c + k] as u32 * w;
                }
                row[x * c + k] = acc >> 8;
            }
        }
    });
    let mut dst = Image8::new(width, height, src.channels);
    dst.data.par_chunks_mut(dw * c).enumerate().for_each(|(y, row)| {
        let t = &yt[y];
        for (i, d) in row.iter_mut().enumerate() {
            let mut acc = 0u64;
            for (j, w) in t.weights.iter().enumerate() {
                acc += tmp[(t.start + j) * dw * c + i] as u64 * *w as u64;
            }
            *d = ((acc + (1 << 23)) >> 24).min(255) as u8;
        }
    });
    dst
}

#[cfg(test)]
mod tests {
    use super::*;

    fn img(w: u32, h: u32, f: impl Fn(u32, u32) -> [u8; 3]) -> Image8 {
        let mut i = Image8::new(w, h, 3);
        for y in 0..h {
            for x in 0..w {
                let o = ((y * w + x) * 3) as usize;
                i.data[o..o + 3].copy_from_slice(&f(x, y));
            }
        }
        i
    }

    #[test]
    fn flat_stays_flat() {
        let r = resize(&img(2456, 1842, |_, _| [10, 128, 255]), 1440, 1080);
        assert_eq!((r.width, r.height), (1440, 1080));
        assert!(r.data.chunks(3).all(|p| p == [10, 128, 255]));
    }

    #[test]
    fn averages_areas() {
        // Columns alternate 0/255: halving averages each pair.
        let r = resize(&img(8, 2, |x, _| [if x % 2 == 0 { 0 } else { 255 }; 3]), 4, 1);
        assert!(r.data.iter().all(|&v| (127..=128).contains(&v)), "{:?}", r.data);
        // 3 → 2: the middle pixel is shared half and half.
        let r = resize(&img(3, 1, |x, _| [[0, 90, 180][x as usize]; 3]), 2, 1);
        assert_eq!(r.data.chunks(3).map(|p| p[0]).collect::<Vec<_>>(), [30, 150]);
    }

    #[test]
    fn same_size_and_enlarge() {
        let a = img(5, 3, |x, y| [x as u8, y as u8, 7]);
        assert_eq!(resize(&a, 5, 3), a);
        let b = resize(&a, 10, 6);
        assert_eq!(&b.data[..3], &[0, 0, 7]);
        assert_eq!(&b.data[(b.data.len() - 3)..], &[4, 2, 7]);
    }
}

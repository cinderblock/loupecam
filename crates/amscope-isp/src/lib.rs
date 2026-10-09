//! Host-side image pipeline for AmScope / ToupTek cameras.
//!
//! The camera delivers a Bayer mosaic (raw, or already white-balanced and tone-mapped
//! by its FPGA). This crate turns that into viewable and saveable images:
//!
//! 1. normalise samples to 16-bit scale, subtract black level, apply white balance
//!    ([`DevelopParams::black_level`], [`DevelopParams::wb`])
//! 2. demosaic ([`Demosaic`])
//! 3. colour matrix, tone curve, monochrome/negative ([`DevelopParams`])
//! 4. orientation ([`Orientation`])
//!
//! Steps 1–3 are fused per output row and parallelised with rayon, so a full 18 MP
//! frame needs no full-size intermediate RGB buffer.
//!
//! [`stats`] computes channel statistics for auto exposure / white balance, and
//! [`encode`] writes PNG, TIFF and JPEG.

pub mod auto;
mod demosaic;
pub mod encode;
pub mod stats;
pub mod tone;

pub use demosaic::Demosaic;
pub use tone::ToneCurve;

use amscope_protocol::frame::{RawFrame, SampleFormat};
use amscope_protocol::model::BayerPattern;
use rayon::prelude::*;

/// Interleaved RGB image (or single-channel when `channels == 1`).
#[derive(Clone, PartialEq)]
pub struct Image<T> {
    pub width: u32,
    pub height: u32,
    pub channels: u8,
    pub data: Vec<T>,
}

pub type Image8 = Image<u8>;
pub type Image16 = Image<u16>;

impl<T> std::fmt::Debug for Image<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Image({}x{}x{})", self.width, self.height, self.channels)
    }
}

impl<T: Copy + Default + Send + Sync> Image<T> {
    pub fn new(width: u32, height: u32, channels: u8) -> Self {
        Image { width, height, channels, data: vec![T::default(); width as usize * height as usize * channels as usize] }
    }

    fn row_len(&self) -> usize {
        self.width as usize * self.channels as usize
    }
}

/// Rotation applied after flips.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Rotation {
    #[default]
    None,
    Cw90,
    Cw180,
    Cw270,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Orientation {
    pub flip_h: bool,
    pub flip_v: bool,
    pub rotation: Rotation,
}

/// Parameters for [`develop`].
#[derive(Debug, Clone, PartialEq)]
pub struct DevelopParams {
    pub pattern: BayerPattern,
    pub demosaic: Demosaic,
    /// Black level in 16-bit-scale units, subtracted before white balance.
    pub black_level: u16,
    /// White-balance gains for R, G, B (1.0 = unity).
    pub wb: [f32; 3],
    /// Colour matrix, row-major, applied to linear camera RGB.
    pub ccm: [[f32; 3]; 3],
    pub tone: ToneCurve,
    /// Output a single luminance channel.
    pub monochrome: bool,
    pub negative: bool,
    pub orientation: Orientation,
}

impl DevelopParams {
    pub fn new(pattern: BayerPattern) -> Self {
        DevelopParams {
            pattern,
            demosaic: Demosaic::Bilinear,
            black_level: 0,
            wb: [1.0; 3],
            ccm: IDENTITY,
            tone: ToneCurve::srgb(),
            monochrome: false,
            negative: false,
            orientation: Orientation::default(),
        }
    }
}

pub const IDENTITY: [[f32; 3]; 3] = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

/// Rec. 709 luma weights, used for monochrome output and statistics.
pub const LUMA: [f32; 3] = [0.2126, 0.7152, 0.0722];

/// Develop a raw frame into an 8-bit image.
pub fn develop(frame: &RawFrame, p: &DevelopParams) -> Image8 {
    let lut = p.tone.lut8(p.negative);
    develop_with(frame, p, |v| lut[(v >> 4) as usize])
}

/// Develop a raw frame into a 16-bit image (tone curve applied at 16-bit precision).
pub fn develop16(frame: &RawFrame, p: &DevelopParams) -> Image16 {
    let lut = p.tone.lut16(p.negative);
    develop_with(frame, p, |v| lut[(v >> 4) as usize])
}

fn develop_with<T, F>(frame: &RawFrame, p: &DevelopParams, out: F) -> Image<T>
where
    T: Copy + Default + Send + Sync,
    F: Fn(u16) -> T + Sync,
{
    let mosaic = demosaic::Padded::prepare(frame, p);
    let (w, h) = p.demosaic.output_size(frame.width, frame.height);
    let channels = if p.monochrome { 1 } else { 3 };
    let mut img = Image::<T>::new(w, h, channels);
    let ccm = fixed_ccm(&p.ccm, p.monochrome);
    let row_len = img.row_len();
    img.data.par_chunks_mut(row_len).enumerate().for_each_init(
        || vec![0u16; w as usize * 3],
        |rgb, (y, row)| {
            p.demosaic.row(&mosaic, y as u32, rgb);
            for (x, px) in rgb.as_chunks::<3>().0.iter().enumerate() {
                let (r, g, b) = (px[0] as i32, px[1] as i32, px[2] as i32);
                let mix = |m: &[i32; 3]| ((m[0] * r + m[1] * g + m[2] * b) >> 12).clamp(0, 65535) as u16;
                if p.monochrome {
                    row[x] = out(mix(&ccm[0]));
                } else {
                    row[x * 3] = out(mix(&ccm[0]));
                    row[x * 3 + 1] = out(mix(&ccm[1]));
                    row[x * 3 + 2] = out(mix(&ccm[2]));
                }
            }
        },
    );
    orient(img, p.orientation)
}

/// CCM in 4.12 fixed point. For monochrome, row 0 becomes luma of the corrected colour.
fn fixed_ccm(m: &[[f32; 3]; 3], mono: bool) -> [[i32; 3]; 3] {
    let m = if mono {
        let mut l = [0.0f32; 3];
        for (j, lj) in l.iter_mut().enumerate() {
            *lj = (0..3).map(|i| LUMA[i] * m[i][j]).sum();
        }
        [l, l, l]
    } else {
        *m
    };
    m.map(|row| row.map(|v| (v * 4096.0).round() as i32))
}

/// Apply flips and rotation.
pub fn orient<T: Copy + Default + Send + Sync>(img: Image<T>, o: Orientation) -> Image<T> {
    let img = if o.flip_h || o.flip_v { flip(img, o.flip_h, o.flip_v) } else { img };
    match o.rotation {
        Rotation::None => img,
        Rotation::Cw180 => flip(img, true, true),
        Rotation::Cw90 => transpose_flip(&img, true),
        Rotation::Cw270 => transpose_flip(&img, false),
    }
}

fn flip<T: Copy + Default + Send + Sync>(mut img: Image<T>, h: bool, v: bool) -> Image<T> {
    let c = img.channels as usize;
    let rl = img.row_len();
    if h {
        img.data.par_chunks_mut(rl).for_each(|row| {
            let n = row.len() / c;
            for x in 0..n / 2 {
                for k in 0..c {
                    row.swap(x * c + k, (n - 1 - x) * c + k);
                }
            }
        });
    }
    if v {
        let hgt = img.height as usize;
        for y in 0..hgt / 2 {
            let (a, b) = img.data.split_at_mut((hgt - 1 - y) * rl);
            a[y * rl..(y + 1) * rl].swap_with_slice(&mut b[..rl]);
        }
    }
    img
}

/// Rotate 90° clockwise (`cw`) or counter-clockwise.
fn transpose_flip<T: Copy + Default + Send + Sync>(src: &Image<T>, cw: bool) -> Image<T> {
    let (w, h, c) = (src.width as usize, src.height as usize, src.channels as usize);
    let mut dst = Image::<T>::new(h as u32, w as u32, src.channels);
    dst.data.par_chunks_mut(h * c).enumerate().for_each(|(y, row)| {
        for x in 0..h {
            let (sx, sy) = if cw { (y, h - 1 - x) } else { (w - 1 - y, x) };
            let s = (sy * w + sx) * c;
            row[x * c..x * c + c].copy_from_slice(&src.data[s..s + c]);
        }
    });
    dst
}

/// Normalisation shift taking a sample to 16-bit scale.
pub(crate) fn scale_shift(f: SampleFormat) -> u32 {
    match f {
        SampleFormat::U8 => 8,
        SampleFormat::U16 { bits } => 16 - bits as u32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotations_compose() {
        let mut img = Image::<u8>::new(3, 2, 1);
        img.data.copy_from_slice(&[1, 2, 3, 4, 5, 6]);
        let cw = orient(img.clone(), Orientation { rotation: Rotation::Cw90, ..Default::default() });
        assert_eq!((cw.width, cw.height), (2, 3));
        assert_eq!(cw.data, [4, 1, 5, 2, 6, 3]);
        let ccw = orient(img.clone(), Orientation { rotation: Rotation::Cw270, ..Default::default() });
        assert_eq!(ccw.data, [3, 6, 2, 5, 1, 4]);
        let r180 = orient(img.clone(), Orientation { rotation: Rotation::Cw180, ..Default::default() });
        assert_eq!(r180.data, [6, 5, 4, 3, 2, 1]);
        let fh = orient(img, Orientation { flip_h: true, ..Default::default() });
        assert_eq!(fh.data, [3, 2, 1, 6, 5, 4]);
    }
}

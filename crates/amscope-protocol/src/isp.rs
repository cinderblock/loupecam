//! Parameters for the camera FPGA's hardware image pipeline.
//!
//! The FPGA applies, in mosaic (Bayer) domain and still delivering 8 or 16 bits per
//! pixel: white-balance gains, a 3×3 colour matrix, and a 4096-entry tone LUT. It also
//! accumulates per-channel sums over two statistics windows, returned in each frame's
//! trailer.

/// FPGA register numbers (sent as `register << 8` in `wIndex`).
pub mod fpga {
    /// 0 = 8-bit transport, 1 = 16-bit (12-bit samples, little-endian).
    pub const TRANSPORT_16BIT: u8 = 0x02;
    /// Test pattern: 0 off, 3 mono diagonal, 5 mono vertical, 7 mono horizontal,
    /// 9 colour diagonal.
    pub const TEST_PATTERN: u8 = 0x1c;
    /// Size index (sensor subsampling level).
    pub const SIZE_INDEX: u8 = 0x20;
    /// Colour matrix, row-major, 9 registers 0x60, 0x62 … 0x70.
    pub const CCM_BASE: u8 = 0x60;
    /// Output width / 4.
    pub const WIDTH_DIV4: u8 = 0xa2;
    pub const HEIGHT: u8 = 0xa4;
    pub const UNKNOWN_A6: u8 = 0xa6;
    pub const UNKNOWN_A8: u8 = 0xa8;
    /// White-balance gains R, G, B (8.8 fixed point).
    pub const WB_R: u8 = 0xd4;
    pub const WB_G: u8 = 0xd6;
    pub const WB_B: u8 = 0xd8;
    /// 1 = hardware ISP on (RGB modes), 0 = raw passthrough.
    pub const ISP_ENABLE: u8 = 0xf2;
    pub const UNKNOWN_F4: u8 = 0xf4;
    pub const UNKNOWN_F6: u8 = 0xf6;
    pub const UNKNOWN_F8: u8 = 0xf8;
    pub const UNKNOWN_FA: u8 = 0xfa;
    pub const UNKNOWN_FC: u8 = 0xfc;
}

/// White-balance channel gains (1.0 = unity).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WbGains {
    pub r: f32,
    pub g: f32,
    pub b: f32,
}

impl WbGains {
    pub const UNITY: WbGains = WbGains { r: 1.0, g: 1.0, b: 1.0 };

    /// 8.8 fixed-point register values.
    pub fn registers(&self) -> [u16; 3] {
        [self.r, self.g, self.b].map(|v| (v * 256.0).round().clamp(0.0, u16::MAX as f32) as u16)
    }
}

/// 3×3 colour-correction matrix, row-major, applied as `out = M · [r g b]ᵀ`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColorMatrix(pub [[f32; 3]; 3]);

/// Fixed-point scale of the FPGA colour matrix: 1.0 is 1023. The vendor matrices' rows
/// each sum to exactly 1023, which preserves white.
pub const CCM_ONE: f32 = 1023.0;

impl ColorMatrix {
    pub const IDENTITY: ColorMatrix =
        ColorMatrix([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);

    /// The vendor's default matrix for the MU1803-HS (hue 0, saturation 128).
    pub fn mu1803_default() -> Self {
        Self::from_registers([1219, -242, 46, -461, 1533, -49, -164, -527, 1714])
    }

    pub fn from_registers(r: [i16; 9]) -> Self {
        let f = |i: usize| r[i] as f32 / CCM_ONE;
        ColorMatrix([[f(0), f(1), f(2)], [f(3), f(4), f(5)], [f(6), f(7), f(8)]])
    }

    /// Signed 16-bit register values, row-major.
    pub fn registers(&self) -> [i16; 9] {
        let mut out = [0i16; 9];
        for (i, v) in self.0.iter().flatten().enumerate() {
            out[i] = (v * CCM_ONE).round().clamp(i16::MIN as f32, i16::MAX as f32) as i16;
        }
        out
    }

    pub fn mul(&self, o: &ColorMatrix) -> ColorMatrix {
        let mut m = [[0.0; 3]; 3];
        for (i, row) in m.iter_mut().enumerate() {
            for (j, cell) in row.iter_mut().enumerate() {
                *cell = (0..3).map(|k| self.0[i][k] * o.0[k][j]).sum();
            }
        }
        ColorMatrix(m)
    }

    /// Saturation adjustment about Rec. 601 luma (s = 1.0 leaves colour unchanged).
    pub fn saturation(s: f32) -> ColorMatrix {
        let (wr, wg, wb) = (0.299, 0.587, 0.114);
        let row = |w: [f32; 3], d: usize| {
            let mut r = [0.0; 3];
            for (k, cell) in r.iter_mut().enumerate() {
                *cell = (1.0 - s) * w[k] + if k == d { s } else { 0.0 };
            }
            r
        };
        let w = [wr, wg, wb];
        ColorMatrix([row(w, 0), row(w, 1), row(w, 2)])
    }

    /// Hue rotation by `degrees` about the grey axis.
    pub fn hue(degrees: f32) -> ColorMatrix {
        let (s, c) = degrees.to_radians().sin_cos();
        let k = (1.0 - c) / 3.0;
        let t = 3f32.sqrt().recip() * s;
        ColorMatrix([
            [c + k, k - t, k + t],
            [k + t, c + k, k - t],
            [k - t, k + t, c + k],
        ])
    }
}

/// The FPGA's 12-bit-in, 12-bit-out tone LUT.
#[derive(Clone, PartialEq)]
pub struct ToneLut(pub Box<[u16; 4096]>);

impl std::fmt::Debug for ToneLut {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ToneLut([{}, {}, … {}])", self.0[0], self.0[1], self.0[4095])
    }
}

impl ToneLut {
    pub fn identity() -> Self {
        let mut t = Box::new([0u16; 4096]);
        for (i, v) in t.iter_mut().enumerate() {
            *v = i as u16;
        }
        ToneLut(t)
    }

    /// Pure power-law gamma: `out = 4095 · (in / 4095)^(1/gamma)`.
    pub fn gamma(gamma: f32) -> Self {
        let mut t = Box::new([0u16; 4096]);
        for (i, v) in t.iter_mut().enumerate() {
            *v = (4095.0 * (i as f32 / 4095.0).powf(1.0 / gamma)).round() as u16;
        }
        ToneLut(t)
    }

    /// The four 2048-byte upload chunks as (`wIndex`, payload).
    pub fn chunks(&self) -> [(u16, Vec<u8>); 4] {
        std::array::from_fn(|c| {
            let data = self.0[c * 1024..(c + 1) * 1024]
                .iter()
                .flat_map(|v| v.to_le_bytes())
                .collect();
            (0x2200 + 0x200 * c as u16, data)
        })
    }
}

/// A rectangle over which the FPGA accumulates per-channel sums.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatsWindow {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

impl StatsWindow {
    /// The vendor's default window: x = ⌊W/10⌋, y = ⌊H/15⌋ forced odd, width = ⌈0.3·W⌉,
    /// height = ⌊H/5⌋. These rules reproduce all three captured sizes exactly.
    pub fn default_for(width: u16, height: u16) -> Self {
        StatsWindow {
            x: width / 10,
            y: (height / 15) | 1,
            width: (width as u32 * 3).div_ceil(10) as u16,
            height: height / 5,
        }
    }

    /// Payload for [`crate::request::STATS_WINDOW`]. `bank` 0 uses FPGA registers
    /// 0x72–0x78, bank 1 uses 0x32–0x38.
    pub fn payload(&self, bank: u8) -> [u8; 16] {
        let base = if bank == 0 { 0x72 } else { 0x32 };
        let mut p = [0u8; 16];
        for (i, v) in [self.x, self.width, self.y, self.height].into_iter().enumerate() {
            p[i * 4] = 0;
            p[i * 4 + 1] = base + 2 * i as u8;
            p[i * 4 + 2..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_ccm_roundtrips() {
        assert_eq!(
            ColorMatrix::mu1803_default().registers(),
            [1219, -242, 46, -461, 1533, -49, -164, -527, 1714]
        );
        assert_eq!(ColorMatrix::IDENTITY.registers(), [1023, 0, 0, 0, 1023, 0, 0, 0, 1023]);
    }

    #[test]
    fn stats_window_matches_captures() {
        // Captured 0xda payloads for 1228x922 and 4912x3684.
        let hex = |s: &str| (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect::<Vec<_>>();
        assert_eq!(StatsWindow::default_for(1228, 922).payload(0).to_vec(), hex("00727a000074710100763d000078b800"));
        assert_eq!(StatsWindow::default_for(1228, 922).payload(1).to_vec(), hex("00327a000034710100363d000038b800"));
        assert_eq!(StatsWindow::default_for(4912, 3684).payload(0).to_vec(), hex("0072eb010074c2050076f5000078e002"));
    }

    #[test]
    fn lut_chunks() {
        let c = ToneLut::identity().chunks();
        assert_eq!(c[1].0, 0x2400);
        assert_eq!(&c[1].1[..4], &[0x00, 0x04, 0x01, 0x04]);
    }
}

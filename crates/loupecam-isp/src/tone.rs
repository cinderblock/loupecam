//! Tone curves: levels, transfer function, brightness and contrast, baked into a LUT.

/// Transfer function from linear light to output code values.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Transfer {
    Linear,
    /// IEC 61966-2-1 sRGB.
    Srgb,
    /// Pure power law, `out = in^(1/gamma)`.
    Gamma(f32),
}

/// A tone curve. Applied to linear values in `0.0..=1.0`, in this order:
/// levels (black/white point), transfer function, contrast (S-curve about mid-grey),
/// brightness (offset).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ToneCurve {
    pub transfer: Transfer,
    /// Input level mapped to black (0.0..1.0).
    pub black_point: f32,
    /// Input level mapped to white (0.0..1.0).
    pub white_point: f32,
    /// −1.0..=1.0; 0 = unchanged.
    pub brightness: f32,
    /// −1.0..=1.0; 0 = unchanged.
    pub contrast: f32,
}

/// LUT resolution: inputs are 16-bit-scale values shifted right by 4.
pub const LUT_SIZE: usize = 4096;

impl ToneCurve {
    pub fn srgb() -> Self {
        Self::with(Transfer::Srgb)
    }

    pub fn linear() -> Self {
        Self::with(Transfer::Linear)
    }

    pub fn gamma(g: f32) -> Self {
        Self::with(Transfer::Gamma(g))
    }

    fn with(transfer: Transfer) -> Self {
        ToneCurve { transfer, black_point: 0.0, white_point: 1.0, brightness: 0.0, contrast: 0.0 }
    }

    /// Evaluate the curve at linear input `x` (0..1), returning output 0..1.
    pub fn eval(&self, x: f32) -> f32 {
        let span = (self.white_point - self.black_point).max(1e-6);
        let v = ((x - self.black_point) / span).clamp(0.0, 1.0);
        let v = match self.transfer {
            Transfer::Linear => v,
            Transfer::Srgb => {
                if v <= 0.003_130_8 { 12.92 * v } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 }
            }
            Transfer::Gamma(g) => v.powf(1.0 / g.max(0.01)),
        };
        let v = if self.contrast != 0.0 {
            // Smooth S-curve: blend towards smoothstep (positive) or its inverse.
            let c = self.contrast.clamp(-1.0, 1.0);
            let s = v * v * (3.0 - 2.0 * v);
            let inv = 0.5 - ((1.0 - 2.0 * v).clamp(-1.0, 1.0).asin() / 3.0).sin();
            if c > 0.0 { v + c * (s - v) } else { v + (-c) * (inv - v) }
        } else {
            v
        };
        (v + self.brightness).clamp(0.0, 1.0)
    }

    fn table<T>(&self, negative: bool, max: f32, conv: impl Fn(f32) -> T) -> Vec<T> {
        (0..LUT_SIZE)
            .map(|i| {
                let v = self.eval(i as f32 / (LUT_SIZE - 1) as f32);
                conv((if negative { 1.0 - v } else { v }) * max + 0.5)
            })
            .collect()
    }

    pub fn lut8(&self, negative: bool) -> Vec<u8> {
        self.table(negative, 255.0, |v| v as u8)
    }

    pub fn lut16(&self, negative: bool) -> Vec<u16> {
        self.table(negative, 65535.0, |v| v as u16)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoints_and_monotonic() {
        for c in [ToneCurve::srgb(), ToneCurve::linear(), ToneCurve::gamma(1.8), ToneCurve { contrast: 0.7, ..ToneCurve::srgb() }, ToneCurve { contrast: -0.7, ..ToneCurve::srgb() }] {
            let l = c.lut8(false);
            assert_eq!(l[0], 0);
            assert_eq!(l[LUT_SIZE - 1], 255);
            assert!(l.windows(2).all(|w| w[0] <= w[1]), "{c:?} not monotonic");
        }
        assert!((ToneCurve::srgb().eval(0.18) - 0.461).abs() < 0.01);
    }
}

//! onsemi AR1820HS register programming as used by the MU1803-HS.
//!
//! Register addresses follow the SMIA/MIPI CCS layout this sensor implements. Values
//! without a public meaning (analog tuning) are reproduced as the camera's own
//! firmware-side configuration expects them.

use crate::{Error, Result};

pub const CHIP_ID: u16 = 0x1820;

pub mod reg {
    pub const MODE_SELECT: u16 = 0x0100;
    pub const SOFTWARE_RESET: u16 = 0x0103;
    pub const GROUPED_PARAMETER_HOLD: u16 = 0x0104;
    pub const DATA_FORMAT: u16 = 0x0112;
    pub const VT_PIX_CLK_DIV: u16 = 0x0300;
    pub const VT_SYS_CLK_DIV: u16 = 0x0302;
    pub const PRE_PLL_CLK_DIV: u16 = 0x0304;
    pub const PLL_MULTIPLIER: u16 = 0x0306;
    pub const OP_PIX_CLK_DIV: u16 = 0x0308;
    pub const OP_SYS_CLK_DIV: u16 = 0x030a;
    pub const FRAME_LENGTH_LINES: u16 = 0x0340;
    pub const LINE_LENGTH_PCK: u16 = 0x0342;
    pub const X_ADDR_START: u16 = 0x0344;
    pub const Y_ADDR_START: u16 = 0x0346;
    pub const X_ADDR_END: u16 = 0x0348;
    pub const Y_ADDR_END: u16 = 0x034a;
    pub const X_OUTPUT_SIZE: u16 = 0x034c;
    pub const Y_OUTPUT_SIZE: u16 = 0x034e;
    pub const COARSE_INTEGRATION_TIME: u16 = 0x3012;
    pub const RESET_REGISTER: u16 = 0x301a;
    pub const DATA_PEDESTAL: u16 = 0x301e;
    pub const READ_MODE: u16 = 0x3040;
    pub const GLOBAL_GAIN: u16 = 0x305e;
    pub const UNKNOWN_30B4: u16 = 0x30b4;
}

/// PLL programming (identical for every mode).
pub const PLL: &[(u16, u16)] = &[
    (reg::PRE_PLL_CLK_DIV, 6),
    (reg::PLL_MULTIPLIER, 100),
    (reg::VT_SYS_CLK_DIV, 1),
    (reg::VT_PIX_CLK_DIV, 6),
    (reg::OP_SYS_CLK_DIV, 1),
    (reg::OP_PIX_CLK_DIV, 12),
];

/// Analog/readout tuning written after the PLL, in this order.
pub const TUNING: &[(u16, u16)] = &[
    (0x31be, 0x000b),
    (0x31ae, 0x0304),
    (0x31c6, 0x8002),
    (0x31c0, 0x06db),
    (reg::DATA_FORMAT, 0x0c0c), // 12-bit pixels in, 12-bit out
    (0x3f3c, 0x0001),
    (0x3ed2, 0x449d),
    (0x31e0, 0x0741),
    (0x31e6, 0x1000),
    (0x3ede, 0x40e0),
    (reg::UNKNOWN_30B4, 0x0001),
    (reg::UNKNOWN_30B4, 0x0011),
];

/// Effective pixel rate used for line timing: `line_time = line_length_pck / PIXEL_RATE`.
///
/// Measured: the vendor SDK converts exposure using 534 MHz. The PLL (24 MHz × 100 / 6 /
/// 6, × 8 pixels per clock) gives 533.3 MHz, within 0.13 %. We keep the SDK figure so
/// exposures match the vendor software line for line.
pub const PIXEL_RATE_HZ: f64 = 534.0e6;

/// `read_mode` bit enabling summing/averaging (bin) instead of skipping.
pub const READ_MODE_BIN: u16 = 0x0800;

/// Sensor programming for one of the model's output sizes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SizeMode {
    /// Final image size delivered by the FPGA.
    pub width: u16,
    pub height: u16,
    /// Sensor output window (a few pixels larger than the image; the FPGA crops).
    pub sensor_out_width: u16,
    pub sensor_out_height: u16,
    pub x_addr_end: u16,
    pub y_addr_end: u16,
    /// `read_mode` in skip mode. OR in [`READ_MODE_BIN`] for binning.
    pub read_mode: u16,
    /// Subsampling factor (1, 2 or 4).
    pub factor: u16,
    pub frame_length_lines: u16,
    /// line_length_pck for speed 0 (slowest) .. 3 (fastest).
    pub line_length: [u16; 4],
    /// Fastest line length the 16-bit transport sustains, if lower than `line_length[3]`
    /// would need too much bandwidth.
    pub line_length_min_16bit: u16,
    /// FPGA register 0x20.
    pub fpga_size_index: u16,
}

pub const SIZES: [SizeMode; 3] = [
    SizeMode {
        width: 4912,
        height: 3684,
        sensor_out_width: 4916,
        sensor_out_height: 3692,
        x_addr_end: 4915,
        y_addr_end: 3691,
        read_mode: 0x4041,
        factor: 1,
        frame_length_lines: 3784,
        line_length: [38400, 21600, 14400, 10800],
        line_length_min_16bit: 14000,
        fpga_size_index: 0,
    },
    SizeMode {
        width: 2456,
        height: 1842,
        sensor_out_width: 2460,
        sensor_out_height: 1850,
        x_addr_end: 4917,
        y_addr_end: 3697,
        read_mode: 0x60c3,
        factor: 2,
        frame_length_lines: 1942,
        line_length: [32000, 16000, 10400, 8000],
        line_length_min_16bit: 8000,
        fpga_size_index: 1,
    },
    SizeMode {
        width: 1228,
        height: 922,
        sensor_out_width: 1232,
        sensor_out_height: 930,
        x_addr_end: 4921,
        y_addr_end: 3713,
        read_mode: 0x61c7,
        factor: 4,
        frame_length_lines: 1022,
        line_length: [25600, 16800, 12000, 9600],
        line_length_min_16bit: 9600,
        fpga_size_index: 2,
    },
];

impl SizeMode {
    pub fn line_length(&self, speed: u8, sixteen_bit: bool) -> u16 {
        let ll = self.line_length[speed.min(3) as usize];
        if sixteen_bit { ll.max(self.line_length_min_16bit) } else { ll }
    }
}

/// Exposure in microseconds to `coarse_integration_time` lines (min 1).
pub fn exposure_lines(exposure_us: u32, line_length_pck: u16) -> u16 {
    let lines = (exposure_us as f64 * PIXEL_RATE_HZ / 1e6 / line_length_pck as f64).round();
    lines.clamp(1.0, u16::MAX as f64) as u16
}

/// Actual exposure in microseconds for a line count.
pub fn lines_to_us(lines: u16, line_length_pck: u16) -> f64 {
    lines as f64 * line_length_pck as f64 / PIXEL_RATE_HZ * 1e6
}

/// Time to read out one line, in microseconds.
pub fn line_time_us(line_length_pck: u16) -> f64 {
    line_length_pck as f64 / PIXEL_RATE_HZ * 1e6
}

/// Analog gain stages seen in use, as (`global_gain` bits 6:0, approximate multiplier).
///
/// Ratios come from where the vendor SDK switches stages (digital gain falls from 127/64
/// to 64/64 at each step). They have not yet been measured optically.
pub const ANALOG_STAGES: &[(u8, f32)] = &[(0x09, 1.0), (0x0a, 2.0), (0x0e, 3.0), (0x7a, 4.0)];

/// Maximum digital gain field value we allow (bits 15:7 of `global_gain`, unit 1/64).
pub const DIGITAL_MAX: u16 = 255;

/// A decoded `global_gain` setting.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Gain {
    pub analog_code: u8,
    /// Digital gain in 1/64 units.
    pub digital: u16,
}

impl Gain {
    pub const UNITY: Gain = Gain { analog_code: 0x09, digital: 64 };

    pub fn register(self) -> u16 {
        (self.digital << 7) | (self.analog_code as u16 & 0x7f)
    }

    pub fn from_register(v: u16) -> Self {
        Gain { analog_code: (v & 0x7f) as u8, digital: v >> 7 }
    }

    /// Approximate total multiplier relative to [`Gain::UNITY`].
    pub fn multiplier(self) -> f32 {
        let a = ANALOG_STAGES
            .iter()
            .find(|(c, _)| *c == self.analog_code)
            .map(|(_, m)| *m)
            .unwrap_or(1.0);
        a * self.digital as f32 / 64.0
    }

    /// Choose a setting for a total multiplier: the highest analog stage that does not
    /// exceed it (best SNR), with digital gain making up the rest.
    pub fn for_multiplier(m: f32) -> Result<Self> {
        let max = Self::max_multiplier();
        if !(1.0..=max).contains(&m) {
            return Err(Error::OutOfRange(format!("gain {m} not in 1.0..={max}")));
        }
        let (code, a) = ANALOG_STAGES
            .iter()
            .rev()
            .find(|(_, a)| *a <= m)
            .copied()
            .unwrap_or(ANALOG_STAGES[0]);
        let digital = ((m / a) * 64.0).round().clamp(64.0, DIGITAL_MAX as f32) as u16;
        Ok(Gain { analog_code: code, digital })
    }

    pub fn max_multiplier() -> f32 {
        ANALOG_STAGES.last().unwrap().1 * DIGITAL_MAX as f32 / 64.0
    }
}

/// Sensor crop for a region of interest given in output-image pixels of `size`.
///
/// The ROI is aligned down to the vendor SDK's granularity (x and width to 4, y and
/// height to 2 output pixels).
pub fn roi_window(size: &SizeMode, x: u16, y: u16, w: u16, h: u16) -> Result<RoiWindow> {
    if w < 16 || h < 16 || x as u32 + w as u32 > size.width as u32 || y as u32 + h as u32 > size.height as u32 {
        return Err(Error::OutOfRange(format!(
            "ROI {w}x{h}+{x}+{y} does not fit {}x{}",
            size.width, size.height
        )));
    }
    let (x, w) = (x & !3, w & !3);
    let (y, h) = (y & !1, h & !1);
    let f = size.factor;
    let pad_w = size.sensor_out_width - size.width;
    let pad_h = size.sensor_out_height - size.height;
    // The array is read mirrored on both axes, so the window is addressed back from the
    // full-frame end coordinates. A skipped window of n output pixels spans
    // n·f − (2f − 1) array positions, matching the full-frame x/y_addr_end values.
    let span = |n: u16| n * f - (2 * f - 1);
    let x_end = size.x_addr_end - x * f;
    let y_end = size.y_addr_end - y * f;
    let x_start = x_end - span(w + pad_w);
    let y_start = y_end - span(h + pad_h);
    Ok(RoiWindow {
        x_start,
        y_start,
        x_end,
        y_end,
        out_width: w + pad_w,
        out_height: h + pad_h,
        width: w,
        height: h,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoiWindow {
    pub x_start: u16,
    pub y_start: u16,
    pub x_end: u16,
    pub y_end: u16,
    pub out_width: u16,
    pub out_height: u16,
    /// Image size the FPGA will deliver.
    pub width: u16,
    pub height: u16,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposure_matches_sdk() {
        // (µs, line_length, lines written by the SDK)
        for (us, ll, lines) in [
            (100, 9600, 6),
            (1000, 9600, 56),
            (5000, 9600, 278),
            (20000, 9600, 1113),
            (33333, 9600, 1854),
            (100000, 9600, 5563),
            (500000, 9600, 27813),
            (20000, 25600, 417),
            (20000, 10800, 989),
            (20000, 38400, 278),
        ] {
            assert_eq!(exposure_lines(us, ll), lines, "{us} µs @ {ll}");
        }
    }

    #[test]
    fn gain_register_roundtrip() {
        for v in [0x1609u16, 0x2c89, 0x258a, 0x217a, 0x208e] {
            assert_eq!(Gain::from_register(v).register(), v);
        }
        assert_eq!(Gain::UNITY.register(), 0x2009);
        let g = Gain::for_multiplier(2.5).unwrap();
        assert_eq!((g.analog_code, g.digital), (0x0a, 80));
    }

    #[test]
    fn full_roi_matches_size_programming() {
        for s in &SIZES {
            let r = roi_window(s, 0, 0, s.width, s.height).unwrap();
            assert_eq!((r.x_start, r.y_start, r.x_end, r.y_end), (0, 0, s.x_addr_end, s.y_addr_end));
            assert_eq!((r.out_width, r.out_height), (s.sensor_out_width, s.sensor_out_height));
        }
    }

    #[test]
    fn roi_matches_sdk_capture() {
        // SDK put_Roi(200, 100, 1000, 800) at 1228x922 (captures/sw_sensor).
        let r = roi_window(&SIZES[2], 200, 100, 1000, 800).unwrap();
        assert_eq!((r.x_start, r.x_end, r.y_start, r.y_end), (112, 4121, 88, 3313));
        assert_eq!((r.out_width, r.out_height), (1004, 808));
    }
}

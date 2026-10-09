//! Supported camera models.
//!
//! Each model is a USB VID/PID pair plus the sensor that sits behind the camera's
//! FPGA. Adding a model means capturing its traffic (see `re/` in the repository) and,
//! if it uses a new sensor, adding a module under [`crate::sensor`].

use crate::sensor::SensorKind;

/// An output size the camera can stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Resolution {
    pub width: u32,
    pub height: u32,
}

impl Resolution {
    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }
}

impl std::fmt::Display for Resolution {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}x{}", self.width, self.height)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorFilter {
    Mono,
    /// Bayer mosaic. The value names the top-left 2×2 cell in reading order.
    Bayer(BayerPattern),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BayerPattern {
    Rggb,
    Grbg,
    Gbrg,
    Bggr,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Model {
    pub name: &'static str,
    pub vendor_id: u16,
    pub product_id: u16,
    pub sensor: SensorKind,
    /// Streamable sizes, largest first. Index = the "size index" used throughout.
    pub resolutions: &'static [Resolution],
    /// Pixel pitch in micrometres.
    pub pixel_size_um: f32,
    pub color: ColorFilter,
    /// Bits per sample delivered in 16-bit transport mode.
    pub max_bit_depth: u8,
    /// Frames arrive bottom-up relative to the vendor software's (upright) image. Raw
    /// frames are delivered as the sensor sends them; developers should flip rows.
    pub rows_bottom_up: bool,
    /// Size indices whose last row is corrupt (the vendor SDK's frames show it too).
    /// The driver replaces it with the same-colour row two above.
    pub bad_last_row: &'static [usize],
}

/// AmScope MU1803-HS: 18 MP, onsemi AR1820HS, USB 3.0.
pub const MU1803_HS: Model = Model {
    name: "MU1803-HS",
    vendor_id: 0x0547,
    product_id: 0x1142,
    sensor: SensorKind::Ar1820,
    resolutions: &[
        Resolution::new(4912, 3684),
        Resolution::new(2456, 1842),
        Resolution::new(1228, 922),
    ],
    pixel_size_um: 1.25,
    // Verified with the sensor's colour-bar test pattern (register 0x0600 = 2) at
    // full resolution: the red bar lights only (0,0), the blue bar only (1,1).
    color: ColorFilter::Bayer(BayerPattern::Rggb),
    max_bit_depth: 12,
    // Verified against an image saved by the vendor app: ours matches it vertically
    // flipped (correlation 0.999).
    rows_bottom_up: true,
    bad_last_row: &[2],
};

/// All models this crate knows how to drive.
pub const MODELS: &[Model] = &[MU1803_HS];

/// Look up a model by USB IDs.
pub fn find(vendor_id: u16, product_id: u16) -> Option<&'static Model> {
    MODELS
        .iter()
        .find(|m| m.vendor_id == vendor_id && m.product_id == product_id)
}

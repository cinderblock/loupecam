//! Image sensors behind the camera FPGA.

pub mod ar1820;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SensorKind {
    /// onsemi (Aptina) AR1820HS, 18 MP, 1.25 µm. Chip ID `0x1820`.
    Ar1820,
}

impl SensorKind {
    /// Value the sensor-ID read returns for this sensor.
    pub fn chip_id(self) -> u16 {
        match self {
            SensorKind::Ar1820 => ar1820::CHIP_ID,
        }
    }
}

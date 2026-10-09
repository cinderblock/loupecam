//! Native driver for AmScope (ToupTek OEM) USB3 microscope cameras.
//!
//! Talks to the camera directly over USB with [`nusb`]. No vendor SDK and no kernel
//! driver are needed: on Windows the camera binds to the built-in WinUSB driver, and
//! on Linux it needs only a udev rule granting access.
//!
//! ```no_run
//! use amscope::{Camera, PixelMode, StreamConfig};
//! let mut cam = Camera::open_first()?;
//! println!("{} serial {}", cam.info().model.name, cam.info().serial);
//! let frames = cam.start(&StreamConfig::new(2, PixelMode::Raw8))?;
//! let frame = frames.recv()?;
//! println!("{}x{} frame #{}", frame.width, frame.height, frame.trailer.sequence);
//! # Ok::<(), amscope::Error>(())
//! ```

mod stream;
mod usb;

pub use amscope_protocol as protocol;
pub use amscope_protocol::frame::{RawFrame, SampleFormat, Trailer};
pub use amscope_protocol::isp::{ColorMatrix, StatsWindow, ToneLut, WbGains};
pub use amscope_protocol::model::{BayerPattern, ColorFilter, Model, Resolution};
pub use amscope_protocol::sensor::ar1820::Gain;
pub use amscope_protocol::session::{DeviceInfo, IspConfig, PixelMode, Roi, StreamConfig, StreamGeometry};
pub use stream::{FrameReceiver, RecvError, StreamStats};

use amscope_protocol::Session;
use std::time::Duration;
use usb::UsbTransport;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Protocol(#[from] amscope_protocol::Error),
    #[error("USB: {0}")]
    Usb(#[from] nusb::Error),
    #[error("no supported camera found")]
    NotFound,
    #[error(transparent)]
    Recv(#[from] RecvError),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// A connected, not yet opened camera.
#[derive(Debug, Clone)]
pub struct CameraDevice {
    pub model: &'static Model,
    /// Platform-specific identifier, stable while the device stays plugged in.
    pub id: String,
    pub bus: String,
    pub port_chain: Vec<u8>,
    nusb: nusb::DeviceInfo,
}

/// List connected cameras of supported models.
pub fn list() -> Result<Vec<CameraDevice>> {
    use nusb::MaybeFuture;
    Ok(nusb::list_devices()
        .wait()?
        .filter_map(|d| {
            let model = protocol::model::find(d.vendor_id(), d.product_id())?;
            Some(CameraDevice {
                model,
                id: format!("{:?}", d.id()),
                bus: d.bus_id().to_string(),
                port_chain: d.port_chain().to_vec(),
                nusb: d,
            })
        })
        .collect())
}

/// An open camera.
pub struct Camera {
    session: Session<UsbTransport>,
    info: DeviceInfo,
    interface: nusb::Interface,
    stream: Option<stream::Reader>,
}

impl Camera {
    pub fn open(device: &CameraDevice) -> Result<Self> {
        use nusb::MaybeFuture;
        let dev = device.nusb.open().wait()?;
        let interface = dev.claim_interface(0).wait()?;
        let transport = UsbTransport::new(interface.clone(), Duration::from_millis(1000));
        let (session, info) = Session::open(transport, device.model, random_u16())?;
        tracing::info!(model = info.model.name, serial = %info.serial, fw = %info.firmware_version, "camera opened");
        Ok(Camera { session, info, interface, stream: None })
    }

    /// Open the first supported camera found.
    pub fn open_first() -> Result<Self> {
        let dev = list()?.into_iter().next().ok_or(Error::NotFound)?;
        Self::open(&dev)
    }

    pub fn info(&self) -> &DeviceInfo {
        &self.info
    }

    pub fn model(&self) -> &'static Model {
        self.info.model
    }

    /// Current stream configuration, if streaming.
    pub fn config(&self) -> Option<&StreamConfig> {
        self.session.config()
    }

    pub fn geometry(&self) -> Option<StreamGeometry> {
        self.session.geometry()
    }

    pub fn is_streaming(&self) -> bool {
        self.stream.is_some()
    }

    /// Start streaming. Frames arrive on the returned receiver. Starting again
    /// reconfigures; earlier receivers are disconnected.
    pub fn start(&mut self, cfg: &StreamConfig) -> Result<FrameReceiver> {
        self.stop()?;
        // Queue bulk transfers before the camera starts sending.
        let res = self.model().resolutions.get(cfg.size_index).copied();
        let res = match (cfg.roi, res) {
            (Some(r), _) => Resolution::new(r.width as u32 & !3, r.height as u32 & !1),
            (None, Some(r)) => r,
            (None, None) => {
                return Err(amscope_protocol::Error::OutOfRange(format!("size index {}", cfg.size_index)).into());
            }
        };
        let (reader, rx) = stream::Reader::spawn(&self.interface, res, cfg.pixel_mode.sample_format())?;
        match self.session.start(cfg) {
            Ok(geom) => {
                reader.set_geometry(geom.resolution, geom.format, self.patch_last_row(cfg));
                self.stream = Some(reader);
                Ok(rx)
            }
            Err(e) => {
                reader.shutdown();
                Err(e.into())
            }
        }
    }

    pub fn stop(&mut self) -> Result<()> {
        if let Some(reader) = self.stream.take() {
            let r = self.session.stop();
            reader.shutdown();
            r?;
        }
        Ok(())
    }

    /// Exposure in µs. Returns the exposure actually programmed.
    pub fn set_exposure(&mut self, exposure_us: u32) -> Result<f64> {
        Ok(self.session.set_exposure(exposure_us)?)
    }

    pub fn set_gain(&mut self, gain: Gain) -> Result<()> {
        Ok(self.session.set_gain(gain)?)
    }

    pub fn set_speed(&mut self, speed: u8) -> Result<StreamGeometry> {
        Ok(self.session.set_speed(speed)?)
    }

    pub fn set_binning(&mut self, binning: bool) -> Result<()> {
        Ok(self.session.set_binning(binning)?)
    }

    pub fn set_roi(&mut self, roi: Option<Roi>) -> Result<StreamGeometry> {
        let geom = self.session.set_roi(roi)?;
        if let Some(r) = &self.stream {
            let patch = self.session.config().is_some_and(|c| self.patch_last_row(c));
            r.set_geometry(geom.resolution, geom.format, patch);
        }
        Ok(geom)
    }

    pub fn set_test_pattern(&mut self, pattern: u8) -> Result<()> {
        Ok(self.session.set_test_pattern(pattern)?)
    }

    pub fn set_wb(&mut self, wb: WbGains) -> Result<()> {
        Ok(self.session.set_wb(wb)?)
    }

    pub fn set_ccm(&mut self, ccm: ColorMatrix) -> Result<()> {
        Ok(self.session.set_ccm(ccm)?)
    }

    pub fn set_lut(&mut self, lut: &ToneLut) -> Result<()> {
        Ok(self.session.set_lut(lut)?)
    }

    pub fn stats(&self) -> Option<StreamStats> {
        self.stream.as_ref().map(|s| s.stats())
    }

    /// The corrupt last row only appears with the full-height window.
    fn patch_last_row(&self, cfg: &StreamConfig) -> bool {
        cfg.roi.is_none() && self.model().bad_last_row.contains(&cfg.size_index)
    }

    /// Low-level access for experimentation.
    pub fn write_sensor(&mut self, register: u16, value: u16) -> Result<()> {
        Ok(self.session.write_sensor(register, value)?)
    }

    /// Low-level access for experimentation.
    pub fn write_fpga(&mut self, register: u8, value: u16) -> Result<()> {
        Ok(self.session.write_fpga(register, value)?)
    }
}

impl Drop for Camera {
    fn drop(&mut self) {
        if let Err(e) = self.stop() {
            tracing::warn!("stopping camera on drop: {e}");
        }
    }
}

/// A random 16-bit value without an RNG dependency: std's `RandomState` is seeded from
/// the OS.
fn random_u16() -> u16 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u64(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64));
    h.finish() as u16
}

//! A control session with one camera: open, configure, start and stop streaming.
//!
//! The sequences here reproduce what the vendor SDK sends, built from named
//! parameters instead of replayed byte blobs. The vendor SDK's 16-byte
//! challenge-response at open is omitted: the camera does not require it (the SDK
//! appears to use it to check the camera is genuine).

use crate::flash::FactoryData;
use crate::frame::SampleFormat;
use crate::isp::{fpga, ColorMatrix, StatsWindow, ToneLut, WbGains};
use crate::model::{Model, Resolution};
use crate::request::{self as rq, ACK};
use crate::sensor::ar1820::{self, reg, Gain, SizeMode};
use crate::{Error, Result, Transport};

/// Identity and version information read at open.
#[derive(Debug, Clone)]
pub struct DeviceInfo {
    pub model: &'static Model,
    pub serial: String,
    pub firmware_version: String,
    pub hardware_version: String,
    /// FPGA version as (major, minor).
    pub fpga_version: (u8, u8),
    pub sensor_id: u16,
    pub production_date: Option<(u16, u8, u8)>,
    pub factory: FactoryData,
}

/// Pixel delivery mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelMode {
    /// Raw sensor data, 8 bits per sample.
    Raw8,
    /// Raw sensor data, 12 significant bits in 16-bit samples.
    Raw12,
    /// FPGA ISP enabled (white balance, colour matrix, tone LUT), 8 bits per sample,
    /// still mosaic-domain: the host demosaics.
    Processed8,
}

impl PixelMode {
    pub fn sample_format(self) -> SampleFormat {
        match self {
            PixelMode::Raw8 | PixelMode::Processed8 => SampleFormat::U8,
            PixelMode::Raw12 => SampleFormat::U16 { bits: 12 },
        }
    }
    fn sixteen_bit(self) -> bool {
        matches!(self, PixelMode::Raw12)
    }
}

/// Region of interest, in output-image pixels of the selected size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Roi {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

/// Hardware ISP parameters (used in [`PixelMode::Processed8`]).
#[derive(Debug, Clone, PartialEq)]
pub struct IspConfig {
    pub wb: WbGains,
    pub ccm: ColorMatrix,
    pub lut: ToneLut,
}

/// Everything needed to start a stream.
#[derive(Debug, Clone, PartialEq)]
pub struct StreamConfig {
    /// Index into [`Model::resolutions`].
    pub size_index: usize,
    pub pixel_mode: PixelMode,
    pub exposure_us: u32,
    pub gain: Gain,
    /// 0 (slowest readout) ..= 3 (fastest).
    pub speed: u8,
    /// Sum neighbouring pixels instead of skipping when subsampling.
    pub binning: bool,
    pub roi: Option<Roi>,
    pub isp: IspConfig,
    pub test_pattern: u8,
}

impl StreamConfig {
    pub fn new(size_index: usize, pixel_mode: PixelMode) -> Self {
        StreamConfig {
            size_index,
            pixel_mode,
            exposure_us: 20_000,
            gain: Gain::UNITY,
            speed: 3,
            binning: false,
            roi: None,
            isp: IspConfig {
                wb: WbGains::UNITY,
                ccm: ColorMatrix::IDENTITY,
                lut: ToneLut::identity(),
            },
            test_pattern: 0,
        }
    }
}

/// Geometry and timing of a running stream.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StreamGeometry {
    pub resolution: Resolution,
    pub format: SampleFormat,
    pub line_length_pck: u16,
    pub frame_length_lines: u16,
    /// Exposure actually programmed, in µs.
    pub exposure_us: f64,
}

impl StreamGeometry {
    /// Upper bound on frame rate from sensor timing.
    pub fn max_fps(&self, exposure_lines: u16) -> f64 {
        let lines = self.frame_length_lines.max(exposure_lines + 1) as f64;
        1e6 / (lines * ar1820::line_time_us(self.line_length_pck))
    }
}

/// Time the sensor needs after [`rq::TRANSFER`] enable before it answers on I²C.
/// Without it, sensor reads/writes answer `09` instead of the `08` ack. The vendor SDK
/// waits ~138 ms.
const SENSOR_POWER_UP: std::time::Duration = std::time::Duration::from_millis(150);

/// Lines the sensor needs beyond the output height (blanking).
const FRAME_BLANKING_LINES: u16 = 92;
/// Exposure programmed before streaming starts; the real one follows.
const STARTUP_EXPOSURE_US: u32 = 1000;

pub struct Session<T: Transport> {
    t: T,
    key: u16,
    model: &'static Model,
    streaming: Option<Active>,
}

#[derive(Debug, Clone)]
struct Active {
    cfg: StreamConfig,
    geom: StreamGeometry,
}

impl<T: Transport> Session<T> {
    /// Open a session: seed the scrambler, enable transfers, and read the camera's
    /// identity. `seed` should be random; any value works.
    pub fn open(mut t: T, model: &'static Model, seed: u16) -> Result<(Self, DeviceInfo)> {
        let r = t.control_in(rq::SEED, seed, 0, 2)?;
        expect_ack(rq::SEED, &r)?;
        let mut s = Session { t, key: rq::session_key(seed), model, streaming: None };
        s.transfer_enable(true)?;
        s.t.delay(SENSOR_POWER_UP);
        let sensor_id = s.read16(rq::READ_SENSOR_ID)?;
        if sensor_id != model.sensor.chip_id() {
            return Err(Error::UnexpectedResponse {
                request: rq::READ,
                detail: format!("sensor ID {sensor_id:#06x}, expected {:#06x} for {}", model.sensor.chip_id(), model.name),
            });
        }
        let _status = s.t.control_in(rq::STATUS, 0, 0, 1)?;
        let factory = FactoryData::parse(&s.read_flash()?)?;
        let firmware_version = s.read_string(rq::FW_VERSION)?;
        let hardware_version = s.read_string(rq::HW_VERSION)?;
        let fpga = s.read16(rq::READ_FPGA_VERSION)?;
        s.write_sensor(reg::UNKNOWN_30B4, 0x0011)?;
        let _ = s.t.control_in(rq::READ3, 0, 0x30b2, 3)?;
        let info = DeviceInfo {
            model,
            serial: factory.serial.clone(),
            firmware_version,
            hardware_version,
            fpga_version: ((fpga >> 8) as u8, fpga as u8),
            sensor_id,
            production_date: factory.production_date(),
            factory,
        };
        Ok((s, info))
    }

    /// Start a new scrambling session on an already-open camera. Required before
    /// starting again after [`stop`](Self::stop): until re-seeded, the camera rejects
    /// register writes (it answers `09` instead of the `08` ack). The vendor SDK does the
    /// same on every restart.
    pub fn reseed(&mut self, seed: u16) -> Result<()> {
        let r = self.t.control_in(rq::SEED, seed, 0, 2)?;
        expect_ack(rq::SEED, &r)?;
        self.key = rq::session_key(seed);
        self.transfer_enable(true)?;
        self.t.delay(SENSOR_POWER_UP);
        let id = self.read16(rq::READ_SENSOR_ID)?;
        if id != self.model.sensor.chip_id() {
            return Err(Error::UnexpectedResponse { request: rq::READ, detail: format!("sensor ID {id:#06x} after reseed") });
        }
        let _status = self.t.control_in(rq::STATUS, 0, 0, 1)?;
        Ok(())
    }

    pub fn model(&self) -> &'static Model {
        self.model
    }

    pub fn geometry(&self) -> Option<StreamGeometry> {
        self.streaming.as_ref().map(|a| a.geom)
    }

    pub fn config(&self) -> Option<&StreamConfig> {
        self.streaming.as_ref().map(|a| &a.cfg)
    }

    /// Program the sensor and FPGA for `cfg` and start streaming. The caller must have
    /// bulk transfers ready on endpoint 0x81.
    pub fn start(&mut self, cfg: &StreamConfig) -> Result<StreamGeometry> {
        if self.streaming.is_some() {
            self.stop()?;
        }
        let size = Self::size_mode(cfg.size_index)?;
        let sixteen = cfg.pixel_mode.sixteen_bit();

        self.write_fpga(fpga::UNKNOWN_FC, 1)?;
        self.write_fpga(fpga::UNKNOWN_F8, 1)?;
        let _ = self.t.control_in(rq::READ3, 0, 0x0000, 3)?;
        let _ = self.t.control_in(rq::READ3, 0, 0x3064, 3)?;

        self.write_sensor(reg::MODE_SELECT, 0)?;
        self.hold(|s| {
            for &(r, v) in ar1820::PLL {
                s.write_sensor(r, v)?;
            }
            Ok(())
        })?;
        self.write_fpga(fpga::TRANSPORT_16BIT, sixteen as u16)?;
        self.write_sensor(reg::MODE_SELECT, 0)?;
        for &(r, v) in ar1820::TUNING {
            self.write_sensor(r, v)?;
        }
        self.write_fpga(fpga::SIZE_INDEX, size.fpga_size_index)?;
        let (resolution, out_h) = self.program_window(&size, cfg.roi)?;
        let line_length = size.line_length(cfg.speed, sixteen);
        let frame_length = out_h + FRAME_BLANKING_LINES;
        self.hold(|s| s.write_sensor(reg::LINE_LENGTH_PCK, line_length))?;
        self.write_exposure_lines(frame_length, ar1820::exposure_lines(cfg.exposure_us, line_length))?;
        let read_mode = size.read_mode | if cfg.binning { ar1820::READ_MODE_BIN } else { 0 };
        self.write_sensor(reg::READ_MODE, read_mode)?;
        self.write_sensor(reg::RESET_REGISTER, 0x0010)?;
        self.write_sensor(reg::DATA_PEDESTAL, 0)?;
        self.write_exposure_lines(frame_length, ar1820::exposure_lines(STARTUP_EXPOSURE_US, line_length))?;
        self.write_sensor(reg::RESET_REGISTER, 0x001e)?; // streaming on

        let processed = cfg.pixel_mode == PixelMode::Processed8;
        self.write_fpga(fpga::ISP_ENABLE, processed as u16)?;
        self.write_fpga(fpga::UNKNOWN_FA, 1)?;
        self.write_fpga(fpga::UNKNOWN_F4, 4)?;
        self.write_fpga(fpga::UNKNOWN_F6, 4)?;
        // The SDK's ordering differs by mode; reproduced as observed.
        if processed {
            self.write_ccm(&cfg.isp.ccm)?;
            self.write_lut(&cfg.isp.lut)?;
            self.write_wb(cfg.isp.wb)?;
        } else {
            self.write_wb(WbGains::UNITY)?;
            self.write_ccm(&ColorMatrix::IDENTITY)?;
            self.write_lut(&ToneLut::identity())?;
        }
        let lines = ar1820::exposure_lines(cfg.exposure_us, line_length);
        self.write_exposure_lines(frame_length, lines)?;
        self.hold(|s| s.write_sensor(reg::GLOBAL_GAIN, cfg.gain.register()))?;
        if cfg.test_pattern != 0 {
            self.write_fpga(fpga::TEST_PATTERN, cfg.test_pattern as u16)?;
        }
        let win = StatsWindow::default_for(resolution.width as u16, resolution.height as u16);
        self.t.control_out(rq::STATS_WINDOW, 0, 0, &win.payload(0))?;
        self.t.control_out(rq::STATS_WINDOW, 0, 0, &win.payload(1))?;
        self.transfer_enable(true)?;

        let geom = StreamGeometry {
            resolution,
            format: cfg.pixel_mode.sample_format(),
            line_length_pck: line_length,
            frame_length_lines: frame_length,
            exposure_us: ar1820::lines_to_us(lines, line_length),
        };
        self.streaming = Some(Active { cfg: cfg.clone(), geom });
        Ok(geom)
    }

    pub fn stop(&mut self) -> Result<()> {
        self.write_sensor(reg::MODE_SELECT, 0)?;
        self.transfer_enable(false)?;
        self.transfer_enable(false)?;
        let _ = self.t.control_in(rq::STOPPED, 0, 0, 2)?;
        self.streaming = None;
        Ok(())
    }

    /// Change exposure while streaming. Returns the exposure actually programmed (µs).
    pub fn set_exposure(&mut self, exposure_us: u32) -> Result<f64> {
        let a = self.active_mut()?;
        a.cfg.exposure_us = exposure_us;
        let (fl, ll) = (a.geom.frame_length_lines, a.geom.line_length_pck);
        let lines = ar1820::exposure_lines(exposure_us, ll);
        self.write_exposure_lines(fl, lines)?;
        let us = ar1820::lines_to_us(lines, ll);
        self.active_mut()?.geom.exposure_us = us;
        Ok(us)
    }

    pub fn set_gain(&mut self, gain: Gain) -> Result<()> {
        self.active_mut()?.cfg.gain = gain;
        self.hold(|s| s.write_sensor(reg::GLOBAL_GAIN, gain.register()))
    }

    pub fn set_speed(&mut self, speed: u8) -> Result<StreamGeometry> {
        let a = self.active_mut()?;
        a.cfg.speed = speed.min(3);
        let size = Self::size_mode(a.cfg.size_index)?;
        let a = self.active_mut()?;
        let ll = size.line_length(a.cfg.speed, a.cfg.pixel_mode.sixteen_bit());
        a.geom.line_length_pck = ll;
        let (fl, us) = (a.geom.frame_length_lines, a.cfg.exposure_us);
        self.hold(|s| s.write_sensor(reg::LINE_LENGTH_PCK, ll))?;
        let lines = ar1820::exposure_lines(us, ll);
        self.write_exposure_lines(fl, lines)?;
        let a = self.active_mut()?;
        a.geom.exposure_us = ar1820::lines_to_us(lines, ll);
        Ok(a.geom)
    }

    pub fn set_binning(&mut self, binning: bool) -> Result<()> {
        let a = self.active_mut()?;
        a.cfg.binning = binning;
        let size = Self::size_mode(a.cfg.size_index)?;
        self.write_fpga(fpga::SIZE_INDEX, size.fpga_size_index)?;
        self.write_sensor(reg::READ_MODE, size.read_mode | if binning { ar1820::READ_MODE_BIN } else { 0 })
    }

    pub fn set_test_pattern(&mut self, pattern: u8) -> Result<()> {
        self.active_mut()?.cfg.test_pattern = pattern;
        self.write_fpga(fpga::TEST_PATTERN, pattern as u16)
    }

    /// Update ISP parameters (only meaningful in [`PixelMode::Processed8`]).
    pub fn set_isp(&mut self, isp: &IspConfig) -> Result<()> {
        self.active_mut()?.cfg.isp = isp.clone();
        self.write_isp(isp)
    }

    pub fn set_wb(&mut self, wb: WbGains) -> Result<()> {
        self.active_mut()?.cfg.isp.wb = wb;
        self.write_wb(wb)
    }

    pub fn set_ccm(&mut self, ccm: ColorMatrix) -> Result<()> {
        self.active_mut()?.cfg.isp.ccm = ccm;
        self.write_ccm(&ccm)
    }

    pub fn set_lut(&mut self, lut: &ToneLut) -> Result<()> {
        self.active_mut()?.cfg.isp.lut = lut.clone();
        self.write_lut(lut)
    }

    /// Change the ROI while streaming. The frame size changes, so the bulk reader must
    /// be told the new geometry (returned).
    pub fn set_roi(&mut self, roi: Option<Roi>) -> Result<StreamGeometry> {
        let a = self.active_mut()?;
        a.cfg.roi = roi;
        let cfg = a.cfg.clone();
        let size = Self::size_mode(cfg.size_index)?;
        let (resolution, out_h) = self.program_window(&size, roi)?;
        let ll = size.line_length(cfg.speed, cfg.pixel_mode.sixteen_bit());
        self.hold(|s| s.write_sensor(reg::LINE_LENGTH_PCK, ll))?;
        let fl = out_h + FRAME_BLANKING_LINES;
        let lines = ar1820::exposure_lines(cfg.exposure_us, ll);
        self.write_exposure_lines(fl, lines)?;
        let a = self.active_mut()?;
        a.geom.resolution = resolution;
        a.geom.frame_length_lines = fl;
        a.geom.line_length_pck = ll;
        a.geom.exposure_us = ar1820::lines_to_us(lines, ll);
        Ok(a.geom)
    }

    /// Raw access for experimentation: write a sensor register.
    pub fn write_sensor(&mut self, register: u16, value: u16) -> Result<()> {
        let r = self.t.control_in(rq::WRITE_SENSOR, value ^ self.key, register ^ self.key, 1)?;
        expect_ack(rq::WRITE_SENSOR, &r).map_err(|e| context(e, format!("sensor register {register:#06x} = {value:#06x}")))
    }

    /// Raw access for experimentation: write an FPGA register.
    pub fn write_fpga(&mut self, register: u8, value: u16) -> Result<()> {
        let index = (register as u16) << 8;
        let r = self.t.control_in(rq::WRITE_FPGA, value ^ self.key, index ^ self.key, 1)?;
        expect_ack(rq::WRITE_FPGA, &r).map_err(|e| context(e, format!("FPGA register {register:#04x} = {value:#06x}")))
    }

    /// Raw access for experimentation: scrambled 16-bit read.
    pub fn read16(&mut self, (value, index): (u16, u16)) -> Result<u16> {
        let r = self.t.control_in(rq::READ, value ^ self.key, index ^ self.key, 2)?;
        match r[..] {
            [hi, lo] => Ok(u16::from_be_bytes([hi, lo])),
            _ => Err(Error::UnexpectedResponse { request: rq::READ, detail: format!("{} bytes", r.len()) }),
        }
    }

    pub fn into_transport(self) -> T {
        self.t
    }

    pub fn transport(&mut self) -> &mut T {
        &mut self.t
    }

    // --- internals ---

    fn active_mut(&mut self) -> Result<&mut Active> {
        self.streaming
            .as_mut()
            .ok_or_else(|| Error::Unsupported("camera is not streaming".into()))
    }

    fn size_mode(index: usize) -> Result<SizeMode> {
        ar1820::SIZES
            .get(index)
            .copied()
            .ok_or_else(|| Error::OutOfRange(format!("size index {index}")))
    }

    fn program_window(&mut self, size: &SizeMode, roi: Option<Roi>) -> Result<(Resolution, u16)> {
        let r = match roi {
            Some(r) => ar1820::roi_window(size, r.x, r.y, r.width, r.height)?,
            None => ar1820::roi_window(size, 0, 0, size.width, size.height)?,
        };
        self.hold(|s| {
            s.write_sensor(reg::X_ADDR_START, r.x_start)?;
            s.write_sensor(reg::X_ADDR_END, r.x_end)?;
            s.write_sensor(reg::Y_ADDR_START, r.y_start)?;
            s.write_sensor(reg::Y_ADDR_END, r.y_end)?;
            s.write_sensor(reg::X_OUTPUT_SIZE, r.out_width)?;
            s.write_sensor(reg::Y_OUTPUT_SIZE, r.out_height)?;
            s.write_fpga(fpga::WIDTH_DIV4, r.width / 4)?;
            s.write_fpga(fpga::HEIGHT, r.height)?;
            s.write_fpga(fpga::UNKNOWN_A6, 0)?;
            s.write_fpga(fpga::UNKNOWN_A8, 2)
        })?;
        Ok((Resolution::new(r.width as u32, r.height as u32), r.out_height))
    }

    fn write_exposure_lines(&mut self, frame_length: u16, lines: u16) -> Result<()> {
        self.write_sensor(reg::FRAME_LENGTH_LINES, frame_length)?;
        self.write_sensor(reg::COARSE_INTEGRATION_TIME, lines)
    }

    fn write_isp(&mut self, isp: &IspConfig) -> Result<()> {
        self.write_wb(isp.wb)?;
        self.write_ccm(&isp.ccm)?;
        self.write_lut(&isp.lut)
    }

    fn write_wb(&mut self, wb: WbGains) -> Result<()> {
        let [r, g, b] = wb.registers();
        self.write_fpga(fpga::WB_R, r)?;
        self.write_fpga(fpga::WB_G, g)?;
        self.write_fpga(fpga::WB_B, b)
    }

    fn write_ccm(&mut self, ccm: &ColorMatrix) -> Result<()> {
        for (i, v) in ccm.registers().into_iter().enumerate() {
            self.write_fpga(fpga::CCM_BASE + 2 * i as u8, v as u16)?;
        }
        Ok(())
    }

    fn write_lut(&mut self, lut: &ToneLut) -> Result<()> {
        for (index, data) in lut.chunks() {
            self.t.control_out(rq::LUT_UPLOAD, 0, index, &data)?;
        }
        Ok(())
    }

    /// Run `f` inside a sensor grouped-parameter hold so its writes take effect on the
    /// same frame.
    fn hold(&mut self, f: impl FnOnce(&mut Self) -> Result<()>) -> Result<()> {
        self.write_sensor(reg::GROUPED_PARAMETER_HOLD, 0x0100)?;
        let r = f(self);
        self.write_sensor(reg::GROUPED_PARAMETER_HOLD, 0x0000)?;
        r
    }

    fn transfer_enable(&mut self, on: bool) -> Result<()> {
        self.t.control_out(rq::TRANSFER, if on { 3 } else { 0 }, 0x0f, &[])
    }

    fn read_flash(&mut self) -> Result<Vec<u8>> {
        let head = self.t.control_in(rq::FLASH_READ, 0, 0, 4)?;
        let len_word = u32::from_le_bytes(
            head.get(..4)
                .and_then(|b| b.try_into().ok())
                .ok_or_else(|| Error::Flash("short length read".into()))?,
        );
        let total = FactoryData::total_len(len_word);
        if total > 1 << 20 {
            return Err(Error::Flash(format!("implausible length {total}")));
        }
        let mut blob = Vec::with_capacity(total);
        while blob.len() < total {
            let n = (total - blob.len()).min(4096) as u16;
            let chunk = self.t.control_in(rq::FLASH_READ, blob.len() as u16, 0, n)?;
            if chunk.is_empty() {
                return Err(Error::Flash(format!("empty read at offset {}", blob.len())));
            }
            blob.extend_from_slice(&chunk);
        }
        Ok(blob)
    }

    fn read_string(&mut self, request: u8) -> Result<String> {
        let b = self.t.control_in(request, 0, 0, 16)?;
        let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
        Ok(String::from_utf8_lossy(&b[..end]).into_owned())
    }
}

fn context(e: Error, what: String) -> Error {
    match e {
        Error::UnexpectedResponse { request, detail } => Error::UnexpectedResponse { request, detail: format!("{what}: {detail}") },
        e => e,
    }
}

fn expect_ack(request: u8, r: &[u8]) -> Result<()> {
    if r.first() == Some(&ACK) {
        Ok(())
    } else {
        Err(Error::UnexpectedResponse { request, detail: format!("expected ack 08, got {r:02x?}") })
    }
}

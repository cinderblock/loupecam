//! Python bindings: the `loupecam._native` extension module.
//!
//! `python/loupecam/__init__.py` re-exports it as the `loupecam` package, and
//! `_native.pyi` documents the API for type checkers and editors. Blocking calls (USB
//! I/O, waiting for frames, developing) release the GIL.

use loupecam::{AnyCamera, CameraDevice, ColorFilter, Gain, Model, PixelMode, RawFrame, RecvError, Roi, SampleFormat, StreamConfig};
use loupecam_isp::encode::{self, Format};
use loupecam_isp::stats;
use loupecam_isp::tone::{ToneCurve, Transfer};
use loupecam_isp::{Demosaic, DevelopParams, Rotation};
use numpy::{IntoPyArray, PyArrayMethods};
use pyo3::create_exception;
use pyo3::exceptions::{PyException, PyTimeoutError, PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

create_exception!(loupecam, LoupecamError, PyException, "An error from the camera or the driver.");
create_exception!(
    loupecam,
    StreamStopped,
    LoupecamError,
    "The stream ended: the camera was stopped, restarted or closed."
);

fn err(e: impl std::fmt::Display) -> PyErr {
    LoupecamError::new_err(e.to_string())
}

fn recv_err(e: RecvError) -> PyErr {
    match e {
        RecvError::Stopped => StreamStopped::new_err("the stream stopped"),
        RecvError::Timeout => PyTimeoutError::new_err("timed out waiting for a frame"),
    }
}

fn cam_err(e: loupecam::Error) -> PyErr {
    match e {
        loupecam::Error::Recv(r) => recv_err(r),
        e => err(e),
    }
}

fn value_err(e: impl std::fmt::Display) -> PyErr {
    PyValueError::new_err(e.to_string())
}

/// A connected camera that isn't open yet.
#[pyclass(module = "loupecam", frozen, name = "CameraDevice")]
struct PyDevice {
    inner: CameraDevice,
}

#[pymethods]
impl PyDevice {
    #[getter]
    fn model(&self) -> PyModel {
        PyModel(self.inner.model)
    }
    /// Platform-specific identifier, stable while the camera stays plugged in.
    #[getter]
    fn id(&self) -> &str {
        &self.inner.id
    }
    #[getter]
    fn bus(&self) -> &str {
        &self.inner.bus
    }
    #[getter]
    fn port_chain(&self) -> Vec<u8> {
        self.inner.port_chain.clone()
    }
    /// Open this camera.
    fn open(&self, py: Python<'_>) -> PyResult<PyCamera> {
        let dev = self.inner.clone();
        let cam = py.detach(move || AnyCamera::open(&dev)).map_err(cam_err)?;
        Ok(PyCamera::new(cam))
    }
    fn __repr__(&self) -> String {
        format!("<CameraDevice {} on bus {} port {:?}>", self.inner.model.name, self.inner.bus, self.inner.port_chain)
    }
}

/// A camera model: its sensor, sizes and colour filter.
#[pyclass(module = "loupecam", frozen, name = "Model")]
struct PyModel(&'static Model);

#[pymethods]
impl PyModel {
    #[getter]
    fn name(&self) -> &'static str {
        self.0.name
    }
    #[getter]
    fn vendor_id(&self) -> u16 {
        self.0.vendor_id
    }
    #[getter]
    fn product_id(&self) -> u16 {
        self.0.product_id
    }
    /// Streamable sizes as `(width, height)`, largest first. The index is the `size`
    /// passed to `Camera.start`.
    #[getter]
    fn resolutions(&self) -> Vec<(u32, u32)> {
        self.0.resolutions.iter().map(|r| (r.width, r.height)).collect()
    }
    /// Pixel pitch in micrometres.
    #[getter]
    fn pixel_size_um(&self) -> f32 {
        self.0.pixel_size_um
    }
    /// The Bayer mosaic's top-left 2×2 cell, e.g. `"RGGB"`; `None` for monochrome.
    #[getter]
    fn bayer_pattern(&self) -> Option<&'static str> {
        match self.0.color {
            ColorFilter::Bayer(p) => Some(p.name()),
            ColorFilter::Mono => None,
        }
    }
    /// Significant bits per sample in the `raw12` mode.
    #[getter]
    fn max_bit_depth(&self) -> u8 {
        self.0.max_bit_depth
    }
    fn __repr__(&self) -> String {
        format!("<Model {}>", self.0.name)
    }
}

/// Identity and versions read from an open camera.
#[pyclass(module = "loupecam", frozen, name = "CameraInfo")]
struct PyInfo(loupecam::DeviceInfo);

#[pymethods]
impl PyInfo {
    #[getter]
    fn model(&self) -> PyModel {
        PyModel(self.0.model)
    }
    #[getter]
    fn serial(&self) -> &str {
        &self.0.serial
    }
    #[getter]
    fn firmware_version(&self) -> &str {
        &self.0.firmware_version
    }
    #[getter]
    fn hardware_version(&self) -> &str {
        &self.0.hardware_version
    }
    #[getter]
    fn fpga_version(&self) -> (u8, u8) {
        self.0.fpga_version
    }
    #[getter]
    fn sensor_id(&self) -> u16 {
        self.0.sensor_id
    }
    /// `(year, month, day)`, when the camera records it.
    #[getter]
    fn production_date(&self) -> Option<(u16, u8, u8)> {
        self.0.production_date
    }
    fn __repr__(&self) -> String {
        format!("<CameraInfo {} serial {} firmware {}>", self.0.model.name, self.0.serial, self.0.firmware_version)
    }
}

/// Geometry and timing of a running stream.
#[pyclass(module = "loupecam", frozen, get_all, name = "Geometry")]
struct PyGeometry {
    width: u32,
    height: u32,
    /// Significant bits per sample (8 or 12).
    bits: u8,
    /// The exposure actually programmed, in µs.
    exposure_us: f64,
    /// The highest frame rate this geometry allows at the current exposure.
    max_fps: f64,
}

#[pymethods]
impl PyGeometry {
    fn __repr__(&self) -> String {
        format!("<Geometry {}x{} {}-bit, {:.0} µs, ≤{:.1} fps>", self.width, self.height, self.bits, self.exposure_us, self.max_fps)
    }
}

fn geometry(g: loupecam::StreamGeometry) -> PyGeometry {
    let lines = loupecam::protocol::sensor::ar1820::exposure_lines(g.exposure_us.round() as u32, g.line_length_pck);
    PyGeometry {
        width: g.resolution.width,
        height: g.resolution.height,
        bits: bits(g.format),
        exposure_us: g.exposure_us,
        max_fps: g.max_fps(lines),
    }
}

fn bits(f: SampleFormat) -> u8 {
    match f {
        SampleFormat::U8 => 8,
        SampleFormat::U16 { bits } => bits,
    }
}

/// Counters for a running stream.
#[pyclass(module = "loupecam", frozen, get_all, name = "StreamStats")]
struct PyStreamStats {
    frames: u64,
    /// Frames dropped because the reader didn't keep up.
    dropped: u64,
    /// Transfers or partial frames discarded while (re)synchronising.
    discarded: u64,
    bytes: u64,
    transfer_errors: u64,
    /// Frame rate over the last second.
    fps: f64,
    /// Frames missing according to the camera's sequence counter.
    sequence_gaps: u64,
}

#[pymethods]
impl PyStreamStats {
    fn __repr__(&self) -> String {
        format!("<StreamStats {} frames, {:.1} fps, {} dropped>", self.frames, self.fps, self.dropped)
    }
}

impl From<loupecam::StreamStats> for PyStreamStats {
    fn from(s: loupecam::StreamStats) -> Self {
        PyStreamStats {
            frames: s.frames,
            dropped: s.dropped,
            discarded: s.discarded,
            bytes: s.bytes,
            transfer_errors: s.transfer_errors,
            fps: s.fps,
            sequence_gaps: s.sequence_gaps,
        }
    }
}

/// An open camera. Closing it (or leaving a `with` block) releases the device.
#[pyclass(module = "loupecam", frozen, name = "Camera")]
struct PyCamera {
    inner: Mutex<Option<AnyCamera>>,
    model: &'static Model,
}

impl PyCamera {
    fn new(cam: AnyCamera) -> Self {
        PyCamera { model: cam.model(), inner: Mutex::new(Some(cam)) }
    }

    fn lock(&self) -> PyResult<MutexGuard<'_, Option<AnyCamera>>> {
        let g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if g.is_none() {
            return Err(err("the camera is closed"));
        }
        Ok(g)
    }

    /// Run `f` on the camera with the GIL released.
    fn with<T: Send>(&self, py: Python<'_>, f: impl FnOnce(&mut AnyCamera) -> loupecam::Result<T> + Send) -> PyResult<T> {
        py.detach(|| {
            let mut g = self.lock()?;
            f(g.as_mut().unwrap()).map_err(cam_err)
        })
    }
}

#[pymethods]
impl PyCamera {
    #[getter]
    fn info(&self) -> PyResult<PyInfo> {
        Ok(PyInfo(self.lock()?.as_ref().unwrap().info().clone()))
    }
    #[getter]
    fn model(&self) -> PyModel {
        PyModel(self.model)
    }
    /// Whether this is the simulated camera.
    #[getter]
    fn simulated(&self) -> PyResult<bool> {
        Ok(self.lock()?.as_ref().unwrap().is_simulated())
    }
    #[getter]
    fn streaming(&self) -> PyResult<bool> {
        Ok(self.lock()?.as_ref().unwrap().is_streaming())
    }
    #[getter]
    fn closed(&self) -> bool {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).is_none()
    }
    /// The running stream's geometry, or `None` when not streaming.
    #[getter]
    fn geometry(&self) -> PyResult<Option<PyGeometry>> {
        Ok(self.lock()?.as_ref().unwrap().geometry().map(geometry))
    }
    #[getter]
    fn stats(&self) -> PyResult<Option<PyStreamStats>> {
        Ok(self.lock()?.as_ref().unwrap().stats().map(Into::into))
    }
    /// The requested exposure in µs, while streaming.
    #[getter]
    fn exposure_us(&self) -> PyResult<Option<u32>> {
        Ok(self.lock()?.as_ref().unwrap().config().map(|c| c.exposure_us))
    }
    /// The gain multiplier, while streaming.
    #[getter]
    fn gain(&self) -> PyResult<Option<f32>> {
        Ok(self.lock()?.as_ref().unwrap().config().map(|c| c.gain.multiplier()))
    }
    /// The largest gain multiplier `set_gain` accepts.
    #[staticmethod]
    fn max_gain() -> f32 {
        Gain::max_multiplier()
    }

    /// Start streaming and return the stream. Starting again reconfigures the
    /// camera, and earlier streams end.
    #[pyo3(signature = (size = 0, mode = "raw8", *, exposure_us = 20_000, gain = 1.0, speed = 3, binning = false, roi = None))]
    #[allow(clippy::too_many_arguments)]
    fn start(
        &self,
        py: Python<'_>,
        size: usize,
        mode: &str,
        exposure_us: u32,
        gain: f32,
        speed: u8,
        binning: bool,
        roi: Option<(u16, u16, u16, u16)>,
    ) -> PyResult<PyStream> {
        let mode: PixelMode = mode.parse().map_err(value_err)?;
        let mut cfg = StreamConfig::new(size, mode);
        cfg.exposure_us = exposure_us;
        cfg.gain = Gain::for_multiplier(gain).map_err(value_err)?;
        cfg.speed = speed;
        cfg.binning = binning;
        cfg.roi = roi.map(|(x, y, width, height)| Roi { x, y, width, height });
        let rx = self.with(py, move |c| c.start(&cfg))?;
        Ok(PyStream { rx, model: self.model })
    }

    /// Stop streaming (the camera stays open).
    fn stop(&self, py: Python<'_>) -> PyResult<()> {
        self.with(py, |c| c.stop())
    }

    /// Stop streaming and release the camera. Closing twice is harmless.
    fn close(&self, py: Python<'_>) {
        let cam = self.inner.lock().unwrap_or_else(|e| e.into_inner()).take();
        // Dropping stops the stream: USB I/O, so without the GIL.
        py.detach(move || drop(cam));
    }

    /// Set the exposure in µs and return the exposure actually programmed.
    fn set_exposure(&self, py: Python<'_>, exposure_us: u32) -> PyResult<f64> {
        self.with(py, move |c| c.set_exposure(exposure_us))
    }

    /// Set the gain as a multiplier (1.0 = unity, up to `Camera.max_gain()`).
    fn set_gain(&self, py: Python<'_>, gain: f32) -> PyResult<()> {
        let g = Gain::for_multiplier(gain).map_err(value_err)?;
        self.with(py, move |c| c.set_gain(g))
    }

    /// Sensor readout speed, 0 (slowest) to 3 (fastest).
    fn set_speed(&self, py: Python<'_>, speed: u8) -> PyResult<PyGeometry> {
        self.with(py, move |c| c.set_speed(speed)).map(geometry)
    }

    fn set_binning(&self, py: Python<'_>, binning: bool) -> PyResult<()> {
        self.with(py, move |c| c.set_binning(binning))
    }

    /// Stream only a region `(x, y, width, height)` of the current size, or the whole
    /// frame with `None`.
    #[pyo3(signature = (roi))]
    fn set_roi(&self, py: Python<'_>, roi: Option<(u16, u16, u16, u16)>) -> PyResult<PyGeometry> {
        let roi = roi.map(|(x, y, width, height)| Roi { x, y, width, height });
        self.with(py, move |c| c.set_roi(roi)).map(geometry)
    }

    fn __enter__(slf: Py<Self>) -> Py<Self> {
        slf
    }

    fn __exit__(&self, py: Python<'_>, _ty: Py<PyAny>, _value: Py<PyAny>, _tb: Py<PyAny>) {
        self.close(py)
    }

    fn __repr__(&self) -> String {
        match self.inner.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            Some(c) => format!("<Camera {} serial {}>", self.model.name, c.info().serial),
            None => format!("<Camera {} (closed)>", self.model.name),
        }
    }
}

/// Frames from a running camera. Iterating yields frames until the stream ends.
#[pyclass(module = "loupecam", frozen, name = "Stream")]
struct PyStream {
    rx: loupecam::FrameReceiver,
    model: &'static Model,
}

/// How long a blocking wait runs before checking for Ctrl-C.
const SIGNAL_POLL: Duration = Duration::from_millis(100);

impl PyStream {
    /// Wait for a frame, staying responsive to Ctrl-C.
    fn wait(&self, py: Python<'_>, timeout: Option<f64>, latest: bool) -> PyResult<PyFrame> {
        let deadline = timeout.map(|t| Instant::now() + Duration::from_secs_f64(t.max(0.0)));
        loop {
            let slice = match deadline {
                Some(d) => d.saturating_duration_since(Instant::now()).min(SIGNAL_POLL),
                None => SIGNAL_POLL,
            };
            let r = py.detach(|| if latest { self.rx.recv_latest(slice) } else { self.rx.recv_timeout(slice) });
            match r {
                Ok(raw) => return Ok(PyFrame { raw, model: self.model }),
                Err(RecvError::Timeout) if deadline.is_none_or(|d| Instant::now() < d) => py.check_signals()?,
                Err(e) => return Err(recv_err(e)),
            }
        }
    }
}

#[pymethods]
impl PyStream {
    /// The next frame, waiting up to `timeout` seconds (forever if `None`). Raises
    /// `TimeoutError`, or `StreamStopped` once the stream has ended.
    #[pyo3(signature = (timeout = None))]
    fn read(&self, py: Python<'_>, timeout: Option<f64>) -> PyResult<PyFrame> {
        self.wait(py, timeout, false)
    }

    /// The newest frame, discarding older queued ones (for live views).
    #[pyo3(signature = (timeout = 1.0))]
    fn latest(&self, py: Python<'_>, timeout: Option<f64>) -> PyResult<PyFrame> {
        self.wait(py, timeout, true)
    }

    /// A frame if one is ready, else `None`.
    fn try_read(&self) -> Option<PyFrame> {
        self.rx.try_recv().map(|raw| PyFrame { raw, model: self.model })
    }

    #[getter]
    fn stats(&self) -> PyStreamStats {
        self.rx.stats().into()
    }

    fn __iter__(slf: Py<Self>) -> Py<Self> {
        slf
    }

    fn __next__(&self, py: Python<'_>) -> PyResult<Option<PyFrame>> {
        match self.wait(py, None, false) {
            Ok(f) => Ok(Some(f)),
            Err(e) if e.is_instance_of::<StreamStopped>(py) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

/// One frame: the raw Bayer mosaic as the sensor delivered it.
#[pyclass(module = "loupecam", frozen, name = "Frame")]
struct PyFrame {
    raw: RawFrame,
    model: &'static Model,
}

/// Statistics of a frame's raw mosaic.
#[pyclass(module = "loupecam", frozen, get_all, name = "FrameStats")]
struct PyFrameStats {
    /// Mean linear level of R, G and B, 0 to 1.
    mean: (f32, f32, f32),
    /// Mean luma (Rec. 709) after white balance, 0 to 1.
    luma: f32,
    /// Fraction of 2×2 cells with a channel at or near full scale.
    clipped: f32,
    /// 256-bin histogram of per-cell linear luma.
    histogram: Vec<u32>,
}

#[pymethods]
impl PyFrameStats {
    fn __repr__(&self) -> String {
        format!("<FrameStats luma {:.3}, clipped {:.1}%>", self.luma, self.clipped * 100.0)
    }
}

impl PyFrame {
    fn pattern(&self) -> loupecam::BayerPattern {
        match self.model.color {
            ColorFilter::Bayer(p) => p,
            ColorFilter::Mono => loupecam::BayerPattern::Rggb,
        }
    }

    fn region(&self, r: Option<(u32, u32, u32, u32)>) -> Option<stats::Rect> {
        r.map(|(x, y, width, height)| stats::Rect { x, y, width, height })
    }

    fn grey_world(&self, region: Option<stats::Rect>) -> [f32; 3] {
        loupecam_isp::auto::grey_world(&stats::compute(&self.raw, self.pattern(), region, 2, [1.0; 3]))
    }

    /// Develop parameters (and output bit depth) from `develop()`'s keyword arguments.
    fn params(&self, options: Option<&Bound<'_, PyDict>>) -> PyResult<(DevelopParams, u8)> {
        let mut p = DevelopParams::for_model(self.model);
        let mut depth = 8;
        let (mut flip_h, mut flip_v, mut rotation) = (false, false, 0);
        let Some(options) = options else { return Ok((p, depth)) };
        for (k, v) in options.iter() {
            let key: String = k.extract()?;
            match key.as_str() {
                "demosaic" => p.demosaic = v.extract::<String>()?.parse::<Demosaic>().map_err(value_err)?,
                "white_balance" => {
                    if let Ok(s) = v.extract::<String>() {
                        match s.as_str() {
                            "auto" => p.wb = self.grey_world(None),
                            "daylight" => {}
                            "none" => p.wb = [1.0; 3],
                            _ => return Err(value_err(format!("white_balance {s:?}: use \"auto\", \"daylight\", \"none\" or (r, g, b)"))),
                        }
                    } else if !v.is_none() {
                        let (r, g, b): (f32, f32, f32) = v.extract()?;
                        p.wb = [r, g, b];
                    }
                }
                "color_matrix" => {
                    if let Ok(on) = v.extract::<bool>() {
                        if !on {
                            p.ccm = loupecam_isp::IDENTITY;
                        }
                    } else if !v.is_none() {
                        let m: [[f32; 3]; 3] = v.extract()?;
                        p.ccm = m;
                    }
                }
                "tone" => {
                    let transfer = if let Ok(g) = v.extract::<f32>() {
                        Transfer::Gamma(g)
                    } else {
                        match v.extract::<String>()?.as_str() {
                            "srgb" => Transfer::Srgb,
                            "linear" => Transfer::Linear,
                            s => return Err(value_err(format!("tone {s:?}: use \"srgb\", \"linear\" or a gamma value"))),
                        }
                    };
                    p.tone = ToneCurve { transfer, ..p.tone };
                }
                "black_point" => p.tone.black_point = v.extract()?,
                "white_point" => p.tone.white_point = v.extract()?,
                "brightness" => p.tone.brightness = v.extract()?,
                "contrast" => p.tone.contrast = v.extract()?,
                "black_level" => {
                    // In the frame's own sample units; the pipeline works at 16-bit scale.
                    let level: u32 = v.extract()?;
                    let shift = 16 - bits(self.raw.format) as u32;
                    p.black_level = (level << shift).min(u16::MAX as u32) as u16;
                }
                "monochrome" => p.monochrome = v.extract()?,
                "negative" => p.negative = v.extract()?,
                "flip_horizontal" => flip_h = v.extract()?,
                "flip_vertical" => flip_v = v.extract()?,
                "rotation" => rotation = v.extract()?,
                "depth" => {
                    depth = v.extract()?;
                    if depth != 8 && depth != 16 {
                        return Err(value_err("depth must be 8 or 16"));
                    }
                }
                _ => return Err(PyTypeError::new_err(format!("unexpected keyword argument {key:?}"))),
            }
        }
        p.orientation.flip_h = flip_h;
        p.orientation.flip_v = flip_v ^ self.model.rows_bottom_up;
        p.orientation.rotation = match rotation {
            0 => Rotation::None,
            90 => Rotation::Cw90,
            180 => Rotation::Cw180,
            270 => Rotation::Cw270,
            r => return Err(value_err(format!("rotation {r}: use 0, 90, 180 or 270"))),
        };
        Ok((p, depth))
    }

    fn encode_bytes(&self, py: Python<'_>, format: Format, raw: bool, options: Option<&Bound<'_, PyDict>>) -> PyResult<Vec<u8>> {
        if raw {
            if format != Format::Tiff {
                return Err(value_err("raw frames are saved as TIFF"));
            }
            return py.detach(|| encode::raw_tiff(&self.raw)).map_err(err);
        }
        let (p, depth) = self.params(options)?;
        py.detach(|| {
            if depth == 16 {
                encode::encode16(&loupecam_isp::develop16(&self.raw, &p), format)
            } else {
                encode::encode8(&loupecam_isp::develop(&self.raw, &p), format)
            }
        })
        .map_err(err)
    }
}

fn format_named(name: &str, quality: u8) -> PyResult<Format> {
    Format::from_extension(name, quality).ok_or_else(|| value_err(format!("format {name:?}: use \"png\", \"tiff\" or \"jpeg\"")))
}

#[pymethods]
impl PyFrame {
    #[getter]
    fn width(&self) -> u32 {
        self.raw.width
    }
    #[getter]
    fn height(&self) -> u32 {
        self.raw.height
    }
    /// Significant bits per sample: 8, or 12 in the `raw12` mode.
    #[getter]
    fn bits(&self) -> u8 {
        bits(self.raw.format)
    }
    /// The camera's frame counter.
    #[getter]
    fn sequence(&self) -> u32 {
        self.raw.trailer.sequence
    }
    /// The Bayer mosaic's top-left 2×2 cell, e.g. `"RGGB"`.
    #[getter]
    fn bayer_pattern(&self) -> Option<&'static str> {
        match self.model.color {
            ColorFilter::Bayer(p) => Some(p.name()),
            ColorFilter::Mono => None,
        }
    }
    #[getter]
    fn model(&self) -> PyModel {
        PyModel(self.model)
    }

    /// The raw mosaic as a `(height, width)` array: `uint8`, or `uint16` holding
    /// `bits` significant bits. Rows are as the sensor sends them (bottom-up on the
    /// MU1803; `develop()` turns them upright). Each access makes a new copy.
    #[getter]
    fn raw<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let (w, h) = (self.raw.width as usize, self.raw.height as usize);
        Ok(match self.raw.format {
            SampleFormat::U8 => self.raw.data.clone().into_pyarray(py).reshape([h, w])?.into_any(),
            SampleFormat::U16 { .. } => {
                let v: Vec<u16> = self.raw.data.as_chunks::<2>().0.iter().map(|b| u16::from_le_bytes(*b)).collect();
                v.into_pyarray(py).reshape([h, w])?.into_any()
            }
        })
    }

    /// NumPy's array protocol: `numpy.asarray(frame)` is the raw mosaic.
    #[pyo3(signature = (dtype = None, copy = None))]
    fn __array__<'py>(&self, py: Python<'py>, dtype: Option<Bound<'py, PyAny>>, copy: Option<bool>) -> PyResult<Bound<'py, PyAny>> {
        if copy == Some(false) {
            return Err(value_err("a frame can't be viewed without copying"));
        }
        let a = self.raw(py)?;
        match dtype {
            Some(d) => a.call_method1("astype", (d,)),
            None => Ok(a),
        }
    }

    /// Develop into an RGB image: a `(height, width, 3)` array (`(height, width)` with
    /// `monochrome=True`), `uint8`, or `uint16` with `depth=16`.
    ///
    /// Defaults to the model's colour matrix and daylight white balance, bilinear
    /// demosaicing and the sRGB curve, upright. See the type stubs for every option.
    #[pyo3(signature = (**options))]
    fn develop<'py>(&self, py: Python<'py>, options: Option<&Bound<'py, PyDict>>) -> PyResult<Bound<'py, PyAny>> {
        let (p, depth) = self.params(options)?;
        let shape = |w: u32, h: u32, c: u8| if c == 1 { vec![h as usize, w as usize] } else { vec![h as usize, w as usize, c as usize] };
        Ok(if depth == 16 {
            let img = py.detach(|| loupecam_isp::develop16(&self.raw, &p));
            img.data.into_pyarray(py).reshape(shape(img.width, img.height, img.channels))?.into_any()
        } else {
            let img = py.detach(|| loupecam_isp::develop(&self.raw, &p));
            img.data.into_pyarray(py).reshape(shape(img.width, img.height, img.channels))?.into_any()
        })
    }

    /// Develop and encode as `"png"`, `"tiff"` or `"jpeg"`. With `raw=True` (TIFF
    /// only), the undeveloped mosaic, scaled to 16 bits.
    #[pyo3(signature = (format = "png", *, quality = 90, raw = false, **options))]
    fn encode<'py>(&self, py: Python<'py>, format: &str, quality: u8, raw: bool, options: Option<&Bound<'py, PyDict>>) -> PyResult<Bound<'py, PyBytes>> {
        let bytes = self.encode_bytes(py, format_named(format, quality)?, raw, options)?;
        Ok(PyBytes::new(py, &bytes))
    }

    /// Develop and save; the format comes from the extension (`.png`, `.tif`/`.tiff`,
    /// `.jpg`/`.jpeg`). Takes the same options as `encode()`.
    #[pyo3(signature = (path, *, quality = 90, raw = false, **options))]
    fn save(&self, py: Python<'_>, path: std::path::PathBuf, quality: u8, raw: bool, options: Option<&Bound<'_, PyDict>>) -> PyResult<()> {
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        let format = Format::from_extension(ext, quality).ok_or_else(|| value_err(format!("unknown image extension {ext:?} (use .png, .tif or .jpg)")))?;
        let bytes = self.encode_bytes(py, format, raw, options)?;
        py.detach(|| std::fs::write(&path, bytes))?;
        Ok(())
    }

    /// Grey-world white balance `(r, g, b)` gains. Over a neutral `region`
    /// `(x, y, width, height)`, this is "click to white balance".
    #[pyo3(signature = (region = None))]
    fn white_balance(&self, py: Python<'_>, region: Option<(u32, u32, u32, u32)>) -> (f32, f32, f32) {
        let region = self.region(region);
        let [r, g, b] = py.detach(|| self.grey_world(region));
        (r, g, b)
    }

    /// Raw statistics, for exposure decisions and histograms. `white_balance` weights
    /// the luma estimate (default: the model's daylight balance).
    #[pyo3(signature = (region = None, white_balance = None))]
    fn stats(&self, py: Python<'_>, region: Option<(u32, u32, u32, u32)>, white_balance: Option<(f32, f32, f32)>) -> PyFrameStats {
        let region = self.region(region);
        let d = self.model.daylight_wb;
        let wb = white_balance.map_or([d.r, d.g, d.b], |(r, g, b)| [r, g, b]);
        let s = py.detach(|| stats::compute(&self.raw, self.pattern(), region, 1, wb));
        PyFrameStats { mean: (s.mean[0], s.mean[1], s.mean[2]), luma: s.luma, clipped: s.clipped, histogram: s.histogram }
    }

    fn __repr__(&self) -> String {
        format!("<Frame #{} {}x{} {}-bit>", self.raw.trailer.sequence, self.raw.width, self.raw.height, bits(self.raw.format))
    }
}

/// Connected cameras of supported models.
#[pyfunction]
fn list_cameras(py: Python<'_>) -> PyResult<Vec<PyDevice>> {
    let devices = py.detach(loupecam::list).map_err(cam_err)?;
    Ok(devices.into_iter().map(|inner| PyDevice { inner }).collect())
}

/// Open a camera: the first one found (`None`), a `CameraDevice`, an index into
/// `list_cameras()`, or a device `id`.
#[pyfunction]
#[pyo3(signature = (device = None))]
fn open(py: Python<'_>, device: Option<&Bound<'_, PyAny>>) -> PyResult<PyCamera> {
    let Some(device) = device else {
        return Ok(PyCamera::new(py.detach(AnyCamera::open_first).map_err(cam_err)?));
    };
    if let Ok(d) = device.cast::<PyDevice>() {
        return d.get().open(py);
    }
    let devices = py.detach(loupecam::list).map_err(cam_err)?;
    let found = if let Ok(i) = device.extract::<usize>() {
        devices.into_iter().nth(i)
    } else {
        let id: String = device.extract()?;
        devices.into_iter().find(|d| d.id == id)
    };
    let dev = found.ok_or_else(|| err(format!("no camera {device}")))?;
    PyDevice { inner: dev }.open(py)
}

/// A simulated MU1803-HS: synthetic frames, no hardware needed.
#[pyfunction]
fn open_simulated() -> PyCamera {
    PyCamera::new(AnyCamera::simulated())
}

#[pymodule]
fn _native(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    m.add("LoupecamError", m.py().get_type::<LoupecamError>())?;
    m.add("StreamStopped", m.py().get_type::<StreamStopped>())?;
    m.add_class::<PyDevice>()?;
    m.add_class::<PyModel>()?;
    m.add_class::<PyInfo>()?;
    m.add_class::<PyGeometry>()?;
    m.add_class::<PyStreamStats>()?;
    m.add_class::<PyCamera>()?;
    m.add_class::<PyStream>()?;
    m.add_class::<PyFrame>()?;
    m.add_class::<PyFrameStats>()?;
    m.add_function(wrap_pyfunction!(list_cameras, m)?)?;
    m.add_function(wrap_pyfunction!(open, m)?)?;
    m.add_function(wrap_pyfunction!(open_simulated, m)?)?;
    Ok(())
}

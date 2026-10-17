//! Hardware H.264 encoding through a V4L2 memory-to-memory encoder, such as the
//! Raspberry Pi 4's `bcm2835-codec` (`/dev/video11`, up to 1920×1080).
//!
//! Stateful encoder API, multi-planar queues, mmap buffers: raw I420 frames go into the
//! OUTPUT queue, the H.264 byte stream comes out of the CAPTURE queue. Only the few
//! ioctls this needs are declared here; their layouts are the kernel's stable ABI
//! (`linux/videodev2.h`), with sizes checked at compile time for 64-bit targets.

use super::encoder::{Encoded, EncoderConfig, VideoEncoder};
use anyhow::{Context, bail};
use loupecam_isp::yuv::I420;
use std::fs::{File, OpenOptions};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const NAME: &str = "v4l2-m2m";

// ---- ABI ----

const fn ioc(dir: u32, nr: u32, size: usize) -> u32 {
    (dir << 30) | ((size as u32) << 16) | ((b'V' as u32) << 8) | nr
}
const fn ior<T>(nr: u32) -> u32 {
    ioc(2, nr, size_of::<T>())
}
const fn iow<T>(nr: u32) -> u32 {
    ioc(1, nr, size_of::<T>())
}
const fn iowr<T>(nr: u32) -> u32 {
    ioc(3, nr, size_of::<T>())
}

const VIDIOC_QUERYCAP: u32 = ior::<Capability>(0);
const VIDIOC_S_FMT: u32 = iowr::<Format>(5);
const VIDIOC_REQBUFS: u32 = iowr::<RequestBuffers>(8);
const VIDIOC_QUERYBUF: u32 = iowr::<Buffer>(9);
const VIDIOC_QBUF: u32 = iowr::<Buffer>(15);
const VIDIOC_DQBUF: u32 = iowr::<Buffer>(17);
const VIDIOC_STREAMON: u32 = iow::<libc::c_int>(18);
const VIDIOC_STREAMOFF: u32 = iow::<libc::c_int>(19);
const VIDIOC_S_PARM: u32 = iowr::<StreamParm>(22);
const VIDIOC_S_CTRL: u32 = iowr::<Control>(28);
const VIDIOC_ENCODER_CMD: u32 = iowr::<EncoderCmd>(77);

const CAP_VIDEO_M2M_MPLANE: u32 = 0x0000_4000;
const CAP_DEVICE_CAPS: u32 = 0x8000_0000;
const BUF_TYPE_CAPTURE_MPLANE: u32 = 9;
const BUF_TYPE_OUTPUT_MPLANE: u32 = 10;
const MEMORY_MMAP: u32 = 1;
const FIELD_NONE: u32 = 1;
const BUF_FLAG_LAST: u32 = 0x0010_0000;
const ENC_CMD_STOP: u32 = 1;

const fn fourcc(s: &[u8; 4]) -> u32 {
    s[0] as u32 | (s[1] as u32) << 8 | (s[2] as u32) << 16 | (s[3] as u32) << 24
}
const PIX_YU12: u32 = fourcc(b"YU12");
const PIX_H264: u32 = fourcc(b"H264");

// Codec controls (V4L2_CID_CODEC_BASE = V4L2_CTRL_CLASS_CODEC | 0x900).
const CID_CODEC_BASE: u32 = 0x0099_0000 | 0x900;
const CID_BITRATE: u32 = CID_CODEC_BASE + 207;
const CID_REPEAT_SEQ_HEADER: u32 = CID_CODEC_BASE + 226;
const CID_H264_I_PERIOD: u32 = CID_CODEC_BASE + 358;
const CID_H264_LEVEL: u32 = CID_CODEC_BASE + 359;
const CID_H264_PROFILE: u32 = CID_CODEC_BASE + 363;
const H264_PROFILE_HIGH: i32 = 4;
const H264_LEVEL_4_0: i32 = 11;

#[repr(C)]
struct Capability {
    driver: [u8; 16],
    card: [u8; 32],
    bus_info: [u8; 32],
    version: u32,
    capabilities: u32,
    device_caps: u32,
    reserved: [u32; 3],
}

#[repr(C, packed)]
#[derive(Clone, Copy, Default)]
struct PlanePixFormat {
    sizeimage: u32,
    bytesperline: u32,
    reserved: [u16; 6],
}

#[repr(C, packed)]
#[derive(Clone, Copy, Default)]
struct PixFormatMplane {
    width: u32,
    height: u32,
    pixelformat: u32,
    field: u32,
    colorspace: u32,
    plane_fmt: [PlanePixFormat; 8],
    num_planes: u8,
    flags: u8,
    ycbcr_enc: u8,
    quantization: u8,
    xfer_func: u8,
    reserved: [u8; 7],
}

/// `struct v4l2_format`: the union holds pointers in some members, so it is
/// pointer-aligned.
#[repr(C)]
union FormatUnion {
    pix_mp: PixFormatMplane,
    raw: [u8; 200],
    _align: *mut libc::c_void,
}

#[repr(C)]
struct Format {
    type_: u32,
    fmt: FormatUnion,
}

#[repr(C)]
#[derive(Default)]
struct RequestBuffers {
    count: u32,
    type_: u32,
    memory: u32,
    capabilities: u32,
    flags: u8,
    reserved: [u8; 3],
}

#[repr(C)]
#[derive(Clone, Copy)]
union PlaneM {
    mem_offset: u32,
    userptr: libc::c_ulong,
    fd: i32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Plane {
    bytesused: u32,
    length: u32,
    m: PlaneM,
    data_offset: u32,
    reserved: [u32; 11],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Timecode {
    type_: u32,
    flags: u32,
    frames: u8,
    seconds: u8,
    minutes: u8,
    hours: u8,
    userbits: [u8; 4],
}

#[repr(C)]
#[derive(Clone, Copy)]
union BufferM {
    offset: u32,
    userptr: libc::c_ulong,
    planes: *mut Plane,
    fd: i32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Buffer {
    index: u32,
    type_: u32,
    bytesused: u32,
    flags: u32,
    field: u32,
    timestamp: libc::timeval,
    timecode: Timecode,
    sequence: u32,
    memory: u32,
    m: BufferM,
    length: u32,
    reserved2: u32,
    request_fd: i32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Fract {
    numerator: u32,
    denominator: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct OutputParm {
    capability: u32,
    outputmode: u32,
    timeperframe: Fract,
    extendedmode: u32,
    writebuffers: u32,
    reserved: [u32; 4],
}

#[repr(C)]
union StreamParmUnion {
    output: OutputParm,
    raw: [u8; 200],
}

#[repr(C)]
struct StreamParm {
    type_: u32,
    parm: StreamParmUnion,
}

#[repr(C)]
struct Control {
    id: u32,
    value: i32,
}

#[repr(C)]
struct EncoderCmd {
    cmd: u32,
    flags: u32,
    data: [u32; 8],
}

// The kernel ABI on 64-bit Linux (what the Pi 4 runs), as `pahole` reports it.
#[cfg(target_pointer_width = "64")]
const _: () = {
    assert!(size_of::<Capability>() == 104);
    assert!(size_of::<PixFormatMplane>() == 192);
    assert!(size_of::<Format>() == 208);
    assert!(size_of::<RequestBuffers>() == 20);
    assert!(size_of::<Plane>() == 64);
    assert!(size_of::<Buffer>() == 88);
    assert!(size_of::<StreamParm>() == 204);
    assert!(size_of::<Control>() == 8);
    assert!(size_of::<EncoderCmd>() == 40);
    assert!(VIDIOC_QBUF == 0xc058_560f);
    assert!(VIDIOC_S_FMT == 0xc0d0_5605);
};

fn zeroed<T>() -> T {
    // SAFETY: only used for plain C structs, for which all-zero is valid.
    unsafe { std::mem::zeroed() }
}

/// `ioctl`, retrying on EINTR.
fn ioctl<T>(fd: &File, req: u32, arg: &mut T) -> std::io::Result<()> {
    loop {
        // SAFETY: `arg` is the struct the request expects (sizes are part of `req`).
        let r = unsafe { libc::ioctl(fd.as_raw_fd(), req as _, arg as *mut T) };
        if r >= 0 {
            return Ok(());
        }
        let e = std::io::Error::last_os_error();
        if e.kind() != std::io::ErrorKind::Interrupted {
            return Err(e);
        }
    }
}

/// An mmap'ed plane of a queue buffer.
struct Mapping {
    ptr: *mut u8,
    len: usize,
}

// SAFETY: the mapping is owned by one encoder at a time.
unsafe impl Send for Mapping {}

impl Mapping {
    fn new(fd: &File, len: usize, offset: u32) -> std::io::Result<Mapping> {
        // SAFETY: mapping a buffer the driver allocated for this fd.
        let ptr = unsafe { libc::mmap(std::ptr::null_mut(), len, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_SHARED, fd.as_raw_fd(), offset as libc::off_t) };
        if ptr == libc::MAP_FAILED {
            return Err(std::io::Error::last_os_error());
        }
        Ok(Mapping { ptr: ptr as *mut u8, len })
    }

    fn bytes(&mut self) -> &mut [u8] {
        // SAFETY: valid for `len` bytes while mapped.
        unsafe { std::slice::from_raw_parts_mut(self.ptr, self.len) }
    }
}

impl Drop for Mapping {
    fn drop(&mut self) {
        // SAFETY: unmapping what `new` mapped.
        unsafe {
            libc::munmap(self.ptr as *mut libc::c_void, self.len);
        }
    }
}

// ---- Encoder ----

pub struct M2mEncoder {
    fd: File,
    width: u32,
    height: u32,
    /// Raw-frame layout in OUTPUT buffers.
    stride: usize,
    luma_rows: usize,
    sizeimage: usize,
    output: Vec<Mapping>,
    /// OUTPUT buffers we may fill (not queued).
    free: Vec<u32>,
    capture: Vec<Mapping>,
    streaming: bool,
}

impl M2mEncoder {
    /// Find and set up a V4L2 H.264 encoder for `cfg`.
    pub fn open(cfg: &EncoderConfig) -> anyhow::Result<M2mEncoder> {
        let mut errors = Vec::new();
        for path in candidates() {
            match Self::open_device(&path, cfg) {
                Ok(e) => {
                    tracing::info!(device = %path.display(), "hardware H.264 encoder");
                    return Ok(e);
                }
                Err(e) => errors.push(format!("{}: {e:#}", path.display())),
            }
        }
        if errors.is_empty() {
            bail!("no V4L2 memory-to-memory devices");
        }
        bail!("{}", errors.join("; "))
    }

    fn open_device(path: &Path, cfg: &EncoderConfig) -> anyhow::Result<M2mEncoder> {
        let fd = OpenOptions::new().read(true).write(true).custom_flags(libc::O_NONBLOCK).open(path)?;
        let mut cap: Capability = zeroed();
        ioctl(&fd, VIDIOC_QUERYCAP, &mut cap)?;
        let caps = if cap.capabilities & CAP_DEVICE_CAPS != 0 { cap.device_caps } else { cap.capabilities };
        if caps & CAP_VIDEO_M2M_MPLANE == 0 {
            bail!("not a multi-planar memory-to-memory device");
        }
        // Raw frames in.
        let mut f = Format { type_: BUF_TYPE_OUTPUT_MPLANE, fmt: FormatUnion { raw: [0; 200] } };
        f.fmt.pix_mp = PixFormatMplane { width: cfg.width, height: cfg.height, pixelformat: PIX_YU12, field: FIELD_NONE, num_planes: 1, ..Default::default() };
        ioctl(&fd, VIDIOC_S_FMT, &mut f).context("raw format")?;
        // SAFETY: the driver filled in pix_mp.
        let out = unsafe { f.fmt.pix_mp };
        let (ow, oh, opf) = (out.width, out.height, out.pixelformat);
        if opf != PIX_YU12 || ow != cfg.width || oh != cfg.height {
            bail!("encoder won't take {}x{} I420 (offers {ow}x{oh})", cfg.width, cfg.height);
        }
        let stride = out.plane_fmt[0].bytesperline as usize;
        let sizeimage = out.plane_fmt[0].sizeimage as usize;
        // Rows of luma before chroma starts (the height, padded by some drivers).
        let luma_rows = (sizeimage * 2 / 3).checked_div(stride).unwrap_or(0).max(cfg.height as usize);
        if stride < cfg.width as usize || sizeimage < stride * luma_rows * 3 / 2 {
            bail!("unexpected raw buffer layout (stride {stride}, size {sizeimage})");
        }
        // H.264 out.
        let mut f = Format { type_: BUF_TYPE_CAPTURE_MPLANE, fmt: FormatUnion { raw: [0; 200] } };
        let mut pm = PixFormatMplane { width: cfg.width, height: cfg.height, pixelformat: PIX_H264, field: FIELD_NONE, num_planes: 1, ..Default::default() };
        pm.plane_fmt[0].sizeimage = (cfg.width * cfg.height).max(1 << 20);
        f.fmt.pix_mp = pm;
        ioctl(&fd, VIDIOC_S_FMT, &mut f).context("H.264 format")?;
        // SAFETY: the driver filled in pix_mp.
        let pf = unsafe { f.fmt.pix_mp.pixelformat };
        if pf != PIX_H264 {
            bail!("not an H.264 encoder");
        }
        // Frame rate (rate control) and encoding parameters. Not every driver supports
        // every control, so these are best effort.
        let mut parm = StreamParm { type_: BUF_TYPE_OUTPUT_MPLANE, parm: StreamParmUnion { raw: [0; 200] } };
        parm.parm.output = OutputParm {
            capability: 0,
            outputmode: 0,
            timeperframe: Fract { numerator: 1000, denominator: (cfg.fps * 1000.0).round().max(1.0) as u32 },
            extendedmode: 0,
            writebuffers: 0,
            reserved: [0; 4],
        };
        if let Err(e) = ioctl(&fd, VIDIOC_S_PARM, &mut parm) {
            tracing::debug!("encoder frame rate: {e}");
        }
        for (id, value, what) in [
            (CID_BITRATE, cfg.bitrate_bps.min(i32::MAX as u32) as i32, "bitrate"),
            (CID_H264_I_PERIOD, cfg.keyframe_interval as i32, "keyframe interval"),
            (CID_H264_PROFILE, H264_PROFILE_HIGH, "profile"),
            (CID_H264_LEVEL, H264_LEVEL_4_0, "level"),
            (CID_REPEAT_SEQ_HEADER, 1, "repeat SPS/PPS"),
        ] {
            if let Err(e) = ioctl(&fd, VIDIOC_S_CTRL, &mut Control { id, value }) {
                tracing::debug!("encoder {what}: {e}");
            }
        }
        let output = map_queue(&fd, BUF_TYPE_OUTPUT_MPLANE, 4)?;
        let capture = map_queue(&fd, BUF_TYPE_CAPTURE_MPLANE, 4)?;
        let mut enc = M2mEncoder {
            free: (0..output.len() as u32).collect(),
            fd,
            width: cfg.width,
            height: cfg.height,
            stride,
            luma_rows,
            sizeimage,
            output,
            capture,
            streaming: false,
        };
        for i in 0..enc.capture.len() as u32 {
            enc.queue_capture(i)?;
        }
        for t in [BUF_TYPE_OUTPUT_MPLANE, BUF_TYPE_CAPTURE_MPLANE] {
            let mut t = t as libc::c_int;
            ioctl(&enc.fd, VIDIOC_STREAMON, &mut t).context("starting the encoder")?;
        }
        enc.streaming = true;
        Ok(enc)
    }

    fn queue_capture(&self, index: u32) -> std::io::Result<()> {
        let mut planes = [zeroed::<Plane>()];
        planes[0].length = self.capture[index as usize].len as u32;
        let mut b: Buffer = zeroed();
        b.index = index;
        b.type_ = BUF_TYPE_CAPTURE_MPLANE;
        b.memory = MEMORY_MMAP;
        b.m.planes = planes.as_mut_ptr();
        b.length = 1;
        ioctl(&self.fd, VIDIOC_QBUF, &mut b)
    }

    /// Dequeue a buffer from `type_` without waiting. Returns the buffer and its plane.
    fn dequeue(&self, type_: u32) -> std::io::Result<Option<(Buffer, Plane)>> {
        let mut planes = [zeroed::<Plane>()];
        let mut b: Buffer = zeroed();
        b.type_ = type_;
        b.memory = MEMORY_MMAP;
        b.m.planes = planes.as_mut_ptr();
        b.length = 1;
        match ioctl(&self.fd, VIDIOC_DQBUF, &mut b) {
            Ok(()) => Ok(Some((b, planes[0]))),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
            // EPIPE: the last buffer was already dequeued (after a drain).
            Err(e) if e.raw_os_error() == Some(libc::EPIPE) => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn wait(&self, events: libc::c_short, timeout: Duration) -> std::io::Result<bool> {
        let mut p = libc::pollfd { fd: self.fd.as_raw_fd(), events, revents: 0 };
        // SAFETY: one valid pollfd.
        let r = unsafe { libc::poll(&mut p, 1, timeout.as_millis() as libc::c_int) };
        if r < 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(r > 0)
    }

    /// Collect finished H.264 output. Returns whether the last buffer (after a drain) was
    /// seen.
    fn collect(&mut self, out: &mut Vec<Encoded>) -> std::io::Result<bool> {
        while let Some((b, plane)) = self.dequeue(BUF_TYPE_CAPTURE_MPLANE)? {
            let (start, used) = (plane.data_offset as usize, plane.bytesused as usize);
            let m = &mut self.capture[b.index as usize];
            if used > start && used <= m.len {
                let time = Duration::from_secs(b.timestamp.tv_sec as u64) + Duration::from_micros(b.timestamp.tv_usec as u64);
                out.push(Encoded { annex_b: m.bytes()[start..used].to_vec(), time });
            }
            let last = b.flags & BUF_FLAG_LAST != 0;
            if last {
                return Ok(true);
            }
            self.queue_capture(b.index)?;
        }
        Ok(false)
    }

    /// Reclaim OUTPUT buffers the encoder is done with.
    fn reclaim(&mut self) -> std::io::Result<()> {
        while let Some((b, _)) = self.dequeue(BUF_TYPE_OUTPUT_MPLANE)? {
            self.free.push(b.index);
        }
        Ok(())
    }

    fn stop(&mut self) {
        if self.streaming {
            for t in [BUF_TYPE_OUTPUT_MPLANE, BUF_TYPE_CAPTURE_MPLANE] {
                let mut t = t as libc::c_int;
                let _ = ioctl(&self.fd, VIDIOC_STREAMOFF, &mut t);
            }
            self.streaming = false;
        }
    }
}

impl Drop for M2mEncoder {
    fn drop(&mut self) {
        self.stop();
        // Mappings unmap themselves; buffers are freed when the fd closes.
    }
}

impl VideoEncoder for M2mEncoder {
    fn name(&self) -> &'static str {
        NAME
    }

    fn encode(&mut self, f: &I420, time: Duration) -> anyhow::Result<Vec<Encoded>> {
        anyhow::ensure!((f.width, f.height) == (self.width, self.height), "frame size changed");
        let mut out = Vec::new();
        // A free raw buffer, waiting for one if the encoder holds them all.
        self.reclaim()?;
        while self.free.is_empty() {
            self.collect(&mut out)?;
            if !self.wait(libc::POLLOUT | libc::POLLIN, Duration::from_secs(2))? {
                bail!("hardware encoder stalled");
            }
            self.reclaim()?;
        }
        let index = self.free.pop().unwrap();
        let (w, h, stride, rows) = (f.width as usize, f.height as usize, self.stride, self.luma_rows);
        {
            let dst = self.output[index as usize].bytes();
            for y in 0..h {
                dst[y * stride..y * stride + w].copy_from_slice(&f.y()[y * w..(y + 1) * w]);
            }
            let (cw, ch, cs) = (w / 2, h / 2, stride / 2);
            let u0 = stride * rows;
            let v0 = u0 + cs * (rows / 2);
            for y in 0..ch {
                dst[u0 + y * cs..u0 + y * cs + cw].copy_from_slice(&f.u()[y * cw..(y + 1) * cw]);
                dst[v0 + y * cs..v0 + y * cs + cw].copy_from_slice(&f.v()[y * cw..(y + 1) * cw]);
            }
        }
        let mut planes = [zeroed::<Plane>()];
        planes[0].bytesused = self.sizeimage as u32;
        planes[0].length = self.output[index as usize].len as u32;
        let mut b: Buffer = zeroed();
        b.index = index;
        b.type_ = BUF_TYPE_OUTPUT_MPLANE;
        b.memory = MEMORY_MMAP;
        b.m.planes = planes.as_mut_ptr();
        b.length = 1;
        // The timestamp is copied to the matching H.264 buffer.
        b.timestamp = libc::timeval { tv_sec: time.as_secs() as _, tv_usec: time.subsec_micros() as _ };
        ioctl(&self.fd, VIDIOC_QBUF, &mut b).context("queueing a frame")?;
        self.collect(&mut out)?;
        Ok(out)
    }

    fn finish(&mut self) -> anyhow::Result<Vec<Encoded>> {
        let mut out = Vec::new();
        if !self.streaming {
            return Ok(out);
        }
        // Drain: encode what's queued, then the driver flags the last buffer.
        let mut cmd = EncoderCmd { cmd: ENC_CMD_STOP, flags: 0, data: [0; 8] };
        if ioctl(&self.fd, VIDIOC_ENCODER_CMD, &mut cmd).is_ok() {
            let deadline = std::time::Instant::now() + Duration::from_secs(3);
            while std::time::Instant::now() < deadline {
                if self.collect(&mut out)? {
                    break;
                }
                self.wait(libc::POLLIN, Duration::from_millis(200))?;
            }
        } else {
            self.collect(&mut out)?;
        }
        self.stop();
        Ok(out)
    }
}

/// Request `count` mmap buffers on a queue and map their (single) planes.
fn map_queue(fd: &File, type_: u32, count: u32) -> anyhow::Result<Vec<Mapping>> {
    let mut req = RequestBuffers { count, type_, memory: MEMORY_MMAP, ..Default::default() };
    ioctl(fd, VIDIOC_REQBUFS, &mut req).context("allocating buffers")?;
    (0..req.count)
        .map(|index| {
            let mut planes = [zeroed::<Plane>()];
            let mut b: Buffer = zeroed();
            b.index = index;
            b.type_ = type_;
            b.memory = MEMORY_MMAP;
            b.m.planes = planes.as_mut_ptr();
            b.length = 1;
            ioctl(fd, VIDIOC_QUERYBUF, &mut b)?;
            // SAFETY: the driver filled in the mmap offset.
            let offset = unsafe { planes[0].m.mem_offset };
            Ok(Mapping::new(fd, planes[0].length as usize, offset)?)
        })
        .collect()
}

/// V4L2 devices, the Raspberry Pi's encoder node first.
fn candidates() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir("/dev")
        .map(|d| d.flatten().map(|e| e.path()).filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("video"))).collect())
        .unwrap_or_default();
    v.sort_by_key(|p| (p != Path::new("/dev/video11"), p.clone()));
    v
}

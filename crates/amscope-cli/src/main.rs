use amscope::{Camera, Gain, PixelMode, RawFrame, Roi, SampleFormat, StreamConfig};
use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand, ValueEnum};
use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};

#[derive(Parser)]
#[command(version, about = "Native tool for AmScope / ToupTek USB3 microscope cameras")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// List connected cameras.
    List,
    /// Open the camera and print its identity.
    Info,
    /// Stream for a while and report frame statistics.
    Stream {
        #[command(flatten)]
        opts: StreamOpts,
        /// Seconds to stream.
        #[arg(long, default_value_t = 5.0)]
        seconds: f64,
    },
    /// Capture frames to files (PGM: 8-bit or 16-bit greyscale mosaic).
    Snap {
        #[command(flatten)]
        opts: StreamOpts,
        /// Output path. With --count > 1, `{n}` is replaced by the frame number.
        #[arg(short, long, default_value = "snap.pgm")]
        out: PathBuf,
        #[arg(long, default_value_t = 1)]
        count: u32,
        /// Frames to discard first (lets exposure settle).
        #[arg(long, default_value_t = 6)]
        skip: u32,
        #[command(flatten)]
        develop: DevelopOpts,
    },
}

/// How to turn the mosaic into an image (for .png/.jpg/.tif outputs).
#[derive(Args, Clone)]
struct DevelopOpts {
    #[arg(long, value_enum, default_value_t = DemosaicArg::Mhc)]
    demosaic: DemosaicArg,
    /// White balance gains r,g,b. Default: grey-world auto white balance.
    #[arg(long, value_parser = parse_f32x3)]
    wb: Option<[f32; 3]>,
    /// Skip colour correction (identity matrix).
    #[arg(long)]
    no_ccm: bool,
    /// Write 16 bits per channel (PNG/TIFF).
    #[arg(long)]
    sixteen: bool,
    /// Write the undeveloped mosaic as 16-bit greyscale TIFF instead.
    #[arg(long)]
    raw: bool,
    #[arg(long, default_value_t = 92)]
    jpeg_quality: u8,
}

#[derive(Copy, Clone, ValueEnum)]
enum DemosaicArg {
    Superpixel,
    Bilinear,
    Mhc,
}

fn parse_f32x3(s: &str) -> Result<[f32; 3], String> {
    let v: Vec<f32> = s.split(',').map(|p| p.trim().parse().map_err(|e| format!("{p}: {e}"))).collect::<Result<_, _>>()?;
    v.try_into().map_err(|_| "expected r,g,b".to_string())
}

#[derive(Args, Clone)]
struct StreamOpts {
    /// Resolution index (0 = largest).
    #[arg(short, long, default_value_t = 2)]
    size: usize,
    #[arg(short, long, value_enum, default_value_t = Mode::Raw8)]
    mode: Mode,
    /// Exposure in microseconds.
    #[arg(short, long, default_value_t = 20000)]
    exposure: u32,
    /// Total gain multiplier (1.0 = unity).
    #[arg(short, long, default_value_t = 1.0)]
    gain: f32,
    /// Readout speed 0 (slowest) ..= 3 (fastest).
    #[arg(long, default_value_t = 3)]
    speed: u8,
    #[arg(long)]
    binning: bool,
    /// Region of interest: x,y,width,height.
    #[arg(long, value_parser = parse_roi)]
    roi: Option<Roi>,
    /// FPGA test pattern (3, 5, 7, 9).
    #[arg(long, default_value_t = 0)]
    test_pattern: u8,
    /// Write a sensor register after starting, `ADDR=VALUE` (hex with 0x or decimal).
    /// Repeatable. For experimentation.
    #[arg(long = "sensor-reg", value_parser = parse_reg)]
    sensor_regs: Vec<(u16, u16)>,
    /// Write an FPGA register after starting, `ADDR=VALUE`. Repeatable.
    #[arg(long = "fpga-reg", value_parser = parse_reg)]
    fpga_regs: Vec<(u16, u16)>,
}

fn parse_num(s: &str) -> Result<u16, String> {
    let s = s.trim();
    match s.strip_prefix("0x") {
        Some(h) => u16::from_str_radix(h, 16),
        None => s.parse(),
    }
    .map_err(|e| format!("{s}: {e}"))
}

fn parse_reg(s: &str) -> Result<(u16, u16), String> {
    let (a, v) = s.split_once('=').ok_or("expected ADDR=VALUE")?;
    Ok((parse_num(a)?, parse_num(v)?))
}

/// Start streaming with `opts`, then apply any raw register writes.
fn start(cam: &mut Camera, opts: &StreamOpts) -> Result<amscope::FrameReceiver> {
    let rx = cam.start(&opts.config()?)?;
    for &(a, v) in &opts.sensor_regs {
        cam.write_sensor(a, v)?;
    }
    for &(a, v) in &opts.fpga_regs {
        cam.write_fpga(u8::try_from(a).context("FPGA register must be < 0x100")?, v)?;
    }
    Ok(rx)
}

#[derive(Copy, Clone, ValueEnum)]
enum Mode {
    Raw8,
    Raw12,
    Processed8,
}

fn parse_roi(s: &str) -> Result<Roi, String> {
    let v: Vec<u16> = s.split(',').map(|p| p.trim().parse().map_err(|e| format!("{p}: {e}"))).collect::<Result<_, _>>()?;
    match v[..] {
        [x, y, width, height] => Ok(Roi { x, y, width, height }),
        _ => Err("expected x,y,width,height".into()),
    }
}

impl StreamOpts {
    fn config(&self) -> Result<StreamConfig> {
        let mode = match self.mode {
            Mode::Raw8 => PixelMode::Raw8,
            Mode::Raw12 => PixelMode::Raw12,
            Mode::Processed8 => PixelMode::Processed8,
        };
        let mut c = StreamConfig::new(self.size, mode);
        c.exposure_us = self.exposure;
        c.gain = Gain::for_multiplier(self.gain)?;
        c.speed = self.speed;
        c.binning = self.binning;
        c.roi = self.roi;
        c.test_pattern = self.test_pattern;
        Ok(c)
    }
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()))
        .with_writer(std::io::stderr)
        .init();
    match Cli::parse().cmd {
        Cmd::List => {
            let devs = amscope::list()?;
            if devs.is_empty() {
                println!("no supported cameras found");
            }
            for d in devs {
                println!("{}  bus {} port {:?}  ({})", d.model.name, d.bus, d.port_chain, d.id);
            }
        }
        Cmd::Info => {
            let cam = Camera::open_first()?;
            let i = cam.info();
            println!("model           {}", i.model.name);
            println!("serial          {}", i.serial);
            if let Some((y, m, d)) = i.production_date {
                println!("produced        {y:04}-{m:02}-{d:02}");
            }
            println!("firmware        {}", i.firmware_version);
            println!("hardware        {}", i.hardware_version);
            println!("fpga            {}.{}", i.fpga_version.0, i.fpga_version.1);
            println!("sensor id       {:#06x} ({:?})", i.sensor_id, i.model.sensor);
            println!("pixel size      {} µm", i.model.pixel_size_um);
            for (n, r) in i.model.resolutions.iter().enumerate() {
                println!("size {n}          {r}");
            }
            println!("factory data    {} bytes (header {:02x?})", i.factory.payload.len(), i.factory.header);
        }
        Cmd::Stream { opts, seconds } => {
            let mut cam = Camera::open_first()?;
            let rx = start(&mut cam, &opts)?;
            let geom = cam.geometry().unwrap();
            eprintln!(
                "streaming {} {:?}, exposure {:.1} µs, sensor max {:.1} fps",
                geom.resolution,
                geom.format,
                geom.exposure_us,
                geom.max_fps(0)
            );
            let t0 = Instant::now();
            let mut n = 0u32;
            let mut first = None;
            while t0.elapsed().as_secs_f64() < seconds {
                match rx.recv_timeout(Duration::from_secs(2)) {
                    Ok(f) => {
                        n += 1;
                        first.get_or_insert_with(|| t0.elapsed());
                        if n % 10 == 1 {
                            let mean = mean(&f);
                            eprintln!(
                                "frame seq {:>5}  mean {:7.2}  stats {:?}",
                                f.trailer.sequence, mean, f.trailer.stats[0]
                            );
                        }
                    }
                    Err(e) => bail!("{e} after {n} frames"),
                }
            }
            let st = cam.stats().unwrap();
            cam.stop()?;
            println!(
                "{n} frames in {seconds:.1} s (first after {:.0} ms); rate {:.1} fps; dropped {}, discarded {}, gaps {}, errors {}, {:.1} MB/s",
                first.unwrap_or_default().as_secs_f64() * 1e3,
                st.fps,
                st.dropped,
                st.discarded,
                st.sequence_gaps,
                st.transfer_errors,
                st.bytes as f64 / seconds / 1e6
            );
        }
        Cmd::Snap { opts, out, count, skip, develop } => {
            let mut cam = Camera::open_first()?;
            let rx = start(&mut cam, &opts)?;
            for _ in 0..skip {
                rx.recv_timeout(Duration::from_secs(5))?;
            }
            for n in 0..count {
                let f = rx.recv_timeout(Duration::from_secs(5))?;
                let path = if count > 1 {
                    PathBuf::from(out.to_string_lossy().replace("{n}", &n.to_string()))
                } else {
                    out.clone()
                };
                save(&path, &f, cam.model(), &develop).with_context(|| format!("writing {}", path.display()))?;
                println!("{} ({}x{}, seq {}, mean {:.2})", path.display(), f.width, f.height, f.trailer.sequence, mean(&f));
            }
            cam.stop()?;
        }
    }
    Ok(())
}

fn mean(f: &RawFrame) -> f64 {
    match f.format {
        SampleFormat::U8 => f.data.iter().map(|&b| b as u64).sum::<u64>() as f64 / f.data.len() as f64,
        SampleFormat::U16 { .. } => {
            let n = f.data.len() / 2;
            f.data.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes([c[0], c[1]]) as u64).sum::<u64>() as f64 / n as f64
        }
    }
}

fn save(path: &std::path::Path, f: &RawFrame, model: &amscope::Model, d: &DevelopOpts) -> Result<()> {
    use amscope_isp::encode::{self, Format};
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    if ext == "pgm" {
        return write_pgm(path, f);
    }
    if d.raw {
        std::fs::write(path, encode::raw_tiff(f)?)?;
        return Ok(());
    }
    let format = match ext.as_str() {
        "png" => Format::Png,
        "tif" | "tiff" => Format::Tiff,
        "jpg" | "jpeg" => Format::Jpeg { quality: d.jpeg_quality },
        _ => bail!("unknown output extension {ext:?} (use .pgm, .png, .tif or .jpg)"),
    };
    let amscope::ColorFilter::Bayer(pattern) = model.color else { bail!("monochrome sensors not supported yet") };
    let mut p = amscope_isp::DevelopParams::new(pattern);
    p.demosaic = match d.demosaic {
        DemosaicArg::Superpixel => amscope_isp::Demosaic::Superpixel,
        DemosaicArg::Bilinear => amscope_isp::Demosaic::Bilinear,
        DemosaicArg::Mhc => amscope_isp::Demosaic::Mhc,
    };
    p.wb = d.wb.unwrap_or_else(|| amscope_isp::auto::grey_world(&amscope_isp::stats::compute(f, pattern, None, 4, [1.0; 3])));
    if !d.no_ccm {
        p.ccm = amscope::ColorMatrix::mu1803_default().0;
    }
    let t = Instant::now();
    let bytes = if d.sixteen {
        let img = amscope_isp::develop16(f, &p);
        let dt = t.elapsed();
        let b = encode::encode16(&img, format)?;
        eprintln!("developed in {:.0} ms, encoded in {:.0} ms (wb {:.2?})", dt.as_secs_f64() * 1e3, (t.elapsed() - dt).as_secs_f64() * 1e3, p.wb);
        b
    } else {
        let img = amscope_isp::develop(f, &p);
        let dt = t.elapsed();
        let b = encode::encode8(&img, format)?;
        eprintln!("developed in {:.0} ms, encoded in {:.0} ms (wb {:.2?})", dt.as_secs_f64() * 1e3, (t.elapsed() - dt).as_secs_f64() * 1e3, p.wb);
        b
    };
    std::fs::write(path, bytes)?;
    Ok(())
}

/// Netpbm greyscale: P5 with maxval 255 (8-bit) or 4095 (12-bit, big-endian samples).
fn write_pgm(path: &std::path::Path, f: &RawFrame) -> Result<()> {
    let mut w = std::io::BufWriter::new(std::fs::File::create(path)?);
    match f.format {
        SampleFormat::U8 => {
            write!(w, "P5\n{} {}\n255\n", f.width, f.height)?;
            w.write_all(&f.data)?;
        }
        SampleFormat::U16 { bits } => {
            write!(w, "P5\n{} {}\n{}\n", f.width, f.height, (1u32 << bits) - 1)?;
            let be: Vec<u8> = f.data.as_chunks::<2>().0.iter().flat_map(|c| [c[1], c[0]]).collect();
            w.write_all(&be)?;
        }
    }
    Ok(())
}

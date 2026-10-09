//! Compare the control sequences [`Session`] generates with ones captured from the
//! vendor SDK (fixtures produced by `re/make_fixture.py`).

use amscope_protocol::isp::{ColorMatrix, ToneLut, WbGains};
use amscope_protocol::model::MU1803_HS;
use amscope_protocol::request as rq;
use amscope_protocol::sensor::ar1820::Gain;
use amscope_protocol::session::{IspConfig, PixelMode, Roi};
use amscope_protocol::{Result, Session, StreamConfig, Transport};
use std::io::Write;

/// Records every transfer (with plain wValue/wIndex) and fabricates plausible replies.
struct Mock {
    key: u16,
    log: Vec<String>,
    flash: Vec<u8>,
}

impl Mock {
    fn new() -> Self {
        // Synthetic factory block: [len][5-byte header][bzip2][32-byte serial].
        let mut bz = bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::default());
        bz.write_all(b"defect map placeholder").unwrap();
        let bz = bz.finish().unwrap();
        let len = 4 + 5 + bz.len();
        let mut flash = (len as u32).to_le_bytes().to_vec();
        flash.extend_from_slice(&[0x32, 0xc7, 0x19, 0x00, 0x00]);
        flash.extend_from_slice(&bz);
        let mut serial = b"TP2401020000000000000000000TEST".to_vec();
        serial.resize(32, 0);
        flash.extend_from_slice(&serial);
        Mock { key: 0, log: Vec::new(), flash }
    }
}

impl Transport for Mock {
    fn control_in(&mut self, req: u8, value: u16, index: u16, length: u16) -> Result<Vec<u8>> {
        let (v, i) = if rq::is_scrambled(req) { (value ^ self.key, index ^ self.key) } else { (value, index) };
        self.log.push(format!("in  {req:02x} {v:04x} {i:04x} {length}"));
        Ok(match req {
            rq::SEED => {
                self.key = rq::session_key(value);
                vec![rq::ACK]
            }
            rq::READ if (v, i) == rq::READ_SENSOR_ID => vec![0x18, 0x20],
            rq::READ => vec![0x05, 0x02],
            rq::WRITE_FPGA | rq::WRITE_SENSOR => vec![rq::ACK],
            rq::STATUS => vec![0x33],
            rq::FLASH_READ => {
                let off = v as usize;
                self.flash[off..(off + length as usize).min(self.flash.len())].to_vec()
            }
            rq::FW_VERSION => b"3.5.5.20210621\0".to_vec(),
            rq::HW_VERSION => b"3.0\0".to_vec(),
            rq::READ3 => vec![0, 0, 8],
            _ => vec![0; length as usize],
        })
    }

    fn delay(&mut self, _: std::time::Duration) {}

    fn control_out(&mut self, req: u8, value: u16, index: u16, data: &[u8]) -> Result<()> {
        let hex: String = data.iter().map(|b| format!("{b:02x}")).collect();
        self.log.push(format!("out {req:02x} {value:04x} {index:04x} {hex}").trim_end().to_string());
        Ok(())
    }
}

fn fixture(name: &str) -> Vec<String> {
    let path = format!("{}/tests/fixtures/{name}.txt", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{path}: {e}"))
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
        .map(|l| l.trim_end().to_string())
        .collect()
}

fn opened() -> Session<Mock> {
    let (mut s, info) = Session::open(Mock::new(), &MU1803_HS, 0x3d97).unwrap();
    assert_eq!(info.sensor_id, 0x1820);
    assert_eq!(info.serial, "TP2401020000000000000000000TEST");
    assert_eq!(info.production_date, Some((2024, 1, 2)));
    assert_eq!(info.fpga_version, (5, 2));
    assert_eq!(info.firmware_version, "3.5.5.20210621");
    s.transport().log.clear();
    s
}

/// Compare logs, optionally masking LUT payloads (the vendor's default tone curve is
/// not reproduced).
fn assert_seq(got: &[String], want: &[String], mask_lut: bool) {
    let norm = |l: &String| {
        if mask_lut && l.starts_with("out d9") { l.split_whitespace().take(4).collect::<Vec<_>>().join(" ") } else { l.clone() }
    };
    let (g, w): (Vec<_>, Vec<_>) = (got.iter().map(norm).collect(), want.iter().map(norm).collect());
    for (i, (a, b)) in g.iter().zip(&w).enumerate() {
        assert_eq!(a, b, "transfer #{i} differs\n got: {a}\nwant: {b}");
    }
    assert_eq!(g.len(), w.len(), "sequence length differs");
}

#[test]
fn start_raw8_1228x922() {
    let mut s = opened();
    let mut cfg = StreamConfig::new(2, PixelMode::Raw8);
    cfg.gain = Gain::from_register(0x1609); // what the SDK sends for its default gain
    s.start(&cfg).unwrap();
    assert_seq(&s.transport().log, &fixture("start_raw8_1228x922"), false);
}

#[test]
fn start_raw12_4912x3684() {
    let mut s = opened();
    let mut cfg = StreamConfig::new(0, PixelMode::Raw12);
    cfg.gain = Gain::from_register(0x2c89);
    s.start(&cfg).unwrap();
    // The SDK re-reads the FPGA version at the start of this session; drop that read.
    let want: Vec<String> = fixture("start_raw12_4912x3684")
        .into_iter()
        .filter(|l| !l.starts_with("in  0a"))
        .collect();
    assert_seq(&s.transport().log, &want, false);
}

#[test]
fn start_processed8_2456x1842() {
    let mut s = opened();
    let mut cfg = StreamConfig::new(1, PixelMode::Processed8);
    cfg.gain = Gain::from_register(0x1609);
    cfg.isp = IspConfig {
        wb: WbGains { r: 392.0 / 256.0, g: 1.0, b: 438.0 / 256.0 },
        ccm: ColorMatrix::mu1803_default(),
        lut: ToneLut::identity(),
    };
    s.start(&cfg).unwrap();
    assert_seq(&s.transport().log, &fixture("start_processed8_2456x1842"), true);
}

#[test]
fn roi_and_speed_changes() {
    let mut s = opened();
    let mut cfg = StreamConfig::new(2, PixelMode::Raw8);
    cfg.gain = Gain::from_register(0x1609);
    s.start(&cfg).unwrap();

    s.transport().log.clear();
    let g = s.set_roi(Some(Roi { x: 200, y: 100, width: 1000, height: 800 })).unwrap();
    assert_eq!((g.resolution.width, g.resolution.height), (1000, 800));
    assert_seq(&s.transport().log, &fixture("roi_1000x800_at_200_100"), false);

    s.set_roi(None).unwrap();
    s.transport().log.clear();
    s.set_speed(0).unwrap();
    assert_seq(&s.transport().log, &fixture("speed0_1228x922"), false);
}

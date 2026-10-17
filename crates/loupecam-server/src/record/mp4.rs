//! A fragmented MP4 (ISO BMFF) writer for one H.264 video track.
//!
//! Fragmented so a recording that is cut short (power loss, crash) is still playable up
//! to its last complete fragment: the header (`moov`) carries no sample tables, and each
//! fragment (`moof` + `mdat`) describes its own samples. Browsers, ffmpeg and VLC all
//! play these files directly.

use std::io::{self, Write};

/// Media timescale: the usual 90 kHz video clock.
pub const TIMESCALE: u32 = 90_000;

/// One H.264 access unit, split out of an Annex-B byte stream.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AccessUnit {
    /// NAL units other than SPS, PPS and access-unit delimiters, without start codes.
    pub nals: Vec<Vec<u8>>,
    pub sps: Option<Vec<u8>>,
    pub pps: Option<Vec<u8>>,
    /// Contains an IDR slice.
    pub keyframe: bool,
}

/// Split an Annex-B byte stream (NAL units separated by `00 00 01` / `00 00 00 01`).
pub fn parse_annex_b(stream: &[u8]) -> AccessUnit {
    let mut au = AccessUnit::default();
    for nal in split_nals(stream) {
        match nal[0] & 0x1f {
            7 => au.sps = Some(nal.to_vec()),
            8 => au.pps = Some(nal.to_vec()),
            9 => {}
            t => {
                au.keyframe |= t == 5;
                au.nals.push(nal.to_vec());
            }
        }
    }
    au
}

fn split_nals(s: &[u8]) -> impl Iterator<Item = &[u8]> {
    // Start-code positions (index of the first byte after `00 00 01`).
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 3 <= s.len() {
        if s[i] == 0 && s[i + 1] == 0 && s[i + 2] == 1 {
            starts.push(i + 3);
            i += 3;
        } else {
            i += 1;
        }
    }
    let ends: Vec<usize> = starts.iter().skip(1).map(|&n| n - 3).chain(std::iter::once(s.len())).collect();
    starts.into_iter().zip(ends).filter_map(move |(a, b)| {
        // Trailing zeros belong to the next 4-byte start code (or are padding).
        let mut b = b;
        while b > a && s[b - 1] == 0 {
            b -= 1;
        }
        (b > a).then(|| &s[a..b])
    })
}

struct Sample {
    data: Vec<u8>,
    pts: u64,
    keyframe: bool,
}

/// Writes the header once the first keyframe supplies SPS/PPS, then a fragment per
/// keyframe (or sooner, see [`request_flush`](Self::request_flush)).
///
/// A sample's duration is the gap to the next one, so each fragment is written when the
/// sample after it arrives. At most the last frame is lost if the process dies.
pub struct Fmp4Writer<W: Write> {
    out: W,
    width: u16,
    height: u16,
    header: bool,
    sequence: u32,
    pending: Vec<Sample>,
    flush: bool,
    last_duration: u32,
    written: u64,
}

impl<W: Write> Fmp4Writer<W> {
    pub fn new(out: W, width: u32, height: u32) -> Self {
        Fmp4Writer {
            out,
            width: width as u16,
            height: height as u16,
            header: false,
            sequence: 0,
            pending: Vec::new(),
            flush: false,
            last_duration: TIMESCALE / 30,
            written: 0,
        }
    }

    pub fn get_ref(&self) -> &W {
        &self.out
    }

    /// Bytes written so far.
    pub fn bytes(&self) -> u64 {
        self.written
    }

    /// Start a new fragment at the next sample even if it isn't a keyframe, so what has
    /// been recorded is on disk soon (timelapses have few keyframes).
    pub fn request_flush(&mut self) {
        self.flush = true;
    }

    /// Add an access unit with presentation time `pts` (in [`TIMESCALE`] units,
    /// increasing). Units before the first keyframe are dropped.
    pub fn push(&mut self, au: &AccessUnit, pts: u64) -> io::Result<()> {
        if !self.header {
            let (Some(sps), Some(pps), true) = (&au.sps, &au.pps, au.keyframe) else { return Ok(()) };
            let header = header(self.width, self.height, sps, pps);
            self.write(&header)?;
            self.header = true;
        }
        if let Some(prev) = self.pending.last() {
            let pts = pts.max(prev.pts + 1);
            self.last_duration = (pts - prev.pts).min(u32::MAX as u64) as u32;
        }
        if (au.keyframe || self.flush) && !self.pending.is_empty() {
            self.write_fragment(self.last_duration)?;
            self.flush = false;
        }
        let mut data = Vec::with_capacity(au.nals.iter().map(|n| n.len() + 4).sum());
        for n in &au.nals {
            data.extend_from_slice(&(n.len() as u32).to_be_bytes());
            data.extend_from_slice(n);
        }
        let pts = self.pending.last().map_or(pts, |p| pts.max(p.pts + 1));
        self.pending.push(Sample { data, pts, keyframe: au.keyframe });
        Ok(())
    }

    /// Write what is pending and return the output.
    pub fn finish(mut self) -> io::Result<W> {
        if !self.pending.is_empty() {
            self.write_fragment(self.last_duration)?;
        }
        self.out.flush()?;
        Ok(self.out)
    }

    fn write(&mut self, b: &[u8]) -> io::Result<()> {
        self.out.write_all(b)?;
        self.written += b.len() as u64;
        Ok(())
    }

    /// Write all pending samples as one fragment; `last` is the final sample's duration.
    fn write_fragment(&mut self, last: u32) -> io::Result<()> {
        let samples = std::mem::take(&mut self.pending);
        self.sequence += 1;
        let base = samples[0].pts;
        let durations: Vec<u32> = samples.windows(2).map(|w| (w[1].pts - w[0].pts) as u32).chain([last]).collect();
        let build = |data_offset: u32| {
            let mut moof = Vec::new();
            bx(&mut moof, b"moof", |b| {
                full(b, b"mfhd", 0, 0, |b| u32be(b, self.sequence));
                bx(b, b"traf", |b| {
                    // default-base-is-moof
                    full(b, b"tfhd", 0, 0x02_0000, |b| u32be(b, 1));
                    full(b, b"tfdt", 1, 0, |b| b.extend_from_slice(&base.to_be_bytes()));
                    // data-offset, sample duration, size and flags present
                    full(b, b"trun", 0, 0x000701, |b| {
                        u32be(b, samples.len() as u32);
                        u32be(b, data_offset);
                        for (s, d) in samples.iter().zip(&durations) {
                            u32be(b, *d);
                            u32be(b, s.data.len() as u32);
                            // Sync samples depend on nothing; others are non-sync.
                            u32be(b, if s.keyframe { 0x0200_0000 } else { 0x0101_0000 });
                        }
                    });
                });
            });
            moof
        };
        let moof_len = build(0).len() as u32;
        let moof = build(moof_len + 8);
        let mdat_len: usize = samples.iter().map(|s| s.data.len()).sum();
        self.write(&moof)?;
        self.write(&((mdat_len + 8) as u32).to_be_bytes())?;
        self.write(b"mdat")?;
        for s in &samples {
            self.write(&s.data)?;
        }
        self.out.flush()
    }
}

/// `ftyp` + `moov` for one AVC track with no samples.
fn header(width: u16, height: u16, sps: &[u8], pps: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    bx(&mut out, b"ftyp", |b| {
        b.extend_from_slice(b"isom");
        u32be(b, 0x200);
        for brand in [b"isom", b"iso5", b"iso6", b"avc1", b"mp41"] {
            b.extend_from_slice(brand);
        }
    });
    const MATRIX: [u32; 9] = [0x0001_0000, 0, 0, 0, 0x0001_0000, 0, 0, 0, 0x4000_0000];
    bx(&mut out, b"moov", |b| {
        full(b, b"mvhd", 0, 0, |b| {
            u32be(b, 0); // creation
            u32be(b, 0); // modification
            u32be(b, 1000); // timescale
            u32be(b, 0); // duration: unknown (fragmented)
            u32be(b, 0x0001_0000); // rate
            b.extend_from_slice(&0x0100u16.to_be_bytes()); // volume
            b.extend_from_slice(&[0; 10]);
            MATRIX.iter().for_each(|&m| u32be(b, m));
            b.extend_from_slice(&[0; 24]);
            u32be(b, 2); // next track id
        });
        bx(b, b"trak", |b| {
            // enabled, in movie
            full(b, b"tkhd", 0, 3, |b| {
                u32be(b, 0);
                u32be(b, 0);
                u32be(b, 1); // track id
                u32be(b, 0);
                u32be(b, 0); // duration
                b.extend_from_slice(&[0; 8]);
                b.extend_from_slice(&[0; 4]); // layer, alternate group
                b.extend_from_slice(&[0; 4]); // volume, reserved
                MATRIX.iter().for_each(|&m| u32be(b, m));
                u32be(b, (width as u32) << 16);
                u32be(b, (height as u32) << 16);
            });
            bx(b, b"mdia", |b| {
                full(b, b"mdhd", 0, 0, |b| {
                    u32be(b, 0);
                    u32be(b, 0);
                    u32be(b, TIMESCALE);
                    u32be(b, 0);
                    b.extend_from_slice(&0x55c4u16.to_be_bytes()); // language "und"
                    b.extend_from_slice(&[0; 2]);
                });
                full(b, b"hdlr", 0, 0, |b| {
                    u32be(b, 0);
                    b.extend_from_slice(b"vide");
                    b.extend_from_slice(&[0; 12]);
                    b.extend_from_slice(b"LoupeCam\0");
                });
                bx(b, b"minf", |b| {
                    full(b, b"vmhd", 0, 1, |b| b.extend_from_slice(&[0; 8]));
                    bx(b, b"dinf", |b| {
                        full(b, b"dref", 0, 0, |b| {
                            u32be(b, 1);
                            // self-contained
                            full(b, b"url ", 0, 1, |_| {});
                        });
                    });
                    bx(b, b"stbl", |b| {
                        full(b, b"stsd", 0, 0, |b| {
                            u32be(b, 1);
                            avc1(b, width, height, sps, pps);
                        });
                        full(b, b"stts", 0, 0, |b| u32be(b, 0));
                        full(b, b"stsc", 0, 0, |b| u32be(b, 0));
                        full(b, b"stsz", 0, 0, |b| b.extend_from_slice(&[0; 8]));
                        full(b, b"stco", 0, 0, |b| u32be(b, 0));
                    });
                });
            });
        });
        bx(b, b"mvex", |b| {
            full(b, b"trex", 0, 0, |b| {
                u32be(b, 1); // track id
                u32be(b, 1); // sample description index
                b.extend_from_slice(&[0; 12]);
            });
        });
    });
    out
}

fn avc1(b: &mut Vec<u8>, width: u16, height: u16, sps: &[u8], pps: &[u8]) {
    bx(b, b"avc1", |b| {
        b.extend_from_slice(&[0; 6]);
        b.extend_from_slice(&1u16.to_be_bytes()); // data reference index
        b.extend_from_slice(&[0; 16]);
        b.extend_from_slice(&width.to_be_bytes());
        b.extend_from_slice(&height.to_be_bytes());
        u32be(b, 0x0048_0000); // 72 dpi
        u32be(b, 0x0048_0000);
        u32be(b, 0);
        b.extend_from_slice(&1u16.to_be_bytes()); // frames per sample
        let mut name = [0u8; 32];
        name[0] = 8;
        name[1..9].copy_from_slice(b"LoupeCam");
        b.extend_from_slice(&name);
        b.extend_from_slice(&0x18u16.to_be_bytes()); // depth
        b.extend_from_slice(&(-1i16).to_be_bytes());
        bx(b, b"avcC", |b| {
            b.push(1);
            b.extend_from_slice(&sps[1..4]); // profile, compatibility, level
            b.push(0xff); // 4-byte NAL lengths
            b.push(0xe1); // one SPS
            b.extend_from_slice(&(sps.len() as u16).to_be_bytes());
            b.extend_from_slice(sps);
            b.push(1);
            b.extend_from_slice(&(pps.len() as u16).to_be_bytes());
            b.extend_from_slice(pps);
            if matches!(sps[1], 100 | 110 | 122 | 144) {
                // High profiles: 4:2:0, 8-bit, no SPS extensions.
                b.extend_from_slice(&[0xfc | 1, 0xf8, 0xf8, 0]);
            }
        });
        // BT.709 primaries, transfer and matrix, limited range (matches rgb_to_i420).
        bx(b, b"colr", |b| {
            b.extend_from_slice(b"nclx");
            for v in [1u16, 1, 1] {
                b.extend_from_slice(&v.to_be_bytes());
            }
            b.push(0);
        });
    });
}

fn u32be(b: &mut Vec<u8>, v: u32) {
    b.extend_from_slice(&v.to_be_bytes());
}

fn bx(out: &mut Vec<u8>, kind: &[u8; 4], body: impl FnOnce(&mut Vec<u8>)) {
    let start = out.len();
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(kind);
    body(out);
    let len = (out.len() - start) as u32;
    out[start..start + 4].copy_from_slice(&len.to_be_bytes());
}

fn full(out: &mut Vec<u8>, kind: &[u8; 4], version: u8, flags: u32, body: impl FnOnce(&mut Vec<u8>)) {
    bx(out, kind, |b| {
        u32be(b, (version as u32) << 24 | flags);
        body(b);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_annex_b() {
        let s = [0, 0, 0, 1, 0x67, 1, 2, 0, 0, 1, 0x68, 3, 0, 0, 0, 1, 0x09, 0xf0, 0, 0, 1, 0x65, 4, 0, 0, 3, 0, 0, 0];
        let au = parse_annex_b(&s);
        assert_eq!(au.sps.as_deref(), Some(&[0x67, 1, 2][..]));
        assert_eq!(au.pps.as_deref(), Some(&[0x68, 3][..]));
        // Emulation prevention (00 00 03) stays; trailing zeros go.
        assert_eq!(au.nals, vec![vec![0x65, 4, 0, 0, 3]]);
        assert!(au.keyframe);
        assert!(!parse_annex_b(&[0, 0, 1, 0x41, 9]).keyframe);
    }

    /// Box types at the top level of `b`.
    fn top_level(b: &[u8]) -> Vec<String> {
        let mut v = Vec::new();
        let mut i = 0;
        while i + 8 <= b.len() {
            let len = u32::from_be_bytes(b[i..i + 4].try_into().unwrap()) as usize;
            v.push(String::from_utf8_lossy(&b[i + 4..i + 8]).into_owned());
            assert!(len >= 8 && i + len <= b.len(), "bad box length at {i}");
            i += len;
        }
        assert_eq!(i, b.len());
        v
    }

    #[test]
    fn fragments_at_keyframes_and_on_request() {
        let key = AccessUnit { nals: vec![vec![0x65, 1]], sps: Some(vec![0x67, 66, 0, 30]), pps: Some(vec![0x68, 1]), keyframe: true };
        let p = AccessUnit { nals: vec![vec![0x41, 2]], ..Default::default() };
        let mut w = Fmp4Writer::new(Vec::new(), 64, 48);
        // A P-frame before any keyframe is dropped.
        w.push(&p, 0).unwrap();
        assert_eq!(w.bytes(), 0);
        w.push(&key, 0).unwrap();
        w.push(&p, 3000).unwrap();
        w.push(&key, 6000).unwrap(); // closes fragment 1 (2 samples)
        w.request_flush();
        w.push(&p, 9000).unwrap(); // closes fragment 2 (1 sample)
        let out = w.finish().unwrap(); // fragment 3
        assert_eq!(top_level(&out), ["ftyp", "moov", "moof", "mdat", "moof", "mdat", "moof", "mdat"]);
    }
}

//! Bulk image stream decoding.
//!
//! The camera streams frames on bulk endpoint 0x81. Each frame is the pixel data
//! (`width × height × bytes_per_sample`) followed by a 52-byte [`Trailer`], and is
//! terminated by a short USB transfer. Right after stream start the camera may emit
//! a few short status transfers and partial frames. [`FrameAssembler`] discards these
//! and resynchronises on the next short transfer.

/// Size of the per-frame trailer.
pub const TRAILER_LEN: usize = 52;

/// Per-frame metadata appended by the FPGA.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Trailer {
    /// Per-channel sums over the two statistics windows: `[window][R, G, B]`
    /// (channel order not yet verified).
    pub stats: [[u64; 3]; 2],
    /// Frame counter, incremented per frame since stream start.
    pub sequence: u32,
}

impl Trailer {
    pub fn parse(b: &[u8; TRAILER_LEN]) -> Self {
        let u64_at = |i: usize| u64::from_le_bytes(b[i * 8..i * 8 + 8].try_into().unwrap());
        Trailer {
            stats: [
                [u64_at(0), u64_at(1), u64_at(2)],
                [u64_at(3), u64_at(4), u64_at(5)],
            ],
            sequence: u32::from_le_bytes(b[48..52].try_into().unwrap()),
        }
    }
}

/// Pixel sample layout of a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleFormat {
    /// 8 bits per sample.
    U8,
    /// 16-bit little-endian samples holding `bits` significant (low) bits.
    U16 { bits: u8 },
}

impl SampleFormat {
    pub fn bytes_per_sample(self) -> usize {
        match self {
            SampleFormat::U8 => 1,
            SampleFormat::U16 { .. } => 2,
        }
    }
}

/// A complete frame as delivered by the camera (still mosaic-domain).
#[derive(Clone)]
pub struct RawFrame {
    pub width: u32,
    pub height: u32,
    pub format: SampleFormat,
    /// Pixel data, row-major, no padding.
    pub data: Vec<u8>,
    pub trailer: Trailer,
}

impl std::fmt::Debug for RawFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RawFrame")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("format", &self.format)
            .field("bytes", &self.data.len())
            .field("trailer", &self.trailer)
            .finish()
    }
}

/// What happened to a transfer fed to the assembler.
#[derive(Debug, PartialEq, Eq)]
pub enum Feed {
    /// More data is needed.
    Pending,
    /// A frame completed; take it with [`FrameAssembler::take`].
    Frame,
    /// A short transfer ended a run of the wrong length; it was dropped.
    Discarded { bytes: usize },
}

/// Reassembles frames from bulk transfers.
pub struct FrameAssembler {
    width: u32,
    height: u32,
    format: SampleFormat,
    expected: usize,
    buf: Vec<u8>,
    /// True while skipping data after an overrun, until the next short transfer.
    resync: bool,
    ready: Option<RawFrame>,
}

impl FrameAssembler {
    pub fn new(width: u32, height: u32, format: SampleFormat) -> Self {
        let expected = width as usize * height as usize * format.bytes_per_sample() + TRAILER_LEN;
        FrameAssembler {
            width,
            height,
            format,
            expected,
            buf: Vec::with_capacity(expected),
            resync: false,
            ready: None,
        }
    }

    /// Total bytes per frame on the wire, including the trailer.
    pub fn frame_bytes(&self) -> usize {
        self.expected
    }

    /// Feed one completed transfer. `requested` is the transfer's requested length:
    /// a transfer shorter than that ends a frame.
    pub fn feed(&mut self, data: &[u8], requested: usize) -> Feed {
        let short = data.len() < requested;
        if !self.resync {
            if self.buf.len() + data.len() > self.expected {
                self.resync = true;
                self.buf.clear();
            } else {
                self.buf.extend_from_slice(data);
            }
        }
        if !short {
            return Feed::Pending;
        }
        if self.resync {
            self.resync = false;
            return Feed::Discarded { bytes: data.len() };
        }
        if self.buf.len() != self.expected {
            let bytes = self.buf.len();
            self.buf.clear();
            return Feed::Discarded { bytes };
        }
        let mut data = std::mem::replace(&mut self.buf, Vec::with_capacity(self.expected));
        let trailer_bytes: [u8; TRAILER_LEN] = data[data.len() - TRAILER_LEN..].try_into().unwrap();
        data.truncate(data.len() - TRAILER_LEN);
        self.ready = Some(RawFrame {
            width: self.width,
            height: self.height,
            format: self.format,
            data,
            trailer: Trailer::parse(&trailer_bytes),
        });
        Feed::Frame
    }

    /// Take the frame completed by the last [`feed`](Self::feed) that returned
    /// [`Feed::Frame`].
    pub fn take(&mut self) -> Option<RawFrame> {
        self.ready.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trailer(seq: u32) -> Vec<u8> {
        let mut t = Vec::new();
        for v in [1u64, 2, 3, 1, 2, 3] {
            t.extend_from_slice(&v.to_le_bytes());
        }
        t.extend_from_slice(&seq.to_le_bytes());
        t
    }

    #[test]
    fn assembles_and_resyncs() {
        let (w, h) = (64u32, 4u32);
        let mut a = FrameAssembler::new(w, h, SampleFormat::U8);
        let xfer = 128;
        // Start-up noise: a 4-byte status transfer, then a partial frame.
        assert_eq!(a.feed(&[0; 4], xfer), Feed::Discarded { bytes: 4 });
        assert_eq!(a.feed(&[0; 128], xfer), Feed::Pending);
        assert_eq!(a.feed(&[0; 10], xfer), Feed::Discarded { bytes: 138 });
        // A full frame: 256 pixel bytes + 52 trailer = 308 = 128 + 128 + 52.
        let mut frame: Vec<u8> = (0..256).map(|i| i as u8).collect();
        frame.extend(trailer(7));
        assert_eq!(a.feed(&frame[..128], xfer), Feed::Pending);
        assert_eq!(a.feed(&frame[128..256], xfer), Feed::Pending);
        assert_eq!(a.feed(&frame[256..], xfer), Feed::Frame);
        let f = a.take().unwrap();
        assert_eq!(f.data.len(), 256);
        assert_eq!(f.data[255], 255);
        assert_eq!(f.trailer.sequence, 7);
        assert_eq!(f.trailer.stats[1], [1, 2, 3]);
        // Overrun: too much data without a short transfer is dropped up to the next short.
        for _ in 0..3 {
            a.feed(&[0; 128], xfer);
        }
        assert_eq!(a.feed(&[0; 20], xfer), Feed::Discarded { bytes: 20 });
    }

    #[test]
    fn parses_captured_trailer() {
        let hex = "deae0300000000003f76050000000000157906000000000\
                   0deae0300000000003f760500000000001579060000000000\
                   08000000";
        let b: Vec<u8> = (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap()).collect();
        let t = Trailer::parse(&b.try_into().unwrap());
        assert_eq!(t.sequence, 8);
        assert_eq!(t.stats[0], [0x3aede, 0x5763f, 0x67915]);
        assert_eq!(t.stats[0], t.stats[1]);
    }
}

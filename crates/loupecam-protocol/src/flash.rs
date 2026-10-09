//! The camera's factory data block, read with [`crate::request::FLASH_READ`].
//!
//! Layout:
//!
//! ```text
//! [u32 LE len] [5-byte header] [bzip2 stream …] [32-byte NUL-padded ASCII serial]
//! └──────────────── len bytes ────────────────┘
//! ```
//!
//! The serial number encodes the production date (`TPyyMMdd…`). The bzip2 payload looks
//! like a factory defect-pixel map. Its format is not decoded yet, so it is exposed
//! decompressed but uninterpreted.

use crate::{Error, Result};
use std::io::Read;

/// Bytes of serial number after the length-prefixed body.
pub const SERIAL_LEN: usize = 32;

#[derive(Debug, Clone)]
pub struct FactoryData {
    pub serial: String,
    /// Header bytes between the length and the bzip2 stream (meaning unknown).
    pub header: [u8; 5],
    /// Decompressed payload (believed to be a defect-pixel map).
    pub payload: Vec<u8>,
}

impl FactoryData {
    /// Total bytes to read given the leading length word.
    pub fn total_len(len_word: u32) -> usize {
        len_word as usize + SERIAL_LEN
    }

    pub fn parse(blob: &[u8]) -> Result<Self> {
        if blob.len() < 4 + 5 + SERIAL_LEN {
            return Err(Error::Flash(format!("block too short ({} bytes)", blob.len())));
        }
        let len = u32::from_le_bytes(blob[..4].try_into().unwrap()) as usize;
        if blob.len() < len + SERIAL_LEN {
            return Err(Error::Flash(format!("block truncated: have {}, need {}", blob.len(), len + SERIAL_LEN)));
        }
        let header: [u8; 5] = blob[4..9].try_into().unwrap();
        let mut payload = Vec::new();
        bzip2::read::BzDecoder::new(&blob[9..len])
            .read_to_end(&mut payload)
            .map_err(|e| Error::Flash(format!("bzip2: {e}")))?;
        let serial_bytes = &blob[len..len + SERIAL_LEN];
        let end = serial_bytes.iter().position(|&b| b == 0).unwrap_or(SERIAL_LEN);
        let serial = String::from_utf8_lossy(&serial_bytes[..end]).into_owned();
        Ok(FactoryData { serial, header, payload })
    }

    /// Production date `(year, month, day)` from a `TPyyMMdd…` serial.
    pub fn production_date(&self) -> Option<(u16, u8, u8)> {
        let s = self.serial.strip_prefix("TP")?;
        let n = |r: std::ops::Range<usize>| s.get(r)?.parse::<u16>().ok();
        let (y, m, d) = (n(0..2)?, n(2..4)?, n(4..6)?);
        ((1..=12).contains(&m) && (1..=31).contains(&d)).then_some((2000 + y, m as u8, d as u8))
    }
}

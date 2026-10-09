//! Vendor request codes and the per-session scrambling of `wValue`/`wIndex`.

/// Seed exchange. IN, `wValue` = random seed, 2-byte read, the camera answers `[0x08]`.
/// Sets the session's scrambling key (see [`session_key`]).
pub const SEED: u8 = 0x16;
/// Enable. OUT, `wIndex` = 0x0f, `wValue` = 3 to enable, 0 to disable. Sent after the
/// seed and at stream start; disabled (twice) at stop. It gates the sensor itself: after
/// enabling, sensor I²C requests ([`WRITE_SENSOR`], [`READ3`]) fail with `09` for roughly
/// the first 140 ms, so it most likely switches sensor power or clock.
pub const TRANSFER: u8 = 0x01;
/// Scrambled 16-bit read. IN, 2 bytes, big-endian result. See [`READ_SENSOR_ID`] and
/// [`READ_FPGA_VERSION`] for the known addresses.
pub const READ: u8 = 0x0a;
/// Scrambled FPGA register write, sent as an IN request: `wValue` = value,
/// `wIndex` = `register << 8`. The camera answers `[0x08]`.
pub const WRITE_FPGA: u8 = 0x0b;
/// Unscrambled 3-byte read, `wIndex` = address; the last byte is the ack (`08`, or `09`
/// when the sensor does not answer). Probably a sensor register read. The vendor SDK
/// issues it at open and at stream start.
pub const READ3: u8 = 0x0c;
/// Scrambled sensor (I²C) register write, sent as an IN request: `wValue` = value,
/// `wIndex` = 16-bit register address. The camera answers `[0x08]`.
pub const WRITE_SENSOR: u8 = 0x0d;
/// 1-byte status read. Purpose unknown (answers `0x33`).
pub const STATUS: u8 = 0x10;
/// 2-byte read issued after stopping a stream. Purpose unknown (answers `0000`).
pub const STOPPED: u8 = 0x17;
/// Firmware version string (NUL-terminated ASCII, up to 16 bytes).
pub const FW_VERSION: u8 = 0x1e;
/// Hardware version string (NUL-terminated ASCII, up to 16 bytes).
pub const HW_VERSION: u8 = 0x1f;
/// Flash/EEPROM read. IN, `wValue` = byte offset, up to 4096 bytes per request.
pub const FLASH_READ: u8 = 0x20;
/// FPGA LUT upload. OUT, 2048 bytes (1024 × u16 LE), `wIndex` = 0x2200 + 0x200 × chunk.
pub const LUT_UPLOAD: u8 = 0xd9;
/// FPGA statistics-window setup. OUT, 16 bytes = four `[0x00, reg, u16 LE]` records.
pub const STATS_WINDOW: u8 = 0xda;

/// Acknowledgement byte returned by register writes and the seed exchange.
pub const ACK: u8 = 0x08;

/// Plain (unscrambled) [`READ`] address of the sensor chip ID.
pub const READ_SENSOR_ID: (u16, u16) = (0x0000, 0xffff);
/// Plain (unscrambled) [`READ`] address of the FPGA version.
pub const READ_FPGA_VERSION: (u16, u16) = (0x0000, 0xfeff);

/// Requests whose `wValue` and `wIndex` are XORed with the session key.
pub fn is_scrambled(request: u8) -> bool {
    matches!(request, READ | WRITE_FPGA | WRITE_SENSOR)
}

/// The key derived from a session seed: the seed rotated right by 4 bits.
///
/// Every later scrambled request has `wValue ^ key` and `wIndex ^ key` on the wire.
pub fn session_key(seed: u16) -> u16 {
    seed.rotate_right(4)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two captured sessions: seed and the on-wire `wValue`/`wIndex` of the sensor-ID read.
    #[test]
    fn key_matches_captures() {
        for (seed, wire_v, wire_i) in [(0x3d97u16, 0x73d9u16, 0x8c26u16), (0xee4e, 0xeee4, 0x111b)] {
            let k = session_key(seed);
            assert_eq!((wire_v ^ k, wire_i ^ k), READ_SENSOR_ID);
        }
    }
}

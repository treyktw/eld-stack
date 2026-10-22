//! J1939 parameter validity + little-endian readers.
//!
//! SAE J1939-71 reserves the top of every parameter range, and the reserved
//! band is far wider than the single "all ones" value:
//!
//! | Width  | Valid measurement      | Reserved            | Error               | Not available       |
//! |--------|------------------------|---------------------|---------------------|---------------------|
//! | 1 byte | `0x00..=0xFA`          | `0xFB..=0xFD`       | `0xFE`              | `0xFF`              |
//! | 2 byte | `0x0000..=0xFAFF`      | `0xFB00..=0xFDFF`   | `0xFE00..=0xFEFF`   | `0xFF00..=0xFFFF`   |
//! | 4 byte | `0x0000_0000..=0xFAFF_FFFF` | `0xFB00_0000..=0xFDFF_FFFF` | `0xFE00_0000..=0xFEFF_FFFF` | `0xFF00_0000..=0xFFFF_FFFF` |
//!
//! This previously rejected only the exact top values — `0xFF`, `0xFFFF`,
//! `0xFFFF_FFFF`, and one error value each. Everything else in the reserved
//! band was treated as a measurement. A four-byte odometer reading of
//! `0xFF00_0001` is "not available" per the spec, but was accepted and scaled
//! into roughly 536 million kilometres. Engine hours behaved the same way.
//!
//! For an ELD that matters more than it looks: a fabricated odometer or
//! engine-hours value does not announce itself as wrong, it just lands in the
//! record. Absent has to stay absent.

/// Highest valid value for a 1-byte parameter.
const MAX_VALID_U8: u8 = 0xFA;
/// Highest valid value for a 2-byte parameter.
const MAX_VALID_U16: u16 = 0xFAFF;
/// Highest valid value for a 4-byte parameter.
const MAX_VALID_U32: u32 = 0xFAFF_FFFF;

/// Read a 1-byte SPN at `i`. `None` unless it is a real measurement.
pub fn read_u8(data: &[u8], i: usize) -> Option<u8> {
    match *data.get(i)? {
        v if v <= MAX_VALID_U8 => Some(v),
        _ => None,
    }
}

/// Read a little-endian 2-byte SPN at `i..i+2`.
pub fn read_u16_le(data: &[u8], i: usize) -> Option<u16> {
    let lo = *data.get(i)?;
    let hi = *data.get(i + 1)?;
    match u16::from_le_bytes([lo, hi]) {
        v if v <= MAX_VALID_U16 => Some(v),
        _ => None,
    }
}

/// Read a little-endian 4-byte SPN at `i..i+4`.
pub fn read_u32_le(data: &[u8], i: usize) -> Option<u32> {
    let b = data.get(i..i + 4)?;
    match u32::from_le_bytes([b[0], b[1], b[2], b[3]]) {
        v if v <= MAX_VALID_U32 => Some(v),
        _ => None,
    }
}

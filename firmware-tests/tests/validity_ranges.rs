//! J1939 parameter validity ranges.
//!
//! SAE J1939-71 reserves the top of every parameter range, not just the single
//! all-ones value. The previous implementation rejected only `0xFF`, `0xFFFF`,
//! `0xFFFF_FFFF` and one error value each, so a four-byte odometer of
//! `0xFF00_0001` — "not available" per the spec — was accepted and scaled into
//! hundreds of millions of kilometres.
//!
//! An ELD records odometer and engine hours as regulatory facts. A fabricated
//! value does not look wrong, it just lands in the record.

use eld_decoders_test::validity::{read_u16_le, read_u32_le, read_u8};

// ---- 1 byte: valid 0x00..=0xFA -------------------------------------------

#[test]
fn u8_accepts_the_whole_measurement_range() {
    assert_eq!(read_u8(&[0x00], 0), Some(0x00));
    assert_eq!(read_u8(&[0x7F], 0), Some(0x7F));
    assert_eq!(read_u8(&[0xFA], 0), Some(0xFA));
}

#[test]
fn u8_rejects_reserved_error_and_not_available() {
    for raw in [0xFBu8, 0xFC, 0xFD, 0xFE, 0xFF] {
        assert_eq!(read_u8(&[raw], 0), None, "0x{raw:02X} is not a measurement");
    }
}

// ---- 2 byte: valid 0x0000..=0xFAFF ---------------------------------------

#[test]
fn u16_accepts_the_whole_measurement_range() {
    assert_eq!(read_u16_le(&[0x00, 0x00], 0), Some(0x0000));
    assert_eq!(read_u16_le(&[0xFF, 0xFA], 0), Some(0xFAFF));
}

#[test]
fn u16_rejects_the_reserved_band_not_just_the_top_value() {
    // 0xFB00 is the first reserved value. Little-endian: lo, hi.
    assert_eq!(read_u16_le(&[0x00, 0xFB], 0), None);
    assert_eq!(read_u16_le(&[0x00, 0xFE], 0), None, "error range start");
    assert_eq!(read_u16_le(&[0xFF, 0xFE], 0), None, "error range end");
    assert_eq!(read_u16_le(&[0x00, 0xFF], 0), None, "not-available start");
    assert_eq!(read_u16_le(&[0xFF, 0xFF], 0), None, "not-available end");
}

// ---- 4 byte: valid 0x0000_0000..=0xFAFF_FFFF -----------------------------

#[test]
fn u32_accepts_the_whole_measurement_range() {
    assert_eq!(read_u32_le(&[0x00, 0x00, 0x00, 0x00], 0), Some(0));
    assert_eq!(read_u32_le(&[0xFF, 0xFF, 0xFF, 0xFA], 0), Some(0xFAFF_FFFF));
}

#[test]
fn u32_rejects_a_not_available_odometer_that_is_not_all_ones() {
    // The regression this whole file exists for. 0xFF00_0001 scaled at
    // 0.125 km/bit is about 534 million km — recorded as a real odometer.
    assert_eq!(read_u32_le(&[0x01, 0x00, 0x00, 0xFF], 0), None);
    // And the same shape for engine hours at 0.05 h/bit.
    assert_eq!(read_u32_le(&[0x2A, 0x00, 0x00, 0xFE], 0), None);
}

#[test]
fn u32_rejects_every_reserved_band() {
    for high in [0xFBu8, 0xFC, 0xFD, 0xFE, 0xFF] {
        assert_eq!(
            read_u32_le(&[0x00, 0x00, 0x00, high], 0),
            None,
            "high byte 0x{high:02X} is not a measurement"
        );
    }
}

// ---- bounds ---------------------------------------------------------------

#[test]
fn short_payloads_yield_none_rather_than_panicking() {
    assert_eq!(read_u8(&[], 0), None);
    assert_eq!(read_u16_le(&[0x01], 0), None);
    assert_eq!(read_u32_le(&[0x01, 0x02, 0x03], 0), None);
    // Offset past the end of an otherwise full frame.
    assert_eq!(read_u32_le(&[0u8; 8], 5), None);
}

#[test]
fn a_full_frame_reads_at_its_last_valid_offset() {
    let frame = [0u8; 8];
    assert_eq!(read_u32_le(&frame, 4), Some(0));
    assert_eq!(read_u16_le(&frame, 6), Some(0));
    assert_eq!(read_u8(&frame, 7), Some(0));
}

// ---- the values these ranges protect --------------------------------------

#[test]
fn a_plausible_odometer_still_decodes() {
    // SPN 245 at 0.125 km/bit. 800_000 raw = 100_000 km.
    let raw: u32 = 800_000;
    let bytes = raw.to_le_bytes();
    let frame = [0, 0, 0, 0, bytes[0], bytes[1], bytes[2], bytes[3]];
    let value = read_u32_le(&frame, 4).expect("valid measurement");
    assert!(((value as f32 * 0.125) - 100_000.0).abs() < 0.001);
}

#[test]
fn a_plausible_engine_hours_still_decodes() {
    // SPN 247 at 0.05 h/bit. 200_000 raw = 10_000 hours.
    let raw: u32 = 200_000;
    let bytes = raw.to_le_bytes();
    let frame = [bytes[0], bytes[1], bytes[2], bytes[3], 0, 0, 0, 0];
    let value = read_u32_le(&frame, 0).expect("valid measurement");
    assert!(((value as f32 * 0.05) - 10_000.0).abs() < 0.01);
}

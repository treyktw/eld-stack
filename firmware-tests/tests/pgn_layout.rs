//! SAE J1939-71 byte layouts, pinned.
//!
//! These encode where each SPN sits inside its PGN. They exist because the
//! Arduino sketch this firmware was ported from read PGN 0xFEE0 at byte offset
//! 0 and reported it as "odometer" — but offset 0 is SPN 244 *Trip* Distance.
//! Total Vehicle Distance (SPN 245) is at bytes 5-8, offset 4. The two are
//! numerically plausible and differ silently, which is the worst kind of wrong
//! for a regulatory odometer.

use eld_decoders_test::decoder::{decode, J1939Signal};
use eld_decoders_test::id::J1939Id;
use eld_decoders_test::pgn;

fn id_for(pgn_value: u32) -> J1939Id {
    J1939Id {
        pgn: pgn_value,
        priority: 6,
        source: 0x00,
        destination: None,
    }
}

fn le32(value: u32) -> [u8; 4] {
    value.to_le_bytes()
}

#[test]
fn fee0_total_distance_is_at_offset_4_not_0() {
    // Trip distance in bytes 1-4, total in bytes 5-8. Deliberately different
    // values so reading the wrong field cannot accidentally pass.
    let trip = le32(8_000); // 1 000 km at 0.125 km/bit
    let total = le32(800_000); // 100 000 km
    let frame = [
        trip[0], trip[1], trip[2], trip[3], total[0], total[1], total[2], total[3],
    ];

    match decode(&id_for(pgn::VEHICLE_DISTANCE), &frame) {
        J1939Signal::Odometer { km, spn, .. } => {
            assert_eq!(spn, 245, "must decode Total Vehicle Distance");
            assert!(
                (km - 100_000.0).abs() < 0.001,
                "read {km} km — offset 0 would give 1000 km"
            );
        }
        other => panic!("expected odometer, got {other:?}"),
    }
}

#[test]
fn fec1_hires_total_distance_is_at_offset_0() {
    // SPN 917 total in bytes 1-4 at 5 m/bit; SPN 918 trip in bytes 5-8.
    let total = le32(20_000_000); // 100 000 km
    let trip = le32(200_000);
    let frame = [
        total[0], total[1], total[2], total[3], trip[0], trip[1], trip[2], trip[3],
    ];

    match decode(&id_for(pgn::HIRES_VEHICLE_DISTANCE), &frame) {
        J1939Signal::Odometer {
            km, spn, priority, ..
        } => {
            assert_eq!(spn, 917);
            assert!((km - 100_000.0).abs() < 0.01, "read {km} km");
            assert!(priority > 10, "hi-res must outrank the 0.125 km/bit source");
        }
        other => panic!("expected odometer, got {other:?}"),
    }
}

#[test]
fn eec1_engine_speed_is_at_offset_3() {
    // SPN 190, bytes 4-5, 0.125 rpm/bit. 12 000 raw = 1 500 rpm.
    let raw: u16 = 12_000;
    let b = raw.to_le_bytes();
    let frame = [0xAA, 0xAA, 0xAA, b[0], b[1], 0xAA, 0xAA, 0xAA];

    match decode(&id_for(pgn::EEC1), &frame) {
        J1939Signal::EngineRpm { rpm, spn, .. } => {
            assert_eq!(spn, 190);
            assert!((rpm - 1_500.0).abs() < 0.001, "read {rpm} rpm");
        }
        other => panic!("expected rpm, got {other:?}"),
    }
}

#[test]
fn ccvs1_wheel_speed_is_at_offset_1() {
    // SPN 84, bytes 2-3, 1/256 km/h per bit. 25 600 raw = 100 km/h.
    let raw: u16 = 25_600;
    let b = raw.to_le_bytes();
    let frame = [0xAA, b[0], b[1], 0xAA, 0xAA, 0xAA, 0xAA, 0xAA];

    match decode(&id_for(pgn::CCVS1), &frame) {
        J1939Signal::WheelSpeed { kph, spn, .. } => {
            assert_eq!(spn, 84);
            assert!((kph - 100.0).abs() < 0.001, "read {kph} km/h");
        }
        other => panic!("expected wheel speed, got {other:?}"),
    }
}

#[test]
fn dash_display_fuel_level_is_at_offset_1() {
    // SPN 96, byte 2, 0.4 %/bit. 200 raw = 80 %.
    let frame = [0xAA, 200, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA];

    match decode(&id_for(pgn::DASH_DISPLAY), &frame) {
        J1939Signal::FuelLevel { percent, spn, .. } => {
            assert_eq!(spn, 96);
            assert!((percent - 80.0).abs() < 0.01, "read {percent}%");
        }
        other => panic!("expected fuel level, got {other:?}"),
    }
}

#[test]
fn engine_hours_is_at_offset_0() {
    // SPN 247, bytes 1-4, 0.05 h/bit. 200 000 raw = 10 000 h.
    let hours = le32(200_000);
    let revolutions = le32(123_456); // SPN 249, bytes 5-8
    let frame = [
        hours[0],
        hours[1],
        hours[2],
        hours[3],
        revolutions[0],
        revolutions[1],
        revolutions[2],
        revolutions[3],
    ];

    match decode(&id_for(pgn::ENGINE_HOURS), &frame) {
        J1939Signal::EngineHours { hours, spn, .. } => {
            assert_eq!(spn, 247);
            assert!((hours - 10_000.0).abs() < 0.01, "read {hours} h");
        }
        other => panic!("expected engine hours, got {other:?}"),
    }
}

#[test]
fn miles_conversion_uses_the_exact_statute_mile() {
    let total = le32(800_000); // 100 000 km
    let frame = [0, 0, 0, 0, total[0], total[1], total[2], total[3]];

    match decode(&id_for(pgn::VEHICLE_DISTANCE), &frame) {
        J1939Signal::Odometer { km, miles, .. } => {
            // 1 mile is exactly 1.609344 km by definition.
            let expected = km / 1.609_344;
            assert!((miles - expected).abs() < 0.01, "got {miles} mi");
        }
        other => panic!("expected odometer, got {other:?}"),
    }
}

#[test]
fn a_frame_shorter_than_the_field_decodes_nothing_rather_than_guessing() {
    // A truncated frame must not be read as a zero odometer.
    let frame = [0u8; 4];
    assert!(matches!(
        decode(&id_for(pgn::VEHICLE_DISTANCE), &frame),
        J1939Signal::UnknownPgn { .. }
    ));
}

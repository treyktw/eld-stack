//! Signal source precedence.
//!
//! Total vehicle distance arrives on two PGNs: 0xFEE0 at 0.125 km/bit and
//! 0xFEC1 at 5 m/bit — twenty-five times finer. Both previously wrote to the
//! same field, so whichever frame landed last won and the odometer visibly
//! jittered between resolutions.

use eld_decoders_test::decoder::{decode, J1939Signal};
use eld_decoders_test::distance::DistanceState;
use eld_decoders_test::id::J1939Id;
use eld_decoders_test::pgn;
use eld_decoders_test::signal_map::{best_priority_for, defs_for_pgn, is_known, SignalKind};

fn frame_id(pgn_value: u32, source: u8) -> J1939Id {
    J1939Id {
        pgn: pgn_value,
        priority: 6,
        source,
        destination: None,
    }
}

#[test]
fn hires_distance_outranks_coarse_distance() {
    let coarse = defs_for_pgn(pgn::VEHICLE_DISTANCE)
        .find(|d| d.kind == SignalKind::Odometer)
        .expect("0xFEE0 defines total distance");
    let hires = defs_for_pgn(pgn::HIRES_VEHICLE_DISTANCE)
        .find(|d| d.kind == SignalKind::Odometer)
        .expect("0xFEC1 defines total distance");

    assert!(hires.priority > coarse.priority);
    assert_eq!(best_priority_for(SignalKind::Odometer), hires.priority);
}

#[test]
fn a_coarse_reading_cannot_overwrite_a_hires_one() {
    let mut state = DistanceState::default();
    state.set_odometer(100_000.5, 20); // hi-res
    state.set_odometer(100_000.0, 10); // coarse, arrives later
    assert_eq!(state.odometer_km, Some(100_000.5));
}

#[test]
fn a_hires_reading_replaces_a_coarse_one() {
    let mut state = DistanceState::default();
    state.set_odometer(100_000.0, 10);
    state.set_odometer(100_000.5, 20);
    assert_eq!(state.odometer_km, Some(100_000.5));
    assert_eq!(state.source_priority(), 20);
}

#[test]
fn the_same_source_still_updates() {
    let mut state = DistanceState::default();
    state.set_odometer(100_000.0, 20);
    state.set_odometer(100_001.0, 20);
    assert_eq!(state.odometer_km, Some(100_001.0));
}

#[test]
fn the_first_reading_is_accepted_at_any_priority() {
    let mut state = DistanceState::default();
    state.set_odometer(5.0, 1);
    assert_eq!(state.odometer_km, Some(5.0));
}

#[test]
fn decode_reads_engine_hours_from_the_table() {
    // SPN 247, 0.05 h/bit, bytes 0..4 LE. 20 raw = 1.0 hour.
    let data = [20, 0, 0, 0, 0, 0, 0, 0];
    match decode(&frame_id(pgn::ENGINE_HOURS, 0x00), &data) {
        J1939Signal::EngineHours { hours, spn, .. } => {
            assert!((hours - 1.0).abs() < 1e-6);
            assert_eq!(spn, 247);
        }
        other => panic!("expected engine hours, got {other:?}"),
    }
}

#[test]
fn decode_carries_the_source_address_and_spn() {
    let data = [0, 0, 0, 0x00, 0x20, 0, 0, 0];
    match decode(&frame_id(pgn::EEC1, 0x0B), &data) {
        J1939Signal::EngineRpm { source, spn, .. } => {
            assert_eq!(source, 0x0B);
            assert_eq!(spn, 190);
        }
        other => panic!("expected engine rpm, got {other:?}"),
    }
}

#[test]
fn a_not_available_value_is_absent_rather_than_zero() {
    // J1939 encodes "not available" as all-ones. Reporting that as 0 km would
    // be a fabricated odometer reading.
    let data = [0xFF; 8];
    assert!(matches!(
        decode(&frame_id(pgn::HIRES_VEHICLE_DISTANCE, 0x00), &data),
        J1939Signal::UnknownPgn { .. }
    ));
}

#[test]
fn an_undefined_pgn_is_reported_for_mining() {
    let data = [1, 2, 3, 4, 5, 6, 7, 8];
    match decode(&frame_id(0xFF10, 0x00), &data) {
        J1939Signal::UnknownPgn { pgn: seen, .. } => assert_eq!(seen, 0xFF10),
        other => panic!("expected unknown pgn, got {other:?}"),
    }
    assert!(!is_known(0xFF10));
    assert!(is_known(pgn::EEC1));
}

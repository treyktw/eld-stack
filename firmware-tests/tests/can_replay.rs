//! The `can-replay` demo, pinned: the synthetic capture must decode to the
//! values the generator encoded (scripts/gen-synthetic-candump.py).

use eld_decoders_test::obd_pid::Value;
use eld_decoders_test::replay::{parse_line, replay};
use eld_decoders_test::verifier::mileage::MileageSource;

const LOG: &str = include_str!("../fixtures/synthetic-drive.log");

#[test]
fn candump_lines_parse_with_correct_id_width() {
    let ext = parse_line("(1780000001.000000) vcan0 0CF00400#FFFFFFF82AFFFFFF").unwrap();
    assert!(ext.extended);
    assert_eq!(ext.id, 0x0CF0_0400);
    assert_eq!(ext.ts_ms, 1_780_000_001_000);
    assert_eq!(ext.data.len(), 8);

    let std = parse_line("(1780000061.5) vcan0 7E8#04410C1AF8000000").unwrap();
    assert!(!std.extended);
    assert_eq!(std.id, 0x7E8);
    assert_eq!(std.ts_ms, 1_780_000_061_500);

    assert!(parse_line("garbage").is_none());
    assert!(parse_line("(1.0) vcan0 7E8#ABC").is_none(), "odd hex length");
}

#[test]
fn synthetic_drive_decodes_vin_and_faults_over_bam() {
    let s = replay(LOG);
    assert_eq!(s.skipped_lines, 0);
    assert_eq!(s.vin.as_deref(), Some("TSTDEMX00T0000042"));
    let spns: Vec<u32> = s.faults.iter().map(|d| d.spn).collect();
    assert_eq!(spns, vec![100, 110, 520_200], "SPN > 16 bits needs the 3 high bits");
}

#[test]
fn synthetic_drive_decodes_engine_signals() {
    let s = replay(LOG);
    assert!((s.speed_kph.unwrap() - 96.0).abs() < 0.01);
    assert!((s.fuel_percent.unwrap() - 72.4).abs() < 0.5);
    assert!((s.engine_hours.unwrap() - 18_420.35).abs() < 0.1);
    let rpm = s.rpm.unwrap();
    assert!((1350.0..=1425.0).contains(&rpm), "rpm {rpm}");
}

#[test]
fn hi_res_odometer_wins_and_agrees_with_speed_integration() {
    let s = replay(LOG);
    // 412 345 km + 96 km/h for 56 s (last HRVD sample) at 5 m/bit.
    let km = s.odometer_km.unwrap();
    // f32 resolution at ~412 000 km is ~0.03 km, hence the tolerance.
    assert!((km - 412_346.47).abs() < 0.05, "odometer {km}");

    let m = s.mileage.expect("verifier window opened");
    assert_eq!(m.source, MileageSource::Odometer);
    // Odometer and integrated wheel speed describe the same drive.
    assert!(m.confidence > 0.95, "confidence {}", m.confidence);
    assert!((m.miles - 0.93).abs() < 0.05, "trip {} mi", m.miles);
}

#[test]
fn obd_mode01_frames_on_the_same_capture_decode() {
    let s = replay(LOG);
    let values: Vec<Value> = s.obd_readings.iter().map(|(_, v)| *v).collect();
    assert!(values.contains(&Value::Rpm(1726.0)));
    assert!(values.contains(&Value::SpeedKph(96.0)));
    assert!(values.contains(&Value::CoolantTempC(83.0)));
}

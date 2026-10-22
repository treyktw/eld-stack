//! Telemetry send cadence.
//!
//! This is the code that decides the cellular bill. It used to be a fixed
//! 30-second timer: ~2,880 uploads per truck per day, each paying its own TLS
//! handshake, the vast majority restating what the last one said while parked.

use eld_decoders_test::cadence::{telemetry_due, CadencePolicy};
use eld_decoders_test::snapshot::TelemetrySnapshot;

const POLICY: CadencePolicy = CadencePolicy {
    min_interval_ms: 30_000,
    heartbeat_ms: 300_000,
    speed_delta_kph: 8.0,
    odometer_delta_km: 1.6,
    engine_hours_delta: 0.25,
};

fn snap(speed: Option<f32>, odo: Option<f32>, hours: Option<f32>) -> TelemetrySnapshot {
    TelemetrySnapshot {
        rpm: None,
        speed_kph: speed,
        fuel_percent: None,
        odometer_km: odo,
        odometer_mi: odo.map(|km| km / 1.609),
        engine_hours: hours,
    }
}

#[test]
fn the_first_snapshot_is_always_sent() {
    // The server needs a baseline, or it cannot tell a quiet truck from a
    // device that never checked in.
    assert!(telemetry_due(
        None,
        &snap(Some(0.0), Some(1000.0), Some(10.0)),
        0,
        &POLICY
    ));
}

#[test]
fn a_parked_truck_does_not_upload_between_heartbeats() {
    let parked = snap(Some(0.0), Some(1000.0), Some(10.0));
    // Four minutes on, nothing has changed: this is the case that used to send
    // eight separate uploads.
    assert!(!telemetry_due(Some(&parked), &parked, 240_000, &POLICY));
}

#[test]
fn a_parked_truck_still_reports_on_the_heartbeat() {
    let parked = snap(Some(0.0), Some(1000.0), Some(10.0));
    assert!(telemetry_due(Some(&parked), &parked, 300_000, &POLICY));
}

#[test]
fn pulling_away_is_reported() {
    let parked = snap(Some(0.0), Some(1000.0), Some(10.0));
    let moving = snap(Some(45.0), Some(1000.2), Some(10.0));
    assert!(telemetry_due(Some(&parked), &moving, 30_000, &POLICY));
}

#[test]
fn a_small_speed_wobble_is_not_reported() {
    let a = snap(Some(88.0), Some(1000.0), Some(10.0));
    let b = snap(Some(91.0), Some(1000.5), Some(10.0));
    // 3 km/h and half a kilometre: cruising noise, not news.
    assert!(!telemetry_due(Some(&a), &b, 30_000, &POLICY));
}

#[test]
fn distance_travelled_is_reported_even_at_steady_speed() {
    let a = snap(Some(88.0), Some(1000.0), Some(10.0));
    let b = snap(Some(88.0), Some(1002.0), Some(10.0));
    assert!(telemetry_due(Some(&a), &b, 30_000, &POLICY));
}

#[test]
fn the_rate_limit_holds_even_when_everything_changes() {
    let a = snap(Some(0.0), Some(1000.0), Some(10.0));
    let b = snap(Some(90.0), Some(1050.0), Some(12.0));
    // A noisy or flapping signal must not be able to flood a metered link.
    assert!(!telemetry_due(Some(&a), &b, 29_999, &POLICY));
    assert!(telemetry_due(Some(&a), &b, 30_000, &POLICY));
}

#[test]
fn losing_a_signal_is_itself_reportable() {
    let with_speed = snap(Some(88.0), Some(1000.0), Some(10.0));
    let lost_speed = snap(None, Some(1000.0), Some(10.0));
    // Losing engine speed mid-trip matters as much as its value changing — it
    // is the input to engine-synchronization diagnostics.
    assert!(telemetry_due(
        Some(&with_speed),
        &lost_speed,
        30_000,
        &POLICY
    ));
    assert!(telemetry_due(
        Some(&lost_speed),
        &with_speed,
        30_000,
        &POLICY
    ));
}

#[test]
fn two_absent_signals_are_not_a_change() {
    let a = snap(None, None, None);
    assert!(!telemetry_due(Some(&a), &a, 30_000, &POLICY));
}

#[test]
fn the_heartbeat_outranks_a_misconfigured_rate_limit() {
    // If min_interval were ever set above heartbeat, the device must still
    // report rather than going silent.
    let broken = CadencePolicy {
        min_interval_ms: 600_000,
        ..POLICY
    };
    let parked = snap(Some(0.0), Some(1000.0), Some(10.0));
    assert!(telemetry_due(Some(&parked), &parked, 300_000, &broken));
}

#[test]
fn engine_hours_accruing_while_idling_is_reported() {
    let a = snap(Some(0.0), Some(1000.0), Some(10.0));
    let b = snap(Some(0.0), Some(1000.0), Some(10.3));
    // Stationary but running: engine hours are the only thing moving, and they
    // feed on-duty time.
    assert!(telemetry_due(Some(&a), &b, 30_000, &POLICY));
}

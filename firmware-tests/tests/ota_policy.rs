//! When it is safe to install an update and reboot.
//!
//! The failure this guards against is concrete: a reboot mid-drive loses several
//! seconds of duty-status and position data that the carrier is required to
//! hold. So every ambiguous case must resolve to "wait".

use eld_decoders_test::ota::policy::{
    decide, image_is_installable, Blocker, InstallDecision, VehicleState, MIN_STATIONARY_MS,
};

const DIGEST: &str = "1cce4833e976a8075afecf3105866b4fc5e15a93ceb19af45d23301668a4d651";

/// Parked: engine off, stopped, fresh readings.
fn parked() -> VehicleState {
    VehicleState {
        engine_on: Some(false),
        speed_kph: Some(0.0),
        stationary_ms: 120_000,
        reading_age_ms: 500,
    }
}

#[test]
fn installs_when_parked_with_the_engine_off() {
    assert_eq!(decide(parked()), InstallDecision::Install);
}

#[test]
fn refuses_while_moving() {
    let state = VehicleState {
        engine_on: Some(true),
        speed_kph: Some(88.0),
        stationary_ms: 0,
        reading_age_ms: 200,
    };
    assert_eq!(decide(state), InstallDecision::Hold(Blocker::Moving));
}

#[test]
fn refuses_at_walking_pace() {
    // Creeping through a yard is still moving.
    let state = VehicleState {
        speed_kph: Some(3.0),
        ..parked()
    };
    assert_eq!(decide(state), InstallDecision::Hold(Blocker::Moving));
}

#[test]
fn tolerates_speed_dither_around_zero() {
    // J1939 wheel speed jitters on a stationary truck; demanding exactly 0.0
    // would mean some vehicles never update.
    let state = VehicleState {
        speed_kph: Some(0.4),
        ..parked()
    };
    assert_eq!(decide(state), InstallDecision::Install);
}

#[test]
fn refuses_when_readings_are_stale() {
    // A stale snapshot reading 0.0 is not evidence of a stopped truck — it is
    // evidence we stopped hearing from the bus, which is exactly when a truck
    // might be moving with a disconnected harness.
    let state = VehicleState {
        reading_age_ms: 60_000,
        ..parked()
    };
    assert_eq!(decide(state), InstallDecision::Hold(Blocker::StaleReadings));
}

#[test]
fn staleness_is_checked_before_speed() {
    // Even an explicit "moving" reading should report staleness first, so the
    // diagnosis names the real problem.
    let state = VehicleState {
        engine_on: Some(true),
        speed_kph: Some(90.0),
        stationary_ms: 0,
        reading_age_ms: 60_000,
    };
    assert_eq!(decide(state), InstallDecision::Hold(Blocker::StaleReadings));
}

#[test]
fn refuses_when_speed_is_unknown() {
    let state = VehicleState {
        speed_kph: None,
        ..parked()
    };
    assert_eq!(decide(state), InstallDecision::Hold(Blocker::SpeedUnknown));
}

#[test]
fn refuses_nan_speed() {
    // NaN compares false against every bound, so a naive `!(speed > limit)`
    // check would treat it as stopped.
    let state = VehicleState {
        speed_kph: Some(f32::NAN),
        ..parked()
    };
    assert_eq!(decide(state), InstallDecision::Hold(Blocker::Moving));
}

#[test]
fn engine_running_requires_the_stationary_dwell() {
    // Stopped at a light: about to move again.
    let at_a_light = VehicleState {
        engine_on: Some(true),
        speed_kph: Some(0.0),
        stationary_ms: 4_000,
        reading_age_ms: 200,
    };
    assert_eq!(
        decide(at_a_light),
        InstallDecision::Hold(Blocker::NotStationaryLongEnough)
    );

    let idling_at_a_stop = VehicleState {
        stationary_ms: MIN_STATIONARY_MS,
        ..at_a_light
    };
    assert_eq!(decide(idling_at_a_stop), InstallDecision::Install);
}

#[test]
fn unknown_engine_state_still_requires_the_dwell() {
    let state = VehicleState {
        engine_on: None,
        speed_kph: Some(0.0),
        stationary_ms: 1_000,
        reading_age_ms: 100,
    };
    assert_eq!(
        decide(state),
        InstallDecision::Hold(Blocker::NotStationaryLongEnough)
    );
}

#[test]
fn engine_off_does_not_override_movement() {
    // A rolling truck with the engine reported off — coasting, or a bad reading.
    // "Engine off" must not be a shortcut past the speed check.
    let state = VehicleState {
        engine_on: Some(false),
        speed_kph: Some(40.0),
        stationary_ms: 0,
        reading_age_ms: 100,
    };
    assert_eq!(decide(state), InstallDecision::Hold(Blocker::Moving));
}

#[test]
fn image_must_match_digest_and_size() {
    assert!(image_is_installable(DIGEST, DIGEST, 1024, 1024));
}

#[test]
fn rejects_a_truncated_download() {
    assert!(!image_is_installable(DIGEST, DIGEST, 1024, 1000));
}

#[test]
fn rejects_a_same_length_substitution() {
    // The size check alone would pass this; the digest is what catches it.
    let other = "0000000000000000000000000000000000000000000000000000000000000000";
    assert!(!image_is_installable(DIGEST, other, 1024, 1024));
}

#[test]
fn rejects_a_malformed_expected_digest() {
    // If the expected digest is not a full SHA-256, a matching "actual" proves
    // nothing — refuse rather than compare two equally short strings.
    assert!(!image_is_installable("abc", "abc", 1024, 1024));
}

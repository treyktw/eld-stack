use eld_decoders_test::decoder::J1939Signal;
use eld_decoders_test::telemetry_store::TelemetryStore;

fn speed(kph: f32) -> J1939Signal {
    J1939Signal::WheelSpeed {
        kph,
        source: 0,
        spn: 84,
        priority: 10,
    }
}

#[test]
fn fresh_speed_is_available_at_the_timeout_boundary() {
    let mut store = TelemetryStore::new();
    store.ingest(&speed(80.0), 1_000);
    assert_eq!(store.snapshot(6_000).speed_kph, Some(80.0));
}

#[test]
fn stale_speed_disappears_instead_of_driving_trip_state_forever() {
    let mut store = TelemetryStore::new();
    store.ingest(&speed(80.0), 1_000);
    assert_eq!(store.snapshot(6_001).speed_kph, None);
}

#[test]
fn a_fresh_reading_restores_the_signal() {
    let mut store = TelemetryStore::new();
    store.ingest(&speed(80.0), 1_000);
    assert_eq!(store.snapshot(7_000).speed_kph, None);
    store.ingest(&speed(0.0), 7_000);
    assert_eq!(store.snapshot(7_000).speed_kph, Some(0.0));
}

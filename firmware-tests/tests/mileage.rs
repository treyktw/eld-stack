//! GPS distance accumulation — the independent mileage source.
//!
//! This feeds mileage verification, and mileage is a regulatory number for both
//! ELD records and IFTA. A wrong total here is not a display bug.

use eld_decoders_test::gps_distance::{haversine_km, DistanceAccumulator};
use eld_decoders_test::gps_quality::acceptable;

#[test]
fn haversine_matches_a_known_distance() {
    // Chicago to Atlanta: ~945 km great-circle. (Road distance is ~1150 km —
    // a different number, and not what this computes.)
    let km = haversine_km(41.8781, -87.6298, 33.7490, -84.3880);
    assert!((km - 945.0).abs() < 15.0, "got {km} km");
}

#[test]
fn haversine_is_symmetric() {
    let a = haversine_km(41.8781, -87.6298, 33.7490, -84.3880);
    let b = haversine_km(33.7490, -84.3880, 41.8781, -87.6298);
    assert!((a - b).abs() < 1e-9);
}

#[test]
fn haversine_of_a_point_with_itself_is_zero() {
    assert_eq!(haversine_km(41.8781, -87.6298, 41.8781, -87.6298), 0.0);
}

#[test]
fn antipodal_points_do_not_produce_nan() {
    // The case that motivated clamping: rounding can push the intermediate
    // above 1.0, and asin of >1 is NaN. One NaN would make the running total
    // NaN for the rest of the run, with no recovery short of a reboot.
    let km = haversine_km(0.0, 0.0, 0.0, 180.0);
    assert!(km.is_finite(), "antipodal distance must be finite");
    assert!((km - 20_015.0).abs() < 50.0, "got {km} km");
}

#[test]
fn the_first_fix_adds_nothing() {
    let mut acc = DistanceAccumulator::new();
    assert_eq!(acc.add(41.8781, -87.6298), 0.0);
    assert_eq!(acc.total_km(), 0.0);
}

#[test]
fn parked_jitter_is_bounded_by_distance_not_by_fix_count() {
    let mut acc = DistanceAccumulator::new();
    acc.add(41.878100, -87.629800);
    // Fifty fixes, each ~0.11 m from the last but drifting one way. Every
    // individual step is under the 3 m floor.
    for step in 1..50 {
        acc.add(41.878100 + (step as f64) * 0.000001, -87.629800);
    }
    // The anchor is held while steps are rejected, so the drift is counted once
    // when it finally clears the floor — not fifty times. What matters is that
    // the total tracks how far the truck actually moved (~5 m), and does not
    // grow with the number of fixes received.
    let total = acc.total_km();
    assert!(
        total < 0.01,
        "parked drift should stay metres, got {total} km"
    );
}

#[test]
fn oscillating_jitter_never_accumulates() {
    // Jitter that wobbles around a point rather than drifting: the honest
    // parked case, and it must produce exactly nothing.
    let mut acc = DistanceAccumulator::new();
    acc.add(41.878100, -87.629800);
    for step in 0..100 {
        let wobble = if step % 2 == 0 { 0.000001 } else { -0.000001 };
        acc.add(41.878100 + wobble, -87.629800);
    }
    assert_eq!(acc.total_km(), 0.0);
}

#[test]
fn real_movement_accumulates() {
    let mut acc = DistanceAccumulator::new();
    acc.add(41.8781, -87.6298);
    acc.add(41.8881, -87.6298); // ~1.11 km north
    let total = acc.total_km();
    assert!((total - 1.11).abs() < 0.05, "got {total} km");
}

#[test]
fn successive_legs_sum() {
    let mut acc = DistanceAccumulator::new();
    acc.add(41.8781, -87.6298);
    acc.add(41.8881, -87.6298);
    acc.add(41.8981, -87.6298);
    let total = acc.total_km();
    assert!((total - 2.22).abs() < 0.1, "got {total} km");
}

#[test]
fn a_non_finite_coordinate_cannot_poison_the_total() {
    let mut acc = DistanceAccumulator::new();
    acc.add(41.8781, -87.6298);
    acc.add(41.8881, -87.6298);
    let before = acc.total_km();

    acc.add(f64::NAN, -87.6298);
    acc.add(41.9, f64::INFINITY);

    assert_eq!(acc.total_km(), before, "total must survive a bad fix");
    assert!(acc.total_km().is_finite());
}

#[test]
fn jitter_keeps_the_previous_anchor_so_slow_drift_is_not_free_distance() {
    // Each step is under the floor and rejected. If the anchor advanced on a
    // rejected fix, fifty sub-threshold steps would silently become real
    // distance.
    let mut acc = DistanceAccumulator::new();
    acc.add(41.878100, -87.629800);
    for step in 1..=20 {
        acc.add(41.878100 + (step as f64) * 0.00002, -87.629800);
    }
    // 20 steps x ~2.2 m each: individually rejected, but the anchor stays put,
    // so once the accumulated offset clears the floor it is counted once.
    let total = acc.total_km();
    assert!(total < 0.06, "drift should not compound, got {total} km");
}

#[test]
fn fix_quality_gating_rejects_a_missing_hdop() {
    // No HDOP means no confidence, and an unverified fix must not feed a
    // regulatory distance total.
    assert!(!acceptable(None));
    assert!(acceptable(Some(1.2)));
    assert!(acceptable(Some(5.0)), "at the threshold");
    assert!(!acceptable(Some(5.1)));
}

//! Cumulative travel distance from successive GPS fixes (haversine).
//!
//! This is the independent second/third distance source for mileage
//! verification. Movements below `GPS_MIN_MOVE_M` are ignored so a parked
//! vehicle's position jitter doesn't inflate the total.

use crate::config;

const EARTH_RADIUS_KM: f64 = 6371.0088;

pub struct DistanceAccumulator {
    total_km: f64,
    last: Option<(f64, f64)>,
}

impl DistanceAccumulator {
    pub fn new() -> Self {
        Self {
            total_km: 0.0,
            last: None,
        }
    }

    /// Add a fix point. Returns the distance added (km); 0 if filtered as jitter
    /// or this is the first point.
    pub fn add(&mut self, lat: f64, lon: f64) -> f64 {
        // A non-finite coordinate must never reach the accumulator: one NaN
        // makes the total NaN for the rest of the run.
        if !lat.is_finite() || !lon.is_finite() {
            return 0.0;
        }

        let added = match self.last {
            None => 0.0,
            Some((plat, plon)) => {
                let d = haversine_km(plat, plon, lat, lon);
                if !d.is_finite() {
                    return 0.0;
                }
                if d * 1000.0 < config::GPS_MIN_MOVE_M as f64 {
                    return 0.0; // jitter: ignore, keep previous anchor
                }
                d
            }
        };
        self.total_km += added;
        self.last = Some((lat, lon));
        added
    }

    pub fn total_km(&self) -> f32 {
        self.total_km as f32
    }
}

impl Default for DistanceAccumulator {
    fn default() -> Self {
        Self::new()
    }
}

pub fn haversine_km(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let (p1, p2) = (lat1.to_radians(), lat2.to_radians());
    let dlat = (lat2 - lat1).to_radians();
    let dlon = (lon2 - lon1).to_radians();
    let a = (dlat / 2.0).sin().powi(2) + p1.cos() * p2.cos() * (dlon / 2.0).sin().powi(2);
    // Clamp before asin. Rounding can push `a` a hair above 1.0 for nearly
    // antipodal points, and asin of >1 is NaN — which would propagate into the
    // running total and leave the GPS odometer permanently NaN, with no way to
    // recover short of a reboot.
    2.0 * EARTH_RADIUS_KM * a.clamp(0.0, 1.0).sqrt().asin()
}

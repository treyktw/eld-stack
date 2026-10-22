//! Trip distance from the vehicle odometer (preferred mileage source).
//!
//! Captures the odometer at trip start and tracks the latest reading; the
//! delta is the trip distance. `None` until at least one reading arrives.

pub struct OdometerTracker {
    start_km: Option<f32>,
    last_km: Option<f32>,
}

impl OdometerTracker {
    pub fn new() -> Self {
        Self {
            start_km: None,
            last_km: None,
        }
    }

    /// Capture the starting reading (may be `None` if unavailable at start).
    pub fn capture_start(&mut self, km: Option<f32>) {
        self.start_km = km;
        self.last_km = km;
    }

    /// Record the latest reading. Back-fills the start if it was missing.
    pub fn update(&mut self, km: f32) {
        if self.start_km.is_none() {
            self.start_km = Some(km);
        }
        self.last_km = Some(km);
    }

    /// Trip distance (end - start), if both ends are known. Clamps tiny
    /// negative noise to 0; a real decrease means a counter reset (returns None).
    pub fn delta_km(&self) -> Option<f32> {
        match (self.start_km, self.last_km) {
            (Some(s), Some(l)) => {
                let d = l - s;
                if d < -0.5 {
                    None // odometer went backwards: reset/rollover, untrusted
                } else {
                    Some(d.max(0.0))
                }
            }
            _ => None,
        }
    }
}

impl Default for OdometerTracker {
    fn default() -> Self {
        Self::new()
    }
}

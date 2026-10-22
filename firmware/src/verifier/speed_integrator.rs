//! Distance by integrating wheel speed over time.
//!
//! Each update adds `speed * dt` to a running total. This is the fallback
//! mileage source when no odometer PGN is available, and a cross-check when one
//! is. Uses monotonic ms for `dt`, so it's robust to clock changes.

pub struct SpeedIntegrator {
    km: f32,
    last_ms: Option<u64>,
}

impl SpeedIntegrator {
    pub fn new() -> Self {
        Self {
            km: 0.0,
            last_ms: None,
        }
    }

    pub fn reset(&mut self) {
        self.km = 0.0;
        self.last_ms = None;
    }

    /// Integrate one sample: `speed_kph` held over the interval since the last
    /// update (rectangle rule — fine at typical sub-second CAN rates).
    pub fn update(&mut self, speed_kph: f32, now_ms: u64) {
        if let Some(last) = self.last_ms {
            if now_ms > last {
                let dt_h = (now_ms - last) as f32 / 3_600_000.0;
                self.km += speed_kph.max(0.0) * dt_h;
            }
        }
        self.last_ms = Some(now_ms);
    }

    pub fn km(&self) -> f32 {
        self.km
    }
}

impl Default for SpeedIntegrator {
    fn default() -> Self {
        Self::new()
    }
}

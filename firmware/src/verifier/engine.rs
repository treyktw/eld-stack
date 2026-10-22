//! Verifier engine: combine distance sources into a verified mileage.
//!
//! Decision flow:
//!   odometer delta available → primary = odometer, cross-check vs speed
//!   otherwise                → primary = speed integration (lower confidence)
//!
//! GPS becomes a third source in Phase 5.

use super::confidence;
use super::mileage::{MileageSource, VerifiedMileage};
use super::odometer::OdometerTracker;
use super::speed_integrator::SpeedIntegrator;
use crate::j1939::spn;

pub struct VerifierEngine {
    integrator: SpeedIntegrator,
    odometer: OdometerTracker,
    /// GPS lifetime distance tracked like the odometer (start/last → delta).
    gps: OdometerTracker,
}

impl VerifierEngine {
    pub fn new() -> Self {
        Self {
            integrator: SpeedIntegrator::new(),
            odometer: OdometerTracker::new(),
            gps: OdometerTracker::new(),
        }
    }

    /// Start a fresh verification window (called at trip start).
    pub fn begin(&mut self, odometer_km: Option<f32>, gps_km: Option<f32>, _now_ms: u64) {
        self.integrator.reset();
        self.odometer.capture_start(odometer_km);
        self.gps.capture_start(gps_km);
    }

    /// Feed the latest telemetry + GPS distance.
    pub fn update(
        &mut self,
        speed_kph: Option<f32>,
        odometer_km: Option<f32>,
        gps_km: Option<f32>,
        now_ms: u64,
    ) {
        if let Some(s) = speed_kph {
            self.integrator.update(s, now_ms);
        }
        if let Some(o) = odometer_km {
            self.odometer.update(o);
        }
        if let Some(g) = gps_km {
            self.gps.update(g);
        }
    }

    /// Produce the verified mileage for the window so far.
    ///
    /// Priority: odometer > GPS > speed integration. The non-primary sources
    /// cross-check the primary to set confidence.
    pub fn finalize(&self) -> VerifiedMileage {
        let integ_km = self.integrator.km();
        let gps_km = self.gps.delta_km();

        if let Some(odo_km) = self.odometer.delta_km() {
            // Cross-check against GPS if present, else speed integration.
            let secondary = gps_km.unwrap_or(integ_km);
            return VerifiedMileage {
                miles: km_to_miles(odo_km),
                source: MileageSource::Odometer,
                confidence: confidence::agreement(odo_km, secondary),
                notes: "odometer primary, cross-checked",
            };
        }

        if let Some(g) = gps_km {
            // No odometer: GPS is more reliable than raw speed integration.
            return VerifiedMileage {
                miles: km_to_miles(g),
                source: MileageSource::GpsIntegration,
                confidence: (confidence::agreement(g, integ_km) * 0.9).clamp(0.0, 1.0),
                notes: "no odometer; gps primary, speed cross-checked",
            };
        }

        VerifiedMileage {
            miles: km_to_miles(integ_km),
            source: MileageSource::SpeedIntegration,
            confidence: 0.6,
            notes: "no odometer/gps; speed-integrated",
        }
    }
}

impl Default for VerifierEngine {
    fn default() -> Self {
        Self::new()
    }
}

fn km_to_miles(km: f32) -> f32 {
    km / spn::KM_PER_MILE
}

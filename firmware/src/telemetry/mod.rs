//! Telemetry store: the current truth table.
//!
//! Decoded `J1939Signal`s are folded into per-domain state; `snapshot()`
//! squashes the latest values into a flat `TelemetrySnapshot` for
//! logging/upload/trips. Source + timestamp travel with the upload envelope, so
//! the store keeps only the latest value per signal.

pub mod cadence;
pub mod distance;
pub mod engine;
pub mod fuel;
pub mod snapshot;
pub mod vehicle;

use crate::j1939::decoder::J1939Signal;
use crate::j1939::spn;

use distance::DistanceState;
use engine::EngineState;
use fuel::FuelState;
use snapshot::TelemetrySnapshot;
use vehicle::VehicleState;

#[derive(Debug, Default)]
pub struct TelemetryStore {
    pub engine: EngineState,
    pub vehicle: VehicleState,
    pub fuel: FuelState,
    pub distance: DistanceState,
    rpm_observed_ms: Option<u64>,
    speed_observed_ms: Option<u64>,
    fuel_observed_ms: Option<u64>,
    hours_observed_ms: Option<u64>,
    odometer_observed_ms: Option<u64>,
}

/// Trust levels for OBD-sourced values, on the same scale as
/// `j1939::signal_map` (which uses 10 and 20).
pub mod obd {
    /// Below every J1939 source. A vehicle reporting an odometer over both
    /// keeps the J1939 figure: OBD PID 0xA6 is rarely implemented on heavy
    /// vehicles, and where it is, it is more often rounded or derived.
    pub const ODOMETER_PRIORITY: u8 = 5;
}

impl TelemetryStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Fold a decoded signal into the store.
    pub fn ingest(&mut self, sig: &J1939Signal, now_ms: u64) {
        match *sig {
            J1939Signal::EngineRpm { rpm, .. } => {
                self.engine.set_rpm(rpm);
                self.rpm_observed_ms = Some(now_ms);
            }
            J1939Signal::WheelSpeed { kph, .. } => {
                self.vehicle.set_speed(kph);
                self.speed_observed_ms = Some(now_ms);
            }
            J1939Signal::FuelLevel { percent, .. } => {
                self.fuel.set_level(percent);
                self.fuel_observed_ms = Some(now_ms);
            }
            J1939Signal::EngineHours { hours, .. } => {
                self.engine.set_hours(hours);
                self.hours_observed_ms = Some(now_ms);
            }
            J1939Signal::Odometer { km, priority, .. } => {
                if self.distance.set_odometer(km, priority) {
                    self.odometer_observed_ms = Some(now_ms);
                }
            }
            J1939Signal::UnknownPgn { .. } => {}
        }
    }

    /// Ingest a decoded OBD-II reading.
    ///
    /// Feeds the same store as J1939 so everything downstream — trip detection,
    /// mileage, the telemetry snapshot — is indifferent to which protocol the
    /// value arrived over. A vehicle answering both must not produce two
    /// parallel views of its own speed.
    pub fn ingest_obd(&mut self, reading: &crate::obd::pid::Reading, now_ms: u64) {
        use crate::obd::pid::Value;

        match reading.value {
            Value::Rpm(rpm) => {
                self.engine.set_rpm(rpm);
                self.rpm_observed_ms = Some(now_ms);
            }
            Value::SpeedKph(kph) => {
                self.vehicle.set_speed(kph);
                self.speed_observed_ms = Some(now_ms);
            }
            Value::FuelLevelPercent(percent) => {
                self.fuel.set_level(percent);
                self.fuel_observed_ms = Some(now_ms);
            }
            // Run time is seconds since the engine started, which is not the
            // same quantity as J1939 total engine hours. Converted here rather
            // than conflated: a value that resets every key cycle must never
            // overwrite a lifetime total.
            Value::RunTimeSeconds(_) => {}
            // Lower priority than a J1939 odometer. Both are odometers, but
            // OBD 0xA6 is rarely implemented on heavy vehicles and where it is,
            // it is more often a rounded or derived figure.
            Value::OdometerKm(km) if self.distance.set_odometer(km, obd::ODOMETER_PRIORITY) => {
                self.odometer_observed_ms = Some(now_ms);
            }
            _ => {}
        }
    }

    /// Squash the latest values into a flat snapshot.
    pub fn snapshot(&self, now_ms: u64) -> TelemetrySnapshot {
        let km = self.distance.odometer_km;
        TelemetrySnapshot {
            rpm: fresh(
                self.engine.rpm,
                self.rpm_observed_ms,
                now_ms,
                crate::config::FAST_SIGNAL_MAX_AGE_MS,
            ),
            speed_kph: fresh(
                self.vehicle.speed_kph,
                self.speed_observed_ms,
                now_ms,
                crate::config::FAST_SIGNAL_MAX_AGE_MS,
            ),
            fuel_percent: fresh(
                self.fuel.level_pct,
                self.fuel_observed_ms,
                now_ms,
                crate::config::FUEL_SIGNAL_MAX_AGE_MS,
            ),
            odometer_km: fresh(
                km,
                self.odometer_observed_ms,
                now_ms,
                crate::config::SLOW_SIGNAL_MAX_AGE_MS,
            ),
            odometer_mi: fresh(
                km,
                self.odometer_observed_ms,
                now_ms,
                crate::config::SLOW_SIGNAL_MAX_AGE_MS,
            )
            .map(|k| k / spn::KM_PER_MILE),
            engine_hours: fresh(
                self.engine.hours,
                self.hours_observed_ms,
                now_ms,
                crate::config::SLOW_SIGNAL_MAX_AGE_MS,
            ),
        }
    }
}

fn fresh(
    value: Option<f32>,
    observed_ms: Option<u64>,
    now_ms: u64,
    max_age_ms: u64,
) -> Option<f32> {
    match (value, observed_ms) {
        (Some(value), Some(observed)) if now_ms.saturating_sub(observed) <= max_age_ms => {
            Some(value)
        }
        _ => None,
    }
}

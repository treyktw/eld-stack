//! A flat, point-in-time copy of current telemetry — the value the rest of the
//! firmware reads. Sources/timestamps live in the store; this is the squashed
//! "truth table" for logging, uploads, and trip summaries.

#[derive(Debug, Default, Clone, Copy)]
pub struct TelemetrySnapshot {
    pub rpm: Option<f32>,
    pub speed_kph: Option<f32>,
    pub fuel_percent: Option<f32>,
    pub odometer_km: Option<f32>,
    pub odometer_mi: Option<f32>,
    pub engine_hours: Option<f32>,
}

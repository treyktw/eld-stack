//! When a telemetry snapshot is worth sending.
//!
//! Telemetry is operational, not regulatory. Duty-status and trip events are
//! enqueued the moment they happen and never pass through here.
//!
//! It used to be uploaded on a fixed 30-second timer — roughly 2,880 uploads a
//! day per truck, the overwhelming majority of them restating what the previous
//! one said while the truck sat parked. On a metered cellular link each of
//! those paid a TLS handshake several times larger than the payload itself.
//!
//! Sending on meaningful change, with a heartbeat slow enough to still prove
//! the device is alive, carries the same information for a small fraction of
//! the bytes.

use super::snapshot::TelemetrySnapshot;

/// Thresholds that decide whether a change is worth reporting.
#[derive(Debug, Clone, Copy)]
pub struct CadencePolicy {
    /// Never send more often than this, however fast values move.
    pub min_interval_ms: u64,
    /// Always send at least this often, even when nothing changes.
    pub heartbeat_ms: u64,
    pub speed_delta_kph: f32,
    pub odometer_delta_km: f32,
    pub engine_hours_delta: f32,
}

/// A signal appearing or disappearing is itself news — losing the speed signal
/// mid-trip matters as much as its value changing.
fn changed(previous: Option<f32>, current: Option<f32>, delta: f32) -> bool {
    match (previous, current) {
        (Some(a), Some(b)) => (a - b).abs() >= delta,
        (None, None) => false,
        _ => true,
    }
}

/// Whether to send `current`, given what was last sent and how long ago.
pub fn telemetry_due(
    last_sent: Option<&TelemetrySnapshot>,
    current: &TelemetrySnapshot,
    since_last_upload_ms: u64,
    policy: &CadencePolicy,
) -> bool {
    // Nothing sent yet this boot: send one so the server has a baseline and can
    // tell a quiet truck from a device that never checked in.
    let Some(previous) = last_sent else {
        return true;
    };

    // The heartbeat outranks the rate limit; otherwise a misconfiguration where
    // the minimum interval exceeded the heartbeat would silence the device.
    if since_last_upload_ms >= policy.heartbeat_ms {
        return true;
    }
    if since_last_upload_ms < policy.min_interval_ms {
        return false;
    }

    changed(
        previous.speed_kph,
        current.speed_kph,
        policy.speed_delta_kph,
    ) || changed(
        previous.odometer_km,
        current.odometer_km,
        policy.odometer_delta_km,
    ) || changed(
        previous.engine_hours,
        current.engine_hours,
        policy.engine_hours_delta,
    )
}

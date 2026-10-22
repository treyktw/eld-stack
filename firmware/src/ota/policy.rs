//! When it is safe to install a downloaded image and reboot.
//!
//! Downloading is harmless and happens whenever there is signal. Installing is
//! not: it ends in a reboot, and a reboot means the device stops recording for
//! several seconds. Doing that while the truck is moving loses duty-status and
//! position data for a stretch of driving that a carrier is required to have,
//! and does it at the least forgivable moment.
//!
//! Every rule here fails closed. A missing or stale reading is treated as
//! "moving", never as "stopped" — the cost of waiting is a delayed update, and
//! the cost of guessing wrong is a gap in a federal record.

/// Vehicle state as the device currently understands it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VehicleState {
    /// Engine running, if known.
    pub engine_on: Option<bool>,
    /// Road speed in km/h, if a recent reading exists.
    pub speed_kph: Option<f32>,
    /// How long speed has been continuously at or below `STOPPED_SPEED_KPH`.
    pub stationary_ms: u64,
    /// Age of the newest vehicle reading. Anything older than
    /// `MAX_READING_AGE_MS` is not evidence about the present.
    pub reading_age_ms: u64,
}

/// At or below this is "stopped".
///
/// Not zero: J1939 wheel-based speed dithers around zero on a stationary truck,
/// and demanding an exact 0.0 would mean an update never installs on some
/// vehicles. 0.5 km/h is well inside the noise and far below any real movement.
pub const STOPPED_SPEED_KPH: f32 = 0.5;

/// How long the truck must have been stationary before installing.
///
/// Guards against installing during a momentary stop — a light, a queue at a
/// gate — where the truck is about to move again.
pub const MIN_STATIONARY_MS: u64 = 30_000;

/// Oldest vehicle reading still treated as describing now.
pub const MAX_READING_AGE_MS: u64 = 5_000;

/// Why an install is being held back. Reported so a device that never updates
/// can be diagnosed without guessing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Blocker {
    /// No recent vehicle data — we cannot claim the truck is stopped.
    StaleReadings,
    /// No speed reading at all.
    SpeedUnknown,
    /// The truck is moving.
    Moving,
    /// Stopped, but not yet for long enough.
    NotStationaryLongEnough,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallDecision {
    Install,
    Hold(Blocker),
}

/// Decide whether to install now.
///
/// Engine-off is sufficient on its own *provided* the truck is also stopped:
/// a shut-down engine with the vehicle stationary is unambiguously parked. With
/// the engine running, the stationary dwell is what separates "parked at a truck
/// stop" from "waiting at a light".
///
/// Note the ordering — staleness is checked before anything else. A stale
/// snapshot that happens to say `speed: 0.0` is not evidence of a stopped truck;
/// it is evidence that we stopped hearing from the bus, which is exactly when a
/// truck might be moving with a disconnected harness.
pub fn decide(state: VehicleState) -> InstallDecision {
    if state.reading_age_ms > MAX_READING_AGE_MS {
        return InstallDecision::Hold(Blocker::StaleReadings);
    }

    let Some(speed) = state.speed_kph else {
        return InstallDecision::Hold(Blocker::SpeedUnknown);
    };

    if !speed.is_finite() || speed > STOPPED_SPEED_KPH {
        return InstallDecision::Hold(Blocker::Moving);
    }

    // Engine explicitly off and the truck stopped: parked, install now.
    if state.engine_on == Some(false) {
        return InstallDecision::Install;
    }

    // Engine on, or unknown: require the dwell.
    if state.stationary_ms >= MIN_STATIONARY_MS {
        InstallDecision::Install
    } else {
        InstallDecision::Hold(Blocker::NotStationaryLongEnough)
    }
}

/// Whether the downloaded image may be installed at all.
///
/// Separate from `decide` because these are different kinds of question:
/// this one asks whether the bytes are trustworthy, `decide` asks whether the
/// moment is safe. Both must pass, and neither substitutes for the other.
pub fn image_is_installable(
    expected_sha256: &str,
    actual_sha256: &str,
    expected_size: u32,
    actual_size: u32,
) -> bool {
    // Length is checked first so a truncated download is rejected cheaply, but
    // the digest is what actually decides — a same-length substitution would
    // pass the size check alone.
    actual_size == expected_size && expected_sha256.len() == 64 && expected_sha256 == actual_sha256
}

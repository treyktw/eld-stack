//! Host harness that re-includes the firmware's pure J1939 modules so they can
//! be unit-tested with the normal toolchain. These files have no esp-idf deps —
//! only `heapless` and each other (`super::`). The source of truth stays in
//! `../firmware/src/j1939/`; nothing is copied.
//!
//! The modules are declared at the crate root (so `#[path]` resolves against
//! `src/` and each file's `use super::…` resolves to the crate root). A `j1939`
//! facade re-exports them so tests can mirror the firmware's namespace.

#![allow(clippy::duplicate_mod)]

// Time + NMEA are pure too: the esp-idf FFI in `time.rs` is cfg-gated to
// `target_os = "espidf"`, so the clock and calendar logic compile on the host.
#[path = "../../firmware/src/time.rs"]
pub mod time;

#[path = "../../firmware/src/gps/nmea.rs"]
pub mod nmea;

// Channel probe results are pure: which connector pair to believe.
/// Minimal stand-in for the firmware's build-time config, so `pins.rs` — which
/// reads a couple of constants from it — can be compiled on the host. Values
/// mirror `firmware/src/config/mod.rs`.
pub mod config {
    pub const CAN_TX_PIN: u8 = 15;
    pub const CAN_RX_PIN: u8 = 16;
    pub const CAN_CONNECTOR_PINS: &str = "6/14";
    pub const GPS_MIN_MOVE_M: f32 = 3.0;
    pub const GPS_MAX_HDOP: f32 = 5.0;
    pub const FAST_SIGNAL_MAX_AGE_MS: u64 = 5_000;
    pub const SLOW_SIGNAL_MAX_AGE_MS: u64 = 30_000;
    pub const FUEL_SIGNAL_MAX_AGE_MS: u64 = 60_000;
}

#[path = "../../firmware/src/can/pins.rs"]
pub mod pins;

#[path = "../../firmware/src/can/channel_probe_pure.rs"]
pub mod channel_probe;

/// Mirrors the firmware's `can::` namespace for tests.
pub mod can {
    pub use super::{channel_probe, pins};
}

// Telemetry cadence is pure: it decides how much cellular data the fleet burns.
#[path = "../../firmware/src/telemetry/snapshot.rs"]
pub mod snapshot;

#[path = "../../firmware/src/telemetry/cadence.rs"]
pub mod cadence;

/// Mirrors the firmware's `telemetry::` namespace for tests.
pub mod telemetry {
    pub use super::{cadence, snapshot};
}

// Identity's FFI is cfg-gated to espidf, so the id formatting is host-testable.
#[path = "../../firmware/src/device/identity.rs"]
pub mod identity;

#[path = "../../firmware/src/device/seq.rs"]
pub mod seq;

/// Mirrors the firmware's `gps::` namespace for tests.
pub mod gps {
    pub use super::nmea;
}

#[path = "../../firmware/src/j1939/pgn.rs"]
pub mod pgn;

#[path = "../../firmware/src/j1939/spn.rs"]
pub mod spn;

#[path = "../../firmware/src/j1939/validity.rs"]
pub mod validity;

#[path = "../../firmware/src/j1939/id.rs"]
pub mod id;

#[path = "../../firmware/src/j1939/signal_map.rs"]
pub mod signal_map;

#[path = "../../firmware/src/telemetry/distance.rs"]
pub mod distance;

#[path = "../../firmware/src/gps/distance.rs"]
pub mod gps_distance;

#[path = "../../firmware/src/gps/quality.rs"]
pub mod gps_quality;

#[path = "../../firmware/src/j1939/decoder.rs"]
pub mod decoder;

#[path = "../../firmware/src/j1939/transport.rs"]
pub mod transport;

// OTA decision-making is pure: version comparison, manifest parsing, and the
// rule about when rebooting a truck is safe. The flashing itself lives behind
// an espidf cfg gate and is not included here.
// Declared at the crate root, like every other re-included module, so
// `manifest.rs`'s `use super::version::…` resolves the same way it does inside
// the firmware's `ota` module.
#[path = "../../firmware/src/ota/version.rs"]
pub mod version;

#[path = "../../firmware/src/ota/manifest.rs"]
pub mod manifest;

#[path = "../../firmware/src/ota/policy.rs"]
pub mod policy;

#[path = "../../firmware/src/ota/signature.rs"]
pub mod signature;

// Key-trust policy is pure. The eFuse read that feeds it is in
// firmware/src/ota/verifier.rs behind an espidf gate and is not included.
#[path = "../../firmware/src/ota/key.rs"]
pub mod key;

// Cellular AT parsing and the link state machine are pure. The UART driver is
// behind an espidf cfg gate and is not included here.
#[path = "../../firmware/src/comms/cellular/at.rs"]
pub mod at;

#[path = "../../firmware/src/comms/cellular/link.rs"]
pub mod link;

// Line assembly is pure; the UART driver that uses it is espidf-gated.
#[path = "../../firmware/src/comms/cellular/lines.rs"]
pub mod line_assembler;

/// Mirrors the firmware's `comms::cellular::` namespace for tests.
pub mod cellular {
    pub use super::{at, line_assembler as lines, link};
}

// OBD-II decoding and PID discovery are pure: no CAN driver involved.
#[path = "../../firmware/src/obd/pid.rs"]
pub mod obd_pid;

#[path = "../../firmware/src/obd/support.rs"]
pub mod support;

#[path = "../../firmware/src/obd/poller.rs"]
pub mod obd_poller;

/// Mirrors the firmware's `obd::` namespace for tests.
pub mod obd {
    pub use super::obd_pid as pid;
    pub use super::obd_poller as poller;
    pub use super::support;
}

/// Mirrors the firmware's `ota::` namespace for tests.
pub mod ota {
    pub use super::{key, manifest, policy, signature, version};
}

/// Mirrors the firmware's `j1939::` namespace for tests.
pub mod j1939 {
    pub use super::{decoder, id, pgn, signal_map, spn, transport, validity};
}

#[path = "../../firmware/src/telemetry/mod.rs"]
pub mod telemetry_store;

// Mileage verification is pure: odometer delta vs GPS vs speed integration.
// Its `use crate::j1939::spn` resolves to the facade above.
#[path = "../../firmware/src/verifier/mod.rs"]
pub mod verifier;

#[path = "../../firmware/src/j1939/known_pgns.rs"]
pub mod known_pgns;

// Not firmware code: the host-only candump replay used by the `can-replay`
// demo binary and its integration test. Lives here so it is driven by exactly
// the same `#[path]`-included decoders the tests exercise.
pub mod replay;

//! OBD-II (SAE J1979) telemetry.
//!
//! The device speaks OBD-II only. J1939 decoding remains in `crate::j1939` for
//! the units already fielded, but new work targets OBD.
//!
//! Worth keeping straight, because the naming in the prototype sketches is
//! actively misleading: an earlier Arduino prototype sets `extd = 1` and calls
//! `decodeJ1939` — that is J1939 arriving on an OBD-shaped J1962 connector. The
//! connector is not the protocol. Real OBD-II is 11-bit, request `0x7DF`,
//! responses `0x7E8..=0x7EF`, which is what a captured OBD-II session from the
//! 2013 Volvo tractor actually shows.
//!
//! - [`pid`] — request framing, response parsing, and J1979 scaling.
//! - [`support`] — asking the ECU which PIDs it answers, rather than assuming.
//!
//! On that last point: the captured session returned exactly five PIDs — RPM,
//! speed, coolant, throttle and module voltage. No odometer, no fuel level, no
//! engine hours. Whether that is the truck's limit or the sketch's is not
//! recorded anywhere, which is precisely why support is discovered at runtime
//! instead of assumed from a hard-coded list.

pub mod pid;
pub mod poller;
pub mod support;

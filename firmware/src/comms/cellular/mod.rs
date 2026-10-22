//! Cellular uplink (SIMCom SIM7670G class, AT over UART).
//!
//! The record path must not depend on a driver's phone being alive, charged,
//! unbroken and in range — "the driver's phone died" is not a defence anyone
//! wants to offer an inspector.
//!
//! Split so the parts that can be wrong in subtle ways are testable without
//! hardware:
//!
//! - [`at`] — what a line from the modem means, including the unsolicited ones
//!   that arrive mid-response.
//! - [`link`] — bring-up, backoff, and recovery, as a pure state machine.
//!
//! The UART driver itself belongs behind a `target_os = "espidf"` gate, so
//! neither of the above needs a device to exercise.

pub mod at;
pub mod lines;
pub mod link;
pub mod modem;
pub mod uplink;

//! Device layer: identity, boot/reset reason, diagnostics, power, health.

pub mod board;
pub mod boot;
pub mod diagnostics;
pub mod health;
pub mod identity;
pub mod power;
pub mod seq;

pub use diagnostics::DiagnosticReport;
pub use identity::DeviceInfo;
pub use seq::SeqGen;

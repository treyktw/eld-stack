//! Central error type for the ELD firmware.
//!
//! Each subsystem maps its failures into a variant here. Implements
//! `std::error::Error` so `?` flows cleanly into `anyhow::Result` in `main`.

use core::fmt;

/// Firmware-wide result alias.
pub type Result<T> = core::result::Result<T, EldError>;

#[derive(Debug)]
pub enum EldError {
    /// CAN/TWAI driver or bus error.
    Can(&'static str),
    /// SD / filesystem / log storage error.
    Storage(&'static str),
    /// WiFi / Bluetooth / upload error.
    Comms(&'static str),
    /// GPS driver / NMEA parse error.
    Gps(&'static str),
    /// Device init / diagnostics error.
    Device(&'static str),
}

impl fmt::Display for EldError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EldError::Can(m) => write!(f, "can: {m}"),
            EldError::Storage(m) => write!(f, "storage: {m}"),
            EldError::Comms(m) => write!(f, "comms: {m}"),
            EldError::Gps(m) => write!(f, "gps: {m}"),
            EldError::Device(m) => write!(f, "device: {m}"),
        }
    }
}

impl std::error::Error for EldError {}

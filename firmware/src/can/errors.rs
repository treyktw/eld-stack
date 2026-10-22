//! CAN-layer errors.

use core::fmt;

use crate::error::EldError;

#[derive(Debug)]
pub enum CanError {
    /// Driver install / start / stop failed.
    Driver(&'static str),
    /// Receive returned an error (distinct from a benign timeout).
    Receive,
    /// Transmit failed.
    Transmit,
    /// Building a frame to send failed (bad dlc / payload).
    BadFrame,
}

impl fmt::Display for CanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CanError::Driver(m) => write!(f, "driver {m}"),
            CanError::Receive => f.write_str("receive failed"),
            CanError::Transmit => f.write_str("transmit failed"),
            CanError::BadFrame => f.write_str("bad frame"),
        }
    }
}

impl std::error::Error for CanError {}

impl From<CanError> for EldError {
    fn from(e: CanError) -> Self {
        match e {
            CanError::Driver(m) => EldError::Can(m),
            CanError::Receive => EldError::Can("receive failed"),
            CanError::Transmit => EldError::Can("transmit failed"),
            CanError::BadFrame => EldError::Can("bad frame"),
        }
    }
}

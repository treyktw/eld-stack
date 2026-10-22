//! Fix-quality scoring from satellite count and HDOP.

use crate::config;

/// Is a fix good enough to trust for distance accumulation?
pub fn acceptable(hdop: Option<f32>) -> bool {
    match hdop {
        Some(h) => h <= config::GPS_MAX_HDOP,
        None => false,
    }
}

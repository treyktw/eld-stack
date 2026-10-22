//! CAN bitrates the scanner cycles through.
//!
//! J1939 heavy-truck buses run at 250k (most common) or 500k. The scanner tries
//! each and keeps whichever sees valid traffic.

use esp_idf_svc::hal::can::config::Timing;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanBitrate {
    K250,
    K500,
}

impl CanBitrate {
    /// Scan order: 250k first (most common on heavy trucks), then 500k.
    pub const SCAN_ORDER: [CanBitrate; 2] = [CanBitrate::K250, CanBitrate::K500];

    pub fn bits_per_sec(self) -> u32 {
        match self {
            CanBitrate::K250 => 250_000,
            CanBitrate::K500 => 500_000,
        }
    }

    pub fn timing(self) -> Timing {
        match self {
            CanBitrate::K250 => Timing::B250K,
            CanBitrate::K500 => Timing::B500K,
        }
    }

    /// Short label for logs, e.g. "250k".
    pub fn label(self) -> &'static str {
        match self {
            CanBitrate::K250 => "250k",
            CanBitrate::K500 => "500k",
        }
    }
}

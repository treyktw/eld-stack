//! Structured log records.
//!
//! Records are not pretty strings — they're typed and serialized to a compact,
//! line-delimited format: `<ts_ms> <TYPE> key=val key=val ...`. One record per
//! line so logs are append-only and crash-tolerant, and `tools/decode-log.py`
//! (or a later JSONL pass) can parse them.

use std::fmt::Write;

pub use crate::time::RecordStamp;

#[derive(Debug, Clone)]
pub enum LogRecord {
    Boot {
        firmware: &'static str,
        board: &'static str,
    },
    /// Raw CAN frame as received from TWAI.
    CanFrame {
        id: u32,
        extended: bool,
        remote: bool,
        dlc: u8,
        data: [u8; 8],
    },
    /// A decoded J1939 signal.
    Signal {
        name: &'static str,
        value: f32,
        source: u8,
    },
    /// Periodic bus/telemetry heartbeat.
    Heartbeat {
        bitrate: &'static str,
        rx: u32,
        j1939: u32,
        errors: u32,
    },
    // --- reserved for later phases ---
    TripEvent {
        what: &'static str,
    },
    VerifiedMileage {
        miles: f32,
        confidence: f32,
    },
}

impl LogRecord {
    /// The log file this record belongs in.
    pub fn kind(&self) -> super::files::LogKind {
        use super::files::LogKind;
        match self {
            LogRecord::Boot { .. } => LogKind::System,
            LogRecord::CanFrame { .. } => LogKind::Can,
            LogRecord::Signal { .. } => LogKind::Events,
            LogRecord::Heartbeat { .. } => LogKind::Heartbeat,
            LogRecord::TripEvent { .. } | LogRecord::VerifiedMileage { .. } => LogKind::Events,
        }
    }

    /// Serialize to a single line (no trailing newline).
    pub fn to_line(&self, stamp: RecordStamp) -> String {
        let mut s = String::with_capacity(80);
        s.push_str(&stamp.prefix());
        match self {
            LogRecord::Boot { firmware, board } => {
                let _ = write!(s, "BOOT fw={firmware} board={board}");
            }
            LogRecord::CanFrame {
                id,
                extended,
                remote,
                dlc,
                data,
            } => {
                let width = if *extended { 8 } else { 3 };
                let _ = write!(
                    s,
                    "CAN id=0x{id:0width$X} ext={} remote={} dlc={} data=",
                    *extended as u8, *remote as u8, dlc
                );
                for byte in data.iter().take(*dlc as usize) {
                    let _ = write!(s, "{byte:02X}");
                }
            }
            LogRecord::Signal {
                name,
                value,
                source,
            } => {
                let _ = write!(s, "SIGNAL name={name} value={value:.3} src=0x{source:02X}");
            }
            LogRecord::Heartbeat {
                bitrate,
                rx,
                j1939,
                errors,
            } => {
                let _ = write!(
                    s,
                    "HEARTBEAT bus={bitrate} rx={rx} j1939={j1939} err={errors}"
                );
            }
            LogRecord::TripEvent { what } => {
                let _ = write!(s, "TRIP what={what}");
            }
            LogRecord::VerifiedMileage { miles, confidence } => {
                let _ = write!(s, "MILEAGE miles={miles:.2} conf={confidence:.2}");
            }
        }
        s
    }
}

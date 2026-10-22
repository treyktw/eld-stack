//! Log file kinds, paths, and naming.
//!
//! Date-stamped names (YYYY-MM-DD) need a wall clock we don't have yet (no
//! RTC/GPS/NTP in early phases), so the prototype uses fixed per-kind names and
//! relies on size-based rotation. Switch to dated names once GPS/NTP lands.

use std::path::{Path, PathBuf};

/// VFS mount point for the SD card (see `sd::mount`).
pub const SD_ROOT: &str = "/sd";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogKind {
    /// Raw CAN frame dump.
    Can,
    /// Decoded signals + trip/mileage events.
    Events,
    /// Periodic heartbeat/diagnostics.
    Heartbeat,
    /// Boot/power/error system log.
    System,
    /// Pending upload queue (JSONL).
    Upload,
}

impl LogKind {
    /// Subdirectory under the storage root.
    pub fn dir(self) -> &'static str {
        match self {
            LogKind::Can => "can",
            LogKind::Events => "events",
            LogKind::Heartbeat => "system",
            LogKind::System => "system",
            LogKind::Upload => "upload",
        }
    }

    /// File name within the subdirectory.
    pub fn file(self) -> &'static str {
        match self {
            LogKind::Can => "raw.log",
            LogKind::Events => "signals.log",
            LogKind::Heartbeat => "heartbeat.log",
            LogKind::System => "system.log",
            LogKind::Upload => "pending.jsonl",
        }
    }

    /// All directories that must exist.
    pub const DIRS: [&'static str; 3] = ["can", "events", "system"];
}

/// Full path for a log kind under `root`.
pub fn path(root: &Path, kind: LogKind) -> PathBuf {
    root.join(kind.dir()).join(kind.file())
}

/// The upload-queue path under `root`.
pub fn upload_path(root: &Path) -> PathBuf {
    root.join(LogKind::Upload.dir())
        .join(LogKind::Upload.file())
}

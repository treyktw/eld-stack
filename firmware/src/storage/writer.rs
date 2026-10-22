//! Append records to storage.
//!
//! The writer is backend-agnostic. `Disabled` drops everything (used when SD
//! logging is off or the card failed to mount). `Fs` appends to files under a
//! root path — the SD VFS mount (`/sd`) on device, but any path works, so the
//! logic is identical with or without a card.
//!
//! Each append opens, writes one line + flush, and closes. Slower than holding
//! handles open, but power-loss safe: a yanked-power event can lose at most the
//! in-flight line, never a buffered backlog.

use std::io::Write;
use std::path::{Path, PathBuf};

use super::files::{self, LogKind};
use super::rotation;

pub enum Writer {
    Disabled,
    Fs { root: PathBuf },
}

impl Writer {
    pub fn disabled() -> Self {
        Writer::Disabled
    }

    /// Create an FS-backed writer rooted at `root`, ensuring the directory tree.
    pub fn fs(root: impl Into<PathBuf>) -> std::io::Result<Self> {
        let root = root.into();
        for dir in LogKind::DIRS {
            std::fs::create_dir_all(root.join(dir))?;
        }
        std::fs::create_dir_all(root.join(LogKind::Upload.dir()))?;
        Ok(Writer::Fs { root })
    }

    pub fn is_enabled(&self) -> bool {
        matches!(self, Writer::Fs { .. })
    }

    pub fn root(&self) -> Option<&Path> {
        match self {
            Writer::Fs { root } => Some(root),
            Writer::Disabled => None,
        }
    }

    /// Append a single line to the file for `kind`. Best-effort.
    pub fn append(&self, kind: LogKind, line: &str) {
        let Writer::Fs { root } = self else {
            return;
        };
        let path = files::path(root, kind);
        rotation::rotate_if_needed(&path);
        self.append_path(&path, line);
    }

    /// Append a line to an explicit path (used by the upload queue).
    pub fn append_path(&self, path: &Path, line: &str) {
        if !self.is_enabled() {
            return;
        }
        let result = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .and_then(|mut f| {
                f.write_all(line.as_bytes())?;
                f.write_all(b"\n")?;
                f.flush()
            });
        if result.is_err() {
            // Avoid log spam on a failing card; counters live in StorageEngine.
        }
    }
}

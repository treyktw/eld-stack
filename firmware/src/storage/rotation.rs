//! Size-based file rotation.
//!
//! When a log exceeds `MAX_BYTES`, the current file is renamed to `<file>.1`
//! (overwriting any previous `.1`) and a fresh file is started. Single-
//! generation rotation keeps SD usage bounded without unbounded history; bump
//! to N generations later if needed.

use std::path::Path;

/// Rotate a log once it passes this size.
pub const MAX_BYTES: u64 = 1024 * 1024; // 1 MiB

/// Rotate `path` if it is at/over `MAX_BYTES`. No-op if missing or smaller.
pub fn rotate_if_needed(path: &Path) {
    let len = match std::fs::metadata(path) {
        Ok(m) => m.len(),
        Err(_) => return,
    };
    if len < MAX_BYTES {
        return;
    }
    let rotated = path.with_extension("1");
    // Best-effort: drop the old generation, then move current aside.
    let _ = std::fs::remove_file(&rotated);
    let _ = std::fs::rename(path, &rotated);
}

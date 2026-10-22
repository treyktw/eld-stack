//! Boot recovery after power loss.
//!
//! ELDs lose power abruptly (ignition off, unplugged). Since the writer flushes
//! each line, files are always append-consistent — there's nothing to repair.
//! Recovery here is light: confirm the tree exists and report what survived
//! (sizes, pending-upload backlog) so boot diagnostics can flag a dirty restart.

use std::path::Path;

use super::files::{self, LogKind};

#[derive(Debug, Default, Clone, Copy)]
pub struct RecoveryReport {
    /// Bytes already in the events log from a prior session.
    pub events_bytes: u64,
    /// Bytes queued for upload but not yet sent.
    pub pending_upload_bytes: u64,
    /// True if any prior-session data was found (i.e. not a clean first boot).
    pub prior_session: bool,
}

/// Inspect existing logs under `root`. Read-only; never deletes.
pub fn recover(root: &Path) -> RecoveryReport {
    let size = |p: std::path::PathBuf| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);

    let events_bytes = size(files::path(root, LogKind::Events));
    let pending_upload_bytes = size(files::upload_path(root));
    let system_bytes = size(files::path(root, LogKind::System));

    RecoveryReport {
        events_bytes,
        pending_upload_bytes,
        prior_session: events_bytes > 0 || pending_upload_bytes > 0 || system_bytes > 0,
    }
}

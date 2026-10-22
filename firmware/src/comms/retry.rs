//! Exponential backoff for failed uploads.

use crate::config;

pub struct Backoff {
    base_ms: u64,
    max_ms: u64,
    current_ms: u64,
}

impl Backoff {
    pub fn new() -> Self {
        Self {
            base_ms: config::UPLOAD_BACKOFF_BASE_MS,
            max_ms: config::UPLOAD_BACKOFF_MAX_MS,
            current_ms: 0,
        }
    }

    /// Reset to no backoff (after a success).
    pub fn reset(&mut self) {
        self.current_ms = 0;
    }

    /// Next delay, doubling each call up to the cap.
    pub fn next_delay(&mut self) -> u64 {
        self.current_ms = if self.current_ms == 0 {
            self.base_ms
        } else {
            (self.current_ms * 2).min(self.max_ms)
        };
        self.current_ms
    }

    pub fn current_delay(&self) -> u64 {
        self.current_ms
    }
}

impl Default for Backoff {
    fn default() -> Self {
        Self::new()
    }
}

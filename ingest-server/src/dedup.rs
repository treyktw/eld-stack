//! In-memory fast-path dedup cache.
//!
//! Postgres `UNIQUE` is authoritative, but a bounded recent-`event_id` set lets
//! us skip a DB round-trip for obvious replays (common when both the ELD-direct
//! and phone-relay paths deliver the same event close together).

use std::collections::{HashSet, VecDeque};

pub struct DedupCache {
    seen: HashSet<String>,
    order: VecDeque<String>,
    capacity: usize,
}

impl DedupCache {
    pub fn new(capacity: usize) -> Self {
        Self {
            seen: HashSet::with_capacity(capacity),
            order: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    /// Whether this id has been seen, without recording it.
    ///
    /// Separate from [`Self::record`] on purpose. Recording an id before the
    /// database has accepted it means a failed insert still poisons the cache:
    /// the device retries, the cache reports a duplicate, the server answers
    /// "already have it", and the record exists nowhere.
    pub fn contains(&self, event_id: &str) -> bool {
        self.seen.contains(event_id)
    }

    /// Record an id that has been durably stored.
    pub fn record(&mut self, event_id: &str) {
        let _ = self.insert(event_id);
    }

    /// Returns true if newly inserted, false if it was already present.
    fn insert(&mut self, event_id: &str) -> bool {
        if self.seen.contains(event_id) {
            return false;
        }
        if self.order.len() >= self.capacity {
            if let Some(old) = self.order.pop_front() {
                self.seen.remove(&old);
            }
        }
        self.seen.insert(event_id.to_string());
        self.order.push_back(event_id.to_string());
        true
    }
}

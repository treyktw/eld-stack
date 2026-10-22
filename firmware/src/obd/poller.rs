//! Deciding what to ask the ECU next, and when.
//!
//! Pure: it takes the clock and returns an action. No CAN here, so the pacing
//! and the discovery-then-poll transition are testable without a truck.
//!
//! Two phases. First discovery — walk the support bitmaps to learn what this
//! particular vehicle answers. Then a steady round-robin over the intersection
//! of "supported" and "useful to us".
//!
//! The intersection matters. Asking for a PID the vehicle does not implement
//! costs a frame and a timeout on every cycle forever, and the captured 2013
//! Volvo answered only five of them. Asking for everything the vehicle *does*
//! implement is equally wrong — a truck advertising forty PIDs would have the
//! ones that matter polled forty times slower.

use super::support::{next_range_to_query, SupportedPids};

/// PIDs worth reading, in priority order.
///
/// Speed leads because it drives trip detection and mileage; a stale speed is
/// the reading most likely to produce a wrong duty-status inference.
pub const WANTED_PIDS: [u8; 9] = [
    0x0D, // vehicle speed
    0x0C, // engine rpm
    0xA6, // odometer
    0x1F, // run time since engine start
    0x2F, // fuel level
    0x5E, // engine fuel rate
    0x05, // coolant temperature
    0x42, // control module voltage
    0x11, // throttle position
];

/// Gap between requests.
///
/// One in flight at a time, paced. OBD-II has no flow control for concurrent
/// requests to a broadcast address, and firing them back to back means
/// responses from several PIDs interleaving with no reliable way to tell which
/// answer belongs to which question beyond the echoed PID byte.
pub const POLL_INTERVAL_MS: u64 = 250;

/// How long to wait for a support bitmap before retrying.
pub const DISCOVERY_TIMEOUT_MS: u64 = 2_000;

/// Discovery attempts before falling back.
///
/// A vehicle that never answers 0x00 is either not OBD-II or not awake. Falling
/// back to polling the wanted list blind is better than reporting nothing: the
/// worst case is unanswered requests, and the best case is a vehicle that
/// simply does not implement the support bitmap but does answer real PIDs.
pub const MAX_DISCOVERY_ATTEMPTS: u8 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PollAction {
    /// Ask for a support bitmap.
    Discover(u8),
    /// Ask for a data PID.
    Read(u8),
    /// Nothing due yet.
    Idle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Discovering,
    Polling,
}

pub struct ObdPoller {
    supported: SupportedPids,
    asked: heapless::Vec<u8, 8>,
    phase: Phase,
    /// Range currently awaited, and when it was asked.
    pending: Option<(u8, u64)>,
    attempts: u8,
    next_index: usize,
    last_request_ms: u64,
    /// Set when discovery gave up; the wanted list is polled unconditionally.
    blind: bool,
}

impl Default for ObdPoller {
    fn default() -> Self {
        Self::new()
    }
}

impl ObdPoller {
    pub fn new() -> Self {
        Self {
            supported: SupportedPids::new(),
            asked: heapless::Vec::new(),
            phase: Phase::Discovering,
            pending: None,
            attempts: 0,
            next_index: 0,
            last_request_ms: 0,
            blind: false,
        }
    }

    pub fn supported(&self) -> &SupportedPids {
        &self.supported
    }

    pub fn discovering(&self) -> bool {
        self.phase == Phase::Discovering
    }

    /// True when discovery finished without the ECU answering.
    pub fn is_blind(&self) -> bool {
        self.blind
    }

    /// Record a support bitmap response.
    pub fn on_support_response(&mut self, base: u8, data: [u8; 4]) {
        if self.pending.map(|(b, _)| b) == Some(base) {
            self.pending = None;
            self.attempts = 0;
        }
        self.supported.absorb_range(base, data);
        if self.asked.iter().all(|&a| a != base) {
            let _ = self.asked.push(base);
        }
    }

    /// What to do now.
    pub fn poll(&mut self, now_ms: u64) -> PollAction {
        if now_ms.saturating_sub(self.last_request_ms) < POLL_INTERVAL_MS {
            return PollAction::Idle;
        }

        if self.phase == Phase::Discovering {
            if let Some(action) = self.drive_discovery(now_ms) {
                return action;
            }
        }

        self.next_read(now_ms)
    }

    fn drive_discovery(&mut self, now_ms: u64) -> Option<PollAction> {
        // Still waiting on an outstanding range?
        if let Some((base, asked_at)) = self.pending {
            if now_ms.saturating_sub(asked_at) < DISCOVERY_TIMEOUT_MS {
                return Some(PollAction::Idle);
            }

            // Timed out.
            self.attempts = self.attempts.saturating_add(1);
            if self.attempts >= MAX_DISCOVERY_ATTEMPTS {
                self.blind = true;
                self.phase = Phase::Polling;
                self.pending = None;
                return None;
            }
            self.pending = Some((base, now_ms));
            self.last_request_ms = now_ms;
            return Some(PollAction::Discover(base));
        }

        match next_range_to_query(&self.supported, &self.asked) {
            Some(range) => {
                self.pending = Some((range, now_ms));
                self.last_request_ms = now_ms;
                Some(PollAction::Discover(range))
            }
            None => {
                self.phase = Phase::Polling;
                None
            }
        }
    }

    /// Next wanted PID this vehicle supports.
    ///
    /// Returns `Idle` rather than looping forever when nothing is both wanted
    /// and supported — a vehicle can legitimately offer none of them, and
    /// spinning the list on every tick would be a busy loop.
    fn next_read(&mut self, now_ms: u64) -> PollAction {
        for _ in 0..WANTED_PIDS.len() {
            let pid = WANTED_PIDS[self.next_index % WANTED_PIDS.len()];
            self.next_index = (self.next_index + 1) % WANTED_PIDS.len();

            if self.blind || self.supported.contains(pid) {
                self.last_request_ms = now_ms;
                return PollAction::Read(pid);
            }
        }
        PollAction::Idle
    }
}

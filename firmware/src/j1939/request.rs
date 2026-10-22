//! Request missing PGNs (J1939 Request PGN, 59904 / 0xEA00).
//!
//! `build_request` constructs the CAN frame. `Requester` tracks which wanted
//! PGNs we've seen recently and decides which are due for a (re)request,
//! honouring REQUEST_AFTER_MS (how stale before asking) and
//! REQUEST_INTERVAL_MS (minimum gap between repeats).

use esp_idf_svc::hal::can::Frame as HalFrame;
use heapless::Vec;

use super::known_pgns;
use crate::can::frame::CanFrame;
use crate::config;

/// Source address we transmit from. 0xF9 is a common diagnostic/service-tool SA.
pub const SOURCE_ADDR: u8 = 0xF9;
/// Global destination (all nodes).
pub const GLOBAL_DEST: u8 = 0xFF;
/// J1939 request priority.
pub const REQUEST_PRIORITY: u8 = 6;

/// Max wanted PGNs we track / request per pass.
const MAX_WANTED: usize = 8;
/// Max distinct ECU source addresses we'll target.
const MAX_SOURCES: usize = 16;
/// Global broadcast requests to try before escalating to per-ECU targeting.
const GLOBAL_ATTEMPTS_BEFORE_TARGETED: u32 = 3;

/// Build a Request-PGN frame asking `dest` to transmit `pgn`.
///
/// The request is PDU1 (PF 0xEA); the 3-byte payload is the requested PGN, LE.
pub fn build_request(pgn: u32, dest: u8, source: u8) -> Option<HalFrame> {
    let id: u32 = ((REQUEST_PRIORITY as u32 & 0x7) << 26)
        | (0xEA << 16)
        | ((dest as u32) << 8)
        | (source as u32);

    let data = [
        (pgn & 0xFF) as u8,
        ((pgn >> 8) & 0xFF) as u8,
        ((pgn >> 16) & 0xFF) as u8,
    ];

    CanFrame::extended(id, &data).ok()?.to_hal().ok()
}

struct WantedPgn {
    pgn: u32,
    last_seen_ms: Option<u64>,
    last_request_ms: Option<u64>,
    /// How many global (0xFF) requests we've sent for this PGN.
    global_attempts: u32,
    /// Round-robin cursor into the discovered source list for targeted requests.
    target_cursor: usize,
}

/// A due request: which PGN, and which ECU to ask (0xFF = all).
pub type DueRequest = (u32, u8);

pub struct Requester {
    wanted: Vec<WantedPgn, MAX_WANTED>,
}

impl Requester {
    /// Seed the wanted list from the known-PGN registry.
    pub fn new() -> Self {
        let mut wanted = Vec::new();
        for info in known_pgns::KNOWN {
            let _ = wanted.push(WantedPgn {
                pgn: info.pgn,
                last_seen_ms: None,
                last_request_ms: None,
                global_attempts: 0,
                target_cursor: 0,
            });
        }
        Self { wanted }
    }

    /// Note that `pgn` was just observed on the bus.
    pub fn mark_seen(&mut self, pgn: u32, now_ms: u64) {
        if let Some(w) = self.wanted.iter_mut().find(|w| w.pgn == pgn) {
            w.last_seen_ms = Some(now_ms);
        }
    }

    /// Requests due this pass, each paired with a destination.
    ///
    /// Strategy: a wanted PGN is first asked globally (0xFF) so every ECU that
    /// owns it replies. If it stays unseen after a few global tries, escalate to
    /// targeted requests, cycling through the `sources` discovered on the bus —
    /// one ECU per pass — to flush out reluctant or multiple responders.
    pub fn due(&mut self, now_ms: u64, sources: &[u8]) -> Vec<DueRequest, MAX_WANTED> {
        let mut out = Vec::new();
        for w in self.wanted.iter_mut() {
            let stale = match w.last_seen_ms {
                Some(t) => now_ms.saturating_sub(t) > config::REQUEST_AFTER_MS,
                None => now_ms > config::REQUEST_AFTER_MS,
            };
            let cooled = match w.last_request_ms {
                Some(t) => now_ms.saturating_sub(t) >= config::REQUEST_INTERVAL_MS,
                None => true,
            };
            if !(stale && cooled) {
                continue;
            }

            let dest = if w.global_attempts < GLOBAL_ATTEMPTS_BEFORE_TARGETED || sources.is_empty()
            {
                w.global_attempts += 1;
                GLOBAL_DEST
            } else {
                let n = sources.len().min(MAX_SOURCES);
                let sa = sources[w.target_cursor % n];
                w.target_cursor = w.target_cursor.wrapping_add(1);
                sa
            };

            w.last_request_ms = Some(now_ms);
            let _ = out.push((w.pgn, dest));
        }
        out
    }
}

impl Default for Requester {
    fn default() -> Self {
        Self::new()
    }
}

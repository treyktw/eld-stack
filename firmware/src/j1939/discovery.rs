//! Bus discovery: learn what a given truck actually offers.
//!
//! J1939 PGNs are standardized, but *availability* varies by manufacturer. We
//! passively record every (pgn, source) seen (via `SourceTracker`), collect
//! proprietary PGNs (0xEF00 / 0xFF00–0xFFFF) for later mining, and can tell how
//! many of our known/wanted PGNs are actually present.

use heapless::Vec;

use super::id::J1939Id;
use super::known_pgns::{self, KNOWN};
use super::source_address::SourceTracker;

/// Unrecognised PGNs kept for offline mining. Generous, because on an
/// unfamiliar truck these are exactly the ones worth having.
const MAX_UNRECOGNISED: usize = 64;

pub struct Discovery {
    sources: SourceTracker,
    /// Every PGN seen that no signal definition covers — proprietary ranges and
    /// standard PGNs alike. The old version tracked only the proprietary
    /// ranges, which meant a standard PGN this firmware does not yet decode
    /// went unrecorded, and an unfamiliar truck looked emptier than it was.
    unrecognised: Vec<u32, MAX_UNRECOGNISED>,
}

impl Discovery {
    pub fn new() -> Self {
        Self {
            sources: SourceTracker::new(),
            unrecognised: Vec::new(),
        }
    }

    /// Fold a parsed frame into the bus picture. `data` is retained per
    /// (pgn, source) so an unrecognised PGN can be decoded later at a desk.
    pub fn observe(&mut self, id: &J1939Id, data: &[u8], now_ms: u64) {
        self.sources.observe(id.pgn, id.source, data, now_ms);

        if known_pgns::name_of(id.pgn).is_none() && !self.unrecognised.contains(&id.pgn) {
            let _ = self.unrecognised.push(id.pgn);
        }
    }

    /// PGNs seen that this firmware has no definition for.
    pub fn unrecognised(&self) -> &[u32] {
        &self.unrecognised
    }

    /// The subset of unrecognised PGNs in the J1939 proprietary ranges, which
    /// are the manufacturer-specific ones worth reverse-engineering first.
    pub fn proprietary(&self) -> Vec<u32, MAX_UNRECOGNISED> {
        let mut out: Vec<u32, MAX_UNRECOGNISED> = Vec::new();
        for pgn in self
            .unrecognised
            .iter()
            .copied()
            .filter(|p| is_proprietary(*p))
        {
            let _ = out.push(pgn);
        }
        out
    }

    /// Full census, for export.
    pub fn census(&self) -> &SourceTracker {
        &self.sources
    }

    pub fn distinct_sources(&self) -> usize {
        self.sources.distinct_sources()
    }

    /// Distinct ECU source addresses seen (for targeted requests).
    pub fn source_addresses(&self) -> heapless::Vec<u8, 160> {
        self.sources.addresses()
    }

    /// How many of the known/wanted PGNs have actually been seen.
    pub fn known_pgns_seen(&self) -> usize {
        KNOWN
            .iter()
            .filter(|k| self.sources.entries().iter().any(|e| e.pgn == k.pgn))
            .count()
    }

    /// The census as JSON, for SD and upload.
    ///
    /// This is the artifact you take off an unfamiliar truck. Every (PGN,
    /// source) it emitted, how often, and the last payload — enough to decode a
    /// manufacturer-specific PGN at a desk instead of booking another vehicle.
    /// `known` marks the ones this firmware already has a definition for, so
    /// the gap is obvious at a glance.
    pub fn census_json(&self, limit: usize) -> String {
        let entries = self.sources.entries();
        let mut out = String::with_capacity(64 + entries.len().min(limit) * 96);
        out.push_str("{\"sources\":");
        out.push_str(&self.sources.distinct_sources().to_string());
        out.push_str(",\"pairs\":");
        out.push_str(&entries.len().to_string());
        out.push_str(",\"overflowed\":");
        out.push_str(&self.sources.overflowed().to_string());
        out.push_str(",\"unrecognised\":");
        out.push_str(&self.unrecognised.len().to_string());
        out.push_str(",\"entries\":[");

        for (index, entry) in entries.iter().take(limit).enumerate() {
            if index > 0 {
                out.push(',');
            }
            out.push_str("{\"pgn\":");
            out.push_str(&entry.pgn.to_string());
            out.push_str(",\"sa\":");
            out.push_str(&entry.source.to_string());
            out.push_str(",\"count\":");
            out.push_str(&entry.count.to_string());
            out.push_str(",\"first_ms\":");
            out.push_str(&entry.first_ms.to_string());
            out.push_str(",\"last_ms\":");
            out.push_str(&entry.last_ms.to_string());
            out.push_str(",\"period_ms\":");
            match entry.period_ms {
                Some(p) => out.push_str(&p.to_string()),
                None => out.push_str("null"),
            }
            out.push_str(",\"known\":");
            out.push_str(if known_pgns::name_of(entry.pgn).is_some() {
                "true"
            } else {
                "false"
            });
            out.push_str(",\"decoded\":");
            out.push_str(if super::signal_map::is_known(entry.pgn) {
                "true"
            } else {
                "false"
            });
            // Last payload as hex, so an unrecognised PGN is analysable later.
            out.push_str(",\"data\":\"");
            for byte in &entry.last_data[..entry.last_dlc as usize] {
                out.push(hex_nibble(byte >> 4));
                out.push(hex_nibble(byte & 0x0f));
            }
            out.push_str("\"}");
        }

        out.push_str("]}");
        out
    }
}

fn hex_nibble(value: u8) -> char {
    match value {
        0..=9 => (b'0' + value) as char,
        _ => (b'a' + (value - 10)) as char,
    }
}

impl Default for Discovery {
    fn default() -> Self {
        Self::new()
    }
}

/// J1939 proprietary ranges: Proprietary A (PF 0xEF) and Proprietary B (PF 0xFF).
fn is_proprietary(pgn: u32) -> bool {
    let pf = (pgn >> 8) & 0xFF;
    pf == 0xEF || pf == 0xFF
}

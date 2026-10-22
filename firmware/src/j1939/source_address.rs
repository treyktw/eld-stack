//! Bus census: every (PGN, source address) this truck actually emits.
//!
//! J1939 standardises what a PGN *means*, not which ones a given truck sends.
//! Manufacturers differ, model years differ, and the same logical value can
//! arrive from more than one ECU or not at all. A fixed table of "the eight
//! PGNs we care about" is therefore not portable — it silently produces a
//! device that works on the truck it was written against.
//!
//! So this records everything seen, known or not: per (PGN, source) the count,
//! first and last sighting, an estimated broadcast period, and the most recent
//! payload. That last field is what makes an unfamiliar truck analysable after
//! the fact — a proprietary PGN with sample bytes can be decoded at a desk
//! instead of requiring another trip to the vehicle.

use heapless::Vec;

/// Max distinct (pgn, source) pairs tracked.
///
/// A loaded heavy-truck bus can carry well over a hundred distinct pairs once
/// body controllers and trailer ABS are counted. Sized so a real truck fits;
/// `overflowed` records the truth if one does not.
const MAX_ENTRIES: usize = 160;

#[derive(Debug, Clone, Copy)]
pub struct SourceEntry {
    pub pgn: u32,
    pub source: u8,
    pub first_ms: u64,
    pub last_ms: u64,
    pub count: u32,
    /// Estimated broadcast period (ms), `None` until two sightings.
    pub period_ms: Option<u32>,
    /// Most recent payload, for offline mining of unrecognised PGNs.
    pub last_data: [u8; 8],
    pub last_dlc: u8,
}

pub struct SourceTracker {
    entries: Vec<SourceEntry, MAX_ENTRIES>,
    /// Distinct pairs that did not fit. Non-zero means this census is partial
    /// and MAX_ENTRIES needs raising — better to know than to quietly truncate.
    overflowed: u32,
}

impl SourceTracker {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            overflowed: 0,
        }
    }

    /// Record a frame sighting. Updates count, EWMA period, and last payload.
    pub fn observe(&mut self, pgn: u32, source: u8, data: &[u8], now_ms: u64) {
        let mut last_data = [0u8; 8];
        let dlc = data.len().min(8);
        last_data[..dlc].copy_from_slice(&data[..dlc]);

        if let Some(e) = self
            .entries
            .iter_mut()
            .find(|e| e.pgn == pgn && e.source == source)
        {
            let gap = now_ms.saturating_sub(e.last_ms) as u32;
            e.period_ms = Some(match e.period_ms {
                // EWMA, alpha = 1/4.
                Some(p) => (p * 3 + gap) / 4,
                None => gap,
            });
            e.last_ms = now_ms;
            e.count = e.count.wrapping_add(1);
            e.last_data = last_data;
            e.last_dlc = dlc as u8;
            return;
        }

        let entry = SourceEntry {
            pgn,
            source,
            first_ms: now_ms,
            last_ms: now_ms,
            count: 1,
            period_ms: None,
            last_data,
            last_dlc: dlc as u8,
        };
        if self.entries.push(entry).is_err() {
            self.overflowed = self.overflowed.wrapping_add(1);
        }
    }

    pub fn entries(&self) -> &[SourceEntry] {
        &self.entries
    }

    /// Distinct (pgn, source) pairs that did not fit in the census.
    pub fn overflowed(&self) -> u32 {
        self.overflowed
    }

    /// Distinct source addresses seen.
    pub fn addresses(&self) -> Vec<u8, MAX_ENTRIES> {
        let mut seen: Vec<u8, MAX_ENTRIES> = Vec::new();
        for e in &self.entries {
            if !seen.contains(&e.source) {
                let _ = seen.push(e.source);
            }
        }
        seen
    }

    /// Number of distinct source addresses seen.
    pub fn distinct_sources(&self) -> usize {
        self.addresses().len()
    }
}

impl Default for SourceTracker {
    fn default() -> Self {
        Self::new()
    }
}

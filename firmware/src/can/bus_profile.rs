//! Detected bus configuration snapshot.
//!
//! A flat, copyable summary of what the link + discovery learned, for
//! diagnostics and uploads. Built by the diagnostics layer from `CanManager`
//! (bitrate/mode) and `j1939::Discovery` (sources/PGNs).

use super::bitrate::CanBitrate;
use super::pins::CanWiring;

#[derive(Debug, Clone, Copy)]
pub struct BusProfile {
    pub bitrate: Option<CanBitrate>,
    pub listen_only: bool,
    pub wiring: CanWiring,
    /// Distinct ECU source addresses observed.
    pub distinct_sources: usize,
    /// Known/wanted PGNs actually present on this bus.
    pub known_pgns_seen: usize,
    /// Manufacturer-proprietary PGNs observed (for later mining).
    pub proprietary_count: usize,
}

impl BusProfile {
    pub fn new(
        bitrate: Option<CanBitrate>,
        listen_only: bool,
        wiring: CanWiring,
        distinct_sources: usize,
        known_pgns_seen: usize,
        proprietary_count: usize,
    ) -> Self {
        Self {
            bitrate,
            listen_only,
            wiring,
            distinct_sources,
            known_pgns_seen,
            proprietary_count,
        }
    }

    pub fn empty(wiring: CanWiring) -> Self {
        Self::new(None, false, wiring, 0, 0, 0)
    }

    pub fn is_up(self) -> bool {
        self.bitrate.is_some()
    }

    pub fn saw_j1939(self) -> bool {
        self.distinct_sources > 0 || self.known_pgns_seen > 0
    }

    pub fn known_coverage_pct(self, known_total: usize) -> f32 {
        if known_total == 0 {
            0.0
        } else {
            (self.known_pgns_seen as f32 / known_total as f32) * 100.0
        }
    }

    pub fn bitrate_label(self) -> &'static str {
        self.bitrate.map(|b| b.label()).unwrap_or("down")
    }
}

impl Default for BusProfile {
    fn default() -> Self {
        Self::empty(CanWiring::from_config())
    }
}

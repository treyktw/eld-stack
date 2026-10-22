//! Pure probe-result logic, split out so it can be unit-tested on the host.
//!
//! `channel.rs` re-exports these; the split exists because the selector needs
//! `crate::config` and the ESP toolchain, while deciding which pair to believe
//! is arithmetic on counters.

/// Connector pin pair, mirrored here to keep this file dependency-free.
pub use super::pins::ObdPinSet;

/// What a probe of one pair found.
#[derive(Debug, Clone, Copy)]
pub struct ChannelResult {
    pub pair: ObdPinSet,
    /// Bitrate that locked, if any.
    pub bitrate_label: Option<&'static str>,
    /// Extended (29-bit) frames seen — J1939 uses extended IDs.
    pub extended_frames: u32,
    /// Standard (11-bit) frames seen. Traffic here with no extended frames
    /// suggests a non-J1939 CAN channel, such as ISO 15765 / OBD.
    pub standard_frames: u32,
}

impl ChannelResult {
    pub fn none(pair: ObdPinSet) -> Self {
        Self {
            pair,
            bitrate_label: None,
            extended_frames: 0,
            standard_frames: 0,
        }
    }

    /// A pair worth using for J1939.
    pub fn carries_j1939(&self) -> bool {
        self.extended_frames > 0
    }

    /// Live CAN, but nothing with an extended ID. Worth recording separately:
    /// it means the wiring is good and the protocol is not what was expected,
    /// which is a very different problem from a dead pair.
    pub fn carries_non_j1939_can(&self) -> bool {
        self.extended_frames == 0 && self.standard_frames > 0
    }

    pub fn summary(&self) -> &'static str {
        if self.carries_j1939() {
            "j1939"
        } else if self.carries_non_j1939_can() {
            "can-but-not-j1939"
        } else if self.bitrate_label.is_some() {
            "locked-but-silent"
        } else {
            "no-traffic"
        }
    }
}

/// Pick the pair to run on: the first that yielded J1939.
pub fn best(results: &[ChannelResult]) -> Option<&ChannelResult> {
    results
        .iter()
        .find(|r| r.carries_j1939())
        .or_else(|| results.iter().find(|r| r.carries_non_j1939_can()))
}

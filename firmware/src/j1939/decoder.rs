//! Decode a J1939 frame into a typed signal.
//!
//! The decoder is pure: it returns an event and never logs or stores. Unknown
//! or unhandled PGNs become `UnknownPgn` so callers can track/mine them.

use super::id::J1939Id;
use super::signal_map::{self, SignalKind};
use super::spn;

/// A decoded value plus where it came from.
///
/// Provenance is not decoration: two PGNs can carry the same quantity at
/// different resolutions, and an ELD has to be able to say which source a
/// recorded odometer or engine-hours figure came from.
#[derive(Debug, Clone, Copy)]
pub enum J1939Signal {
    EngineRpm {
        rpm: f32,
        source: u8,
        spn: u32,
        priority: u8,
    },
    WheelSpeed {
        kph: f32,
        source: u8,
        spn: u32,
        priority: u8,
    },
    FuelLevel {
        percent: f32,
        source: u8,
        spn: u32,
        priority: u8,
    },
    EngineHours {
        hours: f32,
        source: u8,
        spn: u32,
        priority: u8,
    },
    Odometer {
        km: f32,
        miles: f32,
        source: u8,
        spn: u32,
        priority: u8,
    },
    UnknownPgn {
        pgn: u32,
        source: u8,
    },
}

impl J1939Signal {
    /// How much to trust this against another source of the same quantity.
    pub fn priority(&self) -> u8 {
        match *self {
            J1939Signal::EngineRpm { priority, .. }
            | J1939Signal::WheelSpeed { priority, .. }
            | J1939Signal::FuelLevel { priority, .. }
            | J1939Signal::EngineHours { priority, .. }
            | J1939Signal::Odometer { priority, .. } => priority,
            J1939Signal::UnknownPgn { .. } => 0,
        }
    }
}

/// Decode the payload of a parsed J1939 frame.
///
/// Table-driven: definitions live in `signal_map`, so supporting another
/// manufacturer is a table row rather than a new match arm. When a PGN yields
/// several signals the highest-priority one is returned; `decode_all` returns
/// every match.
pub fn decode(id: &J1939Id, data: &[u8]) -> J1939Signal {
    let mut best: Option<J1939Signal> = None;
    for def in signal_map::defs_for_pgn(id.pgn) {
        let Some(value) = def.read(data) else {
            continue;
        };
        let signal = build(def.kind, value, id.source, def.spn, def.priority);
        if best
            .as_ref()
            .map(|b| def.priority > b.priority())
            .unwrap_or(true)
        {
            best = Some(signal);
        }
    }

    best.unwrap_or(J1939Signal::UnknownPgn {
        pgn: id.pgn,
        source: id.source,
    })
}

/// Every signal a frame carries, for PGNs that pack more than one.
pub fn decode_all(id: &J1939Id, data: &[u8]) -> heapless::Vec<J1939Signal, 4> {
    let mut out: heapless::Vec<J1939Signal, 4> = heapless::Vec::new();
    for def in signal_map::defs_for_pgn(id.pgn) {
        if let Some(value) = def.read(data) {
            let _ = out.push(build(def.kind, value, id.source, def.spn, def.priority));
        }
    }
    out
}

fn build(kind: SignalKind, value: f32, source: u8, spn_id: u32, priority: u8) -> J1939Signal {
    match kind {
        SignalKind::EngineRpm => J1939Signal::EngineRpm {
            rpm: value,
            source,
            spn: spn_id,
            priority,
        },
        SignalKind::WheelSpeed => J1939Signal::WheelSpeed {
            kph: value,
            source,
            spn: spn_id,
            priority,
        },
        SignalKind::FuelLevel => J1939Signal::FuelLevel {
            percent: value,
            source,
            spn: spn_id,
            priority,
        },
        SignalKind::EngineHours => J1939Signal::EngineHours {
            hours: value,
            source,
            spn: spn_id,
            priority,
        },
        SignalKind::Odometer => J1939Signal::Odometer {
            km: value,
            miles: value / spn::KM_PER_MILE,
            source,
            spn: spn_id,
            priority,
        },
    }
}

/// Longest VIN string we keep (SAE VIN is 17 chars; allow slack for vendors
/// that pad or append).
pub const MAX_VIN: usize = 64;
/// Most active DTCs we surface from a DM1 message.
pub const MAX_DTCS: usize = 16;

pub type VinString = heapless::String<MAX_VIN>;
pub type Faults = heapless::Vec<Dtc, MAX_DTCS>;

/// One active diagnostic trouble code (DM1 entry).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Dtc {
    /// Suspect Parameter Number (the failing signal).
    pub spn: u32,
    /// Failure Mode Identifier (how it failed).
    pub fmi: u8,
    /// Occurrence count.
    pub oc: u8,
}

/// Decode a reassembled Vehicle Identification (PGN 65260) payload into the VIN
/// string. The field is ASCII terminated by `*` (and may be NUL/0xFF padded).
pub fn decode_vin(data: &[u8]) -> Option<VinString> {
    let mut vin = VinString::new();
    for &b in data {
        // Field terminator / padding ends the VIN.
        if matches!(b, b'*' | 0x00 | 0xFF) {
            break;
        }
        // Keep printable ASCII only; stop if we run out of room.
        if (0x20..=0x7E).contains(&b) && vin.push(b as char).is_err() {
            break;
        }
    }
    let trimmed = vin.trim();
    if trimmed.is_empty() {
        return None;
    }
    // Re-pack trimmed content (trim borrows; rebuild owned).
    let mut out = VinString::new();
    let _ = out.push_str(trimmed);
    Some(out)
}

/// Decode active DTCs from a DM1 (PGN 65226) payload. Works for both the
/// single-frame form and a reassembled multi-packet payload: two lamp-status
/// bytes followed by 4-byte DTC records. Uses the version-4 SPN conversion
/// (the de-facto modern encoding). Padding/empty DTCs (SPN 0) are skipped.
pub fn decode_dm1(data: &[u8]) -> Faults {
    let mut faults = Faults::new();
    if data.len() < 2 {
        return faults;
    }
    let mut i = 2; // skip the two lamp-status bytes
    while i + 4 <= data.len() {
        let b0 = data[i];
        let b1 = data[i + 1];
        let b2 = data[i + 2];
        let b3 = data[i + 3];
        i += 4;

        // All-0xFF (or all-zero) record marks the end / padding.
        if (b0 == 0xFF && b1 == 0xFF && b2 == 0xFF) || (b0 == 0 && b1 == 0 && b2 == 0) {
            continue;
        }

        let spn = (b0 as u32) | ((b1 as u32) << 8) | (((b2 >> 5) as u32) << 16);
        let fmi = b2 & 0x1F;
        let oc = b3 & 0x7F;
        if spn == 0 {
            continue;
        }
        if faults.push(Dtc { spn, fmi, oc }).is_err() {
            break;
        }
    }
    faults
}

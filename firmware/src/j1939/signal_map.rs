//! Declarative signal definitions.
//!
//! Decoding used to be a hand-written `match` on PGN with byte offsets inlined
//! at each arm. That works for one truck. Supporting a second manufacturer
//! meant editing decoder logic, and two PGNs carrying the same logical value
//! (total distance arrives as both 0xFEE0 and 0xFEC1) simply raced — whichever
//! frame landed last won, so a coarse 0.125 km/bit source could overwrite a
//! 5 m/bit one.
//!
//! Signals are data now. Adding a truck means adding rows: the PGN, where in
//! the payload the value sits, how to scale it, and how much to trust it
//! relative to other sources of the same quantity. Proprietary
//! manufacturer PGNs go in the same table once decoded from a bus census.

use super::pgn;
use super::spn;
use super::validity;

/// How a raw value is laid out in the payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Width {
    U8,
    U16Le,
    U32Le,
}

/// The logical quantity a definition produces. Several definitions may produce
/// the same kind from different PGNs; `priority` decides which is believed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalKind {
    EngineRpm,
    WheelSpeed,
    FuelLevel,
    EngineHours,
    Odometer,
}

#[derive(Debug, Clone, Copy)]
pub struct SignalDef {
    /// SAE suspect parameter number, for provenance.
    pub spn: u32,
    pub pgn: u32,
    /// Zero-based byte offset of the value within the payload.
    pub offset: usize,
    pub width: Width,
    /// Engineering units per raw bit.
    pub scale: f32,
    pub kind: SignalKind,
    /// Higher wins when two definitions supply the same kind. Resolution and
    /// directness of the source are what this encodes — not how recently the
    /// frame arrived.
    pub priority: u8,
}

/// Metres per kilometre, for the hi-res distance definition.
const M_PER_KM: f32 = 1000.0;

pub static SIGNALS: &[SignalDef] = &[
    SignalDef {
        spn: 190,
        pgn: pgn::EEC1,
        offset: 3,
        width: Width::U16Le,
        scale: spn::RPM_PER_BIT,
        kind: SignalKind::EngineRpm,
        priority: 10,
    },
    SignalDef {
        spn: 84,
        pgn: pgn::CCVS1,
        offset: 1,
        width: Width::U16Le,
        scale: spn::WHEEL_SPEED_KPH_PER_BIT,
        kind: SignalKind::WheelSpeed,
        priority: 10,
    },
    SignalDef {
        spn: 96,
        pgn: pgn::DASH_DISPLAY,
        offset: 1,
        width: Width::U8,
        scale: spn::FUEL_PCT_PER_BIT,
        kind: SignalKind::FuelLevel,
        priority: 10,
    },
    SignalDef {
        spn: 247,
        pgn: pgn::ENGINE_HOURS,
        offset: 0,
        width: Width::U32Le,
        scale: spn::ENGINE_HOURS_PER_BIT,
        kind: SignalKind::EngineHours,
        priority: 10,
    },
    // Total vehicle distance, 0.125 km/bit. The fallback: correct, but coarse.
    SignalDef {
        spn: 245,
        pgn: pgn::VEHICLE_DISTANCE,
        offset: 4,
        width: Width::U32Le,
        scale: spn::VEH_DIST_KM_PER_BIT,
        kind: SignalKind::Odometer,
        priority: 10,
    },
    // High-resolution total distance, 5 m/bit — 25x finer, so preferred
    // wherever the ECM offers it.
    SignalDef {
        spn: 917,
        pgn: pgn::HIRES_VEHICLE_DISTANCE,
        offset: 0,
        width: Width::U32Le,
        scale: spn::HIRES_DIST_M_PER_BIT / M_PER_KM,
        kind: SignalKind::Odometer,
        priority: 20,
    },
];

impl SignalDef {
    /// Extract this signal from a payload, honouring J1939 not-available and
    /// error encodings. `None` means the value is absent, never zero.
    pub fn read(&self, data: &[u8]) -> Option<f32> {
        let raw = match self.width {
            Width::U8 => validity::read_u8(data, self.offset)? as f32,
            Width::U16Le => validity::read_u16_le(data, self.offset)? as f32,
            Width::U32Le => validity::read_u32_le(data, self.offset)? as f32,
        };
        Some(raw * self.scale)
    }
}

/// Every definition that applies to a PGN. A PGN may carry several signals.
pub fn defs_for_pgn(pgn: u32) -> impl Iterator<Item = &'static SignalDef> {
    SIGNALS.iter().filter(move |def| def.pgn == pgn)
}

/// Whether any definition covers this PGN.
pub fn is_known(pgn: u32) -> bool {
    SIGNALS.iter().any(|def| def.pgn == pgn)
}

/// Highest priority among definitions producing a kind — the best this
/// firmware could do for that quantity if the truck offers every source.
pub fn best_priority_for(kind: SignalKind) -> u8 {
    SIGNALS
        .iter()
        .filter(|def| def.kind == kind)
        .map(|def| def.priority)
        .max()
        .unwrap_or(0)
}

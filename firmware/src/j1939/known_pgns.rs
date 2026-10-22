//! Registry of PGNs the firmware cares about.
//!
//! This is a *known-PGN table*, not an assumption that all exist on a given
//! bus. Discovery matches what's actually seen against this; the requester asks
//! for any that haven't appeared.

use super::pgn;

pub struct PgnInfo {
    pub pgn: u32,
    pub name: &'static str,
}

pub static KNOWN: &[PgnInfo] = &[
    PgnInfo {
        pgn: pgn::EEC1,
        name: "EEC1/rpm",
    },
    PgnInfo {
        pgn: pgn::CCVS1,
        name: "CCVS/speed",
    },
    PgnInfo {
        pgn: pgn::DASH_DISPLAY,
        name: "DASH/fuel",
    },
    PgnInfo {
        pgn: pgn::ENGINE_HOURS,
        name: "engine_hours",
    },
    PgnInfo {
        pgn: pgn::VEHICLE_DISTANCE,
        name: "odometer",
    },
    PgnInfo {
        pgn: pgn::HIRES_VEHICLE_DISTANCE,
        name: "odometer_hires",
    },
    PgnInfo {
        pgn: pgn::VEHICLE_ID,
        name: "vin",
    },
    PgnInfo {
        pgn: pgn::DM1,
        name: "dm1/faults",
    },
];

/// Friendly name for a PGN, if known.
pub fn name_of(pgn: u32) -> Option<&'static str> {
    KNOWN.iter().find(|p| p.pgn == pgn).map(|p| p.name)
}

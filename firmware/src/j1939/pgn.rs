//! Standard J1939 PGN constants (SAE-defined; identical across manufacturers).
//!
//! What varies per truck is *availability* — which of these an ECU actually
//! broadcasts, and proprietary PGNs in 0xFF00..=0xFFFF. See `known_pgns` and the
//! discovery logic for how we find what a given bus offers.

/// Electronic Engine Controller 1 (61444) — engine speed (RPM).
pub const EEC1: u32 = 0xF004;
/// Cruise Control / Vehicle Speed 1 (65265) — wheel-based speed.
pub const CCVS1: u32 = 0xFEF1;
/// Dash Display (65276) — fuel level.
pub const DASH_DISPLAY: u32 = 0xFEFC;
/// Engine Hours / Revolutions (65253) — total engine hours.
pub const ENGINE_HOURS: u32 = 0xFEE5;
/// Vehicle Distance (65248) — total + trip distance (0.125 km/bit).
pub const VEHICLE_DISTANCE: u32 = 0xFEE0;
/// High Resolution Vehicle Distance (65217) — total + trip (5 m/bit).
pub const HIRES_VEHICLE_DISTANCE: u32 = 0xFEC1;
/// Vehicle Identification (65260) — ASCII VIN string. Multi-packet (TP).
pub const VEHICLE_ID: u32 = 0xFEEC;
/// Active Diagnostic Trouble Codes, DM1 (65226) — single- or multi-packet.
pub const DM1: u32 = 0xFECA;

// ---- Transport Protocol (multi-packet) ------------------------------------
// PDU1 PGNs: destination is carried in the ID's PS byte, not the PGN. BAM uses
// the global destination 0xFF.

/// TP Connection Management (60416) — BAM/RTS/CTS/EndOfMsg/Abort control.
pub const TP_CM: u32 = 0xEC00;
/// TP Data Transfer (60160) — sequenced 7-byte data packets.
pub const TP_DT: u32 = 0xEB00;

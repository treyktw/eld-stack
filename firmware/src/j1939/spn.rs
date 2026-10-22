//! SPN scaling factors for the signals we decode.
//!
//! Each constant is the engineering value per raw bit, per the SAE J1939-71
//! definition for that SPN. Offsets are 0 for all of these.

/// SPN 190 — Engine speed (EEC1). 0.125 rpm/bit.
pub const RPM_PER_BIT: f32 = 0.125;
/// SPN 84 — Wheel-based vehicle speed (CCVS1). 1/256 km/h per bit.
pub const WHEEL_SPEED_KPH_PER_BIT: f32 = 1.0 / 256.0;
/// SPN 96 — Fuel level (Dash Display). 0.4 %/bit.
pub const FUEL_PCT_PER_BIT: f32 = 0.4;
/// SPN 247 — Engine total hours of operation. 0.05 h/bit.
pub const ENGINE_HOURS_PER_BIT: f32 = 0.05;
/// SPN 245 — Total vehicle distance (VEHICLE_DISTANCE). 0.125 km/bit.
pub const VEH_DIST_KM_PER_BIT: f32 = 0.125;
/// SPN 917 — Total HiRes vehicle distance (HIRES_VEHICLE_DISTANCE). 5 m/bit.
pub const HIRES_DIST_M_PER_BIT: f32 = 5.0;

/// Exact kilometres per statute mile (for odometer mi conversion).
pub const KM_PER_MILE: f32 = 1.609_344;

//! Host-side candump replay.
//!
//! Feeds a `candump -L` style log through the firmware's pure decoders, in the
//! same order `App::handle_frame` does on the device:
//!
//! 1. 11-bit frames go to the OBD-II Mode 01 parser.
//! 2. 29-bit frames are parsed as J1939 IDs, fed to the TP/BAM reassembler
//!    (VIN, multi-DTC DM1), then single-frame DM1, then the table-driven
//!    signal decoder.
//! 3. Odometer and wheel speed feed the mileage verifier, so the replay ends
//!    with the same odometer-vs-speed-integration cross-check the device logs
//!    at trip end.
//!
//! Nothing here is firmware code; it only drives firmware code.

use crate::decoder::{self, Dtc, J1939Signal};
use crate::id::J1939Id;
use crate::known_pgns;
use crate::obd_pid::{self, Value};
use crate::pgn;
use crate::telemetry_store::distance::DistanceState;
use crate::transport::TransportReassembler;
use crate::verifier::mileage::VerifiedMileage;
use crate::verifier::VerifierEngine;

/// One parsed log line.
#[derive(Debug, Clone, PartialEq)]
pub struct LogFrame {
    pub ts_ms: u64,
    pub id: u32,
    pub extended: bool,
    pub data: Vec<u8>,
}

/// Parse `(1700000000.123456) can0 18FEF100#FF00320000000000`.
///
/// The ID width decides 11- vs 29-bit, exactly as candump writes it: three hex
/// digits for a standard frame, eight for an extended one. Comment lines (`#`)
/// and blanks are skipped by the caller.
pub fn parse_line(line: &str) -> Option<LogFrame> {
    let line = line.trim();
    let (ts, rest) = line.strip_prefix('(')?.split_once(')')?;
    let (secs, frac) = ts.split_once('.').unwrap_or((ts, "0"));
    let micros: u64 = format!("{frac:0<6}")[..6].parse().ok()?;
    let ts_ms = secs.parse::<u64>().ok()? * 1000 + micros / 1000;

    let mut parts = rest.split_whitespace();
    let _iface = parts.next()?;
    let frame = parts.next()?;
    let (id_hex, data_hex) = frame.split_once('#')?;
    let extended = id_hex.len() > 3;
    let id = u32::from_str_radix(id_hex, 16).ok()?;
    if data_hex.len() % 2 != 0 || data_hex.len() > 16 {
        return None;
    }
    let data = (0..data_hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&data_hex[i..i + 2], 16))
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    Some(LogFrame {
        ts_ms,
        id,
        extended,
        data,
    })
}

/// What a replay produced. Every field is the *latest* value, the way the
/// device's telemetry store holds "current truth".
#[derive(Debug, Default)]
pub struct ReplaySummary {
    pub frames: usize,
    pub j1939_frames: usize,
    pub obd_frames: usize,
    pub skipped_lines: usize,
    pub vin: Option<String>,
    pub faults: Vec<Dtc>,
    pub rpm: Option<f32>,
    pub speed_kph: Option<f32>,
    pub fuel_percent: Option<f32>,
    pub engine_hours: Option<f32>,
    pub odometer_km: Option<f32>,
    pub obd_readings: Vec<(u16, Value)>,
    pub mileage: Option<VerifiedMileage>,
    /// Human-readable decode log, one entry per interesting event.
    pub events: Vec<String>,
}

pub fn replay(log: &str) -> ReplaySummary {
    let mut out = ReplaySummary::default();
    let mut tp = TransportReassembler::new();
    let mut distance = DistanceState::default();
    let mut verifier = VerifierEngine::new();
    let mut verifier_started = false;
    let mut first_ts: Option<u64> = None;

    for raw in log.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some(frame) = parse_line(line) else {
            out.skipped_lines += 1;
            continue;
        };
        out.frames += 1;
        let t0 = *first_ts.get_or_insert(frame.ts_ms);
        let t = frame.ts_ms - t0;

        if !frame.extended {
            out.obd_frames += 1;
            if let Ok(reading) = obd_pid::parse_response(frame.id as u16, &frame.data) {
                out.events.push(format!(
                    "{t:>6} ms  OBD   0x{:03X} pid=0x{:02X} {:?}",
                    reading.source, reading.pid, reading.value
                ));
                out.obd_readings.push((reading.source, reading.value));
            }
            continue;
        }

        let Some(id) = J1939Id::parse(frame.id) else {
            out.skipped_lines += 1;
            continue;
        };
        out.j1939_frames += 1;

        if let Some(msg) = tp.ingest(&id, &frame.data) {
            match msg.pgn {
                pgn::VEHICLE_ID => {
                    if let Some(vin) = decoder::decode_vin(&msg.data) {
                        out.events.push(format!(
                            "{t:>6} ms  TP    VIN reassembled from SA 0x{:02X}: {}",
                            msg.source,
                            vin.as_str()
                        ));
                        out.vin = Some(vin.as_str().to_owned());
                    }
                }
                pgn::DM1 => {
                    let faults = decoder::decode_dm1(&msg.data);
                    out.events.push(format!(
                        "{t:>6} ms  TP    DM1 reassembled from SA 0x{:02X}: {} active DTC(s)",
                        msg.source,
                        faults.len()
                    ));
                    out.faults = faults.to_vec();
                }
                other => out.events.push(format!(
                    "{t:>6} ms  TP    PGN 0x{other:04X} reassembled ({} bytes)",
                    msg.data.len()
                )),
            }
        }
        if id.pgn == pgn::DM1 {
            let faults = decoder::decode_dm1(&frame.data);
            if !faults.is_empty() {
                out.events.push(format!(
                    "{t:>6} ms  DM1   single-frame from SA 0x{:02X}: {:?}",
                    id.source,
                    faults.as_slice()
                ));
                out.faults = faults.to_vec();
            }
        }

        let signals = decoder::decode_all(&id, &frame.data);
        for signal in &signals {
            let name = known_pgns::name_of(id.pgn).unwrap_or("?");
            let mut believed = true;
            match *signal {
                J1939Signal::EngineRpm { rpm, .. } => out.rpm = Some(rpm),
                J1939Signal::WheelSpeed { kph, .. } => {
                    out.speed_kph = Some(kph);
                }
                J1939Signal::FuelLevel { percent, .. } => out.fuel_percent = Some(percent),
                J1939Signal::EngineHours { hours, .. } => out.engine_hours = Some(hours),
                J1939Signal::Odometer { km, priority, .. } => {
                    if distance.set_odometer(km, priority) {
                        out.odometer_km = Some(km);
                    } else {
                        believed = false;
                        out.events.push(format!(
                            "{t:>6} ms  J1939 PGN 0x{:04X} ({name}) SA 0x{:02X} {} -> ignored, \
                             a higher-resolution odometer is already believed",
                            id.pgn,
                            id.source,
                            describe(signal)
                        ));
                    }
                }
                J1939Signal::UnknownPgn { .. } => {}
            }
            if believed && !matches!(signal, J1939Signal::UnknownPgn { .. }) {
                out.events.push(format!(
                    "{t:>6} ms  J1939 PGN 0x{:04X} ({name}) SA 0x{:02X} {}",
                    id.pgn,
                    id.source,
                    describe(signal)
                ));
            }
        }

        // Mileage verification window: opens on the first odometer reading,
        // like a trip starting, and is fed every speed/odometer update.
        if !verifier_started && distance.odometer_km.is_some() {
            verifier.begin(distance.odometer_km, None, t);
            verifier_started = true;
        }
        if verifier_started {
            verifier.update(out.speed_kph, distance.odometer_km, None, t);
        }
    }

    if verifier_started {
        out.mileage = Some(verifier.finalize());
    }
    out
}

fn describe(signal: &J1939Signal) -> String {
    match *signal {
        J1939Signal::EngineRpm { rpm, spn, .. } => format!("rpm={rpm:.1} (SPN {spn})"),
        J1939Signal::WheelSpeed { kph, spn, .. } => format!("speed={kph:.2} km/h (SPN {spn})"),
        J1939Signal::FuelLevel { percent, spn, .. } => format!("fuel={percent:.1}% (SPN {spn})"),
        J1939Signal::EngineHours { hours, spn, .. } => format!("hours={hours:.2} (SPN {spn})"),
        J1939Signal::Odometer {
            km, miles, spn, ..
        } => format!("odometer={km:.3} km / {miles:.3} mi (SPN {spn})"),
        J1939Signal::UnknownPgn { pgn, .. } => format!("unknown PGN 0x{pgn:04X}"),
    }
}

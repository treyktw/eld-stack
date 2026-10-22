//! OBD-II (SAE J1979) Mode 01 request/response handling.
//!
//! Pure — no CAN driver — so the frame layout and every scaling factor are
//! testable. A wrong divisor here does not fail loudly; it produces a plausible
//! number that ends up in an hours-of-service record.
//!
//! # Addressing
//!
//! Requests go to `0x7DF`, the functional broadcast address, on **11-bit**
//! standard identifiers. Every ECU that implements the PID answers on its own
//! physical address in `0x7E8..=0x7EF`.
//!
//! Note the contrast with an earlier Arduino prototype, which despite its
//! OBD-flavoured name sets `extd = 1` and decodes J1939 — that is 29-bit J1939
//! arriving on an OBD-shaped J1962 connector. The connector is not the protocol. This module
//! is the real OBD-II path, matching an earlier PID-support scan sketch and the
//! `0x7E8` responses in a captured OBD-II session from a 2013 Volvo tractor.
//!
//! # Multiple responders
//!
//! A broadcast request can be answered by several ECUs. They are not
//! interchangeable: engine and transmission both report speed and can disagree.
//! Every decoded value therefore carries the address that produced it, so a
//! caller can prefer a specific ECU rather than accepting whichever replied last.

/// Functional broadcast request address.
pub const REQUEST_ID: u16 = 0x7DF;

/// Range of physical response addresses.
pub const RESPONSE_ID_MIN: u16 = 0x7E8;
pub const RESPONSE_ID_MAX: u16 = 0x7EF;

/// Mode 01: current data.
pub const MODE_CURRENT_DATA: u8 = 0x01;

/// A positive response echoes the mode with 0x40 added.
pub const MODE_RESPONSE_OFFSET: u8 = 0x40;

/// A decoded value, in its natural unit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Value {
    Rpm(f32),
    SpeedKph(f32),
    CoolantTempC(f32),
    ThrottlePercent(f32),
    ControlModuleVoltage(f32),
    FuelLevelPercent(f32),
    FuelRateLph(f32),
    /// Seconds since the engine started.
    RunTimeSeconds(u32),
    /// Kilometres. Rarely supported on heavy vehicles — see `odometer`.
    OdometerKm(f32),
    DistanceSinceCodesClearedKm(f32),
    EngineLoadPercent(f32),
    IntakeTempC(f32),
    MafGramsPerSecond(f32),
    BarometricPressureKpa(f32),
    /// A supported PID with no decoder here yet. Kept rather than dropped so
    /// discovery can report what a truck offers that we do not read.
    Unknown {
        pid: u8,
    },
}

/// One decoded response.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Reading {
    /// Which ECU answered, e.g. `0x7E8`.
    pub source: u16,
    pub pid: u8,
    pub value: Value,
}

/// Build a Mode 01 request frame for `pid`.
///
/// Eight bytes, zero padded. The leading byte is the number of meaningful bytes
/// that follow, which is 2 for a Mode 01 single-PID request.
pub fn request_frame(pid: u8) -> [u8; 8] {
    [0x02, MODE_CURRENT_DATA, pid, 0, 0, 0, 0, 0]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseError {
    /// Identifier outside `0x7E8..=0x7EF`.
    NotAResponseAddress,
    /// Frame too short to contain a length, mode and PID.
    Truncated,
    /// Length byte disagrees with the frame.
    BadLength,
    /// Mode was not `0x41`.
    NotCurrentData,
    /// Fewer payload bytes than the PID requires.
    ShortPayload,
}

/// Parse one CAN frame into a reading.
///
/// Rejects rather than guesses. A frame that does not look exactly like a Mode
/// 01 response is discarded — this bus carries traffic from many ECUs, and
/// loosely accepting something that merely resembles a response is how a
/// transmission temperature becomes a road speed.
pub fn parse_response(id: u16, frame: &[u8]) -> Result<Reading, ParseError> {
    if !(RESPONSE_ID_MIN..=RESPONSE_ID_MAX).contains(&id) {
        return Err(ParseError::NotAResponseAddress);
    }
    if frame.len() < 3 {
        return Err(ParseError::Truncated);
    }

    let length = frame[0] as usize;
    // Length counts mode + pid + data. Two is the minimum (mode + pid) and it
    // cannot exceed what the frame actually carries.
    if length < 2 || length > frame.len() - 1 {
        return Err(ParseError::BadLength);
    }
    if frame[1] != MODE_CURRENT_DATA + MODE_RESPONSE_OFFSET {
        return Err(ParseError::NotCurrentData);
    }

    let pid = frame[2];
    let data = &frame[3..1 + length];

    Ok(Reading {
        source: id,
        pid,
        value: decode(pid, data)?,
    })
}

/// How many payload bytes each known PID carries.
pub fn payload_len(pid: u8) -> usize {
    match pid {
        0x04 | 0x05 | 0x0D | 0x0F | 0x11 | 0x2F | 0x33 => 1,
        0x0C | 0x10 | 0x1F | 0x31 | 0x5E => 2,
        0x42 => 2,
        0xA6 => 4,
        _ => 0,
    }
}

fn u16_be(data: &[u8]) -> u32 {
    ((data[0] as u32) << 8) | data[1] as u32
}

/// Decode a payload for `pid`.
///
/// Every scaling factor here is from SAE J1979. They are written as explicit
/// arithmetic rather than table constants so each one can be read against the
/// standard without following an indirection.
pub fn decode(pid: u8, data: &[u8]) -> Result<Value, ParseError> {
    let need = payload_len(pid);
    if need > 0 && data.len() < need {
        return Err(ParseError::ShortPayload);
    }

    Ok(match pid {
        // ((A * 256) + B) / 4
        0x0C => Value::Rpm(u16_be(data) as f32 / 4.0),
        // A, km/h
        0x0D => Value::SpeedKph(data[0] as f32),
        // A - 40, °C. The offset is why a raw 0 is -40 and not freezing point.
        0x05 => Value::CoolantTempC(data[0] as f32 - 40.0),
        0x0F => Value::IntakeTempC(data[0] as f32 - 40.0),
        // A * 100 / 255
        0x11 => Value::ThrottlePercent(data[0] as f32 * 100.0 / 255.0),
        0x04 => Value::EngineLoadPercent(data[0] as f32 * 100.0 / 255.0),
        0x2F => Value::FuelLevelPercent(data[0] as f32 * 100.0 / 255.0),
        // ((A * 256) + B) / 1000, volts
        0x42 => Value::ControlModuleVoltage(u16_be(data) as f32 / 1000.0),
        // ((A * 256) + B) / 100, g/s
        0x10 => Value::MafGramsPerSecond(u16_be(data) as f32 / 100.0),
        // ((A * 256) + B) / 20, L/h
        0x5E => Value::FuelRateLph(u16_be(data) as f32 / 20.0),
        // (A * 256) + B, seconds
        0x1F => Value::RunTimeSeconds(u16_be(data)),
        // (A * 256) + B, km
        0x31 => Value::DistanceSinceCodesClearedKm(u16_be(data) as f32),
        // A, kPa
        0x33 => Value::BarometricPressureKpa(data[0] as f32),
        // 32-bit, tenths of a km
        0xA6 => Value::OdometerKm(
            (((data[0] as u32) << 24)
                | ((data[1] as u32) << 16)
                | ((data[2] as u32) << 8)
                | data[3] as u32) as f32
                / 10.0,
        ),
        other => Value::Unknown { pid: other },
    })
}

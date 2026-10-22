//! Minimal NMEA 0183 parser (GGA + RMC), checksum-validated.
//!
//! Pure: takes a line, returns a typed sentence or `None`. Coordinates are
//! converted from `ddmm.mmmm` + hemisphere to signed decimal degrees.
//!
//! RMC is parsed for its **date**. GGA carries a time of day but no date, so it
//! cannot anchor a UTC clock on its own — and 49 CFR 395 Appendix A requires
//! event timestamps within ten minutes of UTC. Speed still comes from the CAN
//! bus; RMC's speed-over-ground is deliberately ignored, because engine
//! synchronization is the regulatory source for vehicle motion.

use crate::time::UtcDateTime;

#[derive(Debug, Clone, Copy)]
pub enum Nmea {
    /// Global positioning fix data.
    Gga {
        fix_quality: u8,
        hdop: Option<f32>,
        lat: Option<f64>,
        lon: Option<f64>,
    },
    /// Recommended minimum data. The only sentence here that carries a date.
    Rmc {
        /// `true` only when the receiver reports status "A" (valid).
        valid: bool,
        datetime: Option<UtcDateTime>,
        lat: Option<f64>,
        lon: Option<f64>,
    },
}

/// Parse a single NMEA line. Returns `None` if malformed, checksum-bad, or an
/// unsupported sentence type.
pub fn parse(line: &str) -> Option<Nmea> {
    let line = line.trim();
    let body = line.strip_prefix('$')?;

    // Split off and verify the checksum (if present).
    let (payload, checksum) = match body.split_once('*') {
        Some((p, c)) => (p, Some(c)),
        None => (body, None),
    };
    if let Some(cs) = checksum {
        let expected = u8::from_str_radix(cs.trim(), 16).ok()?;
        let actual = payload.bytes().fold(0u8, |acc, b| acc ^ b);
        if actual != expected {
            return None;
        }
    }

    let mut f = payload.split(',');
    let kind = f.next()?;
    // Match the 3-char sentence id, ignoring the talker prefix (GP/GN/GL/...).
    let id = &kind[kind.len().saturating_sub(3)..];

    match id {
        "GGA" => {
            let _utc = f.next();
            let lat = coord(f.next(), f.next());
            let lon = coord(f.next(), f.next());
            let fix_quality = f.next().and_then(|s| s.parse().ok()).unwrap_or(0);
            let _satellites = f.next(); // count not consumed yet
            let hdop = f.next().and_then(|s| s.parse().ok());
            Some(Nmea::Gga {
                fix_quality,
                hdop,
                lat,
                lon,
            })
        }
        "RMC" => {
            let time = f.next();
            let valid = matches!(f.next(), Some("A"));
            let lat = coord(f.next(), f.next());
            let lon = coord(f.next(), f.next());
            let _speed_knots = f.next(); // motion comes from the ECM, not GPS
            let _track_angle = f.next();
            let date = f.next();
            Some(Nmea::Rmc {
                valid,
                datetime: datetime(date, time),
                lat,
                lon,
            })
        }
        _ => None,
    }
}

/// Combine RMC's `ddmmyy` date and `hhmmss.sss` time into a UTC datetime.
///
/// Returns `None` on anything unparseable rather than guessing: a wrong date
/// silently mis-files a duty record on the wrong log day.
fn datetime(date: Option<&str>, time: Option<&str>) -> Option<UtcDateTime> {
    let date = date?;
    let time = time?;
    if date.len() < 6 || time.len() < 6 {
        return None;
    }
    if !date.is_char_boundary(6) || !time.is_char_boundary(6) {
        return None;
    }

    let day: u8 = date.get(0..2)?.parse().ok()?;
    let month: u8 = date.get(2..4)?.parse().ok()?;
    let year_two: u16 = date.get(4..6)?.parse().ok()?;

    let hour: u8 = time.get(0..2)?.parse().ok()?;
    let minute: u8 = time.get(2..4)?.parse().ok()?;
    let second: u8 = time.get(4..6)?.parse().ok()?;
    let millis: u16 = match time.get(6..) {
        Some(rest) if rest.starts_with('.') => {
            let frac = rest.get(1..).unwrap_or("");
            let digits: String = frac.chars().take(3).collect();
            if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
                0
            } else {
                // ".5" is 500 ms, not 5 ms.
                let scaled = format!("{digits:0<3}");
                scaled.parse().unwrap_or(0)
            }
        }
        _ => 0,
    };

    Some(UtcDateTime {
        // NMEA gives a two-digit year. `UtcDateTime::to_unix_ms` rejects
        // anything before 2020, which catches a receiver reporting a
        // pre-epoch default rather than letting it anchor the clock.
        year: 2000 + year_two,
        month,
        day,
        hour,
        minute,
        second,
        millis,
    })
}

/// Convert `ddmm.mmmm` + hemisphere (N/S/E/W) to signed decimal degrees.
fn coord(value: Option<&str>, hemi: Option<&str>) -> Option<f64> {
    let v = value?;
    if v.is_empty() {
        return None;
    }
    let raw: f64 = v.parse().ok()?;
    // Degrees are the integer hundreds; the remainder is decimal minutes.
    let deg = (raw / 100.0).trunc();
    let minutes = raw - deg * 100.0;
    let mut dd = deg + minutes / 60.0;
    if matches!(hemi, Some("S") | Some("W")) {
        dd = -dd;
    }
    Some(dd)
}

//! Monotonic and UTC wall-clock helpers.
//!
//! Backed by the ESP-IDF high-resolution timer (`esp_timer_get_time`), which
//! counts microseconds since boot and is monotonic (not affected by RTC/NTP
//! changes). Timeout math must continue to use monotonic time. Regulatory event
//! timestamps come from `UtcClock`, which is synchronized from a validated GPS
//! date/time and deliberately becomes unsynchronized when holdover is too old.

#[cfg(target_os = "espidf")]
use esp_idf_svc::sys::esp_timer_get_time;

/// We do not claim a timestamp remains within the Appendix A ten-minute UTC
/// tolerance after this much time without a fresh GPS time observation.
pub const MAX_UTC_HOLDOVER_MS: u64 = 10 * 60 * 1_000;
const MIN_UTC_MS: u64 = 1_577_836_800_000; // 2020-01-01T00:00:00Z
const MAX_UTC_MS: u64 = 4_102_444_800_000; // 2100-01-01T00:00:00Z

/// Microseconds since boot.
#[inline]
#[cfg(target_os = "espidf")]
pub fn now_us() -> u64 {
    // SAFETY: FFI call with no arguments, always valid after IDF init.
    unsafe { esp_timer_get_time() as u64 }
}

/// Milliseconds since boot.
#[inline]
#[cfg(target_os = "espidf")]
pub fn now_ms() -> u64 {
    now_us() / 1_000
}

/// Milliseconds elapsed since an earlier `now_ms()` reading.
#[inline]
#[cfg(target_os = "espidf")]
pub fn elapsed_ms(since_ms: u64) -> u64 {
    now_ms().saturating_sub(since_ms)
}

/// The time a record was written.
///
/// Both clocks are recorded deliberately. `monotonic_ms` is uptime and always
/// available; `utc_ms` is present only while the device can stand behind the
/// 49 CFR 395 Appendix A ten-minute UTC tolerance. Emitting the sync state
/// alongside them means a record written before the first GPS fix can never be
/// mistaken for one with a real wall-clock time — which is what happened when
/// every record was stamped with uptime alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RecordStamp {
    pub monotonic_ms: u64,
    pub utc_ms: Option<u64>,
}

impl RecordStamp {
    pub const fn unsynchronized(monotonic_ms: u64) -> Self {
        Self {
            monotonic_ms,
            utc_ms: None,
        }
    }

    /// Leading fields shared by every record line.
    pub fn prefix(&self) -> String {
        match self.utc_ms {
            Some(utc) => format!("{utc} up={} clk=utc ", self.monotonic_ms),
            // The leading field stays positional, so a reader that only wants
            // "when" still gets a number — but `clk=unsync` says plainly that
            // it is uptime, not wall-clock time.
            None => format!("{} up={} clk=unsync ", self.monotonic_ms, self.monotonic_ms),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClockStatus {
    Unsynchronized,
    GpsSynchronized,
    Holdover,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClockReading {
    pub utc_ms: Option<u64>,
    pub status: ClockStatus,
    pub gps_age_ms: Option<u64>,
}

/// A UTC clock anchored to monotonic time. It never substitutes uptime for UTC.
#[derive(Debug, Clone, Copy, Default)]
pub struct UtcClock {
    anchor: Option<UtcAnchor>,
}

#[derive(Debug, Clone, Copy)]
struct UtcAnchor {
    utc_ms: u64,
    monotonic_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClockSyncError {
    InvalidUtc,
    MonotonicWentBackwards,
    ImplausibleCorrection,
}

impl UtcClock {
    pub const fn new() -> Self {
        Self { anchor: None }
    }

    /// Accept a GPS UTC observation after calendar/NMEA validation. Once
    /// running, reject corrections larger than the regulatory UTC tolerance;
    /// those must be diagnosed rather than silently rewriting event time.
    pub fn synchronize(
        &mut self,
        gps_utc_ms: u64,
        monotonic_ms: u64,
    ) -> Result<(), ClockSyncError> {
        if !(MIN_UTC_MS..MAX_UTC_MS).contains(&gps_utc_ms) {
            return Err(ClockSyncError::InvalidUtc);
        }
        if let Some(anchor) = self.anchor {
            if monotonic_ms < anchor.monotonic_ms {
                return Err(ClockSyncError::MonotonicWentBackwards);
            }
            let predicted = anchor
                .utc_ms
                .saturating_add(monotonic_ms - anchor.monotonic_ms);
            if predicted.abs_diff(gps_utc_ms) > MAX_UTC_HOLDOVER_MS {
                return Err(ClockSyncError::ImplausibleCorrection);
            }
        }
        self.anchor = Some(UtcAnchor {
            utc_ms: gps_utc_ms,
            monotonic_ms,
        });
        Ok(())
    }

    pub fn reading(&self, monotonic_ms: u64) -> ClockReading {
        let Some(anchor) = self.anchor else {
            return ClockReading {
                utc_ms: None,
                status: ClockStatus::Unsynchronized,
                gps_age_ms: None,
            };
        };
        if monotonic_ms < anchor.monotonic_ms {
            return ClockReading {
                utc_ms: None,
                status: ClockStatus::Unsynchronized,
                gps_age_ms: None,
            };
        }
        let age = monotonic_ms - anchor.monotonic_ms;
        if age > MAX_UTC_HOLDOVER_MS {
            return ClockReading {
                utc_ms: None,
                status: ClockStatus::Unsynchronized,
                gps_age_ms: Some(age),
            };
        }
        ClockReading {
            utc_ms: Some(anchor.utc_ms.saturating_add(age)),
            status: if age == 0 {
                ClockStatus::GpsSynchronized
            } else {
                ClockStatus::Holdover
            },
            gps_age_ms: Some(age),
        }
    }

    /// Convert an observation made at `observed_monotonic_ms` to UTC, provided
    /// the clock is still within its validated holdover window at `now_ms`.
    pub fn utc_at(&self, observed_monotonic_ms: u64, now_ms: u64) -> Option<u64> {
        self.reading(now_ms).utc_ms?;
        let anchor = self.anchor?;
        if observed_monotonic_ms >= anchor.monotonic_ms {
            Some(
                anchor
                    .utc_ms
                    .saturating_add(observed_monotonic_ms - anchor.monotonic_ms),
            )
        } else {
            anchor
                .utc_ms
                .checked_sub(anchor.monotonic_ms - observed_monotonic_ms)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UtcDateTime {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    pub millis: u16,
}

impl UtcDateTime {
    pub fn to_unix_ms(self) -> Option<u64> {
        if self.year < 2020
            || self.year >= 2100
            || !(1..=12).contains(&self.month)
            || self.day == 0
            || self.day > days_in_month(self.year, self.month)
            || self.hour > 23
            || self.minute > 59
            || self.second > 59
            || self.millis > 999
        {
            return None;
        }
        let days = days_from_civil(self.year as i32, self.month, self.day);
        if days < 0 {
            return None;
        }
        Some(
            (days as u64 * 86_400
                + self.hour as u64 * 3_600
                + self.minute as u64 * 60
                + self.second as u64)
                * 1_000
                + self.millis as u64,
        )
    }
}

#[allow(clippy::manual_is_multiple_of)]
fn days_in_month(year: u16, month: u8) -> u8 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        _ => 0,
    }
}

// Howard Hinnant's civil-date conversion, returning days since Unix epoch.
fn days_from_civil(year: i32, month: u8, day: u8) -> i64 {
    let y = year - i32::from(month <= 2);
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let m = month as i32;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + day as i32 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146_097 + doe - 719_468) as i64
}

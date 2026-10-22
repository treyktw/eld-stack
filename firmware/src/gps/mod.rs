//! GPS engine: read UART NMEA, track the latest fix, accumulate distance.
//!
//! For mileage verification GPS is not just "where" — it's an independent
//! distance source. `total_km()` is the lifetime travelled distance; the
//! verifier deltas it across a trip (same pattern as the odometer).
//!
//! Gated by ENABLE_GPS. Disabled → `total_km()` returns `None` and the verifier
//! simply ignores GPS.

pub mod distance;
pub mod driver;
pub mod fix;
pub mod nmea;
pub mod quality;

use esp_idf_svc::hal::gpio::AnyIOPin;
use esp_idf_svc::hal::uart::UART1;
use log::{info, warn};

use crate::config;
use crate::time::{ClockReading, UtcClock};
use distance::DistanceAccumulator;
use driver::GpsDriver;
use fix::{FixQuality, GpsFix};
use nmea::Nmea;

pub struct GpsEngine {
    driver: Option<GpsDriver<'static>>,
    dist: DistanceAccumulator,
    last_fix: Option<GpsFix>,
    clock: UtcClock,
    sentences_seen: u32,
    fix_reported: bool,
}

impl GpsEngine {
    pub fn disabled() -> Self {
        Self {
            driver: None,
            dist: DistanceAccumulator::new(),
            last_fix: None,
            clock: UtcClock::new(),
            sentences_seen: 0,
            fix_reported: false,
        }
    }

    /// Bring up GPS on the supplied UART. Peripherals come from `App::boot`.
    pub fn open(uart1: UART1<'static>, tx: AnyIOPin<'static>, rx: AnyIOPin<'static>) -> Self {
        match GpsDriver::open(uart1, tx, rx) {
            Ok(driver) => {
                info!("gps: UART1 open @ {} baud", config::GPS_BAUD);
                Self {
                    driver: Some(driver),
                    dist: DistanceAccumulator::new(),
                    last_fix: None,
                    clock: UtcClock::new(),
                    sentences_seen: 0,
                    fix_reported: false,
                }
            }
            Err(e) => {
                warn!("gps: open failed ({e}); disabled");
                Self::disabled()
            }
        }
    }

    pub fn enabled(&self) -> bool {
        self.driver.is_some()
    }

    /// Drain the UART, update the latest fix, the distance accumulator, and the
    /// UTC clock.
    ///
    /// `now_ms` is monotonic. It anchors the clock and stamps fix staleness, so
    /// neither is affected by a later time correction.
    pub fn update(&mut self, now_ms: u64) {
        let Some(driver) = self.driver.as_mut() else {
            return;
        };
        let mut sentences: heapless::Vec<Nmea, 8> = heapless::Vec::new();
        driver.read_into(&mut sentences);

        if !sentences.is_empty() {
            if self.sentences_seen == 0 {
                info!("gps: receiving parsed NMEA from SIM7670G GNSS UART");
            }
            self.sentences_seen = self.sentences_seen.saturating_add(sentences.len() as u32);
        }

        for s in sentences {
            match s {
                Nmea::Gga {
                    fix_quality,
                    hdop,
                    lat,
                    lon,
                } => {
                    let quality = FixQuality::from_gga(fix_quality);
                    if let (true, Some(lat), Some(lon)) = (quality.has_fix(), lat, lon) {
                        if quality::acceptable(hdop) {
                            self.dist.add(lat, lon);
                        }
                        self.last_fix = Some(GpsFix {
                            quality,
                            lat,
                            lon,
                            hdop,
                            observed_ms: now_ms,
                        });
                    } else {
                        // A receiver-declared no-fix invalidates the prior
                        // coordinate. Keeping it would make tunnels and antenna
                        // failures look like a stationary valid fix forever.
                        self.last_fix = None;
                    }
                }
                Nmea::Rmc {
                    valid,
                    datetime,
                    lat,
                    lon,
                } => {
                    // Only a receiver-validated sentence may anchor the clock.
                    // A "V" (void) RMC still carries a plausible-looking date.
                    if !valid {
                        self.last_fix = None;
                        continue;
                    }
                    if let Some(utc_ms) = datetime.and_then(|dt| dt.to_unix_ms()) {
                        // A rejected sync is not fatal: the clock keeps its
                        // existing anchor and ages into Unsynchronized on its
                        // own, which is the honest state to record.
                        let _ = self.clock.synchronize(utc_ms, now_ms);
                    }
                    // RMC position is a fallback for receivers that emit it
                    // more often than GGA; GGA still wins on quality/HDOP.
                    if self.last_fix.is_none() {
                        if let (Some(lat), Some(lon)) = (lat, lon) {
                            self.last_fix = Some(GpsFix {
                                quality: FixQuality::Gps,
                                lat,
                                lon,
                                hdop: None,
                                observed_ms: now_ms,
                            });
                        }
                    }
                }
            }
        }

        let current_fix = self
            .last_fix
            .as_ref()
            .filter(|fix| fix.age_ms(now_ms) <= config::GPS_FIX_MAX_AGE_MS);
        match (self.fix_reported, current_fix) {
            (false, Some(fix)) => {
                info!(
                    "gps: fix acquired lat={:.6} lon={:.6} hdop={} quality={:?}",
                    fix.lat,
                    fix.lon,
                    fix.hdop
                        .map(|value| format!("{value:.1}"))
                        .unwrap_or_else(|| "--".to_string()),
                    fix.quality,
                );
                self.fix_reported = true;
            }
            (true, None) => {
                warn!("gps: fix lost; NMEA receiver is still being monitored");
                self.fix_reported = false;
            }
            _ => {}
        }
    }

    /// Current UTC clock state. `utc_ms` is `None` whenever the device cannot
    /// stand behind the Appendix A ten-minute tolerance.
    pub fn clock_reading(&self, now_ms: u64) -> ClockReading {
        self.clock.reading(now_ms)
    }

    /// UTC corresponding to an earlier monotonic observation, if the clock is
    /// currently synchronized closely enough to make that claim.
    pub fn utc_at(&self, observed_ms: u64, now_ms: u64) -> Option<u64> {
        self.clock.utc_at(observed_ms, now_ms)
    }

    /// Lifetime travelled distance (km), or `None` if GPS is disabled.
    pub fn total_km(&self) -> Option<f32> {
        self.driver.as_ref().map(|_| self.dist.total_km())
    }

    /// Parsed NMEA sentence count since boot. A rising value proves that the
    /// modem/GNSS UART path works even before the receiver has a sky fix.
    pub fn sentences_seen(&self) -> u32 {
        self.sentences_seen
    }

    pub fn latest_fix(&self, now_ms: u64) -> Option<&GpsFix> {
        self.last_fix
            .as_ref()
            .filter(|fix| fix.age_ms(now_ms) <= config::GPS_FIX_MAX_AGE_MS)
    }
}

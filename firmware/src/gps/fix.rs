//! GPS fix state.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FixQuality {
    NoFix,
    Gps,
    Dgps,
    Other,
}

impl FixQuality {
    pub fn from_gga(q: u8) -> Self {
        match q {
            0 => FixQuality::NoFix,
            1 => FixQuality::Gps,
            2 => FixQuality::Dgps,
            _ => FixQuality::Other,
        }
    }

    pub fn has_fix(self) -> bool {
        !matches!(self, FixQuality::NoFix)
    }
}

/// Latest fix state.
///
/// Position is carried here, not discarded. Every ELD event in 49 CFR 395
/// Appendix A §4.5.1 requires latitude and longitude; a fix that only reports
/// "we have a fix" cannot satisfy that.
#[derive(Debug, Clone, Copy)]
pub struct GpsFix {
    pub quality: FixQuality,
    pub lat: f64,
    pub lon: f64,
    pub hdop: Option<f32>,
    /// Monotonic ms when this fix was observed, for staleness checks.
    pub observed_ms: u64,
}

impl GpsFix {
    pub fn has_fix(&self) -> bool {
        self.quality.has_fix()
    }

    /// Age of this fix against a monotonic reading.
    pub fn age_ms(&self, now_ms: u64) -> u64 {
        now_ms.saturating_sub(self.observed_ms)
    }
}

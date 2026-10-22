//! Completed-trip summary.

use crate::verifier::VerifiedMileage;

#[derive(Debug, Clone, Copy)]
pub struct TripSummary {
    pub id: u32,
    /// Monotonic ms at trip start / end.
    pub start_ms: u64,
    pub end_ms: u64,
    pub mileage: VerifiedMileage,
}

impl TripSummary {
    pub fn duration_ms(&self) -> u64 {
        self.end_ms.saturating_sub(self.start_ms)
    }
}

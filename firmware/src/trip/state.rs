//! Trip state machine states.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TripState {
    /// No active trip; waiting for motion.
    Idle,
    /// Vehicle moving; trip in progress.
    Driving,
    /// Vehicle paused mid-trip; may resume or time out into a completed trip.
    Stopped,
}

impl TripState {
    pub fn label(self) -> &'static str {
        match self {
            TripState::Idle => "idle",
            TripState::Driving => "driving",
            TripState::Stopped => "stopped",
        }
    }
}

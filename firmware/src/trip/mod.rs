//! Trip engine: detect trip start/stop/resume/complete and verify mileage.
//!
//! Fed the latest speed + odometer each tick. State machine:
//!   Idle ──move──▶ Driving ──stop──▶ Stopped ──move──▶ Driving
//!                                       └──timeout──▶ Completed ─▶ Idle
//!
//! While in a trip the engine feeds a `VerifierEngine`; on completion it
//! finalizes a `VerifiedMileage` into a `TripSummary`.

pub mod events;
pub mod lifecycle;
pub mod state;
pub mod summary;

use crate::config;
use crate::verifier::VerifierEngine;

use events::TripEvent;
use lifecycle::Motion;
use state::TripState;
use summary::TripSummary;

pub struct TripEngine {
    state: TripState,
    verifier: VerifierEngine,
    trip_id: u32,
    start_ms: u64,
    start_odo_km: Option<f32>,
    stop_since_ms: Option<u64>,
}

impl TripEngine {
    pub fn new() -> Self {
        Self {
            state: TripState::Idle,
            verifier: VerifierEngine::new(),
            trip_id: 0,
            start_ms: 0,
            start_odo_km: None,
            stop_since_ms: None,
        }
    }

    pub fn state(&self) -> TripState {
        self.state
    }

    /// Advance the state machine with the latest telemetry. Returns an event on
    /// a transition, else `None`.
    pub fn tick(
        &mut self,
        speed_kph: Option<f32>,
        odometer_km: Option<f32>,
        gps_km: Option<f32>,
        now_ms: u64,
    ) -> Option<TripEvent> {
        // Accumulate distance only while a trip is active.
        if matches!(self.state, TripState::Driving | TripState::Stopped) {
            self.verifier.update(speed_kph, odometer_km, gps_km, now_ms);
        }

        let motion = lifecycle::motion(speed_kph, self.state);

        match self.state {
            TripState::Idle => {
                if motion == Motion::Moving {
                    self.begin(odometer_km, gps_km, now_ms);
                    return Some(TripEvent::Started { id: self.trip_id });
                }
                None
            }
            TripState::Driving => {
                if motion == Motion::Stopped {
                    self.state = TripState::Stopped;
                    self.stop_since_ms = Some(now_ms);
                    return Some(TripEvent::Stopped { id: self.trip_id });
                }
                None
            }
            TripState::Stopped => {
                if motion == Motion::Moving {
                    self.state = TripState::Driving;
                    self.stop_since_ms = None;
                    return Some(TripEvent::Resumed { id: self.trip_id });
                }
                if motion == Motion::Unknown {
                    // Completion requires a continuous confirmed stop. Signal
                    // loss is diagnostic state, not evidence of zero speed.
                    self.stop_since_ms = None;
                    return None;
                }
                if self.stop_since_ms.is_none() {
                    self.stop_since_ms = Some(now_ms);
                }
                if let Some(t) = self.stop_since_ms {
                    if now_ms.saturating_sub(t) >= config::TRIP_STOP_TIMEOUT_MS {
                        return Some(self.complete(now_ms));
                    }
                }
                None
            }
        }
    }

    fn begin(&mut self, odometer_km: Option<f32>, gps_km: Option<f32>, now_ms: u64) {
        self.trip_id += 1;
        self.state = TripState::Driving;
        self.start_ms = now_ms;
        self.start_odo_km = odometer_km;
        self.stop_since_ms = None;
        self.verifier.begin(odometer_km, gps_km, now_ms);
    }

    fn complete(&mut self, now_ms: u64) -> TripEvent {
        let summary = TripSummary {
            id: self.trip_id,
            start_ms: self.start_ms,
            end_ms: now_ms,
            mileage: self.verifier.finalize(),
        };
        self.state = TripState::Idle;
        self.stop_since_ms = None;
        TripEvent::Completed(summary)
    }
}

impl Default for TripEngine {
    fn default() -> Self {
        Self::new()
    }
}

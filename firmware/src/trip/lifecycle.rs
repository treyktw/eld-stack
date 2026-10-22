//! Motion classification with hysteresis.
//!
//! The start threshold is higher than the stop threshold so speed noise near
//! zero doesn't flap the state machine. The thresholds depend on the current
//! state: a stopped/idle vehicle must exceed START to be "moving"; a driving
//! vehicle must drop to/under STOP to be "stopped".

use crate::config;
use crate::trip::state::TripState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Motion {
    Moving,
    Stopped,
    Unknown,
}

/// Classify motion from a fresh speed. Missing speed is unknown, not stopped;
/// treating ECM silence as zero creates false stop and duty transitions.
pub fn motion(speed_kph: Option<f32>, state: TripState) -> Motion {
    let Some(s) = speed_kph else {
        return Motion::Unknown;
    };
    match state {
        TripState::Driving => {
            if s <= config::TRIP_STOP_SPEED_KPH {
                Motion::Stopped
            } else {
                Motion::Moving
            }
        }
        _ => {
            if s >= config::TRIP_START_SPEED_KPH {
                Motion::Moving
            } else {
                Motion::Stopped
            }
        }
    }
}

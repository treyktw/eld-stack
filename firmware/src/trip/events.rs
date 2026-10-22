//! Trip lifecycle events emitted by the engine.

use crate::trip::summary::TripSummary;

#[derive(Debug, Clone, Copy)]
pub enum TripEvent {
    Started { id: u32 },
    Stopped { id: u32 },
    Resumed { id: u32 },
    Completed(TripSummary),
}

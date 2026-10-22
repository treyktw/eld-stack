//! Mileage verification: reconcile odometer, speed integration, and (later)
//! GPS into a single trusted distance with a confidence score.

pub mod confidence;
pub mod engine;
pub mod mileage;
pub mod odometer;
pub mod speed_integrator;

pub use engine::VerifierEngine;
pub use mileage::VerifiedMileage;

//! The verified-mileage result type.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MileageSource {
    /// Vehicle odometer delta (most trustworthy).
    Odometer,
    /// Integrated wheel speed (used when no odometer).
    SpeedIntegration,
    /// Integrated GPS distance.
    GpsIntegration,
}

impl MileageSource {
    pub fn label(self) -> &'static str {
        match self {
            MileageSource::Odometer => "odometer",
            MileageSource::SpeedIntegration => "speed_integration",
            MileageSource::GpsIntegration => "gps_integration",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct VerifiedMileage {
    pub miles: f32,
    pub source: MileageSource,
    /// 0.0–1.0 trust score (agreement between sources).
    pub confidence: f32,
    pub notes: &'static str,
}

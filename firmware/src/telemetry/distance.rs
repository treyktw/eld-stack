//! Distance signals: total vehicle odometer (km). Trip distance is derived by
//! the trip engine, not stored here.

#[derive(Debug, Default, Clone, Copy)]
pub struct DistanceState {
    /// Total vehicle distance in kilometres.
    pub odometer_km: Option<f32>,
    /// Priority of the source the current value came from.
    ///
    /// Two PGNs carry total distance: 0xFEE0 at 0.125 km/bit and 0xFEC1 at
    /// 5 m/bit — twenty-five times finer. Both used to write here, so whichever
    /// frame arrived last won and a coarse reading could overwrite a precise
    /// one, making the odometer visibly jitter. Recording the source's priority
    /// is what stops that.
    source_priority: u8,
}

impl DistanceState {
    /// Accept a reading only from a source at least as trusted as the current
    /// one. Equal priority updates, so the newest reading from the same source
    /// still wins.
    pub fn set_odometer(&mut self, km: f32, priority: u8) -> bool {
        if self.odometer_km.is_some() && priority < self.source_priority {
            return false;
        }
        self.odometer_km = Some(km);
        self.source_priority = priority;
        true
    }

    /// Priority of the source currently believed.
    pub fn source_priority(&self) -> u8 {
        self.source_priority
    }
}

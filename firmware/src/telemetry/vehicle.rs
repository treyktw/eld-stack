//! Vehicle motion signals: wheel-based speed (CCVS1). VIN added later.

#[derive(Debug, Default, Clone, Copy)]
pub struct VehicleState {
    pub speed_kph: Option<f32>,
}

impl VehicleState {
    pub fn set_speed(&mut self, kph: f32) {
        self.speed_kph = Some(kph);
    }
}

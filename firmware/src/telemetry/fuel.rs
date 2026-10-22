//! Fuel signals: tank level percent (Dash Display). Fuel-used estimate later.

#[derive(Debug, Default, Clone, Copy)]
pub struct FuelState {
    pub level_pct: Option<f32>,
}

impl FuelState {
    pub fn set_level(&mut self, percent: f32) {
        self.level_pct = Some(percent);
    }
}

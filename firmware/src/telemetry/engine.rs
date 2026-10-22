//! Engine signals: RPM (EEC1) and total hours (Engine Hours PGN).

#[derive(Debug, Default, Clone, Copy)]
pub struct EngineState {
    pub rpm: Option<f32>,
    pub hours: Option<f32>,
}

impl EngineState {
    pub fn set_rpm(&mut self, rpm: f32) {
        self.rpm = Some(rpm);
    }

    pub fn set_hours(&mut self, hours: f32) {
        self.hours = Some(hours);
    }
}

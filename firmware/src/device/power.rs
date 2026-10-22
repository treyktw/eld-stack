//! Power state.
//!
//! The ESP32 can only report supply voltage if it's wired through an ADC
//! divider or a PMIC — software alone can't "confirm power" beyond the boot/
//! reset reason. Phase 7 stub: voltage is `None` until ADC sensing is wired.

#[derive(Debug, Default, Clone, Copy)]
pub struct PowerState {
    /// Measured supply voltage (V), if an ADC divider is wired.
    pub voltage: Option<f32>,
}

impl PowerState {
    pub fn read() -> Self {
        Self { voltage: None }
    }

    pub fn label(&self) -> &'static str {
        match self.voltage {
            None => "unknown",
            Some(v) if v < 11.5 => "low",
            Some(_) => "ok",
        }
    }
}

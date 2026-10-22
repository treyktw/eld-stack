//! Rolling bus counters, mirrors the Arduino scanner's tallies.

#[derive(Debug, Default, Clone, Copy)]
pub struct CanStats {
    /// Total frames received.
    pub rx: u32,
    /// Standard (11-bit) frames.
    pub std_frames: u32,
    /// Extended (29-bit) frames — J1939 uses these.
    pub ext_frames: u32,
    /// Remote request frames observed on the bus.
    pub remote_frames: u32,
    /// Frames whose PGN matched a known J1939 PGN.
    pub j1939_frames: u32,
    /// Receive errors / timeouts.
    pub errors: u32,
    /// Requests we transmitted.
    pub requests_sent: u32,
}

impl CanStats {
    pub fn on_rx(&mut self, extended: bool, remote: bool) {
        self.rx = self.rx.wrapping_add(1);
        if extended {
            self.ext_frames = self.ext_frames.wrapping_add(1);
        } else {
            self.std_frames = self.std_frames.wrapping_add(1);
        }
        if remote {
            self.remote_frames = self.remote_frames.wrapping_add(1);
        }
    }

    pub fn on_error(&mut self) {
        self.errors = self.errors.wrapping_add(1);
    }

    pub fn on_j1939(&mut self) {
        self.j1939_frames = self.j1939_frames.wrapping_add(1);
    }

    pub fn on_request(&mut self) {
        self.requests_sent = self.requests_sent.wrapping_add(1);
    }
}

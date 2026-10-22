//! Self-check / diagnostic report.
//!
//! A flat snapshot of system health, assembled by `App` from every subsystem
//! (kept as primitive fields so this module stays decoupled). Serializes to a
//! log line and a JSON payload for upload.

use std::fmt::Write;

#[derive(Debug, Clone, Copy)]
pub struct DiagnosticReport {
    pub reset_reason: &'static str,
    pub uptime_s: u64,
    pub free_heap: u32,
    pub min_free_heap: u32,
    pub power: &'static str,
    pub sd_mounted: bool,
    pub can_up: bool,
    pub bitrate: &'static str,
    pub listen_only: bool,
    pub rx: u32,
    pub ext_frames: u32,
    pub j1939_frames: u32,
    pub distinct_sources: usize,
    pub known_pgns_seen: usize,
    pub proprietary_count: usize,
    pub gps_enabled: bool,
    pub gps_fix: bool,
    pub wifi_online: bool,
    pub upload_queue: usize,
    pub trip_state: &'static str,
}

impl Default for DiagnosticReport {
    fn default() -> Self {
        Self {
            reset_reason: "",
            uptime_s: 0,
            free_heap: 0,
            min_free_heap: 0,
            power: "unknown",
            sd_mounted: false,
            can_up: false,
            bitrate: "-",
            listen_only: false,
            rx: 0,
            ext_frames: 0,
            j1939_frames: 0,
            distinct_sources: 0,
            known_pgns_seen: 0,
            proprietary_count: 0,
            gps_enabled: false,
            gps_fix: false,
            wifi_online: false,
            upload_queue: 0,
            trip_state: "idle",
        }
    }
}

impl DiagnosticReport {
    /// Compact human-readable line for the console.
    pub fn line(&self) -> String {
        let mut s = String::with_capacity(192);
        let _ = write!(
            s,
            "DIAG reset={} up={}s heap={}/{} pwr={} sd={} can={}@{} listen={} rx={} ext={} j1939={} \
             srcs={} pgns={} prop={} gps={}/{} wifi={} q={} trip={}",
            self.reset_reason,
            self.uptime_s,
            self.free_heap,
            self.min_free_heap,
            self.power,
            self.sd_mounted,
            self.can_up,
            self.bitrate,
            self.listen_only,
            self.rx,
            self.ext_frames,
            self.j1939_frames,
            self.distinct_sources,
            self.known_pgns_seen,
            self.proprietary_count,
            self.gps_enabled,
            self.gps_fix,
            self.wifi_online,
            self.upload_queue,
            self.trip_state,
        );
        s
    }

    /// Inner JSON payload object (the upload envelope adds device_id/seq/ts).
    pub fn payload(&self) -> String {
        let mut s = String::with_capacity(320);
        let _ = write!(
            s,
            "{{\"reset\":\"{}\",\"uptime_s\":{},\
             \"free_heap\":{},\"min_free_heap\":{},\"power\":\"{}\",\"sd\":{},\"can_up\":{},\
             \"bitrate\":\"{}\",\"listen_only\":{},\"rx\":{},\"ext\":{},\"j1939\":{},\
             \"sources\":{},\"known_pgns\":{},\"proprietary\":{},\"gps_enabled\":{},\"gps_fix\":{},\
             \"wifi_online\":{},\"upload_queue\":{},\"trip\":\"{}\"}}",
            self.reset_reason,
            self.uptime_s,
            self.free_heap,
            self.min_free_heap,
            self.power,
            self.sd_mounted,
            self.can_up,
            self.bitrate,
            self.listen_only,
            self.rx,
            self.ext_frames,
            self.j1939_frames,
            self.distinct_sources,
            self.known_pgns_seen,
            self.proprietary_count,
            self.gps_enabled,
            self.gps_fix,
            self.wifi_online,
            self.upload_queue,
            self.trip_state,
        );
        s
    }
}

//! Phone/app-facing control surface (pairing, manual trip controls, live
//! status, diagnostics screen).
//!
//! BLE GATT and WiFi SoftAP can feed this same command model. Keeping commands
//! transport-neutral prevents the trip/device layers from depending on BLE.
//!
//! The command/response set mirrors the mobile app's `EldBleTransport` contract
//! (phone-relay/src/protocol.ts):
//!   readDeviceInfo   -> GetDeviceInfo
//!   readLiveData     -> GetLiveData
//!   getUnsyncedCount -> GetUnsyncedCount
//!   getLatestSeq     -> GetLatestSeq
//!   readRecordsRange -> PullRange
//!   ackSyncedRange   -> AckSynced { from, to }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhoneCommand {
    GetDeviceInfo,
    GetLiveData,
    GetUnsyncedCount,
    GetLatestSeq,
    PullRange {
        from_seq: u64,
        to_seq: u64,
    },
    /// Mark a (possibly non-contiguous) range as synced and droppable.
    AckSynced {
        from_seq: u64,
        to_seq: u64,
    },
}

impl PhoneCommand {
    /// Number of distinct commands the firmware understands.
    pub const COUNT: usize = 6;

    /// Parse one ASCII command line (`ELD <VERB> [args]`). Shared by the cable
    /// and BLE relays so both speak the same protocol.
    pub fn parse(line: &str) -> Option<PhoneCommand> {
        let rest = line.trim().strip_prefix("ELD ")?;
        let mut parts = rest.split_ascii_whitespace();
        match parts.next()? {
            "INFO" => Some(PhoneCommand::GetDeviceInfo),
            "LIVE" => Some(PhoneCommand::GetLiveData),
            "UNSYNCED" => Some(PhoneCommand::GetUnsyncedCount),
            "GET_LATEST" | "LATEST?" => Some(PhoneCommand::GetLatestSeq),
            "PULL" => {
                let from_seq = parts.next()?.parse().ok()?;
                let to_seq = parts.next()?.parse().ok()?;
                Some(PhoneCommand::PullRange { from_seq, to_seq })
            }
            "ACK" => {
                let from_seq = parts.next()?.parse().ok()?;
                // Single-arg `ACK <seq>` acks just that seq; two-arg acks a range.
                let to_seq = parts.next().map(|s| s.parse()).transpose().ok()?;
                Some(PhoneCommand::AckSynced {
                    from_seq,
                    to_seq: to_seq.unwrap_or(from_seq),
                })
            }
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PhoneBatch {
    pub from_seq: u64,
    pub to_seq: u64,
    pub events: Vec<String>,
}

impl PhoneBatch {
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
}

/// Device-identity snapshot answered to `GetDeviceInfo`. Matches the mobile
/// `EldDeviceInfo` shape.
#[derive(Debug, Clone)]
pub struct DeviceInfoView {
    pub device_id: &'static str,
    pub firmware_version: &'static str,
    pub vin: Option<String>,
    /// "ok" | "full" | "missing" | "error".
    pub sd_status: &'static str,
    pub input_voltage: Option<f32>,
}

/// Live-telemetry snapshot answered to `GetLiveData`. Matches the mobile
/// `EldLiveData` shape (speed already converted to mph).
#[derive(Debug, Clone)]
pub struct LiveDataView {
    pub rpm: Option<f32>,
    pub speed_mph: Option<f32>,
    pub fuel_percent: Option<f32>,
    pub engine_hours: Option<f32>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub gps_hdop: Option<f32>,
    pub gps_fix_age_ms: Option<u64>,
    pub gps_sentences: u32,
    /// Active faults, pre-formatted as `"SPN <n> FMI <n>"`.
    pub faults: Vec<String>,
}

#[derive(Debug, Default)]
pub struct PhoneControl;

impl PhoneControl {
    pub fn new() -> Self {
        Self
    }

    pub fn connected(&self) -> bool {
        false
    }

    pub fn link_label(&self) -> &'static str {
        "bluetooth"
    }

    pub fn supported_command_count(&self) -> usize {
        PhoneCommand::COUNT
    }
}

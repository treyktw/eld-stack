//! Canonical wire types shared by firmware, phone relay, and ingest.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The only record types accepted by the public ingest boundary.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RecordType {
    EngineSnapshot,
    FaultCode,
    Vin,
    Odometer,
    DriverEvent,
    Heartbeat,
    BusCensus,
}

impl RecordType {
    /// Legacy storage grouping. This is derived server-side and is not part of
    /// the wire contract or the deduplication identity.
    pub fn stream(self) -> &'static str {
        match self {
            Self::EngineSnapshot | Self::Odometer | Self::Vin => "telemetry",
            Self::FaultCode | Self::Heartbeat | Self::BusCensus => "diag",
            Self::DriverEvent => "trip",
        }
    }
}

/// One uploaded event. `ts_ms` is UTC Unix time when the device has a trusted
/// clock; `monotonic_ms` preserves when in the boot session it occurred.
#[derive(Debug, Deserialize, Serialize)]
pub struct Envelope {
    pub device_id: String,
    pub seq: i64,
    pub ts_ms: Option<i64>,
    /// Optional only for compatibility with records queued by pre-split-clock
    /// firmware. Current firmware always sends it.
    pub monotonic_ms: Option<i64>,
    #[serde(rename = "type")]
    pub type_: RecordType,
    pub data: serde_json::Value,
}

impl Envelope {
    /// Stable hash of the data bytes. Producers must keep object key ordering
    /// stable; the hash is an integrity/idempotency aid, not an ELD file check.
    pub fn data_hash(&self) -> String {
        let bytes = serde_json::to_vec(&self.data).unwrap_or_default();
        hex::encode(Sha256::digest(&bytes))
    }

    /// Idempotency key: device + sequence + type + data hash.
    pub fn event_id(&self, data_hash: &str) -> String {
        let mut h = Sha256::new();
        h.update(self.device_id.as_bytes());
        h.update(b"|");
        h.update(self.seq.to_le_bytes());
        h.update(b"|");
        h.update(
            serde_json::to_string(&self.type_)
                .unwrap_or_default()
                .as_bytes(),
        );
        h.update(b"|");
        h.update(data_hash.as_bytes());
        hex::encode(h.finalize())
    }

    /// Basic shape validation beyond serde.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.device_id.is_empty() || self.device_id.len() > 128 {
            return Err("empty device_id");
        }
        if self.seq < 0 {
            return Err("negative seq");
        }
        if self.ts_ms.is_some_and(|timestamp| timestamp <= 0) {
            return Err("invalid ts_ms");
        }
        if self.monotonic_ms.is_some_and(|timestamp| timestamp < 0) {
            return Err("negative monotonic_ms");
        }
        if !self.data.is_object() {
            return Err("data must be an object");
        }
        Ok(())
    }
}

/// Accept either a single envelope or a JSON array of them.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum IngestBody {
    Batch(Vec<Envelope>),
    Single(Envelope),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_firmware_record_deserializes() {
        let record: Envelope = serde_json::from_str(
            r#"{"device_id":"eld-001","seq":42,"ts_ms":1780000000000,"monotonic_ms":1200,"type":"engine_snapshot","data":{"rpm":612}}"#,
        )
        .expect("canonical record");
        assert_eq!(record.type_, RecordType::EngineSnapshot);
        assert_eq!(record.monotonic_ms, Some(1_200));
        assert_eq!(record.type_.stream(), "telemetry");
        assert!(record.validate().is_ok());
    }

    #[test]
    fn legacy_split_schema_is_rejected() {
        let legacy = r#"{"device_id":"eld-001","stream":"telemetry","seq":1,"ts_ms":42,"type":"telemetry","payload":{}}"#;
        assert!(serde_json::from_str::<Envelope>(legacy).is_err());
    }

    #[test]
    fn non_object_data_is_rejected() {
        let record: Envelope = serde_json::from_str(
            r#"{"device_id":"eld-001","seq":1,"ts_ms":42,"monotonic_ms":1,"type":"heartbeat","data":null}"#,
        )
        .expect("shape parses");
        assert_eq!(record.validate(), Err("data must be an object"));
    }

    #[test]
    fn unsynchronized_record_is_retained_without_fake_utc() {
        let record: Envelope = serde_json::from_str(
            r#"{"device_id":"eld-001","seq":1,"ts_ms":null,"monotonic_ms":42,"type":"heartbeat","data":{}}"#,
        )
        .expect("shape parses");
        assert_eq!(record.ts_ms, None);
        assert!(record.validate().is_ok());
    }

    #[test]
    fn queued_pre_split_clock_record_remains_ingestible() {
        let record: Envelope = serde_json::from_str(
            r#"{"device_id":"eld-001","seq":1,"ts_ms":42,"type":"heartbeat","data":{}}"#,
        )
        .expect("old queue entry parses");
        assert_eq!(record.monotonic_ms, None);
        assert!(record.validate().is_ok());
    }
}

impl IngestBody {
    pub fn into_vec(self) -> Vec<Envelope> {
        match self {
            IngestBody::Batch(v) => v,
            IngestBody::Single(e) => vec![e],
        }
    }
}

/// Per-batch result counts.
#[derive(Debug, Default, Serialize)]
pub struct Ack {
    pub accepted: u32,
    pub duplicate: u32,
    /// Permanently unusable — malformed, and retrying will not help. The device
    /// may drop these.
    pub rejected: u32,
    /// Not stored, but the fault was ours. The device MUST retry these, so a
    /// batch containing any of them is answered with a 5xx.
    pub failed: u32,
}

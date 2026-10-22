//! Wire protocol: link types and outbound message envelope.
//!
//! Messages are transport-agnostic: a `kind` tag plus an already-serialized
//! JSON payload. The producer (App) builds the payload; the transport (WiFi/
//! BLE/serial) just ships bytes.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkType {
    Disabled,
    Wifi,
    Bluetooth,
    Cellular,
}

impl LinkType {
    pub fn label(self) -> &'static str {
        match self {
            LinkType::Disabled => "disabled",
            LinkType::Wifi => "wifi",
            LinkType::Bluetooth => "bluetooth",
            LinkType::Cellular => "cellular",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutboundKind {
    TelemetrySnapshot,
    TripSummary,
    DiagnosticReport,
    BusCensus,
    Vin,
    FaultCode,
    PendingUploadChunk,
}

impl OutboundKind {
    /// Record `type` written on the wire. MUST stay within the server's
    /// `eldRecordType` enum (engine_snapshot | fault_code | vin | odometer |
    /// driver_event | heartbeat | bus_census) or the ingest API rejects it.
    /// `PendingUploadChunk` re-ships an already-serialized record, so its label
    /// is never written to the wire.
    pub fn label(self) -> &'static str {
        match self {
            OutboundKind::TelemetrySnapshot => "engine_snapshot",
            OutboundKind::TripSummary => "driver_event",
            OutboundKind::DiagnosticReport => "heartbeat",
            OutboundKind::BusCensus => "bus_census",
            OutboundKind::Vin => "vin",
            OutboundKind::FaultCode => "fault_code",
            OutboundKind::PendingUploadChunk => "chunk",
        }
    }
}

#[derive(Debug, Clone)]
pub struct OutboundMessage {
    pub kind: OutboundKind,
    /// Serialized JSON line for this message.
    pub payload: String,
}

impl OutboundMessage {
    pub fn new(kind: OutboundKind, payload: String) -> Self {
        Self { kind, payload }
    }

    pub fn payload_len(&self) -> usize {
        self.payload.len()
    }
}

//! Comms engine: expose local records to a phone relay.
//!
//! The ELD never depends on comms to keep logging. Producers append upload
//! envelopes to SD first; the phone asks for missing sequence ranges and uploads
//! them to the server when internet exists. Optional direct WiFi remains a
//! secondary transport for depot/testing, not the primary road path.

pub mod bluetooth;
pub mod cellular;
pub mod offline_queue;
pub mod phone;
pub mod protocol;
pub mod retry;
pub mod serial;
pub mod uploader;
pub mod wifi;

use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::modem::Modem;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use log::{info, warn};

use crate::config;
use crate::storage::StorageEngine;
use bluetooth::BluetoothLink;
use offline_queue::OfflineQueue;
use phone::{DeviceInfoView, LiveDataView, PhoneBatch, PhoneCommand, PhoneControl};
use protocol::{OutboundKind, OutboundMessage};
use retry::Backoff;
use serial::SerialRelay;
use uploader::Transport;

#[derive(Debug, Clone, Copy)]
pub struct CommsStatus {
    pub direct_online: bool,
    pub direct_link: &'static str,
    pub direct_rssi_dbm: Option<i16>,
    pub cellular_backoff_ms: Option<u64>,
    pub queue_len: usize,
    pub dropped: u32,
    pub uploaded: u32,
    pub failed: u32,
    pub backoff_ms: u64,
    pub next_attempt_ms: u64,
    pub phone_link: &'static str,
    pub phone_connected: bool,
    pub phone_commands: usize,
    pub last_synced_seq: Option<u64>,
    pub latest_local_seq: Option<u64>,
    pub phone_pending_events: usize,
    pub cable_enabled: bool,
    pub cable_commands_rx: u32,
    pub cable_responses_tx: u32,
    pub cable_parse_errors: u32,
    pub ble_enabled: bool,
    pub ble_advertising: bool,
    pub ble_connected: bool,
    pub ble_subscribed: bool,
    pub ble_commands_rx: u32,
    pub ble_responses_tx: u32,
    pub ble_queued_responses: usize,
    pub ble_parse_errors: u32,
}

pub struct CommsEngine {
    transport: Transport,
    queue: OfflineQueue,
    backoff: Backoff,
    /// Earliest monotonic ms at which the next upload attempt may run.
    next_attempt_ms: u64,
    uploaded: u32,
    failed: u32,
    phone: PhoneControl,
    last_synced_seq: Option<u64>,
    latest_local_seq: Option<u64>,
    phone_pending_events: usize,
    serial: Option<SerialRelay>,
    bluetooth: Option<BluetoothLink>,
    /// Durable lines represented by the message currently at the queue head.
    /// Empty when the head did not come from the durable upload queue.
    pending_batch: Vec<String>,
}

impl CommsEngine {
    pub fn disabled() -> Self {
        Self {
            transport: Transport::Null,
            queue: OfflineQueue::new(),
            backoff: Backoff::new(),
            next_attempt_ms: 0,
            uploaded: 0,
            failed: 0,
            phone: PhoneControl::new(),
            last_synced_seq: None,
            latest_local_seq: None,
            phone_pending_events: 0,
            serial: serial_relay(),
            bluetooth: None,
            pending_batch: Vec::new(),
        }
    }

    /// Bring up comms over WiFi. Peripherals come from `App::boot`.
    pub fn connect(
        modem: Modem<'static>,
        sysloop: EspSystemEventLoop,
        nvs: EspDefaultNvsPartition,
    ) -> Self {
        info!("comms: bringing up WiFi link");
        Self {
            transport: Transport::wifi_or_null(modem, sysloop, nvs),
            queue: OfflineQueue::new(),
            backoff: Backoff::new(),
            next_attempt_ms: 0,
            uploaded: 0,
            failed: 0,
            phone: PhoneControl::new(),
            last_synced_seq: None,
            latest_local_seq: None,
            phone_pending_events: 0,
            serial: serial_relay(),
            bluetooth: None,
            pending_batch: Vec::new(),
        }
    }

    pub fn with_bluetooth(modem: Modem<'static>, nvs: EspDefaultNvsPartition) -> Self {
        info!("comms: bringing up BLE phone relay");
        let bluetooth = match BluetoothLink::open(modem, nvs) {
            Ok(link) => Some(link),
            Err(e) => {
                log::warn!("comms: BLE relay failed ({e})");
                None
            }
        };
        Self {
            transport: Transport::Null,
            queue: OfflineQueue::new(),
            backoff: Backoff::new(),
            next_attempt_ms: 0,
            uploaded: 0,
            failed: 0,
            phone: PhoneControl::new(),
            last_synced_seq: None,
            latest_local_seq: None,
            phone_pending_events: 0,
            serial: serial_relay(),
            bluetooth,
            pending_batch: Vec::new(),
        }
    }

    /// Attach an already-powered cellular modem as the direct uplink while
    /// preserving the independently configured BLE/serial phone relays.
    pub fn enable_cellular(&mut self, modem: cellular::modem::Modem<'static>, now_ms: u64) {
        info!("comms: enabling SIM7670G cellular link");
        self.transport =
            Transport::Cellular(Box::new(cellular::uplink::CellularLink::new(modem, now_ms)));
    }

    pub fn direct_online(&self) -> bool {
        self.transport.online()
    }

    pub fn queue_len(&self) -> usize {
        self.queue.len()
    }

    pub fn status(&self) -> CommsStatus {
        let ble = self.bluetooth.as_ref().map(|link| link.status());
        CommsStatus {
            direct_online: self.direct_online(),
            direct_link: self.transport.link_type().label(),
            direct_rssi_dbm: self.transport.rssi_dbm(),
            cellular_backoff_ms: self.transport.cellular_backoff_ms(),
            queue_len: self.queue_len(),
            dropped: self.queue.dropped(),
            uploaded: self.uploaded,
            failed: self.failed,
            backoff_ms: self.backoff.current_delay(),
            next_attempt_ms: self.next_attempt_ms,
            phone_link: self
                .bluetooth
                .as_ref()
                .map(|link| link.link_type().label())
                .unwrap_or_else(|| self.phone.link_label()),
            phone_connected: self.phone.connected() || ble.map(|s| s.connected).unwrap_or(false),
            phone_commands: self.phone.supported_command_count(),
            last_synced_seq: self.last_synced_seq,
            latest_local_seq: self.latest_local_seq,
            phone_pending_events: self.phone_pending_events,
            cable_enabled: self.serial.is_some(),
            cable_commands_rx: self
                .serial
                .as_ref()
                .map(|serial| serial.commands_rx())
                .unwrap_or(0),
            cable_responses_tx: self
                .serial
                .as_ref()
                .map(|serial| serial.responses_tx())
                .unwrap_or(0),
            cable_parse_errors: self
                .serial
                .as_ref()
                .map(|serial| serial.parse_errors())
                .unwrap_or(0),
            ble_enabled: ble.map(|s| s.enabled).unwrap_or(false),
            ble_advertising: ble.map(|s| s.advertising).unwrap_or(false),
            ble_connected: ble.map(|s| s.connected).unwrap_or(false),
            ble_subscribed: ble.map(|s| s.subscribed).unwrap_or(false),
            ble_commands_rx: ble.map(|s| s.commands_rx).unwrap_or(0),
            ble_responses_tx: ble.map(|s| s.responses_tx).unwrap_or(0),
            ble_queued_responses: ble.map(|s| s.queued_responses).unwrap_or(0),
            ble_parse_errors: ble.map(|s| s.parse_errors).unwrap_or(0),
        }
    }

    /// Queue a record for optional direct upload. The canonical durable copy is
    /// written by `StorageEngine::enqueue_upload` before this method is called.
    pub fn enqueue(&mut self, kind: OutboundKind, payload: String) {
        self.queue.push(OutboundMessage::new(kind, payload));
    }

    pub fn latest_seq(&self, storage: &StorageEngine, issued_latest: Option<u64>) -> Option<u64> {
        storage.latest_pending_seq().or(issued_latest)
    }

    pub fn pull_missing(&self, storage: &StorageEngine, from_seq: u64, to_seq: u64) -> PhoneBatch {
        let events = storage.pending_upload_range(from_seq, to_seq, config::PHONE_BATCH_MAX_EVENTS);
        PhoneBatch {
            from_seq,
            to_seq,
            events,
        }
    }

    fn ack_synced(&mut self, storage: &StorageEngine, from_seq: u64, to_seq: u64) -> usize {
        let acked = storage.ack_pending_upload_range(from_seq, to_seq);
        // Track the high-water mark so the auto-window logic stays monotonic.
        self.last_synced_seq = Some(self.last_synced_seq.map_or(to_seq, |s| s.max(to_seq)));
        acked
    }

    pub fn handle_phone_command(
        &mut self,
        storage: &StorageEngine,
        issued_latest: Option<u64>,
        device: &DeviceInfoView,
        live: &LiveDataView,
        command: PhoneCommand,
    ) -> PhoneResponse {
        match command {
            PhoneCommand::GetDeviceInfo => PhoneResponse::DeviceInfo(device.clone()),
            PhoneCommand::GetLiveData => PhoneResponse::LiveData(live.clone()),
            PhoneCommand::GetUnsyncedCount => PhoneResponse::UnsyncedCount {
                count: storage.pending_upload_count(),
            },
            PhoneCommand::GetLatestSeq => PhoneResponse::LatestSeq {
                seq: self.latest_seq(storage, issued_latest),
            },
            PhoneCommand::PullRange { from_seq, to_seq } => {
                PhoneResponse::Batch(self.pull_missing(storage, from_seq, to_seq))
            }
            PhoneCommand::AckSynced { from_seq, to_seq } => PhoneResponse::Acked {
                from_seq,
                to_seq,
                removed: self.ack_synced(storage, from_seq, to_seq),
            },
        }
    }

    /// Optional direct upload for depot/test WiFi. Phone relay remains primary.
    pub fn poll(
        &mut self,
        now_ms: u64,
        storage: &StorageEngine,
        issued_latest: Option<u64>,
        device: &DeviceInfoView,
        live: &LiveDataView,
    ) {
        // Step any link that needs it before deciding whether we are online.
        // Cellular bring-up is a state machine, and nothing else in the loop
        // advances it — without this the modem never leaves Booting.
        self.transport.tick(now_ms);

        if issued_latest > self.latest_local_seq {
            self.latest_local_seq = issued_latest;
        }
        self.poll_serial(storage, issued_latest, device, live);
        self.poll_bluetooth(storage, issued_latest, device, live);

        if self.transport.online() && self.queue.is_empty() {
            // Pull a batch, not a single record. One POST per record means one
            // TLS handshake per record, and a handshake is an order of
            // magnitude larger than an envelope — on a metered cellular link
            // that overhead, not the data, is the bill.
            let batch = storage.pending_upload_batch(config::UPLOAD_BATCH_MAX);
            if !batch.is_empty() {
                self.queue.push(OutboundMessage::new(
                    OutboundKind::PendingUploadChunk,
                    // The ingest endpoint accepts a JSON array of envelopes.
                    format!("[{}]", batch.join(",")),
                ));
                self.pending_batch = batch;
            }
        }

        if self.has_active_phone_session() {
            self.refresh_phone_window(storage, issued_latest, device, live);
        }

        if !self.transport.online() || now_ms < self.next_attempt_ms || self.queue.is_empty() {
            return;
        }

        let Some(front) = self.queue.front() else {
            return;
        };

        match self.transport.send(front) {
            Ok(()) => {
                info!(
                    "comms: uploaded {} bytes as {}",
                    front.payload_len(),
                    front.kind.label()
                );
                let sent = self.queue.pop();
                if !self.pending_batch.is_empty() {
                    let batch = std::mem::take(&mut self.pending_batch);
                    let count = batch.len();
                    if storage.ack_pending_upload_batch(&batch) {
                        info!("comms: acked {count} durable records");
                    } else {
                        // The head moved under us. Leave the records queued and
                        // let the server dedup the resend: sending twice is
                        // recoverable, dropping an unsent record is not.
                        warn!("comms: durable queue head changed; {count} records stay queued");
                    }
                } else if let Some(sent) = sent {
                    if !storage.ack_pending_upload_if_front_matches(&sent.payload) {
                        info!("comms: sent RAM message; durable queue head unchanged");
                    }
                }
                self.backoff.reset();
                self.uploaded = self.uploaded.wrapping_add(1);
                // Pace successive sends so we don't hog the loop.
                self.next_attempt_ms = now_ms + config::UPLOAD_INTERVAL_MS;
            }
            Err(()) => {
                self.failed = self.failed.wrapping_add(1);
                self.next_attempt_ms = now_ms + self.backoff.next_delay();
                // Keep `pending_batch` as-is: the same message stays at the
                // head and will be retried, and the batch still describes it.
            }
        }
    }

    fn poll_serial(
        &mut self,
        storage: &StorageEngine,
        issued_latest: Option<u64>,
        device: &DeviceInfoView,
        live: &LiveDataView,
    ) {
        let Some(serial) = &mut self.serial else {
            return;
        };
        let Some(command) = serial.poll_command() else {
            return;
        };
        let response = self.handle_phone_command(storage, issued_latest, device, live, command);
        if let PhoneResponse::Acked {
            from_seq,
            to_seq,
            removed,
        } = &response
        {
            info!("comms: cable acked seq {from_seq}..={to_seq}, removed {removed}");
        }
        if let Some(serial) = &mut self.serial {
            serial.write_response(&response);
        }
    }

    fn poll_bluetooth(
        &mut self,
        storage: &StorageEngine,
        issued_latest: Option<u64>,
        device: &DeviceInfoView,
        live: &LiveDataView,
    ) {
        loop {
            let command = {
                let Some(bluetooth) = &mut self.bluetooth else {
                    return;
                };
                bluetooth.pop_command()
            };
            let Some(command) = command else {
                break;
            };
            let response = self.handle_phone_command(storage, issued_latest, device, live, command);
            let lines = response_lines(&response);
            if let Some(bluetooth) = &mut self.bluetooth {
                for line in lines {
                    bluetooth.enqueue_response_line(&line);
                }
            }
        }

        if let Some(bluetooth) = &mut self.bluetooth {
            bluetooth.flush();
        }
    }

    fn refresh_phone_window(
        &mut self,
        storage: &StorageEngine,
        issued_latest: Option<u64>,
        device: &DeviceInfoView,
        live: &LiveDataView,
    ) {
        if let PhoneResponse::LatestSeq { seq } = self.handle_phone_command(
            storage,
            issued_latest,
            device,
            live,
            PhoneCommand::GetLatestSeq,
        ) {
            self.latest_local_seq = seq;
        }

        let Some(latest) = self.latest_local_seq else {
            self.phone_pending_events = 0;
            return;
        };
        let from = self
            .last_synced_seq
            .map(|seq| seq.saturating_add(1))
            .unwrap_or(0);
        if from > latest {
            self.phone_pending_events = 0;
            return;
        }

        if let PhoneResponse::Batch(batch) = self.handle_phone_command(
            storage,
            issued_latest,
            device,
            live,
            PhoneCommand::PullRange {
                from_seq: from,
                to_seq: latest,
            },
        ) {
            let _window = (batch.from_seq, batch.to_seq);
            self.phone_pending_events = if batch.is_empty() {
                0
            } else {
                batch.events.len()
            };
        }
    }

    fn has_active_phone_session(&self) -> bool {
        if self.phone.connected() {
            return true;
        }
        self.bluetooth
            .as_ref()
            .map(|bluetooth| {
                let status = bluetooth.status();
                status.connected || status.subscribed
            })
            .unwrap_or(false)
    }
}

fn serial_relay() -> Option<SerialRelay> {
    if config::ENABLE_CABLE_RELAY {
        Some(SerialRelay::new())
    } else {
        None
    }
}

pub(crate) fn response_lines(response: &PhoneResponse) -> Vec<String> {
    match response {
        PhoneResponse::DeviceInfo(info) => vec![format!("ELD INFO {}", device_info_json(info))],
        PhoneResponse::LiveData(live) => vec![format!("ELD LIVE {}", live_data_json(live))],
        PhoneResponse::UnsyncedCount { count } => vec![format!("ELD UNSYNCED {count}")],
        PhoneResponse::LatestSeq { seq } => {
            vec![format!(
                "ELD LATEST {}",
                seq.map(|s| s.to_string())
                    .unwrap_or_else(|| "null".to_string())
            )]
        }
        PhoneResponse::Batch(batch) => {
            let mut lines = Vec::with_capacity(batch.events.len() + 2);
            lines.push(format!(
                "ELD BATCH from={} to={} count={}",
                batch.from_seq,
                batch.to_seq,
                batch.events.len()
            ));
            for event in &batch.events {
                lines.push(format!("ELD EVENT {event}"));
            }
            lines.push("ELD END".to_string());
            lines
        }
        PhoneResponse::Acked {
            from_seq,
            to_seq,
            removed,
        } => vec![format!(
            "ELD ACKED from={from_seq} to={to_seq} removed={removed}"
        )],
    }
}

/// JSON for `ELD INFO`, keyed to match the mobile `EldDeviceInfo` shape.
fn device_info_json(info: &DeviceInfoView) -> String {
    let vin = info
        .vin
        .as_deref()
        .map(|v| format!("\"{v}\""))
        .unwrap_or_else(|| "null".to_string());
    let voltage = info
        .input_voltage
        .map(|v| format!("{v:.2}"))
        .unwrap_or_else(|| "null".to_string());
    format!(
        "{{\"deviceId\":\"{}\",\"firmwareVersion\":\"{}\",\"vin\":{},\"sdStatus\":\"{}\",\"inputVoltage\":{}}}",
        info.device_id, info.firmware_version, vin, info.sd_status, voltage,
    )
}

/// JSON for `ELD LIVE`, keyed to match the mobile `EldLiveData` shape.
fn live_data_json(live: &LiveDataView) -> String {
    let f = |v: Option<f32>| {
        v.map(|x| format!("{x:.2}"))
            .unwrap_or_else(|| "null".to_string())
    };
    let coordinate = |v: Option<f64>| {
        v.map(|x| format!("{x:.6}"))
            .unwrap_or_else(|| "null".to_string())
    };
    let age = live
        .gps_fix_age_ms
        .map(|value| value.to_string())
        .unwrap_or_else(|| "null".to_string());
    let faults = live
        .faults
        .iter()
        .map(|s| format!("\"{s}\""))
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "{{\"rpm\":{},\"speedMph\":{},\"fuelPercent\":{},\"engineHours\":{},\"latitude\":{},\"longitude\":{},\"gpsHdop\":{},\"gpsFixAgeMs\":{},\"gpsSentences\":{},\"faults\":[{}]}}",
        f(live.rpm),
        f(live.speed_mph),
        f(live.fuel_percent),
        f(live.engine_hours),
        coordinate(live.latitude),
        coordinate(live.longitude),
        f(live.gps_hdop),
        age,
        live.gps_sentences,
        faults,
    )
}

#[derive(Debug, Clone)]
pub enum PhoneResponse {
    DeviceInfo(DeviceInfoView),
    LiveData(LiveDataView),
    UnsyncedCount {
        count: usize,
    },
    LatestSeq {
        seq: Option<u64>,
    },
    Batch(PhoneBatch),
    Acked {
        from_seq: u64,
        to_seq: u64,
        removed: usize,
    },
}

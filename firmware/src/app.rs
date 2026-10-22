//! Firmware orchestrator.
//!
//! `App` owns every subsystem and the main loop. `main.rs` does nothing but
//! init the logger, build an `App`, `boot()`, then `run()`. All real work
//! happens through managers owned here, never in `main`.
//!
//! Phase 1 wires CAN + J1939: scan the bus, decode signals, request missing
//! PGNs, and emit heartbeat stats. Telemetry/storage/trip/etc. arrive in later
//! phases (see README).

use log::{info, warn};

use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::nvs::EspDefaultNvsPartition;

use crate::can::{frame::CanFrame, CanManager};
use crate::comms::phone::{DeviceInfoView, LiveDataView};
use crate::comms::protocol::OutboundKind;
use crate::comms::CommsEngine;
use crate::device::boot::ResetReason;
use crate::device::{health::Health, power::PowerState, DiagnosticReport, SeqGen};
use crate::error::{EldError, Result};
use crate::gps::GpsEngine;
use crate::j1939::{
    self, decoder, decoder::J1939Signal, pgn, request, spn, transport::Reassembled, Discovery,
    J1939Id, Requester, TransportReassembler,
};
use crate::storage::schema::RecordStamp;
use crate::telemetry::cadence::{self, CadencePolicy};
use crate::telemetry::snapshot::TelemetrySnapshot;

const TELEMETRY_CADENCE: CadencePolicy = CadencePolicy {
    min_interval_ms: config::TELEMETRY_MIN_INTERVAL_MS,
    heartbeat_ms: config::TELEMETRY_HEARTBEAT_MS,
    speed_delta_kph: config::TELEMETRY_SPEED_DELTA_KPH,
    odometer_delta_km: config::TELEMETRY_ODOMETER_DELTA_KM,
    engine_hours_delta: config::TELEMETRY_ENGINE_HOURS_DELTA,
};
use crate::storage::StorageEngine;
use crate::telemetry::TelemetryStore;
use crate::trip::events::TripEvent;
use crate::trip::summary::TripSummary;
use crate::trip::TripEngine;
use crate::{config, device, logging, time};

/// How long a single `poll` blocks waiting for a frame.
const POLL_TIMEOUT_MS: u32 = 200;

pub struct App {
    device: device::DeviceInfo,
    boot_ms: u64,
    can: Option<CanManager>,
    requester: Requester,
    discovery: Discovery,
    telemetry: TelemetryStore,
    storage: StorageEngine,
    trip: TripEngine,
    gps: GpsEngine,
    comms: CommsEngine,
    reset_reason: ResetReason,
    seq: SeqGen,
    /// Reassembles multi-packet J1939 messages (VIN, multi-DTC DM1).
    tp: TransportReassembler,
    /// Latest VIN decoded from the bus, once seen.
    vin: Option<String>,
    /// Latest active DTC set; we only emit a fault_code record when it changes.
    faults: decoder::Faults,
    /// OBD-II request pacing and per-vehicle PID discovery.
    obd: crate::obd::poller::ObdPoller,
    /// Keeps the T-SIM modem control pins asserted for the life of the app.
    modem_power: Option<device::board::ModemPower>,
    /// Owns the AT UART while GNSS-only bring-up is active. When a SIM is
    /// installed this moves into `comms` as the cellular transport instead.
    standby_modem: Option<crate::comms::cellular::modem::Modem<'static>>,
}

impl App {
    pub fn new() -> Self {
        Self {
            device: device::DeviceInfo::new(),
            boot_ms: time::now_ms(),
            can: None,
            requester: Requester::new(),
            discovery: Discovery::new(),
            telemetry: TelemetryStore::new(),
            storage: StorageEngine::disabled(),
            trip: TripEngine::new(),
            gps: GpsEngine::disabled(),
            comms: CommsEngine::disabled(),
            reset_reason: ResetReason::read(),
            seq: SeqGen::new(),
            tp: TransportReassembler::new(),
            vin: None,
            faults: decoder::Faults::new(),
            obd: crate::obd::poller::ObdPoller::new(),
            modem_power: None,
            standby_modem: None,
        }
    }

    /// One-time bring-up: banner, subsystem init, CAN bitrate scan.
    pub fn boot(&mut self) -> Result<()> {
        info!("====================================");
        info!("ELD FIRMWARE BOOT");
        info!("====================================");
        info!("Device ID:   {}", self.device.device_id);
        if !self.device.is_derived() {
            // Two units sharing an id collide in ingest dedup and make records
            // unattributable. Loud, because it must not reach a truck.
            warn!(
                "device: identity is NOT chip-derived ({}); do not deploy this unit",
                self.device.device_id
            );
        }
        info!("Firmware:    {}", self.device.firmware_version);
        info!("Board:       {}", self.device.board);
        info!("Mode:        {}", config::BOOT_MODE);
        info!("Reset:       {}", self.reset_reason.label());
        if self.reset_reason.is_unexpected() {
            warn!("device: unexpected reset ({})", self.reset_reason.label());
        }

        logging::init();

        // Single peripherals acquisition; hand the relevant parts to each
        // enabled subsystem. (CAN uses its own steal loop in the scanner.)
        let p = Peripherals::take().map_err(|_| EldError::Device("peripherals take failed"))?;

        // Sequence allocation and radio configuration share the default NVS
        // partition through cheap cloned handles. Persist the next boot epoch
        // before any record can be created.
        let nvs = EspDefaultNvsPartition::take().ok();
        if let Some(partition) = nvs.as_ref() {
            match SeqGen::persistent(partition.clone()) {
                Ok(seq) => self.seq = seq,
                Err(e) => warn!("device: sequence NVS unavailable ({e}); using random boot epoch"),
            }
        } else {
            warn!("device: NVS unavailable; sequence boot epoch is not persistent");
        }

        if config::ENABLE_BLUETOOTH {
            match nvs.as_ref() {
                Some(nvs) => {
                    self.comms = CommsEngine::with_bluetooth(p.modem, nvs.clone());
                }
                None => warn!("comms: nvs unavailable; BLE phone relay disabled"),
            }
        } else if config::ENABLE_WIFI_UPLOAD {
            match (EspSystemEventLoop::take(), nvs.as_ref()) {
                (Ok(sysloop), Some(nvs)) => {
                    self.comms = CommsEngine::connect(p.modem, sysloop, nvs.clone());
                }
                _ => warn!("comms: sysloop/nvs unavailable; upload disabled"),
            }
        } else {
            info!("comms: phone relay/direct upload disabled (flags off)");
        }

        if config::ENABLE_CELLULAR {
            match device::board::ModemPower::start(
                p.uart2,
                p.pins.gpio4.into(),  // ESP TX -> SIM7670G RX
                p.pins.gpio5.into(),  // ESP RX <- SIM7670G TX
                p.pins.gpio46.into(), // PWRKEY
                p.pins.gpio7.into(),  // DTR, low keeps modem awake
                p.pins.gpio42.into(), // high selects full-power rail
            ) {
                Ok((power, modem)) => {
                    self.modem_power = Some(power);
                    if config::ENABLE_CELLULAR_UPLINK {
                        self.comms.enable_cellular(modem, time::now_ms());
                    } else {
                        info!(
                            "cellular: uplink paused until SIM installation; GNSS remains active"
                        );
                        self.standby_modem = Some(modem);
                    }
                }
                Err(e) => warn!("cellular: board bring-up failed ({e}); offline"),
            }
        } else {
            info!("cellular: disabled (flag off)");
        }

        if config::ENABLE_SD_LOGGING {
            self.storage = StorageEngine::with_sd(
                p.spi2,
                p.pins.gpio12.into(), // SCLK (onboard TF socket)
                p.pins.gpio11.into(), // MOSI
                p.pins.gpio13.into(), // MISO
                p.pins.gpio10.into(), // CS
            );
        } else {
            info!("storage: SD logging disabled (flag off)");
        }
        self.storage
            .log_boot(self.device.firmware_version, self.device.board);

        if config::ENABLE_GPS {
            self.gps = GpsEngine::open(p.uart1, p.pins.gpio48.into(), p.pins.gpio45.into());
        } else {
            info!("gps: disabled (flag off)");
        }

        let can = CanManager::start()?;
        self.can = Some(can);

        info!("ELD firmware initialized.");
        Ok(())
    }

    /// Main loop: read CAN → decode J1939 → request missing → heartbeat.
    pub fn run(&mut self) -> Result<()> {
        let mut can = self.can.take().expect("boot() must run before run()");
        let mut last_heartbeat = time::now_ms();
        let mut last_telemetry_upload = time::now_ms();
        let mut last_telemetry_sent: Option<TelemetrySnapshot> = None;
        let mut last_diag = time::now_ms();
        let mut last_census_log = time::now_ms();
        let mut census_uploaded = false;

        loop {
            // 1. Read + decode one frame (if any arrived this slice).
            match can.poll(POLL_TIMEOUT_MS) {
                Ok(Some(frame)) => self.handle_frame(&mut can, &frame),
                Ok(None) => {}
                Err(_) => warn!("can: receive error"),
            }

            let now = time::now_ms();

            // 2. Request PGNs we haven't seen recently (never in listen-only).
            //    Global first, then targeted at each discovered ECU.
            if config::ENABLE_PGN_REQUESTS && !can.listen_only() {
                let sources = self.discovery.source_addresses();
                for (pgn, dest) in self.requester.due(now, &sources) {
                    if let Some(f) = request::build_request(pgn, dest, request::SOURCE_ADDR) {
                        if can.transmit(&f).is_ok() {
                            info!("REQ 0x{pgn:06X} -> 0x{dest:02X}");
                        }
                    }
                }
            }

            // 2b. OBD-II: discover what this vehicle answers, then poll it.
            //
            // Listen-only is respected for the same reason as J1939 requests —
            // in that mode the device must not transmit at all, and an OBD
            // request is a transmission like any other.
            if config::ENABLE_OBD_POLLING && !can.listen_only() {
                use crate::obd::pid as obd_pid;
                use crate::obd::poller::PollAction;

                let pid = match self.obd.poll(now) {
                    PollAction::Discover(range) => Some(range),
                    PollAction::Read(pid) => Some(pid),
                    PollAction::Idle => None,
                };

                if let Some(pid) = pid {
                    // 11-bit standard frame to the functional broadcast
                    // address. Extended would be J1939 and no OBD ECU answers.
                    let payload = obd_pid::request_frame(pid);
                    if let Ok(frame) = CanFrame::standard(obd_pid::REQUEST_ID, &payload) {
                        if let Ok(hal) = frame.to_hal() {
                            let _ = can.transmit(&hal);
                        }
                    }
                }
            }

            // 3. GPS: drain UART, update fix + distance + the UTC clock.
            self.gps.update(now);

            // Refresh the clock records are stamped with. Before the first
            // valid RMC this stays unsynchronized, and records say so — a
            // regulatory timestamp must never be uptime wearing a UTC label.
            let clock = self.gps.clock_reading(now);
            self.storage.set_stamp(match clock.utc_ms {
                Some(utc_ms) => RecordStamp {
                    monotonic_ms: now,
                    utc_ms: Some(utc_ms),
                },
                None => RecordStamp::unsynchronized(now),
            });

            // 4. Trip detection + mileage verification.
            let snap = self.telemetry.snapshot(now);
            let gps_km = self.gps.total_km();
            if let Some(event) = self
                .trip
                .tick(snap.speed_kph, snap.odometer_km, gps_km, now)
            {
                self.handle_trip_event(&event);
            }

            // 5. Comms: enqueue telemetry on change, drain the upload queue.
            //
            // Compliance records are NOT throttled here — trip and duty events
            // are enqueued the moment they occur, above. This governs only the
            // operational telemetry snapshot, which used to fire every 30
            // seconds whether or not anything had happened.
            if cadence::telemetry_due(
                last_telemetry_sent.as_ref(),
                &snap,
                now.saturating_sub(last_telemetry_upload),
                &TELEMETRY_CADENCE,
            ) {
                let payload = self.telemetry_payload();
                let json = self.envelope(OutboundKind::TelemetrySnapshot, now, &payload);
                self.storage.enqueue_upload(&json);
                self.comms.enqueue(OutboundKind::TelemetrySnapshot, json);
                last_telemetry_upload = now;
                last_telemetry_sent = Some(snap);
            }
            let device_view = self.device_view();
            let live_view = self.live_view();
            self.comms.poll(
                now,
                &self.storage,
                self.seq.latest(),
                &device_view,
                &live_view,
            );

            // 5b. Bus census: what this truck actually emits.
            //
            // J1939 standardises meaning, not availability — manufacturers and
            // model years differ, and the same value can come from a different
            // PGN or not at all. Recording the whole bus, known PGNs and not,
            // is what makes the next unfamiliar truck a desk problem instead of
            // another field visit.
            if now.saturating_sub(last_census_log) >= config::CENSUS_LOG_MS {
                let census = self.discovery.census_json(config::CENSUS_EXPORT_MAX);
                self.storage.log_census(&census);
                last_census_log = now;
            }
            if !census_uploaded && now >= config::CENSUS_UPLOAD_AFTER_MS {
                let census = self.discovery.census_json(config::CENSUS_EXPORT_MAX);
                let json = self.envelope(OutboundKind::BusCensus, now, &census);
                self.storage.enqueue_upload(&json);
                census_uploaded = true;
                info!(
                    "census: {} pairs, {} sources, {} unrecognised PGNs",
                    self.discovery.census().entries().len(),
                    self.discovery.distinct_sources(),
                    self.discovery.unrecognised().len(),
                );
            }

            // 6. Diagnostics self-check.
            if now.saturating_sub(last_diag) >= config::DIAGNOSTICS_MS {
                let health = Health::read(self.boot_ms);
                let power = PowerState::read();
                let gps_fix = self.gps.latest_fix(now);
                info!(
                    "gps | enabled={} nmea={} fix={} lat={} lon={} hdop={} age_ms={} distance_km={}",
                    self.gps.enabled(),
                    self.gps.sentences_seen(),
                    gps_fix.is_some(),
                    gps_fix
                        .map(|fix| format!("{:.6}", fix.lat))
                        .unwrap_or_else(|| "--".to_string()),
                    gps_fix
                        .map(|fix| format!("{:.6}", fix.lon))
                        .unwrap_or_else(|| "--".to_string()),
                    gps_fix
                        .and_then(|fix| fix.hdop)
                        .map(|value| format!("{value:.1}"))
                        .unwrap_or_else(|| "--".to_string()),
                    gps_fix
                        .map(|fix| fix.age_ms(now).to_string())
                        .unwrap_or_else(|| "--".to_string()),
                    self.gps
                        .total_km()
                        .map(|value| format!("{value:.3}"))
                        .unwrap_or_else(|| "--".to_string()),
                );
                let st = &can.stats;
                let comms_status = self.comms.status();
                let profile = can.profile(
                    self.discovery.distinct_sources(),
                    self.discovery.known_pgns_seen(),
                    self.discovery.proprietary().len(),
                );
                let report = DiagnosticReport {
                    reset_reason: self.reset_reason.label(),
                    uptime_s: health.uptime_s,
                    free_heap: health.free_heap,
                    min_free_heap: health.min_free_heap,
                    power: power.label(),
                    sd_mounted: self.storage.enabled(),
                    can_up: profile.is_up(),
                    bitrate: profile.bitrate_label(),
                    listen_only: profile.listen_only,
                    rx: st.rx,
                    ext_frames: st.ext_frames,
                    j1939_frames: st.j1939_frames,
                    distinct_sources: profile.distinct_sources,
                    known_pgns_seen: profile.known_pgns_seen,
                    proprietary_count: profile.proprietary_count,
                    gps_enabled: self.gps.enabled(),
                    gps_fix: gps_fix.map(|f| f.has_fix()).unwrap_or(false),
                    wifi_online: comms_status.direct_online,
                    upload_queue: comms_status.queue_len,
                    trip_state: self.trip.state().label(),
                };
                info!("{}", report.line());
                let payload = report.payload();
                let json = self.envelope(OutboundKind::DiagnosticReport, now, &payload);
                self.storage.enqueue_upload(&json);
                self.comms.enqueue(OutboundKind::DiagnosticReport, json);
                last_diag = now;
            }

            // 7. Heartbeat.
            if now.saturating_sub(last_heartbeat) >= config::HEARTBEAT_MS {
                let s = &can.stats;
                let comms_status = self.comms.status();
                let profile = can.profile(
                    self.discovery.distinct_sources(),
                    self.discovery.known_pgns_seen(),
                    self.discovery.proprietary().len(),
                );
                let channel = can.channel_result();
                info!(
                    "heartbeat | bus={} wired={} selected={} channel={} active={} coverage={:.1}% uptime={}s rx={} ext={} std={} remote={} j1939={} src={} known={} prop={} err={} req={}",
                    profile.bitrate_label(),
                    profile.wiring.label(),
                    channel.pair.label(),
                    channel.summary(),
                    profile.saw_j1939(),
                    profile.known_coverage_pct(j1939::known_pgns::KNOWN.len()),
                    time::elapsed_ms(self.boot_ms) / 1000,
                    s.rx,
                    s.ext_frames,
                    s.std_frames,
                    s.remote_frames,
                    s.j1939_frames,
                    profile.distinct_sources,
                    profile.known_pgns_seen,
                    profile.proprietary_count,
                    s.errors,
                    s.requests_sent,
                );
                self.storage
                    .log_heartbeat(can.bitrate().label(), s.rx, s.j1939_frames, s.errors);
                let obd_supported = self.obd.supported();
                let obd_sample: heapless::Vec<u8, 8> = obd_supported.iter().take(8).collect();
                info!(
                    "obd | discovering={} blind={} supported={} empty={} sample={obd_sample:?}",
                    self.obd.discovering(),
                    self.obd.is_blind(),
                    obd_supported.count(),
                    obd_supported.is_empty(),
                );
                info!(
                    "comms | direct_link={} direct_online={} rssi_dbm={} cellular_backoff_ms={} queue={} dropped={} direct_uploaded={} failed={} retry_backoff={}ms next={} phone_link={} phone_connected={} phone_commands={} latest_local={} phone_pending={} last_synced={} cable={} cable_rx={} cable_tx={} cable_err={} ble={} adv={} conn={} sub={} ble_rx={} ble_tx={} ble_q={} ble_err={}",
                    comms_status.direct_link,
                    comms_status.direct_online,
                    comms_status
                        .direct_rssi_dbm
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "--".to_string()),
                    comms_status
                        .cellular_backoff_ms
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "--".to_string()),
                    comms_status.queue_len,
                    comms_status.dropped,
                    comms_status.uploaded,
                    comms_status.failed,
                    comms_status.backoff_ms,
                    comms_status.next_attempt_ms,
                    comms_status.phone_link,
                    comms_status.phone_connected,
                    comms_status.phone_commands,
                    comms_status
                        .latest_local_seq
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| "none".to_string()),
                    comms_status.phone_pending_events,
                    comms_status
                        .last_synced_seq
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| "none".to_string()),
                    comms_status.cable_enabled,
                    comms_status.cable_commands_rx,
                    comms_status.cable_responses_tx,
                    comms_status.cable_parse_errors,
                    comms_status.ble_enabled,
                    comms_status.ble_advertising,
                    comms_status.ble_connected,
                    comms_status.ble_subscribed,
                    comms_status.ble_commands_rx,
                    comms_status.ble_responses_tx,
                    comms_status.ble_queued_responses,
                    comms_status.ble_parse_errors,
                );
                self.log_telemetry();
                last_heartbeat = now;
            }
        }
    }

    /// Parse + decode a single extended frame and log any signal.
    fn handle_frame(&mut self, can: &mut CanManager, frame: &CanFrame) {
        self.storage.log_can_frame(frame);
        let now = time::now_ms();

        // OBD-II responses are 11-bit and would be discarded by the J1939
        // check below, which is why they were invisible until now.
        if !frame.extended {
            self.handle_obd_frame(frame, now);
            return;
        }

        if !frame.is_j1939_candidate() {
            return;
        }
        let Some(id) = J1939Id::parse(frame.id) else {
            return;
        };

        can.stats.on_j1939();
        self.requester.mark_seen(id.pgn, now);
        self.discovery.observe(&id, frame.payload(), now);

        // Multi-packet messages (VIN, multi-DTC DM1) arrive via TP; feed every
        // frame and act when a session completes.
        if let Some(msg) = self.tp.ingest(&id, frame.payload()) {
            self.handle_reassembled(&msg, now);
        }
        // Single-frame DM1 carries one DTC inline (no TP).
        if id.pgn == pgn::DM1 {
            let faults = decoder::decode_dm1(frame.payload());
            self.update_faults(faults, now);
        }

        // Some PGNs contain more than one useful signal. Process every
        // definition instead of silently keeping only the highest-priority one.
        let mut signals = j1939::decoder::decode_all(&id, frame.payload());
        if signals.is_empty() {
            let _ = signals.push(j1939::decoder::decode(&id, frame.payload()));
        }
        for signal in signals {
            self.telemetry.ingest(&signal, now);
            match signal {
                J1939Signal::EngineRpm {
                    rpm, source, spn, ..
                } => {
                    info!("DATA rpm {rpm:.0} spn={spn} src=0x{source:02X}");
                    self.storage.log_signal("rpm", rpm, source);
                }
                J1939Signal::WheelSpeed {
                    kph, source, spn, ..
                } => {
                    info!("DATA speed_kph {kph:.1} spn={spn} src=0x{source:02X}");
                    self.storage.log_signal("speed_kph", kph, source);
                }
                J1939Signal::FuelLevel {
                    percent,
                    source,
                    spn,
                    ..
                } => {
                    info!("DATA fuel_level_pct {percent:.1} spn={spn} src=0x{source:02X}");
                    self.storage.log_signal("fuel_level_pct", percent, source);
                }
                J1939Signal::EngineHours {
                    hours, source, spn, ..
                } => {
                    info!("DATA engine_hours {hours:.1} spn={spn} src=0x{source:02X}");
                    self.storage.log_signal("engine_hours", hours, source);
                }
                J1939Signal::Odometer {
                    km,
                    miles,
                    source,
                    spn,
                    ..
                } => {
                    info!(
                        "DATA odometer_km {km:.1} odometer_mi {miles:.1} spn={spn} src=0x{source:02X}"
                    );
                    self.storage.log_signal("odometer_km", km, source);
                }
                J1939Signal::UnknownPgn { pgn, source } => {
                    info!(
                        "DATA unknown_pgn 0x{pgn:06X} src=0x{source:02X} id={}",
                        frame.log_id()
                    );
                }
            }
        }
    }

    /// Act on a completed multi-packet message (VIN or DM1).
    /// Handle an 11-bit frame as a possible OBD-II response.
    ///
    /// Everything is validated by `obd::pid::parse_response` — address range,
    /// mode, length, payload width — so a standard-ID frame from something
    /// other than an OBD ECU is discarded rather than half-decoded. The bus
    /// carries plenty of 11-bit traffic that is not an answer to us.
    fn handle_obd_frame(&mut self, frame: &CanFrame, now_ms: u64) {
        use crate::obd::pid::{self, Value};

        let Ok(reading) = pid::parse_response(frame.id as u16, frame.payload()) else {
            return;
        };

        // A support bitmap is discovery data, not telemetry. It arrives as an
        // Unknown value because the range PIDs have no scaling of their own.
        if matches!(reading.value, Value::Unknown { .. })
            && crate::obd::support::SUPPORT_RANGE_PIDS.contains(&reading.pid)
        {
            let data = frame.payload();
            if data.len() >= 7 {
                self.obd
                    .on_support_response(reading.pid, [data[3], data[4], data[5], data[6]]);
            }
            return;
        }

        self.telemetry.ingest_obd(&reading, now_ms);
    }

    fn handle_reassembled(&mut self, msg: &Reassembled, now: u64) {
        self.requester.mark_seen(msg.pgn, now);
        match msg.pgn {
            pgn::VEHICLE_ID => {
                if let Some(vin) = decoder::decode_vin(&msg.data) {
                    info!("RX vin src=0x{:02X}", msg.source);
                    self.update_vin(&vin, now);
                }
            }
            pgn::DM1 => {
                let faults = decoder::decode_dm1(&msg.data);
                info!("RX dm1 src=0x{:02X} dtcs={}", msg.source, faults.len());
                self.update_faults(faults, now);
            }
            _ => {}
        }
    }

    /// Store a freshly decoded VIN; emit a `vin` record only when it changes.
    fn update_vin(&mut self, vin: &str, now: u64) {
        if self.vin.as_deref() == Some(vin) {
            return;
        }
        info!("DATA vin {vin}");
        self.vin = Some(vin.to_string());
        let payload = format!("{{\"vin\":\"{vin}\"}}");
        let json = self.envelope(OutboundKind::Vin, now, &payload);
        self.storage.enqueue_upload(&json);
        self.comms.enqueue(OutboundKind::Vin, json);
    }

    /// Store the active DTC set; emit a `fault_code` record only when it changes
    /// (DM1 broadcasts continuously, so re-emitting every frame would be noise).
    fn update_faults(&mut self, faults: decoder::Faults, now: u64) {
        if faults == self.faults {
            return;
        }
        self.faults = faults;
        info!("DATA faults active={}", self.faults.len());
        let payload = Self::faults_payload(&self.faults);
        let json = self.envelope(OutboundKind::FaultCode, now, &payload);
        self.storage.enqueue_upload(&json);
        self.comms.enqueue(OutboundKind::FaultCode, json);
    }

    /// `{"faults":[{"spn":..,"fmi":..,"oc":..}, ...]}`.
    fn faults_payload(faults: &[decoder::Dtc]) -> String {
        let mut items = String::new();
        for (i, d) in faults.iter().enumerate() {
            if i > 0 {
                items.push(',');
            }
            items.push_str(&format!(
                "{{\"spn\":{},\"fmi\":{},\"oc\":{}}}",
                d.spn, d.fmi, d.oc
            ));
        }
        format!("{{\"faults\":[{items}]}}")
    }

    /// Build the device-identity view served to `ELD INFO`.
    fn device_view(&self) -> DeviceInfoView {
        DeviceInfoView {
            device_id: self.device.device_id,
            firmware_version: self.device.firmware_version,
            vin: self.vin.clone(),
            sd_status: if self.storage.enabled() {
                "ok"
            } else {
                "missing"
            },
            input_voltage: PowerState::read().voltage,
        }
    }

    /// Build the live-telemetry view served to `ELD LIVE` (speed in mph).
    fn live_view(&self) -> LiveDataView {
        let now = time::now_ms();
        let s = self.telemetry.snapshot(now);
        let fix = self.gps.latest_fix(now);
        LiveDataView {
            rpm: s.rpm,
            speed_mph: s.speed_kph.map(|k| k / spn::KM_PER_MILE),
            fuel_percent: s.fuel_percent,
            engine_hours: s.engine_hours,
            latitude: fix.map(|value| value.lat),
            longitude: fix.map(|value| value.lon),
            gps_hdop: fix.and_then(|value| value.hdop),
            gps_fix_age_ms: fix.map(|value| value.age_ms(now)),
            gps_sentences: self.gps.sentences_seen(),
            faults: self
                .faults
                .iter()
                .map(|d| format!("SPN {} FMI {}", d.spn, d.fmi))
                .collect(),
        }
    }

    /// Log + persist a trip lifecycle event.
    fn handle_trip_event(&mut self, event: &TripEvent) {
        match event {
            TripEvent::Started { id } => {
                info!("TRIP started id={id}");
                self.storage.log_trip("started");
            }
            TripEvent::Stopped { id } => {
                info!("TRIP stopped id={id}");
                self.storage.log_trip("stopped");
            }
            TripEvent::Resumed { id } => {
                info!("TRIP resumed id={id}");
                self.storage.log_trip("resumed");
            }
            TripEvent::Completed(s) => {
                let m = &s.mileage;
                info!(
                    "TRIP completed id={} dur={}s miles={:.1} src={} conf={:.2} ({})",
                    s.id,
                    s.duration_ms() / 1000,
                    m.miles,
                    m.source.label(),
                    m.confidence,
                    m.notes,
                );
                self.storage.log_trip("completed");
                self.storage.log_mileage(m.miles, m.confidence);
                let payload = Self::trip_payload(s);
                let json = self.envelope(OutboundKind::TripSummary, s.end_ms, &payload);
                self.storage.enqueue_upload(&json);
                self.comms.enqueue(OutboundKind::TripSummary, json);
            }
        }
    }

    /// Wrap a payload object as an `EldRecord`. `ts_ms` is UTC Unix time or
    /// JSON null; uptime is always carried separately as `monotonic_ms`.
    fn envelope(&mut self, kind: OutboundKind, observed_ms: u64, payload: &str) -> String {
        let utc = self
            .gps
            .utc_at(observed_ms, time::now_ms())
            .map(|value| value.to_string())
            .unwrap_or_else(|| "null".to_string());
        format!(
            "{{\"device_id\":\"{}\",\"seq\":{},\"ts_ms\":{},\"monotonic_ms\":{},\"type\":\"{}\",\"data\":{}}}",
            self.device.device_id,
            self.seq.next(),
            utc,
            observed_ms,
            kind.label(),
            payload,
        )
    }

    /// Telemetry `data` object (inner; envelope adds device_id/seq/etc). Speed is
    /// converted to mph and keys match what the mobile app/server store.
    fn telemetry_payload(&self) -> String {
        let s = self.telemetry.snapshot(time::now_ms());
        let f = |v: Option<f32>| {
            v.map(|x| format!("{x:.3}"))
                .unwrap_or_else(|| "null".into())
        };
        let speed_mph = s.speed_kph.map(|k| k / spn::KM_PER_MILE);

        // Position and clock state travel with every snapshot. 49 CFR 395
        // Appendix A §4.5.1 requires latitude and longitude on each event, and
        // a timestamp the device can stand behind. Sending the position without
        // saying how old the fix is, or sending a time without saying whether
        // it is real UTC, would make the server guess.
        let stamp = self.storage.stamp();
        let fix = self.gps.latest_fix(stamp.monotonic_ms);
        let (lat, lon, fix_age_ms) = match fix {
            Some(fix) if fix.has_fix() => (
                format!("{:.6}", fix.lat),
                format!("{:.6}", fix.lon),
                format!("{}", fix.age_ms(stamp.monotonic_ms)),
            ),
            _ => ("null".to_string(), "null".to_string(), "null".to_string()),
        };
        let utc_ms = stamp
            .utc_ms
            .map(|v| v.to_string())
            .unwrap_or_else(|| "null".into());

        format!(
            "{{\"rpm\":{},\"speed_mph\":{},\"fuel_percent\":{},\"odometer_mi\":{},\"engine_hours\":{},\"lat\":{},\"lon\":{},\"fix_age_ms\":{},\"utc_ms\":{},\"clock\":\"{}\"}}",
            f(s.rpm),
            f(speed_mph),
            f(s.fuel_percent),
            f(s.odometer_mi),
            f(s.engine_hours),
            lat,
            lon,
            fix_age_ms,
            utc_ms,
            if stamp.utc_ms.is_some() { "utc" } else { "unsync" },
        )
    }

    /// Completed-trip payload object.
    fn trip_payload(s: &TripSummary) -> String {
        let m = &s.mileage;
        format!(
            "{{\"id\":{},\"start_ms\":{},\"end_ms\":{},\"miles\":{:.2},\"source\":\"{}\",\"confidence\":{:.2}}}",
            s.id,
            s.start_ms,
            s.end_ms,
            m.miles,
            m.source.label(),
            m.confidence,
        )
    }

    /// Log the squashed telemetry snapshot (missing signals shown as "--").
    fn log_telemetry(&self) {
        let s = self.telemetry.snapshot(time::now_ms());
        let f = |v: Option<f32>, prec: usize| match v {
            Some(x) => format!("{x:.*}", prec),
            None => "--".to_string(),
        };
        info!(
            "telemetry | rpm={} speed_kph={} fuel_pct={} odo_mi={} eng_hrs={} odo_priority={}/{}",
            f(s.rpm, 0),
            f(s.speed_kph, 1),
            f(s.fuel_percent, 1),
            f(s.odometer_mi, 1),
            f(s.engine_hours, 1),
            self.telemetry.distance.source_priority(),
            j1939::signal_map::best_priority_for(j1939::signal_map::SignalKind::Odometer),
        );
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

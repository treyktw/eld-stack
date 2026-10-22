//! BLE GATT phone relay.
//!
//! Service layout:
//! - command characteristic: phone writes ASCII commands (`ELD GET_LATEST`,
//!   `ELD PULL <from> <to>`, `ELD ACK <seq>`)
//! - response characteristic: phone subscribes to indications and receives
//!   framed `ELD ...` response lines.

#[cfg(feature = "bluetooth")]
use std::sync::{Arc, Mutex};

use esp_idf_svc::hal::modem::Modem;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
#[cfg(feature = "bluetooth")]
use log::{info, warn};

use super::phone::PhoneCommand;
use super::protocol::LinkType;

#[cfg(feature = "bluetooth")]
const MAX_COMMANDS: usize = 8;
#[cfg(feature = "bluetooth")]
const MAX_RESPONSES: usize = 24;
#[cfg(feature = "bluetooth")]
const MAX_COMMAND_LEN: usize = 96;
// The largest ATT payload permitted by the BLE spec (517-byte MTU minus the
// three-byte ATT header). Actual sends are capped to each phone's negotiated
// MTU, which is commonly much smaller on iOS.
#[cfg(feature = "bluetooth")]
const MAX_INDICATION_PAYLOAD: usize = 514;

#[cfg(feature = "bluetooth")]
pub const SERVICE_UUID: u128 = 0xfc59c4f2_bb03_4328_8361_267acb7e0842;
#[cfg(feature = "bluetooth")]
pub const COMMAND_CHARACTERISTIC_UUID: u128 = 0xfc59c4f3_bb03_4328_8361_267acb7e0842;
#[cfg(feature = "bluetooth")]
pub const RESPONSE_CHARACTERISTIC_UUID: u128 = 0xfc59c4f4_bb03_4328_8361_267acb7e0842;

#[derive(Debug, Clone, Copy)]
pub struct BluetoothStatus {
    pub enabled: bool,
    pub advertising: bool,
    pub connected: bool,
    pub subscribed: bool,
    pub commands_rx: u32,
    pub responses_tx: u32,
    pub queued_responses: usize,
    pub parse_errors: u32,
}

#[cfg(feature = "bluetooth")]
mod imp {
    use super::*;

    use enumset::enum_set;
    use esp_idf_svc::bt::ble::gap::{AdvConfiguration, BleGapEvent, EspBleGap};
    use esp_idf_svc::bt::ble::gatt::server::{ConnectionId, EspGatts, GattsEvent, TransferId};
    use esp_idf_svc::bt::ble::gatt::{
        AutoResponse, GattCharacteristic, GattDescriptor, GattId, GattInterface, GattResponse,
        GattServiceId, GattStatus, Handle, Permission, Property,
    };
    use esp_idf_svc::bt::{BdAddr, Ble, BtDriver, BtStatus, BtUuid};
    use esp_idf_svc::sys::{EspError, ESP_FAIL};
    use heapless::{Deque, Vec};

    const APP_ID: u16 = 0;
    // Commands and responses form one ordered session. Advertising stops on
    // connect, so accepting exactly one phone prevents one driver's response
    // from ever being delivered to a different subscribed client.
    const MAX_CONNECTIONS: usize = 1;
    const READVERTISE_DELAY_MS: u64 = 500;

    type Bt = BtDriver<'static, Ble>;
    type Gap = Arc<EspBleGap<'static, Ble, Arc<Bt>>>;
    type Gatts = Arc<EspGatts<'static, Ble, Arc<Bt>>>;

    #[derive(Debug, Clone)]
    struct Connection {
        peer: BdAddr,
        conn_id: Handle,
        subscribed: bool,
        mtu: Option<u16>,
    }

    #[derive(Default)]
    struct State {
        gatt_if: Option<GattInterface>,
        service_handle: Option<Handle>,
        command_handle: Option<Handle>,
        response_handle: Option<Handle>,
        response_cccd_handle: Option<Handle>,
        connections: heapless::Vec<Connection, MAX_CONNECTIONS>,
        response: GattResponse,
        commands: Deque<PhoneCommand, MAX_COMMANDS>,
        responses: Deque<std::string::String, MAX_RESPONSES>,
        response_offset: usize,
        indication_in_flight: bool,
        in_flight_len: usize,
        adv_configurations_pending: u8,
        advertising_configured: bool,
        advertising_start_pending: bool,
        advertising_not_before_ms: u64,
        advertising: bool,
        commands_rx: u32,
        responses_tx: u32,
        parse_errors: u32,
    }

    #[derive(Clone)]
    struct Server {
        gap: Gap,
        gatts: Gatts,
        state: Arc<Mutex<State>>,
    }

    impl Server {
        fn new(gap: Gap, gatts: Gatts) -> Self {
            Self {
                gap,
                gatts,
                state: Arc::new(Mutex::new(State::default())),
            }
        }

        fn start(&self) -> Result<(), EspError> {
            let gap_server = self.clone();
            self.gap.subscribe(move |event| {
                if let Err(e) = gap_server.on_gap_event(event) {
                    warn!("ble: GAP error ({e})");
                }
            })?;

            let gatts_server = self.clone();
            self.gatts.subscribe(move |(gatt_if, event)| {
                if let Err(e) = gatts_server.on_gatts_event(gatt_if, event) {
                    warn!("ble: GATTS error ({e})");
                }
            })?;

            self.gatts.register_app(APP_ID)
        }

        fn pop_command(&self) -> Option<PhoneCommand> {
            self.state.lock().unwrap().commands.pop_front()
        }

        fn enqueue_response(&self, line: &str) {
            let mut state = self.state.lock().unwrap();
            // Treat the characteristic as a newline-delimited byte stream.
            // A logical response may span several ATT indications.
            let mut msg = std::string::String::with_capacity(line.len() + 1);
            msg.push_str(line);
            msg.push('\n');
            if state.responses.is_full() {
                // Never evict the front: it may be awaiting confirmation and
                // advancing a replacement line with that confirmation would
                // corrupt the stream. Normal phone commands are serialized,
                // so reaching this guard indicates a misbehaving client.
                warn!("ble: response queue full; dropping newest response");
                return;
            }
            let _ = state.responses.push_back(msg);
        }

        fn flush(&self) {
            if let Err(e) = self.start_advertising_if_ready() {
                warn!("ble: advertising start failed ({e})");
            }
            let (gatt_if, response_handle, conn_id, chunk) = {
                let mut state = self.state.lock().unwrap();
                if state.indication_in_flight {
                    return;
                }
                let Some(gatt_if) = state.gatt_if else { return };
                let Some(response_handle) = state.response_handle else {
                    return;
                };
                let Some(conn_id) = state
                    .connections
                    .iter()
                    .find(|conn| conn.subscribed)
                    .map(|conn| conn.conn_id)
                else {
                    return;
                };
                let Some(line) = state.responses.front() else {
                    return;
                };
                let mtu = state
                    .connections
                    .iter()
                    .find(|conn| conn.conn_id == conn_id)
                    .and_then(|conn| conn.mtu)
                    .unwrap_or(23);
                let max_payload = usize::from(mtu)
                    .saturating_sub(3)
                    .clamp(1, MAX_INDICATION_PAYLOAD);
                let start = state.response_offset.min(line.len());
                let end = (start + max_payload).min(line.len());
                let mut chunk = Vec::<u8, MAX_INDICATION_PAYLOAD>::new();
                let _ = chunk.extend_from_slice(&line.as_bytes()[start..end]);
                state.indication_in_flight = true;
                state.in_flight_len = chunk.len();
                (gatt_if, response_handle, conn_id, chunk)
            };

            if self
                .gatts
                .indicate(gatt_if, conn_id, response_handle, chunk.as_slice())
                .is_err()
            {
                let mut state = self.state.lock().unwrap();
                state.indication_in_flight = false;
                state.in_flight_len = 0;
                warn!("ble: indication failed; response retained for retry");
            }
        }

        /// Advertising is started from the app loop after both the primary
        /// advertisement and scan-response payloads are configured.
        fn start_advertising_if_ready(&self) -> Result<(), EspError> {
            let now_ms = crate::time::now_ms();
            let should_start = {
                let mut state = self.state.lock().unwrap();
                let ready = state.advertising_configured
                    && state.adv_configurations_pending == 0
                    && now_ms >= state.advertising_not_before_ms
                    && !state.advertising_start_pending
                    && !state.advertising
                    && state.connections.is_empty();
                if ready {
                    state.advertising_start_pending = true;
                }
                ready
            };
            if should_start {
                if let Err(error) = self.gap.start_advertising() {
                    self.state.lock().unwrap().advertising_start_pending = false;
                    return Err(error);
                }
            }
            Ok(())
        }

        fn status(&self) -> BluetoothStatus {
            let state = self.state.lock().unwrap();
            BluetoothStatus {
                enabled: true,
                advertising: state.advertising,
                connected: !state.connections.is_empty(),
                subscribed: state.connections.iter().any(|conn| conn.subscribed),
                commands_rx: state.commands_rx,
                responses_tx: state.responses_tx,
                queued_responses: state.responses.len(),
                parse_errors: state.parse_errors,
            }
        }

        fn on_gap_event(&self, event: BleGapEvent) -> Result<(), EspError> {
            match event {
                BleGapEvent::AdvertisingConfigured(status)
                | BleGapEvent::ScanResponseConfigured(status) => {
                    check_bt_status(status)?;
                    let mut state = self.state.lock().unwrap();
                    state.adv_configurations_pending =
                        state.adv_configurations_pending.saturating_sub(1);
                    state.advertising_configured = state.adv_configurations_pending == 0;
                }
                BleGapEvent::AdvertisingStarted(status) => {
                    check_bt_status(status)?;
                    let mut state = self.state.lock().unwrap();
                    state.advertising_start_pending = false;
                    state.advertising = true;
                    drop(state);
                    info!("ble: advertising as ELD-DONGLE");
                }
                BleGapEvent::AdvertisingStopped(status) => {
                    check_bt_status(status)?;
                    let mut state = self.state.lock().unwrap();
                    state.advertising_start_pending = false;
                    state.advertising = false;
                }
                _ => {}
            }
            Ok(())
        }

        fn on_gatts_event(
            &self,
            gatt_if: GattInterface,
            event: GattsEvent,
        ) -> Result<(), EspError> {
            match event {
                GattsEvent::ServiceRegistered { status, app_id } => {
                    check_gatt_status(status)?;
                    if app_id == APP_ID {
                        self.create_service(gatt_if)?;
                    }
                }
                GattsEvent::ServiceCreated {
                    status,
                    service_handle,
                    ..
                } => {
                    check_gatt_status(status)?;
                    self.state.lock().unwrap().service_handle = Some(service_handle);
                    self.gatts.start_service(service_handle)?;
                    self.add_characteristics(service_handle)?;
                }
                GattsEvent::CharacteristicAdded {
                    status,
                    attr_handle,
                    service_handle,
                    char_uuid,
                } => {
                    check_gatt_status(status)?;
                    self.register_characteristic(service_handle, attr_handle, char_uuid)?;
                }
                GattsEvent::DescriptorAdded {
                    status,
                    attr_handle,
                    service_handle,
                    descr_uuid,
                } => {
                    check_gatt_status(status)?;
                    self.register_descriptor(service_handle, attr_handle, descr_uuid)?;
                }
                GattsEvent::PeerConnected { conn_id, addr, .. } => {
                    self.add_connection(conn_id, addr)?;
                }
                GattsEvent::PeerDisconnected { addr, .. } => {
                    self.remove_connection(addr)?;
                }
                GattsEvent::Mtu { conn_id, mtu } => {
                    self.register_mtu(conn_id, mtu);
                }
                GattsEvent::Confirm { status, .. } => {
                    self.confirm_indication(status)?;
                }
                GattsEvent::Write {
                    conn_id,
                    trans_id,
                    addr,
                    handle,
                    offset,
                    need_rsp,
                    is_prep,
                    value,
                } => {
                    let handled = self.recv(conn_id, addr, handle, offset, value)?;
                    if handled {
                        self.send_write_response(
                            gatt_if, conn_id, trans_id, handle, offset, need_rsp, is_prep, value,
                        )?;
                    }
                }
                _ => {}
            }
            Ok(())
        }

        /// Advertise the service UUID, and put the name in the scan response.
        ///
        /// iOS cannot scan by device name, and CoreBluetooth ignores a
        /// service-less background scan entirely — so a name-only
        /// advertisement is invisible to a backgrounded iPhone, which is
        /// exactly the state a driver's phone is in. The UUID has to be in the
        /// advertisement itself for background discovery and reconnection to
        /// work.
        ///
        /// Both cannot share one packet: a 128-bit UUID is 18 bytes, the flags
        /// are 3, and "ELD-DONGLE" is 12 — over the 31-byte limit. The name
        /// therefore moves to the scan response, which costs nothing because
        /// any scanner that wants it will ask.
        fn configure_advertising(&self) -> Result<(), EspError> {
            {
                let mut state = self.state.lock().unwrap();
                state.adv_configurations_pending = 2;
                state.advertising_configured = false;
                state.advertising_start_pending = false;
                state.advertising_not_before_ms = 0;
                state.advertising = false;
            }
            self.gap.set_adv_conf(&AdvConfiguration {
                set_scan_rsp: false,
                include_name: false,
                include_txpower: false,
                // LE General Discoverable | BR/EDR Not Supported. The previous
                // value omitted the BR/EDR bit that BLE-only peripherals set.
                flag: 6,
                service_uuid: Some(BtUuid::uuid128(SERVICE_UUID)),
                ..Default::default()
            })?;
            self.gap.set_adv_conf(&AdvConfiguration {
                set_scan_rsp: true,
                include_name: true,
                ..Default::default()
            })
        }

        fn create_service(&self, gatt_if: GattInterface) -> Result<(), EspError> {
            self.state.lock().unwrap().gatt_if = Some(gatt_if);
            self.gap.set_device_name("ELD-DONGLE")?;
            self.configure_advertising()?;
            self.gatts.create_service(
                gatt_if,
                &GattServiceId {
                    id: GattId {
                        uuid: BtUuid::uuid128(SERVICE_UUID),
                        inst_id: 0,
                    },
                    is_primary: true,
                },
                8,
            )
        }

        fn add_characteristics(&self, service_handle: Handle) -> Result<(), EspError> {
            self.gatts.add_characteristic(
                service_handle,
                &GattCharacteristic {
                    uuid: BtUuid::uuid128(COMMAND_CHARACTERISTIC_UUID),
                    permissions: enum_set!(Permission::Write),
                    properties: enum_set!(Property::Write),
                    max_len: MAX_COMMAND_LEN,
                    auto_rsp: AutoResponse::ByApp,
                },
                &[],
            )?;

            self.gatts.add_characteristic(
                service_handle,
                &GattCharacteristic {
                    uuid: BtUuid::uuid128(RESPONSE_CHARACTERISTIC_UUID),
                    permissions: enum_set!(Permission::Read),
                    properties: enum_set!(Property::Indicate),
                    max_len: MAX_INDICATION_PAYLOAD,
                    auto_rsp: AutoResponse::ByApp,
                },
                &[],
            )
        }

        fn register_characteristic(
            &self,
            service_handle: Handle,
            attr_handle: Handle,
            char_uuid: BtUuid,
        ) -> Result<(), EspError> {
            let add_cccd = {
                let mut state = self.state.lock().unwrap();
                if state.service_handle != Some(service_handle) {
                    false
                } else if char_uuid == BtUuid::uuid128(COMMAND_CHARACTERISTIC_UUID) {
                    state.command_handle = Some(attr_handle);
                    false
                } else if char_uuid == BtUuid::uuid128(RESPONSE_CHARACTERISTIC_UUID) {
                    state.response_handle = Some(attr_handle);
                    true
                } else {
                    false
                }
            };
            if add_cccd {
                self.gatts.add_descriptor(
                    service_handle,
                    &GattDescriptor {
                        uuid: BtUuid::uuid16(0x2902),
                        permissions: enum_set!(Permission::Read | Permission::Write),
                    },
                )?;
            }
            Ok(())
        }

        fn register_descriptor(
            &self,
            service_handle: Handle,
            attr_handle: Handle,
            descr_uuid: BtUuid,
        ) -> Result<(), EspError> {
            let mut state = self.state.lock().unwrap();
            if state.service_handle == Some(service_handle) && descr_uuid == BtUuid::uuid16(0x2902)
            {
                state.response_cccd_handle = Some(attr_handle);
            }
            Ok(())
        }

        fn add_connection(&self, conn_id: ConnectionId, addr: BdAddr) -> Result<(), EspError> {
            let mut state = self.state.lock().unwrap();
            // Legacy advertising stops automatically when a connection is
            // accepted; not every controller reports a separate stop event.
            state.advertising = false;
            state.advertising_start_pending = false;
            if state.connections.len() < MAX_CONNECTIONS {
                let _ = state.connections.push(Connection {
                    peer: addr,
                    conn_id,
                    subscribed: false,
                    mtu: None,
                });
            }
            Ok(())
        }

        fn remove_connection(&self, addr: BdAddr) -> Result<(), EspError> {
            let mut state = self.state.lock().unwrap();
            if let Some(index) = state.connections.iter().position(|conn| conn.peer == addr) {
                state.connections.swap_remove(index);
            }
            state.advertising = false;
            state.advertising_start_pending = false;
            // The payload remains valid across a connection. Reconfiguring it
            // from this Bluedroid callback can deadlock; `flush` restarts the
            // existing advertisement from the app task instead.
            state.advertising_configured = true;
            state.adv_configurations_pending = 0;
            state.advertising_not_before_ms =
                crate::time::now_ms().saturating_add(READVERTISE_DELAY_MS);
            if state.connections.is_empty() {
                state.responses.clear();
                state.response_offset = 0;
                state.indication_in_flight = false;
                state.in_flight_len = 0;
            }
            Ok(())
        }

        fn confirm_indication(&self, status: GattStatus) -> Result<(), EspError> {
            let ok = matches!(status, GattStatus::Ok);
            let mut state = self.state.lock().unwrap();
            state.indication_in_flight = false;
            if ok {
                state.response_offset = state.response_offset.saturating_add(state.in_flight_len);
                let complete = state
                    .responses
                    .front()
                    .map(|line| state.response_offset >= line.len())
                    .unwrap_or(false);
                if complete {
                    let _ = state.responses.pop_front();
                    state.response_offset = 0;
                    state.responses_tx = state.responses_tx.wrapping_add(1);
                }
            }
            state.in_flight_len = 0;
            drop(state);
            check_gatt_status(status)
        }

        fn register_mtu(&self, conn_id: ConnectionId, mtu: u16) {
            let mut state = self.state.lock().unwrap();
            if let Some(conn) = state
                .connections
                .iter_mut()
                .find(|conn| conn.conn_id == conn_id)
            {
                conn.mtu = Some(mtu);
            }
        }

        fn recv(
            &self,
            conn_id: ConnectionId,
            _addr: BdAddr,
            handle: Handle,
            offset: u16,
            value: &[u8],
        ) -> Result<bool, EspError> {
            let mut state = self.state.lock().unwrap();
            let command_handle = state.command_handle;
            let cccd_handle = state.response_cccd_handle;

            let Some(conn) = state
                .connections
                .iter_mut()
                .find(|conn| conn.conn_id == conn_id)
            else {
                return Ok(false);
            };

            if Some(handle) == cccd_handle {
                if offset == 0 && value.len() == 2 {
                    let cccd = u16::from_le_bytes([value[0], value[1]]);
                    conn.subscribed = cccd == 0x02;
                }
                return Ok(true);
            }

            if Some(handle) != command_handle || offset != 0 {
                return Ok(false);
            }

            let Ok(line) = core::str::from_utf8(value) else {
                state.parse_errors = state.parse_errors.wrapping_add(1);
                return Ok(true);
            };
            match PhoneCommand::parse(line.trim()) {
                Some(command) => {
                    if state.commands.is_full() {
                        let _ = state.commands.pop_front();
                    }
                    let _ = state.commands.push_back(command);
                    state.commands_rx = state.commands_rx.wrapping_add(1);
                }
                None => {
                    state.parse_errors = state.parse_errors.wrapping_add(1);
                }
            }
            Ok(true)
        }

        #[allow(clippy::too_many_arguments)]
        fn send_write_response(
            &self,
            gatt_if: GattInterface,
            conn_id: ConnectionId,
            trans_id: TransferId,
            handle: Handle,
            offset: u16,
            need_rsp: bool,
            is_prep: bool,
            value: &[u8],
        ) -> Result<(), EspError> {
            if !need_rsp {
                return Ok(());
            }
            if is_prep {
                let mut state = self.state.lock().unwrap();
                state
                    .response
                    .attr_handle(handle)
                    .auth_req(0)
                    .offset(offset)
                    .value(value)
                    .map_err(|_| EspError::from_infallible::<ESP_FAIL>())?;
                self.gatts.send_response(
                    gatt_if,
                    conn_id,
                    trans_id,
                    GattStatus::Ok,
                    Some(&state.response),
                )
            } else {
                self.gatts
                    .send_response(gatt_if, conn_id, trans_id, GattStatus::Ok, None)
            }
        }
    }

    pub struct BluetoothLink {
        _bt: Arc<Bt>,
        server: Server,
    }

    impl BluetoothLink {
        pub fn open(
            modem: Modem<'static>,
            nvs: EspDefaultNvsPartition,
        ) -> Result<Self, crate::error::EldError> {
            let bt = Arc::new(
                BtDriver::new(modem, Some(nvs))
                    .map_err(|_| crate::error::EldError::Comms("ble init"))?,
            );
            let gap = Arc::new(
                EspBleGap::new(bt.clone()).map_err(|_| crate::error::EldError::Comms("ble gap"))?,
            );
            let gatts = Arc::new(
                EspGatts::new(bt.clone())
                    .map_err(|_| crate::error::EldError::Comms("ble gatts"))?,
            );
            let server = Server::new(gap, gatts);
            server
                .start()
                .map_err(|_| crate::error::EldError::Comms("ble start"))?;
            info!("ble: GATT relay initialized; preparing ELD-DONGLE advertisement");
            Ok(Self { _bt: bt, server })
        }

        pub fn pop_command(&mut self) -> Option<PhoneCommand> {
            self.server.pop_command()
        }

        pub fn enqueue_response_line(&mut self, line: &str) {
            self.server.enqueue_response(line);
        }

        pub fn flush(&mut self) {
            self.server.flush();
        }

        pub fn status(&self) -> BluetoothStatus {
            self.server.status()
        }

        pub fn link_type(&self) -> LinkType {
            LinkType::Bluetooth
        }
    }

    fn check_bt_status(status: BtStatus) -> Result<(), EspError> {
        if matches!(status, BtStatus::Success) {
            Ok(())
        } else {
            Err(EspError::from_infallible::<ESP_FAIL>())
        }
    }

    fn check_gatt_status(status: GattStatus) -> Result<(), EspError> {
        if matches!(status, GattStatus::Ok) {
            Ok(())
        } else {
            Err(EspError::from_infallible::<ESP_FAIL>())
        }
    }
}

#[cfg(not(feature = "bluetooth"))]
mod imp {
    use super::*;

    pub struct BluetoothLink;

    impl BluetoothLink {
        pub fn open(
            _modem: Modem<'static>,
            _nvs: EspDefaultNvsPartition,
        ) -> Result<Self, crate::error::EldError> {
            Err(crate::error::EldError::Comms("ble not enabled"))
        }

        pub fn pop_command(&mut self) -> Option<PhoneCommand> {
            None
        }

        pub fn enqueue_response_line(&mut self, _line: &str) {}

        pub fn flush(&mut self) {}

        pub fn status(&self) -> BluetoothStatus {
            BluetoothStatus {
                enabled: false,
                advertising: false,
                connected: false,
                subscribed: false,
                commands_rx: 0,
                responses_tx: 0,
                queued_responses: 0,
                parse_errors: 0,
            }
        }

        pub fn link_type(&self) -> LinkType {
            LinkType::Bluetooth
        }
    }
}

pub use imp::BluetoothLink;

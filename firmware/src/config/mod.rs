//! Build-time configuration: pins, constants, feature flags.
//!
//! Phase 1 keeps this flat. It splits into board/pins/constants/profiles/
//! feature_flags submodules as the firmware grows.

pub const BOOT_MODE: &str = "development";

// ---- CAN / TWAI -----------------------------------------------------------

/// TWAI TX pin (to an external CAN transceiver).
///
/// GPIO4/5 are reserved for the onboard SIM7670G on this board. GPIO15/16 are
/// available when the optional camera is not fitted, which is the ELD build.
pub const CAN_TX_PIN: u8 = 15;
/// TWAI RX pin (from an external CAN transceiver).
pub const CAN_RX_PIN: u8 = 16;
/// Physical diagnostic connector pin pair wired to the CAN transceiver.
///
/// Valid: "3/11" (J1962 CAN2, J1939), "6/14" (J1962 CAN1, ISO15765/OBD),
/// "C/D" (Deutsch 9-pin J1939), "F/G" (Deutsch 9-pin J1708 — not CAN).
///
/// Defaults to 6/14, which is the pair that actually produced frames on the
/// 2013 Volvo field test. Vendor documentation predicts 3/11 for that chassis;
/// the truck disagreed. Treat this as the starting candidate, not the answer —
/// `PROBE_ORDER` in `can::pins` is the list to work through.
pub const CAN_CONNECTOR_PINS: &str = "6/14";

/// GPIO driving a CAN channel mux/relay, if one is fitted.
///
/// The ESP32 has a single TWAI controller on a single transceiver, so it can
/// only ever watch one connector pin pair. Probing 6/14 against 3/11 — which
/// the 2013 Volvo showed is necessary, since the pair that worked was not the
/// documented one — needs a mux to switch the transceiver between them.
/// `None` means no mux: the device reports only on the pair it is wired to,
/// and a "no traffic" result says nothing about the other pair.
pub const CAN_CHANNEL_SELECT_GPIO: Option<u8> = None;

/// Run the TWAI controller in listen-only mode: receive without ACKing and
/// never transmit. Use on a bench/tap where this node must stay silent, or
/// where a lone Normal-mode controller would go bus-off with nothing to ACK.
/// Disables PGN requests.
pub const CAN_LISTEN_ONLY: bool = false;

// ---- SD card (SPI) --------------------------------------------------------
// Onboard TF socket, SPI2: SCLK=gpio12, MOSI=gpio11, MISO=gpio13, CS=gpio10.
// Mounted at "/sd".

/// Conservative SD-SPI speed for bring-up. Raise after hardware is proven.
pub const SD_SPI_SPEED_KHZ: u32 = 400;
/// Format a card only after card init succeeds but FAT mount fails.
pub const SD_FORMAT_ON_FIRST_USE: bool = false;
/// Bring-up escape hatch: format the SD card before mounting.
/// Keep this false unless you intentionally want to wipe the card.
pub const SD_FORMAT_ON_BOOT: bool = false;

// ---- Scanner / request timing (milliseconds) ------------------------------

/// How long to listen at each candidate bitrate before deciding.
pub const CAN_SCAN_MS: u64 = 8_000;
/// After this long without seeing a wanted PGN, start requesting it.
pub const REQUEST_AFTER_MS: u64 = 15_000;
/// Minimum gap between repeated requests for the same PGN.
pub const REQUEST_INTERVAL_MS: u64 = 3_000;
/// Heartbeat log cadence.
pub const HEARTBEAT_MS: u64 = 5_000;
/// Self-check / diagnostic report cadence.
pub const DIAGNOSTICS_MS: u64 = 30_000;

// ---- Bus census -----------------------------------------------------------
//
// The census is how an unfamiliar truck becomes analysable: every (PGN, source)
// it emits, with rate and last payload. Written to SD on a slow cadence, and
// uploaded once per boot after the bus has settled — it is diagnostic data, not
// a compliance record, so it should not compete for cellular bytes.

/// How often the census is written to SD.
pub const CENSUS_LOG_MS: u64 = 300_000;
/// Delay after boot before the census is uploaded, so the bus has been heard.
pub const CENSUS_UPLOAD_AFTER_MS: u64 = 120_000;
/// Max entries per census export, to bound message size.
pub const CENSUS_EXPORT_MAX: usize = 160;

// ---- GPS (UART) -----------------------------------------------------------
// SIM7670G GNSS NMEA port on UART1: ESP32 TX=gpio48 → modem GPS RX,
// ESP32 RX=gpio45 ← modem GPS TX. The main modem AT UART is separate.

pub const GPS_BAUD: u32 = 115_200;
/// Ignore per-fix movement below this (metres) — filters stationary jitter.
pub const GPS_MIN_MOVE_M: f32 = 3.0;
/// Reject fixes with HDOP worse (larger) than this.
pub const GPS_MAX_HDOP: f32 = 5.0;
/// A coordinate older than this is not a current fix and must not be emitted as
/// though the receiver still has position.
pub const GPS_FIX_MAX_AGE_MS: u64 = 10_000;

// ---- Signal freshness -----------------------------------------------------

/// Motion and engine-running state must disappear promptly when the ECM stops
/// reporting. Slower lifetime counters are allowed a longer broadcast gap.
pub const FAST_SIGNAL_MAX_AGE_MS: u64 = 5_000;
pub const SLOW_SIGNAL_MAX_AGE_MS: u64 = 30_000;
pub const FUEL_SIGNAL_MAX_AGE_MS: u64 = 60_000;

// ---- Trip detection -------------------------------------------------------
// Hysteresis: start threshold is higher than the stop threshold so noise near
// zero speed doesn't flap the state machine.

/// Speed at/above which an Idle vehicle is considered to have started moving.
pub const TRIP_START_SPEED_KPH: f32 = 5.0;
/// Speed at/below which a Driving vehicle is considered stopped.
pub const TRIP_STOP_SPEED_KPH: f32 = 2.0;
/// Continuous stopped time after which a trip is completed.
pub const TRIP_STOP_TIMEOUT_MS: u64 = 300_000; // 5 minutes

// ---- Comms / phone relay --------------------------------------------------

/// Max JSON envelopes returned to the phone in a single backfill batch.
pub const PHONE_BATCH_MAX_EVENTS: usize = 16;

/// Hologram's packet-data APN. No username or password is required.
pub const CELLULAR_APN: &str = "hologram";

pub const WIFI_SSID: &str = "";
pub const WIFI_PASS: &str = "";
/// Endpoint that accepts POSTed envelopes (ingest-server /ingest).
pub const UPLOAD_URL: &str = match option_env!("ELD_UPLOAD_URL") {
    Some(value) => value,
    None => "https://example.invalid/ingest",
};
/// Bearer token sent as `Authorization: Bearer <token>` (server INGEST_TOKEN).
/// Empty = no auth header.
pub const UPLOAD_TOKEN: &str = match option_env!("ELD_UPLOAD_TOKEN") {
    Some(value) => value,
    None => "",
};
/// Min gap between upload attempts on success (drains queue at this rate).
pub const UPLOAD_INTERVAL_MS: u64 = 1_000;
/// Envelopes sent per POST.
///
/// Each request pays a TLS handshake of several kilobytes against an envelope of
/// a few hundred bytes, so batching is the single biggest lever on the cellular
/// data bill. The ingest endpoint accepts up to 250 records per batch; this
/// stays well under that to bound RAM and request size.
pub const UPLOAD_BATCH_MAX: usize = 32;
/// Backoff bounds for failed uploads.
pub const UPLOAD_BACKOFF_BASE_MS: u64 = 2_000;
pub const UPLOAD_BACKOFF_MAX_MS: u64 = 60_000;
// ---- Telemetry cadence ----------------------------------------------------
//
// Telemetry is an operational nicety, not a compliance record — duty-status and
// trip events are enqueued the moment they happen and are not affected by any
// of this. A fixed 30-second snapshot meant ~2,880 uploads a day per truck,
// almost all of them repeating what the last one said while parked. Sending on
// meaningful change with a slow heartbeat carries the same information for a
// fraction of the bytes.

/// Never send telemetry more often than this, however fast things change.
pub const TELEMETRY_MIN_INTERVAL_MS: u64 = 30_000;
/// Send unconditionally at least this often, so the server can tell "nothing
/// changed" apart from "the device is gone".
pub const TELEMETRY_HEARTBEAT_MS: u64 = 300_000;
/// Speed change that counts as worth reporting.
pub const TELEMETRY_SPEED_DELTA_KPH: f32 = 8.0;
/// Distance travelled that counts as worth reporting (~1 mile).
pub const TELEMETRY_ODOMETER_DELTA_KM: f32 = 1.6;
/// Engine-hours change that counts as worth reporting.
pub const TELEMETRY_ENGINE_HOURS_DELTA: f32 = 0.25;

// ---- Feature flags --------------------------------------------------------
//
// BLE is a GATT phone relay, not classic Bluetooth pairing. Use a BLE client
// such as nRF Connect or the phone app, not the phone's pairing settings.

// ---- OBD-II ---------------------------------------------------------------
/// Poll OBD-II (SAE J1979) Mode 01 alongside J1939.
///
/// Separate from ENABLE_PGN_REQUESTS because they are different buses' worth of
/// traffic on the same wire: J1939 requests are 29-bit, OBD requests are 11-bit
/// to 0x7DF. A vehicle may answer one, both, or neither.
pub const ENABLE_OBD_POLLING: bool = true;

pub const ENABLE_PGN_REQUESTS: bool = true;
pub const ENABLE_SD_LOGGING: bool = true;
pub const ENABLE_CELLULAR: bool = true;
/// Start network registration and uploads. Keep false until the Hologram SIM
/// is installed; the modem itself and GNSS still run with ENABLE_CELLULAR.
/// Enabled when `ELD_CELLULAR_UPLINK` exists at build time. Omit the variable
/// entirely for a bench/no-SIM build; its textual value is intentionally not
/// parsed in this compile-time constant.
pub const ENABLE_CELLULAR_UPLINK: bool = option_env!("ELD_CELLULAR_UPLINK").is_some();
pub const ENABLE_WIFI_UPLOAD: bool = false;
// SD, modem power, and GNSS have passed board bring-up; expose the GATT phone
// relay while cellular registration remains independently paused until a SIM
// is installed.
pub const ENABLE_BLUETOOTH: bool = cfg!(feature = "bluetooth");
pub const ENABLE_CABLE_RELAY: bool = true;
pub const ENABLE_GPS: bool = true;

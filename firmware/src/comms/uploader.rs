//! Transport abstraction over the active link.
//!
//! `Null` is always offline and drops sends (used when uploads are disabled or
//! the link failed to come up). `Wifi` ships JSON via HTTP POST.

use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::modem::Modem;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use log::warn;

use super::cellular::uplink::CellularLink;
use super::protocol::{LinkType, OutboundMessage};
use super::wifi::WifiLink;
use crate::config;

pub enum Transport {
    Null,
    Wifi(Box<WifiLink>),
    /// Modem uplink. Primary on the road — the record path must not depend on
    /// a driver's phone being alive, charged and in range.
    Cellular(Box<CellularLink>),
}

impl Transport {
    /// Try to bring up WiFi; fall back to `Null` on any failure.
    pub fn wifi_or_null(
        modem: Modem<'static>,
        sysloop: EspSystemEventLoop,
        nvs: EspDefaultNvsPartition,
    ) -> Self {
        match WifiLink::connect(modem, sysloop, nvs) {
            Ok(link) => Transport::Wifi(Box::new(link)),
            Err(e) => {
                warn!("comms: wifi link failed ({e}); offline");
                Transport::Null
            }
        }
    }

    pub fn online(&self) -> bool {
        match self {
            Transport::Null => false,
            Transport::Wifi(link) => link.is_up(),
            Transport::Cellular(link) => link.is_up(),
        }
    }

    /// Advance any link that needs periodic work.
    ///
    /// Cellular bring-up is a state machine that has to be stepped; WiFi
    /// manages itself and Null has nothing to do.
    pub fn tick(&mut self, now_ms: u64) {
        if let Transport::Cellular(link) = self {
            link.tick(now_ms);
        }
    }

    pub fn link_type(&self) -> LinkType {
        match self {
            Transport::Null => LinkType::Disabled,
            Transport::Wifi(_) => LinkType::Wifi,
            Transport::Cellular(_) => LinkType::Cellular,
        }
    }

    pub fn rssi_dbm(&self) -> Option<i16> {
        match self {
            Transport::Cellular(link) => link.rssi_dbm(),
            Transport::Null | Transport::Wifi(_) => None,
        }
    }

    pub fn cellular_backoff_ms(&self) -> Option<u64> {
        match self {
            Transport::Cellular(link) => Some(link.backoff_ms()),
            Transport::Null | Transport::Wifi(_) => None,
        }
    }

    /// Send one message. `Err(())` means try again later (stay queued).
    pub fn send(&mut self, msg: &OutboundMessage) -> Result<(), ()> {
        match self {
            Transport::Null => Err(()),
            Transport::Cellular(link) => link.post(
                config::UPLOAD_URL,
                msg.payload.as_bytes(),
                crate::time::now_ms(),
            ),
            Transport::Wifi(link) => match link.post(config::UPLOAD_URL, msg.payload.as_bytes()) {
                Ok(status) if (200..300).contains(&status) => Ok(()),
                Ok(status) => {
                    warn!("comms: upload HTTP {status}");
                    Err(())
                }
                Err(e) => {
                    warn!("comms: upload error ({e})");
                    Err(())
                }
            },
        }
    }
}

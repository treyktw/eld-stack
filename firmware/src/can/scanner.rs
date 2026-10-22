//! Bitrate scanner — the Rust port of the Arduino `scanBitrate`.
//!
//! Tries each candidate bitrate in turn, listens for valid extended (J1939)
//! traffic, and keeps the first that responds. If none do, it leaves the bus
//! up at the last bitrate so requests can still be attempted.
//!
//! The TWAI peripheral and its pins are singletons. We `steal` fresh handles
//! per attempt, which is sound here because each attempt's driver is fully
//! dropped (uninstalled) before the next `steal`, and the scan is single
//! threaded.

use esp_idf_svc::hal::can::CAN;
use esp_idf_svc::hal::gpio::AnyIOPin;
use log::{info, warn};

use super::bitrate::CanBitrate;
use super::driver::CanDriver;
use super::errors::CanError;
use super::pins::CanWiring;
use crate::j1939::id::J1939Id;
use crate::{config, time};

/// Scan all bitrates and return a *started* driver on the chosen one.
pub fn scan(wiring: CanWiring) -> Result<CanDriver<'static>, CanError> {
    let mut last: Option<CanDriver<'static>> = None;

    for &br in CanBitrate::SCAN_ORDER.iter() {
        // Drop the previous attempt's driver first (uninstalls it) so the steal
        // below grabs an idle peripheral.
        drop(last.take());

        // SAFETY: single-threaded scan; no other live driver/pin handles exist.
        let can = unsafe { CAN::steal() };
        let tx = unsafe { AnyIOPin::steal(wiring.tx_gpio) };
        let rx = unsafe { AnyIOPin::steal(wiring.rx_gpio) };

        let mut drv = CanDriver::install(can, tx, rx, br, config::CAN_LISTEN_ONLY)?;
        drv.start()?;
        info!(
            "can: listening at {} for {}ms (listen_only={})",
            br.label(),
            config::CAN_SCAN_MS,
            config::CAN_LISTEN_ONLY,
        );

        if listen_for_traffic(&drv, config::CAN_SCAN_MS) {
            info!("can: J1939 traffic detected at {}", br.label());
            return Ok(drv);
        }

        warn!("can: no J1939 traffic at {}", br.label());
        let _ = drv.stop();
        last = Some(drv);
    }

    warn!("can: no traffic on any bitrate; leaving bus up at last candidate");
    let mut drv = last.expect("SCAN_ORDER is non-empty");
    drv.start()?;
    Ok(drv)
}

/// Listen up to `window_ms` for at least one valid extended J1939 frame.
fn listen_for_traffic(drv: &CanDriver, window_ms: u64) -> bool {
    let start = time::now_ms();
    while time::elapsed_ms(start) < window_ms {
        if let Ok(Some(frame)) = drv.receive(200) {
            if frame.is_j1939_candidate() && J1939Id::parse(frame.id).is_some() {
                return true;
            }
        }
    }
    false
}

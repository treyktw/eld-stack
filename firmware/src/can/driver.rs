//! Thin wrapper over the ESP-IDF TWAI (CAN) driver.
//!
//! Owns the installed `twai` driver. Constructing installs it; dropping
//! uninstalls it (HAL `Drop`). Receive distinguishes a benign timeout
//! (`Ok(None)`) from a real bus error (`Err`).

use esp_idf_svc::hal::can::{config, CanDriver as HalCanDriver, Frame as HalFrame, CAN};
use esp_idf_svc::hal::delay::TickType;
use esp_idf_svc::hal::gpio::AnyIOPin;
use esp_idf_svc::sys::ESP_ERR_TIMEOUT;

use super::bitrate::CanBitrate;
use super::errors::CanError;
use super::frame::CanFrame;

/// Transmit timeout for queued request frames.
const TX_TIMEOUT_MS: u64 = 50;

pub struct CanDriver<'d> {
    inner: HalCanDriver<'d>,
    bitrate: CanBitrate,
    listen_only: bool,
}

impl<'d> CanDriver<'d> {
    /// Install (but do not start) the TWAI driver at `bitrate`, accepting all
    /// IDs. Normal mode can transmit PGN requests but needs another node to
    /// ACK; ListenOnly receives silently and never transmits.
    pub fn install(
        can: CAN<'d>,
        tx: AnyIOPin<'d>,
        rx: AnyIOPin<'d>,
        bitrate: CanBitrate,
        listen_only: bool,
    ) -> Result<Self, CanError> {
        let mode = if listen_only {
            config::Mode::ListenOnly
        } else {
            config::Mode::Normal
        };
        // Default filter is "standard_allow_all", whose mask is all-ones and so
        // accepts every frame (standard and extended).
        let cfg = config::Config::new()
            .timing(bitrate.timing())
            .mode(mode)
            .rx_queue_len(64)
            .tx_queue_len(8);

        let inner =
            HalCanDriver::new(can, tx, rx, &cfg).map_err(|_| CanError::Driver("install failed"))?;
        Ok(Self {
            inner,
            bitrate,
            listen_only,
        })
    }

    pub fn listen_only(&self) -> bool {
        self.listen_only
    }

    pub fn start(&mut self) -> Result<(), CanError> {
        self.inner
            .start()
            .map_err(|_| CanError::Driver("start failed"))
    }

    pub fn stop(&mut self) -> Result<(), CanError> {
        self.inner
            .stop()
            .map_err(|_| CanError::Driver("stop failed"))
    }

    pub fn bitrate(&self) -> CanBitrate {
        self.bitrate
    }

    /// Receive one frame, waiting up to `timeout_ms`.
    /// `Ok(None)` means the wait elapsed with no frame.
    pub fn receive(&self, timeout_ms: u32) -> Result<Option<CanFrame>, CanError> {
        let ticks = TickType::new_millis(timeout_ms as u64).ticks();
        match self.inner.receive(ticks) {
            Ok(f) => Ok(Some(CanFrame::from_hal(&f))),
            Err(e) if e.code() == ESP_ERR_TIMEOUT => Ok(None),
            Err(_) => Err(CanError::Receive),
        }
    }

    pub fn transmit(&self, frame: &HalFrame) -> Result<(), CanError> {
        if self.listen_only {
            return Err(CanError::Transmit);
        }
        let ticks = TickType::new_millis(TX_TIMEOUT_MS).ticks();
        self.inner
            .transmit(frame, ticks)
            .map_err(|_| CanError::Transmit)
    }
}

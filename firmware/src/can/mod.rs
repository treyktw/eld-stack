//! CAN/TWAI subsystem: driver, bitrate scan, raw frames, bus stats.

pub mod bitrate;
pub mod bus_profile;
pub mod channel;
pub mod channel_probe_pure;
pub mod driver;
pub mod errors;
pub mod frame;
pub mod pins;
pub mod scanner;
pub mod stats;

use esp_idf_svc::hal::can::Frame as HalFrame;
use log::{info, warn};

use bitrate::CanBitrate;
use bus_profile::BusProfile;
use channel_probe_pure::ChannelResult;
use driver::CanDriver;
use errors::CanError;
use frame::CanFrame;
use pins::CanWiring;
use stats::CanStats;

/// Owns the live CAN bus and rolling statistics.
pub struct CanManager {
    driver: CanDriver<'static>,
    wiring: CanWiring,
    pub stats: CanStats,
}

impl CanManager {
    /// Scan bitrates and bring the bus up.
    pub fn start() -> Result<Self, CanError> {
        info!(
            "can: scanning bitrates {:?}",
            CanBitrate::SCAN_ORDER.map(|b| b.label())
        );
        let wiring = CanWiring::from_config();
        info!(
            "can: wiring {} ({}), tx=gpio{}, rx=gpio{}",
            wiring.label(),
            wiring.description(),
            wiring.tx_gpio,
            wiring.rx_gpio,
        );
        if !wiring.is_can() {
            // Deutsch F/G is J1708, not CAN. Its signalling still toggles a
            // transceiver's RXD line, so the pair looks alive while TWAI
            // decodes nothing — the "rx moves but rx_frames = 0" dead end from
            // the 2026-05-25 session. Name it at boot instead.
            warn!(
                "can: configured pins {} ({}) are not a CAN pair; RXD may still toggle but no frame will ever decode",
                wiring.label(),
                wiring.description(),
            );
        }
        info!(
            "can: connector is J1939 candidate={}",
            wiring.connector.j1939_candidate()
        );
        // What this build can actually reach. Without a mux this is one pair,
        // and "no traffic" is a statement about that pair only — not about the
        // truck. Saying so here keeps a wiring limitation from being read as a
        // vehicle capability.
        let selector = channel::ChannelSelector::from_config(wiring.connector);
        if !selector.can_probe() {
            info!(
                "can: fixed to pins {} — no channel mux fitted, other pairs cannot be probed",
                wiring.label(),
            );
        } else {
            info!(
                "can: channel mux on gpio{}; probe order {:?}",
                selector.select_gpio().unwrap_or_default(),
                selector
                    .candidates()
                    .iter()
                    .map(|p| p.label())
                    .collect::<heapless::Vec<_, 4>>(),
            );
        }

        let driver = scanner::scan(wiring)?;
        info!(
            "can: bus up at {} ({} bit/s)",
            driver.bitrate().label(),
            driver.bitrate().bits_per_sec(),
        );
        Ok(Self {
            driver,
            wiring,
            stats: CanStats::default(),
        })
    }

    pub fn bitrate(&self) -> CanBitrate {
        self.driver.bitrate()
    }

    pub fn listen_only(&self) -> bool {
        self.driver.listen_only()
    }

    /// Classify the physical connector pair from traffic observed by the live
    /// firmware. This is intentionally based on measurements, not the label
    /// printed on the vehicle connector.
    pub fn channel_result(&self) -> ChannelResult {
        let mut measured = ChannelResult::none(self.wiring.connector);
        if self.stats.rx > 0 {
            measured.bitrate_label = Some(self.bitrate().label());
            measured.extended_frames = self.stats.ext_frames;
            measured.standard_frames = self.stats.std_frames;
        }

        // This board has one reachable pair, but still pass measurements
        // through the same selector that multi-channel PCB hardware will use.
        // A silent fixed pair remains reportable even though it is not usable.
        channel_probe_pure::best(core::slice::from_ref(&measured))
            .copied()
            .unwrap_or(measured)
    }

    pub fn profile(
        &self,
        distinct_sources: usize,
        known_pgns_seen: usize,
        proprietary_count: usize,
    ) -> BusProfile {
        BusProfile::new(
            Some(self.bitrate()),
            self.listen_only(),
            self.wiring,
            distinct_sources,
            known_pgns_seen,
            proprietary_count,
        )
    }

    /// Poll one frame, waiting up to `timeout_ms`. Updates rx/error stats.
    pub fn poll(&mut self, timeout_ms: u32) -> Result<Option<CanFrame>, CanError> {
        match self.driver.receive(timeout_ms) {
            Ok(Some(f)) => {
                self.stats.on_rx(f.extended, f.remote);
                Ok(Some(f))
            }
            Ok(none) => Ok(none),
            Err(e) => {
                self.stats.on_error();
                Err(e)
            }
        }
    }

    /// Transmit a prepared request frame.
    pub fn transmit(&mut self, frame: &HalFrame) -> Result<(), CanError> {
        self.driver.transmit(frame)?;
        self.stats.on_request();
        Ok(())
    }
}

//! Diagnostic connector pinouts.
//!
//! Getting this wrong costs field days, so the mapping is spelled out rather
//! than assumed. Software can configure whichever transceiver is wired; it
//! cannot re-route connector pins. Reading two CAN channels at once needs two
//! transceivers or a switch in hardware.
//!
//! Field results, 2013 Volvo semi:
//!
//! - A Class 8 truck does **not** necessarily have a 9-pin Deutsch. 2013+
//!   Volvo/Mack chassis expose diagnostics on a 16-pin J1962 connector — the
//!   same shell as light-duty OBD-II.
//! - **Pins 6/14 worked on that truck. Pins 3/11 did not.** Vendor
//!   documentation predicts the opposite (J1939 on CAN2 3/11, ISO 15765 on CAN1
//!   6/14), so the documentation is a hypothesis and the bus is the authority.
//! - On a 9-pin Deutsch, J1939 is conventionally on **C/D**, and **F/G is
//!   J1708** — a different physical layer, not CAN. J1708 signalling still
//!   toggles a CAN transceiver's RXD line while TWAI decodes zero valid frames,
//!   which matches the "RXD moves but rx=0" symptom from the 2026-05-25
//!   session. A bench test with F/G on both ends proves the harness, not the
//!   truck-side convention.
//!
//! The lesson encoded here: **never assume a pin pair from the connector type.**
//! Treat every CAN-capable pair as a candidate and let observation decide.

use crate::config;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObdPinSet {
    /// 16-pin J1962, CAN channel 2: CAN-H pin 3 / CAN-L pin 11.
    /// Documented as the J1939 channel on 2013+ Volvo/Mack — but it produced
    /// nothing on the truck tested. Still worth probing; do not assume it.
    J1962Pins3And11,
    /// 16-pin J1962, CAN channel 1: CAN-H pin 6 / CAN-L pin 14.
    /// Documented as the ISO 15765 / OBD channel — but this is the pair that
    /// carried usable data on the 2013 Volvo, so it is not OBD-only in practice.
    J1962Pins6And14,
    /// 9-pin Deutsch: J1939 CAN-H pin C / CAN-L pin D.
    DeutschCAndD,
    /// 9-pin Deutsch pins F/G. **J1708, not CAN.** Present so a misconfiguration
    /// is named rather than silently looking like a dead bus.
    DeutschFAndG,
}

impl ObdPinSet {
    pub const ALL: [Self; 4] = [
        Self::J1962Pins3And11,
        Self::J1962Pins6And14,
        Self::DeutschCAndD,
        Self::DeutschFAndG,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ObdPinSet::J1962Pins3And11 => "3/11",
            ObdPinSet::J1962Pins6And14 => "6/14",
            ObdPinSet::DeutschCAndD => "C/D",
            ObdPinSet::DeutschFAndG => "F/G",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            ObdPinSet::J1962Pins3And11 => "J1962 16-pin CAN2: CAN-H 3 / CAN-L 11",
            ObdPinSet::J1962Pins6And14 => "J1962 16-pin CAN1: CAN-H 6 / CAN-L 14",
            ObdPinSet::DeutschCAndD => "Deutsch 9-pin: J1939 CAN-H C / CAN-L D",
            ObdPinSet::DeutschFAndG => "Deutsch 9-pin F/G: J1708, NOT CAN",
        }
    }

    /// Whether this pair is a CAN differential pair at all.
    ///
    /// Only Deutsch F/G is definitively not: it is J1708. Every other pair here
    /// is CAN, and which protocol rides on it varies by manufacturer.
    pub fn is_can(self) -> bool {
        !matches!(self, Self::DeutschFAndG)
    }

    /// Whether J1939 might be found here, subject to confirmation.
    ///
    /// Deliberately permissive. The 2013 Volvo yielded J1939 on 6/14, the pair
    /// vendor documentation calls the ISO 15765 channel — so ruling a CAN pair
    /// out on paper is how you lose a field day. Only a frame decoded from the
    /// bus settles it.
    pub fn j1939_candidate(self) -> bool {
        self.is_can()
    }

    /// Candidate pairs to probe, in the order worth trying.
    ///
    /// 6/14 leads because it is the pair that actually worked on the truck this
    /// was tested against.
    pub const PROBE_ORDER: [Self; 3] = [
        Self::J1962Pins6And14,
        Self::J1962Pins3And11,
        Self::DeutschCAndD,
    ];

    pub fn from_label(label: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|pin_set| pin_set.label() == label)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CanWiring {
    pub connector: ObdPinSet,
    pub tx_gpio: u8,
    pub rx_gpio: u8,
}

impl CanWiring {
    pub const fn new(connector: ObdPinSet, tx_gpio: u8, rx_gpio: u8) -> Self {
        Self {
            connector,
            tx_gpio,
            rx_gpio,
        }
    }

    pub fn from_config() -> Self {
        let connector = match ObdPinSet::from_label(config::CAN_CONNECTOR_PINS) {
            Some(pin_set) => pin_set,
            None => ObdPinSet::J1962Pins6And14,
        };
        Self::new(connector, config::CAN_TX_PIN, config::CAN_RX_PIN)
    }

    pub fn label(self) -> &'static str {
        self.connector.label()
    }

    pub fn description(self) -> &'static str {
        self.connector.description()
    }

    /// Whether the configured pair is CAN at all.
    pub fn is_can(self) -> bool {
        self.connector.is_can()
    }
}

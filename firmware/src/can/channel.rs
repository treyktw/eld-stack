//! Selecting which connector pin pair the CAN transceiver is looking at.
//!
//! ## The hardware constraint
//!
//! The ESP32 has **one TWAI controller**, wired to **one transceiver**, whose
//! CANH/CANL land on **one physical pin pair**. No amount of firmware can point
//! it at a different pair — probing several requires hardware:
//!
//! - an analog mux or relay switching the transceiver between pairs, driven by
//!   a GPIO (cheap, one pair at a time — enough for capability detection), or
//! - a second CAN controller, e.g. an MCP2515 over SPI, to watch both at once.
//!
//! Without one of those, `ChannelSelector::Fixed` is the truth: the device can
//! only report on the pair it happens to be soldered to, and a "not found"
//! result says nothing about the other pair.
//!
//! ## Why probe at all
//!
//! Because the connector does not tell you. On the 2013 Volvo, vendor
//! documentation put J1939 on J1962 pins 3/11 and the OBD channel on 6/14 — and
//! 6/14 was the pair that produced frames while 3/11 produced none. Any
//! firmware that hardcodes a pair from the connector type will be wrong on some
//! manufacturer, and the failure looks identical to a dead bus.

use super::pins::ObdPinSet;

/// How the device can reach connector pin pairs.
pub enum ChannelSelector {
    /// One transceiver hardwired to one pair. Probing is not possible.
    Fixed(ObdPinSet),
    /// A mux/relay can point the transceiver at any of the listed pairs.
    Switched {
        /// GPIO driving the mux select line.
        select_gpio: u8,
        available: &'static [ObdPinSet],
    },
}

impl ChannelSelector {
    /// Build from build-time config: switched when a mux GPIO is configured,
    /// fixed to the wired pair otherwise.
    pub fn from_config(wired: ObdPinSet) -> Self {
        match crate::config::CAN_CHANNEL_SELECT_GPIO {
            Some(select_gpio) => ChannelSelector::Switched {
                select_gpio,
                available: &ObdPinSet::PROBE_ORDER,
            },
            None => ChannelSelector::Fixed(wired),
        }
    }

    /// Pairs this device can actually attempt, in probe order.
    pub fn candidates(&self) -> &[ObdPinSet] {
        match self {
            ChannelSelector::Fixed(pair) => core::slice::from_ref(pair),
            ChannelSelector::Switched { available, .. } => available,
        }
    }

    pub fn can_probe(&self) -> bool {
        matches!(self, ChannelSelector::Switched { .. })
    }

    /// GPIO controlling the optional connector-channel mux.
    pub fn select_gpio(&self) -> Option<u8> {
        match self {
            ChannelSelector::Fixed(_) => None,
            ChannelSelector::Switched { select_gpio, .. } => Some(*select_gpio),
        }
    }
}

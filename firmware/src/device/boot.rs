//! Boot / reset reason.

use esp_idf_svc::sys::{self, esp_reset_reason};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResetReason {
    PowerOn,
    External,
    Software,
    Panic,
    Watchdog,
    Brownout,
    DeepSleep,
    Other,
}

impl ResetReason {
    pub fn read() -> Self {
        // SAFETY: simple FFI read, always valid after IDF init.
        let raw = unsafe { esp_reset_reason() };
        match raw {
            sys::esp_reset_reason_t_ESP_RST_POWERON => ResetReason::PowerOn,
            sys::esp_reset_reason_t_ESP_RST_EXT => ResetReason::External,
            sys::esp_reset_reason_t_ESP_RST_SW => ResetReason::Software,
            sys::esp_reset_reason_t_ESP_RST_PANIC => ResetReason::Panic,
            sys::esp_reset_reason_t_ESP_RST_INT_WDT
            | sys::esp_reset_reason_t_ESP_RST_TASK_WDT
            | sys::esp_reset_reason_t_ESP_RST_WDT => ResetReason::Watchdog,
            sys::esp_reset_reason_t_ESP_RST_BROWNOUT => ResetReason::Brownout,
            sys::esp_reset_reason_t_ESP_RST_DEEPSLEEP => ResetReason::DeepSleep,
            _ => ResetReason::Other,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ResetReason::PowerOn => "power-on",
            ResetReason::External => "external",
            ResetReason::Software => "software",
            ResetReason::Panic => "panic",
            ResetReason::Watchdog => "watchdog",
            ResetReason::Brownout => "brownout",
            ResetReason::DeepSleep => "deep-sleep",
            ResetReason::Other => "other",
        }
    }

    /// True if the last reset suggests a fault (vs clean power/boot).
    pub fn is_unexpected(self) -> bool {
        match self {
            ResetReason::Panic | ResetReason::Watchdog | ResetReason::Brownout => true,
            ResetReason::PowerOn
            | ResetReason::External
            | ResetReason::Software
            | ResetReason::DeepSleep
            | ResetReason::Other => false,
        }
    }
}

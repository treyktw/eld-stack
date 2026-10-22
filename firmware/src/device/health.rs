//! Runtime health: heap + uptime.

use esp_idf_svc::sys::{esp_get_free_heap_size, esp_get_minimum_free_heap_size};

use crate::time;

#[derive(Debug, Clone, Copy)]
pub struct Health {
    pub free_heap: u32,
    /// Lowest free heap ever seen since boot (watermark).
    pub min_free_heap: u32,
    pub uptime_s: u64,
}

impl Health {
    pub fn read(boot_ms: u64) -> Self {
        // SAFETY: FFI reads, valid after IDF init.
        let free_heap = unsafe { esp_get_free_heap_size() };
        let min_free_heap = unsafe { esp_get_minimum_free_heap_size() };
        Self {
            free_heap,
            min_free_heap,
            uptime_s: time::elapsed_ms(boot_ms) / 1000,
        }
    }
}

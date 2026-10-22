//! Per-device monotonic sequence numbers for upload dedup.
//!
//! The server dedups on `(device_id, stream, seq)`, so `seq` must be unique per
//! device across reboots. We pack a persisted 23-bit boot epoch in the high
//! bits and an in-RAM counter in the low 30 bits. The 53-bit result stays exact
//! in JavaScript as well as fitting Postgres/Rust's signed `i64` range.
#[cfg(target_os = "espidf")]
use esp_idf_svc::nvs::{EspDefaultNvsPartition, EspNvs};
#[cfg(target_os = "espidf")]
use esp_idf_svc::sys::esp_random;

const COUNTER_BITS: u32 = 30;
const COUNTER_MASK: u64 = (1 << COUNTER_BITS) - 1;
const EPOCH_MASK: u64 = 0x7F_FFFF; // 23 bits; epoch + counter = 53 bits
#[cfg(target_os = "espidf")]
const NVS_NAMESPACE: &str = "eld_seq";
#[cfg(target_os = "espidf")]
const NVS_BOOT_EPOCH_KEY: &str = "boot_epoch";

pub struct SeqGen {
    epoch: u64,
    counter: u64,
    latest: Option<u64>,
}

impl SeqGen {
    pub fn new() -> Self {
        #[cfg(target_os = "espidf")]
        // SAFETY: hardware RNG read, valid after IDF init.
        let epoch = (unsafe { esp_random() } as u64) & EPOCH_MASK;
        #[cfg(not(target_os = "espidf"))]
        let epoch = 0;
        Self::from_epoch(epoch)
    }

    /// Construct from a known boot epoch. Kept pure for boundary tests.
    pub const fn from_epoch(epoch: u64) -> Self {
        Self {
            epoch: epoch & EPOCH_MASK,
            counter: 0,
            latest: None,
        }
    }

    /// Advance and persist the boot epoch before any event is issued.
    #[cfg(target_os = "espidf")]
    pub fn persistent(
        partition: EspDefaultNvsPartition,
    ) -> Result<Self, esp_idf_svc::sys::EspError> {
        let nvs = EspNvs::new(partition, NVS_NAMESPACE, true)?;
        let seed = (unsafe { esp_random() } as u64) & EPOCH_MASK;
        let previous = nvs.get_u64(NVS_BOOT_EPOCH_KEY)?.unwrap_or(seed);
        let epoch = previous.wrapping_add(1) & EPOCH_MASK;
        nvs.set_u64(NVS_BOOT_EPOCH_KEY, epoch)?;
        Ok(Self::from_epoch(epoch))
    }

    /// Next sequence number. Unique per device for the life of this firmware.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> u64 {
        let seq = (self.epoch << COUNTER_BITS) | (self.counter & COUNTER_MASK);
        self.counter = self.counter.wrapping_add(1);
        self.latest = Some(seq);
        seq
    }

    pub fn latest(&self) -> Option<u64> {
        self.latest
    }
}

impl Default for SeqGen {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const JS_MAX_SAFE_INTEGER: u64 = (1_u64 << 53) - 1;

    #[test]
    fn maximum_sequence_is_exact_in_javascript() {
        let mut seq = SeqGen::from_epoch(EPOCH_MASK);
        seq.counter = COUNTER_MASK;

        assert_eq!(seq.next(), JS_MAX_SAFE_INTEGER);
    }

    #[test]
    fn sequence_increments_within_a_boot_epoch() {
        let mut seq = SeqGen::from_epoch(42);

        assert_eq!(seq.next() + 1, seq.next());
    }
}

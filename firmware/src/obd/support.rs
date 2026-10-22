//! Runtime discovery of which PIDs an ECU actually answers.
//!
//! This is the "universal" half of the puller. Two trucks from the same
//! manufacturer, a year apart, answer different PID sets — and a captured OBD-II
//! session from the field shows only five PIDs coming back from a 2013 Volvo. Hard-coding a poll list means either asking for things that never
//! answer (wasting bus time on every cycle, forever) or missing data a different
//! truck would have given.
//!
//! So the device asks. Mode 01 PID 0x00 returns a bitmap of which PIDs in
//! 0x01–0x20 are supported; if 0x20 is itself supported, PID 0x20 returns the
//! bitmap for 0x21–0x40, and so on to 0xC0. Walking that chain yields the exact
//! capability set of whatever is plugged in.
//!
//! Pure — no CAN — so the bit arithmetic is testable, which matters because an
//! off-by-one here silently drops a PID the truck does support.

/// PIDs that return a support bitmap for the range that follows them.
pub const SUPPORT_RANGE_PIDS: [u8; 7] = [0x00, 0x20, 0x40, 0x60, 0x80, 0xA0, 0xC0];

/// Highest PID expressible in Mode 01.
pub const MAX_PID: u8 = 0xE0;

/// A set of supported PIDs, one bit each.
///
/// 256 bits in 32 bytes. Fixed size rather than a heap collection: this lives on
/// a device with no allocator in the hot path, and the whole point is to be
/// cheap enough to consult before every poll.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SupportedPids {
    bits: [u8; 32],
}

impl Default for SupportedPids {
    fn default() -> Self {
        Self::new()
    }
}

impl SupportedPids {
    pub const fn new() -> Self {
        Self { bits: [0u8; 32] }
    }

    pub fn contains(&self, pid: u8) -> bool {
        self.bits[(pid >> 3) as usize] & (0x80 >> (pid & 7)) != 0
    }

    pub fn insert(&mut self, pid: u8) {
        self.bits[(pid >> 3) as usize] |= 0x80 >> (pid & 7);
    }

    pub fn count(&self) -> u32 {
        self.bits.iter().map(|b| b.count_ones()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.count() == 0
    }

    /// Every supported PID, ascending.
    pub fn iter(&self) -> impl Iterator<Item = u8> + '_ {
        (0u16..=MAX_PID as u16).filter_map(move |pid| {
            let pid = pid as u8;
            self.contains(pid).then_some(pid)
        })
    }

    /// Merge a support bitmap response.
    ///
    /// `base` is the PID that was asked (0x00, 0x20, …) and `data` is its four
    /// payload bytes. The MSB of the first byte is `base + 1`, running to
    /// `base + 32` at the LSB of the last.
    ///
    /// Returns whether the range *after* this one is advertised, so a caller can
    /// stop walking rather than asking for ranges the ECU has already said it
    /// does not have.
    pub fn absorb_range(&mut self, base: u8, data: [u8; 4]) -> bool {
        for (byte_index, byte) in data.iter().enumerate() {
            for bit in 0..8u8 {
                if byte & (0x80 >> bit) == 0 {
                    continue;
                }
                let offset = (byte_index as u16) * 8 + bit as u16 + 1;
                let pid = base as u16 + offset;
                // A bitmap can only describe its own 32-PID window; anything
                // beyond that is a malformed response rather than a discovery.
                if pid <= 0xFF {
                    self.insert(pid as u8);
                }
            }
        }

        // The last bit of the window is the "next range supported" flag, and it
        // is also a real PID number — which is why it is inserted above as well
        // as read here.
        let next_range = base.wrapping_add(0x20);
        next_range != 0 && self.contains(next_range)
    }
}

/// The next support-range PID to query, given what is known so far.
///
/// Returns `None` when the walk is finished: either the ECU stopped advertising
/// further ranges, or every range has been asked.
pub fn next_range_to_query(known: &SupportedPids, asked: &[u8]) -> Option<u8> {
    for &range in SUPPORT_RANGE_PIDS.iter() {
        if asked.contains(&range) {
            continue;
        }
        // 0x00 is always askable; later ranges only if advertised.
        if range == 0x00 || known.contains(range) {
            return Some(range);
        }
    }
    None
}

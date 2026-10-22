//! Parse a 29-bit J1939 CAN identifier.
//!
//! Layout (MSB→LSB of the 29-bit ID):
//!   priority [28:26] | EDP [25] | DP [24] | PF [23:16] | PS [15:8] | SA [7:0]
//!
//! PGN derivation depends on PDU format (PF):
//!   PF <  0xF0  → PDU1 (destination-specific): PGN = DP/EDP | PF<<8; PS = dest
//!   PF >= 0xF0  → PDU2 (broadcast):            PGN = DP/EDP | PF<<8 | PS

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct J1939Id {
    pub priority: u8,
    pub pgn: u32,
    pub source: u8,
    /// Destination address for PDU1 frames; `None` for broadcast (PDU2).
    pub destination: Option<u8>,
}

impl J1939Id {
    /// Parse an extended (29-bit) ID. Returns `None` if `id` exceeds 29 bits.
    pub fn parse(id: u32) -> Option<Self> {
        if id > 0x1FFF_FFFF {
            return None;
        }

        let priority = ((id >> 26) & 0x7) as u8;
        let edp = (id >> 25) & 0x1;
        let dp = (id >> 24) & 0x1;
        let pf = ((id >> 16) & 0xFF) as u8;
        let ps = ((id >> 8) & 0xFF) as u8;
        let sa = (id & 0xFF) as u8;

        // EDP+DP form the two high bits of the PGN.
        let page = (edp << 1) | dp;

        let (pgn, destination) = if pf < 0xF0 {
            // PDU1: PS carries the destination address, not part of the PGN.
            ((page << 16) | ((pf as u32) << 8), Some(ps))
        } else {
            // PDU2: PS is the group extension, part of the PGN.
            ((page << 16) | ((pf as u32) << 8) | (ps as u32), None)
        };

        Some(Self {
            priority,
            pgn,
            source: sa,
            destination,
        })
    }
}

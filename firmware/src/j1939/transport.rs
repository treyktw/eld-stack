//! J1939 Transport Protocol reassembly (BAM only).
//!
//! Messages longer than 8 bytes (VIN, multi-DTC DM1) are split across a TP.CM
//! announcement plus a run of TP.DT data packets. This reassembler handles the
//! **broadcast** flavour (BAM, control byte `0x20`) which covers the signals we
//! care about. The directed RTS/CTS handshake (CMDT) is intentionally not
//! implemented — it would require us to transmit CTS frames mid-decode. ECUs
//! that only answer a VIN request over CMDT won't be reassembled this pass.
//!
//! Feed every parsed J1939 frame to [`TransportReassembler::ingest`]; it returns
//! a [`Reassembled`] message on the TP.DT packet that completes a session.

use super::id::J1939Id;
use super::pgn;

/// Largest reassembled payload we keep. VIN is well under 200 bytes; a DM1 with
/// many active DTCs is rare. Anything larger is dropped rather than grown.
const MAX_MSG: usize = 512;
/// Concurrent BAM sessions (one per source address mid-transfer).
const MAX_SESSIONS: usize = 4;
/// Bytes of payload carried per TP.DT packet (the first byte is the sequence).
const DT_PAYLOAD: usize = 7;

/// TP.CM control byte for a Broadcast Announce Message.
const CONTROL_BAM: u8 = 0x20;

/// A fully reassembled multi-packet message.
pub struct Reassembled {
    pub pgn: u32,
    pub source: u8,
    pub data: heapless::Vec<u8, MAX_MSG>,
}

#[derive(Clone, Copy)]
struct Session {
    source: u8,
    pgn: u32,
    size: usize,
    packets: u8,
    received: u8,
    buf: [u8; MAX_MSG],
    active: bool,
}

impl Session {
    const EMPTY: Session = Session {
        source: 0,
        pgn: 0,
        size: 0,
        packets: 0,
        received: 0,
        buf: [0u8; MAX_MSG],
        active: false,
    };
}

pub struct TransportReassembler {
    sessions: [Session; MAX_SESSIONS],
}

impl TransportReassembler {
    pub fn new() -> Self {
        Self {
            sessions: [Session::EMPTY; MAX_SESSIONS],
        }
    }

    /// Feed one parsed J1939 frame. Returns the reassembled message when a
    /// TP.DT packet completes a BAM session; otherwise `None`.
    pub fn ingest(&mut self, id: &J1939Id, data: &[u8]) -> Option<Reassembled> {
        match id.pgn {
            pgn::TP_CM => {
                self.on_cm(id.source, data);
                None
            }
            pgn::TP_DT => self.on_dt(id.source, data),
            _ => None,
        }
    }

    fn on_cm(&mut self, source: u8, data: &[u8]) {
        if data.len() < 8 || data[0] != CONTROL_BAM {
            return;
        }
        let size = u16::from_le_bytes([data[1], data[2]]) as usize;
        let packets = data[3];
        let pgn = (data[5] as u32) | ((data[6] as u32) << 8) | ((data[7] as u32) << 16);

        // Ignore announcements we can't buffer.
        if size == 0 || size > MAX_MSG || packets == 0 {
            self.clear_source(source);
            return;
        }

        let slot = self.slot_for(source);
        let s = &mut self.sessions[slot];
        s.source = source;
        s.pgn = pgn;
        s.size = size;
        s.packets = packets;
        s.received = 0;
        s.active = true;
        // Leave buf as-is; only the first `size` bytes are read on completion.
    }

    fn on_dt(&mut self, source: u8, data: &[u8]) -> Option<Reassembled> {
        if data.is_empty() {
            return None;
        }
        let seq = data[0];
        let idx = self.find_active(source)?;
        let s = &mut self.sessions[idx];

        if seq == 0 || seq > s.packets {
            return None;
        }
        let offset = (seq as usize - 1) * DT_PAYLOAD;
        let payload = &data[1..];
        for (i, &b) in payload.iter().enumerate() {
            let pos = offset + i;
            if pos < s.size {
                s.buf[pos] = b;
            }
        }
        s.received = s.received.saturating_add(1);

        if s.received < s.packets {
            return None;
        }

        // Complete: snapshot, then free the slot.
        let mut out = heapless::Vec::<u8, MAX_MSG>::new();
        let _ = out.extend_from_slice(&s.buf[..s.size]);
        let result = Reassembled {
            pgn: s.pgn,
            source: s.source,
            data: out,
        };
        s.active = false;
        Some(result)
    }

    fn find_active(&self, source: u8) -> Option<usize> {
        self.sessions
            .iter()
            .position(|s| s.active && s.source == source)
    }

    fn clear_source(&mut self, source: u8) {
        if let Some(i) = self.find_active(source) {
            self.sessions[i].active = false;
        }
    }

    /// Pick a slot for a new session: reuse this source's slot, else a free one,
    /// else slot 0 (oldest wins — a stalled session shouldn't wedge the table).
    fn slot_for(&self, source: u8) -> usize {
        if let Some(i) = self.find_active(source) {
            return i;
        }
        self.sessions.iter().position(|s| !s.active).unwrap_or(0)
    }
}

impl Default for TransportReassembler {
    fn default() -> Self {
        Self::new()
    }
}

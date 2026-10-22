//! In-RAM outbound queue for dead-zone operation.
//!
//! Holds unsent messages FIFO. When full, the oldest is dropped so the most
//! recent telemetry always survives. Durable backlog lives on SD
//! (`storage::queue`); this is the hot path that drains when a link is up.

use heapless::Deque;

use super::protocol::OutboundMessage;

const CAPACITY: usize = 32;

pub struct OfflineQueue {
    items: Deque<OutboundMessage, CAPACITY>,
    dropped: u32,
}

impl OfflineQueue {
    pub fn new() -> Self {
        Self {
            items: Deque::new(),
            dropped: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn dropped(&self) -> u32 {
        self.dropped
    }

    /// Enqueue, evicting the oldest if full.
    pub fn push(&mut self, msg: OutboundMessage) {
        if self.items.is_full() {
            let _ = self.items.pop_front();
            self.dropped = self.dropped.wrapping_add(1);
        }
        // Safe: we just ensured space.
        let _ = self.items.push_back(msg);
    }

    pub fn front(&self) -> Option<&OutboundMessage> {
        self.items.front()
    }

    pub fn pop(&mut self) -> Option<OutboundMessage> {
        self.items.pop_front()
    }
}

impl Default for OfflineQueue {
    fn default() -> Self {
        Self::new()
    }
}

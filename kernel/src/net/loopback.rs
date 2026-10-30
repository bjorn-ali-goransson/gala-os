//! The loopback device: IP packets transmitted on it are received back.
//! Each packet is a buffer of MTU capacity, allocated (fallibly) when a
//! transmit token is handed out (no buffer: no token, smoltcp retries on
//! the next poll), then owned by the queue and the receive token.

use alloc::vec::Vec;
use smoltcp::phy::{self, DeviceCapabilities, Medium};
use smoltcp::time::Instant;

/// Largest packet; smoltcp derives the MSS on this interface from it.
pub const MTU: usize = 16 * 1024;
const SLOTS: usize = 32;

pub struct Loopback {
    queue: [Option<Vec<u8>>; SLOTS],
    head: usize,
    count: usize,
    pub packets: u64,
    pub dropped: u64,
}

impl Default for Loopback {
    fn default() -> Self {
        Self::new()
    }
}

impl Loopback {
    pub const fn new() -> Self {
        Self { queue: [const { None }; SLOTS], head: 0, count: 0, packets: 0, dropped: 0 }
    }
}

pub struct RxToken(Vec<u8>);

impl phy::RxToken for RxToken {
    fn consume<R, F: FnOnce(&[u8]) -> R>(self, f: F) -> R {
        f(&self.0)
    }
}

pub struct TxToken<'a>(&'a mut Loopback, Vec<u8>);

fn packet_buffer() -> Option<Vec<u8>> {
    let mut packet = Vec::new();
    packet.try_reserve_exact(MTU).ok()?;
    Some(packet)
}

impl phy::TxToken for TxToken<'_> {
    fn consume<R, F: FnOnce(&mut [u8]) -> R>(self, len: usize, f: F) -> R {
        let TxToken(lo, mut packet) = self;
        // Within the reserved capacity (smoltcp keeps to the MTU).
        packet.resize(len.min(MTU), 0);
        let result = f(&mut packet);
        let i = (lo.head + lo.count) % SLOTS;
        lo.queue[i] = Some(packet);
        lo.count += 1;
        lo.packets += 1;
        result
    }
}

impl phy::Device for Loopback {
    type RxToken<'a> = RxToken;
    type TxToken<'a> = TxToken<'a>;

    fn receive(&mut self, _now: Instant) -> Option<(RxToken, TxToken<'_>)> {
        if self.count == 0 {
            return None;
        }
        let Some(reply) = packet_buffer() else {
            self.dropped += 1;
            return None;
        };
        let packet = self.queue[self.head].take()?;
        self.head = (self.head + 1) % SLOTS;
        self.count -= 1;
        Some((RxToken(packet), TxToken(self, reply)))
    }

    fn transmit(&mut self, _now: Instant) -> Option<TxToken<'_>> {
        if self.count == SLOTS {
            return None;
        }
        let buffer = packet_buffer()?;
        Some(TxToken(self, buffer))
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.medium = Medium::Ip;
        caps.max_transmission_unit = MTU;
        caps
    }
}

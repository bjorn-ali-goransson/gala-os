//! The Ethernet card the stack sends on: a virtio-net card, or a USB
//! CDC-ECM/NCM adapter (`usb::cdc`, driven by `usb::xhci`), as one smoltcp
//! `Device`; and the frame counters both keep. Neither raises an
//! interrupt the stack waits on: the `net` thread polls every
//! millisecond while a card is up (`tcp::Stack::poll_delay_ms`).

use super::virtio_net::{self, VirtioNet};
use core::sync::atomic::{AtomicU64, Ordering};
use smoltcp::phy::{self, DeviceCapabilities, Medium};
use smoltcp::time::Instant;

pub static RX_FRAMES: AtomicU64 = AtomicU64::new(0);
pub static TX_FRAMES: AtomicU64 = AtomicU64::new(0);
/// Frames dropped because the card had no free transmit slot.
pub static TX_FULL: AtomicU64 = AtomicU64::new(0);
/// Ethernet bytes received and sent (the task manager's network graph).
pub static RX_BYTES: AtomicU64 = AtomicU64::new(0);
pub static TX_BYTES: AtomicU64 = AtomicU64::new(0);

/// The largest Ethernet frame without FCS.
pub const MTU: usize = 1514;

// One card lives in the stack for the whole run, so the size the virtio
// variant gives every Card costs nothing; boxing it would add a
// fallible allocation to set-up for no gain.
#[allow(clippy::large_enum_variant)]
pub enum Card {
    Virtio(VirtioNet),
    /// The USB adapter `usb` holds (the controller's lock is taken per
    /// frame, inside the stack's: STACK before USB, never the reverse).
    Usb(UsbNet),
}

impl Card {
    pub fn mac(&self) -> [u8; 6] {
        match self {
            Card::Virtio(v) => v.mac,
            Card::Usb(u) => u.mac,
        }
    }
}

/// The USB Ethernet adapter, as the stack sees it.
pub struct UsbNet {
    pub mac: [u8; 6],
}

pub enum RxToken {
    Virtio(virtio_net::RxToken),
    Usb(UsbRx),
}

pub enum TxToken<'a> {
    Virtio(virtio_net::TxToken<'a>),
    Usb,
}

pub struct UsbRx {
    frame: [u8; MTU],
    len: usize,
}

impl phy::RxToken for RxToken {
    fn consume<R, F: FnOnce(&[u8]) -> R>(self, f: F) -> R {
        match self {
            RxToken::Virtio(t) => t.consume(f),
            RxToken::Usb(t) => f(&t.frame[..t.len]),
        }
    }
}

impl phy::TxToken for TxToken<'_> {
    fn consume<R, F: FnOnce(&mut [u8]) -> R>(self, len: usize, f: F) -> R {
        match self {
            TxToken::Virtio(t) => t.consume(len, f),
            TxToken::Usb => {
                let mut frame = [0u8; MTU];
                let len = len.min(MTU);
                let result = f(&mut frame[..len]);
                // Fault injection: the frame is lost (TCP retransmits).
                if !crate::failpoint::hit(crate::failpoint::Point::NetTx)
                    && crate::usb::net_send(&frame[..len])
                {
                    TX_FRAMES.fetch_add(1, Ordering::Relaxed);
                    TX_BYTES.fetch_add(len as u64, Ordering::Relaxed);
                } else {
                    TX_FULL.fetch_add(1, Ordering::Relaxed);
                }
                result
            }
        }
    }
}

impl phy::Device for Card {
    type RxToken<'a> = RxToken;
    type TxToken<'a> = TxToken<'a>;

    fn receive(&mut self, now: Instant) -> Option<(RxToken, TxToken<'_>)> {
        match self {
            Card::Virtio(v) => {
                v.receive(now).map(|(rx, tx)| (RxToken::Virtio(rx), TxToken::Virtio(tx)))
            }
            Card::Usb(_) => {
                let mut t = UsbRx { frame: [0; MTU], len: 0 };
                t.len = crate::usb::net_receive(&mut t.frame)?;
                RX_FRAMES.fetch_add(1, Ordering::Relaxed);
                RX_BYTES.fetch_add(t.len as u64, Ordering::Relaxed);
                Some((RxToken::Usb(t), TxToken::Usb))
            }
        }
    }

    fn transmit(&mut self, now: Instant) -> Option<TxToken<'_>> {
        match self {
            Card::Virtio(v) => v.transmit(now).map(TxToken::Virtio),
            // Whether a slot is free is known once the frame is handed
            // over (`usb::net_send`); a full adapter drops it there.
            Card::Usb(_) => Some(TxToken::Usb),
        }
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.medium = Medium::Ethernet;
        caps.max_transmission_unit = MTU;
        caps
    }
}

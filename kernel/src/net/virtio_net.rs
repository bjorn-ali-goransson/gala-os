//! virtio-net over the legacy PCI interface (`arch::virtio`), as a smoltcp
//! `Device`. Queue 0 receives, queue 1 transmits; every buffer starts with
//! the 10-byte legacy `virtio_net_hdr` (no offloads negotiated, so it is
//! all zeros on transmit and ignored on receive).

use crate::arch::pci::{self, Function};
use crate::arch::virtio::{InitError, LegacyDevice};
use core::sync::atomic::{AtomicU64, Ordering};
use smoltcp::phy::{self, DeviceCapabilities, Medium};
use smoltcp::time::Instant;

const VENDOR_VIRTIO: u16 = 0x1af4;
/// Transitional virtio-net (the legacy interface is available).
const DEVICE_NET_TRANSITIONAL: u16 = 0x1000;
const SUBSYSTEM_NET: u16 = 1;

const F_MAC: u32 = 1 << 5;
const F_ANY_LAYOUT: u32 = 1 << 27;

const RX: u16 = 0;
const TX: u16 = 1;
const HEADER: usize = 10;
/// Header plus the largest Ethernet frame without FCS.
const SLOT: usize = 2048;
const MTU: usize = 1514;
const SLOTS: u16 = 32;

pub static RX_FRAMES: AtomicU64 = AtomicU64::new(0);
pub static TX_FRAMES: AtomicU64 = AtomicU64::new(0);
pub static TX_FULL: AtomicU64 = AtomicU64::new(0);
/// `netreset=<n>`: the device is reset under the driver after the n-th
/// frame sent (the driver's recovery test; 0: never).
pub static RESET_AT: AtomicU64 = AtomicU64::new(0);
/// Times the device was set up again.
pub static RESETS: AtomicU64 = AtomicU64::new(0);

pub struct VirtioNet {
    dev: LegacyDevice,
    pub mac: [u8; 6],
    pub at: pci::Address,
}

#[derive(Debug)]
pub enum Error {
    NotFound,
    Init(InitError),
    NoMac,
}

impl From<InitError> for Error {
    fn from(e: InitError) -> Self {
        Self::Init(e)
    }
}

impl VirtioNet {
    /// Finds the first transitional virtio-net function and starts it.
    pub fn probe() -> Result<Self, Error> {
        let mut found: Option<Function> = None;
        pci::for_each(|f| {
            let net = f.vendor == VENDOR_VIRTIO
                && f.device == DEVICE_NET_TRANSITIONAL
                && f.subsystem == SUBSYSTEM_NET;
            if net && found.is_none() {
                found = Some(*f);
            }
        });
        let function = found.ok_or(Error::NotFound)?;
        let (dev, features) = LegacyDevice::new(&function, F_MAC | F_ANY_LAYOUT)?;
        if features & F_MAC == 0 {
            return Err(Error::NoMac);
        }
        let mut mac = [0; 6];
        for (i, byte) in mac.iter_mut().enumerate() {
            *byte = dev.config_u8(i as u16);
        }
        let mut net = Self { dev, mac, at: function.address };
        net.setup()?;
        Ok(net)
    }

    /// The queues, the device started, every receive buffer posted.
    fn setup(&mut self) -> Result<(), InitError> {
        self.dev.setup_queue(RX, SLOTS, SLOT, true)?;
        self.dev.setup_queue(TX, SLOTS, SLOT, false)?;
        self.dev.start();
        if let Some(rx) = self.dev.queue(RX) {
            for slot in 0..rx.slots() {
                rx.post(slot, SLOT);
            }
        }
        self.dev.notify(RX);
        Ok(())
    }

    /// Sets the device up again if it gave up or was reset under the
    /// driver (ARCHITECTURE invariant 4): in place, the queues new; frames
    /// in flight are lost (TCP sends them again).
    fn recover(&mut self) {
        if !self.dev.needs_reset() {
            return;
        }
        crate::warn!("net: the device was reset or gave up; setting it up again");
        let features = self.dev.reinit(F_MAC | F_ANY_LAYOUT);
        if features & F_MAC == 0 || self.setup().is_err() {
            crate::warn!("net: the device could not be set up again");
            return;
        }
        RESETS.fetch_add(1, Ordering::Relaxed);
    }

    pub fn failed(&self) -> bool {
        self.dev.failed()
    }

    /// Takes back the transmit slots the device has sent.
    fn reclaim_tx(&mut self) {
        if let Some(tx) = self.dev.queue(TX) {
            while tx.pop_used().is_some() {}
        }
    }
}

pub struct RxToken {
    frame: [u8; MTU],
    len: usize,
}

impl phy::RxToken for RxToken {
    fn consume<R, F: FnOnce(&[u8]) -> R>(self, f: F) -> R {
        f(&self.frame[..self.len])
    }
}

pub struct TxToken<'a>(&'a mut VirtioNet);

impl phy::TxToken for TxToken<'_> {
    fn consume<R, F: FnOnce(&mut [u8]) -> R>(self, len: usize, f: F) -> R {
        let mut frame = [0u8; HEADER + MTU];
        let len = len.min(MTU);
        let result = f(&mut frame[HEADER..HEADER + len]);
        let net = self.0;
        // Fault injection: the frame is lost (TCP retransmits).
        if crate::failpoint::hit(crate::failpoint::Point::NetTx) {
            TX_FULL.fetch_add(1, Ordering::Relaxed);
            return result;
        }
        let sent = net.dev.queue(TX).is_some_and(|tx| {
            tx.free_slot().is_some_and(|slot| {
                tx.write_slot(slot, 0, &frame[..HEADER + len]) && tx.post(slot, HEADER + len)
            })
        });
        if sent {
            net.dev.notify(TX);
            let n = TX_FRAMES.fetch_add(1, Ordering::Relaxed) + 1;
            if RESET_AT.load(Ordering::Relaxed) == n {
                net.dev.reset_now();
            }
        } else {
            TX_FULL.fetch_add(1, Ordering::Relaxed);
        }
        result
    }
}

impl phy::Device for VirtioNet {
    type RxToken<'a> = RxToken;
    type TxToken<'a> = TxToken<'a>;

    fn receive(&mut self, _now: Instant) -> Option<(RxToken, TxToken<'_>)> {
        self.recover();
        self.reclaim_tx();
        let rx = self.dev.queue(RX)?;
        let (slot, written) = rx.pop_used()?;
        let mut token = RxToken { frame: [0; MTU], len: written.saturating_sub(HEADER).min(MTU) };
        rx.read_slot(slot, HEADER, &mut token.frame[..token.len]);
        rx.post(slot, SLOT);
        self.dev.notify(RX);
        RX_FRAMES.fetch_add(1, Ordering::Relaxed);
        Some((token, TxToken(self)))
    }

    fn transmit(&mut self, _now: Instant) -> Option<TxToken<'_>> {
        self.recover();
        self.reclaim_tx();
        self.dev.queue(TX)?.free_slot()?;
        Some(TxToken(self))
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.medium = Medium::Ethernet;
        caps.max_transmission_unit = MTU;
        caps
    }
}

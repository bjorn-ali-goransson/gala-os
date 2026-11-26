//! USB: an xHCI host controller driver (`xhci`), the HID boot keyboard
//! and report-descriptor mouse it reads (`hid`), and the CDC-ECM/NCM
//! Ethernet adapter it drives (`cdc`); the input thread polls the HID
//! devices (`poll`), the network stack sends and receives frames
//! (`net_send`, `net_receive`). ARCHITECTURE "USB".

macro_rules! log {
    ($level:ident, $($arg:tt)*) => {{
        #[cfg(not(test))]
        crate::$level!($($arg)*);
        #[cfg(test)]
        std::eprintln!($($arg)*);
    }};
}

pub mod cdc;
pub mod hid;
pub mod xhci;

#[cfg(not(test))]
pub use kernel::{init, net_mac, net_receive, net_send, poll, present, stats};

/// The host tests drive the controller model directly.
#[cfg(test)]
pub fn init() {}

#[cfg(test)]
pub fn present() -> bool {
    false
}

#[cfg(test)]
pub fn poll(_: impl FnMut(hid::Event)) -> (u64, u64) {
    (0, 0)
}

#[cfg(test)]
pub fn net_mac() -> Option<([u8; 6], cdc::Model)> {
    None
}

#[cfg(test)]
pub fn net_send(_: &[u8]) -> bool {
    false
}

#[cfg(test)]
pub fn net_receive(_: &mut [u8]) -> Option<usize> {
    None
}

#[cfg(not(test))]
mod kernel {
    use super::xhci::{Stats, Xhci};
    use super::{cdc, hid};
    use crate::arch::mmio::Mmio;
    use crate::arch::pci::{self, Function};
    use crate::disk::hw::KernelPlatform;
    use crate::sync::IrqMutex;
    use core::sync::atomic::{AtomicBool, Ordering};

    static USB: IrqMutex<Option<Xhci<KernelPlatform>>> = IrqMutex::new(None);
    static PRESENT: AtomicBool = AtomicBool::new(false);

    /// Finds the first xHCI controller and sets up the keyboards and mice
    /// on its ports.
    pub fn init() {
        let mut found: Option<Function> = None;
        pci::for_each(|f| {
            let prog_if = f.config_u32(0x08) >> 8 & 0xff;
            if found.is_none() && f.class == 0x0c && f.subclass == 0x03 && prog_if == 0x30 {
                found = Some(*f);
            }
        });
        let Some(f) = found else { return };
        if crate::time::raw_monotonic_ns() == 0 {
            warn!("usb: xhci at {}: no calibrated clock; not used", f.address);
            return;
        }
        f.enable_memory_and_dma();
        // The capability registers say how far the window reaches.
        let (caplen, ports, dboff, rtsoff) = match Mmio::bar(&f, 0, 0x20) {
            Ok(r) => (
                (r.read32(0) & 0xff) as usize,
                (r.read32(4) >> 24) as usize,
                (r.read32(0x14) & !3) as usize,
                (r.read32(0x18) & !0x1f) as usize,
            ),
            Err(e) => {
                warn!("usb: xhci at {}: its registers could not be mapped ({e:?})", f.address);
                return;
            }
        };
        let len = (dboff + 256 * 4).max(rtsoff + 0x40).max(caplen + 0x400 + 0x10 * ports);
        // A floor of two pages: the extended capabilities (the firmware
        // handoff) usually sit there, and their walk stops at the window's end.
        let len = len.max(0x2000).next_multiple_of(0x1000);
        let regs = match Mmio::bar(&f, 0, len) {
            Ok(r) => r,
            Err(e) => {
                warn!(
                    "usb: xhci at {}: its {len:#x} bytes of registers could not be mapped ({e:?})",
                    f.address
                );
                return;
            }
        };
        let phys = regs.phys();
        info!("usb: xhci at {}, registers at {phys:#x} ({len:#x} bytes)", f.address);
        match Xhci::start(KernelPlatform::new(regs), len) {
            Ok(dev) => {
                let (mut n, mut hid) = (0, 0);
                for (port, name) in dev.devices() {
                    info!("usb: port {port}: read as a {name}");
                    n += 1;
                    if matches!(name, "keyboard" | "mouse") {
                        hid += 1;
                    }
                }
                info!("usb: {n} device(s) in use");
                PRESENT.store(hid > 0, Ordering::Release);
                *USB.lock() = Some(dev);
            }
            Err(e) => warn!("usb: xhci at {} not usable: {e}", f.address),
        }
    }

    /// Whether a USB keyboard or mouse is in use (the input thread reads
    /// it).
    pub fn present() -> bool {
        PRESENT.load(Ordering::Acquire)
    }

    /// Reports read under the lock at a time: a boot keyboard's report
    /// is at most 20 events (six keys up, six down, eight modifiers).
    const BATCH_REPORTS: usize = 4;

    /// Reads what the devices reported since the last call; the reports
    /// read (keyboards', mice's). `f` runs with the controller's lock
    /// released (it goes on to the desktop, which may start a program):
    /// the events are taken a few reports at a time.
    pub fn poll(mut f: impl FnMut(hid::Event)) -> (u64, u64) {
        let mut total = (0, 0);
        // A bound, as `Xhci::poll`'s: a ring's worth of batches.
        for _ in 0..64 {
            let mut batch = [None; BATCH_REPORTS * 20];
            let mut len = 0;
            let n = match USB.lock().as_mut() {
                Some(dev) => dev.poll(BATCH_REPORTS, |e| {
                    if let Some(slot) = batch.get_mut(len) {
                        *slot = Some(e);
                    }
                    len += 1;
                }),
                None => return total,
            };
            if len > batch.len() {
                warn!(
                    "usb: {} events in {BATCH_REPORTS} reports: the last {} dropped",
                    len,
                    len - batch.len()
                );
            }
            batch.into_iter().flatten().for_each(&mut f);
            total = (total.0 + n.0, total.1 + n.1);
            if n.0 + n.1 == 0 {
                break;
            }
        }
        total
    }

    /// The first USB Ethernet adapter in use: its MAC address and model.
    pub fn net_mac() -> Option<([u8; 6], cdc::Model)> {
        USB.lock().as_ref().and_then(Xhci::net_mac)
    }

    /// Sends one Ethernet frame on the adapter; false if it was not
    /// taken (gone, or every transfer slot in flight).
    pub fn net_send(frame: &[u8]) -> bool {
        USB.lock().as_mut().is_some_and(|dev| dev.net_send(frame))
    }

    /// The next frame the adapter received, copied into `out`; its
    /// length.
    pub fn net_receive(out: &mut [u8]) -> Option<usize> {
        USB.lock().as_mut().and_then(|dev| dev.net_receive(out))
    }

    /// The counters and the events no one asked for.
    pub fn stats() -> Option<(Stats, u64)> {
        USB.lock().as_ref().map(|dev| (dev.stats, dev.strays()))
    }
}

//! The event stream's transport by DMA: a virtio console (`virtconsole`
//! on QEMU's `virtio-serial-pci`) over the modern PCI interface
//! (`arch::virtio::ModernDevice`). The debug console holds each line of
//! the stream until its newline (`arch::debugcon::Line`) and hands it
//! here once this device has started; until then, or on a machine
//! without one, it writes the line to its port. A port write leaves the
//! virtual machine for every byte under KVM, string I/O included, so an
//! event of 200 bytes cost 2-3 ms; here a line is copied into a buffer
//! the device reads and costs one notify (ARCHITECTURE "The event
//! stream's transport").
//!
//! Port 0 only: the multiport feature is not asked for, so the
//! transmit queue is queue 1 and no control queue is set up. The driver
//! polls, as every virtio driver here does: a buffer is taken back when
//! the device has used it, and a line that finds every buffer in flight
//! waits for one. A device that stops taking buffers for a second is
//! given up on, said once in the log, and the stream goes back to the
//! port: slower, nothing lost.
//!
//! Source (CLEAN_ROOM.md, THIRD_PARTY.md): OASIS *Virtual I/O Device
//! (VIRTIO) Version 1.x*, "Console Device": the device id (3), its
//! queues (receiveq 0 and transmitq 1 for port 0, without
//! VIRTIO_CONSOLE_F_MULTIPORT).

use crate::arch::pci::{self, Function};
use crate::arch::virtio::{InitError, ModernDevice};
use crate::sync::IrqMutex;
use core::sync::atomic::{AtomicU64, Ordering};

const VENDOR_VIRTIO: u16 = 0x1af4;
/// The transitional id (with subsystem 3) and the modern-only one
/// (0x1040 + device type 3).
const DEVICE_CONSOLE_TRANSITIONAL: u16 = 0x1003;
const DEVICE_CONSOLE_MODERN: u16 = 0x1043;
const SUBSYSTEM_CONSOLE: u16 = 3;

/// Port 0's transmit queue (the device reads it).
const TRANSMITQ: u16 = 1;
/// Lines in flight at once.
const SLOTS: u16 = 64;
/// A buffer holds what the debug console hands over at once: a line, or
/// a piece of a longer one (`arch::debugcon::LINE`).
const SLOT: usize = 1024;
/// How long a line waits for a buffer before the device is given up on.
const GIVE_UP_NS: u64 = 1_000_000_000;

struct Console {
    dev: ModernDevice,
    at: pci::Address,
}

static CONSOLE: IrqMutex<Option<Console>> = IrqMutex::new(None);

/// Lines the device took, and lines that went to the port instead after
/// it was given up on (for the run's summary).
static LINES: AtomicU64 = AtomicU64::new(0);
static GAVE_UP: AtomicU64 = AtomicU64::new(0);

/// Whether `f` is a virtio console.
fn is_console(f: &Function) -> bool {
    f.vendor == VENDOR_VIRTIO
        && (f.device == DEVICE_CONSOLE_MODERN
            || (f.device == DEVICE_CONSOLE_TRANSITIONAL && f.subsystem == SUBSYSTEM_CONSOLE))
}

/// Finds the first virtio console and starts it: from here on the event
/// stream's lines go through it. Says which device, or why none is used
/// (none at all is said by nothing: most machines have none).
pub fn init() {
    let mut found: Option<Function> = None;
    pci::for_each(|f| {
        if is_console(f) && found.is_none() {
            found = Some(*f);
        }
    });
    let Some(function) = found else { return };
    match start(&function) {
        Ok(dev) => {
            *CONSOLE.lock() = Some(Console { dev, at: function.address });
            crate::info!(
                "events: the stream goes through the virtio console at {} from here on, a \
                 notify a line (ARCHITECTURE \"The event stream's transport\")",
                function.address
            );
        }
        Err(e) => crate::warn!(
            "events: the virtio console at {} did not start ({e:?}); the stream stays on the \
             debug console's port",
            function.address
        ),
    }
}

fn start(function: &Function) -> Result<ModernDevice, InitError> {
    let mut dev = ModernDevice::new(function, 0)?;
    dev.setup_queue(TRANSMITQ, SLOTS, SLOT, false)?;
    dev.start();
    Ok(dev)
}

/// Hands `bytes` (a line, or a piece of one no longer than a buffer) to
/// the device. False when there is no device, it has failed, or it has
/// been given up on: the caller writes the bytes to its port instead.
pub fn write(bytes: &[u8]) -> bool {
    if bytes.len() > SLOT {
        return false;
    }
    let mut guard = CONSOLE.lock();
    let Some(console) = guard.as_mut() else { return false };
    if console.dev.failed() || console.dev.needs_reset() {
        give_up(guard, "the device failed or was reset");
        return false;
    }
    let Some(slot) = free_slot(&mut console.dev) else {
        give_up(guard, "it took no buffer back for a second");
        return false;
    };
    let Some(queue) = console.dev.queue(TRANSMITQ) else { return false };
    if !queue.write_slot(slot, 0, bytes) || !queue.post(slot, bytes.len()) {
        return false;
    }
    console.dev.notify(TRANSMITQ);
    LINES.fetch_add(1, Ordering::Relaxed);
    true
}

/// A buffer the device does not hold, taking back the ones it has used;
/// waits for one while every buffer is in flight, a second at most.
fn free_slot(dev: &mut ModernDevice) -> Option<u16> {
    let queue = dev.queue(TRANSMITQ)?;
    while queue.pop_used().is_some() {}
    if let Some(slot) = queue.free_slot() {
        return Some(slot);
    }
    let since = crate::time::raw_monotonic_ns();
    loop {
        if queue.pop_used().is_some() {
            return queue.free_slot();
        }
        if crate::time::raw_monotonic_ns().saturating_sub(since) > GIVE_UP_NS {
            return None;
        }
        core::hint::spin_loop();
    }
}

/// Waits until the device has taken every line handed to it (a second at
/// most), so that a run that ends next -- QEMU's exit device ends it at
/// once -- loses none: the host writes a line only when its notify has
/// been handled, which with KVM is QEMU's main loop and not the exit.
pub fn drain() {
    let mut guard = CONSOLE.lock();
    let Some(console) = guard.as_mut() else { return };
    let Some(queue) = console.dev.queue(TRANSMITQ) else { return };
    let since = crate::time::raw_monotonic_ns();
    loop {
        while queue.pop_used().is_some() {}
        if !queue.busy() {
            return;
        }
        if crate::time::raw_monotonic_ns().saturating_sub(since) > GIVE_UP_NS {
            give_up(guard, "it had not taken the last lines a second later");
            return;
        }
        core::hint::spin_loop();
    }
}

/// Stops using the device: the stream goes back to the port, and the log
/// says why and from where (the host's file then lacks the lines still
/// in flight, which xtask's count of the file's lines names).
/// The log line is written once the device's lock is released.
fn give_up(mut guard: crate::sync::IrqGuard<'_, Option<Console>>, why: &str) {
    let Some(console) = guard.take() else { return };
    drop(guard);
    GAVE_UP.fetch_add(1, Ordering::Relaxed);
    crate::warn!(
        "events: the virtio console at {} is given up on ({why}) after {} lines; the stream \
         goes back to the debug console's port",
        console.at,
        LINES.load(Ordering::Relaxed)
    );
}

/// Whether the device was given up on during the run.
pub fn gave_up() -> bool {
    GAVE_UP.load(Ordering::Relaxed) > 0
}

/// Lines the device took.
pub fn lines() -> u64 {
    LINES.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn function(device: u16, subsystem: u16) -> Function {
        Function {
            address: pci::Address { bus: 0, device: 0x1e, function: 0 },
            vendor: VENDOR_VIRTIO,
            device,
            subsystem,
            class: 0x07,
            subclass: 0x80,
        }
    }

    #[test]
    fn a_console_is_found_by_either_id() {
        assert!(is_console(&function(DEVICE_CONSOLE_MODERN, 0)));
        assert!(is_console(&function(DEVICE_CONSOLE_TRANSITIONAL, SUBSYSTEM_CONSOLE)));
        // 0x1003 with another subsystem is not a console.
        assert!(!is_console(&function(DEVICE_CONSOLE_TRANSITIONAL, 1)));
        // virtio-net's modern id.
        assert!(!is_console(&function(0x1041, 0)));
    }
}

//! The virtio PCI legacy interface (virtio 1.x specification, "Legacy
//! Interface: A Note on PCI Device Layout" and "Legacy Interfaces: A Note
//! on Virtqueue Layout"; OASIS, see THIRD_PARTY.md): registers in I/O BAR 0,
//! split virtqueues at a page-aligned guest-physical address.
//!
//! The device reads and writes guest memory (DMA) only through the buffers
//! posted here, which is why this lives in `arch`: the queues own their
//! memory, and the device is reset before that memory is freed. Drivers
//! (`net::virtio_net`) use the safe API: post a slot, notify, pop used.
//! No interrupts: drivers poll.

use super::pci::Function;
use super::port::{Port, PortValue};
use crate::mm::frame::Frame;
use core::sync::atomic::{Ordering, fence};

const DEVICE_FEATURES: u16 = 0x00;
const GUEST_FEATURES: u16 = 0x04;
const QUEUE_ADDRESS: u16 = 0x08;
const QUEUE_SIZE: u16 = 0x0c;
const QUEUE_SELECT: u16 = 0x0e;
const QUEUE_NOTIFY: u16 = 0x10;
const DEVICE_STATUS: u16 = 0x12;
/// Device-specific configuration (without MSI-X, which we never enable).
const DEVICE_CONFIG: u16 = 0x14;

const STATUS_ACKNOWLEDGE: u8 = 1;
const STATUS_DRIVER: u8 = 2;
const STATUS_DRIVER_OK: u8 = 4;
const STATUS_FAILED: u8 = 0x80;

const DESC_F_WRITE: u16 = 2;
const AVAIL_F_NO_INTERRUPT: u16 = 1;
const PAGE: usize = 4096;

/// Legacy virtqueues are page-aligned at a 32-bit page number.
const MAX_QUEUE_PHYS: u64 = 1 << 44;

/// Queues per device (enough for a network card: receive and transmit).
pub const MAX_QUEUES: usize = 2;

/// Physically contiguous, zeroed, HHDM-mapped memory the device may access.
struct DmaRegion {
    start: Frame,
    pages: u64,
}

impl DmaRegion {
    fn new(bytes: usize) -> Option<Self> {
        let pages = bytes.div_ceil(PAGE) as u64;
        let start = crate::mm::allocate_frames(pages, 1)?;
        let region = Self { start, pages };
        // SAFETY: freshly allocated frames, HHDM-mapped, owned by `region`.
        unsafe { region.ptr(0).write_bytes(0, region.len()) };
        Some(region)
    }

    fn phys(&self) -> u64 {
        self.start.start()
    }

    fn len(&self) -> usize {
        self.pages as usize * PAGE
    }

    fn ptr(&self, offset: usize) -> *mut u8 {
        assert!(offset <= self.len(), "DMA offset out of range");
        core::ptr::with_exposed_provenance_mut(
            (crate::mm::hhdm_offset() + self.phys()) as usize + offset,
        )
    }

    fn read<T: Copy>(&self, offset: usize) -> T {
        assert!(offset + size_of::<T>() <= self.len() && offset.is_multiple_of(align_of::<T>()));
        // SAFETY: in bounds and aligned (checked); the region is mapped and
        // owned; volatile because the device writes it concurrently.
        unsafe { self.ptr(offset).cast::<T>().read_volatile() }
    }

    fn write<T: Copy>(&mut self, offset: usize, value: T) {
        assert!(offset + size_of::<T>() <= self.len() && offset.is_multiple_of(align_of::<T>()));
        // SAFETY: as in `read`.
        unsafe { self.ptr(offset).cast::<T>().write_volatile(value) }
    }

    fn copy_out(&self, offset: usize, out: &mut [u8]) {
        assert!(offset + out.len() <= self.len());
        // SAFETY: in bounds (checked); the device does not write a slot
        // that is not posted (callers check), so the bytes are stable.
        unsafe { core::ptr::copy_nonoverlapping(self.ptr(offset), out.as_mut_ptr(), out.len()) };
    }

    fn copy_in(&mut self, offset: usize, data: &[u8]) {
        assert!(offset + data.len() <= self.len());
        // SAFETY: as in `copy_out`.
        unsafe { core::ptr::copy_nonoverlapping(data.as_ptr(), self.ptr(offset), data.len()) };
    }
}

impl Drop for DmaRegion {
    fn drop(&mut self) {
        for i in 0..self.pages {
            let _ = crate::mm::free_frame(Frame::from_number(self.start.number() + i));
        }
    }
}

/// A split virtqueue whose descriptors each point at a fixed buffer slot
/// (no chains): slot `i` is descriptor `i`.
pub struct Virtqueue {
    rings: DmaRegion,
    buffers: DmaRegion,
    size: u16,
    used_offset: usize,
    slot_size: usize,
    slots: u16,
    device_writes: bool,
    avail_idx: u16,
    last_used: u16,
    in_flight: u64,
}

/// The legacy layout of a queue of `size` entries: the offset of the used
/// ring (descriptors and the available ring come first, then padding to a
/// page) and the total size.
fn layout(size: u16) -> (usize, usize) {
    let n = usize::from(size);
    let used_offset = (16 * n + 6 + 2 * n).next_multiple_of(PAGE);
    (used_offset, used_offset + 6 + 8 * n)
}

impl Virtqueue {
    fn new(size: u16, slots: u16, slot_size: usize, device_writes: bool) -> Option<Self> {
        let n = usize::from(size);
        let (used_offset, total) = layout(size);
        let rings = DmaRegion::new(total)?;
        let slots = slots.min(size).min(64);
        let buffers = DmaRegion::new(usize::from(slots) * slot_size)?;
        if rings.phys() >= MAX_QUEUE_PHYS {
            return None;
        }
        let mut queue = Self {
            rings,
            buffers,
            size,
            used_offset,
            slot_size,
            slots,
            device_writes,
            avail_idx: 0,
            last_used: 0,
            in_flight: 0,
        };
        // We poll; ask the device not to interrupt.
        queue.rings.write(16 * n, AVAIL_F_NO_INTERRUPT);
        Some(queue)
    }

    pub fn slots(&self) -> u16 {
        self.slots
    }

    pub fn slot_size(&self) -> usize {
        self.slot_size
    }

    fn posted(&self, slot: u16) -> bool {
        self.in_flight & 1 << slot != 0
    }

    /// A slot the device does not own, if any.
    pub fn free_slot(&self) -> Option<u16> {
        (0..self.slots).find(|&s| !self.posted(s))
    }

    /// Copies `data` into a free slot at `offset`; false if the slot is
    /// posted or the data does not fit.
    pub fn write_slot(&mut self, slot: u16, offset: usize, data: &[u8]) -> bool {
        if slot >= self.slots || self.posted(slot) || offset + data.len() > self.slot_size {
            return false;
        }
        self.buffers.copy_in(usize::from(slot) * self.slot_size + offset, data);
        true
    }

    /// Copies a free slot's bytes at `offset` into `out`.
    pub fn read_slot(&self, slot: u16, offset: usize, out: &mut [u8]) -> bool {
        if slot >= self.slots || self.posted(slot) || offset + out.len() > self.slot_size {
            return false;
        }
        self.buffers.copy_out(usize::from(slot) * self.slot_size + offset, out);
        true
    }

    /// Gives slot `slot` (its first `len` bytes) to the device. The caller
    /// then notifies the device (`LegacyDevice::notify`).
    pub fn post(&mut self, slot: u16, len: usize) -> bool {
        if slot >= self.slots || self.posted(slot) || len > self.slot_size {
            return false;
        }
        let desc = 16 * usize::from(slot);
        let addr = self.buffers.phys() + (usize::from(slot) * self.slot_size) as u64;
        self.rings.write(desc, addr);
        self.rings.write(desc + 8, len as u32);
        self.rings.write(desc + 12, if self.device_writes { DESC_F_WRITE } else { 0 });
        self.rings.write(desc + 14, 0u16);
        let avail = 16 * usize::from(self.size);
        let ring_entry = avail + 4 + 2 * usize::from(self.avail_idx % self.size);
        self.rings.write(ring_entry, slot);
        // The descriptor and ring entry before the index the device reads.
        fence(Ordering::SeqCst);
        self.avail_idx = self.avail_idx.wrapping_add(1);
        self.rings.write(avail + 2, self.avail_idx);
        self.in_flight |= 1 << slot;
        true
    }

    /// The next slot the device has finished with, and how many bytes it
    /// wrote into it.
    pub fn pop_used(&mut self) -> Option<(u16, usize)> {
        let used_idx: u16 = self.rings.read(self.used_offset + 2);
        if used_idx == self.last_used {
            return None;
        }
        // Read the entry only after seeing the index that covers it.
        fence(Ordering::SeqCst);
        let entry = self.used_offset + 4 + 8 * usize::from(self.last_used % self.size);
        let id: u32 = self.rings.read(entry);
        let len: u32 = self.rings.read(entry + 4);
        self.last_used = self.last_used.wrapping_add(1);
        let slot = u16::try_from(id).ok().filter(|&s| s < self.slots && self.posted(s))?;
        self.in_flight &= !(1 << slot);
        Some((slot, (len as usize).min(self.slot_size)))
    }
}

/// A virtio device driven through the legacy I/O interface.
pub struct LegacyDevice {
    io: u16,
    // Dropped after `drop` has reset the device (fields drop last).
    queues: [Option<Virtqueue>; MAX_QUEUES],
}

#[derive(Debug, Clone, Copy)]
pub enum InitError {
    NoIoBar,
    NoQueue(u16),
    OutOfMemory,
}

impl LegacyDevice {
    /// Resets the device, negotiates `wanted & offered` features and
    /// returns the device (status ACKNOWLEDGE | DRIVER); set up the queues
    /// with `setup_queue`, then call `start`.
    pub fn new(function: &Function, wanted: u32) -> Result<(Self, u32), InitError> {
        let io = function.io_bar(0).ok_or(InitError::NoIoBar)?;
        function.enable_io_and_dma();
        let mut dev = Self { io, queues: [const { None }; MAX_QUEUES] };
        dev.write(DEVICE_STATUS, 0u8);
        dev.write(DEVICE_STATUS, STATUS_ACKNOWLEDGE);
        dev.write(DEVICE_STATUS, STATUS_ACKNOWLEDGE | STATUS_DRIVER);
        let features = dev.read::<u32>(DEVICE_FEATURES) & wanted;
        dev.write(GUEST_FEATURES, features);
        Ok((dev, features))
    }

    fn port<T: PortValue>(&self, offset: u16) -> Port<T> {
        // SAFETY: the registers of the device this value owns (I/O BAR 0).
        // Writes can make the device DMA, but only to addresses this module
        // gives it (queues it owns).
        unsafe { Port::new(self.io + offset) }
    }

    fn read<T: PortValue>(&self, offset: u16) -> T {
        self.port(offset).read()
    }

    fn write<T: PortValue>(&mut self, offset: u16, value: T) {
        self.port(offset).write(value);
    }

    /// Byte `offset` of the device-specific configuration.
    pub fn config_u8(&self, offset: u16) -> u8 {
        self.read(DEVICE_CONFIG + offset)
    }

    /// Sets up queue `index` with up to `slots` buffers of `slot_size` bytes
    /// (`device_writes`: receive buffers).
    pub fn setup_queue(
        &mut self,
        index: u16,
        slots: u16,
        slot_size: usize,
        device_writes: bool,
    ) -> Result<(), InitError> {
        let i = usize::from(index);
        if i >= MAX_QUEUES {
            return Err(InitError::NoQueue(index));
        }
        self.write(QUEUE_SELECT, index);
        let size: u16 = self.read(QUEUE_SIZE);
        if size == 0 || !size.is_power_of_two() {
            return Err(InitError::NoQueue(index));
        }
        let queue =
            Virtqueue::new(size, slots, slot_size, device_writes).ok_or(InitError::OutOfMemory)?;
        self.write(QUEUE_ADDRESS, (queue.rings.phys() / PAGE as u64) as u32);
        self.queues[i] = Some(queue);
        Ok(())
    }

    /// Tells the device the driver is ready.
    pub fn start(&mut self) {
        self.write(DEVICE_STATUS, STATUS_ACKNOWLEDGE | STATUS_DRIVER | STATUS_DRIVER_OK);
    }

    /// Whether the device gave up (DEVICE_NEEDS_RESET / FAILED bits).
    pub fn failed(&self) -> bool {
        self.read::<u8>(DEVICE_STATUS) & (STATUS_FAILED | 0x40) != 0
    }

    pub fn queue(&mut self, index: u16) -> Option<&mut Virtqueue> {
        self.queues.get_mut(usize::from(index))?.as_mut()
    }

    /// Tells the device that queue `index` has new buffers.
    pub fn notify(&mut self, index: u16) {
        self.write(QUEUE_NOTIFY, index);
    }
}

impl Drop for LegacyDevice {
    fn drop(&mut self) {
        // Reset: the device stops using the queues before they are freed.
        self.write(DEVICE_STATUS, 0u8);
    }
}

#[cfg(test)]
mod tests {
    use super::layout;

    #[test]
    fn legacy_queue_layout() {
        // 256 entries: 4096 bytes of descriptors, 518 of available ring,
        // the used ring on the next page.
        assert_eq!(layout(256), (8192, 8192 + 6 + 8 * 256));
        assert_eq!(layout(16), (4096, 4096 + 6 + 128));
    }
}

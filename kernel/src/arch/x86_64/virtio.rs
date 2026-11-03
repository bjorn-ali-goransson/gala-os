//! The virtio PCI legacy interface (virtio 1.x specification, "Legacy
//! Interface: A Note on PCI Device Layout" and "Legacy Interfaces: A Note
//! on Virtqueue Layout"; OASIS, see THIRD_PARTY.md): registers in I/O BAR 0,
//! split virtqueues at a page-aligned guest-physical address. And the
//! modern interface ("Virtio Over PCI Bus": the common, notify and device
//! configuration structures found through vendor capabilities, in memory
//! BARs) for devices that have no legacy one (virtio-gpu): `ModernDevice`.
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

const DESC_F_NEXT: u16 = 1;
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

/// A split virtqueue whose descriptors each point at a fixed buffer slot:
/// slot `i` is descriptor `i`, or, in a chained queue, descriptors `2i`
/// (the device reads) and `2i + 1` (the device writes) over consecutive
/// parts of slot `i`.
pub struct Virtqueue {
    rings: DmaRegion,
    buffers: DmaRegion,
    size: u16,
    used_offset: usize,
    slot_size: usize,
    slots: u16,
    device_writes: bool,
    chained: bool,
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
    fn new(
        size: u16,
        slots: u16,
        slot_size: usize,
        device_writes: bool,
        chained: bool,
    ) -> Option<Self> {
        let n = usize::from(size);
        let (used_offset, total) = layout(size);
        let rings = DmaRegion::new(total)?;
        let slots = slots.min(if chained { size / 2 } else { size }).min(64);
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
            chained,
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
        if self.chained || slot >= self.slots || self.posted(slot) || len > self.slot_size {
            return false;
        }
        let flags = if self.device_writes { DESC_F_WRITE } else { 0 };
        self.describe(slot, 0, len, flags, None);
        self.make_available(slot, slot)
    }

    /// Gives slot `slot` of a chained queue to the device: its first
    /// `readable` bytes for the device to read, the `writable` bytes after
    /// them for it to write.
    pub fn post_chain(&mut self, slot: u16, readable: usize, writable: usize) -> bool {
        if !self.chained
            || slot >= self.slots
            || self.posted(slot)
            || readable == 0
            || writable == 0
            || readable + writable > self.slot_size
        {
            return false;
        }
        let (head, tail) = (2 * slot, 2 * slot + 1);
        self.describe(head, 0, readable, DESC_F_NEXT, Some(tail));
        self.describe(tail, readable, writable, DESC_F_WRITE, None);
        self.make_available(slot, head)
    }

    /// Fills descriptor `desc` with `len` bytes at `offset` in the slot the
    /// descriptor belongs to.
    fn describe(&mut self, desc: u16, offset: usize, len: usize, flags: u16, next: Option<u16>) {
        let slot = if self.chained { desc / 2 } else { desc };
        let at = 16 * usize::from(desc);
        let addr = self.buffers.phys() + (usize::from(slot) * self.slot_size + offset) as u64;
        self.rings.write(at, addr);
        self.rings.write(at + 8, len as u32);
        self.rings.write(at + 12, flags);
        self.rings.write(at + 14, next.unwrap_or(0));
    }

    /// Puts descriptor (chain) `head` of slot `slot` on the available ring.
    fn make_available(&mut self, slot: u16, head: u16) -> bool {
        let avail = 16 * usize::from(self.size);
        let ring_entry = avail + 4 + 2 * usize::from(self.avail_idx % self.size);
        self.rings.write(ring_entry, head);
        // The descriptor and ring entry before the index the device reads.
        fence(Ordering::SeqCst);
        self.avail_idx = self.avail_idx.wrapping_add(1);
        self.rings.write(avail + 2, self.avail_idx);
        self.in_flight |= 1 << slot;
        true
    }

    /// Guest-physical addresses of the descriptor table, the available
    /// (driver) ring and the used (device) ring, and the queue's size.
    fn addresses(&self) -> (u64, u64, u64, u16) {
        let base = self.rings.phys();
        (base, base + 16 * u64::from(self.size), base + self.used_offset as u64, self.size)
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
        let head = u16::try_from(id).ok()?;
        let slot = if self.chained { head / 2 } else { head };
        let slot = Some(slot).filter(|&s| s < self.slots && self.posted(s))?;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InitError {
    NoIoBar,
    NoQueue(u16),
    OutOfMemory,
    /// No common or notify structure among the capabilities.
    NoModernLayout,
    /// A structure's BAR is not an assigned memory BAR.
    NoMemoryBar,
    /// The status did not read 0 after a reset.
    ResetTimeout,
    /// The device does not offer VIRTIO_F_VERSION_1.
    NoVersion1,
    /// The device cleared FEATURES_OK.
    FeaturesRefused,
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

    /// The little-endian 32-bit field at `offset` of the configuration.
    pub fn config_u32(&self, offset: u16) -> u32 {
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
        self.setup(index, slots, slot_size, device_writes, false)
    }

    /// Sets up queue `index` for requests the device both reads and
    /// writes (`Virtqueue::post_chain`), as a block device's are.
    pub fn setup_chained_queue(
        &mut self,
        index: u16,
        slots: u16,
        slot_size: usize,
    ) -> Result<(), InitError> {
        self.setup(index, slots, slot_size, false, true)
    }

    fn setup(
        &mut self,
        index: u16,
        slots: u16,
        slot_size: usize,
        device_writes: bool,
        chained: bool,
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
        let queue = Virtqueue::new(size, slots, slot_size, device_writes, chained)
            .ok_or(InitError::OutOfMemory)?;
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

    /// Whether the device must be set up again: it gave up, or it was
    /// reset under the driver (DRIVER_OK gone: its queues are forgotten).
    pub fn needs_reset(&self) -> bool {
        let status = self.read::<u8>(DEVICE_STATUS);
        status & (STATUS_FAILED | 0x40) != 0 || status & STATUS_DRIVER_OK == 0
    }

    /// Sets the device up again in place (after it gave up or was reset):
    /// reset, the old queues freed (the device no longer uses them),
    /// `wanted & offered` features negotiated again; set up the queues and
    /// `start` again. The negotiated features.
    pub fn reinit(&mut self, wanted: u32) -> u32 {
        self.write(DEVICE_STATUS, 0u8);
        self.queues = [const { None }; MAX_QUEUES];
        self.write(DEVICE_STATUS, STATUS_ACKNOWLEDGE);
        self.write(DEVICE_STATUS, STATUS_ACKNOWLEDGE | STATUS_DRIVER);
        let features = self.read::<u32>(DEVICE_FEATURES) & wanted;
        self.write(GUEST_FEATURES, features);
        features
    }

    /// Resets the device now (status 0), as a test of the driver's
    /// recovery does (`diskreset=`): what was in flight is lost.
    pub fn reset_now(&mut self) {
        self.write(DEVICE_STATUS, 0u8);
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

/// Resets every virtio device on the PCI bus (status 0), whichever driver
/// owns it, so that none reads or writes memory any more: before a kernel
/// handover, when this kernel's memory is about to be reused. The drivers
/// are not told (no CPU runs them any more). Returns how many were reset.
/// Modern devices are reset through the status register their driver
/// registered (`MODERN_STATUS`): no mapping is made here.
pub fn reset_all() -> u32 {
    let mut count = 0;
    for slot in &MODERN_STATUS {
        let status = slot.load(Ordering::Acquire);
        if status != 0 {
            // SAFETY: a registered address is the device_status byte of a
            // live ModernDevice's common configuration (mapped uncached for
            // as long as the kernel runs: MMIO mappings are never removed);
            // writing 0 resets the device.
            unsafe {
                core::ptr::with_exposed_provenance_mut::<u8>(status as usize).write_volatile(0)
            };
            count += 1;
        }
    }
    super::pci::for_each(|f| {
        if f.vendor != 0x1af4 {
            return;
        }
        if let Some(io) = f.io_bar(0) {
            // SAFETY: the device's legacy status register; writing 0 resets
            // the device, which then stops using its queues.
            unsafe { Port::<u8>::new(io + DEVICE_STATUS) }.write(0);
            count += 1;
        }
    });
    count
}

// ---- The modern interface ------------------------------------------------

/// `VIRTIO_F_VERSION_1`: a modern device refuses a driver without it.
pub const F_VERSION_1: u64 = 1 << 32;
const STATUS_FEATURES_OK: u8 = 8;
const STATUS_NEEDS_RESET: u8 = 0x40;

/// Vendor-specific PCI capability (`virtio_pci_cap`) and its types.
const CAP_VENDOR: u8 = 0x09;
const CAP_COMMON: u8 = 1;
const CAP_NOTIFY: u8 = 2;
const CAP_DEVICE: u8 = 4;

/// Offsets in the common configuration (`virtio_pci_common_cfg`).
const C_DEVICE_FEATURE_SELECT: usize = 0x00;
const C_DEVICE_FEATURE: usize = 0x04;
const C_DRIVER_FEATURE_SELECT: usize = 0x08;
const C_DRIVER_FEATURE: usize = 0x0c;
const C_NUM_QUEUES: usize = 0x12;
const C_DEVICE_STATUS: usize = 0x14;
const C_QUEUE_SELECT: usize = 0x16;
const C_QUEUE_SIZE: usize = 0x18;
const C_QUEUE_ENABLE: usize = 0x1c;
const C_QUEUE_NOTIFY_OFF: usize = 0x1e;
const C_QUEUE_DESC: usize = 0x20;
const C_QUEUE_DRIVER: usize = 0x28;
const C_QUEUE_DEVICE: usize = 0x30;
const COMMON_LEN: u64 = 0x38;

/// Where one structure is: BAR, offset in it, length.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CapRef {
    pub bar: u8,
    pub offset: u32,
    pub length: u32,
}

/// The structures a modern driver needs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ModernLayout {
    pub common: CapRef,
    pub notify: CapRef,
    pub notify_multiplier: u32,
    pub device: Option<CapRef>,
}

/// Finds the modern structures among the capabilities (`caps`: offset and
/// id of each, `read` a configuration dword). The first of each type
/// counts, as the specification asks.
fn find_layout(caps: &[(u8, u8)], read: impl Fn(u8) -> u32) -> Option<ModernLayout> {
    let (mut common, mut notify, mut device) = (None, None, None);
    let mut multiplier = 0;
    for &(at, id) in caps {
        if id != CAP_VENDOR {
            continue;
        }
        let head = read(at);
        let (cap_len, kind) = ((head >> 16) as u8, (head >> 24) as u8);
        if cap_len < 16 {
            continue;
        }
        let bar = read(at + 4) as u8;
        let r = CapRef { bar, offset: read(at + 8), length: read(at + 12) };
        if bar > 5 {
            continue;
        }
        match kind {
            CAP_COMMON if common.is_none() => common = Some(r),
            CAP_NOTIFY if notify.is_none() && cap_len >= 20 => {
                notify = Some(r);
                multiplier = read(at + 16);
            }
            CAP_DEVICE if device.is_none() => device = Some(r),
            _ => {}
        }
    }
    Some(ModernLayout {
        common: common.filter(|c| u64::from(c.length) >= COMMON_LEN)?,
        notify: notify?,
        notify_multiplier: multiplier,
        device,
    })
}

/// Modern devices' device_status addresses, for `reset_all` (0: free).
static MODERN_STATUS: [core::sync::atomic::AtomicU64; 8] =
    [const { core::sync::atomic::AtomicU64::new(0) }; 8];

/// A virtio device driven through the modern PCI interface. The queues
/// are the same split virtqueues as the legacy ones (`Virtqueue`), their
/// three parts given to the device separately.
pub struct ModernDevice {
    common: u64,
    notify: u64,
    notify_multiplier: u32,
    device: u64,
    device_len: u32,
    wanted: u64,
    features: u64,
    /// Our slot in MODERN_STATUS.
    registered: Option<usize>,
    queue_notify: [u16; MAX_QUEUES],
    // Dropped after `drop` has reset the device (fields drop last).
    queues: [Option<Virtqueue>; MAX_QUEUES],
}

impl ModernDevice {
    /// Finds the device's structures, maps them, resets the device and
    /// negotiates `wanted & offered` features (VERSION_1 always); set up
    /// the queues with `setup_queue`/`setup_chained_queue`, then `start`.
    pub fn new(function: &Function, wanted: u64) -> Result<Self, InitError> {
        let mut caps = [(0u8, 0u8); 48];
        let mut n = 0;
        function.capabilities(|at, id| {
            if n < caps.len() {
                caps[n] = (at, id);
                n += 1;
            }
        });
        let layout =
            find_layout(&caps[..n], |o| function.config_u32(o)).ok_or(InitError::NoModernLayout)?;
        let map = |r: CapRef, min: u64| -> Result<u64, InitError> {
            let base = function.memory_bar(r.bar).ok_or(InitError::NoMemoryBar)?;
            let len = u64::from(r.length).max(min);
            super::paging::map_mmio(base + u64::from(r.offset), len)
                .map_err(|_| InitError::OutOfMemory)
        };
        let common = map(layout.common, COMMON_LEN)?;
        // Notify addresses: queue_notify_off * multiplier within the
        // structure (at least the 2 bytes of queue 0's).
        let notify = map(layout.notify, 2)?;
        let (device, device_len) = match layout.device {
            Some(r) => (map(r, 4)?, r.length),
            None => (0, 0),
        };
        function.enable_memory_and_dma();
        let mut dev = Self {
            common,
            notify,
            notify_multiplier: layout.notify_multiplier,
            device,
            device_len,
            wanted: wanted | F_VERSION_1,
            features: 0,
            registered: None,
            queue_notify: [0; MAX_QUEUES],
            queues: [const { None }; MAX_QUEUES],
        };
        dev.registered = MODERN_STATUS.iter().position(|s| {
            s.compare_exchange(
                0,
                common + C_DEVICE_STATUS as u64,
                Ordering::AcqRel,
                Ordering::Relaxed,
            )
            .is_ok()
        });
        dev.negotiate()?;
        Ok(dev)
    }

    fn reg<T>(&self, offset: usize) -> *mut T {
        core::ptr::with_exposed_provenance_mut((self.common as usize) + offset)
    }

    fn read<T: Copy>(&self, offset: usize) -> T {
        assert!(offset + size_of::<T>() <= COMMON_LEN as usize);
        // SAFETY: a register of the common configuration this value mapped
        // (uncached, COMMON_LEN bytes), naturally aligned (the offsets are
        // the specification's).
        unsafe { self.reg::<T>(offset).read_volatile() }
    }

    fn write<T: Copy>(&mut self, offset: usize, value: T) {
        assert!(offset + size_of::<T>() <= COMMON_LEN as usize);
        // SAFETY: as in `read`. Writes can make the device DMA, but only to
        // queues this value owns (their addresses are the only ones given).
        unsafe { self.reg::<T>(offset).write_volatile(value) }
    }

    fn status(&self) -> u8 {
        self.read(C_DEVICE_STATUS)
    }

    fn set_status(&mut self, status: u8) {
        self.write(C_DEVICE_STATUS, status);
    }

    /// Reset, ACKNOWLEDGE, DRIVER, features, FEATURES_OK.
    fn negotiate(&mut self) -> Result<(), InitError> {
        self.set_status(0);
        // The device is reset when the status reads 0 again.
        let mut spins = 0u32;
        while self.status() != 0 {
            spins += 1;
            if spins > 1_000_000 {
                return Err(InitError::ResetTimeout);
            }
            core::hint::spin_loop();
        }
        self.set_status(STATUS_ACKNOWLEDGE);
        self.set_status(STATUS_ACKNOWLEDGE | STATUS_DRIVER);
        let mut offered = 0u64;
        for half in 0..2u32 {
            self.write(C_DEVICE_FEATURE_SELECT, half);
            offered |= u64::from(self.read::<u32>(C_DEVICE_FEATURE)) << (32 * half);
        }
        let features = offered & self.wanted;
        if features & F_VERSION_1 == 0 {
            self.set_status(STATUS_FAILED);
            return Err(InitError::NoVersion1);
        }
        for half in 0..2u32 {
            self.write(C_DRIVER_FEATURE_SELECT, half);
            self.write(C_DRIVER_FEATURE, (features >> (32 * half)) as u32);
        }
        self.set_status(STATUS_ACKNOWLEDGE | STATUS_DRIVER | STATUS_FEATURES_OK);
        if self.status() & STATUS_FEATURES_OK == 0 {
            self.set_status(STATUS_FAILED);
            return Err(InitError::FeaturesRefused);
        }
        self.features = features;
        Ok(())
    }

    /// The negotiated features.
    pub fn features(&self) -> u64 {
        self.features
    }

    pub fn num_queues(&self) -> u16 {
        self.read(C_NUM_QUEUES)
    }

    /// Sets up queue `index` with up to `slots` buffers of `slot_size`
    /// bytes (`device_writes`: buffers the device fills).
    pub fn setup_queue(
        &mut self,
        index: u16,
        slots: u16,
        slot_size: usize,
        device_writes: bool,
    ) -> Result<(), InitError> {
        self.setup(index, slots, slot_size, device_writes, false)
    }

    /// Sets up queue `index` for requests the device reads and answers
    /// (`Virtqueue::post_chain`).
    pub fn setup_chained_queue(
        &mut self,
        index: u16,
        slots: u16,
        slot_size: usize,
    ) -> Result<(), InitError> {
        self.setup(index, slots, slot_size, false, true)
    }

    fn setup(
        &mut self,
        index: u16,
        slots: u16,
        slot_size: usize,
        device_writes: bool,
        chained: bool,
    ) -> Result<(), InitError> {
        let i = usize::from(index);
        if i >= MAX_QUEUES || index >= self.num_queues() {
            return Err(InitError::NoQueue(index));
        }
        self.write(C_QUEUE_SELECT, index);
        let max: u16 = self.read(C_QUEUE_SIZE);
        if max == 0 {
            return Err(InitError::NoQueue(index));
        }
        // A small ring is enough: we keep few requests in flight (the
        // largest power of two up to 16 the device allows).
        let most = max.min(16);
        let size = 1u16 << (15 - most.leading_zeros());
        let queue = Virtqueue::new(size, slots, slot_size, device_writes, chained)
            .ok_or(InitError::OutOfMemory)?;
        let (desc, driver, device, size) = queue.addresses();
        self.write(C_QUEUE_SIZE, size);
        for (offset, addr) in
            [(C_QUEUE_DESC, desc), (C_QUEUE_DRIVER, driver), (C_QUEUE_DEVICE, device)]
        {
            self.write(offset, addr as u32);
            self.write(offset + 4, (addr >> 32) as u32);
        }
        self.queue_notify[i] = self.read(C_QUEUE_NOTIFY_OFF);
        self.write(C_QUEUE_ENABLE, 1u16);
        self.queues[i] = Some(queue);
        Ok(())
    }

    /// Tells the device the driver is ready.
    pub fn start(&mut self) {
        self.set_status(STATUS_ACKNOWLEDGE | STATUS_DRIVER | STATUS_FEATURES_OK | STATUS_DRIVER_OK);
    }

    /// Whether the device must be set up again: it gave up
    /// (DEVICE_NEEDS_RESET, FAILED), or it was reset under the driver
    /// (DRIVER_OK gone: its queues are forgotten).
    pub fn needs_reset(&self) -> bool {
        let status = self.status();
        status & (STATUS_FAILED | STATUS_NEEDS_RESET) != 0 || status & STATUS_DRIVER_OK == 0
    }

    /// Sets the device up again in place (after it gave up or was reset):
    /// reset, the old queues freed (the device no longer uses them), the
    /// features negotiated again; set up the queues and `start` again.
    pub fn reinit(&mut self) -> Result<(), InitError> {
        self.set_status(0);
        self.queues = [const { None }; MAX_QUEUES];
        self.negotiate()
    }

    /// Resets the device now (status 0), as a recovery test does
    /// (`gpureset=`): what was in flight is lost.
    pub fn reset_now(&mut self) {
        self.set_status(0);
    }

    pub fn queue(&mut self, index: u16) -> Option<&mut Virtqueue> {
        self.queues.get_mut(usize::from(index))?.as_mut()
    }

    /// Tells the device that queue `index` has new buffers.
    pub fn notify(&mut self, index: u16) {
        let i = usize::from(index);
        if i >= MAX_QUEUES || self.queues[i].is_none() {
            return;
        }
        let at = u64::from(self.queue_notify[i]) * u64::from(self.notify_multiplier);
        let addr = core::ptr::with_exposed_provenance_mut::<u16>((self.notify + at) as usize);
        // SAFETY: the queue's notify address inside the notify structure
        // this value mapped (the device gives queue_notify_off and the
        // multiplier so that it lies inside); writing the queue's index
        // makes the device read that queue, which this value owns.
        unsafe { addr.write_volatile(index) };
    }

    /// The little-endian u32 at `offset` of the device configuration (0
    /// outside it).
    pub fn config_u32(&self, offset: u16) -> u32 {
        if self.device == 0 || u32::from(offset) + 4 > self.device_len.max(4) {
            return 0;
        }
        let addr =
            core::ptr::with_exposed_provenance::<u32>((self.device + u64::from(offset)) as usize);
        // SAFETY: inside the device configuration this value mapped,
        // 4-byte aligned (callers' offsets are the specification's).
        unsafe { addr.read_volatile() }
    }

    pub fn set_config_u32(&mut self, offset: u16, value: u32) {
        if self.device == 0 || u32::from(offset) + 4 > self.device_len.max(4) {
            return;
        }
        let addr = core::ptr::with_exposed_provenance_mut::<u32>(
            (self.device + u64::from(offset)) as usize,
        );
        // SAFETY: as in `config_u32`.
        unsafe { addr.write_volatile(value) }
    }
}

impl Drop for ModernDevice {
    fn drop(&mut self) {
        // Reset: the device stops using the queues before they are freed.
        self.set_status(0);
        if let Some(i) = self.registered {
            MODERN_STATUS[i].store(0, Ordering::Release);
        }
    }
}

// ---- Device-readable memory in chunks --------------------------------------

/// Zeroed memory the device may read and write, in physically contiguous
/// chunks (a GPU resource's backing: megabytes, so not one region). The
/// owner must make the device forget it (detach it, or reset the device)
/// before dropping it.
pub struct DmaBuffer {
    regions: alloc::vec::Vec<DmaRegion>,
    len: usize,
}

impl DmaBuffer {
    /// `bytes` in chunks of at most `max_chunk_pages` pages (fewer when
    /// that much contiguous memory is not free; at most `max_chunks`).
    pub fn new(bytes: usize, max_chunk_pages: usize, max_chunks: usize) -> Option<Self> {
        use crate::mm::heap::TryVec;
        let mut regions = alloc::vec::Vec::new();
        regions.try_reserve_exact(max_chunks.min(bytes.div_ceil(PAGE))).ok()?;
        let mut left = bytes.div_ceil(PAGE);
        let mut chunk = max_chunk_pages.max(1);
        while left > 0 {
            if regions.len() == max_chunks {
                return None;
            }
            let pages = chunk.min(left);
            match DmaRegion::new(pages * PAGE) {
                Some(r) => {
                    regions.try_push(r).ok()?;
                    left -= pages;
                }
                None if chunk > 1 => chunk /= 2,
                None => return None,
            }
        }
        Some(Self { regions, len: bytes })
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn chunks(&self) -> usize {
        self.regions.len()
    }

    /// Chunk `index`: guest-physical address and bytes.
    pub fn chunk(&self, index: usize) -> (u64, u32) {
        self.regions.get(index).map_or((0, 0), |r| (r.phys(), r.len() as u32))
    }

    /// Stores `pixels` (as little-endian u32s) from byte `offset`; what
    /// falls past the end is dropped.
    pub fn write_u32s(&mut self, offset: usize, pixels: &[u32]) {
        // SAFETY: u32s are 4 initialized bytes each; x86 is little-endian,
        // so these are the bytes the device expects.
        let bytes =
            unsafe { core::slice::from_raw_parts(pixels.as_ptr().cast::<u8>(), pixels.len() * 4) };
        let bytes = &bytes[..bytes.len().min(self.len.saturating_sub(offset))];
        let mut base = 0;
        for region in &mut self.regions {
            let size = region.len();
            if let Some((at, from, n)) = overlap(base, size, offset, bytes.len()) {
                region.copy_in(at, &bytes[from..from + n]);
            }
            base += size;
        }
    }
}

/// The part of a write of `len` bytes at `offset` that falls in the chunk
/// of `size` bytes at `base`: (offset in the chunk, offset in the data,
/// bytes).
fn overlap(base: usize, size: usize, offset: usize, len: usize) -> Option<(usize, usize, usize)> {
    let (lo, hi) = (offset.max(base), (offset + len).min(base + size));
    (lo < hi).then(|| (lo - base, lo - offset, hi - lo))
}

#[cfg(test)]
mod tests {
    use super::{CapRef, ModernLayout, find_layout, layout, overlap};

    #[test]
    fn modern_layout_from_capabilities() {
        // QEMU's virtio-pci layout: common, ISR, device, notify in BAR 4.
        let cfg = [
            (0x84u8, 0x0110_7009u32), // vendor, next 0x70, len 16, common
            (0x88, 4),
            (0x8c, 0x0000),
            (0x90, 0x1000),
            (0x70, 0x0310_6009), // ISR (3): ignored
            (0x74, 4),
            (0x60, 0x0410_5009), // device configuration
            (0x64, 4),
            (0x68, 0x2000),
            (0x6c, 0x1000),
            (0xa0, 0x0214_0009), // notify, cap_len 20
            (0xa4, 4),
            (0xa8, 0x3000),
            (0xac, 0x1000),
            (0xb0, 4), // notify_off_multiplier
        ];
        let read = |o: u8| cfg.iter().find(|d| d.0 == o).map_or(0, |d| d.1);
        let caps = [(0x84, 9), (0x70, 9), (0x60, 9), (0xa0, 9), (0x40, 0x11)];
        let layout = find_layout(&caps, read).unwrap();
        assert_eq!(
            layout,
            ModernLayout {
                common: CapRef { bar: 4, offset: 0, length: 0x1000 },
                notify: CapRef { bar: 4, offset: 0x3000, length: 0x1000 },
                notify_multiplier: 4,
                device: Some(CapRef { bar: 4, offset: 0x2000, length: 0x1000 }),
            }
        );
        // Without a notify structure there is no modern interface.
        assert_eq!(find_layout(&caps[..3], read), None);
    }

    #[test]
    fn writes_split_over_chunks() {
        // Chunks of 8, 8 and 16 bytes at 0, 8, 16; 12 bytes written at 6.
        let pieces: Vec<_> =
            [(0, 8), (8, 8), (16, 16)].iter().map(|&(b, s)| overlap(b, s, 6, 12)).collect();
        assert_eq!(pieces, [Some((6, 0, 2)), Some((0, 2, 8)), Some((0, 10, 2))]);
        assert_eq!(overlap(0, 8, 8, 8), None);
        assert_eq!(overlap(8, 8, 8, 8), Some((0, 0, 8)));
        assert_eq!(overlap(0, 8, 4, 0), None);
    }

    #[test]
    fn legacy_queue_layout() {
        // 256 entries: 4096 bytes of descriptors, 518 of available ring,
        // the used ring on the next page.
        assert_eq!(layout(256), (8192, 8192 + 6 + 8 * 256));
        assert_eq!(layout(16), (4096, 4096 + 6 + 128));
    }
}

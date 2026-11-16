//! The kernel's address space on aarch64: its own translation tables for
//! the higher half (TTBR1_EL1, VMSAv8-64 with the 4 KiB granule:
//! `mm::paging::Mapper` with the `Vmsa` encoding), built and switched to
//! once the frame allocator is up, as x86-64's. User address spaces
//! (TTBR0_EL1) come with the port's user mode (ROADMAP AR); their
//! functions below are still the stub's.

use core::ops::Range;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::mm::PAGE_SIZE;
use crate::mm::frame::Frame;
use crate::mm::memmap::{MemoryRegion, RegionKind};
use crate::mm::paging::{
    CacheMode, MapError, Mapper, PageFlags, PageSize, PageTable, TableMemory, Translation, Vmsa,
};
use crate::sync::IrqMutex;

/// MAIR_EL1: one attribute per `CacheMode`, at its value (the `Vmsa`
/// encoding's AttrIndx): 0 write-back (0xff), 1 write-through (0xbb), 2
/// UC- as Device-nGnRE (0x04: `early_map`'s slot), 3 UC as Device-nGnRnE
/// (0x00), 4 write-protect as write-through (0xbb: VMSA has no such type,
/// the leaf is read-only anyway), 5 write-combining as Normal
/// non-cacheable (0x44). Slot 0 is Limine's write-back, as before.
pub const MAIR: u64 = 0x0000_44bb_0004_bbff;

unsafe extern "C" {
    static __kernel_start: u8;
    static __rodata_start: u8;
    static __rodata_end: u8;
    static __text_start: u8;
    static __text_end: u8;
    static __data_start: u8;
    static __data_end: u8;
    static __kernel_end: u8;
}

fn sym(s: &u8) -> u64 {
    s as *const u8 as u64
}

/// Translation tables accessed through the higher-half direct map.
pub struct HhdmTables {
    offset: u64,
}

impl TableMemory for HhdmTables {
    fn table(&mut self, frame: Frame) -> &mut PageTable {
        let virt = self.offset + frame.start();
        // SAFETY: table frames come from the frame allocator (usable RAM,
        // HHDM-mapped, owned by this address space) and are only reached
        // through the `Mapper` that owns this `HhdmTables`, one at a time.
        unsafe { &mut *core::ptr::with_exposed_provenance_mut::<PageTable>(virt as usize) }
    }

    /// Through the frame's HHDM address (`cpu::sync_code`).
    fn sync_code(&mut self, phys: u64, bytes: u64) {
        super::cpu::sync_code(self.offset + phys, bytes);
    }
    const SYNCS_CODE: bool = true;
}

static KERNEL_SPACE: IrqMutex<Option<Mapper<HhdmTables, Vmsa>>> = IrqMutex::new(None);

/// Device mappings (bump-allocated, never freed).
const MMIO_WINDOW: Range<u64> = 0xffff_e000_0000_0000..0xffff_e100_0000_0000;
static MMIO_NEXT: AtomicU64 = AtomicU64::new(MMIO_WINDOW.start);

/// The kernel's TTBR1 root (0 before `init`).
static KERNEL_ROOT: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
pub enum PagingError {
    NotInitialized,
    MmioWindowFull,
    Map(MapError),
}

impl From<MapError> for PagingError {
    fn from(e: MapError) -> Self {
        PagingError::Map(e)
    }
}

/// Inputs from the bootloader.
pub struct KernelLayout<'a> {
    pub hhdm_offset: u64,
    pub kernel_phys_base: u64,
    pub kernel_virt_base: u64,
    pub memory_map: &'a mut dyn Iterator<Item = MemoryRegion>,
}

/// Statistics for the boot log.
#[derive(Debug, Default)]
pub struct BuildStats {
    pub hhdm_pages: [u64; 3],
    pub kernel_pages: u64,
}

pub(in crate::arch) fn alloc_table() -> Option<Frame> {
    crate::mm::allocate_frame(crate::mm::Use::PageTable)
}

/// HHDM flags for a memory map region (what Limine maps there), or None.
fn hhdm_flags(kind: RegionKind) -> Option<PageFlags> {
    match kind {
        RegionKind::Usable
        | RegionKind::BootloaderReclaimable
        | RegionKind::ExecutableAndModules
        | RegionKind::AcpiReclaimable
        | RegionKind::AcpiNvs
        | RegionKind::ReservedMapped => Some(PageFlags::KERNEL_RW),
        RegionKind::Framebuffer => {
            Some(PageFlags { cache: CacheMode::WriteCombining, ..PageFlags::KERNEL_RW })
        }
        RegionKind::Reserved | RegionKind::BadMemory | RegionKind::Unknown(_) => None,
    }
}

/// Builds the kernel's higher half (the HHDM for every region Limine maps
/// there, the image with per-segment permissions, the device pages mapped
/// so far by `early_map`, at the same addresses) and switches TTBR1 to it.
/// Once, after the frame allocator is up.
pub fn init(layout: KernelLayout<'_>) -> Result<BuildStats, PagingError> {
    let mut stats = BuildStats::default();
    let mut mapper: Mapper<HhdmTables, Vmsa> =
        Mapper::new(HhdmTables { offset: layout.hhdm_offset }, &mut alloc_table)?;
    // Regions are sorted; round outward to pages, never a page twice.
    let mut mapped_until = 0;
    for region in layout.memory_map {
        let Some(flags) = hhdm_flags(region.kind) else { continue };
        let start = (region.base / PAGE_SIZE * PAGE_SIZE).max(mapped_until);
        let end = region.end().next_multiple_of(PAGE_SIZE);
        if start >= end {
            continue;
        }
        let virt = layout.hhdm_offset + start;
        let counts = mapper.map_range(virt, start, end - start, flags, &mut alloc_table)?;
        for (total, n) in stats.hhdm_pages.iter_mut().zip(counts) {
            *total += n;
        }
        mapped_until = end;
    }
    // SAFETY: linker-script symbols; only their addresses are used.
    let segments: [(Range<u64>, PageFlags); 3] = unsafe {
        [
            (sym(&__rodata_start)..sym(&__rodata_end), PageFlags::KERNEL_RO),
            (sym(&__text_start)..sym(&__text_end), PageFlags::KERNEL_RX),
            (sym(&__data_start)..sym(&__data_end), PageFlags::KERNEL_RW),
        ]
    };
    for (range, flags) in segments {
        let phys = range.start - layout.kernel_virt_base + layout.kernel_phys_base;
        let len = range.end - range.start;
        mapper.map_range(range.start, phys, len, flags, &mut alloc_table)?;
        stats.kernel_pages += len / PAGE_SIZE;
    }
    for (virt, phys) in super::early_map::mapped() {
        let device = PageFlags { cache: CacheMode::UncachedMinus, ..PageFlags::MMIO };
        mapper.map(virt, phys, PageSize::Size4K, device, &mut alloc_table)?;
    }
    let root = mapper.root().start();
    // SAFETY: the new tables map everything the kernel uses now at the same
    // addresses with the same attributes (MAIR's slot 0 stays write-back,
    // slot 2 Device-nGnRE): its image, through the HHDM the boot stack,
    // the bootloader's data, the frame bitmap and the tables, and the
    // early device pages. The old entries are invalidated (all of EL1's:
    // the kernel's are global) before any further access.
    unsafe {
        core::arch::asm!(
            "msr mair_el1, {mair}",
            "dsb ishst",
            "msr ttbr1_el1, {root}",
            "isb",
            "tlbi vmalle1",
            "dsb nsh",
            "isb",
            mair = in(reg) MAIR,
            root = in(reg) root,
            options(nostack, preserves_flags)
        )
    };
    KERNEL_ROOT.store(root, Ordering::Release);
    *KERNEL_SPACE.lock() = Some(mapper);
    Ok(stats)
}

fn with_space<R>(f: impl FnOnce(&mut Mapper<HhdmTables, Vmsa>) -> R) -> Option<R> {
    KERNEL_SPACE.lock().as_mut().map(f)
}

pub fn translate(virt: u64) -> Option<Translation> {
    with_space(|m| m.translate(virt)).flatten()
}

/// The higher half's root (TTBR1_EL1's table address).
pub fn active_root() -> u64 {
    let ttbr1: u64;
    // SAFETY: reads TTBR1_EL1.
    unsafe {
        core::arch::asm!("mrs {}, ttbr1_el1", out(reg) ttbr1, options(nomem, nostack, preserves_flags))
    };
    ttbr1 & 0x0000_ffff_ffff_f000
}

/// Kernel image bounds `[start, end)`.
pub fn kernel_image() -> Range<u64> {
    // SAFETY: linker-script symbols; only their addresses are used.
    unsafe { sym(&__kernel_start)..sym(&__kernel_end) }
}

/// The kernel's code `[start, end)`.
pub fn kernel_text() -> Range<u64> {
    // SAFETY: linker-script symbols; only their addresses are used.
    unsafe { sym(&__text_start)..sym(&__text_end) }
}

/// Maps `len` bytes of device memory at physical `phys` (Device-nGnRnE)
/// and returns the virtual address corresponding to `phys`.
pub fn map_mmio(phys: u64, len: u64) -> Result<u64, PagingError> {
    let start = phys / PAGE_SIZE * PAGE_SIZE;
    let size = (phys + len).next_multiple_of(PAGE_SIZE) - start;
    let virt = MMIO_NEXT.fetch_add(size, Ordering::Relaxed);
    if virt + size > MMIO_WINDOW.end {
        return Err(PagingError::MmioWindowFull);
    }
    with_space(|m| m.map_range(virt, start, size, PageFlags::MMIO, &mut alloc_table))
        .ok_or(PagingError::NotInitialized)??;
    Ok(virt + (phys - start))
}

/// Maps one 4 KiB kernel page. The page must not be mapped already.
pub fn map_kernel_page(virt: u64, frame: Frame, flags: PageFlags) -> Result<(), PagingError> {
    with_space(|m| m.map(virt, frame.start(), PageSize::Size4K, flags, &mut alloc_table))
        .ok_or(PagingError::NotInitialized)??;
    // The entry was invalid: only the store must reach the walker.
    // SAFETY: barriers only.
    unsafe { core::arch::asm!("dsb ishst", "isb", options(nostack, preserves_flags)) };
    Ok(())
}

/// Unmaps one 4 KiB kernel page and invalidates it on every CPU of the
/// inner shareable domain (`tlbi vaae1is`). Returns the frame it mapped.
pub fn unmap_kernel_page(virt: u64) -> Option<Frame> {
    let frame = with_space(|m| m.unmap_4k(virt)).flatten()?;
    // SAFETY: invalidating TLB entries cannot break memory safety.
    unsafe {
        core::arch::asm!(
            "dsb ishst",
            "tlbi vaae1is, {}",
            "dsb ish",
            "isb",
            in(reg) (virt >> 12) & 0x0000_0fff_ffff_ffff,
            options(nostack, preserves_flags)
        )
    };
    Some(frame)
}

/// Frames leaked because their deferred free could not be recorded.
pub fn leaked_frames() -> u64 {
    crate::arch::user_space::leaked_frames()
}

/// A TTBR0 root mapping the code page at physical `page` at its own
/// address (executable, not global: a TLB entry of it never outlives a
/// `tlbi vmalle1`): what a CPU started by PSCI runs the instruction after
/// turning its MMU on from. Its tables are kept (one root serves every
/// CPU the kernel starts).
pub fn identity_root(page: u64) -> Result<u64, PagingError> {
    let mut mapper: Mapper<HhdmTables, Vmsa> =
        Mapper::new(HhdmTables { offset: crate::mm::hhdm_offset() }, &mut alloc_table)?;
    let flags = PageFlags { global: false, ..PageFlags::KERNEL_RX };
    let frame = Frame::containing(page);
    mapper.map(frame.start(), frame.start(), PageSize::Size4K, flags, &mut alloc_table)?;
    Ok(mapper.root().start())
}

/// The kernel's TTBR1 root (0 before `init`).
pub fn kernel_root() -> u64 {
    KERNEL_ROOT.load(Ordering::Acquire)
}

/// TTBR0's root while no user space is loaded (a kernel thread): an empty
/// table, so the walker never reaches a freed space's tables (0: not
/// made yet).
static EMPTY_USER_ROOT: AtomicU64 = AtomicU64::new(0);

fn empty_user_root() -> u64 {
    let root = EMPTY_USER_ROOT.load(Ordering::Acquire);
    if root != 0 {
        return root;
    }
    let Some(frame) = alloc_table() else { return 0 };
    // SAFETY: a fresh frame, HHDM-mapped: zeroed, it is an empty table.
    unsafe { hhdm_ptr(frame.start()).write_bytes(0, PAGE_SIZE as usize) };
    match EMPTY_USER_ROOT.compare_exchange(0, frame.start(), Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => frame.start(),
        Err(other) => {
            let _ = crate::mm::free_frame(frame);
            other
        }
    }
}

/// Loads an address space on this CPU: a user space's root in TTBR0 (the
/// kernel's root, TTBR1's, stands for no user space: an empty TTBR0).
/// Every user space is tagged ASID 0 (not global), so a switch drops this
/// CPU's ASID-0 entries (`tlbi aside1`); the kernel's are global and stay.
///
/// # Safety
/// `root` is the kernel's root or a live `UserSpace`'s, which stays alive
/// while loaded.
pub unsafe fn activate(root: u64) {
    let ttbr0 = if root == kernel_root() { empty_user_root() } else { root };
    if ttbr0 == 0 {
        return;
    }
    let current: u64;
    // SAFETY: reads TTBR0_EL1.
    unsafe {
        core::arch::asm!("mrs {}, ttbr0_el1", out(reg) current, options(nomem, nostack, preserves_flags))
    };
    if current & ADDRESS_BITS == ttbr0 {
        return;
    }
    // SAFETY: forwarded to the caller: the kernel (TTBR1's) is unchanged,
    // so the code, stack and data in use stay mapped.
    unsafe {
        core::arch::asm!(
            "msr ttbr0_el1, {root}",
            "isb",
            "tlbi aside1, xzr",
            "dsb nsh",
            "isb",
            root = in(reg) ttbr0,
            options(nostack, preserves_flags)
        )
    };
}

/// TTBR0's and TTBR1's table address field.
const ADDRESS_BITS: u64 = 0x0000_ffff_ffff_f000;

pub use crate::arch::user_space::{
    SharedPage, SharedUserSpace, SpaceGuard, SpaceLock, UserFault, UserSpace,
};

/// A user space's tables (`arch::user_space`): VMSAv8-64's encoding.
pub(in crate::arch) type UserMapper = Mapper<HhdmTables, Vmsa>;

pub(in crate::arch) fn hhdm_tables() -> HhdmTables {
    HhdmTables { offset: crate::mm::hhdm_offset() }
}

/// A new user space's tables: a TTBR0 root of its own (the kernel's half
/// is TTBR1's, in no user root).
pub(in crate::arch) fn user_mapper() -> Result<UserMapper, PagingError> {
    if kernel_root() == 0 {
        return Err(PagingError::NotInitialized);
    }
    Ok(Mapper::new(hhdm_tables(), &mut alloc_table)?)
}

/// Drops every CPU's TLB entry for user page `virt` (ASID 0) after a
/// change: a broadcast to the inner shareable domain, complete when the
/// DSB is (so `smp::flush_user_space` has nothing left to do).
pub(in crate::arch) fn invalidate_user_page(virt: u64) {
    // SAFETY: TLB maintenance only.
    unsafe {
        core::arch::asm!(
            "dsb ishst",
            "tlbi vale1is, {page}",
            "dsb ish",
            "isb",
            page = in(reg) (virt >> 12) & 0xfff_ffff_ffff,
            options(nostack, preserves_flags)
        )
    };
}

fn hhdm_ptr(phys: u64) -> *mut u8 {
    core::ptr::with_exposed_provenance_mut((crate::mm::hhdm_offset() + phys) as usize)
}

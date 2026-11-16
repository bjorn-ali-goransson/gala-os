//! Kernel address space: builds our own page tables and switches to them.

use core::arch::asm;
use core::ops::Range;

use core::sync::atomic::{AtomicU64, Ordering};

use super::cpu;
use crate::mm::PAGE_SIZE;
use crate::mm::frame::Frame;
use crate::mm::memmap::{MemoryRegion, RegionKind};
use crate::mm::paging::{
    CacheMode, MapError, Mapper, PageFlags, PageTable, TableMemory, Translation,
};
use crate::sync::IrqMutex;

const IA32_PAT: u32 = 0x277;
const IA32_EFER: u32 = 0xc000_0080;
const EFER_NXE: u64 = 1 << 11;
const CR4_PGE: u64 = 1 << 7;

/// PAT entries in `CacheMode` index order: WB, WT, UC-, UC, WP, WC, UC-, UC.
/// (Same as Limine's layout, but programmed explicitly so we do not depend on it.)
pub const PAT_VALUE: u64 = 0x00_07_01_05_00_07_04_06;

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

/// Page tables accessed through the higher-half direct map.
pub struct HhdmTables {
    offset: u64,
}

impl HhdmTables {
    /// Tables whose frames the caller owns (see `table`).
    pub(super) fn new(offset: u64) -> Self {
        HhdmTables { offset }
    }
}

impl TableMemory for HhdmTables {
    fn table(&mut self, frame: Frame) -> &mut PageTable {
        let virt = self.offset + frame.start();
        // SAFETY: page-table frames come from the frame allocator (usable RAM,
        // HHDM-mapped, owned exclusively by this address space) and are only
        // accessed through the `Mapper` that owns this `HhdmTables`, one
        // table at a time.
        unsafe { &mut *core::ptr::with_exposed_provenance_mut::<PageTable>(virt as usize) }
    }
}

static KERNEL_SPACE: IrqMutex<Option<Mapper<HhdmTables>>> = IrqMutex::new(None);

/// Virtual window for device memory mappings (bump-allocated, never freed).
const MMIO_WINDOW: Range<u64> = 0xffff_e000_0000_0000..0xffff_e100_0000_0000;
static MMIO_NEXT: AtomicU64 = AtomicU64::new(MMIO_WINDOW.start);

#[derive(Debug)]
pub enum PagingError {
    NoExecuteUnsupported,
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

fn enable_cpu_features() -> Result<(), PagingError> {
    const NX_BIT: u32 = 1 << 20;
    if cpu::cpuid(0x8000_0001, 0).edx & NX_BIT == 0 {
        return Err(PagingError::NoExecuteUnsupported);
    }
    // SAFETY: PAT and EFER exist on every x86_64 CPU. The new PAT keeps index
    // 0 (used by every existing mapping) as WB, and Limine's index 5 (its
    // framebuffer mapping) as WC, so current mappings keep their types.
    // NXE is already required by Limine's own NX mappings if NX exists.
    unsafe {
        cpu::wrmsr(IA32_PAT, PAT_VALUE);
        cpu::wrmsr(IA32_EFER, cpu::rdmsr(IA32_EFER) | EFER_NXE);
        let cr4 = cpu::read_cr4() | CR4_PGE;
        asm!("mov cr4, {}", in(reg) cr4, options(nostack, preserves_flags));
    }
    Ok(())
}

/// HHDM flags for a memory map region, or `None` if it is not mapped.
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

/// Maps the HHDM at `offset` for every region of `regions` (sorted) that
/// Limine maps there; returns the pages mapped per size (4K, 2M, 1G).
pub(super) fn map_hhdm(
    mapper: &mut Mapper<HhdmTables>,
    offset: u64,
    regions: impl Iterator<Item = MemoryRegion>,
    alloc: &mut impl FnMut() -> Option<Frame>,
) -> Result<[u64; 3], PagingError> {
    // Regions are sorted; round outward to pages but never map a page
    // twice (non-RAM regions may share pages).
    let mut pages = [0; 3];
    let mut mapped_until = 0;
    for region in regions {
        let Some(flags) = hhdm_flags(region.kind) else { continue };
        let start = (region.base / PAGE_SIZE * PAGE_SIZE).max(mapped_until);
        let end = region.end().next_multiple_of(PAGE_SIZE);
        if start >= end {
            continue;
        }
        let counts = mapper.map_range(offset + start, start, end - start, flags, alloc)?;
        for (total, n) in pages.iter_mut().zip(counts) {
            *total += n;
        }
        mapped_until = end;
    }
    Ok(pages)
}

/// Builds the kernel address space (kernel image with per-section
/// permissions, plus the HHDM for every region Limine maps there) and loads
/// it. Must be called once, after the frame allocator is initialized.
pub fn init(layout: KernelLayout<'_>) -> Result<BuildStats, PagingError> {
    enable_cpu_features()?;
    let mut stats = BuildStats::default();
    let mut mapper = Mapper::new(HhdmTables { offset: layout.hhdm_offset }, &mut alloc_table)?;
    stats.hhdm_pages =
        map_hhdm(&mut mapper, layout.hhdm_offset, layout.memory_map, &mut alloc_table)?;

    // Kernel image.
    // SAFETY: these are linker-script symbols; only their addresses are used.
    let sections: [(Range<u64>, PageFlags); 3] = unsafe {
        [
            (sym(&__rodata_start)..sym(&__rodata_end), PageFlags::KERNEL_RO),
            (sym(&__text_start)..sym(&__text_end), PageFlags::KERNEL_RX),
            (sym(&__data_start)..sym(&__data_end), PageFlags::KERNEL_RW),
        ]
    };
    for (range, flags) in sections {
        let phys = range.start - layout.kernel_virt_base + layout.kernel_phys_base;
        let len = range.end - range.start;
        mapper.map_range(range.start, phys, len, flags, &mut alloc_table)?;
        stats.kernel_pages += len / PAGE_SIZE;
    }

    finish_kernel_half(&mut mapper)?;
    let root = mapper.root().start();
    // SAFETY: the new tables map everything the kernel currently uses: its
    // image (code, statics), and through the HHDM the current stack and boot
    // data (bootloader-reclaimable), frame-allocator bitmap and page tables
    // (usable) and the framebuffer, at the same virtual addresses as before.
    unsafe { asm!("mov cr3, {}", in(reg) root, options(nostack, preserves_flags)) };
    *KERNEL_SPACE.lock() = Some(mapper);
    Ok(stats)
}

fn with_space<R>(f: impl FnOnce(&mut Mapper<HhdmTables>) -> R) -> Option<R> {
    KERNEL_SPACE.lock().as_mut().map(f)
}

pub fn translate(virt: u64) -> Option<Translation> {
    with_space(|m| m.translate(virt)).flatten()
}

/// Physical address of the active PML4.
pub fn active_root() -> u64 {
    cpu::read_cr3() & !0xfff
}

/// Kernel image bounds `[start, end)`.
pub fn kernel_image() -> Range<u64> {
    // SAFETY: linker-script symbols; only their addresses are used.
    unsafe { sym(&__kernel_start)..sym(&__kernel_end) }
}

/// The kernel's code `[start, end)` (`.text`; a hot swap checks that it is
/// the build its plan was made for).
pub fn kernel_text() -> Range<u64> {
    // SAFETY: linker-script symbols; only their addresses are used.
    unsafe { sym(&__text_start)..sym(&__text_end) }
}

/// Maps `len` bytes of device memory at physical `phys` uncached and returns
/// the virtual address corresponding to `phys`.
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

/// Maps one 4 KiB kernel page. The range must not be mapped already.
pub fn map_kernel_page(virt: u64, frame: Frame, flags: PageFlags) -> Result<(), PagingError> {
    use crate::mm::paging::PageSize;
    with_space(|m| m.map(virt, frame.start(), PageSize::Size4K, flags, &mut alloc_table))
        .ok_or(PagingError::NotInitialized)??;
    Ok(())
}

/// Unmaps one 4 KiB kernel page and flushes it from this CPU's TLB.
/// Returns the frame that was mapped there.
// TODO(smp): TLB shootdown on other CPUs.
pub fn unmap_kernel_page(virt: u64) -> Option<Frame> {
    let frame = with_space(|m| m.unmap_4k(virt)).flatten()?;
    // SAFETY: invalidating a TLB entry cannot break memory safety.
    unsafe { asm!("invlpg [{}]", in(reg) virt, options(nostack, preserves_flags)) };
    Some(frame)
}

/// Frames kept forever because their deferred free could not be recorded.
/// Frames leaked because their deferred free could not be recorded.
pub fn leaked_frames() -> u64 {
    crate::arch::user_space::leaked_frames()
}

/// Physical address of the kernel's PML4 (0 before `init`).
static KERNEL_ROOT: AtomicU64 = AtomicU64::new(0);

/// Must run at the end of `init`: shares one kernel half with all future
/// address spaces.
fn finish_kernel_half(mapper: &mut Mapper<HhdmTables>) -> Result<(), PagingError> {
    mapper.populate_kernel_half(&mut alloc_table)?;
    KERNEL_ROOT.store(mapper.root().start(), Ordering::Release);
    Ok(())
}

pub fn kernel_root() -> u64 {
    KERNEL_ROOT.load(Ordering::Acquire)
}

/// Loads the page tables rooted at physical `root`.
///
/// # Safety
/// `root` must be the kernel's root or the root of a live `UserSpace`
/// (whose kernel half is the shared one), and must stay alive while loaded.
pub unsafe fn activate(root: u64) {
    // Before loading it: a shootdown that misses this CPU's new root then
    // happened before the load, which flushes non-global entries.
    super::smp::note_root(root);
    if cpu::read_cr3() & !0xfff != root || super::smp::tlb_paranoid() {
        // SAFETY: forwarded to the caller; the kernel half, and therefore
        // the code, stack and data in use, is identical in every such space.
        unsafe { asm!("mov cr3, {}", in(reg) root, options(nostack, preserves_flags)) };
    }
}

pub use crate::arch::user_space::{
    SharedPage, SharedUserSpace, SpaceGuard, SpaceLock, UserFault, UserSpace,
};

/// A user space's tables (`arch::user_space`): x86-64's encoding.
pub(in crate::arch) type UserMapper = Mapper<HhdmTables>;

pub(in crate::arch) fn hhdm_tables() -> HhdmTables {
    HhdmTables { offset: crate::mm::hhdm_offset() }
}

/// A new user space's tables: its own lower half, the kernel's upper half
/// shared (the same PML4 entries as the kernel's root).
pub(in crate::arch) fn user_mapper() -> Result<UserMapper, PagingError> {
    let kernel = kernel_root();
    if kernel == 0 {
        return Err(PagingError::NotInitialized);
    }
    Ok(Mapper::new_sharing_kernel_half(hhdm_tables(), Frame::containing(kernel), &mut alloc_table)?)
}

/// Drops this CPU's TLB entry for user page `virt` after a change (the
/// others' at the space lock's release: `smp::flush_user_space`).
pub(in crate::arch) fn invalidate_user_page(virt: u64) {
    // SAFETY: invalidating a TLB entry cannot break memory safety. Doing it
    // for a space that is not active only costs an unnecessary flush.
    unsafe { asm!("invlpg [{}]", in(reg) virt, options(nostack, preserves_flags)) };
}

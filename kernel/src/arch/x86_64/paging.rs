//! Kernel address space: builds our own page tables and switches to them.

use core::arch::asm;
use core::ops::Range;

use core::sync::atomic::{AtomicU64, Ordering};

use super::cpu;
use crate::mm::PAGE_SIZE;
use crate::mm::frame::Frame;
use crate::mm::heap::TryVec;
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

fn alloc_table() -> Option<Frame> {
    crate::mm::allocate_frame()
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

/// Builds the kernel address space (kernel image with per-section
/// permissions, plus the HHDM for every region Limine maps there) and loads
/// it. Must be called once, after the frame allocator is initialized.
pub fn init(layout: KernelLayout<'_>) -> Result<BuildStats, PagingError> {
    enable_cpu_features()?;
    let mut stats = BuildStats::default();
    let mut mapper = Mapper::new(HhdmTables { offset: layout.hhdm_offset }, &mut alloc_table)?;

    // HHDM. Regions are sorted; round outward to pages but never map a page
    // twice (non-RAM regions may share pages).
    let mut mapped_until = 0;
    for region in layout.memory_map {
        let Some(flags) = hhdm_flags(region.kind) else { continue };
        let start = (region.base / PAGE_SIZE * PAGE_SIZE).max(mapped_until);
        let end = region.end().next_multiple_of(PAGE_SIZE);
        if start >= end {
            continue;
        }
        let counts = mapper.map_range(
            layout.hhdm_offset + start,
            start,
            end - start,
            flags,
            &mut alloc_table,
        )?;
        for (total, n) in stats.hhdm_pages.iter_mut().zip(counts) {
            *total += n;
        }
        mapped_until = end;
    }

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
static LEAKED_FRAMES: AtomicU64 = AtomicU64::new(0);

/// Frames leaked because their deferred free could not be recorded.
pub fn leaked_frames() -> u64 {
    LEAKED_FRAMES.load(Ordering::Relaxed)
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
    if cpu::read_cr3() & !0xfff != root {
        // SAFETY: forwarded to the caller; the kernel half, and therefore
        // the code, stack and data in use, is identical in every such space.
        unsafe { asm!("mov cr3, {}", in(reg) root, options(nostack, preserves_flags)) };
    }
}

fn hhdm_ptr(phys: u64) -> *mut u8 {
    core::ptr::with_exposed_provenance_mut((crate::mm::hhdm_offset() + phys) as usize)
}

/// A user address space shared by the threads of a process.
pub type SharedUserSpace = crate::mm::heap::KArc<SpaceLock>;

/// The lock around a `UserSpace`. Releasing it completes the mapping
/// changes made under it: CPUs that may cache the old translations are
/// flushed (TLB shootdown), then the frames those changes unmapped are
/// freed. Threads of the space may run on other CPUs meanwhile.
pub struct SpaceLock(IrqMutex<UserSpace>);

impl SpaceLock {
    pub fn new(space: UserSpace) -> Self {
        SpaceLock(IrqMutex::new(space))
    }

    pub fn lock(&self) -> SpaceGuard<'_> {
        SpaceGuard { guard: Some(self.0.lock()) }
    }
}

pub struct SpaceGuard<'a> {
    guard: Option<crate::sync::IrqGuard<'a, UserSpace>>,
}

impl core::ops::Deref for SpaceGuard<'_> {
    type Target = UserSpace;

    fn deref(&self) -> &UserSpace {
        self.guard.as_ref().expect("guard is present until drop")
    }
}

impl core::ops::DerefMut for SpaceGuard<'_> {
    fn deref_mut(&mut self) -> &mut UserSpace {
        self.guard.as_mut().expect("guard is present until drop")
    }
}

impl Drop for SpaceGuard<'_> {
    fn drop(&mut self) {
        let Some(mut guard) = self.guard.take() else { return };
        let dirty = core::mem::take(&mut guard.dirty);
        let frames = core::mem::take(&mut guard.freed);
        let root = guard.root();
        drop(guard);
        if dirty {
            super::smp::flush_user_space(root);
        }
        for frame in frames {
            let _ = crate::mm::free_frame(frame);
        }
    }
}

/// A user address space: private lower half, shared kernel half. Owns its
/// page tables and every frame mapped in its lower half.
pub struct UserSpace {
    mapper: Mapper<HhdmTables>,
    /// Mappings were changed or removed since the lock was taken: other
    /// CPUs must be flushed when it is released.
    dirty: bool,
    /// Frames unmapped under the lock, freed once other CPUs are flushed.
    freed: alloc::vec::Vec<Frame>,
}

/// A kernel-owned page that is also mapped into user address spaces
/// (e.g. KUSER_SHARED_DATA). The kernel writes it through the HHDM with
/// volatile stores, since user threads read it concurrently.
pub struct SharedPage {
    frame: Frame,
}

impl SharedPage {
    pub fn new() -> Result<Self, PagingError> {
        let frame = crate::mm::allocate_frame().ok_or(PagingError::Map(MapError::OutOfMemory))?;
        // SAFETY: a freshly allocated frame, HHDM-mapped, owned by us.
        unsafe { hhdm_ptr(frame.start()).write_bytes(0, PAGE_SIZE as usize) };
        Ok(Self { frame })
    }

    pub fn frame(&self) -> Frame {
        self.frame
    }

    /// Writes `bytes` at `offset`; out-of-range writes are dropped.
    pub fn write(&self, offset: usize, bytes: &[u8]) {
        if offset.checked_add(bytes.len()).is_none_or(|end| end > PAGE_SIZE as usize) {
            return;
        }
        let base = hhdm_ptr(self.frame.start());
        for (i, &b) in bytes.iter().enumerate() {
            // SAFETY: in bounds of the owned frame; volatile because user
            // mode reads the page concurrently through its own mapping.
            unsafe { base.add(offset + i).write_volatile(b) };
        }
    }

    pub fn write_u32(&self, offset: usize, value: u32) {
        if !offset.is_multiple_of(4) || offset + 4 > PAGE_SIZE as usize {
            return;
        }
        // SAFETY: aligned and in bounds of the owned frame; a single
        // volatile store, so readers never see a torn 32-bit value.
        unsafe { hhdm_ptr(self.frame.start()).add(offset).cast::<u32>().write_volatile(value) };
    }
}

/// A user address was unmapped, not user-accessible, or (for writes) not
/// writable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UserFault {
    pub address: u64,
}

fn invlpg(virt: u64) {
    // SAFETY: invalidating a TLB entry cannot break memory safety. Doing it
    // for a space that is not active only costs an unnecessary flush.
    unsafe { asm!("invlpg [{}]", in(reg) virt, options(nostack, preserves_flags)) };
}

impl UserSpace {
    /// Maps one fresh zeroed page at `virt` (demand-zero fault, commit).
    pub fn map_zero_page(&mut self, virt: u64, flags: PageFlags) -> Result<(), PagingError> {
        self.map_zeroed(virt / PAGE_SIZE * PAGE_SIZE, 1, flags)
    }

    /// Whether `virt` has a page-table mapping (of any kind).
    pub fn is_mapped(&mut self, virt: u64) -> bool {
        self.mapper.translate(virt).is_some()
    }

    /// Changes the flags of a mapped page; `user` is not forced, so a
    /// kernel-only flags value makes the page inaccessible to user mode.
    pub fn set_flags(&mut self, virt: u64, mut flags: PageFlags) -> Result<(), PagingError> {
        flags.global = false;
        self.mapper.update_flags_4k(virt / PAGE_SIZE * PAGE_SIZE, flags)?;
        invlpg(virt);
        self.dirty = true;
        Ok(())
    }

    /// Unmaps a page and frees its frame. Returns whether it was mapped.
    pub fn unmap_page(&mut self, virt: u64) -> bool {
        let virt = virt / PAGE_SIZE * PAGE_SIZE;
        match self.mapper.unmap_4k_owned(virt) {
            Some((frame, owned)) => {
                invlpg(virt);
                self.dirty = true;
                // Other CPUs may still reach the frame through their TLBs
                // until the lock is released (see `SpaceGuard`).
                if owned && self.freed.try_push(frame).is_err() {
                    // No memory to defer the free: keep the frame (a
                    // leak) rather than reuse it while still mapped.
                    LEAKED_FRAMES.fetch_add(1, Ordering::Relaxed);
                }
                true
            }
            None => false,
        }
    }

    pub fn new() -> Result<Self, PagingError> {
        let kernel = kernel_root();
        if kernel == 0 {
            return Err(PagingError::NotInitialized);
        }
        let mem = HhdmTables { offset: crate::mm::hhdm_offset() };
        let mapper =
            Mapper::new_sharing_kernel_half(mem, Frame::containing(kernel), &mut alloc_table)?;
        Ok(Self { mapper, dirty: false, freed: alloc::vec::Vec::new() })
    }

    pub fn root(&self) -> u64 {
        self.mapper.root().start()
    }

    /// Maps `pages` fresh zeroed pages at `virt` (page-aligned, user half).
    pub fn map_zeroed(
        &mut self,
        virt: u64,
        pages: u64,
        mut flags: PageFlags,
    ) -> Result<(), PagingError> {
        use crate::mm::paging::{PageSize, USER_END};
        flags.user = true;
        flags.global = false;
        let end =
            virt.checked_add(pages * PAGE_SIZE).ok_or(PagingError::Map(MapError::BadAddress))?;
        if end > USER_END || !virt.is_multiple_of(PAGE_SIZE) {
            return Err(PagingError::Map(MapError::BadAddress));
        }
        for i in 0..pages {
            let frame =
                crate::mm::allocate_frame().ok_or(PagingError::Map(MapError::OutOfMemory))?;
            // SAFETY: a freshly allocated frame, HHDM-mapped, owned by us.
            unsafe { hhdm_ptr(frame.start()).write_bytes(0, PAGE_SIZE as usize) };
            if let Err(e) = self.mapper.map(
                virt + i * PAGE_SIZE,
                frame.start(),
                PageSize::Size4K,
                flags,
                &mut alloc_table,
            ) {
                let _ = crate::mm::free_frame(frame);
                return Err(e.into());
            }
        }
        Ok(())
    }

    /// Calls `f(phys, len, offset)` for each physically contiguous piece of
    /// the user range `[virt, virt + len)`, checking user access (and
    /// writability if `write`).
    fn for_each_piece(
        &mut self,
        virt: u64,
        len: usize,
        write: bool,
        mut f: impl FnMut(u64, usize, usize),
    ) -> Result<(), UserFault> {
        use crate::mm::paging::USER_END;
        let end = virt
            .checked_add(len as u64)
            .filter(|&e| e <= USER_END)
            .ok_or(UserFault { address: virt })?;
        let mut addr = virt;
        while addr < end {
            let t = self.mapper.translate(addr).ok_or(UserFault { address: addr })?;
            if !t.flags.user || (write && !t.flags.writable) {
                return Err(UserFault { address: addr });
            }
            let page_left = PAGE_SIZE - (addr % PAGE_SIZE);
            let n = page_left.min(end - addr) as usize;
            f(t.phys, n, (addr - virt) as usize);
            addr += n as u64;
        }
        Ok(())
    }

    /// Copies from user memory (as a user-mode read would).
    pub fn read(&mut self, virt: u64, buf: &mut [u8]) -> Result<(), UserFault> {
        let dst = buf.as_mut_ptr();
        self.for_each_piece(virt, buf.len(), false, |phys, n, off| {
            // SAFETY: `phys..phys+n` is a mapped user frame owned by this
            // space (HHDM-mapped); `dst+off..+n` is inside `buf`. Raw copies,
            // since user threads may write the frame concurrently.
            unsafe { core::ptr::copy_nonoverlapping(hhdm_ptr(phys), dst.add(off), n) }
        })
    }

    /// Copies into user memory, honouring page writability (as a user-mode
    /// write would).
    pub fn write(&mut self, virt: u64, data: &[u8]) -> Result<(), UserFault> {
        self.copy_in(virt, data, true)
    }

    /// Copies into user memory ignoring writability (for the loader, e.g.
    /// to fill read-only code pages).
    pub fn write_initial(&mut self, virt: u64, data: &[u8]) -> Result<(), UserFault> {
        self.copy_in(virt, data, false)
    }

    fn copy_in(&mut self, virt: u64, data: &[u8], check_writable: bool) -> Result<(), UserFault> {
        let src = data.as_ptr();
        self.for_each_piece(virt, data.len(), check_writable, |phys, n, off| {
            // SAFETY: as in `read`, with source and destination swapped.
            unsafe { core::ptr::copy_nonoverlapping(src.add(off), hhdm_ptr(phys), n) }
        })
    }
}

impl UserSpace {
    /// Maps `page` at `virt` with `flags` (user forced); the frame stays
    /// owned by the `SharedPage`.
    pub fn map_shared(
        &mut self,
        virt: u64,
        page: &SharedPage,
        flags: PageFlags,
    ) -> Result<(), PagingError> {
        self.map_borrowed(virt, page.frame(), flags)
    }

    /// Maps a frame owned elsewhere (section pages, shared pages) at
    /// `virt`; it is never freed by this space.
    pub fn map_borrowed(
        &mut self,
        virt: u64,
        frame: Frame,
        mut flags: PageFlags,
    ) -> Result<(), PagingError> {
        use crate::mm::paging::PageSize;
        flags.user = true;
        flags.global = false;
        flags.borrowed = true;
        self.mapper.map(
            virt / PAGE_SIZE * PAGE_SIZE,
            frame.start(),
            PageSize::Size4K,
            flags,
            &mut alloc_table,
        )?;
        Ok(())
    }

    /// The frame mapped at `virt` and whether it is borrowed.
    pub fn mapping(&mut self, virt: u64) -> Option<(Frame, bool)> {
        let t = self.mapper.translate(virt)?;
        Some((Frame::containing(t.phys), t.flags.borrowed))
    }

    /// Maps at `virt` a new private frame holding a copy of `source`
    /// (copy-on-write), replacing what was mapped there (an owned frame
    /// there is freed after the copy, a borrowed one is left alone).
    pub fn map_private_copy(
        &mut self,
        virt: u64,
        source: Frame,
        mut flags: PageFlags,
    ) -> Result<(), PagingError> {
        use crate::mm::paging::PageSize;
        let frame = crate::mm::allocate_frame().ok_or(MapError::OutOfMemory)?;
        // SAFETY: `frame` is freshly allocated and `source` is a mapped
        // user frame; both are distinct 4 KiB frames in the HHDM.
        unsafe {
            core::ptr::copy_nonoverlapping(
                hhdm_ptr(source.start()),
                hhdm_ptr(frame.start()),
                PAGE_SIZE as usize,
            );
        }
        self.unmap_page(virt);
        flags.user = true;
        flags.global = false;
        flags.borrowed = false;
        let virt = virt / PAGE_SIZE * PAGE_SIZE;
        if let Err(e) =
            self.mapper.map(virt, frame.start(), PageSize::Size4K, flags, &mut alloc_table)
        {
            let _ = crate::mm::free_frame(frame);
            return Err(e.into());
        }
        invlpg(virt);
        Ok(())
    }

    /// Allocates a frame holding `data` (at most a page), zero-filled.
    pub fn frame_with(data: &[u8]) -> Option<Frame> {
        let frame = Self::zeroed_frame()?;
        let n = data.len().min(PAGE_SIZE as usize);
        // SAFETY: a freshly allocated frame, HHDM-mapped, owned by the caller.
        unsafe { core::ptr::copy_nonoverlapping(data.as_ptr(), hhdm_ptr(frame.start()), n) };
        Some(frame)
    }

    /// Allocates a zeroed frame (for section pages backed on demand).
    pub fn zeroed_frame() -> Option<Frame> {
        let frame = crate::mm::allocate_frame()?;
        // SAFETY: a freshly allocated frame, HHDM-mapped, owned by the caller.
        unsafe { hhdm_ptr(frame.start()).write_bytes(0, PAGE_SIZE as usize) };
        Some(frame)
    }
}

impl Drop for UserSpace {
    fn drop(&mut self) {
        // The space must not be active on any CPU any more (see `activate`).
        let mem = HhdmTables { offset: crate::mm::hhdm_offset() };
        let dummy = Frame::from_number(0);
        let mapper = core::mem::replace(&mut self.mapper, Mapper::from_root(mem, dummy));
        let free = &mut |f: Frame| {
            let _ = crate::mm::free_frame(f);
        };
        mapper.destroy(&mut |f| free(f), &mut |f| free(f));
    }
}

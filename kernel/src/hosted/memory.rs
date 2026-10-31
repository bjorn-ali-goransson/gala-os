//! gala-um's memory: "physical memory" is one memfd. Frames are its pages
//! (handed out by the kernel's own frame allocator), the HHDM is one
//! shared view of the whole memfd, and a user page is that frame mapped
//! with `mmap(MAP_SHARED | MAP_FIXED)` at its user address, so the
//! kernel's model (owned and borrowed frames, sections sharing frames,
//! copy-on-write by copying a frame) carries over as it is.
//!
//! The user address space is the process's own lower half: gala-um runs
//! one NT process per Linux process.

use alloc::collections::{BTreeMap, BTreeSet};
use core::sync::atomic::{AtomicI32, Ordering};

use crate::arch::paging::{PagingError, UserFault};
use crate::mm::frame::Frame;
use crate::mm::paging::{MapError, PageFlags, USER_END};
use crate::mm::{self, PAGE_SIZE};
use crate::sync::{IrqGuard, IrqMutex};

/// Bytes of "physical memory" (sparse: the host only backs touched pages).
pub const MEMORY_SIZE: u64 = 1 << 30;

static MEMFD: AtomicI32 = AtomicI32::new(-1);

/// Creates the memfd, maps it as the HHDM and starts the frame allocator.
pub fn init() -> Result<(), &'static str> {
    // SAFETY: plain system calls; the view is never unmapped.
    unsafe {
        let fd = libc::memfd_create(c"gala-um-memory".as_ptr(), libc::MFD_CLOEXEC);
        if fd < 0 || libc::ftruncate(fd, MEMORY_SIZE as libc::off_t) != 0 {
            return Err("memfd");
        }
        let view = libc::mmap(
            core::ptr::null_mut(),
            MEMORY_SIZE as usize,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED | libc::MAP_NORESERVE,
            fd,
            0,
        );
        if view == libc::MAP_FAILED {
            return Err("mmap of the memfd");
        }
        MEMFD.store(fd, Ordering::Relaxed);
        let words = crate::mm::frame::FrameAllocator::words_for(MEMORY_SIZE);
        let bitmap = alloc::vec![0u64; words].leak();
        mm::init_hosted(view as u64, MEMORY_SIZE, bitmap);
    }
    Ok(())
}

/// The global allocator of gala-um's binary: page-sized, page-aligned
/// allocations (ramfs file pages, which sections map into programs) are
/// memfd frames, as the kernel heap's page allocations are frames in the
/// HHDM; everything else comes from the host's allocator.
pub struct Allocator;

fn in_view(addr: u64) -> bool {
    let base = mm::hhdm_offset();
    base != 0 && (base..base + MEMORY_SIZE).contains(&addr)
}

// SAFETY: frames are exclusively owned page-sized, page-aligned memory
// while allocated; the rest is the system allocator's contract.
unsafe impl core::alloc::GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: core::alloc::Layout) -> *mut u8 {
        if layout.size() == PAGE_SIZE as usize
            && layout.align() == PAGE_SIZE as usize
            && let Some(frame) = mm::allocate_frame()
        {
            return hhdm_ptr(frame);
        }
        // SAFETY: forwarded.
        unsafe { std::alloc::System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: core::alloc::Layout) {
        if in_view(ptr as u64) {
            let _ = mm::free_frame(Frame::containing(ptr as u64 - mm::hhdm_offset()));
        } else {
            // SAFETY: forwarded; it came from the system allocator.
            unsafe { std::alloc::System.dealloc(ptr, layout) }
        }
    }
}

/// The frame of a heap page from `Allocator`'s frame path.
pub fn frame_of_heap_page(addr: usize) -> Option<Frame> {
    in_view(addr as u64).then(|| Frame::containing(addr as u64 - mm::hhdm_offset()))
}

fn hhdm_ptr(frame: Frame) -> *mut u8 {
    (mm::hhdm_offset() + frame.start()) as *mut u8
}

fn zeroed_frame() -> Option<Frame> {
    let frame = mm::allocate_frame()?;
    // SAFETY: a freshly allocated frame of the memfd view, owned by us.
    unsafe { hhdm_ptr(frame).write_bytes(0, PAGE_SIZE as usize) };
    Some(frame)
}

fn prot(flags: PageFlags) -> i32 {
    if !flags.user {
        return libc::PROT_NONE; // kernel-only: inaccessible to the program
    }
    let mut p = libc::PROT_READ;
    if flags.writable {
        p |= libc::PROT_WRITE;
    }
    if flags.executable {
        p |= libc::PROT_EXEC;
    }
    p
}

fn map_err() -> PagingError {
    PagingError::Map(MapError::OutOfMemory)
}

/// Maps `frame` at `virt` (one page). `ours`: gala-um placed something
/// there before and may replace it; otherwise the page must be free, so a
/// host mapping (the PEB area shares the 0x7f... band with the host's
/// libraries) is never replaced.
fn mmap_frame(virt: u64, frame: Frame, flags: PageFlags, ours: bool) -> Result<(), PagingError> {
    let fixed = if ours { libc::MAP_FIXED } else { libc::MAP_FIXED_NOREPLACE };
    // SAFETY: replaces at most a page gala-um mapped itself (see above).
    let p = unsafe {
        libc::mmap(
            virt as *mut libc::c_void,
            PAGE_SIZE as usize,
            prot(flags),
            libc::MAP_SHARED | fixed,
            MEMFD.load(Ordering::Relaxed),
            frame.start() as libc::off_t,
        )
    };
    if p == libc::MAP_FAILED || p as u64 != virt {
        crate::warn!("gala-um: cannot map {virt:#x}: {}", std::io::Error::last_os_error());
        return Err(PagingError::Map(MapError::AlreadyMapped));
    }
    Ok(())
}

/// Leaves `virt` reserved but inaccessible (so the host never places its
/// own mappings there).
fn unmap(virt: u64) {
    // SAFETY: a page gala-um mapped (callers check).
    unsafe {
        libc::mmap(
            virt as *mut libc::c_void,
            PAGE_SIZE as usize,
            libc::PROT_NONE,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_FIXED | libc::MAP_NORESERVE,
            -1,
            0,
        );
    }
}

fn check_user(virt: u64) -> Result<u64, PagingError> {
    let page = virt / PAGE_SIZE * PAGE_SIZE;
    if page >= USER_END {
        return Err(PagingError::Map(MapError::BadAddress));
    }
    Ok(page)
}

struct Page {
    frame: Frame,
    flags: PageFlags,
    borrowed: bool,
}

/// The program's address space: the pages gala-um has mapped there.
pub struct UserSpace {
    pages: BTreeMap<u64, Page>,
    /// Every page gala-um has placed a mapping (or a placeholder) at.
    placed: BTreeSet<u64>,
}

/// A user address space shared by the threads of a process.
pub type SharedUserSpace = crate::mm::heap::KArc<SpaceLock>;

pub struct SpaceLock(IrqMutex<UserSpace>);

impl SpaceLock {
    pub fn new(space: UserSpace) -> Self {
        SpaceLock(IrqMutex::new(space))
    }

    pub fn lock(&self) -> IrqGuard<'_, UserSpace> {
        self.0.lock()
    }
}

impl UserSpace {
    pub fn new() -> Result<Self, PagingError> {
        Ok(Self { pages: BTreeMap::new(), placed: BTreeSet::new() })
    }

    /// Kernel API compatibility: gala-um has no page tables.
    pub fn root(&self) -> u64 {
        0
    }

    fn insert(&mut self, page: u64, entry: Page) -> Result<(), PagingError> {
        mmap_frame(page, entry.frame, entry.flags, self.placed.contains(&page))?;
        self.placed.insert(page);
        if let Some(old) = self.pages.insert(page, entry)
            && !old.borrowed
        {
            let _ = mm::free_frame(old.frame);
        }
        Ok(())
    }

    pub fn map_zero_page(&mut self, virt: u64, flags: PageFlags) -> Result<(), PagingError> {
        self.map_zeroed(virt / PAGE_SIZE * PAGE_SIZE, 1, flags)
    }

    pub fn is_mapped(&mut self, virt: u64) -> bool {
        self.pages.contains_key(&(virt / PAGE_SIZE * PAGE_SIZE))
    }

    pub fn set_flags(&mut self, virt: u64, flags: PageFlags) -> Result<(), PagingError> {
        let page = virt / PAGE_SIZE * PAGE_SIZE;
        let entry = self.pages.get_mut(&page).ok_or(PagingError::Map(MapError::BadAddress))?;
        entry.flags = PageFlags { borrowed: entry.borrowed, ..flags };
        // SAFETY: a page of the program's half that gala-um mapped.
        let r =
            unsafe { libc::mprotect(page as *mut libc::c_void, PAGE_SIZE as usize, prot(flags)) };
        if r != 0 { Err(map_err()) } else { Ok(()) }
    }

    pub fn unmap_page(&mut self, virt: u64) -> bool {
        let page = virt / PAGE_SIZE * PAGE_SIZE;
        match self.pages.remove(&page) {
            Some(old) => {
                unmap(page);
                if !old.borrowed {
                    let _ = mm::free_frame(old.frame);
                }
                true
            }
            None => false,
        }
    }

    pub fn map_zeroed(
        &mut self,
        virt: u64,
        pages: u64,
        mut flags: PageFlags,
    ) -> Result<(), PagingError> {
        flags.user = true;
        if !virt.is_multiple_of(PAGE_SIZE) {
            return Err(PagingError::Map(MapError::BadAddress));
        }
        for i in 0..pages {
            let page = check_user(virt + i * PAGE_SIZE)?;
            let frame = zeroed_frame().ok_or_else(map_err)?;
            if let Err(e) = self.insert(page, Page { frame, flags, borrowed: false }) {
                let _ = mm::free_frame(frame);
                return Err(e);
            }
        }
        Ok(())
    }

    pub fn map_shared(
        &mut self,
        virt: u64,
        page: &SharedPage,
        flags: PageFlags,
    ) -> Result<(), PagingError> {
        self.map_borrowed(virt, page.frame(), flags)
    }

    pub fn map_borrowed(
        &mut self,
        virt: u64,
        frame: Frame,
        mut flags: PageFlags,
    ) -> Result<(), PagingError> {
        flags.user = true;
        flags.borrowed = true;
        let page = check_user(virt)?;
        self.insert(page, Page { frame, flags, borrowed: true })
    }

    pub fn mapping(&mut self, virt: u64) -> Option<(Frame, bool)> {
        self.pages.get(&(virt / PAGE_SIZE * PAGE_SIZE)).map(|p| (p.frame, p.borrowed))
    }

    pub fn map_private_copy(
        &mut self,
        virt: u64,
        source: Frame,
        mut flags: PageFlags,
    ) -> Result<(), PagingError> {
        let page = check_user(virt)?;
        let frame = mm::allocate_frame().ok_or_else(map_err)?;
        // SAFETY: two distinct frames of the memfd view.
        unsafe {
            core::ptr::copy_nonoverlapping(hhdm_ptr(source), hhdm_ptr(frame), PAGE_SIZE as usize)
        };
        flags.user = true;
        flags.borrowed = false;
        if let Err(e) = self.insert(page, Page { frame, flags, borrowed: false }) {
            let _ = mm::free_frame(frame);
            return Err(e);
        }
        Ok(())
    }

    /// Calls `f(frame pointer, len, offset)` for each page piece of the
    /// user range, checking user access (and writability if `write`).
    fn for_each_piece(
        &self,
        virt: u64,
        len: usize,
        write: bool,
        mut f: impl FnMut(*mut u8, usize, usize),
    ) -> Result<(), UserFault> {
        let end = virt
            .checked_add(len as u64)
            .filter(|&e| e <= USER_END)
            .ok_or(UserFault { address: virt })?;
        let mut addr = virt;
        while addr < end {
            let page = addr / PAGE_SIZE * PAGE_SIZE;
            let p = self.pages.get(&page).ok_or(UserFault { address: addr })?;
            if !p.flags.user || (write && !p.flags.writable) {
                return Err(UserFault { address: addr });
            }
            let n = (PAGE_SIZE - (addr - page)).min(end - addr) as usize;
            // SAFETY: inside the frame's page of the memfd view.
            let ptr = unsafe { hhdm_ptr(p.frame).add((addr - page) as usize) };
            f(ptr, n, (addr - virt) as usize);
            addr += n as u64;
        }
        Ok(())
    }

    pub fn read(&mut self, virt: u64, buf: &mut [u8]) -> Result<(), UserFault> {
        let dst = buf.as_mut_ptr();
        // SAFETY: `ptr..+n` is in a mapped frame, `dst+off..+n` in `buf`.
        self.for_each_piece(virt, buf.len(), false, |ptr, n, off| unsafe {
            core::ptr::copy_nonoverlapping(ptr, dst.add(off), n)
        })
    }

    pub fn write(&mut self, virt: u64, data: &[u8]) -> Result<(), UserFault> {
        self.copy_in(virt, data, true)
    }

    pub fn write_initial(&mut self, virt: u64, data: &[u8]) -> Result<(), UserFault> {
        self.copy_in(virt, data, false)
    }

    fn copy_in(&mut self, virt: u64, data: &[u8], check_writable: bool) -> Result<(), UserFault> {
        let src = data.as_ptr();
        // SAFETY: as in `read`, with source and destination swapped.
        self.for_each_piece(virt, data.len(), check_writable, |ptr, n, off| unsafe {
            core::ptr::copy_nonoverlapping(src.add(off), ptr, n)
        })
    }

    pub fn frame_with(data: &[u8]) -> Option<Frame> {
        let frame = zeroed_frame()?;
        let n = data.len().min(PAGE_SIZE as usize);
        // SAFETY: a fresh frame of the memfd view, owned by the caller.
        unsafe { core::ptr::copy_nonoverlapping(data.as_ptr(), hhdm_ptr(frame), n) };
        Some(frame)
    }

    pub fn zeroed_frame() -> Option<Frame> {
        zeroed_frame()
    }
}

impl Drop for UserSpace {
    fn drop(&mut self) {
        for (_, p) in core::mem::take(&mut self.pages) {
            if !p.borrowed {
                let _ = mm::free_frame(p.frame);
            }
        }
        for page in core::mem::take(&mut self.placed) {
            // SAFETY: a page gala-um mapped.
            unsafe { libc::munmap(page as *mut libc::c_void, PAGE_SIZE as usize) };
        }
    }
}

/// A frame the NT layer writes and every process maps (KUSER_SHARED_DATA).
pub struct SharedPage {
    frame: Frame,
}

impl SharedPage {
    pub fn new() -> Result<Self, PagingError> {
        Ok(Self { frame: zeroed_frame().ok_or_else(map_err)? })
    }

    pub fn frame(&self) -> Frame {
        self.frame
    }

    pub fn write(&self, offset: usize, bytes: &[u8]) {
        if offset.checked_add(bytes.len()).is_none_or(|end| end > PAGE_SIZE as usize) {
            return;
        }
        for (i, &b) in bytes.iter().enumerate() {
            // SAFETY: in bounds of the owned frame; volatile because the
            // program reads it concurrently.
            unsafe { hhdm_ptr(self.frame).add(offset + i).write_volatile(b) };
        }
    }

    pub fn write_u32(&self, offset: usize, value: u32) {
        if !offset.is_multiple_of(4) || offset + 4 > PAGE_SIZE as usize {
            return;
        }
        // SAFETY: aligned and in bounds; a single volatile store.
        unsafe { hhdm_ptr(self.frame).add(offset).cast::<u32>().write_volatile(value) };
    }
}

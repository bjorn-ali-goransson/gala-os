//! User address spaces, for every architecture: a `UserSpace` owns its
//! user half's page tables (the architecture's `Mapper` encoding) and
//! every frame mapped there; `SpaceLock` is the lock its process's
//! threads share, whose release completes the mapping changes made under
//! it (other CPUs' stale translations dropped, then the frames those
//! changes unmapped freed).
//!
//! The architecture's part (`paging`, `smp`):
//! - `paging::user_mapper`: a new space's tables (x86-64: one root with
//!   the kernel's half shared in; aarch64: a TTBR0 root holding the user
//!   half alone, the kernel's being TTBR1's);
//! - `paging::invalidate_user_page`: a changed page's TLB entry dropped
//!   under the lock (x86-64: this CPU's, `invlpg`; aarch64: every CPU's,
//!   a broadcast TLBI);
//! - `smp::flush_user_space`: the other CPUs', at the lock's release
//!   (x86-64: the shootdown; aarch64: nothing left to do).

use core::sync::atomic::{AtomicU64, Ordering};

use super::paging::{
    PagingError, UserMapper, alloc_table, hhdm_tables, invalidate_user_page, user_mapper,
};
use crate::mm::PAGE_SIZE;
use crate::mm::frame::Frame;
use crate::mm::heap::TryVec;
use crate::mm::paging::{MapError, Mapper, PageFlags};
use crate::sync::IrqMutex;

/// Frames kept forever because their deferred free could not be
/// recorded (no memory for the list).
static LEAKED_FRAMES: AtomicU64 = AtomicU64::new(0);

pub fn leaked_frames() -> u64 {
    LEAKED_FRAMES.load(Ordering::Relaxed)
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
        let changed_on = guard.changed_on.take();
        let frames = core::mem::take(&mut guard.freed);
        let root = guard.root();
        drop(guard);
        if dirty {
            // The thread may be on another CPU by now (an interrupt held
            // back while the lock was held is taken at its release), one
            // whose entries were not invalidated (see `flush_user_space`).
            crate::arch::smp::flush_user_space(root, changed_on);
        }
        for frame in frames {
            let _ = crate::mm::free_frame(frame);
        }
    }
}

/// A user address space: its user half's page tables (`paging::user_mapper`)
/// and every frame mapped there.
pub struct UserSpace {
    mapper: UserMapper,
    /// Mappings were changed or removed since the lock was taken: other
    /// CPUs must be flushed when it is released.
    dirty: bool,
    /// Frames unmapped under the lock, freed once other CPUs are flushed.
    freed: alloc::vec::Vec<Frame>,
    /// The CPU that invalidated its own entries for the changes
    /// (`invalidate_user_page` under the lock, with interrupts off: one
    /// CPU per hold).
    changed_on: Option<u32>,
}

/// A kernel-owned page that is also mapped into user address spaces
/// (e.g. KUSER_SHARED_DATA). The kernel writes it through the HHDM with
/// volatile stores, since user threads read it concurrently.
pub struct SharedPage {
    frame: Frame,
}

impl SharedPage {
    pub fn new() -> Result<Self, PagingError> {
        let frame = crate::mm::allocate_frame(crate::mm::Use::SectionPage)
            .ok_or(PagingError::Map(MapError::OutOfMemory))?;
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

impl UserSpace {
    /// Drops this CPU's entry for `virt` after a change, and records that
    /// the CPUs with the space loaded must be flushed when the lock goes.
    fn invalidate(&mut self, virt: u64) {
        invalidate_user_page(virt);
        self.dirty = true;
        self.changed_on = Some(crate::arch::percpu::cpu_index());
    }

    /// Maps one fresh zeroed page at `virt` (demand-zero fault, commit).
    pub fn map_zero_page(&mut self, virt: u64, flags: PageFlags) -> Result<(), PagingError> {
        self.map_zeroed(virt / PAGE_SIZE * PAGE_SIZE, 1, flags)
    }

    /// Whether `virt` has a page-table mapping (of any kind).
    pub fn is_mapped(&mut self, virt: u64) -> bool {
        self.mapper.translate(virt).is_some()
    }

    /// The first mapped page at or after `virt` and below `end` (absent
    /// page tables skipped whole).
    pub fn next_mapped(&mut self, virt: u64, end: u64) -> Option<u64> {
        self.mapper.next_mapped(virt / PAGE_SIZE * PAGE_SIZE, end)
    }

    /// Changes the flags of a mapped page; `user` is not forced, so a
    /// kernel-only flags value makes the page inaccessible to user mode.
    /// An owned frame another space shares (a fork's copy-on-write page)
    /// stays read-only: only `make_writable` makes it writable, by
    /// copying it.
    pub fn set_flags(&mut self, virt: u64, mut flags: PageFlags) -> Result<(), PagingError> {
        let virt = virt / PAGE_SIZE * PAGE_SIZE;
        flags.global = false;
        if flags.writable
            && let Some(t) = self.mapper.translate(virt)
            && !t.flags.borrowed
            && crate::mm::frame_shared(Frame::containing(t.phys))
        {
            flags.writable = false;
        }
        self.mapper.update_flags_4k(virt, flags)?;
        self.invalidate(virt);
        Ok(())
    }

    /// Gives the page at `virt` `flags`, writable (a write the VAD
    /// allows): an owned frame another space shares (a fork's
    /// copy-on-write page) is copied into a frame of this space's own
    /// first, and the shared one given up (its last owner keeps it).
    /// Returns whether it was copied.
    pub fn make_writable(&mut self, virt: u64, flags: PageFlags) -> Result<bool, PagingError> {
        match self.mapping(virt) {
            Some((frame, false)) if crate::mm::frame_shared(frame) => {
                self.map_private_copy(virt, frame, flags)?;
                crate::mm::COW_COPIES.fetch_add(1, Ordering::Relaxed);
                Ok(true)
            }
            _ => self.set_flags(virt, flags).map(|()| false),
        }
    }

    /// Gives `child` (a fork's new space) the page mapped at `virt`: a
    /// borrowed frame is mapped there too; an owned one gets the child as
    /// another owner and is read-only in both until a write copies it
    /// (`make_writable`), or is copied now if it cannot have another
    /// owner. On an error the child has nothing there (and the frame no
    /// extra owner).
    pub fn share_into(
        &mut self,
        child: &mut UserSpace,
        virt: u64,
    ) -> Result<crate::mm::paging::ForkedPage, PagingError> {
        use crate::mm::paging::{ForkedPage, PageSize};
        let virt = virt / PAGE_SIZE * PAGE_SIZE;
        let Some(t) = self.mapper.translate(virt) else {
            // Only NT processes' pages go to the page file; a fork's parent
            // is a Linux process. Refused rather than lost.
            if self.mapper.swapped_4k(virt).is_some() {
                return Err(PagingError::Map(MapError::AlreadyMapped));
            }
            return Ok(ForkedPage::Absent);
        };
        let frame = Frame::containing(t.phys);
        if t.flags.borrowed {
            child.map_borrowed(virt, frame, t.flags)?;
            return Ok(ForkedPage::Borrowed);
        }
        if !crate::mm::share_frame(frame) {
            child.map_private_copy(virt, frame, t.flags)?;
            return Ok(ForkedPage::Copied);
        }
        let read_only = PageFlags { writable: false, global: false, ..t.flags };
        if let Err(e) =
            child.mapper.map(virt, frame.start(), PageSize::Size4K, read_only, &mut alloc_table)
        {
            // The child's share given back.
            let _ = crate::mm::free_frame(frame);
            return Err(e.into());
        }
        crate::mm::note_user_map(frame, true);
        if t.flags.writable {
            // A flags change of a mapped leaf allocates nothing.
            let _ = self.mapper.update_flags_4k(virt, read_only);
            self.invalidate(virt);
        }
        Ok(ForkedPage::CopyOnWrite)
    }

    /// Unmaps a page and frees its frame (a page in the page file: gives
    /// its slot back). Returns whether a frame was mapped.
    pub fn unmap_page(&mut self, virt: u64) -> bool {
        let virt = virt / PAGE_SIZE * PAGE_SIZE;
        if let Some(token) = self.mapper.clear_swapped_4k(virt) {
            crate::mm::swap::release(token);
            return false;
        }
        match self.mapper.unmap_4k_owned(virt) {
            Some((frame, owned)) => {
                crate::mm::note_user_map(frame, false);
                self.invalidate(virt);
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
        let mapper = user_mapper()?;
        Ok(Self { mapper, dirty: false, freed: alloc::vec::Vec::new(), changed_on: None })
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
            let frame = crate::mm::allocate_frame(crate::mm::Use::UserPage)
                .ok_or(PagingError::Map(MapError::OutOfMemory))?;
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
            crate::mm::note_user_map(frame, true);
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
        })?;
        // Written through the HHDM, not the user mapping: the processor
        // set no dirty bit, which a page out in progress tests
        // (`page_out`).
        // Code written this way (a loader's, WriteProcessMemory's) reaches
        // instruction fetch where that needs maintenance (aarch64).
        let mut page = virt / PAGE_SIZE * PAGE_SIZE;
        while page < virt + data.len() as u64 {
            self.mapper.set_dirty_4k(page, true);
            self.mapper.sync_written_code(page);
            page += PAGE_SIZE;
        }
        Ok(())
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
        crate::mm::note_user_map(frame, true);
        Ok(())
    }

    /// Maps `frame`, which the caller hands over (its contents kept), at
    /// `virt` as an owned page of this space: a kernel handover gives a
    /// process its own pages back in the new kernel.
    pub fn map_adopted(
        &mut self,
        virt: u64,
        frame: Frame,
        mut flags: PageFlags,
    ) -> Result<(), PagingError> {
        use crate::mm::paging::PageSize;
        flags.global = false;
        flags.borrowed = false;
        self.mapper.map(
            virt / PAGE_SIZE * PAGE_SIZE,
            frame.start(),
            PageSize::Size4K,
            flags,
            &mut alloc_table,
        )?;
        crate::mm::note_user_map(frame, true);
        Ok(())
    }

    /// Every user mapping: `f(virt, frame, flags)` (flags.borrowed: the
    /// frame belongs to a section, an image or a shared page).
    pub fn for_each_page(&mut self, mut f: impl FnMut(u64, Frame, PageFlags)) {
        self.mapper.for_each_user_leaf(&mut |virt, t| f(virt, Frame::containing(t.phys), t.flags));
    }

    /// Moves the page mapped at `from` to `to` (which must be unmapped),
    /// its frame and flags with it, owned or borrowed as it was: memory
    /// moved without a copy (a Linux mremap). Returns whether `from` was
    /// mapped; on an error (no memory for a page table at `to`) nothing
    /// changed.
    pub fn move_page(&mut self, from: u64, to: u64) -> Result<bool, PagingError> {
        use crate::mm::paging::PageSize;
        let (from, to) = (from / PAGE_SIZE * PAGE_SIZE, to / PAGE_SIZE * PAGE_SIZE);
        let Some(t) = self.mapper.translate(from) else { return Ok(false) };
        let frame = Frame::containing(t.phys);
        self.mapper.map(to, frame.start(), PageSize::Size4K, t.flags, &mut alloc_table)?;
        crate::mm::note_user_map(frame, true);
        if self.mapper.unmap_4k_owned(from).is_some() {
            crate::mm::note_user_map(frame, false);
        }
        self.invalidate(from);
        Ok(true)
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
        let frame =
            crate::mm::allocate_frame(crate::mm::Use::UserPage).ok_or(MapError::OutOfMemory)?;
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
        crate::mm::note_user_map(frame, true);
        self.invalidate(virt);
        Ok(())
    }

    /// The page file's look at the page at `virt` (`nt::pagefile`'s
    /// clock): a present page of this space's own, shared with no other,
    /// that was accessed since the last look loses its accessed bit
    /// (`Recent`: a second chance); one that was not is `Old` and loses
    /// its dirty bit, so that a write from now on shows (`page_out`).
    pub fn age_page(&mut self, virt: u64) -> crate::mm::paging::Age {
        use crate::mm::paging::Age;
        let virt = virt / PAGE_SIZE * PAGE_SIZE;
        let Some(t) = self.mapper.translate(virt) else { return Age::Absent };
        let frame = Frame::containing(t.phys);
        if t.flags.borrowed || t.size != crate::mm::paging::PageSize::Size4K {
            return Age::Kept;
        }
        if crate::mm::frame_shared(frame) {
            return Age::Kept;
        }
        let recent = self.mapper.take_accessed_4k(virt) == Some(true);
        if !recent {
            self.mapper.set_dirty_4k(virt, false);
        }
        self.invalidate(virt);
        if recent { Age::Recent } else { Age::Old(frame) }
    }

    /// Copies the page at `virt` into `buf` (4 KiB) if it still maps
    /// `frame` as its own (the page out's copy, taken after the flush
    /// that `age_page`'s change asked for).
    pub fn copy_page(&mut self, virt: u64, frame: Frame, buf: &mut [u8]) -> bool {
        match self.mapping(virt) {
            Some((f, false)) if f == frame && buf.len() >= PAGE_SIZE as usize => {
                // SAFETY: `frame` is a user frame this space owns and maps
                // (HHDM-mapped), `buf` holds a page. A raw copy: user
                // threads may write it meanwhile (the dirty bit tells).
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        hhdm_ptr(frame.start()),
                        buf.as_mut_ptr(),
                        PAGE_SIZE as usize,
                    )
                };
                true
            }
            _ => false,
        }
    }

    /// Room to free `n` frames under this lock without allocating
    /// (`page_out`'s); false without the memory for it.
    pub fn reserve_frees(&mut self, n: usize) -> bool {
        self.freed.try_reserve(n).is_ok()
    }

    /// Puts the page at `virt` in the page file: if it still maps `frame`
    /// as its own and was not written since `age_page` (its dirty bit),
    /// the leaf becomes `token` and the frame is freed (after the TLB
    /// flush). False (nothing changed) otherwise: the caller gives the
    /// slot back. Needs `reserve_frees` first.
    pub fn page_out(&mut self, virt: u64, frame: Frame, token: u64) -> bool {
        let virt = virt / PAGE_SIZE * PAGE_SIZE;
        if self.mapping(virt) != Some((frame, false))
            || self.mapper.set_dirty_4k(virt, false) != Some(false)
            || crate::mm::frame_shared(frame)
            || self.freed.len() == self.freed.capacity()
        {
            return false;
        }
        self.mapper.swap_out_4k(virt, token);
        crate::mm::note_user_map(frame, false);
        self.invalidate(virt);
        let _ = self.freed.try_push(frame);
        true
    }

    /// The page file token of the page at `virt`, if it is in the page
    /// file.
    pub fn swapped(&mut self, virt: u64) -> Option<u64> {
        self.mapper.swapped_4k(virt / PAGE_SIZE * PAGE_SIZE)
    }

    /// Brings the page at `virt` back from the page file: if its leaf
    /// still holds `token`, a new frame of this space's own holding
    /// `data` (4 KiB) is mapped there with `flags`, accessed and dirty
    /// (its slot goes back: the caller releases it). Ok(false) if the
    /// leaf changed meanwhile.
    pub fn page_in(
        &mut self,
        virt: u64,
        token: u64,
        data: &[u8],
        mut flags: PageFlags,
    ) -> Result<bool, PagingError> {
        use crate::mm::paging::PageSize;
        let virt = virt / PAGE_SIZE * PAGE_SIZE;
        if self.mapper.swapped_4k(virt) != Some(token) || data.len() < PAGE_SIZE as usize {
            return Ok(false);
        }
        let frame =
            crate::mm::allocate_frame(crate::mm::Use::UserPage).ok_or(MapError::OutOfMemory)?;
        // SAFETY: a freshly allocated frame, HHDM-mapped, owned by us;
        // `data` holds a page.
        unsafe {
            core::ptr::copy_nonoverlapping(
                data.as_ptr(),
                hhdm_ptr(frame.start()),
                PAGE_SIZE as usize,
            )
        };
        flags.global = false;
        flags.borrowed = false;
        // The map replaces the token (a SWAPPED leaf is not present); on
        // an error the token stays.
        if let Err(e) =
            self.mapper.map(virt, frame.start(), PageSize::Size4K, flags, &mut alloc_table)
        {
            let _ = crate::mm::free_frame(frame);
            return Err(e.into());
        }
        // The slot is given back: the page exists only here now.
        self.mapper.set_dirty_4k(virt, true);
        crate::mm::note_user_map(frame, true);
        Ok(true)
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
        let frame = crate::mm::allocate_frame(crate::mm::Use::SectionPage)?;
        // SAFETY: a freshly allocated frame, HHDM-mapped, owned by the caller.
        unsafe { hhdm_ptr(frame.start()).write_bytes(0, PAGE_SIZE as usize) };
        Some(frame)
    }
}

impl Drop for UserSpace {
    fn drop(&mut self) {
        // The space must not be active on any CPU any more (see `activate`).
        if crate::mm::FRAME_CHECK.load(Ordering::Relaxed) {
            self.mapper.for_each_user_leaf(&mut |_, t| {
                crate::mm::note_user_map(Frame::containing(t.phys), false);
            });
        }
        let dummy = Frame::from_number(0);
        let mapper = core::mem::replace(&mut self.mapper, Mapper::from_root(hhdm_tables(), dummy));
        let free = &mut |f: Frame| {
            let _ = crate::mm::free_frame(f);
        };
        mapper.destroy(&mut |f| free(f), &mut |f| free(f), &mut crate::mm::swap::release);
    }
}

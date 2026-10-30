//! Section objects (pagefile-backed so far) and their views.
//!
//! A section owns its frames, allocated zeroed on first access through any
//! view; views map them into address spaces as borrowed pages, so unmapping
//! a view or tearing down a process never frees them. Frames are freed with
//! the section, i.e. when its last handle and last view are gone.

use alloc::vec::Vec;

use super::object::ObjectRef;
use super::process::Process;
use super::status::NtStatus;
use super::vad::{self, Access};
use crate::arch::paging::UserSpace;
use crate::mm::PAGE_SIZE;
use crate::mm::frame::Frame;
use crate::mm::heap::TryVec;
use crate::sync::IrqMutex;

pub const SEC_RESERVE: u32 = 0x400_0000;
pub const SEC_COMMIT: u32 = 0x800_0000;
pub const SEC_IMAGE: u32 = 0x100_0000;

pub struct Section {
    pub size: u64,
    /// SectionPageProtection: the most a view may be granted.
    pub protect: u32,
    pub attributes: u32,
    pages: IrqMutex<Vec<Option<Frame>>>,
}

impl Section {
    pub fn new(size: u64, protect: u32, attributes: u32) -> Result<Self, NtStatus> {
        vad::access_of(protect)?;
        let count =
            usize::try_from(size.div_ceil(PAGE_SIZE)).map_err(|_| NtStatus::SECTION_TOO_BIG)?;
        let mut pages = Vec::new();
        pages.try_reserve_exact(count).map_err(|_| NtStatus::NO_MEMORY)?;
        pages.resize(count, None);
        Ok(Section { size, protect, attributes, pages: IrqMutex::new(pages) })
    }

    /// The frame backing page `index`, allocated zeroed on first use.
    pub fn frame(&self, index: usize) -> Option<Frame> {
        let mut pages = self.pages.lock();
        let slot = pages.get_mut(index)?;
        if slot.is_none() {
            *slot = Some(UserSpace::zeroed_frame()?);
        }
        *slot
    }

    /// Whether a view with protection `protect` fits this section's.
    pub fn allows(&self, protect: u32) -> Result<(), NtStatus> {
        let want = vad::access_of(protect)?;
        let have = vad::access_of(self.protect)?;
        let subset = |w: bool, h: bool| !w || h;
        let Access { read, write, execute } = want;
        if subset(read, have.read || have.execute)
            && subset(write, have.write)
            && subset(execute, have.execute)
        {
            Ok(())
        } else {
            Err(NtStatus::SECTION_PROTECTION)
        }
    }
}

impl Drop for Section {
    fn drop(&mut self) {
        for frame in self.pages.lock().iter().flatten() {
            let _ = crate::mm::free_frame(*frame);
        }
    }
}

/// A mapped view of a section in one process.
pub struct View {
    pub base: u64,
    pub size: u64,
    /// Byte offset of the view in the section.
    pub offset: u64,
    pub section: ObjectRef,
}

fn section_of(object: &ObjectRef) -> &Section {
    match &object.body {
        super::object::ObjectBody::Section(s) => s,
        _ => unreachable!("views hold section objects"),
    }
}

/// Maps a view of `object` (a section) into `process`.
pub fn map_view(
    process: &Process,
    object: &ObjectRef,
    base: u64,
    offset: u64,
    size: u64,
    protect: u32,
    top_down: bool,
) -> Result<(u64, u64), NtStatus> {
    let section = section_of(object);
    section.allows(protect)?;
    if !offset.is_multiple_of(vad::ALLOCATION_GRANULARITY) {
        return Err(NtStatus::MAPPED_ALIGNMENT);
    }
    if offset >= section.size {
        return Err(NtStatus::INVALID_VIEW_SIZE);
    }
    let size = if size == 0 { section.size - offset } else { size };
    if offset.checked_add(size).is_none_or(|end| end > section.size) {
        return Err(NtStatus::INVALID_VIEW_SIZE);
    }
    let mut views = process.views.lock();
    views.try_reserve(1).map_err(|_| NtStatus::NO_MEMORY)?;
    let mut vad = process.vad.lock();
    let at = (base != 0).then_some(base);
    let (base, size) = vad.reserve(at, size, protect, vad::MEM_MAPPED, top_down)?;
    if section.attributes & SEC_RESERVE == 0 {
        vad.commit(base, size, protect)?;
    }
    let _ = views.try_push(View { base, size, offset, section: object.clone() });
    Ok((base, size))
}

/// Unmaps the view containing `addr`.
pub fn unmap_view(process: &Process, addr: u64) -> Result<(), NtStatus> {
    let view = {
        let mut views = process.views.lock();
        let i = views
            .iter()
            .position(|v| (v.base..v.base + v.size).contains(&addr))
            .ok_or(NtStatus::NOT_MAPPED_VIEW)?;
        views.swap_remove(i)
    };
    process.vad.lock().release(view.base, 0)?;
    let mut space = process.space.lock();
    let mut page = view.base;
    while page < view.base + view.size {
        space.unmap_page(page); // borrowed: frames stay with the section
        page += PAGE_SIZE;
    }
    Ok(())
}

/// The section frame backing user address `addr`, if it is in a view.
pub fn frame_for(process: &Process, addr: u64) -> Option<Frame> {
    let views = process.views.lock();
    let v = views.iter().find(|v| (v.base..v.base + v.size).contains(&addr))?;
    let index = usize::try_from((addr - v.base + v.offset) / PAGE_SIZE).ok()?;
    section_of(&v.section).frame(index)
}

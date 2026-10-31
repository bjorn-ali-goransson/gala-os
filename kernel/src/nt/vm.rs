//! Virtual memory services on top of the VAD and the user page tables.

use super::platform;
use super::platform::PAGE_SIZE;
use super::platform::{CacheMode, PageFlags};
use super::process::Process;
use super::status::NtStatus;
use super::vad::{self, PageState};

/// Page-table flags for a committed page with `protect`. Pages without user
/// access (NOACCESS, GUARD) stay mapped but kernel-only, so any user access
/// faults while their contents survive.
pub fn flags_for(protect: u32) -> PageFlags {
    let access = vad::access_of(protect).unwrap_or(vad::Access {
        read: false,
        write: false,
        execute: false,
    });
    let user = (access.read || access.execute) && protect & vad::PAGE_GUARD == 0;
    // Copy-on-write pages are mapped read-only; the first write faults and
    // gets a private copy (`resolve_fault`).
    let writable = access.write && !is_write_copy(protect);
    let cache = if protect & vad::PAGE_NOCACHE != 0 {
        CacheMode::Uncached
    } else if protect & vad::PAGE_WRITECOMBINE != 0 {
        CacheMode::WriteCombining
    } else {
        CacheMode::WriteBack
    };
    PageFlags {
        writable: user && writable,
        executable: user && access.execute,
        user,
        global: false,
        cache,
        borrowed: false,
    }
}

/// Whether `protect` is PAGE_WRITECOPY or PAGE_EXECUTE_WRITECOPY.
pub fn is_write_copy(protect: u32) -> bool {
    matches!(protect & 0xff, vad::PAGE_WRITECOPY | vad::PAGE_EXECUTE_WRITECOPY)
}

/// What a copy-on-write page becomes once copied (modifiers kept).
fn after_copy(protect: u32) -> u32 {
    let base = match protect & 0xff {
        vad::PAGE_EXECUTE_WRITECOPY => vad::PAGE_EXECUTE_READWRITE,
        _ => vad::PAGE_READWRITE,
    };
    (protect & !0xff) | base
}

/// Image pages cannot become shared-writable: Windows turns a request for
/// read/write on them into copy-on-write.
fn image_protection(protect: u32) -> u32 {
    let base = match protect & 0xff {
        vad::PAGE_READWRITE => vad::PAGE_WRITECOPY,
        vad::PAGE_EXECUTE_READWRITE => vad::PAGE_EXECUTE_WRITECOPY,
        other => other,
    };
    (protect & !0xff) | base
}

/// Kind of user access that faulted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccessKind {
    Read,
    Write,
    Execute,
}

/// What a user page fault comes to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    /// The page is usable now: retry the access.
    Resolved,
    /// Raise this exception (the guard is gone; a retry may succeed).
    Raise(NtStatus),
    /// Not allowed: an access violation.
    Denied,
}

/// Resolves a fault on a committed page: maps a zeroed (or section) frame
/// on first use, and handles guard pages (one-shot
/// STATUS_GUARD_PAGE_VIOLATION, or stack growth for the current thread's
/// stack guard).
pub fn resolve_fault(process: &Process, addr: u64, kind: AccessKind) -> Fault {
    let (protect, mapped) = {
        let vad = process.vad.lock();
        let Some(PageState { committed: true, protect }) = vad.page(addr) else {
            return Fault::Denied;
        };
        let mapped = vad.query(addr).is_ok_and(|q| q.kind == vad::MEM_MAPPED);
        (protect, mapped)
    };
    let Ok(access) = vad::access_of(protect) else { return Fault::Denied };
    let allowed = match kind {
        AccessKind::Read => access.read,
        AccessKind::Write => access.write,
        AccessKind::Execute => access.execute,
    };
    if !allowed {
        return Fault::Denied;
    }
    if protect & vad::PAGE_GUARD != 0 {
        return guard_hit(process, addr, protect & !vad::PAGE_GUARD);
    }
    // Section pages come from the section (views lock before space lock).
    let section_frame = if mapped && !process.space.lock().is_mapped(addr) {
        match super::section::frame_for(process, addr) {
            Some(f) => Some(f),
            None => return Fault::Denied,
        }
    } else {
        None
    };
    if kind == AccessKind::Write && is_write_copy(protect) {
        return copy_on_write(process, addr, protect, section_frame);
    }
    let mut space = process.space.lock();
    let done = if space.is_mapped(addr) {
        // Present and allowed by the VAD: the PTE is stale. Re-sync it.
        space.set_flags(addr, flags_for(protect)).is_ok()
    } else {
        match section_frame {
            Some(frame) => space.map_borrowed(addr, frame, flags_for(protect)).is_ok(),
            None => space.map_zero_page(addr, flags_for(protect)).is_ok(),
        }
    };
    if done { Fault::Resolved } else { Fault::Denied }
}

/// A write to a copy-on-write page: the page becomes private to this view
/// (a copy of the shared frame, or the frame it already owns) and is
/// reported as PAGE_READWRITE (or PAGE_EXECUTE_READWRITE) from now on.
fn copy_on_write(
    process: &Process,
    addr: u64,
    protect: u32,
    section_frame: Option<platform::Frame>,
) -> Fault {
    let page = addr / PAGE_SIZE * PAGE_SIZE;
    let writable = after_copy(protect);
    if process.vad.lock().protect(page, PAGE_SIZE, writable).is_err() {
        return Fault::Denied;
    }
    let flags = flags_for(writable);
    let mut space = process.space.lock();
    let done = match space.mapping(page) {
        Some((frame, true)) => space.map_private_copy(page, frame, flags).is_ok(),
        Some((_, false)) => space.set_flags(page, flags).is_ok(),
        None => match section_frame {
            Some(frame) => space.map_private_copy(page, frame, flags).is_ok(),
            None => space.map_zero_page(page, flags).is_ok(),
        },
    };
    if done { Fault::Resolved } else { Fault::Denied }
}

/// A touched guard page: the guard is removed. In the current thread's
/// stack, the guard moves one page down and StackLimit follows, silently;
/// with no room left (the bottom page stays reserved) the stack overflows.
/// Elsewhere the program gets STATUS_GUARD_PAGE_VIOLATION.
fn guard_hit(process: &Process, addr: u64, protect: u32) -> Fault {
    let page = addr / PAGE_SIZE * PAGE_SIZE;
    let stack = super::thread::current_stack(process);
    let (result, new_limit) = {
        let mut vad = process.vad.lock();
        if vad.protect(page, PAGE_SIZE, protect).is_err() {
            return Fault::Denied;
        }
        match stack {
            Some((teb, bottom, base)) if (bottom..base).contains(&page) => {
                let next = page - PAGE_SIZE;
                let room = next > bottom;
                let grown = room
                    && match vad.page(next) {
                        Some(PageState { committed: true, .. }) => true,
                        Some(_) => vad.commit(next, PAGE_SIZE, protect | vad::PAGE_GUARD).is_ok(),
                        None => false,
                    };
                let result =
                    if grown { Fault::Resolved } else { Fault::Raise(NtStatus::STACK_OVERFLOW) };
                (result, Some((teb, page)))
            }
            _ => (Fault::Raise(NtStatus::GUARD_PAGE_VIOLATION), None),
        }
    };
    let mut space = process.space.lock();
    if space.is_mapped(page) {
        let _ = space.set_flags(page, flags_for(protect));
    }
    if let Some((teb, limit)) = new_limit {
        let _ = space.write(teb + super::layout::teb::STACK_LIMIT, &limit.to_le_bytes());
    }
    result
}

/// Re-applies VAD state to the page tables for `[start, start + size)`:
/// committed pages get their flags, others lose their frame.
fn sync_range(process: &Process, start: u64, size: u64) {
    let vad = process.vad.lock();
    let mut space = process.space.lock();
    let mut addr = start;
    while addr < start + size {
        match vad.page(addr) {
            Some(PageState { committed: true, protect }) => {
                if space.is_mapped(addr) {
                    let _ = space.set_flags(addr, flags_for(protect));
                }
            }
            _ => {
                space.unmap_page(addr);
            }
        }
        addr += PAGE_SIZE;
    }
}

pub const MEM_RESET: u32 = 0x8_0000;
pub const MEM_TOP_DOWN: u32 = 0x10_0000;
pub const MEM_DECOMMIT: u32 = 0x4000;
pub const MEM_RELEASE: u32 = 0x8000;

/// NtAllocateVirtualMemory core: returns the final (base, size).
pub fn allocate(
    process: &Process,
    base: u64,
    size: u64,
    kind: u32,
    protect: u32,
) -> Result<(u64, u64), NtStatus> {
    let known = vad::MEM_COMMIT | vad::MEM_RESERVE | MEM_RESET | MEM_TOP_DOWN;
    if kind & !known != 0 || kind & (vad::MEM_COMMIT | vad::MEM_RESERVE | MEM_RESET) == 0 {
        return Err(NtStatus::INVALID_PARAMETER_5);
    }
    if kind & MEM_RESET != 0 {
        // Contents may be discarded; keeping them is a valid implementation.
        return Ok((base / PAGE_SIZE * PAGE_SIZE, size.next_multiple_of(PAGE_SIZE)));
    }
    let mut vad = process.vad.lock();
    let reserve = kind & vad::MEM_RESERVE != 0 || base == 0;
    let (base, size) = if reserve {
        let at = (base != 0).then_some(base);
        vad.reserve(at, size, protect, vad::MEM_PRIVATE, kind & MEM_TOP_DOWN != 0)?
    } else {
        (base, size)
    };
    if kind & vad::MEM_COMMIT != 0 {
        match vad.commit(base, size, protect) {
            Ok(range) => return Ok(range),
            Err(e) => {
                if reserve {
                    let _ = vad.release(base, 0);
                }
                return Err(e);
            }
        }
    }
    Ok((base, size))
}

/// NtFreeVirtualMemory core.
pub fn free(
    process: &Process,
    base: u64,
    size: u64,
    free_type: u32,
) -> Result<(u64, u64), NtStatus> {
    if process
        .vad
        .lock()
        .query(base)
        .is_ok_and(|q| q.kind == vad::MEM_MAPPED || q.kind == vad::MEM_IMAGE)
    {
        return Err(NtStatus::UNABLE_TO_DELETE_SECTION);
    }
    let range = match free_type {
        MEM_DECOMMIT => process.vad.lock().decommit(base, size)?,
        MEM_RELEASE => process.vad.lock().release(base, size)?,
        _ => return Err(NtStatus::INVALID_PARAMETER_4),
    };
    sync_range(process, range.0, range.1);
    Ok(range)
}

/// NtProtectVirtualMemory core: returns (old protection, base, size).
pub fn protect(
    process: &Process,
    base: u64,
    size: u64,
    new: u32,
) -> Result<(u32, u64, u64), NtStatus> {
    let result = {
        let mut vad = process.vad.lock();
        let image = vad.query(base).is_ok_and(|q| q.kind == vad::MEM_IMAGE);
        vad.protect(base, size, if image { image_protection(new) } else { new })?
    };
    sync_range(process, result.1, result.2);
    Ok(result)
}

/// Registers an already mapped (or demand-zero) region in the VAD.
pub fn register(
    process_vad: &mut vad::Vad,
    base: u64,
    size: u64,
    kind: u32,
    protect: u32,
) -> Result<(), NtStatus> {
    process_vad.reserve(Some(base), size, protect, kind, false)?;
    process_vad.commit(base, size, protect)?;
    Ok(())
}

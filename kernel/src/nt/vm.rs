//! Virtual memory services on top of the VAD and the user page tables.

use super::process::Process;
use super::status::NtStatus;
use super::vad::{self, PageState};
use crate::mm::PAGE_SIZE;
use crate::mm::paging::{CacheMode, PageFlags};

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
    let cache = if protect & vad::PAGE_NOCACHE != 0 {
        CacheMode::Uncached
    } else if protect & vad::PAGE_WRITECOMBINE != 0 {
        CacheMode::WriteCombining
    } else {
        CacheMode::WriteBack
    };
    PageFlags {
        writable: user && access.write,
        executable: user && access.execute,
        user,
        global: false,
        cache,
        borrowed: false,
    }
}

/// Kind of user access that faulted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccessKind {
    Read,
    Write,
    Execute,
}

/// Resolves a fault on a committed but not yet backed page by mapping a
/// zeroed frame. Returns false if the access is not allowed (an access
/// violation for the program).
pub fn resolve_fault(process: &Process, addr: u64, kind: AccessKind) -> bool {
    let (protect, mapped) = {
        let vad = process.vad.lock();
        let Some(PageState { committed: true, protect }) = vad.page(addr) else { return false };
        let mapped = vad.query(addr).is_ok_and(|q| q.kind == vad::MEM_MAPPED);
        (protect, mapped)
    };
    let Ok(access) = vad::access_of(protect) else { return false };
    let allowed = match kind {
        AccessKind::Read => access.read,
        AccessKind::Write => access.write,
        AccessKind::Execute => access.execute,
    };
    if !allowed || protect & vad::PAGE_GUARD != 0 {
        return false;
    }
    // Section pages come from the section (views lock before space lock).
    let section_frame = if mapped {
        match super::section::frame_for(process, addr) {
            Some(f) => Some(f),
            None => return false,
        }
    } else {
        None
    };
    let mut space = process.space.lock();
    if space.is_mapped(addr) {
        // Present and allowed by the VAD: the PTE is stale. Re-sync it.
        return space.set_flags(addr, flags_for(protect)).is_ok();
    }
    match section_frame {
        Some(frame) => space.map_borrowed(addr, frame, flags_for(protect)).is_ok(),
        None => space.map_zero_page(addr, flags_for(protect)).is_ok(),
    }
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
    if process.vad.lock().query(base).is_ok_and(|q| q.kind == vad::MEM_MAPPED) {
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
    let result = process.vad.lock().protect(base, size, new)?;
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

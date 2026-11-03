//! Virtual memory services on top of the VAD and the user page tables.

use super::charge;
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
    /// No memory to resolve it now (`resolve_fault` retries).
    OutOfMemory,
}

/// How often `resolve_fault` retries a fault that found no memory, 1 ms
/// apart.
const OOM_RETRIES: u32 = 100;

/// Resolves a fault on a committed page: maps a zeroed (or section) frame
/// on first use, and handles guard pages (one-shot
/// STATUS_GUARD_PAGE_VIOLATION, or stack growth for the current thread's
/// stack guard). A fault on committed memory does not fail on Windows for
/// lack of memory (the commit charge guarantees it; the memory manager
/// waits for a page), so a shortage is waited out for a while before the
/// program gets an access violation (the commit charge, nt::charge,
/// covers private memory only, not the kernel's own use). Fault
/// injection found this: one failed allocation in a copy-on-write fault
/// raised an access violation that exception_test's handler continued
/// from for ever.
pub fn resolve_fault(process: &Process, addr: u64, kind: AccessKind) -> Fault {
    for _ in 0..OOM_RETRIES {
        match resolve_once(process, addr, kind) {
            Fault::OutOfMemory => platform::sleep_ms(1),
            other => return other,
        }
    }
    out_of_memory(process, addr);
    Fault::Denied
}

fn resolve_once(process: &Process, addr: u64, kind: AccessKind) -> Fault {
    // The VAD and the page tables change together: the locks (in their
    // order: views, VAD, space) are held from reading the page's state to
    // mapping it, as in every service that changes either. A second
    // thread faulting on the page, or a protect, free or unmap on another
    // processor, then sees the state before or after, never between: with
    // the VAD read first and the page mapped later, a copy-on-write fault
    // that had made the page private in the VAD let another thread's fault
    // find it "read/write and mapped" and make the shared frame writable
    // (smpcow_test).
    // A data section's page is looked up first, with no lock held (making
    // a file's page may deliver change notifications, which write user
    // memory); it is used only if the view is still there below.
    let looked_up = !process.space.lock().is_mapped(addr);
    let section_page = if looked_up { super::section::frame_for(process, addr) } else { None };
    let views = process.views.lock();
    let mut vad = process.vad.lock();
    let Some(PageState { committed: true, protect }) = vad.page(addr) else {
        return Fault::Denied;
    };
    let mapped = vad.query(addr).is_ok_and(|q| q.kind == vad::MEM_MAPPED);
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
        return guard_hit(process, &mut vad, addr, protect & !vad::PAGE_GUARD);
    }
    let section_frame = if mapped && !process.space.lock().is_mapped(addr) {
        match &section_page {
            Some((section, frame)) if super::section::in_view_of(&views, addr, section) => {
                Some(*frame)
            }
            // No page there (past the section's end).
            None if looked_up => return Fault::Denied,
            // The page or view changed since the lookup: look again (the
            // retry of a fault that found no memory).
            _ => return Fault::OutOfMemory,
        }
    } else {
        None
    };
    // `views` stays held to the end: IrqMutex guards must go in the
    // reverse order of taking (dropping it first turned interrupts back on
    // with the VAD lock still held; lockdep then saw that lock held across
    // a preemption).
    if kind == AccessKind::Write && is_write_copy(protect) {
        return copy_on_write(process, &mut vad, addr, protect, section_frame);
    }
    let mut space = process.space.lock();
    let done = if space.is_mapped(addr) {
        // Present and allowed by the VAD: the PTE is stale (another
        // processor's fault on the page mapped it first). Re-sync it.
        space.set_flags(addr, flags_for(protect)).is_ok()
    } else {
        match section_frame {
            Some(frame) => space.map_borrowed(addr, frame, flags_for(protect)).is_ok(),
            None => space.map_zero_page(addr, flags_for(protect)).is_ok(),
        }
    };
    if done { Fault::Resolved } else { Fault::OutOfMemory }
}

/// A committed page could not be given a frame: the machine is out of
/// memory. The commit charge (nt::charge) makes this rare: what is not
/// charged (the kernel heap, section and image pages) can still use the
/// frames up, so the program sees an access violation; this is logged (once per
/// process) and counted in the ledger so it is not mistaken for a bug.
fn out_of_memory(process: &Process, addr: u64) {
    let (free, total) = platform::frame_stats();
    super::ledger::record("user page fault", "out of memory", free);
    if !process.oom_logged.swap(true, core::sync::atomic::Ordering::Relaxed) {
        crate::warn!(
            "vm: out of memory in pid {} at {addr:#x} ({free} of {total} frames free)",
            process.id
        );
    }
}

/// A write to a copy-on-write page: the page becomes private to this view
/// (a copy of the shared frame, or the frame it already owns) and is
/// reported as PAGE_READWRITE (or PAGE_EXECUTE_READWRITE) from now on.
fn copy_on_write(
    process: &Process,
    vad: &mut vad::Vad,
    addr: u64,
    protect: u32,
    section_frame: Option<platform::Frame>,
) -> Fault {
    let page = addr / PAGE_SIZE * PAGE_SIZE;
    let writable = after_copy(protect);
    let flags = flags_for(writable);
    // The copy first, the VAD after: a page the VAD calls private is never
    // the shared frame (after a failed copy the page is still
    // copy-on-write, and the retry copies it).
    let mut space = process.space.lock();
    let done = match space.mapping(page) {
        Some((frame, true)) => space.map_private_copy(page, frame, flags).is_ok(),
        Some((_, false)) => space.set_flags(page, flags).is_ok(),
        None => match section_frame {
            Some(frame) => space.map_private_copy(page, frame, flags).is_ok(),
            None => space.map_zero_page(page, flags).is_ok(),
        },
    };
    drop(space);
    // Only an allocation fails in the VAD (the page is committed); the
    // private page stays mapped and the retry finds it.
    if done && vad.protect(page, PAGE_SIZE, writable).is_ok() {
        Fault::Resolved
    } else {
        Fault::OutOfMemory
    }
}

/// A touched guard page: the guard is removed. In the current thread's
/// stack, the guard moves one page down and StackLimit follows, silently;
/// with no room left (the bottom page stays reserved) the stack overflows.
/// Elsewhere the program gets STATUS_GUARD_PAGE_VIOLATION.
fn guard_hit(process: &Process, vad: &mut vad::Vad, addr: u64, protect: u32) -> Fault {
    let page = addr / PAGE_SIZE * PAGE_SIZE;
    let stack = super::thread::current_stack(process);
    let (result, new_limit) = {
        if vad.protect(page, PAGE_SIZE, protect).is_err() {
            return Fault::OutOfMemory;
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
                charge::sync(process, vad.private_commit());
                let result = if grown {
                    Fault::Resolved
                } else {
                    crate::warn!(
                        "vm: stack overflow in pid {} at {addr:#x} (stack {bottom:#x}..{base:#x})",
                        process.id
                    );
                    Fault::Raise(NtStatus::STACK_OVERFLOW)
                };
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
/// committed pages get their flags, others lose their frame. The caller
/// holds the VAD lock from its change to this (see `resolve_once`).
fn sync_range(process: &Process, vad: &vad::Vad, start: u64, size: u64) {
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
/// Windows 8+: takes back a MEM_RESET; succeeds when the data is intact.
pub const MEM_RESET_UNDO: u32 = 0x100_0000;
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
    // MEM_RESET_UNDO goes alone.
    let alone = kind == MEM_RESET_UNDO;
    if !alone
        && (kind & !known != 0 || kind & (vad::MEM_COMMIT | vad::MEM_RESERVE | MEM_RESET) == 0)
    {
        return Err(NtStatus::INVALID_PARAMETER_5);
    }
    if kind & (MEM_RESET | MEM_RESET_UNDO) != 0 {
        // Contents may be discarded; keeping them is a valid implementation
        // (and MEM_RESET_UNDO then finds them intact: success).
        return Ok((base / PAGE_SIZE * PAGE_SIZE, size.next_multiple_of(PAGE_SIZE)));
    }
    let mut vad = process.vad.lock();
    charge::sync(process, vad.private_commit());
    let reserve = kind & vad::MEM_RESERVE != 0 || base == 0;
    let (base, size) = if reserve {
        let at = (base != 0).then_some(base);
        vad.reserve(at, size, protect, vad::MEM_PRIVATE, kind & MEM_TOP_DOWN != 0)?
    } else {
        (base, size)
    };
    if kind & vad::MEM_COMMIT != 0 {
        // Charged before the VAD takes it; a failed commit is given back.
        let charged = charge::try_charge(process, vad.commit_cost(base, size));
        let result = charged.and_then(|()| vad.commit(base, size, protect));
        charge::sync(process, vad.private_commit());
        match result {
            Ok(range) => {
                // Pages committed already take the new protection: their
                // mappings follow (as NtProtectVirtualMemory's do).
                sync_range(process, &vad, range.0, range.1);
                return Ok(range);
            }
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
    let mut vad = process.vad.lock();
    let range = match free_type {
        MEM_DECOMMIT => vad.decommit(base, size)?,
        MEM_RELEASE => vad.release(base, size)?,
        _ => return Err(NtStatus::INVALID_PARAMETER_4),
    };
    charge::sync(process, vad.private_commit());
    // Before the VAD lock goes: memory allocated at once at the same place
    // must not find these frames still mapped (and their old contents).
    sync_range(process, &vad, range.0, range.1);
    Ok(range)
}

/// NtProtectVirtualMemory core: returns (old protection, base, size).
pub fn protect(
    process: &Process,
    base: u64,
    size: u64,
    new: u32,
) -> Result<(u32, u64, u64), NtStatus> {
    let mut vad = process.vad.lock();
    let image = vad.query(base).is_ok_and(|q| q.kind == vad::MEM_IMAGE);
    let result = vad.protect(base, size, if image { image_protection(new) } else { new })?;
    sync_range(process, &vad, result.1, result.2);
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

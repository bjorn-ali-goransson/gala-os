//! Linux memory calls on the core address space: anonymous mmap(2),
//! munmap(2), mprotect(2) and brk(2), page-granular, with Linux's errors
//! (man-pages; user/lx/mmap_test and brk_test record what Linux 6.18
//! does).
//!
//! The pages live in the process's VAD (`nt::vad`, its page-granular
//! Linux methods) as committed private memory, so the core's fault path
//! (`nt::vm::resolve_fault`) gives them zeroed frames on first touch, and
//! `nt::vm::sync_range` keeps the page tables in step (frames of removed
//! pages are freed; changed protections take effect). Linux overcommits:
//! the pages are counted in the commit charge but never refused by it.
//!
//! Lock order as everywhere: the VAD, then the space (inside
//! `sync_range`). No user memory is touched with the VAD lock held.

use super::errno::{EBADF, EEXIST, EINVAL, ENOMEM};
use crate::nt::process::Process;
use crate::nt::status::NtStatus;
use crate::nt::vad::{self, Vad};
use crate::nt::{charge, vm};

pub const PAGE: u64 = 4096;
/// The end of the user address space for Linux's checks (TASK_SIZE on
/// x86_64 with 4-level paging; elf::USER_END).
pub const TASK_SIZE: u64 = super::elf::USER_END;
/// The top of the main thread's stack (its last byte is below), and its
/// size (RLIMIT_STACK's usual 8 MiB, committed, given frames on touch).
pub const STACK_END: u64 = 0x7fff_fffe_0000;
pub const STACK_SIZE: u64 = 8 << 20;
/// mmap without a usable hint places mappings top-down from here (below
/// the stack, as Linux's mmap base is).
pub const MMAP_TOP: u64 = STACK_END - STACK_SIZE - (16 << 20);

pub const PROT_READ: u64 = 1;
pub const PROT_WRITE: u64 = 2;
pub const PROT_EXEC: u64 = 4;

pub const MAP_SHARED: u64 = 0x01;
pub const MAP_PRIVATE: u64 = 0x02;
pub const MAP_SHARED_VALIDATE: u64 = 0x03;
pub const MAP_TYPE: u64 = 0x0f;
pub const MAP_FIXED: u64 = 0x10;
pub const MAP_ANONYMOUS: u64 = 0x20;
pub const MAP_FIXED_NOREPLACE: u64 = 0x10_0000;

/// The NT page protection for Linux's PROT_* bits (on x86_64 write and
/// exec imply read).
pub fn protect_of(prot: u64) -> u32 {
    let (w, x) = (prot & PROT_WRITE != 0, prot & PROT_EXEC != 0);
    match (prot & 7 == 0, w, x) {
        (true, ..) => vad::PAGE_NOACCESS,
        (_, true, true) => vad::PAGE_EXECUTE_READWRITE,
        (_, false, true) => vad::PAGE_EXECUTE_READ,
        (_, true, false) => vad::PAGE_READWRITE,
        (_, false, false) => vad::PAGE_READONLY,
    }
}

fn round_up(len: u64) -> Option<u64> {
    len.checked_next_multiple_of(PAGE)
}

/// After a change of `[start, end)` in the VAD: the charge follows and
/// the page tables match.
fn sync(process: &Process, vad: &Vad, start: u64, end: u64) {
    charge::sync(process, vad.private_commit());
    vm::sync_range(process, vad, start, end - start);
}

/// mmap(2) of anonymous memory. Returns the address or an errno.
pub fn mmap(
    process: &Process,
    addr: u64,
    len: u64,
    prot: u64,
    flags: u64,
    fd: i64,
    offset: u64,
) -> Result<u64, u16> {
    if len == 0 || !offset.is_multiple_of(PAGE) {
        return Err(EINVAL);
    }
    match flags & MAP_TYPE {
        MAP_SHARED | MAP_PRIVATE | MAP_SHARED_VALIDATE => {}
        _ => return Err(EINVAL),
    }
    if flags & MAP_ANONYMOUS == 0 {
        // File mappings come with the file system calls (milestone 2):
        // no fd can name a file yet.
        let _ = fd;
        return Err(EBADF);
    }
    if prot & !(PROT_READ | PROT_WRITE | PROT_EXEC) != 0 {
        return Err(EINVAL);
    }
    let len = round_up(len).filter(|&l| l <= TASK_SIZE).ok_or(ENOMEM)?;
    let fixed = flags & (MAP_FIXED | MAP_FIXED_NOREPLACE) != 0;
    if fixed && !addr.is_multiple_of(PAGE) {
        return Err(EINVAL);
    }
    let protect = protect_of(prot);
    let mut vad = process.vad.lock();
    let start = if fixed {
        let end = addr.checked_add(len).filter(|&e| e <= vad::USER_MAX).ok_or(ENOMEM)?;
        if addr < vad::USER_MIN {
            return Err(ENOMEM);
        }
        if !vad.is_range_free(addr, end) {
            if flags & MAP_FIXED_NOREPLACE != 0 {
                return Err(EEXIST);
            }
            // MAP_FIXED replaces what was there.
            vad.unmap_range(addr, end).map_err(|_| ENOMEM)?;
            sync(process, &vad, addr, end);
        }
        addr
    } else {
        let hint = addr / PAGE * PAGE;
        let usable = hint >= vad::USER_MIN
            && hint
                .checked_add(len)
                .is_some_and(|e| e <= vad::USER_MAX && vad.is_range_free(hint, e));
        if usable { hint } else { vad.find_free_below(len, MMAP_TOP).ok_or(ENOMEM)? }
    };
    vad.map_exact(start, start + len, protect, vad::MEM_PRIVATE)
        .map_err(|e| if e == NtStatus::NO_MEMORY { ENOMEM } else { EINVAL })?;
    sync(process, &vad, start, start + len);
    Ok(start)
}

/// munmap(2).
pub fn munmap(process: &Process, addr: u64, len: u64) -> Result<u64, u16> {
    if !addr.is_multiple_of(PAGE) || len == 0 {
        return Err(EINVAL);
    }
    let end = round_up(len).and_then(|l| addr.checked_add(l)).ok_or(EINVAL)?;
    if end > TASK_SIZE {
        return Err(EINVAL);
    }
    let mut vad = process.vad.lock();
    let end = end.min(vad::USER_MAX);
    if addr < end {
        vad.unmap_range(addr, end).map_err(|_| ENOMEM)?;
        sync(process, &vad, addr, end);
    }
    Ok(0)
}

/// mprotect(2): every page of the range must be mapped (ENOMEM).
pub fn mprotect(process: &Process, addr: u64, len: u64, prot: u64) -> Result<u64, u16> {
    if !addr.is_multiple_of(PAGE) || prot & !(PROT_READ | PROT_WRITE | PROT_EXEC) != 0 {
        return Err(EINVAL);
    }
    let end = round_up(len).and_then(|l| addr.checked_add(l)).ok_or(ENOMEM)?;
    if end > TASK_SIZE {
        return Err(ENOMEM);
    }
    if addr == end {
        return Ok(0);
    }
    let protect = protect_of(prot);
    let mut vad = process.vad.lock();
    if !vad.all_committed(addr, end) {
        return Err(ENOMEM);
    }
    let pieces = vad.pieces(addr, end).map_err(|_| ENOMEM)?;
    for (s, e) in pieces {
        vad.protect(s, e - s, protect).map_err(|_| ENOMEM)?;
    }
    sync(process, &vad, addr, end);
    Ok(0)
}

/// brk(2): returns the new break, or the current one when the request is
/// refused (below the start, past the address space, onto a mapping, or
/// out of memory); brk(0) asks for it.
pub fn brk(process: &Process, brk: &crate::sync::IrqMutex<super::state::Brk>, addr: u64) -> u64 {
    let mut b = brk.lock();
    let current = b.current;
    if addr < b.start || addr > MMAP_TOP {
        return current;
    }
    let (Some(old_end), Some(new_end)) = (round_up(current), round_up(addr)) else {
        return current;
    };
    let mut vad = process.vad.lock();
    if new_end > old_end {
        if !vad.is_range_free(old_end, new_end)
            || vad.map_exact(old_end, new_end, vad::PAGE_READWRITE, vad::MEM_PRIVATE).is_err()
        {
            return current;
        }
        sync(process, &vad, old_end, new_end);
    } else if new_end < old_end {
        if vad.unmap_range(new_end, old_end).is_err() {
            return current;
        }
        sync(process, &vad, new_end, old_end);
    }
    b.current = addr;
    addr
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linux_protections_become_nt_ones() {
        assert_eq!(protect_of(0), vad::PAGE_NOACCESS);
        assert_eq!(protect_of(PROT_READ), vad::PAGE_READONLY);
        assert_eq!(protect_of(PROT_WRITE), vad::PAGE_READWRITE);
        assert_eq!(protect_of(PROT_READ | PROT_WRITE), vad::PAGE_READWRITE);
        assert_eq!(protect_of(PROT_EXEC), vad::PAGE_EXECUTE_READ);
        assert_eq!(protect_of(PROT_READ | PROT_EXEC), vad::PAGE_EXECUTE_READ);
        assert_eq!(protect_of(7), vad::PAGE_EXECUTE_READWRITE);
    }

    #[test]
    fn the_layout_leaves_room() {
        const { assert!(MMAP_TOP < STACK_END - STACK_SIZE) };
        const { assert!(STACK_END <= vad::USER_MAX && STACK_END.is_multiple_of(PAGE)) };
        // A static-pie's base and mmap_test's 8 GiB hint are below it.
        const { assert!(super::super::elf::DEFAULT_DYN_BASE < MMAP_TOP) };
    }
}

//! Memory probes and offers (ROADMAP OF: what Office imports):
//! IsBadReadPtr and its companions, OfferVirtualMemory and
//! ReclaimVirtualMemory.
//!
//! Windows' IsBad*Ptr touch the memory under an exception handler; gala's
//! ask the memory manager instead (NtQueryVirtualMemory): a range is good
//! when every page of it is committed with a protection that allows the
//! access and no PAGE_GUARD. A guard page the walk reaches is bad and is
//! used up as Windows' touch uses it up (its one-shot exception is taken
//! by the probe): the walk clears PAGE_GUARD on the page it would have
//! touched, so VirtualQuery then shows the plain protection (Windows,
//! office_k32_test) and a thread's stack can no longer grow through it.
//! The walk stops at the first bad page, as the touch does, so a guard
//! page after a bad one keeps its guard. Neither sets the last error.
//!
//! An offer makes the pages inaccessible (PAGE_NOACCESS, as Microsoft
//! documents) and a reclaim makes them read-write again. gala never
//! discards offered memory, so a reclaim always finds the contents and
//! returns ERROR_SUCCESS (Windows may return ERROR_BUSY: discarded). The
//! pages offered must be private, committed and read-write, the protection
//! a reclaim restores: other memory is refused and recorded.

use super::*;

#[cfg_attr(
    target_arch = "x86",
    link(name = "ntdll", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "ntdll", kind = "raw-dylib"))]
unsafe extern "system" {
    fn NtProtectVirtualMemory(
        process: isize,
        base: *mut *mut c_void,
        size: *mut usize,
        protect: u32,
        old: *mut u32,
    ) -> u32;
}

const MEM_COMMIT: u32 = 0x1000;
const MEM_PRIVATE: u32 = 0x2_0000;
const PAGE_NOACCESS: u32 = 0x01;
const PAGE_READWRITE: u32 = 0x04;
const PAGE_WRITECOPY: u32 = 0x08;
const PAGE_EXECUTE_READWRITE: u32 = 0x40;
const PAGE_EXECUTE_WRITECOPY: u32 = 0x80;
const PAGE_GUARD: u32 = 0x100;
const PAGE_SIZE: usize = 0x1000;

/// The region `address` is in (NtQueryVirtualMemory's
/// MemoryBasicInformation), without touching the last error.
fn region(address: usize) -> Option<MemoryBasic> {
    let mut mbi = MemoryBasic::default();
    let mut ret = 0;
    // SAFETY: a MEMORY_BASIC_INFORMATION buffer of ours.
    let status = unsafe {
        NtQueryVirtualMemory(
            CURRENT_PROCESS,
            address,
            0,
            (&raw mut mbi).cast(),
            size_of::<MemoryBasic>(),
            &mut ret,
        )
    };
    (status == STATUS_SUCCESS).then_some(mbi)
}

/// Walks `[address, address + size)` region by region: Ok when `good`
/// accepts every one (size 0: always), else the address where the walk
/// stopped and its region (None: no region there, or the range wraps).
fn walk(
    address: usize,
    size: usize,
    good: impl Fn(&MemoryBasic) -> bool,
) -> Result<(), Option<(usize, MemoryBasic)>> {
    if size == 0 {
        return Ok(());
    }
    let Some(last) = address.checked_add(size - 1) else { return Err(None) };
    let mut at = address;
    loop {
        let Some(mbi) = region(at) else { return Err(None) };
        if !good(&mbi) {
            return Err(Some((at, mbi)));
        }
        match mbi.base.checked_add(mbi.region_size) {
            Some(end) if end > last => return Ok(()),
            Some(end) if end > at => at = end,
            _ => return Err(None),
        }
    }
}

/// Whether every page of `[address, address + size)` is in a region
/// `good` accepts (size 0: always).
fn every_region(address: usize, size: usize, good: impl Fn(&MemoryBasic) -> bool) -> bool {
    walk(address, size, good).is_ok()
}

/// `every_region` for a probe that touches the memory: the first bad
/// page, when it is a guard page, has its guard used up, as Windows'
/// touch does.
fn probe(address: usize, size: usize, good: impl Fn(&MemoryBasic) -> bool) -> bool {
    match walk(address, size, good) {
        Ok(()) => true,
        Err(stop) => {
            if let Some((at, mbi)) = stop {
                use_guard(at, &mbi);
            }
            false
        }
    }
}

/// Clears PAGE_GUARD on the page holding `at` when `mbi` (its region) is
/// a committed guard page: the touch that found the page bad took its
/// one-shot exception.
fn use_guard(at: usize, mbi: &MemoryBasic) {
    if mbi.state != MEM_COMMIT || mbi.protect & PAGE_GUARD == 0 {
        return;
    }
    let page = at & !(PAGE_SIZE - 1);
    // A failure leaves the guard in place: the answer (bad) is the same.
    let _ = protect(page, PAGE_SIZE, mbi.protect & !PAGE_GUARD);
}

/// Committed, not a guard page, and readable (every protection but
/// PAGE_NOACCESS reads on x86).
fn readable(mbi: &MemoryBasic) -> bool {
    mbi.state == MEM_COMMIT && mbi.protect & (PAGE_GUARD | PAGE_NOACCESS) == 0 && mbi.protect != 0
}

/// Readable and writable (copy-on-write counts: a write makes a copy).
fn writable(mbi: &MemoryBasic) -> bool {
    const WRITE: u32 =
        PAGE_READWRITE | PAGE_WRITECOPY | PAGE_EXECUTE_READWRITE | PAGE_EXECUTE_WRITECOPY;
    readable(mbi) && mbi.protect & WRITE != 0
}

/// IsBadReadPtr(lp, ucb): whether any byte of the range cannot be read;
/// FALSE for a size of 0.
#[unsafe(no_mangle)]
pub extern "system" fn IsBadReadPtr(p: *const c_void, size: usize) -> Bool {
    Bool::from(!probe(p as usize, size, readable))
}

/// IsBadWritePtr(lp, ucb): whether any byte of the range cannot be written.
#[unsafe(no_mangle)]
pub extern "system" fn IsBadWritePtr(p: *mut c_void, size: usize) -> Bool {
    Bool::from(!probe(p as usize, size, writable))
}

/// IsBadHugeReadPtr: IsBadReadPtr (the same on every Win32).
#[unsafe(no_mangle)]
pub extern "system" fn IsBadHugeReadPtr(p: *const c_void, size: usize) -> Bool {
    IsBadReadPtr(p, size)
}

/// IsBadHugeWritePtr: IsBadWritePtr.
#[unsafe(no_mangle)]
pub extern "system" fn IsBadHugeWritePtr(p: *mut c_void, size: usize) -> Bool {
    IsBadWritePtr(p, size)
}

/// IsBadCodePtr(lpfn): whether the byte at the address cannot be read
/// (Microsoft: "read access to the memory at the specified address").
#[unsafe(no_mangle)]
pub extern "system" fn IsBadCodePtr(p: *const c_void) -> Bool {
    IsBadReadPtr(p, 1)
}

/// Whether a string of `T` at `p` cannot be read up to its NUL or `max`
/// characters, whichever comes first (`max` 0: never bad).
fn bad_string<T: Copy + PartialEq + Default>(p: *const T, max: usize) -> Bool {
    if max == 0 {
        return FALSE;
    }
    let width = size_of::<T>();
    let mut at = p as usize;
    let mut left = max;
    loop {
        let Some(mbi) = region(at) else { return TRUE };
        if !readable(&mbi) {
            use_guard(at, &mbi);
            return TRUE;
        }
        let Some(end) = mbi.base.checked_add(mbi.region_size).filter(|&e| e > at) else {
            return TRUE;
        };
        while at + width <= end {
            // SAFETY: inside a committed, readable region.
            let c = unsafe { core::ptr::read_unaligned(at as *const T) };
            left -= 1;
            if c == T::default() || left == 0 {
                return FALSE;
            }
            at += width;
        }
        if at < end {
            // A character straddling into the next region: its first part
            // is readable; the next region decides.
            if !probe(end, at + width - end, readable) {
                return TRUE;
            }
            // SAFETY: both parts are readable.
            let c = unsafe { core::ptr::read_unaligned(at as *const T) };
            left -= 1;
            if c == T::default() || left == 0 {
                return FALSE;
            }
            at += width;
        }
    }
}

/// IsBadStringPtrA(lpsz, ucchMax).
#[unsafe(no_mangle)]
pub extern "system" fn IsBadStringPtrA(p: *const u8, max: usize) -> Bool {
    bad_string(p, max)
}

/// IsBadStringPtrW(lpsz, ucchMax): in WCHARs.
#[unsafe(no_mangle)]
pub extern "system" fn IsBadStringPtrW(p: *const u16, max: usize) -> Bool {
    bad_string(p, max)
}

/// The page-aligned whole pages `[address, address + size)`, or None.
fn pages(address: *const c_void, size: usize) -> Option<(usize, usize)> {
    let a = address as usize;
    (a != 0
        && size != 0
        && a.is_multiple_of(PAGE_SIZE)
        && size.is_multiple_of(PAGE_SIZE)
        && a.checked_add(size).is_some())
    .then_some((a, size))
}

/// Changes the pages' protection; the Win32 error.
fn protect(address: usize, size: usize, protection: u32) -> u32 {
    let (mut base, mut len, mut old) = (address as *mut c_void, size, 0);
    // SAFETY: our own outputs; the range was checked.
    let status = unsafe {
        NtProtectVirtualMemory(CURRENT_PROCESS, &mut base, &mut len, protection, &mut old)
    };
    if status == STATUS_SUCCESS {
        0
    } else {
        // SAFETY: plain call.
        unsafe { RtlNtStatusToDosError(status) }
    }
}

/// Private committed memory with protection `protection` throughout.
fn private_with(protection: u32) -> impl Fn(&MemoryBasic) -> bool {
    move |mbi| mbi.state == MEM_COMMIT && mbi.kind == MEM_PRIVATE && mbi.protect == protection
}

/// OfferVirtualMemory(VirtualAddress, Size, Priority): the pages are
/// inaccessible until reclaimed (Priority VmOfferPriorityVeryLow 1 to
/// VmOfferPriorityNormal 4). ERROR_INVALID_PARAMETER for another priority,
/// a range not of whole pages, or memory that is not private, committed
/// read-write memory (recorded).
#[unsafe(no_mangle)]
pub extern "system" fn OfferVirtualMemory(address: *mut c_void, size: usize, priority: u32) -> u32 {
    let Some((a, n)) = pages(address, size).filter(|_| (1..=4).contains(&priority)) else {
        return ERROR_INVALID_PARAMETER;
    };
    if !every_region(a, n, private_with(PAGE_READWRITE)) {
        let protection = region(a).map_or(0, |m| m.protect);
        unsupported(b"kernel32.dll!OfferVirtualMemory(memory)", protection.into());
        return ERROR_INVALID_PARAMETER;
    }
    protect(a, n, PAGE_NOACCESS)
}

/// ReclaimVirtualMemory(VirtualAddress, Size): offered pages read-write
/// again, their contents kept (gala discards nothing): ERROR_SUCCESS.
/// ERROR_INVALID_PARAMETER for a range not of whole pages or memory that
/// was not offered (recorded).
#[unsafe(no_mangle)]
pub extern "system" fn ReclaimVirtualMemory(address: *const c_void, size: usize) -> u32 {
    let Some((a, n)) = pages(address, size) else { return ERROR_INVALID_PARAMETER };
    if !every_region(a, n, private_with(PAGE_NOACCESS)) {
        let protection = region(a).map_or(0, |m| m.protect);
        unsupported(b"kernel32.dll!ReclaimVirtualMemory(memory)", protection.into());
        return ERROR_INVALID_PARAMETER;
    }
    protect(a, n, PAGE_READWRITE)
}

//! Global and local memory (GlobalAlloc, LocalAlloc and friends), as
//! Microsoft documents them and as Wine's conformance tests observe them
//! on Windows (`dlls/kernel32/tests/heap.c`).
//!
//! Fixed memory (GMEM_FIXED / LMEM_FIXED) is a process-heap block and its
//! pointer is the handle. Moveable memory is a handle to an entry of a
//! handle table, `{flags, pointer}` (16 bytes on x64, 8 on x86); the
//! handle is the address of the pointer, so `handle % 16 == 8` (x86:
//! `% 8 == 4`) tells it from a heap pointer (aligned to the entry's
//! size), and `*(void **)handle` is the data, as old programs assume. The entry's flags are Windows': 1 in use, 2 moveable, 4
//! discardable, 8 discarded (no memory: size 0 or discarded), 0x8000
//! GMEM_DDESHARE, the lock count in bits 16-23 (saturating at 255). A
//! freed entry holds the next free entry's address instead (bit 0 clear),
//! so a stale handle is caught (ERROR_INVALID_HANDLE).
//!
//! The table is one reservation of 65536 entries (Windows' limit: the
//! next moveable allocation fails with ERROR_NOT_ENOUGH_MEMORY),
//! committed a page (256 entries) at a time; a spinlock serializes it.
//! Global and local handles are one kind; GlobalUnlock and LocalUnlock
//! differ for fixed memory and bad handles, GlobalFlags and LocalFlags in
//! how they spell "discardable".
//!
//! Values that are neither a table entry nor a mapped address (checked
//! with NtQueryVirtualMemory, since gala's DLLs have no exception
//! handling of their own) fail as Windows' do after catching the fault.

use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use super::heap::{GetProcessHeap, HeapAlloc, HeapFree, HeapReAlloc, HeapSize};
use super::*;

const GMEM_MOVEABLE: u32 = 0x2;
const GMEM_ZEROINIT: u32 = 0x40;
const GMEM_MODIFY: u32 = 0x80;
/// LMEM_DISCARDABLE (0xf00) covers GMEM_DISCARDABLE (0x100).
const LMEM_DISCARDABLE: u32 = 0xf00;
const GMEM_DISCARDABLE: u32 = 0x100;
const GMEM_DDESHARE: u32 = 0x2000;
const GMEM_DISCARDED: u32 = 0x4000;
const GMEM_INVALID_HANDLE: u32 = 0x8000;

const HEAP_ZERO_MEMORY: u32 = 0x8;
const HEAP_REALLOC_IN_PLACE_ONLY: u32 = 0x10;

const ERROR_NOACCESS: u32 = 998;
const ERROR_DISCARDED: u32 = 157;
const ERROR_NOT_LOCKED: u32 = 158;

/// Entry flags (Windows' values, visible at the word before the handle).
const IN_USE: usize = 0x1;
const MOVEABLE: usize = 0x2;
const DISCARDABLE: usize = 0x4;
const DISCARDED: usize = 0x8;
const DDESHARE: usize = 0x8000;
const LOCK_SHIFT: u32 = 16;
const LOCK_MAX: usize = 0xff;

const ENTRIES: usize = 0x10000;
const ENTRY: usize = size_of::<Entry>();
/// Where an entry's pointer (the handle) is.
const POINTER_AT: usize = core::mem::offset_of!(Entry, ptr);
const TABLE_BYTES: usize = ENTRIES * ENTRY;
const PAGE: usize = 4096;

#[repr(C)]
struct Entry {
    flags: usize,
    ptr: *mut c_void,
}

static LOCK: AtomicBool = AtomicBool::new(false);
/// The table's reservation (0 until the first moveable allocation).
static BASE: AtomicUsize = AtomicUsize::new(0);
/// Bytes of the table committed (and linked into entries).
static COMMITTED: AtomicUsize = AtomicUsize::new(0);
/// The first free entry, or 0; a free entry's flags hold the next one,
/// or END_OF_LIST.
static FREE: AtomicUsize = AtomicUsize::new(0);
/// A free entry's link at the end of the list: non-zero and aligned to
/// the entry's size, like every link.
const END_OF_LIST: usize = 0x10;

/// Runs `f` under the table's lock.
fn locked<R>(f: impl FnOnce() -> R) -> R {
    let mut spins = 0u32;
    while LOCK.compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        spins += 1;
        if spins < 64 {
            core::hint::spin_loop();
        } else {
            SwitchToThread();
        }
    }
    let r = f();
    LOCK.store(false, Ordering::Release);
    r
}

/// Whether `h` has the shape of a moveable handle.
fn entry_shaped(h: *const c_void) -> bool {
    h as usize & (ENTRY - 1) == POINTER_AT
}

/// The live entry behind `h` (under the lock).
fn find(h: *const c_void) -> Option<&'static mut Entry> {
    let base = BASE.load(Ordering::Relaxed);
    let h = h as usize;
    if base == 0
        || !entry_shaped(h as *const c_void)
        || h < base
        || h >= base + COMMITTED.load(Ordering::Relaxed)
    {
        return None;
    }
    // SAFETY: inside the committed table; entries are aligned to their
    // size and the handle is the address of the second word.
    let entry = unsafe { &mut *((h - POINTER_AT) as *mut Entry) };
    (entry.flags & (IN_USE | MOVEABLE) == IN_USE | MOVEABLE).then_some(entry)
}

/// A free entry (under the lock), committing another page of the table
/// when the free list is empty; None when all 65536 are in use.
fn take_entry() -> Option<&'static mut Entry> {
    if FREE.load(Ordering::Relaxed) == 0 && !grow() {
        return None;
    }
    let e = FREE.load(Ordering::Relaxed);
    // SAFETY: free-list entries are committed table entries.
    let entry = unsafe { &mut *(e as *mut Entry) };
    FREE.store(if entry.flags == END_OF_LIST { 0 } else { entry.flags }, Ordering::Relaxed);
    Some(entry)
}

/// Puts `entry` back on the free list (under the lock).
fn put_entry(entry: &mut Entry) {
    let free = FREE.load(Ordering::Relaxed);
    entry.flags = if free == 0 { END_OF_LIST } else { free };
    entry.ptr = core::ptr::null_mut();
    FREE.store(entry as *mut Entry as usize, Ordering::Relaxed);
}

/// Commits the table's next page and links its entries (under the lock).
fn grow() -> bool {
    let mut base = BASE.load(Ordering::Relaxed);
    if base == 0 {
        let mut size = TABLE_BYTES;
        // SAFETY: a new reservation in this process.
        let status = unsafe {
            NtAllocateVirtualMemory(CURRENT_PROCESS, &mut base, 0, &mut size, 0x2000, 0x04)
        }; // MEM_RESERVE, PAGE_READWRITE
        if status != STATUS_SUCCESS {
            unsupported(b"kernel32.dll!GlobalAlloc(handle table reservation)", status.into());
            return false;
        }
        BASE.store(base, Ordering::Relaxed);
    }
    let committed = COMMITTED.load(Ordering::Relaxed);
    if committed == TABLE_BYTES {
        return false;
    }
    let (mut page, mut size) = (base + committed, PAGE);
    // SAFETY: a page of our own reservation.
    let status =
        unsafe { NtAllocateVirtualMemory(CURRENT_PROCESS, &mut page, 0, &mut size, 0x1000, 0x04) }; // MEM_COMMIT
    if status != STATUS_SUCCESS {
        return false;
    }
    COMMITTED.store(committed + PAGE, Ordering::Relaxed);
    // Linked in reverse so the lowest entry comes out first.
    for i in (0..PAGE / ENTRY).rev() {
        // SAFETY: the page just committed (zeroed).
        put_entry(unsafe { &mut *((base + committed + i * ENTRY) as *mut Entry) });
    }
    true
}

/// Whether `p` (a fixed block) and its heap header are mapped readable.
fn readable(p: *const c_void) -> bool {
    let p = p as usize;
    if p < 0x10000 {
        return false;
    }
    let mapped = |address: usize| {
        let mut info = MemoryBasic::default();
        let mut ret = 0;
        // SAFETY: a MEMORY_BASIC_INFORMATION buffer.
        let status = unsafe {
            NtQueryVirtualMemory(
                CURRENT_PROCESS,
                address,
                0,
                (&raw mut info).cast(),
                size_of::<MemoryBasic>(),
                &mut ret,
            )
        };
        let (state, protect) = (info.state, info.protect);
        // MEM_COMMIT; not PAGE_NOACCESS or PAGE_GUARD.
        status == STATUS_SUCCESS && state == 0x1000 && protect != 0 && protect & 0x101 == 0
    };
    // The heap block's header: two pointers before it.
    let header = p - ENTRY;
    mapped(p) && (header / PAGE == p / PAGE || mapped(header))
}

fn heap() -> Handle {
    GetProcessHeap()
}

/// Entry flags for allocation flags.
fn entry_flags(flags: u32) -> usize {
    let mut e = IN_USE | MOVEABLE;
    if flags & LMEM_DISCARDABLE != 0 {
        e |= DISCARDABLE;
    }
    if flags & GMEM_DDESHARE != 0 {
        e |= DDESHARE;
    }
    e
}

fn zero(flags: u32) -> u32 {
    if flags & GMEM_ZEROINIT != 0 { HEAP_ZERO_MEMORY } else { 0 }
}

fn alloc(flags: u32, bytes: usize) -> Handle {
    if flags & GMEM_MOVEABLE == 0 {
        // A fixed block of at least one byte (GlobalSize of a 0-byte
        // GMEM_FIXED block is 1 on Windows).
        let p = HeapAlloc(heap(), zero(flags), bytes.max(1));
        return if p.is_null() { fail_with(ERROR_NOT_ENOUGH_MEMORY, p) } else { p };
    }
    let data = if bytes == 0 {
        core::ptr::null_mut()
    } else {
        let p = HeapAlloc(heap(), zero(flags), bytes);
        if p.is_null() {
            return fail_with(ERROR_NOT_ENOUGH_MEMORY, p);
        }
        p
    };
    let handle = locked(|| {
        let entry = take_entry()?;
        entry.flags = entry_flags(flags) | if data.is_null() { DISCARDED } else { 0 };
        entry.ptr = data;
        Some(&mut entry.ptr as *mut *mut c_void as Handle)
    });
    match handle {
        Some(h) => h,
        None => {
            if !data.is_null() {
                HeapFree(heap(), 0, data);
            }
            fail_with(ERROR_NOT_ENOUGH_MEMORY, core::ptr::null_mut())
        }
    }
}

fn free(mem: Handle) -> Handle {
    if mem.is_null() {
        return mem;
    }
    if entry_shaped(mem) {
        let data = locked(|| {
            let entry = find(mem)?;
            let data = entry.ptr;
            put_entry(entry);
            Some(data)
        });
        return match data {
            Some(data) => {
                if !data.is_null() {
                    HeapFree(heap(), 0, data);
                }
                core::ptr::null_mut()
            }
            None => fail_with(ERROR_INVALID_HANDLE, mem),
        };
    }
    if !readable(mem) {
        return fail_with(ERROR_NOACCESS, mem);
    }
    if HeapFree(heap(), 0, mem) != FALSE { core::ptr::null_mut() } else { mem }
}

fn lock(mem: Handle, global: bool) -> *mut c_void {
    if entry_shaped(mem) {
        let r = locked(|| {
            let entry = find(mem).ok_or(ERROR_INVALID_HANDLE)?;
            if entry.ptr.is_null() {
                return Err(ERROR_DISCARDED);
            }
            let count = (entry.flags >> LOCK_SHIFT) & LOCK_MAX;
            if count < LOCK_MAX {
                entry.flags += 1 << LOCK_SHIFT;
            }
            Ok(entry.ptr)
        });
        return r.unwrap_or_else(|error| fail_with(error, core::ptr::null_mut()));
    }
    if mem.is_null() || !readable(mem) {
        // Windows' GlobalLock reports the bad pointer; LocalLock does not.
        return if global {
            fail_with(ERROR_INVALID_HANDLE, core::ptr::null_mut())
        } else {
            core::ptr::null_mut()
        };
    }
    mem
}

fn unlock(mem: Handle, global: bool) -> Bool {
    if !entry_shaped(mem) {
        // Fixed memory has no lock count: GlobalUnlock says it is still
        // locked, LocalUnlock that it was not.
        return if global { TRUE } else { fail_with(ERROR_NOT_LOCKED, FALSE) };
    }
    let r = locked(|| {
        let entry = find(mem).ok_or(ERROR_INVALID_HANDLE)?;
        let count = (entry.flags >> LOCK_SHIFT) & LOCK_MAX;
        if count == 0 {
            return Err(ERROR_NOT_LOCKED);
        }
        entry.flags -= 1 << LOCK_SHIFT;
        Ok(count > 1)
    });
    match r {
        Ok(true) => TRUE,
        Ok(false) => fail_with(0, FALSE), // unlocked: NO_ERROR
        // A bad handle: GlobalUnlock still returns TRUE on Windows.
        Err(ERROR_INVALID_HANDLE) if global => fail_with(ERROR_INVALID_HANDLE, TRUE),
        Err(error) => fail_with(error, FALSE),
    }
}

fn size(mem: Handle) -> usize {
    if entry_shaped(mem) {
        let data = locked(|| find(mem).map(|e| e.ptr));
        return match data {
            Some(p) if p.is_null() => 0,
            Some(p) => HeapSize(heap(), 0, p),
            None => fail_with(ERROR_INVALID_HANDLE, 0),
        };
    }
    if mem.is_null() || !readable(mem) {
        return fail_with(ERROR_INVALID_HANDLE, 0);
    }
    match HeapSize(heap(), 0, mem) {
        usize::MAX => fail_with(ERROR_INVALID_HANDLE, 0),
        n => n,
    }
}

fn flags(mem: Handle, global: bool) -> u32 {
    if entry_shaped(mem) {
        let Some(e) = locked(|| find(mem).map(|e| e.flags)) else {
            return fail_with(ERROR_INVALID_HANDLE, GMEM_INVALID_HANDLE);
        };
        let mut f = ((e >> LOCK_SHIFT) & LOCK_MAX) as u32;
        if e & DISCARDABLE != 0 {
            f |= if global { GMEM_DISCARDABLE } else { LMEM_DISCARDABLE };
        }
        if e & DISCARDED != 0 {
            f |= GMEM_DISCARDED;
        }
        if e & DDESHARE != 0 && global {
            f |= GMEM_DDESHARE;
        }
        return f;
    }
    if mem.is_null() || !readable(mem) {
        return fail_with(ERROR_INVALID_HANDLE, GMEM_INVALID_HANDLE);
    }
    0
}

fn handle(p: *const c_void, global: bool) -> Handle {
    if entry_shaped(p) {
        return match locked(|| find(p).is_some()) {
            true => p.cast_mut(),
            false => fail_with(ERROR_INVALID_HANDLE, core::ptr::null_mut()),
        };
    }
    if p.is_null() || !readable(p) {
        let error = if global { ERROR_INVALID_HANDLE } else { ERROR_NOACCESS };
        return fail_with(error, core::ptr::null_mut());
    }
    // The data of a moveable block: the entry that points to it.
    let owner = locked(|| {
        let base = BASE.load(Ordering::Relaxed);
        (0..COMMITTED.load(Ordering::Relaxed) / ENTRY).find_map(|i| {
            // SAFETY: a committed table entry.
            let e = unsafe { &mut *((base + i * ENTRY) as *mut Entry) };
            (e.flags & IN_USE != 0 && e.ptr.cast_const() == p)
                .then_some(&mut e.ptr as *mut *mut c_void as Handle)
        })
    });
    owner.unwrap_or(p.cast_mut())
}

fn realloc(mem: Handle, bytes: usize, flags: u32) -> Handle {
    if mem.is_null() {
        return mem;
    }
    if entry_shaped(mem) {
        // The heap is called under the table's lock: the entry must not
        // change under a reallocation (the heap has its own lock).
        let r = locked(|| {
            let entry = find(mem).ok_or(ERROR_INVALID_HANDLE)?;
            if flags & GMEM_MODIFY != 0 {
                if flags & LMEM_DISCARDABLE != 0 {
                    entry.flags |= DISCARDABLE;
                }
                return Ok(());
            }
            if flags & LMEM_DISCARDABLE != 0 {
                return Err(ERROR_INVALID_PARAMETER);
            }
            let locks = (entry.flags >> LOCK_SHIFT) & LOCK_MAX;
            if bytes == 0 {
                // Discard: only an unlocked block, and only when asked
                // with GMEM_MOVEABLE (GlobalDiscard).
                if locks != 0 || flags & GMEM_MOVEABLE == 0 {
                    return Err(ERROR_INVALID_PARAMETER);
                }
                if !entry.ptr.is_null() {
                    HeapFree(heap(), 0, entry.ptr);
                }
                entry.ptr = core::ptr::null_mut();
                entry.flags |= DISCARDED;
                return Ok(());
            }
            let p = if entry.ptr.is_null() {
                HeapAlloc(heap(), zero(flags), bytes)
            } else {
                // A locked block moves only with GMEM_MOVEABLE.
                let in_place = if locks != 0 && flags & GMEM_MOVEABLE == 0 {
                    HEAP_REALLOC_IN_PLACE_ONLY
                } else {
                    0
                };
                HeapReAlloc(heap(), zero(flags) | in_place, entry.ptr, bytes)
            };
            if p.is_null() {
                return Err(ERROR_NOT_ENOUGH_MEMORY);
            }
            entry.ptr = p;
            entry.flags &= !DISCARDED;
            Ok(())
        });
        return match r {
            Ok(()) => mem,
            Err(error) => fail_with(error, core::ptr::null_mut()),
        };
    }
    if !readable(mem) {
        return fail_with(ERROR_NOACCESS, core::ptr::null_mut());
    }
    if flags & GMEM_MODIFY != 0 {
        if flags & GMEM_MOVEABLE == 0 {
            return mem; // nothing to change for fixed memory
        }
        // Fixed memory made moveable: an entry for the same block.
        let h = locked(|| {
            let entry = take_entry()?;
            entry.flags = entry_flags(flags);
            entry.ptr = mem;
            Some(&mut entry.ptr as *mut *mut c_void as Handle)
        });
        return h.unwrap_or_else(|| fail_with(ERROR_NOT_ENOUGH_MEMORY, core::ptr::null_mut()));
    }
    if flags & LMEM_DISCARDABLE != 0 {
        return fail_with(ERROR_INVALID_PARAMETER, core::ptr::null_mut());
    }
    // Fixed memory moves only with GMEM_MOVEABLE.
    let in_place = if flags & GMEM_MOVEABLE == 0 { HEAP_REALLOC_IN_PLACE_ONLY } else { 0 };
    let p = HeapReAlloc(heap(), zero(flags) | in_place, mem, bytes);
    if p.is_null() { fail_with(ERROR_NOT_ENOUGH_MEMORY, p) } else { p }
}

#[unsafe(no_mangle)]
pub extern "system" fn GlobalAlloc(flags: u32, bytes: usize) -> Handle {
    alloc(flags, bytes)
}

#[unsafe(no_mangle)]
pub extern "system" fn LocalAlloc(flags: u32, bytes: usize) -> Handle {
    // LMEM has no DDESHARE (0x2000 is not an LMEM flag).
    alloc(flags & !GMEM_DDESHARE, bytes)
}

/// GlobalFree: NULL on success, the handle on failure.
#[unsafe(no_mangle)]
pub extern "system" fn GlobalFree(mem: Handle) -> Handle {
    free(mem)
}

/// LocalFree: NULL on success, the handle on failure.
#[unsafe(no_mangle)]
pub extern "system" fn LocalFree(mem: Handle) -> Handle {
    free(mem)
}

#[unsafe(no_mangle)]
pub extern "system" fn GlobalReAlloc(mem: Handle, bytes: usize, flags: u32) -> Handle {
    realloc(mem, bytes, flags)
}

#[unsafe(no_mangle)]
pub extern "system" fn LocalReAlloc(mem: Handle, bytes: usize, flags: u32) -> Handle {
    realloc(mem, bytes, flags & !GMEM_DDESHARE)
}

#[unsafe(no_mangle)]
pub extern "system" fn GlobalLock(mem: Handle) -> *mut c_void {
    lock(mem, true)
}

#[unsafe(no_mangle)]
pub extern "system" fn LocalLock(mem: Handle) -> *mut c_void {
    lock(mem, false)
}

#[unsafe(no_mangle)]
pub extern "system" fn GlobalUnlock(mem: Handle) -> Bool {
    unlock(mem, true)
}

#[unsafe(no_mangle)]
pub extern "system" fn LocalUnlock(mem: Handle) -> Bool {
    unlock(mem, false)
}

#[unsafe(no_mangle)]
pub extern "system" fn GlobalSize(mem: Handle) -> usize {
    size(mem)
}

#[unsafe(no_mangle)]
pub extern "system" fn LocalSize(mem: Handle) -> usize {
    size(mem)
}

#[unsafe(no_mangle)]
pub extern "system" fn GlobalFlags(mem: Handle) -> u32 {
    flags(mem, true)
}

#[unsafe(no_mangle)]
pub extern "system" fn LocalFlags(mem: Handle) -> u32 {
    flags(mem, false)
}

#[unsafe(no_mangle)]
pub extern "system" fn GlobalHandle(p: *const c_void) -> Handle {
    handle(p, true)
}

#[unsafe(no_mangle)]
pub extern "system" fn LocalHandle(p: *const c_void) -> Handle {
    handle(p, false)
}

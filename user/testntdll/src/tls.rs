//! Static TLS (the PE TLS directory) and thread notifications, as the PE
//! format and "Thread Local Storage" document them.
//!
//! Each image with a TLS directory gets an index when it is loaded, written
//! to its `AddressOfIndex`. Each thread has an array of block pointers at
//! TEB.ThreadLocalStoragePointer (gs:[0x58]); a block is a copy of the
//! image's template followed by `SizeOfZeroFill` zero bytes. A DLL loaded
//! later gets a block in every thread that already exists; new threads get
//! blocks for every image when they start (`GalaInitializeThread`).
//!
//! Thread notifications: a starting thread calls each image's TLS
//! callbacks and each DLL's DllMain with DLL_THREAD_ATTACH (in load order;
//! not for DLLs that called DisableThreadLibraryCalls), an exiting thread
//! with DLL_THREAD_DETACH in reverse order (`LdrShutdownThread`, as
//! kernel32's ExitThread calls it on Windows). Threads that existed before
//! a DLL was loaded get no DLL_THREAD_ATTACH from it, but its detach.
//!
//! Everything here runs under the loader lock (`loader_lock`).

use crate::{
    GalaRecordMissing, MODULE_COUNT, directory, entry_point, heap, rd32, rd64, with_modules,
};

pub const DLL_PROCESS_ATTACH: u32 = 1;
pub const DLL_THREAD_ATTACH: u32 = 2;
pub const DLL_THREAD_DETACH: u32 = 3;

/// TLS indexes (images with TLS in the process's lifetime).
const MAX_TLS: usize = 64;
/// Threads with a TLS array.
const MAX_THREADS: usize = 1024;

struct State {
    /// The image owning each index (0: none or unloaded).
    owners: [usize; MAX_TLS],
    next: usize,
    /// The TLS arrays of the threads (0: free).
    threads: [usize; MAX_THREADS],
}

struct Shared(core::cell::UnsafeCell<State>);

// SAFETY: only used under the loader lock.
unsafe impl Sync for Shared {}

static STATE: Shared = Shared(core::cell::UnsafeCell::new(State {
    owners: [0; MAX_TLS],
    next: 0,
    threads: [0; MAX_THREADS],
}));

fn state() -> &'static mut State {
    // SAFETY: the loader lock serializes every caller.
    unsafe { &mut *STATE.0.get() }
}

/// An image's IMAGE_TLS_DIRECTORY64: (template start, template end, the
/// index's address, the callback array's address, zero fill).
fn tls_directory(base: usize) -> Option<(usize, usize, usize, usize, usize)> {
    let (rva, size) = directory(base, 9); // IMAGE_DIRECTORY_ENTRY_TLS
    if rva == 0 || size < 40 {
        return None;
    }
    let d = base + rva;
    Some((
        rd64(d) as usize,
        rd64(d + 8) as usize,
        rd64(d + 16) as usize,
        rd64(d + 24) as usize,
        rd32(d + 32) as usize,
    ))
}

fn process_heap() -> *mut u8 {
    // SAFETY: gs:[0x60] is the PEB; ProcessHeap is at +0x30.
    unsafe {
        let peb: usize;
        core::arch::asm!("mov {}, gs:[0x60]", out(reg) peb, options(nostack, readonly));
        rd64(peb + 0x30) as *mut u8
    }
}

/// A new block for the image at `base`: its template, then zeros.
fn new_block(base: usize) -> usize {
    let Some((start, end, _, _, zero_fill)) = tls_directory(base) else { return 0 };
    let template = end.saturating_sub(start);
    let block = heap::RtlAllocateHeap(process_heap(), 0x8, (template + zero_fill).max(16)); // zeroed
    if !block.is_null() {
        // SAFETY: the template inside the image; a block of its size.
        unsafe { core::ptr::copy_nonoverlapping(start as *const u8, block, template) };
    }
    block as usize
}

fn current_array() -> usize {
    let array: usize;
    // SAFETY: gs:[0x58] is TEB.ThreadLocalStoragePointer.
    unsafe { core::arch::asm!("mov {}, gs:[0x58]", out(reg) array, options(nostack, readonly)) };
    array
}

/// Gives the current thread its TLS array and a block for every image with
/// TLS (those it does not have yet).
pub fn thread_init() {
    let s = state();
    let mut array = current_array();
    if array == 0 {
        array = heap::RtlAllocateHeap(process_heap(), 0x8, 8 * MAX_TLS) as usize;
        if array == 0 {
            return;
        }
        let Some(free) = s.threads.iter_mut().find(|t| **t == 0) else {
            let what = b"ntdll.dll!TLS(too many threads)";
            GalaRecordMissing(what.as_ptr(), what.len(), 2, MAX_THREADS as u64);
            return;
        };
        *free = array;
        // SAFETY: TEB.ThreadLocalStoragePointer of this thread.
        unsafe { core::arch::asm!("mov gs:[0x58], {}", in(reg) array, options(nostack)) };
    }
    for (index, &owner) in s.owners.iter().enumerate() {
        let slot = (array + 8 * index) as *mut usize;
        // SAFETY: inside this thread's array.
        unsafe {
            if owner != 0 && *slot == 0 {
                *slot = new_block(owner);
            }
        }
    }
}

/// Assigns the image at `base` its TLS index, if it has a TLS directory,
/// and gives every thread a block; whether it has TLS.
pub fn register(base: usize) -> bool {
    let Some((_, _, index_at, _, _)) = tls_directory(base) else { return false };
    let s = state();
    if s.next >= MAX_TLS {
        let what = b"ntdll.dll!TLS(too many images)";
        GalaRecordMissing(what.as_ptr(), what.len(), 2, MAX_TLS as u64);
        return false;
    }
    let index = s.next;
    s.next += 1;
    s.owners[index] = base;
    // SAFETY: the image's _tls_index (writable data).
    unsafe { *(index_at as *mut u32) = index as u32 };
    for &array in s.threads.iter().filter(|&&a| a != 0) {
        // SAFETY: a thread's array of MAX_TLS slots.
        unsafe { *((array + 8 * index) as *mut usize) = new_block(base) };
    }
    true
}

/// Forgets an unloaded image's TLS (its blocks are freed in every thread;
/// the index is not reused).
pub fn unregister(base: usize) {
    let s = state();
    for index in 0..s.next {
        if s.owners[index] != base {
            continue;
        }
        s.owners[index] = 0;
        for &array in s.threads.iter().filter(|&&a| a != 0) {
            let slot = (array + 8 * index) as *mut usize;
            // SAFETY: a thread's array; the block came from the heap.
            unsafe {
                if *slot != 0 {
                    heap::RtlFreeHeap(process_heap(), 0, *slot as *mut u8);
                    *slot = 0;
                }
            }
        }
    }
}

/// Calls the image's TLS callbacks with `reason`.
pub fn callbacks(base: usize, reason: u32) {
    let Some((_, _, _, list, _)) = tls_directory(base) else { return };
    if list == 0 {
        return;
    }
    let mut at = list;
    loop {
        let f = rd64(at) as usize;
        if f == 0 {
            break;
        }
        // SAFETY: a PIMAGE_TLS_CALLBACK the image lists.
        let f: extern "system" fn(usize, u32, usize) = unsafe { core::mem::transmute(f) };
        f(base, reason, 0);
        at += 8;
    }
}

/// Frees the current thread's TLS blocks and array.
fn thread_exit() {
    let array = current_array();
    if array == 0 {
        return;
    }
    let s = state();
    for index in 0..MAX_TLS {
        let slot = (array + 8 * index) as *const usize;
        // SAFETY: this thread's array; blocks came from the heap.
        unsafe {
            if *slot != 0 {
                heap::RtlFreeHeap(process_heap(), 0, *slot as *mut u8);
            }
        }
    }
    if let Some(t) = s.threads.iter_mut().find(|t| **t == array) {
        *t = 0;
    }
    heap::RtlFreeHeap(process_heap(), 0, array as *mut u8);
    // SAFETY: TEB.ThreadLocalStoragePointer of this thread.
    unsafe { core::arch::asm!("mov qword ptr gs:[0x58], 0", options(nostack)) };
}

/// The loaded modules as (base, whether DllMain gets thread
/// notifications), in load order.
fn modules(out: &mut [(usize, bool); MODULE_COUNT]) -> usize {
    with_modules(|m| {
        let mut n = 0;
        for module in m.iter().filter(|m| m.base != 0) {
            out[n] = (module.base, module.attached && !module.no_thread_calls);
            n += 1;
        }
        n
    })
}

/// A new thread: its TLS blocks, then DLL_THREAD_ATTACH to every image's
/// TLS callbacks and DLL's DllMain. gala's kernel32 calls it first thing in
/// every thread it starts (Windows does this in LdrInitializeThunk).
#[unsafe(no_mangle)]
pub extern "system" fn GalaInitializeThread() {
    crate::with_loader_lock(|| {
        thread_init();
        let mut list = [(0usize, false); MODULE_COUNT];
        let n = modules(&mut list);
        for &(base, dll_main) in &list[..n] {
            callbacks(base, DLL_THREAD_ATTACH);
            if dll_main && let Some(entry) = entry_point(base) {
                entry(base, DLL_THREAD_ATTACH, 0);
            }
        }
    });
}

/// LdrShutdownThread: DLL_THREAD_DETACH to every DLL's DllMain and image's
/// TLS callbacks (in reverse load order), then the thread's TLS is freed.
#[unsafe(no_mangle)]
pub extern "system" fn LdrShutdownThread() {
    crate::with_loader_lock(|| {
        let mut list = [(0usize, false); MODULE_COUNT];
        let n = modules(&mut list);
        for &(base, dll_main) in list[..n].iter().rev() {
            if dll_main && let Some(entry) = entry_point(base) {
                entry(base, DLL_THREAD_DETACH, 0);
            }
            callbacks(base, DLL_THREAD_DETACH);
        }
        thread_exit();
    });
}

/// LdrDisableThreadCalloutsForDll: no DLL_THREAD_ATTACH/DETACH to this
/// DLL's DllMain (its TLS callbacks still run).
#[unsafe(no_mangle)]
pub extern "system" fn LdrDisableThreadCalloutsForDll(base: usize) -> u32 {
    crate::with_loader_lock(|| {
        with_modules(|m| match m.iter_mut().find(|m| m.base == base && base != 0) {
            Some(module) => {
                module.no_thread_calls = true;
                0
            }
            None => 0xc000_0135, // STATUS_DLL_NOT_FOUND
        })
    })
}

/// The process's first thread: its TLS array, and the executable's TLS
/// (before its imports load).
pub fn process_init(image: usize) {
    thread_init();
    register(image);
}

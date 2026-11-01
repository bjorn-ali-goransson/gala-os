//! The NT heap API (RtlCreateHeap, RtlAllocateHeap, ...), on which
//! kernel32's Heap* functions sit, and the process heap that
//! LdrInitializeThunk creates and records in PEB.ProcessHeap.
//!
//! A heap is a 4 KiB control page (from NtAllocateVirtualMemory) with free
//! lists of power-of-two blocks from 32 bytes to 64 KiB, carved from 1 MiB
//! regions; larger requests get their own allocation. Every block starts
//! with a 16-byte header, so the caller's memory is 16-byte aligned:
//! (class or LARGE, the size asked for), which RtlSizeHeap reports. Large
//! blocks have 16 more bytes before that header linking them into the
//! heap's list, so RtlDestroyHeap can release everything. A spinlock per
//! heap serializes it (HEAP_NO_SERIALIZE is not needed for correctness).

use core::ptr::null_mut;
use core::sync::atomic::{AtomicBool, Ordering};

use super::{NtAllocateVirtualMemory, NtFreeVirtualMemory};

const MEM_COMMIT_RESERVE: u32 = 0x3000;
const MEM_RELEASE: u32 = 0x8000;
const PAGE_READWRITE: u32 = 4;
const CURRENT_PROCESS: isize = -1;

const HEAP_ZERO_MEMORY: u32 = 0x8;
const HEAP_REALLOC_IN_PLACE_ONLY: u32 = 0x10;

const HEADER: usize = 16;
/// Large blocks: the list links, then the usual header.
const LARGE_HEADER: usize = 32;
const MIN_SHIFT: u32 = 5;
const MAX_SHIFT: u32 = 16;
const CLASSES: usize = (MAX_SHIFT - MIN_SHIFT + 1) as usize;
const REGION: usize = 1 << 20;
const LARGE: u64 = u64::MAX;
const MAGIC: u64 = u64::from_le_bytes(*b"galaheap");

#[repr(C)]
struct Heap {
    magic: u64,
    lock: AtomicBool,
    free: [*mut u8; CLASSES],
    next: usize,
    end: usize,
    /// Regions, each starting with a pointer to the next.
    regions: *mut u8,
    /// Large blocks (the start of their allocation): next, prev.
    large: *mut u8,
}

fn alloc_pages(size: usize) -> *mut u8 {
    let (mut base, mut len) = (0usize, size);
    let status = NtAllocateVirtualMemory(
        CURRENT_PROCESS,
        &mut base,
        0,
        &mut len,
        MEM_COMMIT_RESERVE,
        PAGE_READWRITE,
    );
    if status == 0 { base as *mut u8 } else { null_mut() }
}

fn free_pages(p: *mut u8) {
    let (mut base, mut len) = (p as usize, 0usize);
    NtFreeVirtualMemory(CURRENT_PROCESS, &mut base, &mut len, MEM_RELEASE);
}

/// The heap behind a handle, if it is one of ours.
fn heap<'a>(h: *mut u8) -> Option<&'a mut Heap> {
    if h.is_null() {
        return None;
    }
    // SAFETY: heap handles point to their control page.
    let heap = unsafe { &mut *h.cast::<Heap>() };
    (heap.magic == MAGIC).then_some(heap)
}

fn with_lock<R>(heap: &mut Heap, f: impl FnOnce(&mut Heap) -> R) -> R {
    while heap
        .lock
        .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        core::hint::spin_loop();
    }
    let r = f(heap);
    heap.lock.store(false, Ordering::Release);
    r
}

fn class_of(size: usize) -> Option<usize> {
    let block = size.checked_add(HEADER)?.max(1 << MIN_SHIFT).checked_next_power_of_two()?;
    let shift = block.trailing_zeros();
    (shift <= MAX_SHIFT).then(|| (shift - MIN_SHIFT) as usize)
}

fn class_size(class: usize) -> usize {
    1 << (class as u32 + MIN_SHIFT)
}

/// A block of class `class`: from its free list or the current region.
fn take(heap: &mut Heap, class: usize) -> *mut u8 {
    let head = heap.free[class];
    if !head.is_null() {
        // SAFETY: free blocks keep the next free block after the header.
        heap.free[class] = unsafe { *head.add(HEADER).cast::<*mut u8>() };
        return head;
    }
    let size = class_size(class);
    if heap.end - heap.next < size {
        let region = alloc_pages(REGION);
        if region.is_null() {
            return null_mut();
        }
        // SAFETY: the region's first 16 bytes link the regions.
        unsafe { *region.cast::<*mut u8>() = heap.regions };
        heap.regions = region;
        heap.next = region as usize + HEADER;
        heap.end = region as usize + REGION;
    }
    let block = heap.next as *mut u8;
    heap.next += size;
    block
}

/// RtlCreateHeap(Flags, HeapBase, ReserveSize, CommitSize, Lock,
/// Parameters): a new growable heap (the sizes and a caller's base are
/// not used: the heap grows by regions as needed).
#[unsafe(no_mangle)]
pub extern "system" fn RtlCreateHeap(
    _flags: u32,
    _base: *mut u8,
    _reserve: usize,
    _commit: usize,
    _lock: *mut u8,
    _parameters: *mut u8,
) -> *mut u8 {
    let page = alloc_pages(4096);
    if page.is_null() {
        return page;
    }
    let heap = Heap {
        magic: MAGIC,
        lock: AtomicBool::new(false),
        free: [null_mut(); CLASSES],
        next: 0,
        end: 0,
        regions: null_mut(),
        large: null_mut(),
    };
    // SAFETY: a fresh page, large enough.
    unsafe { page.cast::<Heap>().write(heap) };
    page
}

/// RtlDestroyHeap: releases the heap and everything in it; NULL on
/// success, else the handle (as for the process heap, which stays).
#[unsafe(no_mangle)]
pub extern "system" fn RtlDestroyHeap(h: *mut u8) -> *mut u8 {
    let Some(heap) = heap(h) else { return h };
    if h == process_heap() {
        return h;
    }
    let (mut region, mut large) = (heap.regions, heap.large);
    heap.magic = 0;
    // SAFETY: the heap's own lists of its allocations.
    unsafe {
        while !region.is_null() {
            let next = *region.cast::<*mut u8>();
            free_pages(region);
            region = next;
        }
        while !large.is_null() {
            let next = *large.cast::<*mut u8>();
            free_pages(large);
            large = next;
        }
    }
    free_pages(h);
    null_mut()
}

/// RtlAllocateHeap(HeapHandle, Flags, Size): 16-byte aligned memory, zeroed
/// with HEAP_ZERO_MEMORY; NULL if there is none.
#[unsafe(no_mangle)]
pub extern "system" fn RtlAllocateHeap(h: *mut u8, flags: u32, size: usize) -> *mut u8 {
    let Some(heap) = heap(h) else { return null_mut() };
    let p = match class_of(size) {
        Some(class) => {
            let block = with_lock(heap, |heap| take(heap, class));
            if block.is_null() {
                return block;
            }
            // SAFETY: a block of class_size(class) bytes.
            unsafe {
                *block.cast::<u64>() = class as u64;
                *block.add(8).cast::<u64>() = size as u64;
                block.add(HEADER)
            }
        }
        None => {
            let Some(total) = size.checked_add(LARGE_HEADER) else { return null_mut() };
            let start = alloc_pages(total);
            if start.is_null() {
                return start;
            }
            // SAFETY: a fresh allocation of `total` bytes; the list is the
            // heap's, under its lock.
            unsafe {
                with_lock(heap, |heap| {
                    *start.cast::<*mut u8>() = heap.large;
                    *start.add(8).cast::<*mut u8>() = null_mut();
                    if !heap.large.is_null() {
                        *heap.large.add(8).cast::<*mut u8>() = start;
                    }
                    heap.large = start;
                });
                *start.add(16).cast::<u64>() = LARGE;
                *start.add(24).cast::<u64>() = size as u64;
                // Fresh pages are zero already.
                return start.add(LARGE_HEADER);
            }
        }
    };
    if flags & HEAP_ZERO_MEMORY != 0 {
        // SAFETY: `size` bytes were allocated.
        unsafe { core::ptr::write_bytes(p, 0, size) };
    }
    p
}

/// (tag, size asked for) of an allocated block.
fn header(p: *const u8) -> (u64, usize) {
    // SAFETY: allocated blocks are preceded by their header.
    unsafe { (*p.sub(HEADER).cast::<u64>(), *p.sub(8).cast::<u64>() as usize) }
}

/// RtlFreeHeap: TRUE (NULL frees nothing and succeeds).
#[unsafe(no_mangle)]
pub extern "system" fn RtlFreeHeap(h: *mut u8, _flags: u32, p: *mut u8) -> u8 {
    if p.is_null() {
        return 1;
    }
    let Some(heap) = heap(h) else { return 0 };
    let (tag, _) = header(p);
    if tag == LARGE {
        // SAFETY: a large block: its links precede the header.
        unsafe {
            let start = p.sub(LARGE_HEADER);
            with_lock(heap, |heap| {
                let next = *start.cast::<*mut u8>();
                let prev = *start.add(8).cast::<*mut u8>();
                if prev.is_null() {
                    heap.large = next;
                } else {
                    *prev.cast::<*mut u8>() = next;
                }
                if !next.is_null() {
                    *next.add(8).cast::<*mut u8>() = prev;
                }
            });
            free_pages(start);
        }
        return 1;
    }
    if tag as usize >= CLASSES {
        return 0;
    }
    with_lock(heap, |heap| {
        // SAFETY: the block goes back on its free list.
        unsafe {
            let block = p.sub(HEADER);
            *p.cast::<*mut u8>() = heap.free[tag as usize];
            heap.free[tag as usize] = block;
        }
    });
    1
}

/// RtlSizeHeap: the size asked for at allocation, or (SIZE_T)-1.
#[unsafe(no_mangle)]
pub extern "system" fn RtlSizeHeap(h: *mut u8, _flags: u32, p: *const u8) -> usize {
    if heap(h).is_none() || p.is_null() {
        return usize::MAX;
    }
    header(p).1
}

/// RtlReAllocateHeap: the block resized (in place when it fits, else
/// moved unless HEAP_REALLOC_IN_PLACE_ONLY); growth is zeroed with
/// HEAP_ZERO_MEMORY.
#[unsafe(no_mangle)]
pub extern "system" fn RtlReAllocateHeap(
    h: *mut u8,
    flags: u32,
    p: *mut u8,
    size: usize,
) -> *mut u8 {
    if p.is_null() || heap(h).is_none() {
        return null_mut();
    }
    let (tag, old) = header(p);
    let capacity = if tag == LARGE {
        // Large blocks: their pages, rounded up.
        (old + LARGE_HEADER).next_multiple_of(4096) - LARGE_HEADER
    } else {
        class_size(tag as usize) - HEADER
    };
    if size <= capacity {
        // SAFETY: the block holds `capacity` bytes; its header records
        // the new size.
        unsafe {
            if size > old && flags & HEAP_ZERO_MEMORY != 0 {
                core::ptr::write_bytes(p.add(old), 0, size - old);
            }
            *p.sub(8).cast::<u64>() = size as u64;
        }
        return p;
    }
    if flags & HEAP_REALLOC_IN_PLACE_ONLY != 0 {
        return null_mut();
    }
    let q = RtlAllocateHeap(h, flags, size);
    if !q.is_null() {
        // SAFETY: both blocks hold at least `old` bytes.
        unsafe { core::ptr::copy_nonoverlapping(p, q, old) };
        RtlFreeHeap(h, 0, p);
    }
    q
}

/// RtlValidateHeap: TRUE for one of our heaps (blocks are not walked).
#[unsafe(no_mangle)]
pub extern "system" fn RtlValidateHeap(h: *mut u8, _flags: u32, _p: *const u8) -> u8 {
    u8::from(heap(h).is_some())
}

/// RtlCompactHeap: the largest free block, here what is left of the
/// current region (nothing is returned to the system).
#[unsafe(no_mangle)]
pub extern "system" fn RtlCompactHeap(h: *mut u8, _flags: u32) -> usize {
    heap(h).map_or(0, |heap| heap.end - heap.next)
}

/// PEB.ProcessHeap.
pub(crate) fn process_heap() -> *mut u8 {
    // SAFETY: gs:[0x60] is the PEB; ProcessHeap is at +0x30.
    unsafe {
        let peb: usize;
        core::arch::asm!("mov {}, gs:[0x60]", out(reg) peb, options(nostack, readonly));
        *((peb + 0x30) as *const *mut u8)
    }
}

/// Creates the process heap (LdrInitializeThunk, before any DLL runs).
pub fn create_process_heap() {
    let h = RtlCreateHeap(0, null_mut(), 0, 0, null_mut(), null_mut());
    // SAFETY: the PEB is writable by its process.
    unsafe {
        let peb: usize;
        core::arch::asm!("mov {}, gs:[0x60]", out(reg) peb, options(nostack, readonly));
        *((peb + 0x30) as *mut *mut u8) = h;
    }
}

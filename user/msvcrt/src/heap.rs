//! malloc and friends. Blocks are powers of two from 32 bytes to 64 KiB
//! (a 16-byte header, then the caller's bytes, 16-byte aligned), carved
//! from 1 MiB regions of VirtualAlloc'd memory and recycled through one
//! free list per size; larger requests get their own VirtualAlloc. One
//! spinlock guards it all. Memory is not returned to the system except
//! for large blocks.

use crate::Lock;
use core::ffi::c_void;
use core::ptr::null_mut;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn VirtualAlloc(address: *mut c_void, size: usize, kind: u32, protect: u32) -> *mut c_void;
    fn VirtualFree(address: *mut c_void, size: usize, kind: u32) -> i32;
}

const MEM_COMMIT_RESERVE: u32 = 0x3000;
const MEM_RELEASE: u32 = 0x8000;
const PAGE_READWRITE: u32 = 4;

const HEADER: usize = 16;
const MIN_SHIFT: u32 = 5; // 32-byte blocks
const MAX_SHIFT: u32 = 16; // 64 KiB blocks
const CLASSES: usize = (MAX_SHIFT - MIN_SHIFT + 1) as usize;
const REGION: usize = 1 << 20;
/// Header tag of a block with its own VirtualAlloc.
const LARGE: u64 = u64::MAX;

struct Heap {
    free: [*mut u8; CLASSES],
    next: usize,
    end: usize,
}

static mut HEAP: Heap = Heap { free: [null_mut(); CLASSES], next: 0, end: 0 };
static LOCK: Lock = Lock::new();

fn class_of(size: usize) -> Option<usize> {
    let block = size.checked_add(HEADER)?.max(1 << MIN_SHIFT).checked_next_power_of_two()?;
    let shift = block.trailing_zeros();
    (shift <= MAX_SHIFT).then(|| (shift - MIN_SHIFT) as usize)
}

/// A new block of class `class` (from its free list or a region).
fn take(heap: &mut Heap, class: usize) -> *mut u8 {
    let head = heap.free[class];
    if !head.is_null() {
        // SAFETY: free blocks store the next free block in their body.
        heap.free[class] = unsafe { *head.add(HEADER).cast::<*mut u8>() };
        return head;
    }
    let size = 1usize << (class as u32 + MIN_SHIFT);
    if heap.end - heap.next < size {
        // SAFETY: plain allocation call.
        let region =
            unsafe { VirtualAlloc(null_mut(), REGION, MEM_COMMIT_RESERVE, PAGE_READWRITE) };
        if region.is_null() {
            return null_mut();
        }
        heap.next = region as usize;
        heap.end = heap.next + REGION;
    }
    let block = heap.next as *mut u8;
    heap.next += size;
    block
}

#[unsafe(no_mangle)]
pub extern "C" fn malloc(size: usize) -> *mut u8 {
    let block = match class_of(size) {
        Some(class) => {
            // SAFETY: the heap state, under LOCK.
            let block = LOCK.with(|| take(unsafe { &mut *(&raw mut HEAP) }, class));
            if block.is_null() {
                return block;
            }
            // SAFETY: a block of at least HEADER + size bytes.
            unsafe { *block.cast::<u64>() = class as u64 };
            block
        }
        None => {
            let Some(total) = size.checked_add(HEADER) else { return null_mut() };
            // SAFETY: plain allocation call.
            let block =
                unsafe { VirtualAlloc(null_mut(), total, MEM_COMMIT_RESERVE, PAGE_READWRITE) }
                    .cast::<u8>();
            if block.is_null() {
                return block;
            }
            // SAFETY: as above.
            unsafe {
                *block.cast::<u64>() = LARGE;
                *block.add(8).cast::<u64>() = size as u64;
            }
            block
        }
    };
    // SAFETY: inside the block.
    unsafe { block.add(HEADER) }
}

/// The caller's usable size of an allocated block.
fn usable(p: *mut u8) -> usize {
    // SAFETY: `p` came from malloc: its header precedes it.
    unsafe {
        let block = p.sub(HEADER);
        match *block.cast::<u64>() {
            LARGE => *block.add(8).cast::<u64>() as usize,
            class => (1usize << (class as u32 + MIN_SHIFT)) - HEADER,
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn free(p: *mut u8) {
    if p.is_null() {
        return;
    }
    // SAFETY: `p` came from malloc.
    unsafe {
        let block = p.sub(HEADER);
        let tag = *block.cast::<u64>();
        if tag == LARGE {
            VirtualFree(block.cast(), 0, MEM_RELEASE);
            return;
        }
        LOCK.with(|| {
            let heap = &mut *(&raw mut HEAP);
            *p.cast::<*mut u8>() = heap.free[tag as usize];
            heap.free[tag as usize] = block;
        });
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn calloc(count: usize, size: usize) -> *mut u8 {
    let Some(total) = count.checked_mul(size) else { return null_mut() };
    let p = malloc(total);
    if !p.is_null() {
        // SAFETY: `total` bytes were allocated.
        unsafe { crate::memset(p, 0, total) };
    }
    p
}

#[unsafe(no_mangle)]
pub extern "C" fn realloc(p: *mut u8, size: usize) -> *mut u8 {
    if p.is_null() {
        return malloc(size);
    }
    if size == 0 {
        free(p);
        return null_mut();
    }
    let have = usable(p);
    if size <= have {
        return p;
    }
    let q = malloc(size);
    if !q.is_null() {
        // SAFETY: both blocks hold at least `have` bytes.
        unsafe { crate::memcpy(q, p, have) };
        free(p);
    }
    q
}

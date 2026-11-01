//! The process heap (kernel32's HeapAlloc) as Rust's global allocator,
//! for gala's DLLs whose dependencies use `alloc`: bcrypt (the `rsa`
//! crate) and, because Cargo unifies features across one build, every
//! DLL whose RustCrypto `digest` then has its `alloc` feature (secur32).
//! A DLL links it with `use gala_heap as _;`.
//!
//! The heap aligns to 16; larger alignments over-allocate and keep the
//! original pointer just below the aligned block. Allocation failure in
//! `alloc` users ends in the DLL's panic handler, so DLLs keep their own
//! data on checked HeapAlloc calls and use this only for such crates.

#![no_std]

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetProcessHeap() -> usize;
    fn HeapAlloc(heap: usize, flags: u32, size: usize) -> *mut u8;
    fn HeapFree(heap: usize, flags: u32, p: *mut u8) -> i32;
}

struct ProcessHeap;

// SAFETY: HeapAlloc/HeapFree on the process heap are thread-safe and
// return memory valid for the requested size (null on failure).
unsafe impl core::alloc::GlobalAlloc for ProcessHeap {
    unsafe fn alloc(&self, layout: core::alloc::Layout) -> *mut u8 {
        // SAFETY: plain calls; the arithmetic stays inside the block.
        unsafe {
            if layout.align() <= 16 {
                return HeapAlloc(GetProcessHeap(), 0, layout.size().max(1));
            }
            let raw = HeapAlloc(GetProcessHeap(), 0, layout.size() + layout.align() + 8);
            if raw.is_null() {
                return raw;
            }
            let aligned = (raw as usize + 8).next_multiple_of(layout.align()) as *mut u8;
            aligned.cast::<*mut u8>().sub(1).write(raw);
            aligned
        }
    }

    unsafe fn dealloc(&self, p: *mut u8, layout: core::alloc::Layout) {
        // SAFETY: a block from `alloc` with this layout.
        unsafe {
            let raw = if layout.align() <= 16 { p } else { p.cast::<*mut u8>().sub(1).read() };
            HeapFree(GetProcessHeap(), 0, raw);
        }
    }
}

#[global_allocator]
static HEAP: ProcessHeap = ProcessHeap;

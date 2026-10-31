//! Heaps: kernel32's Heap* functions on ntdll's Rtl*Heap, as on Windows;
//! the process heap is the one ntdll creates at startup (PEB.ProcessHeap).

use super::*;

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn RtlCreateHeap(
        flags: u32,
        base: *mut c_void,
        reserve: usize,
        commit: usize,
        lock: *mut c_void,
        parameters: *mut c_void,
    ) -> Handle;
    fn RtlDestroyHeap(heap: Handle) -> Handle;
    fn RtlAllocateHeap(heap: Handle, flags: u32, size: usize) -> *mut c_void;
    fn RtlReAllocateHeap(heap: Handle, flags: u32, p: *mut c_void, size: usize) -> *mut c_void;
    fn RtlFreeHeap(heap: Handle, flags: u32, p: *mut c_void) -> u8;
    fn RtlSizeHeap(heap: Handle, flags: u32, p: *const c_void) -> usize;
    fn RtlValidateHeap(heap: Handle, flags: u32, p: *const c_void) -> u8;
    fn RtlCompactHeap(heap: Handle, flags: u32) -> usize;
}

/// HEAP_GROWABLE: a heap without a maximum size grows.
const HEAP_GROWABLE: u32 = 0x2;

#[unsafe(no_mangle)]
pub extern "system" fn GetProcessHeap() -> Handle {
    // SAFETY: PEB (gs:[0x60]) -> ProcessHeap (+0x30).
    unsafe { *((teb_u64(0x60) as usize + 0x30) as *const Handle) }
}

#[unsafe(no_mangle)]
pub extern "system" fn HeapCreate(options: u32, initial: usize, maximum: usize) -> Handle {
    let flags = options | if maximum == 0 { HEAP_GROWABLE } else { 0 };
    let null = core::ptr::null_mut();
    // SAFETY: no caller memory.
    let h = unsafe { RtlCreateHeap(flags, null, maximum, initial, null, null) };
    if h.is_null() { fail_with(ERROR_NOT_ENOUGH_MEMORY, h) } else { h }
}

#[unsafe(no_mangle)]
pub extern "system" fn HeapDestroy(heap: Handle) -> Bool {
    // SAFETY: the caller's heap handle.
    if unsafe { RtlDestroyHeap(heap) }.is_null() {
        TRUE
    } else {
        fail_with(ERROR_INVALID_HANDLE, FALSE)
    }
}

/// HeapAlloc: as documented, it does not set the last error.
#[unsafe(no_mangle)]
pub extern "system" fn HeapAlloc(heap: Handle, flags: u32, bytes: usize) -> *mut c_void {
    // SAFETY: the caller's heap handle.
    unsafe { RtlAllocateHeap(heap, flags, bytes) }
}

#[unsafe(no_mangle)]
pub extern "system" fn HeapReAlloc(
    heap: Handle,
    flags: u32,
    p: *mut c_void,
    bytes: usize,
) -> *mut c_void {
    // SAFETY: the caller's heap and block.
    unsafe { RtlReAllocateHeap(heap, flags, p, bytes) }
}

#[unsafe(no_mangle)]
pub extern "system" fn HeapFree(heap: Handle, flags: u32, p: *mut c_void) -> Bool {
    // SAFETY: the caller's heap and block.
    if unsafe { RtlFreeHeap(heap, flags, p) } != 0 {
        TRUE
    } else {
        fail_with(ERROR_INVALID_PARAMETER, FALSE)
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn HeapSize(heap: Handle, flags: u32, p: *const c_void) -> usize {
    // SAFETY: the caller's heap and block.
    unsafe { RtlSizeHeap(heap, flags, p) }
}

#[unsafe(no_mangle)]
pub extern "system" fn HeapValidate(heap: Handle, flags: u32, p: *const c_void) -> Bool {
    // SAFETY: the caller's heap.
    Bool::from(unsafe { RtlValidateHeap(heap, flags, p) } != 0)
}

#[unsafe(no_mangle)]
pub extern "system" fn HeapCompact(heap: Handle, flags: u32) -> usize {
    // SAFETY: the caller's heap.
    unsafe { RtlCompactHeap(heap, flags) }
}

// ---- Local memory (LocalAlloc and friends) ----
//
// On the process heap. LMEM_MOVEABLE memory is fixed too (the handle is
// the pointer, and LocalLock returns it), which programs cannot tell
// apart through the API.

const LMEM_ZEROINIT: u32 = 0x40;

#[unsafe(no_mangle)]
pub extern "system" fn LocalAlloc(flags: u32, bytes: usize) -> *mut c_void {
    let heap_flags = if flags & LMEM_ZEROINIT != 0 { 0x8 } else { 0 }; // HEAP_ZERO_MEMORY
    HeapAlloc(GetProcessHeap(), heap_flags, bytes)
}

/// LocalFree: NULL on success, the memory on failure.
#[unsafe(no_mangle)]
pub extern "system" fn LocalFree(memory: *mut c_void) -> *mut c_void {
    if memory.is_null() || HeapFree(GetProcessHeap(), 0, memory) != FALSE {
        core::ptr::null_mut()
    } else {
        memory
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn LocalReAlloc(memory: *mut c_void, bytes: usize, flags: u32) -> *mut c_void {
    let heap_flags = if flags & LMEM_ZEROINIT != 0 { 0x8 } else { 0 };
    HeapReAlloc(GetProcessHeap(), heap_flags, memory, bytes)
}

#[unsafe(no_mangle)]
pub extern "system" fn LocalSize(memory: *mut c_void) -> usize {
    HeapSize(GetProcessHeap(), 0, memory)
}

#[unsafe(no_mangle)]
pub extern "system" fn LocalLock(memory: *mut c_void) -> *mut c_void {
    memory
}

#[unsafe(no_mangle)]
pub extern "system" fn LocalUnlock(_memory: *mut c_void) -> Bool {
    FALSE // the memory is not moveable: no lock count
}

#[unsafe(no_mangle)]
pub extern "system" fn GlobalAlloc(flags: u32, bytes: usize) -> *mut c_void {
    LocalAlloc(flags, bytes)
}

#[unsafe(no_mangle)]
pub extern "system" fn GlobalFree(memory: *mut c_void) -> *mut c_void {
    LocalFree(memory)
}

#[unsafe(no_mangle)]
pub extern "system" fn GlobalLock(memory: *mut c_void) -> *mut c_void {
    memory
}

#[unsafe(no_mangle)]
pub extern "system" fn GlobalUnlock(_memory: *mut c_void) -> Bool {
    FALSE
}

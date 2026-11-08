//! Window extra bytes: the class's cbWndExtra bytes after each window,
//! zeroed when the window is created (WNDCLASSEX, as Windows documents),
//! read and written by Get/SetWindowLong(Ptr)W at offsets from zero. An
//! offset whose value would not fit inside them fails with
//! ERROR_INVALID_INDEX. The bytes come from the process heap and go with
//! the window.

use crate::window::{Hwnd, set_last_error, with};

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GetProcessHeap() -> usize;
    fn HeapAlloc(heap: usize, flags: u32, size: usize) -> *mut u8;
    fn HeapFree(heap: usize, flags: u32, p: *mut u8) -> i32;
}

const HEAP_ZERO_MEMORY: u32 = 8;
const ERROR_INVALID_INDEX: u32 = 1413;
const ERROR_INVALID_WINDOW_HANDLE: u32 = 1400;
/// The most extra bytes a class may ask for (more is taken as this; real
/// programs ask for a few dozen).
pub(crate) const MAX_EXTRA: u32 = 0x8000;

/// A class's cbWndExtra as kept: a negative count is none.
pub(crate) fn class_extra(requested: i32) -> u32 {
    (requested.max(0) as u32).min(MAX_EXTRA)
}

/// Zeroed extra bytes for a new window (null for none, or when the heap
/// has no room: the window then has none, recorded in the ledger). Called
/// under the state lock: the ledger call takes no user32 lock.
pub(crate) fn alloc(len: u32) -> *mut u8 {
    if len == 0 {
        return core::ptr::null_mut();
    }
    // SAFETY: plain calls.
    let p = unsafe { HeapAlloc(GetProcessHeap(), HEAP_ZERO_MEMORY, len as usize) };
    if p.is_null() {
        crate::window::unsupported(
            b"user32.dll!window extra bytes (out of memory)",
            u64::from(len),
        );
    }
    p
}

/// The extra bytes of a window that is gone.
pub(crate) fn free(p: *mut u8) {
    if !p.is_null() {
        // SAFETY: bytes from alloc, freed once (the window's slot is cleared).
        unsafe { HeapFree(GetProcessHeap(), 0, p) };
    }
}

/// Where `size` bytes at `offset` lie in the window's extra bytes:
/// ERROR_INVALID_WINDOW_HANDLE for no window, ERROR_INVALID_INDEX when
/// they do not fit.
fn place(hwnd: Hwnd, offset: i32, size: u32) -> Result<*mut u8, u32> {
    let (p, len) = crate::window::window(hwnd)
        .map(|w| (w.extra, w.extra_len))
        .ok_or(ERROR_INVALID_WINDOW_HANDLE)?;
    let end = (offset as u32).checked_add(size).ok_or(ERROR_INVALID_INDEX)?;
    if offset < 0 || p.is_null() || end > len {
        return Err(ERROR_INVALID_INDEX);
    }
    // SAFETY: inside the window's `len` extra bytes.
    Ok(unsafe { p.add(offset as usize) })
}

/// GetWindowLong(Ptr)W at a non-negative offset: `size` (4 or the
/// pointer size) bytes, sign-extended; 0 and the last error when they do
/// not fit.
pub(crate) fn get(hwnd: Hwnd, offset: i32, size: u32) -> isize {
    match place(hwnd, offset, size) {
        Ok(p) => with(|_| read(p, size)),
        Err(e) => {
            set_last_error(e);
            0
        }
    }
}

/// SetWindowLong(Ptr)W at a non-negative offset: the previous value, or
/// 0 and the last error when the bytes do not fit.
pub(crate) fn set(hwnd: Hwnd, offset: i32, size: u32, value: isize) -> isize {
    match place(hwnd, offset, size) {
        Ok(p) => with(|_| {
            let old = read(p, size);
            let bytes = (value as i64).to_le_bytes();
            // SAFETY: `size` bytes inside the window's extra bytes, under
            // the state lock (as every reader takes it).
            unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), p, size as usize) };
            old
        }),
        Err(e) => {
            set_last_error(e);
            0
        }
    }
}

fn read(p: *const u8, size: u32) -> isize {
    let mut b = [0u8; 8];
    // SAFETY: `size` (at most 8) bytes inside the window's extra bytes.
    unsafe { core::ptr::copy_nonoverlapping(p, b.as_mut_ptr(), size as usize) };
    if size == 4 {
        i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as isize
    } else {
        i64::from_le_bytes(b) as isize
    }
}

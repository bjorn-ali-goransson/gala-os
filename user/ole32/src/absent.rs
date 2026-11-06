//! ole32 functions gala has no use for yet, as real exports (Chromium's
//! chrome.dll delay-imports them and resolves them with GetProcAddress,
//! which does not return `gala_stubs` stubs). Each reports itself to the
//! ledger (`ole32.dll!<name>`, "unsupported") and fails as documented:
//!
//! - Structured storage on memory (CreateILockBytesOnHGlobal,
//!   StgCreateDocfileOnILockBytes, GetHGlobalFromILockBytes):
//!   E_NOTIMPL, the output NULL.
//! - OleDuplicateData: NULL (the copy failed).
//! - DoDragDrop: E_NOTIMPL (no drag and drop between windows yet).

use core::ffi::c_void;

use super::{E_NOTIMPL, HResult, missing};

fn absent(what: &[u8], out: *mut *mut c_void) -> HResult {
    missing(what, 0);
    if !out.is_null() {
        // SAFETY: the caller's output pointer.
        unsafe { out.write(core::ptr::null_mut()) };
    }
    E_NOTIMPL
}

#[unsafe(no_mangle)]
pub extern "system" fn CreateILockBytesOnHGlobal(
    _global: usize,
    _delete: i32,
    out: *mut *mut c_void,
) -> HResult {
    absent(b"ole32.dll!CreateILockBytesOnHGlobal", out)
}

#[unsafe(no_mangle)]
pub extern "system" fn GetHGlobalFromILockBytes(
    _bytes: *mut c_void,
    out: *mut *mut c_void,
) -> HResult {
    absent(b"ole32.dll!GetHGlobalFromILockBytes", out)
}

#[unsafe(no_mangle)]
pub extern "system" fn StgCreateDocfileOnILockBytes(
    _bytes: *mut c_void,
    _mode: u32,
    _reserved: u32,
    out: *mut *mut c_void,
) -> HResult {
    absent(b"ole32.dll!StgCreateDocfileOnILockBytes", out)
}

#[unsafe(no_mangle)]
pub extern "system" fn OleDuplicateData(_source: usize, _format: u16, _flags: u32) -> usize {
    missing(b"ole32.dll!OleDuplicateData", 0);
    0
}

#[unsafe(no_mangle)]
pub extern "system" fn DoDragDrop(
    _data: *mut c_void,
    _source: *mut c_void,
    _ok: u32,
    effect: *mut u32,
) -> HResult {
    missing(b"ole32.dll!DoDragDrop", 0);
    if !effect.is_null() {
        // SAFETY: the caller's DWORD.
        unsafe { effect.write(0) };
    }
    E_NOTIMPL
}

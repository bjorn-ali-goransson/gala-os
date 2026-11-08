//! gala's dwmapi.dll (the Desktop Window Manager's API, dwmapi.h).
//! gala's windows are composed by its own compositor (shasha), which
//! has no DWM interface yet: the frame and attribute calls Chromium makes
//! are accepted without effect (Set, Extend: reported to the ledger),
//! queries say the information is not available (E_NOTIMPL, reported),
//! and DwmDefWindowProc handles nothing.

#![no_std]
#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

// The 32-bit build (SysWOW64) takes the memory routines, 64-bit
// division helpers and unwind personality from ntrt's x86 runtime
// (ARCHITECTURE "Plan: WOW64").
#[cfg(target_arch = "x86")]
use ntrt as _;

/// MSVC-target codegen references this marker symbol directly, so it cannot
/// come from an import.
#[cfg(target_arch = "x86_64")]
#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

#[cfg_attr(
    target_arch = "x86",
    link(name = "ntdll", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "ntdll", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GalaRecordMissing(name: *const u8, length: usize, what: u32, detail: u64) -> u32;
}

const S_OK: i32 = 0;
const E_NOTIMPL: i32 = 0x8000_4001_u32 as i32;
const E_INVALIDARG: i32 = 0x8007_0057_u32 as i32;

fn missing(what: &[u8], detail: u64) {
    // SAFETY: a valid buffer; the service only reads it.
    unsafe { GalaRecordMissing(what.as_ptr(), what.len(), 2, detail) };
}

/// DwmDefWindowProc(hWnd, msg, wParam, lParam, *plResult): FALSE, the
/// message is not the DWM's (gala draws the frame itself).
#[unsafe(no_mangle)]
pub extern "system" fn DwmDefWindowProc(
    _hwnd: usize,
    _msg: u32,
    _w: usize,
    _l: isize,
    result: *mut isize,
) -> i32 {
    if !result.is_null() {
        // SAFETY: the caller's output.
        unsafe { *result = 0 };
    }
    0
}

/// DwmExtendFrameIntoClientArea(hWnd, pMarInset): accepted, no effect.
#[unsafe(no_mangle)]
pub extern "system" fn DwmExtendFrameIntoClientArea(hwnd: usize, margins: *const i32) -> i32 {
    if hwnd == 0 || margins.is_null() {
        return E_INVALIDARG;
    }
    missing(b"dwmapi.dll!DwmExtendFrameIntoClientArea (no effect)", 0);
    S_OK
}

/// DwmGetCompositionTimingInfo: not available (E_NOTIMPL; callers fall
/// back to their own frame timing).
#[unsafe(no_mangle)]
pub extern "system" fn DwmGetCompositionTimingInfo(_hwnd: usize, _info: *mut u8) -> i32 {
    missing(b"dwmapi.dll!DwmGetCompositionTimingInfo", 0);
    E_NOTIMPL
}

/// DwmGetWindowAttribute(hwnd, dwAttribute, pvAttribute, cbAttribute):
/// not available (E_NOTIMPL, the attribute in the ledger).
#[unsafe(no_mangle)]
pub extern "system" fn DwmGetWindowAttribute(
    _hwnd: usize,
    attribute: u32,
    _value: *mut u8,
    _size: u32,
) -> i32 {
    missing(b"dwmapi.dll!DwmGetWindowAttribute", attribute.into());
    E_NOTIMPL
}

/// DwmSetWindowAttribute: accepted, no effect (the attribute in the
/// ledger).
#[unsafe(no_mangle)]
pub extern "system" fn DwmSetWindowAttribute(
    hwnd: usize,
    attribute: u32,
    value: *const u8,
    _size: u32,
) -> i32 {
    if hwnd == 0 || value.is_null() {
        return E_INVALIDARG;
    }
    missing(b"dwmapi.dll!DwmSetWindowAttribute (no effect)", attribute.into());
    S_OK
}

/// DwmIsCompositionEnabled(*pfEnabled): TRUE, as on every Windows since 8.
#[unsafe(no_mangle)]
pub extern "system" fn DwmIsCompositionEnabled(enabled: *mut i32) -> i32 {
    if enabled.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's output.
    unsafe { *enabled = 1 };
    S_OK
}

/// DwmFlush: nothing waits for a DWM frame on gala.
#[unsafe(no_mangle)]
pub extern "system" fn DwmFlush() -> i32 {
    S_OK
}

/// The DLL entry point: nothing to set up.
#[unsafe(no_mangle)]
pub extern "system" fn mainCRTStartup(_instance: usize, _reason: u32, _reserved: usize) -> i32 {
    1
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}

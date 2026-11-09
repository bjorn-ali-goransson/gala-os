//! gala's imm32.dll (the Input Method Manager, imm.h): gala has no input
//! methods, so no keyboard layout is an IME and there are no input
//! contexts (every call on one fails as on an invalid context).

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

/// ImmGetIMEFileNameW(hKL, lpszFileName, uBufLen): 0, the layout is not
/// an IME.
#[unsafe(no_mangle)]
pub extern "system" fn ImmGetIMEFileNameW(_layout: usize, name: *mut u16, length: u32) -> u32 {
    if !name.is_null() && length > 0 {
        // SAFETY: the caller's buffer of at least one character.
        unsafe { *name = 0 };
    }
    0
}

/// ImmDisableIME(idThread): TRUE. There are no IMEs, so they are
/// disabled already for any thread (0: the calling one, -1: every thread
/// of the process), as Windows reports success (office_api_test).
#[unsafe(no_mangle)]
pub extern "system" fn ImmDisableIME(_thread: u32) -> i32 {
    1
}

/// ImmIsIME(hKL): FALSE.
#[unsafe(no_mangle)]
pub extern "system" fn ImmIsIME(_layout: usize) -> i32 {
    0
}

/// ImmGetContext(hWnd): no input context (NULL).
#[unsafe(no_mangle)]
pub extern "system" fn ImmGetContext(_hwnd: usize) -> usize {
    0
}

#[unsafe(no_mangle)]
pub extern "system" fn ImmReleaseContext(_hwnd: usize, _context: usize) -> i32 {
    1
}

/// ImmAssociateContextEx: nothing to associate; TRUE.
#[unsafe(no_mangle)]
pub extern "system" fn ImmAssociateContextEx(_hwnd: usize, _context: usize, _flags: u32) -> i32 {
    1
}

// ---- Input contexts: gala hands none out ----

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn SetLastError(code: u32);
}

/// The answer for a call on input context `himc`: gala makes no input
/// contexts (ImmGetContext gives NULL), so none is valid; FALSE with
/// ERROR_INVALID_HANDLE for a handle, nothing set for NULL (as Windows
/// answers ImmNotifyIME).
fn no_context(himc: usize) -> i32 {
    // SAFETY: plain calls.
    unsafe { SetLastError(if himc == 0 { 0 } else { 6 }) }; // ERROR_INVALID_HANDLE
    0
}

/// ImmNotifyIME(hIMC, action, index, value): no context, FALSE.
#[unsafe(no_mangle)]
pub extern "system" fn ImmNotifyIME(himc: usize, _action: u32, _index: u32, _value: u32) -> i32 {
    no_context(himc)
}

/// ImmSetCandidateWindow(hIMC, CANDIDATEFORM*): no context, FALSE.
#[unsafe(no_mangle)]
pub extern "system" fn ImmSetCandidateWindow(himc: usize, _form: *const u8) -> i32 {
    no_context(himc)
}

/// ImmSetCompositionWindow(hIMC, COMPOSITIONFORM*): no context, FALSE.
#[unsafe(no_mangle)]
pub extern "system" fn ImmSetCompositionWindow(himc: usize, _form: *const u8) -> i32 {
    no_context(himc)
}

/// ImmSetCompositionFontW(hIMC, LOGFONTW*): no context, FALSE.
#[unsafe(no_mangle)]
pub extern "system" fn ImmSetCompositionFontW(himc: usize, _font: *const u8) -> i32 {
    no_context(himc)
}

/// ImmSetCompositionStringW: no context, FALSE.
#[unsafe(no_mangle)]
pub extern "system" fn ImmSetCompositionStringW(
    himc: usize,
    _index: u32,
    _comp: *const u8,
    _comp_len: u32,
    _read: *const u8,
    _read_len: u32,
) -> i32 {
    no_context(himc)
}

/// ImmGetCompositionStringW: no context, so no composition string (0
/// bytes).
#[unsafe(no_mangle)]
pub extern "system" fn ImmGetCompositionStringW(
    himc: usize,
    _index: u32,
    _buf: *mut u8,
    _len: u32,
) -> i32 {
    no_context(himc)
}

/// ImmEscapeW(hKL, hIMC, escape, data): no IME to pass it to, 0.
#[unsafe(no_mangle)]
pub extern "system" fn ImmEscapeW(
    _layout: usize,
    himc: usize,
    _escape: u32,
    _data: *mut u8,
) -> isize {
    no_context(himc) as isize
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

//! gala's imm32.dll (the Input Method Manager, imm.h): gala has no input
//! methods, so no keyboard layout is an IME and there are no input
//! contexts (every call on one fails as on an invalid context). What a
//! window is associated with is therefore always nothing, but the
//! handles are still answered for: ImmGetContext and ImmAssociateContext
//! tell a window from a handle that is none.

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
#[cfg(not(target_arch = "x86"))]
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

/// ImmGetContext(hWnd): no input context (NULL). A window of a thread
/// that has one answers with it on Windows; gala's threads have none, so
/// the answer is always NULL, with the last error left as it was. A
/// handle that is no window is ERROR_INVALID_WINDOW_HANDLE, a NULL one
/// nothing (`user32_test`).
#[unsafe(no_mangle)]
pub extern "system" fn ImmGetContext(hwnd: usize) -> usize {
    window_or_error(hwnd);
    0
}

/// ImmAssociateContext(hWnd, hIMC): the input context the window was
/// associated with, NULL for none, which is what gala always answers --
/// its threads have no input context, so no window holds one. Windows
/// sets ERROR_SUCCESS when the association changed and leaves the last
/// error as it was when it did not, so a call that changes nothing (the
/// only kind here) leaves it alone. The window is looked at before the
/// context: NULL is that silent nothing, a handle that is no window
/// (destroyed among them) is ERROR_INVALID_WINDOW_HANDLE whatever the
/// context, and then a handle that is no context is
/// ERROR_INVALID_HANDLE -- which, with no contexts of gala's making, is
/// every handle that is not NULL. Windows 11; `user32_test`. MSO's UI
/// delay-loads this (ROADMAP OF).
#[unsafe(no_mangle)]
pub extern "system" fn ImmAssociateContext(hwnd: usize, himc: usize) -> usize {
    if !window_or_error(hwnd) {
        return 0;
    }
    if himc != 0 {
        // SAFETY: plain call.
        unsafe { SetLastError(ERROR_INVALID_HANDLE) };
    }
    // Nothing was associated and nothing is now, so nothing changed and
    // the last error stands.
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

#[cfg_attr(
    target_arch = "x86",
    link(name = "user32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "user32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn IsWindow(hwnd: usize) -> i32;
}

/// ERROR_INVALID_WINDOW_HANDLE: the handle given is no window.
const ERROR_INVALID_WINDOW_HANDLE: u32 = 1400;
/// ERROR_INVALID_HANDLE: the handle given is no input context.
const ERROR_INVALID_HANDLE: u32 = 6;

/// Whether `hwnd` is a window, as the input functions ask: NULL is no
/// window and no error either (the call is a silent nothing), a handle
/// that is no window is ERROR_INVALID_WINDOW_HANDLE, and the last error
/// is left alone for a window.
fn window_or_error(hwnd: usize) -> bool {
    // SAFETY: plain calls.
    unsafe {
        if hwnd == 0 {
            return false;
        }
        if IsWindow(hwnd) == 0 {
            SetLastError(ERROR_INVALID_WINDOW_HANDLE);
            return false;
        }
    }
    true
}

/// The answer for a call on input context `himc`: gala makes no input
/// contexts (ImmGetContext gives NULL), so none is valid; FALSE with
/// ERROR_INVALID_HANDLE for a handle, nothing set for NULL (as Windows
/// answers ImmNotifyIME).
fn no_context(himc: usize) -> i32 {
    // SAFETY: plain calls.
    unsafe { SetLastError(if himc == 0 { 0 } else { ERROR_INVALID_HANDLE }) };
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

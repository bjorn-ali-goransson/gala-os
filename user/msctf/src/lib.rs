//! gala's msctf.dll (the Text Services Framework, inputscope.h): the
//! input scope functions programs call on their edit windows (Delphi's
//! VCL looks up SetInputScope; Inno Setup). gala has no text services
//! (on-screen keyboards, handwriting or speech input) to tell what a
//! window expects, so a window's input scopes are accepted and have no
//! effect; the TSF COM objects are not provided.

#![no_std]
#![allow(non_snake_case)]

/// MSVC-target codegen references this marker symbol directly, so it cannot
/// come from an import.
#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

// The C runtime helpers the compiler may call, exported by ntdll.
#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memmove(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn __chkstk();
}

#[link(name = "user32", kind = "raw-dylib")]
unsafe extern "system" {
    fn IsWindow(hwnd: usize) -> i32;
}

const S_OK: i32 = 0;
const E_INVALIDARG: i32 = 0x8007_0057u32 as i32;

fn window(hwnd: usize) -> i32 {
    // SAFETY: plain call.
    if hwnd != 0 && unsafe { IsWindow(hwnd) } != 0 { S_OK } else { E_INVALIDARG }
}

/// SetInputScope(hwnd, scope): S_OK for a window (nothing reads it).
#[unsafe(no_mangle)]
pub extern "system" fn SetInputScope(hwnd: usize, _scope: i32) -> i32 {
    window(hwnd)
}

/// SetInputScopes(hwnd, scopes, count, phrases, phrase_count, regex,
/// speech): S_OK for a window (nothing reads them).
#[unsafe(no_mangle)]
pub extern "system" fn SetInputScopes(
    hwnd: usize,
    _scopes: *const i32,
    _count: u32,
    _phrases: *const *const u16,
    _phrase_count: u32,
    _regex: *const u16,
    _speech: *const u16,
) -> i32 {
    window(hwnd)
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

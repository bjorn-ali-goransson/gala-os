//! gala's comctl32.dll: InitCommonControls(Ex), which programs call at
//! startup to register the common control window classes. gala has no
//! windows; the call succeeds (the JVM's launcher calls it).

#![no_std]
#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

mod unimplemented;

#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memmove(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
    fn strlen(s: *const u8) -> usize;
    // MSVC-target unwind tables reference it (gala's ntdll provides it).
    fn __CxxFrameHandler3();
    // The stack probe for frames over a page (ntdll exports it).
    fn __chkstk();
}

/// MSVC-target codegen references this marker symbol directly, so it cannot
/// come from an import. (It becomes a harmless extra data export.)
#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

/// InitCommonControlsEx(INITCOMMONCONTROLSEX*): TRUE for a structure of
/// the documented size (8 bytes), FALSE otherwise.
#[unsafe(no_mangle)]
pub extern "system" fn InitCommonControlsEx(icc: *const u32) -> i32 {
    // SAFETY: the caller's structure; dwSize first.
    i32::from(!icc.is_null() && unsafe { *icc } == 8)
}

#[unsafe(no_mangle)]
pub extern "system" fn InitCommonControls() {}

/// DllMain (the entry point our link flags name).
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

//! gala's user32.dll: no windows or desktop, only the text helpers console
//! programs import.

#![no_std]
#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memmove(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
}

/// CharToOemBuffA(src, dst, length): ANSI to the OEM code page. gala's
/// code pages are the same (UTF-8 for ANSI text), so this copies.
#[unsafe(no_mangle)]
pub extern "system" fn CharToOemBuffA(src: *const u8, dst: *mut u8, length: u32) -> i32 {
    if src.is_null() || dst.is_null() {
        return 0;
    }
    // SAFETY: the caller's buffers of `length` bytes (they may be the same).
    unsafe { memmove(dst, src, length as usize) };
    1
}

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

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

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn SetLastError(code: u32);
    fn GetStdHandle(which: u32) -> usize;
    fn WriteFile(h: usize, buf: *const u8, len: u32, written: *mut u32, ov: usize) -> i32;
}

/// GetProcessWindowStation: gala has no window stations (NULL), which is
/// how OpenSSL tells it is not on an interactive desktop.
#[unsafe(no_mangle)]
pub extern "system" fn GetProcessWindowStation() -> usize {
    // SAFETY: plain call.
    unsafe { SetLastError(50) }; // ERROR_NOT_SUPPORTED
    0
}

#[unsafe(no_mangle)]
pub extern "system" fn GetUserObjectInformationW(
    _object: usize,
    _index: i32,
    _info: *mut u8,
    _length: u32,
    _needed: *mut u32,
) -> i32 {
    // SAFETY: plain call.
    unsafe { SetLastError(6) }; // ERROR_INVALID_HANDLE
    0
}

/// MessageBoxW: no desktop; the caption and text go to standard error and
/// the answer is IDOK.
#[unsafe(no_mangle)]
pub extern "system" fn MessageBoxW(
    _window: usize,
    text: *const u16,
    caption: *const u16,
    _kind: u32,
) -> i32 {
    for s in [caption, text] {
        if s.is_null() {
            continue;
        }
        let mut buf = [0u8; 1024];
        let mut n = 0;
        // SAFETY: the caller's NUL-terminated UTF-16 string.
        unsafe {
            let mut i = 0;
            while *s.add(i) != 0 && n < buf.len() - 2 {
                let c = *s.add(i);
                buf[n] = if c < 0x80 { c as u8 } else { b'?' };
                n += 1;
                i += 1;
            }
        }
        buf[n] = b'\r';
        buf[n + 1] = b'\n';
        let mut written = 0;
        // SAFETY: a valid buffer.
        unsafe {
            WriteFile(GetStdHandle(-12i32 as u32), buf.as_ptr(), (n + 2) as u32, &mut written, 0)
        };
    }
    1 // IDOK
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

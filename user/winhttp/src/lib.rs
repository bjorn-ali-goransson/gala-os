//! gala's winhttp.dll: the proxy functions the JDK's net.dll imports
//! (the system proxy selector). There are no Internet Explorer proxy
//! settings on gala (WinHttpGetIEProxyConfigForCurrentUser:
//! ERROR_FILE_NOT_FOUND, as documented for that case); WinHTTP sessions
//! and proxy discovery are not supported (recorded).

#![no_std]
#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

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

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn GalaRecordMissing(name: *const u8, length: usize, what: u32, detail: u64) -> u32;
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn SetLastError(e: u32);
}

const ERROR_FILE_NOT_FOUND: u32 = 2;
const ERROR_NOT_SUPPORTED: u32 = 50;

fn fail(name: &[u8]) {
    // SAFETY: plain calls with a valid buffer.
    unsafe {
        GalaRecordMissing(name.as_ptr(), name.len(), 2, 0);
        SetLastError(ERROR_NOT_SUPPORTED);
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn WinHttpGetIEProxyConfigForCurrentUser(_config: usize) -> i32 {
    // SAFETY: a plain call.
    unsafe { SetLastError(ERROR_FILE_NOT_FOUND) };
    0
}

#[unsafe(no_mangle)]
pub extern "system" fn WinHttpOpen(
    _agent: usize,
    _access: u32,
    _proxy: usize,
    _bypass: usize,
    _flags: u32,
) -> usize {
    fail(b"winhttp.dll!WinHttpOpen");
    0
}

#[unsafe(no_mangle)]
pub extern "system" fn WinHttpGetProxyForUrl(
    _session: usize,
    _url: usize,
    _options: usize,
    _info: usize,
) -> i32 {
    fail(b"winhttp.dll!WinHttpGetProxyForUrl");
    0
}

#[unsafe(no_mangle)]
pub extern "system" fn WinHttpCloseHandle(_h: usize) -> i32 {
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

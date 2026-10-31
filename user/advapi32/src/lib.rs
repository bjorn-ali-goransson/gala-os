//! gala's advapi32.dll: the registry and the event log, which gala does not
//! have yet. The functions exist (programs import them) and fail as
//! documented, reported to the kernel's missing-features ledger.

#![no_std]
#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use core::ffi::c_void;

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn GalaRecordMissing(name: *const u8, length: usize, what: u32, detail: u64) -> u32;
}

#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn SetLastError(code: u32);
}

const ERROR_ACCESS_DENIED: u32 = 5;
const ERROR_NOT_SUPPORTED: u32 = 50;

fn missing(function: &[u8]) {
    // SAFETY: a valid buffer.
    unsafe { GalaRecordMissing(function.as_ptr(), function.len(), 2, 0) };
}

/// RegCreateKeyExA: no registry yet; registry functions return the error
/// (they do not use the last-error value).
#[unsafe(no_mangle)]
pub extern "system" fn RegCreateKeyExA(
    _key: usize,
    _subkey: *const u8,
    _reserved: u32,
    _class: *const u8,
    _options: u32,
    _access: u32,
    _security: *mut c_void,
    _result: *mut usize,
    _disposition: *mut u32,
) -> i32 {
    missing(b"advapi32.dll!RegCreateKeyExA");
    ERROR_ACCESS_DENIED as i32
}

#[unsafe(no_mangle)]
pub extern "system" fn RegSetValueExA(
    _key: usize,
    _name: *const u8,
    _reserved: u32,
    _kind: u32,
    _data: *const u8,
    _size: u32,
) -> i32 {
    missing(b"advapi32.dll!RegSetValueExA");
    ERROR_ACCESS_DENIED as i32
}

#[unsafe(no_mangle)]
pub extern "system" fn RegCloseKey(_key: usize) -> i32 {
    6 // ERROR_INVALID_HANDLE: no key was ever opened
}

/// RegisterEventSourceA: no event log yet (NULL, ERROR_NOT_SUPPORTED).
#[unsafe(no_mangle)]
pub extern "system" fn RegisterEventSourceA(_server: *const u8, _source: *const u8) -> usize {
    missing(b"advapi32.dll!RegisterEventSourceA");
    // SAFETY: plain call.
    unsafe { SetLastError(ERROR_NOT_SUPPORTED) };
    0
}

#[unsafe(no_mangle)]
pub extern "system" fn ReportEventA(
    _log: usize,
    _kind: u16,
    _category: u16,
    _id: u32,
    _sid: *mut c_void,
    _strings: u16,
    _size: u32,
    _text: *const *const u8,
    _data: *mut c_void,
) -> i32 {
    missing(b"advapi32.dll!ReportEventA");
    // SAFETY: plain call.
    unsafe { SetLastError(ERROR_NOT_SUPPORTED) };
    0
}

#[unsafe(no_mangle)]
pub extern "system" fn DeregisterEventSource(_log: usize) -> i32 {
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

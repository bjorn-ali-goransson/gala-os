//! gala's crypt32.dll: certificate contexts (`cert`: X.509 decoding,
//! properties, names, public keys), DPAPI (`dpapi`), and certificate
//! stores, which gala does not have yet. OpenSSL imports the store
//! functions to look up certificates in Windows' stores (an option Nginx
//! does not use); they fail as documented: no store opens, and an
//! enumeration finds nothing (CRYPT_E_NOT_FOUND).

#![no_std]
#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

mod base64;
mod cert;
mod dpapi;
mod unimplemented;

use core::ffi::c_void;

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn GalaRecordMissing(name: *const u8, length: usize, what: u32, detail: u64) -> u32;
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn SetLastError(code: u32);
}

#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
    fn strlen(s: *const u8) -> usize;
    fn memmove(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    // MSVC-target unwind tables reference it (gala's ntdll provides it).
    fn __CxxFrameHandler3();
    // The stack probe for frames over a page (ntdll exports it).
    fn __chkstk();
}

/// MSVC-target codegen references this marker symbol directly.
#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

const ERROR_NOT_SUPPORTED: u32 = 50;
const CRYPT_E_NOT_FOUND: u32 = 0x8009_2004;

fn missing_detail(function: &[u8], detail: u64) {
    // SAFETY: a valid buffer.
    unsafe { GalaRecordMissing(function.as_ptr(), function.len(), 2, detail) };
}

fn fail<T>(function: &[u8], error: u32, value: T) -> T {
    // SAFETY: a valid buffer; a plain call.
    unsafe {
        GalaRecordMissing(function.as_ptr(), function.len(), 2, 0);
        SetLastError(error);
    }
    value
}

#[unsafe(no_mangle)]
pub extern "system" fn CertOpenStore(
    _provider: *const u8,
    _encoding: u32,
    _crypt: usize,
    _flags: u32,
    _parameter: *const c_void,
) -> *mut c_void {
    fail(b"crypt32.dll!CertOpenStore", ERROR_NOT_SUPPORTED, core::ptr::null_mut())
}

#[unsafe(no_mangle)]
pub extern "system" fn CertOpenSystemStoreW(_crypt: usize, _name: *const u16) -> *mut c_void {
    fail(b"crypt32.dll!CertOpenSystemStoreW", ERROR_NOT_SUPPORTED, core::ptr::null_mut())
}

#[unsafe(no_mangle)]
pub extern "system" fn CertCloseStore(_store: *mut c_void, _flags: u32) -> i32 {
    1
}

#[unsafe(no_mangle)]
pub extern "system" fn CertEnumCertificatesInStore(
    _store: *mut c_void,
    _previous: *const c_void,
) -> *const c_void {
    // SAFETY: plain call.
    unsafe { SetLastError(CRYPT_E_NOT_FOUND) };
    core::ptr::null()
}

#[unsafe(no_mangle)]
pub extern "system" fn CertFindCertificateInStore(
    _store: *mut c_void,
    _encoding: u32,
    _flags: u32,
    _kind: u32,
    _parameter: *const c_void,
    _previous: *const c_void,
) -> *const c_void {
    // SAFETY: plain call.
    unsafe { SetLastError(CRYPT_E_NOT_FOUND) };
    core::ptr::null()
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

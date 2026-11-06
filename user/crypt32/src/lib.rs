//! gala's crypt32.dll: certificate contexts (`cert`: X.509 decoding,
//! properties, names, public keys, signatures), the chain engine and its
//! policies (`chain`), CRLs and revocation (`crl`), DPAPI (`dpapi`),
//! memory certificate stores (`store`; ROOT and CA hold gala's machine
//! directories, `roots`; the other system stores open empty),
//! CryptQueryObject and CryptDecodeObjectEx for alternative names
//! (`query`), and smaller helpers (`misc`).

#![no_std]
#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

mod base64;
mod cert;
mod chain;
mod crl;
mod dpapi;
mod misc;
mod pfx;
mod query;
mod roots;
mod store;
mod unimplemented;

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

fn missing_detail(function: &[u8], detail: u64) {
    // SAFETY: a valid buffer.
    unsafe { GalaRecordMissing(function.as_ptr(), function.len(), 2, detail) };
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

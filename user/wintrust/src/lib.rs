//! gala's wintrust.dll: WinVerifyTrust with WINTRUST_ACTION_GENERIC_VERIFY_V2
//! (Authenticode signatures of PE images and cabinets, `verify`), the WTHelper
//! functions over the provider data it keeps, and the catalog functions
//! SQL Server LocalDB imports, not implemented (`unimplemented`).
//!
//! The design (ARCHITECTURE "wintrust: Authenticode"): the PE image hash
//! and the cabinet digest are wintrust's (`pe`, `cab`, through bcrypt); the PKCS #7 SignedData, its
//! signer's signature, the RFC 3161 timestamp and the chain and its
//! Authenticode policy are crypt32's (CryptMsg*, CryptDecodeObjectEx,
//! CryptVerifyTimeStampSignature, CertGetCertificateChain,
//! CertVerifyCertificateChainPolicy), called through their public
//! functions as Windows' wintrust calls them.

#![no_std]
#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

mod cab;
mod layout;
mod pe;
mod unimplemented;
mod verify;

// On x86 the compiler's runtime symbols (memcpy, the 64-bit division
// helpers, __chkstk) come from ntrt's x86 runtime (ARCHITECTURE "Plan:
// WOW64").
#[cfg(target_arch = "x86")]
use ntrt as _;

#[cfg(target_arch = "x86_64")]
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

/// MSVC-target codegen references this marker symbol directly (ntrt's
/// x86 runtime defines it on x86).
#[cfg(target_arch = "x86_64")]
#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

#[cfg_attr(
    target_arch = "x86",
    link(name = "ntdll", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "ntdll", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GalaRecordMissing(name: *const u8, length: usize, what: u32, detail: u64) -> u32;
}

/// Records an unsupported request (the ledger's "unsupported").
pub(crate) fn unsupported(what: &[u8], detail: u64) {
    // SAFETY: a valid buffer.
    unsafe { GalaRecordMissing(what.as_ptr(), what.len(), 2, detail) };
}

/// Records an observation (the ledger's "other"): a verification step
/// that failed, with its error.
pub(crate) fn observed(what: &[u8], detail: u64) {
    // SAFETY: a valid buffer.
    unsafe { GalaRecordMissing(what.as_ptr(), what.len(), 5, detail) };
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

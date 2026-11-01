//! gala's ncrypt.dll (CNG key storage): gala has no key storage
//! providers yet, so opening one fails with NTE_NOT_SUPPORTED (recorded in
//! the ledger) and no key or provider handle can exist; the functions are
//! there because programs bind them at startup (Caddy's TPM attestation
//! library, for one).

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
}

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn GalaRecordMissing(name: *const u8, length: usize, what: u32, detail: u64) -> u32;
}

/// SECURITY_STATUS values.
const NTE_INVALID_HANDLE: i32 = 0x8009_0026u32 as i32;
const NTE_NOT_SUPPORTED: i32 = 0x8009_0029u32 as i32;

fn unsupported(function: &[u8]) {
    // SAFETY: a valid buffer.
    unsafe { GalaRecordMissing(function.as_ptr(), function.len(), 2, 0) };
}

#[unsafe(no_mangle)]
pub extern "system" fn NCryptOpenStorageProvider(
    provider: *mut usize,
    _name: *const u16,
    _flags: u32,
) -> i32 {
    unsupported(b"ncrypt.dll!NCryptOpenStorageProvider");
    if !provider.is_null() {
        // SAFETY: the caller's output pointer.
        unsafe { *provider = 0 };
    }
    NTE_NOT_SUPPORTED
}

#[unsafe(no_mangle)]
pub extern "system" fn NCryptFreeObject(_object: usize) -> i32 {
    NTE_INVALID_HANDLE
}

#[unsafe(no_mangle)]
pub extern "system" fn NCryptGetProperty(
    _object: usize,
    _property: *const u16,
    _output: *mut u8,
    _size: u32,
    _result: *mut u32,
    _flags: u32,
) -> i32 {
    NTE_INVALID_HANDLE
}

#[unsafe(no_mangle)]
pub extern "system" fn NCryptSetProperty(
    _object: usize,
    _property: *const u16,
    _input: *const u8,
    _size: u32,
    _flags: u32,
) -> i32 {
    NTE_INVALID_HANDLE
}

#[unsafe(no_mangle)]
pub extern "system" fn NCryptOpenKey(
    _provider: usize,
    _key: *mut usize,
    _name: *const u16,
    _spec: u32,
    _flags: u32,
) -> i32 {
    NTE_INVALID_HANDLE
}

#[unsafe(no_mangle)]
pub extern "system" fn NCryptCreatePersistedKey(
    _provider: usize,
    _key: *mut usize,
    _algorithm: *const u16,
    _name: *const u16,
    _spec: u32,
    _flags: u32,
) -> i32 {
    NTE_INVALID_HANDLE
}

#[unsafe(no_mangle)]
pub extern "system" fn NCryptFinalizeKey(_key: usize, _flags: u32) -> i32 {
    NTE_INVALID_HANDLE
}

#[unsafe(no_mangle)]
pub extern "system" fn NCryptDeleteKey(_key: usize, _flags: u32) -> i32 {
    NTE_INVALID_HANDLE
}

#[unsafe(no_mangle)]
pub extern "system" fn NCryptExportKey(
    _key: usize,
    _export_key: usize,
    _blob_type: *const u16,
    _parameters: *const u8,
    _output: *mut u8,
    _size: u32,
    _result: *mut u32,
    _flags: u32,
) -> i32 {
    NTE_INVALID_HANDLE
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

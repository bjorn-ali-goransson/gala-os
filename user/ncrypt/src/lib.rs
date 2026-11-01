//! gala's ncrypt.dll (CNG key storage): the Microsoft Software Key
//! Storage Provider opens (also as the default), with an empty key store:
//! no key can be opened (NTE_BAD_KEYSET) and creating one is not
//! supported yet (recorded in the ledger). Other providers (the TPM's
//! Platform Crypto Provider, smart cards) do not exist here.

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

// Windows' ncrypt.dll also exports these bcrypt.dll functions (forwarded
// there; GnuTLS imports them from ncrypt): the same calls, through ours.
// Found at run time: an import of the same name would bind to these
// exports themselves.
#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn LoadLibraryW(name: *const u16) -> usize;
    fn GetProcAddress(module: usize, name: *const u8) -> usize;
}

/// bcrypt.dll's `name` (NUL-terminated), or 0.
fn bcrypt(name: &[u8]) -> usize {
    let dll: [u16; 11] = [
        b'b' as u16,
        b'c' as u16,
        b'r' as u16,
        b'y' as u16,
        b'p' as u16,
        b't' as u16,
        b'.' as u16,
        b'd' as u16,
        b'l' as u16,
        b'l' as u16,
        0,
    ];
    // SAFETY: NUL-terminated names; LoadLibrary counts the reference.
    unsafe {
        let module = LoadLibraryW(dll.as_ptr());
        if module == 0 { 0 } else { GetProcAddress(module, name.as_ptr()) }
    }
}

/// STATUS_PROCEDURE_NOT_FOUND, when bcrypt.dll lacks the function.
const STATUS_PROCEDURE_NOT_FOUND: i32 = 0xc000_007au32 as i32;

#[unsafe(no_mangle)]
pub extern "system" fn BCryptOpenAlgorithmProvider(
    alg: *mut usize,
    id: *const u16,
    implementation: *const u16,
    flags: u32,
) -> i32 {
    type F = extern "system" fn(*mut usize, *const u16, *const u16, u32) -> i32;
    match bcrypt(b"BCryptOpenAlgorithmProvider\0") {
        0 => STATUS_PROCEDURE_NOT_FOUND,
        f => {
            // SAFETY: bcrypt.dll's function of this signature.
            let f = unsafe { core::mem::transmute::<usize, F>(f) };
            f(alg, id, implementation, flags)
        }
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn BCryptCloseAlgorithmProvider(alg: usize, flags: u32) -> i32 {
    type F = extern "system" fn(usize, u32) -> i32;
    match bcrypt(b"BCryptCloseAlgorithmProvider\0") {
        0 => STATUS_PROCEDURE_NOT_FOUND,
        f => {
            // SAFETY: as above.
            let f = unsafe { core::mem::transmute::<usize, F>(f) };
            f(alg, flags)
        }
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn BCryptGenRandom(
    alg: usize,
    buffer: *mut u8,
    length: u32,
    flags: u32,
) -> i32 {
    type F = extern "system" fn(usize, *mut u8, u32, u32) -> i32;
    match bcrypt(b"BCryptGenRandom\0") {
        0 => STATUS_PROCEDURE_NOT_FOUND,
        f => {
            // SAFETY: as above.
            let f = unsafe { core::mem::transmute::<usize, F>(f) };
            f(alg, buffer, length, flags)
        }
    }
}

/// SECURITY_STATUS values.
const NTE_INVALID_HANDLE: i32 = 0x8009_0026u32 as i32;
const NTE_NOT_SUPPORTED: i32 = 0x8009_0029u32 as i32;

fn unsupported(function: &[u8]) {
    // SAFETY: a valid buffer.
    unsafe { GalaRecordMissing(function.as_ptr(), function.len(), 2, 0) };
}

const NTE_BAD_KEYSET: i32 = 0x8009_0016u32 as i32;
const NTE_INVALID_PARAMETER: i32 = 0x8009_0027u32 as i32;
const NTE_PROV_DLL_NOT_FOUND: i32 = 0x8009_001Eu32 as i32;

/// The software provider's handle: the address of this marker (it has no
/// state; opening it again gives the same handle).
static SOFTWARE_PROVIDER: u8 = 0;

fn software_provider() -> usize {
    &raw const SOFTWARE_PROVIDER as usize
}

/// Whether the NUL-terminated `name` is `want` (ASCII, ignoring case).
fn named(name: *const u16, want: &str) -> bool {
    let mut i = 0;
    for b in want.bytes() {
        // SAFETY: the caller's NUL-terminated string, read up to its NUL.
        let c = unsafe { *name.add(i) };
        if c > 0x7f || (c as u8).to_ascii_lowercase() != b.to_ascii_lowercase() {
            return false;
        }
        i += 1;
    }
    // SAFETY: as above.
    unsafe { *name.add(i) == 0 }
}

/// NCryptOpenStorageProvider: the Microsoft Software Key Storage Provider
/// (a null name means it too); no other provider is installed.
#[unsafe(no_mangle)]
pub extern "system" fn NCryptOpenStorageProvider(
    provider: *mut usize,
    name: *const u16,
    _flags: u32,
) -> i32 {
    if provider.is_null() {
        return NTE_INVALID_PARAMETER;
    }
    if !name.is_null() && !named(name, "Microsoft Software Key Storage Provider") {
        unsupported(b"ncrypt.dll!NCryptOpenStorageProvider(provider)");
        // SAFETY: the caller's output pointer.
        unsafe { *provider = 0 };
        return NTE_PROV_DLL_NOT_FOUND;
    }
    // SAFETY: as above.
    unsafe { *provider = software_provider() };
    0
}

/// NCryptFreeObject: providers (no keys exist yet).
#[unsafe(no_mangle)]
pub extern "system" fn NCryptFreeObject(object: usize) -> i32 {
    if object == software_provider() { 0 } else { NTE_INVALID_HANDLE }
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

/// NCryptOpenKey: the software provider's key store is empty.
#[unsafe(no_mangle)]
pub extern "system" fn NCryptOpenKey(
    provider: usize,
    key: *mut usize,
    _name: *const u16,
    _spec: u32,
    _flags: u32,
) -> i32 {
    if provider != software_provider() {
        return NTE_INVALID_HANDLE;
    }
    if !key.is_null() {
        // SAFETY: the caller's output pointer.
        unsafe { *key = 0 };
    }
    NTE_BAD_KEYSET
}

/// NCryptCreatePersistedKey: not supported yet (recorded).
#[unsafe(no_mangle)]
pub extern "system" fn NCryptCreatePersistedKey(
    provider: usize,
    _key: *mut usize,
    _algorithm: *const u16,
    _name: *const u16,
    _spec: u32,
    _flags: u32,
) -> i32 {
    if provider != software_provider() {
        return NTE_INVALID_HANDLE;
    }
    unsupported(b"ncrypt.dll!NCryptCreatePersistedKey");
    NTE_NOT_SUPPORTED
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

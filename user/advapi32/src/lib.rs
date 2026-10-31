//! gala's advapi32.dll: the registry and the event log, which gala does not
//! have yet (the functions exist, as programs import them, and fail as
//! documented, reported to the kernel's missing-features ledger), and the
//! CryptoAPI's random number generator: a CRYPT_VERIFYCONTEXT provider
//! whose CryptGenRandom draws from gala's ProcessPrng (what OpenSSL seeds
//! from). Keys, hashes and signatures are not there yet (reported).

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
    fn memmove(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
    fn strlen(s: *const u8) -> usize;
    // MSVC-target unwind tables reference it (gala's ntdll provides it).
    fn __CxxFrameHandler3();
    // The stack probe for frames over a page (ntdll exports it).
    fn __chkstk();
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn SetLastError(code: u32);
    fn LocalFree(memory: *mut c_void) -> *mut c_void;
}

mod etw;
mod security;

/// The C runtime's marker that floating point is used (the linker wants
/// it once core's float code is linked in).
#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

/// Frees local memory (LocalAlloc).
fn local_free(p: *mut c_void) {
    // SAFETY: memory from LocalAlloc (or null).
    unsafe { LocalFree(p) };
}

#[link(name = "bcryptprimitives", kind = "raw-dylib")]
unsafe extern "system" {
    fn ProcessPrng(data: *mut u8, length: usize) -> i32;
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

/// HKEY_CLASSES_ROOT (0x80000000) to HKEY_CURRENT_USER_LOCAL_SETTINGS
/// (0x80000007): the predefined keys.
fn predefined(key: usize) -> bool {
    (0x8000_0000..=0x8000_0007).contains(&key)
}

const ERROR_FILE_NOT_FOUND: i32 = 2;
const ERROR_INVALID_HANDLE: i32 = 6;

/// RegCloseKey: predefined keys close successfully (as on Windows); no
/// other key can have been opened.
#[unsafe(no_mangle)]
pub extern "system" fn RegCloseKey(key: usize) -> i32 {
    if predefined(key) { 0 } else { ERROR_INVALID_HANDLE }
}

/// RegOpenKeyExW / A, RegOpenKeyW / A: gala has no registry yet, so no key
/// below the predefined ones exists (ERROR_FILE_NOT_FOUND; programs treat
/// that as an ordinary outcome). Recorded in the ledger.
#[unsafe(no_mangle)]
pub extern "system" fn RegOpenKeyExW(
    key: usize,
    _subkey: *const u16,
    _options: u32,
    _access: u32,
    result: *mut usize,
) -> i32 {
    open_key(b"advapi32.dll!RegOpenKeyExW", key, result)
}

#[unsafe(no_mangle)]
pub extern "system" fn RegOpenKeyExA(
    key: usize,
    _subkey: *const u8,
    _options: u32,
    _access: u32,
    result: *mut usize,
) -> i32 {
    open_key(b"advapi32.dll!RegOpenKeyExA", key, result)
}

#[unsafe(no_mangle)]
pub extern "system" fn RegOpenKeyW(key: usize, _subkey: *const u16, result: *mut usize) -> i32 {
    open_key(b"advapi32.dll!RegOpenKeyW", key, result)
}

#[unsafe(no_mangle)]
pub extern "system" fn RegOpenKeyA(key: usize, _subkey: *const u8, result: *mut usize) -> i32 {
    open_key(b"advapi32.dll!RegOpenKeyA", key, result)
}

fn open_key(function: &[u8], key: usize, result: *mut usize) -> i32 {
    missing(function);
    if !result.is_null() {
        // SAFETY: the caller's output pointer.
        unsafe { *result = 0 };
    }
    if predefined(key) { ERROR_FILE_NOT_FOUND } else { ERROR_INVALID_HANDLE }
}

/// RegQueryValueExW / A: only predefined keys can be open, and they have
/// no values here.
#[unsafe(no_mangle)]
pub extern "system" fn RegQueryValueExW(
    key: usize,
    _name: *const u16,
    _reserved: *mut u32,
    _kind: *mut u32,
    _data: *mut u8,
    _size: *mut u32,
) -> i32 {
    missing(b"advapi32.dll!RegQueryValueExW");
    if predefined(key) { ERROR_FILE_NOT_FOUND } else { ERROR_INVALID_HANDLE }
}

#[unsafe(no_mangle)]
pub extern "system" fn RegQueryValueExA(
    key: usize,
    _name: *const u8,
    _reserved: *mut u32,
    _kind: *mut u32,
    _data: *mut u8,
    _size: *mut u32,
) -> i32 {
    missing(b"advapi32.dll!RegQueryValueExA");
    if predefined(key) { ERROR_FILE_NOT_FOUND } else { ERROR_INVALID_HANDLE }
}

/// RegGetValueW: the flags must allow some type (RRF_RT_*), and data
/// needs a size; then the key (or value) is not there, as for
/// RegOpenKeyExW and RegQueryValueExW.
#[unsafe(no_mangle)]
pub extern "system" fn RegGetValueW(
    key: usize,
    _subkey: *const u16,
    _value: *const u16,
    flags: u32,
    kind: *mut u32,
    data: *mut c_void,
    size: *mut u32,
) -> i32 {
    const ERROR_INVALID_PARAMETER: i32 = 87;
    if flags & 0xffff == 0 || (!data.is_null() && size.is_null()) {
        return ERROR_INVALID_PARAMETER;
    }
    missing(b"advapi32.dll!RegGetValueW");
    if !kind.is_null() {
        // SAFETY: the caller's output.
        unsafe { *kind = 0 };
    }
    if predefined(key) { ERROR_FILE_NOT_FOUND } else { ERROR_INVALID_HANDLE }
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

#[unsafe(no_mangle)]
pub extern "system" fn RegisterEventSourceW(_server: *const u16, _source: *const u16) -> usize {
    missing(b"advapi32.dll!RegisterEventSourceW");
    fail(ERROR_NOT_SUPPORTED, 0)
}

#[unsafe(no_mangle)]
pub extern "system" fn ReportEventW(
    _log: usize,
    _kind: u16,
    _category: u16,
    _id: u32,
    _sid: *mut c_void,
    _strings: u16,
    _size: u32,
    _text: *const *const u16,
    _data: *mut c_void,
) -> i32 {
    missing(b"advapi32.dll!ReportEventW");
    fail(ERROR_NOT_SUPPORTED, 0)
}

fn fail<T>(error: u32, value: T) -> T {
    // SAFETY: plain call.
    unsafe { SetLastError(error) };
    value
}

// ---- CryptoAPI ----

const CRYPT_VERIFYCONTEXT: u32 = 0xf000_0000;
/// The one provider handle gala hands out (a verify context: no keys).
const PROVIDER: usize = 0x6761_6c61; // "gala"
const NTE_BAD_KEYSET: u32 = 0x8009_0016;
const NTE_BAD_UID: u32 = 0x8009_0001;
const ERROR_NO_MORE_ITEMS: u32 = 259;

/// CryptAcquireContextW: a CRYPT_VERIFYCONTEXT provider (random numbers);
/// key containers are not supported (NTE_BAD_KEYSET, reported).
#[unsafe(no_mangle)]
pub extern "system" fn CryptAcquireContextW(
    provider: *mut usize,
    container: *const u16,
    _name: *const u16,
    _kind: u32,
    flags: u32,
) -> i32 {
    if flags & CRYPT_VERIFYCONTEXT == 0 || !container.is_null() {
        missing(b"advapi32.dll!CryptAcquireContextW(key container)");
        return fail(NTE_BAD_KEYSET, 0);
    }
    if provider.is_null() {
        return fail(87, 0);
    }
    // SAFETY: the caller's output pointer.
    unsafe { *provider = PROVIDER };
    1
}

#[unsafe(no_mangle)]
pub extern "system" fn CryptReleaseContext(provider: usize, _flags: u32) -> i32 {
    if provider == PROVIDER { 1 } else { fail(NTE_BAD_UID, 0) }
}

/// CryptGenRandom: from ProcessPrng (RDRAND/RDSEED), which never fails.
#[unsafe(no_mangle)]
pub extern "system" fn CryptGenRandom(provider: usize, length: u32, buffer: *mut u8) -> i32 {
    if provider != PROVIDER {
        return fail(NTE_BAD_UID, 0);
    }
    // SAFETY: the caller's buffer of `length` bytes.
    unsafe { ProcessPrng(buffer, length as usize) };
    1
}

/// CryptEnumProvidersW: gala lists no named providers.
#[unsafe(no_mangle)]
pub extern "system" fn CryptEnumProvidersW(
    _index: u32,
    _reserved: *mut u32,
    _flags: u32,
    _kind: *mut u32,
    _name: *mut u16,
    _size: *mut u32,
) -> i32 {
    fail(ERROR_NO_MORE_ITEMS, 0)
}

/// The rest of the CryptoAPI (keys, hashes, signatures): not yet.
macro_rules! not_yet {
    ($($name:ident($($arg:ident: $ty:ty),*);)*) => {$(
        #[unsafe(no_mangle)]
        pub extern "system" fn $name($($arg: $ty),*) -> i32 {
            $(let _ = $arg;)*
            missing(concat!("advapi32.dll!", stringify!($name)).as_bytes());
            fail(NTE_BAD_UID, 0)
        }
    )*};
}

not_yet! {
    CryptCreateHash(provider: usize, algorithm: u32, key: usize, flags: u32, hash: *mut usize);
    CryptDestroyHash(hash: usize);
    CryptSetHashParam(hash: usize, param: u32, data: *const u8, flags: u32);
    CryptSignHashW(hash: usize, key_spec: u32, description: *const u16, flags: u32, signature: *mut u8, length: *mut u32);
    CryptDecrypt(key: usize, hash: usize, last: i32, flags: u32, data: *mut u8, length: *mut u32);
    CryptDestroyKey(key: usize);
    CryptExportKey(key: usize, export: usize, kind: u32, flags: u32, data: *mut u8, length: *mut u32);
    CryptGetProvParam(provider: usize, param: u32, data: *mut u8, length: *mut u32, flags: u32);
    CryptGetUserKey(provider: usize, key_spec: u32, key: *mut usize);
}

// ---- Security and users (busybox-w32 imports these) ----

/// SystemFunction036 (RtlGenRandom): random bytes from ProcessPrng.
#[unsafe(no_mangle)]
pub extern "system" fn SystemFunction036(buffer: *mut u8, length: u32) -> u8 {
    // SAFETY: the caller's buffer of `length` bytes.
    unsafe { ProcessPrng(buffer, length as usize) };
    1
}

const ERROR_INSUFFICIENT_BUFFER: u32 = 122;
/// The one user name gala reports (there are no accounts).
const USER_NAME: &[u8] = b"gala";

/// GetUserNameA(lpBuffer, *pcbBuffer): the size includes the NUL.
#[unsafe(no_mangle)]
pub extern "system" fn GetUserNameA(buffer: *mut u8, size: *mut u32) -> i32 {
    if size.is_null() {
        return fail(87, 0);
    }
    let needed = USER_NAME.len() as u32 + 1;
    // SAFETY: the caller's size and buffer of *size bytes.
    unsafe {
        if buffer.is_null() || *size < needed {
            *size = needed;
            return fail(ERROR_INSUFFICIENT_BUFFER, 0);
        }
        memcpy(buffer, USER_NAME.as_ptr(), USER_NAME.len());
        *buffer.add(USER_NAME.len()) = 0;
        *size = needed;
    }
    1
}

/// EqualSid: same revision, authority and sub-authorities (a SID is 8
/// bytes plus 4 per sub-authority, their count at offset 1).
#[unsafe(no_mangle)]
pub extern "system" fn EqualSid(a: *const u8, b: *const u8) -> i32 {
    if a.is_null() || b.is_null() {
        return 0;
    }
    // SAFETY: the caller's SIDs; their length comes from their header.
    unsafe {
        let len = 8 + 4 * usize::from(*a.add(1));
        if *a.add(1) != *b.add(1) {
            return 0;
        }
        i32::from((0..len).all(|i| *a.add(i) == *b.add(i)))
    }
}

/// Tokens, access checks and Safer levels: not yet (no security model);
/// each fails with ERROR_NOT_SUPPORTED and is recorded.
macro_rules! no_security {
    ($($name:ident($($arg:ident: $ty:ty),*);)*) => {$(
        #[unsafe(no_mangle)]
        pub extern "system" fn $name($($arg: $ty),*) -> i32 {
            $(let _ = $arg;)*
            missing(concat!("advapi32.dll!", stringify!($name)).as_bytes());
            fail(ERROR_NOT_SUPPORTED, 0)
        }
    )*};
}

no_security! {
    SetTokenInformation(token: usize, class: u32, info: *const u8, length: u32);
    DuplicateToken(token: usize, level: u32, new: *mut usize);
    CheckTokenMembership(token: usize, sid: *const u8, member: *mut i32);
    AccessCheck(descriptor: *const u8, token: usize, access: u32, mapping: *const u8, privileges: *mut u8, length: *mut u32, granted: *mut u32, status: *mut i32);
    SaferCreateLevel(scope: u32, level: u32, flags: u32, handle: *mut usize, reserved: *mut c_void);
    SaferComputeTokenFromLevel(level: usize, token: usize, out: *mut usize, flags: u32, reserved: *mut c_void);
}

/// GetSecurityInfo: returns the error itself (not via GetLastError).
#[unsafe(no_mangle)]
pub extern "system" fn GetSecurityInfo(
    _handle: usize,
    _kind: u32,
    _info: u32,
    _owner: *mut usize,
    _group: *mut usize,
    _dacl: *mut usize,
    _sacl: *mut usize,
    _descriptor: *mut usize,
) -> u32 {
    missing(b"advapi32.dll!GetSecurityInfo");
    ERROR_NOT_SUPPORTED
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

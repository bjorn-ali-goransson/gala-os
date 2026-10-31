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
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn SetLastError(code: u32);
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
    OpenProcessToken(process: usize, access: u32, token: *mut usize);
    GetTokenInformation(token: usize, class: u32, info: *mut u8, length: u32, ret: *mut u32);
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

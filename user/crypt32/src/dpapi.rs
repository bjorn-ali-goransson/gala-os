//! DPAPI: CryptProtectData / CryptUnprotectData. Windows protects with the
//! user's master key; gala has one user and nothing persists across boots,
//! so the master key is 32 random bytes made on first use and kept in the
//! registry (HKLM\SOFTWARE\gala\DPAPI, MasterKey: in memory, readable by
//! that user's processes, which is DPAPI's scope). A blob is gala's own:
//! "GDP1", a random IV, an HMAC-SHA256 over the rest, then AES-256-CBC of
//! (description length, description, data); the keys come from the master
//! key and the optional entropy (HMAC-SHA256). The cryptography is gala's
//! bcrypt.

#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]
#![allow(clippy::too_many_arguments)]

use core::ffi::c_void;

// bcrypt's and advapi32's functions, loaded when DPAPI is first used (not
// imported: programs that load crypt32 for certificates need neither).
type OpenAlg = unsafe extern "system" fn(*mut usize, *const u16, *const u16, u32) -> u32;
type CloseAlg = unsafe extern "system" fn(usize, u32) -> u32;
type GenRandom = unsafe extern "system" fn(usize, *mut u8, u32, u32) -> u32;
type HashFn = unsafe extern "system" fn(usize, *const u8, u32, *const u8, u32, *mut u8, u32) -> u32;
type GenKey =
    unsafe extern "system" fn(usize, *mut usize, *mut u8, u32, *const u8, u32, u32) -> u32;
type Crypt = unsafe extern "system" fn(
    usize,
    *const u8,
    u32,
    *const c_void,
    *mut u8,
    u32,
    *mut u8,
    u32,
    *mut u32,
    u32,
) -> u32;
type DestroyKey = unsafe extern "system" fn(usize) -> u32;
type RegCreate = unsafe extern "system" fn(
    usize,
    *const u16,
    u32,
    *const u16,
    u32,
    u32,
    *const c_void,
    *mut usize,
    *mut u32,
) -> i32;
type RegQuery =
    unsafe extern "system" fn(usize, *const u16, *mut u32, *mut u32, *mut u8, *mut u32) -> i32;
type RegSet = unsafe extern "system" fn(usize, *const u16, u32, u32, *const u8, u32) -> i32;
type RegClose = unsafe extern "system" fn(usize) -> i32;

/// The functions DPAPI uses.
struct Api {
    open: OpenAlg,
    close: CloseAlg,
    random: GenRandom,
    hash: HashFn,
    key: GenKey,
    encrypt: Crypt,
    decrypt: Crypt,
    destroy: DestroyKey,
    reg_create: RegCreate,
    reg_query: RegQuery,
    reg_set: RegSet,
    reg_close: RegClose,
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn LoadLibraryA(name: *const u8) -> usize;
    fn GetProcAddress(module: usize, name: *const u8) -> usize;
}

fn api() -> Option<Api> {
    // SAFETY: loading system DLLs and resolving their documented functions
    // with the types they have.
    unsafe {
        let bcrypt = LoadLibraryA(c"bcrypt.dll".as_ptr().cast());
        let advapi = LoadLibraryA(c"advapi32.dll".as_ptr().cast());
        if bcrypt == 0 || advapi == 0 {
            return None;
        }
        let f = |m: usize, name: &core::ffi::CStr| {
            let p = GetProcAddress(m, name.as_ptr().cast());
            (p != 0).then_some(p)
        };
        Some(Api {
            open: core::mem::transmute::<usize, OpenAlg>(f(
                bcrypt,
                c"BCryptOpenAlgorithmProvider",
            )?),
            close: core::mem::transmute::<usize, CloseAlg>(f(
                bcrypt,
                c"BCryptCloseAlgorithmProvider",
            )?),
            random: core::mem::transmute::<usize, GenRandom>(f(bcrypt, c"BCryptGenRandom")?),
            hash: core::mem::transmute::<usize, HashFn>(f(bcrypt, c"BCryptHash")?),
            key: core::mem::transmute::<usize, GenKey>(f(bcrypt, c"BCryptGenerateSymmetricKey")?),
            encrypt: core::mem::transmute::<usize, Crypt>(f(bcrypt, c"BCryptEncrypt")?),
            decrypt: core::mem::transmute::<usize, Crypt>(f(bcrypt, c"BCryptDecrypt")?),
            destroy: core::mem::transmute::<usize, DestroyKey>(f(bcrypt, c"BCryptDestroyKey")?),
            reg_create: core::mem::transmute::<usize, RegCreate>(f(advapi, c"RegCreateKeyExW")?),
            reg_query: core::mem::transmute::<usize, RegQuery>(f(advapi, c"RegQueryValueExW")?),
            reg_set: core::mem::transmute::<usize, RegSet>(f(advapi, c"RegSetValueExW")?),
            reg_close: core::mem::transmute::<usize, RegClose>(f(advapi, c"RegCloseKey")?),
        })
    }
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn LocalAlloc(flags: u32, size: usize) -> *mut u8;
    fn LocalFree(p: *mut u8) -> *mut u8;
}

const HKEY_LOCAL_MACHINE: usize = 0x8000_0002;
const KEY_ALL_ACCESS: u32 = 0xf003f;
const REG_BINARY: u32 = 3;
const BCRYPT_ALG_HANDLE_HMAC_FLAG: u32 = 8;
const BCRYPT_USE_SYSTEM_PREFERRED_RNG: u32 = 2;
const BCRYPT_BLOCK_PADDING: u32 = 1;
const ERROR_INVALID_DATA: u32 = 13;
const ERROR_INVALID_PARAMETER: u32 = 87;
const ERROR_NOT_ENOUGH_MEMORY: u32 = 8;
const MAGIC: &[u8; 4] = b"GDP1";
/// Magic, IV, MAC.
const HEADER: usize = 4 + 16 + 32;

/// DATA_BLOB.
#[repr(C)]
pub struct Blob {
    size: u32,
    data: *mut u8,
}

fn blob<'a>(b: *const Blob) -> &'a [u8] {
    if b.is_null() {
        return &[];
    }
    // SAFETY: the caller's DATA_BLOB.
    unsafe {
        let b = &*b;
        if b.data.is_null() || b.size == 0 {
            &[]
        } else {
            core::slice::from_raw_parts(b.data, b.size as usize)
        }
    }
}

fn wide(text: &str) -> [u16; 32] {
    let mut out = [0u16; 32];
    for (slot, c) in out.iter_mut().zip(text.encode_utf16()) {
        *slot = c;
    }
    out
}

/// HMAC-SHA256(key, parts).
fn hmac(a: &Api, key: &[u8], parts: &[&[u8]]) -> Option<[u8; 32]> {
    let mut alg = 0;
    // SAFETY: bcrypt calls with buffers of ours.
    unsafe {
        let r = (a.open)(
            &mut alg,
            wide("SHA256").as_ptr(),
            core::ptr::null(),
            BCRYPT_ALG_HANDLE_HMAC_FLAG,
        );
        if r != 0 {
            return None;
        }
        let total: usize = parts.iter().map(|p| p.len()).sum();
        let buf = LocalAlloc(0, total.max(1));
        if buf.is_null() {
            (a.close)(alg, 0);
            return None;
        }
        let mut at = 0;
        for p in parts {
            core::ptr::copy_nonoverlapping(p.as_ptr(), buf.add(at), p.len());
            at += p.len();
        }
        let mut out = [0u8; 32];
        let r =
            (a.hash)(alg, key.as_ptr(), key.len() as u32, buf, total as u32, out.as_mut_ptr(), 32);
        LocalFree(buf);
        (a.close)(alg, 0);
        (r == 0).then_some(out)
    }
}

/// The user's master key (made on first use).
fn master_key(a: &Api) -> Option<[u8; 32]> {
    let mut key = 0;
    let path = wide("SOFTWARE\\gala\\DPAPI");
    let name = wide("MasterKey");
    let mut value = [0u8; 32];
    // SAFETY: registry and bcrypt calls with buffers of ours.
    unsafe {
        let r = (a.reg_create)(
            HKEY_LOCAL_MACHINE,
            path.as_ptr(),
            0,
            core::ptr::null(),
            0,
            KEY_ALL_ACCESS,
            core::ptr::null(),
            &mut key,
            core::ptr::null_mut(),
        );
        if r != 0 {
            return None;
        }
        let (mut kind, mut size) = (0u32, 32u32);
        let found = (a.reg_query)(
            key,
            name.as_ptr(),
            core::ptr::null_mut(),
            &mut kind,
            value.as_mut_ptr(),
            &mut size,
        );
        if found != 0 || size != 32 {
            (a.random)(0, value.as_mut_ptr(), 32, BCRYPT_USE_SYSTEM_PREFERRED_RNG);
            (a.reg_set)(key, name.as_ptr(), 0, REG_BINARY, value.as_ptr(), 32);
        }
        (a.reg_close)(key);
    }
    Some(value)
}

/// (encryption key, MAC key) for `entropy`.
fn keys(a: &Api, entropy: &[u8]) -> Option<([u8; 32], [u8; 32])> {
    let base = hmac(a, &master_key(a)?, &[b"gala DPAPI", entropy])?;
    Some((hmac(a, &base, &[b"encrypt"])?, hmac(a, &base, &[b"mac"])?))
}

fn fail<T>(code: u32, value: T) -> T {
    #[link(name = "kernel32", kind = "raw-dylib")]
    unsafe extern "system" {
        fn SetLastError(code: u32);
    }
    // SAFETY: plain call.
    unsafe { SetLastError(code) };
    value
}

/// AES-256-CBC (with padding) of `input` into a new LocalAlloc block;
/// returns (block, length).
fn aes(
    a: &Api,
    key: &[u8; 32],
    iv: &[u8; 16],
    input: &[u8],
    encrypt: bool,
) -> Option<(*mut u8, usize)> {
    let mut alg = 0;
    let mut k = 0;
    // SAFETY: bcrypt calls with buffers of ours.
    unsafe {
        if (a.open)(&mut alg, wide("AES").as_ptr(), core::ptr::null(), 0) != 0 {
            return None;
        }
        if (a.key)(alg, &mut k, core::ptr::null_mut(), 0, key.as_ptr(), 32, 0) != 0 {
            (a.close)(alg, 0);
            return None;
        }
        let out = LocalAlloc(0, input.len() + 16);
        let mut chain = *iv;
        let mut n = 0u32;
        let r = if out.is_null() {
            1
        } else if encrypt {
            (a.encrypt)(
                k,
                input.as_ptr(),
                input.len() as u32,
                core::ptr::null(),
                chain.as_mut_ptr(),
                16,
                out,
                input.len() as u32 + 16,
                &mut n,
                BCRYPT_BLOCK_PADDING,
            )
        } else {
            (a.decrypt)(
                k,
                input.as_ptr(),
                input.len() as u32,
                core::ptr::null(),
                chain.as_mut_ptr(),
                16,
                out,
                input.len() as u32 + 16,
                &mut n,
                BCRYPT_BLOCK_PADDING,
            )
        };
        (a.destroy)(k);
        (a.close)(alg, 0);
        if r != 0 {
            if !out.is_null() {
                LocalFree(out);
            }
            return None;
        }
        Some((out, n as usize))
    }
}

/// CryptProtectData: `input` with its description, protected for this
/// user (the CRYPTPROTECT_* flags change nothing here: one user, no UI).
#[unsafe(no_mangle)]
pub extern "system" fn CryptProtectData(
    input: *const Blob,
    description: *const u16,
    entropy: *const Blob,
    _reserved: *mut c_void,
    _prompt: *mut c_void,
    _flags: u32,
    output: *mut Blob,
) -> i32 {
    if input.is_null() || output.is_null() {
        return fail(ERROR_INVALID_PARAMETER, 0);
    }
    let Some(a) = api() else { return fail(ERROR_NOT_ENOUGH_MEMORY, 0) };
    let Some((enc, mac_key)) = keys(&a, blob(entropy)) else {
        return fail(ERROR_NOT_ENOUGH_MEMORY, 0);
    };
    let mut d = 0;
    if !description.is_null() {
        // SAFETY: the caller's NUL-terminated description.
        while unsafe { *description.add(d) } != 0 {
            d += 1;
        }
    }
    let data = blob(input);
    // The plaintext: description length (bytes), description, data.
    let plain_len = 4 + 2 * d + data.len();
    // SAFETY: allocations of ours; the caller's description and output.
    unsafe {
        let plain = LocalAlloc(0, plain_len);
        if plain.is_null() {
            return fail(ERROR_NOT_ENOUGH_MEMORY, 0);
        }
        plain.cast::<u32>().write_unaligned((2 * d) as u32);
        core::ptr::copy_nonoverlapping(description.cast::<u8>(), plain.add(4), 2 * d);
        core::ptr::copy_nonoverlapping(data.as_ptr(), plain.add(4 + 2 * d), data.len());
        let mut iv = [0u8; 16];
        (a.random)(0, iv.as_mut_ptr(), 16, BCRYPT_USE_SYSTEM_PREFERRED_RNG);
        let sealed = aes(&a, &enc, &iv, core::slice::from_raw_parts(plain, plain_len), true);
        LocalFree(plain);
        let Some((ct, n)) = sealed else { return fail(ERROR_NOT_ENOUGH_MEMORY, 0) };
        let ct_slice = core::slice::from_raw_parts(ct, n);
        let Some(mac) = hmac(&a, &mac_key, &[MAGIC, &iv, ct_slice]) else {
            LocalFree(ct);
            return fail(ERROR_NOT_ENOUGH_MEMORY, 0);
        };
        let out = LocalAlloc(0, HEADER + n);
        if out.is_null() {
            LocalFree(ct);
            return fail(ERROR_NOT_ENOUGH_MEMORY, 0);
        }
        core::ptr::copy_nonoverlapping(MAGIC.as_ptr(), out, 4);
        core::ptr::copy_nonoverlapping(iv.as_ptr(), out.add(4), 16);
        core::ptr::copy_nonoverlapping(mac.as_ptr(), out.add(20), 32);
        core::ptr::copy_nonoverlapping(ct, out.add(HEADER), n);
        LocalFree(ct);
        output.write(Blob { size: (HEADER + n) as u32, data: out });
    }
    1
}

/// CryptUnprotectData: the data (and description) back, if the blob is
/// gala's, unchanged, and the entropy is the same; else ERROR_INVALID_DATA.
#[unsafe(no_mangle)]
pub extern "system" fn CryptUnprotectData(
    input: *const Blob,
    description: *mut *mut u16,
    entropy: *const Blob,
    _reserved: *mut c_void,
    _prompt: *mut c_void,
    _flags: u32,
    output: *mut Blob,
) -> i32 {
    if input.is_null() || output.is_null() {
        return fail(ERROR_INVALID_PARAMETER, 0);
    }
    let data = blob(input);
    if data.len() < HEADER + 16 || &data[..4] != MAGIC {
        return fail(ERROR_INVALID_DATA, 0);
    }
    let Some(a) = api() else { return fail(ERROR_NOT_ENOUGH_MEMORY, 0) };
    let Some((enc, mac_key)) = keys(&a, blob(entropy)) else {
        return fail(ERROR_NOT_ENOUGH_MEMORY, 0);
    };
    let (iv, mac, ct) = (&data[4..20], &data[20..52], &data[HEADER..]);
    let Some(expected) = hmac(&a, &mac_key, &[MAGIC, iv, ct]) else {
        return fail(ERROR_NOT_ENOUGH_MEMORY, 0);
    };
    if expected != mac {
        return fail(ERROR_INVALID_DATA, 0);
    }
    let mut iv16 = [0u8; 16];
    iv16.copy_from_slice(iv);
    let Some((plain, n)) = aes(&a, &enc, &iv16, ct, false) else {
        return fail(ERROR_INVALID_DATA, 0);
    };
    // SAFETY: the decrypted block of `n` bytes; allocations of ours; the
    // caller's outputs.
    unsafe {
        let d = if n >= 4 { plain.cast::<u32>().read_unaligned() as usize } else { usize::MAX };
        if d > n.saturating_sub(4) {
            LocalFree(plain);
            return fail(ERROR_INVALID_DATA, 0);
        }
        let len = n - 4 - d;
        let out = LocalAlloc(0, len.max(1));
        if out.is_null() {
            LocalFree(plain);
            return fail(ERROR_NOT_ENOUGH_MEMORY, 0);
        }
        core::ptr::copy_nonoverlapping(plain.add(4 + d), out, len);
        if !description.is_null() {
            let text = LocalAlloc(0x40, d + 2).cast::<u16>(); // zeroed: NUL-terminated
            if !text.is_null() {
                core::ptr::copy_nonoverlapping(plain.add(4), text.cast::<u8>(), d);
            }
            description.write(text);
        }
        LocalFree(plain);
        output.write(Blob { size: len as u32, data: out });
    }
    1
}

//! gala's bcrypt.dll: the CNG primitives .NET uses most, as documented -
//! random numbers (BCryptGenRandom, from bcryptprimitives' ProcessPrng)
//! and hashes: MD5, SHA-1, SHA-256/384/512 and their HMACs, through
//! algorithm providers or the documented pseudo-handles, incremental
//! (create, hash data, finish, duplicate, reusable) or one shot. The
//! digests come from the RustCrypto crates (THIRD_PARTY.md). Symmetric
//! ciphers, keys and key derivation are not there yet: those functions
//! fail with STATUS_NOT_SUPPORTED, recorded in the ledger.
//!
//! Objects (providers, hashes) live on the process heap and are tagged;
//! a hash's caller-supplied object buffer is not used.

#![no_std]
#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use core::ffi::c_void;
use hmac::{Hmac, Mac};
use sha2::Digest;

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

/// MSVC-target codegen references this marker symbol directly.
#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn GalaRecordMissing(name: *const u8, length: usize, what: u32, detail: u64) -> u32;
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetProcessHeap() -> usize;
    fn HeapAlloc(heap: usize, flags: u32, size: usize) -> *mut u8;
    fn HeapFree(heap: usize, flags: u32, p: *mut u8) -> i32;
}

#[link(name = "bcryptprimitives", kind = "raw-dylib")]
unsafe extern "system" {
    fn ProcessPrng(data: *mut u8, length: usize) -> i32;
}

type Status = u32;
const STATUS_SUCCESS: Status = 0;
const STATUS_INVALID_HANDLE: Status = 0xc000_0008;
const STATUS_INVALID_PARAMETER: Status = 0xc000_000d;
const STATUS_NO_MEMORY: Status = 0xc000_0017;
const STATUS_BUFFER_TOO_SMALL: Status = 0xc000_0023;
const STATUS_NOT_SUPPORTED: Status = 0xc000_00bb;
const STATUS_NOT_FOUND: Status = 0xc000_0225;

const BCRYPT_ALG_HANDLE_HMAC_FLAG: u32 = 0x8;
const BCRYPT_HASH_REUSABLE_FLAG: u32 = 0x20;
const BCRYPT_USE_SYSTEM_PREFERRED_RNG: u32 = 0x2;

fn missing(function: &[u8], detail: u64) {
    // SAFETY: a valid buffer.
    unsafe { GalaRecordMissing(function.as_ptr(), function.len(), 2, detail) };
}

/// The algorithms gala provides.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Md5,
    Sha1,
    Sha256,
    Sha384,
    Sha512,
    Rng,
}

impl Kind {
    fn digest_len(self) -> u32 {
        match self {
            Kind::Md5 => 16,
            Kind::Sha1 => 20,
            Kind::Sha256 => 32,
            Kind::Sha384 => 48,
            Kind::Sha512 => 64,
            Kind::Rng => 0,
        }
    }

    fn block_len(self) -> u32 {
        match self {
            Kind::Sha384 | Kind::Sha512 => 128,
            Kind::Rng => 0,
            _ => 64,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Kind::Md5 => "MD5",
            Kind::Sha1 => "SHA1",
            Kind::Sha256 => "SHA256",
            Kind::Sha384 => "SHA384",
            Kind::Sha512 => "SHA512",
            Kind::Rng => "RNG",
        }
    }
}

/// An algorithm provider (or what a pseudo-handle stands for).
#[derive(Clone, Copy)]
struct Algorithm {
    kind: Kind,
    hmac: bool,
}

const ALG_MAGIC: u32 = 0x6761_6c61; // "gala"
const HASH_MAGIC: u32 = 0x6761_6c68;

#[repr(C)]
struct Provider {
    magic: u32,
    alg: Algorithm,
}

/// The algorithm a handle names: a provider of ours or a pseudo-handle
/// ("CNG Algorithm Pseudo-handles": 0x21 MD5 ... 0xd1 HMAC-SHA512).
fn algorithm(h: usize) -> Option<Algorithm> {
    let pseudo = |kind, hmac| Some(Algorithm { kind, hmac });
    match h {
        0x21 => pseudo(Kind::Md5, false),
        0x31 => pseudo(Kind::Sha1, false),
        0x41 => pseudo(Kind::Sha256, false),
        0x51 => pseudo(Kind::Sha384, false),
        0x61 => pseudo(Kind::Sha512, false),
        0x81 => pseudo(Kind::Rng, false),
        0x91 => pseudo(Kind::Md5, true),
        0xa1 => pseudo(Kind::Sha1, true),
        0xb1 => pseudo(Kind::Sha256, true),
        0xc1 => pseudo(Kind::Sha384, true),
        0xd1 => pseudo(Kind::Sha512, true),
        0 => None,
        _ if h < 0x1000 => None,
        _ => {
            // SAFETY: a handle the caller got from us (tagged).
            let p = unsafe { &*(h as *const Provider) };
            (p.magic == ALG_MAGIC).then_some(p.alg)
        }
    }
}

fn new_object<T>(value: T) -> *mut T {
    // SAFETY: plain call.
    let p = unsafe { HeapAlloc(GetProcessHeap(), 0, core::mem::size_of::<T>()) }.cast::<T>();
    if !p.is_null() {
        // SAFETY: fresh memory of T's size (the heap aligns to 16).
        unsafe { p.write(value) };
    }
    p
}

fn free_object<T>(p: *mut T) {
    // SAFETY: an object of ours, dropped then freed.
    unsafe {
        core::ptr::drop_in_place(p);
        HeapFree(GetProcessHeap(), 0, p.cast());
    }
}

/// A UTF-16 string the caller passed, compared ASCII case-insensitively.
fn wide_is(p: *const u16, text: &str) -> bool {
    if p.is_null() {
        return false;
    }
    // SAFETY: the caller's NUL-terminated string, read up to len + 1.
    unsafe {
        text.bytes().enumerate().all(|(i, b)| {
            let c = *p.add(i);
            c < 128 && (c as u8).eq_ignore_ascii_case(&b)
        }) && *p.add(text.len()) == 0
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn BCryptOpenAlgorithmProvider(
    out: *mut usize,
    id: *const u16,
    _implementation: *const u16,
    flags: u32,
) -> Status {
    if out.is_null() || id.is_null() {
        return STATUS_INVALID_PARAMETER;
    }
    let kinds = [Kind::Md5, Kind::Sha1, Kind::Sha256, Kind::Sha384, Kind::Sha512, Kind::Rng];
    let Some(&kind) = kinds.iter().find(|k| wide_is(id, k.name())) else {
        missing(b"bcrypt.dll!BCryptOpenAlgorithmProvider(algorithm)", 0);
        return STATUS_NOT_FOUND;
    };
    let hmac = flags & BCRYPT_ALG_HANDLE_HMAC_FLAG != 0;
    if hmac && kind == Kind::Rng {
        return STATUS_NOT_FOUND;
    }
    let p = new_object(Provider { magic: ALG_MAGIC, alg: Algorithm { kind, hmac } });
    if p.is_null() {
        return STATUS_NO_MEMORY;
    }
    // SAFETY: the caller's output.
    unsafe { *out = p as usize };
    STATUS_SUCCESS
}

#[unsafe(no_mangle)]
pub extern "system" fn BCryptCloseAlgorithmProvider(h: usize, _flags: u32) -> Status {
    if h < 0x1000 || algorithm(h).is_none() {
        return STATUS_INVALID_HANDLE;
    }
    // SAFETY: a provider of ours: untag it before freeing.
    unsafe { (*(h as *mut Provider)).magic = 0 };
    free_object(h as *mut Provider);
    STATUS_SUCCESS
}

#[unsafe(no_mangle)]
pub extern "system" fn BCryptGenRandom(h: usize, buf: *mut u8, len: u32, flags: u32) -> Status {
    let ok = if flags & BCRYPT_USE_SYSTEM_PREFERRED_RNG != 0 {
        h == 0
    } else {
        algorithm(h).is_some_and(|a| a.kind == Kind::Rng)
    };
    if !ok {
        return STATUS_INVALID_HANDLE;
    }
    if buf.is_null() && len > 0 {
        return STATUS_INVALID_PARAMETER;
    }
    // SAFETY: the caller's buffer of `len` bytes.
    unsafe { ProcessPrng(buf, len as usize) };
    STATUS_SUCCESS
}

/// A hash in progress.
#[derive(Clone)]
enum State {
    Md5(md5::Md5),
    Sha1(sha1::Sha1),
    Sha256(sha2::Sha256),
    Sha384(sha2::Sha384),
    Sha512(sha2::Sha512),
    HmacMd5(Hmac<md5::Md5>),
    HmacSha1(Hmac<sha1::Sha1>),
    HmacSha256(Hmac<sha2::Sha256>),
    HmacSha384(Hmac<sha2::Sha384>),
    HmacSha512(Hmac<sha2::Sha512>),
}

impl State {
    fn new(alg: Algorithm, key: &[u8]) -> Option<State> {
        Some(match (alg.kind, alg.hmac) {
            (Kind::Md5, false) => State::Md5(Default::default()),
            (Kind::Sha1, false) => State::Sha1(Default::default()),
            (Kind::Sha256, false) => State::Sha256(Default::default()),
            (Kind::Sha384, false) => State::Sha384(Default::default()),
            (Kind::Sha512, false) => State::Sha512(Default::default()),
            (Kind::Md5, true) => State::HmacMd5(Hmac::new_from_slice(key).ok()?),
            (Kind::Sha1, true) => State::HmacSha1(Hmac::new_from_slice(key).ok()?),
            (Kind::Sha256, true) => State::HmacSha256(Hmac::new_from_slice(key).ok()?),
            (Kind::Sha384, true) => State::HmacSha384(Hmac::new_from_slice(key).ok()?),
            (Kind::Sha512, true) => State::HmacSha512(Hmac::new_from_slice(key).ok()?),
            (Kind::Rng, _) => return None,
        })
    }

    fn update(&mut self, data: &[u8]) {
        match self {
            State::Md5(s) => s.update(data),
            State::Sha1(s) => s.update(data),
            State::Sha256(s) => s.update(data),
            State::Sha384(s) => s.update(data),
            State::Sha512(s) => s.update(data),
            State::HmacMd5(s) => s.update(data),
            State::HmacSha1(s) => s.update(data),
            State::HmacSha256(s) => s.update(data),
            State::HmacSha384(s) => s.update(data),
            State::HmacSha512(s) => s.update(data),
        }
    }

    /// The digest into `out` (its exact length).
    fn finish(self, out: &mut [u8]) {
        match self {
            State::Md5(s) => out.copy_from_slice(&s.finalize()),
            State::Sha1(s) => out.copy_from_slice(&s.finalize()),
            State::Sha256(s) => out.copy_from_slice(&s.finalize()),
            State::Sha384(s) => out.copy_from_slice(&s.finalize()),
            State::Sha512(s) => out.copy_from_slice(&s.finalize()),
            State::HmacMd5(s) => out.copy_from_slice(&s.finalize().into_bytes()),
            State::HmacSha1(s) => out.copy_from_slice(&s.finalize().into_bytes()),
            State::HmacSha256(s) => out.copy_from_slice(&s.finalize().into_bytes()),
            State::HmacSha384(s) => out.copy_from_slice(&s.finalize().into_bytes()),
            State::HmacSha512(s) => out.copy_from_slice(&s.finalize().into_bytes()),
        }
    }
}

/// A hash object: its state, and for a reusable hash the fresh state to
/// start over from.
#[repr(C)]
struct Hash {
    magic: u32,
    alg: Algorithm,
    state: State,
    fresh: Option<State>,
}

fn hash<'a>(h: usize) -> Option<&'a mut Hash> {
    if h < 0x1000 {
        return None;
    }
    // SAFETY: a handle the caller got from us (tagged).
    let p = unsafe { &mut *(h as *mut Hash) };
    (p.magic == HASH_MAGIC).then_some(p)
}

/// The caller's bytes (empty for a null pointer).
fn bytes<'a>(p: *const u8, len: u32) -> &'a [u8] {
    if p.is_null() || len == 0 {
        &[]
    } else {
        // SAFETY: the caller's buffer of `len` bytes.
        unsafe { core::slice::from_raw_parts(p, len as usize) }
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn BCryptCreateHash(
    h: usize,
    out: *mut usize,
    _object: *mut u8,
    _object_len: u32,
    secret: *const u8,
    secret_len: u32,
    flags: u32,
) -> Status {
    let Some(alg) = algorithm(h).filter(|a| a.kind != Kind::Rng) else {
        return STATUS_INVALID_HANDLE;
    };
    if out.is_null() {
        return STATUS_INVALID_PARAMETER;
    }
    let Some(state) = State::new(alg, bytes(secret, secret_len)) else {
        return STATUS_INVALID_PARAMETER;
    };
    let fresh = (flags & BCRYPT_HASH_REUSABLE_FLAG != 0).then(|| state.clone());
    let p = new_object(Hash { magic: HASH_MAGIC, alg, state, fresh });
    if p.is_null() {
        return STATUS_NO_MEMORY;
    }
    // SAFETY: the caller's output.
    unsafe { *out = p as usize };
    STATUS_SUCCESS
}

#[unsafe(no_mangle)]
pub extern "system" fn BCryptHashData(h: usize, data: *const u8, len: u32, _flags: u32) -> Status {
    let Some(hash) = hash(h) else { return STATUS_INVALID_HANDLE };
    if data.is_null() && len > 0 {
        return STATUS_INVALID_PARAMETER;
    }
    hash.state.update(bytes(data, len));
    STATUS_SUCCESS
}

/// BCryptFinishHash: the output must be exactly the digest's length. A
/// reusable hash starts over; another is spent (only destroying is left).
#[unsafe(no_mangle)]
pub extern "system" fn BCryptFinishHash(h: usize, out: *mut u8, len: u32, _flags: u32) -> Status {
    let Some(hash) = hash(h) else { return STATUS_INVALID_HANDLE };
    if out.is_null() || len != hash.alg.kind.digest_len() {
        return STATUS_INVALID_PARAMETER;
    }
    let next = match &hash.fresh {
        Some(fresh) => fresh.clone(),
        None => hash.state.clone(),
    };
    let state = core::mem::replace(&mut hash.state, next);
    // SAFETY: the caller's buffer of `len` bytes.
    state.finish(unsafe { core::slice::from_raw_parts_mut(out, len as usize) });
    STATUS_SUCCESS
}

#[unsafe(no_mangle)]
pub extern "system" fn BCryptDuplicateHash(
    h: usize,
    out: *mut usize,
    _object: *mut u8,
    _object_len: u32,
    _flags: u32,
) -> Status {
    let Some(hash) = hash(h) else { return STATUS_INVALID_HANDLE };
    if out.is_null() {
        return STATUS_INVALID_PARAMETER;
    }
    let copy = Hash {
        magic: HASH_MAGIC,
        alg: hash.alg,
        state: hash.state.clone(),
        fresh: hash.fresh.clone(),
    };
    let p = new_object(copy);
    if p.is_null() {
        return STATUS_NO_MEMORY;
    }
    // SAFETY: the caller's output.
    unsafe { *out = p as usize };
    STATUS_SUCCESS
}

#[unsafe(no_mangle)]
pub extern "system" fn BCryptDestroyHash(h: usize) -> Status {
    let Some(hash) = hash(h) else { return STATUS_INVALID_HANDLE };
    hash.magic = 0;
    free_object(h as *mut Hash);
    STATUS_SUCCESS
}

/// BCryptHash: create, hash and finish in one call.
#[unsafe(no_mangle)]
pub extern "system" fn BCryptHash(
    h: usize,
    secret: *const u8,
    secret_len: u32,
    input: *const u8,
    input_len: u32,
    out: *mut u8,
    out_len: u32,
) -> Status {
    let Some(alg) = algorithm(h).filter(|a| a.kind != Kind::Rng) else {
        return STATUS_INVALID_HANDLE;
    };
    if out.is_null() || out_len != alg.kind.digest_len() || (input.is_null() && input_len > 0) {
        return STATUS_INVALID_PARAMETER;
    }
    let Some(mut state) = State::new(alg, bytes(secret, secret_len)) else {
        return STATUS_INVALID_PARAMETER;
    };
    state.update(bytes(input, input_len));
    // SAFETY: the caller's buffer of `out_len` bytes.
    state.finish(unsafe { core::slice::from_raw_parts_mut(out, out_len as usize) });
    STATUS_SUCCESS
}

/// BCryptGetProperty of a provider or hash: ObjectLength, HashDigestLength,
/// HashBlockLength (DWORDs) and AlgorithmName.
#[unsafe(no_mangle)]
pub extern "system" fn BCryptGetProperty(
    h: usize,
    name: *const u16,
    out: *mut u8,
    len: u32,
    result: *mut u32,
    _flags: u32,
) -> Status {
    let Some(alg) = algorithm(h).or_else(|| hash(h).map(|x| x.alg)) else {
        return STATUS_INVALID_HANDLE;
    };
    if result.is_null() {
        return STATUS_INVALID_PARAMETER;
    }
    let dword = if wide_is(name, "ObjectLength") {
        Some(core::mem::size_of::<Hash>() as u32)
    } else if wide_is(name, "HashDigestLength") {
        Some(alg.kind.digest_len())
    } else if wide_is(name, "HashBlockLength") {
        Some(alg.kind.block_len())
    } else {
        None
    };
    let mut text = [0u8; 16];
    let value: &[u8] = match dword {
        Some(v) => {
            text[..4].copy_from_slice(&v.to_le_bytes());
            &text[..4]
        }
        None if wide_is(name, "AlgorithmName") => {
            let n = alg.kind.name();
            for (i, b) in n.bytes().enumerate() {
                text[2 * i] = b;
            }
            &text[..2 * n.len() + 2]
        }
        None => {
            missing(b"bcrypt.dll!BCryptGetProperty(property)", 0);
            return STATUS_NOT_SUPPORTED;
        }
    };
    // SAFETY: the caller's outputs.
    unsafe {
        *result = value.len() as u32;
        if out.is_null() {
            return STATUS_SUCCESS;
        }
        if (len as usize) < value.len() {
            return STATUS_BUFFER_TOO_SMALL;
        }
        core::ptr::copy_nonoverlapping(value.as_ptr(), out, value.len());
    }
    STATUS_SUCCESS
}

/// Keys, ciphers and key derivation: not there yet (recorded).
macro_rules! not_supported {
    ($($name:ident($($arg:ident: $ty:ty),*);)*) => {$(
        #[unsafe(no_mangle)]
        pub extern "system" fn $name($($arg: $ty),*) -> Status {
            $(let _ = $arg;)*
            missing(concat!("bcrypt.dll!", stringify!($name)).as_bytes(), 0);
            STATUS_NOT_SUPPORTED
        }
    )*};
}

not_supported! {
    BCryptSetProperty(h: usize, name: *const u16, input: *const u8, len: u32, flags: u32);
    BCryptGenerateSymmetricKey(h: usize, key: *mut usize, object: *mut u8, object_len: u32, secret: *const u8, secret_len: u32, flags: u32);
    BCryptEncrypt(key: usize, input: *const u8, len: u32, padding: *const c_void, iv: *mut u8, iv_len: u32, out: *mut u8, out_len: u32, result: *mut u32, flags: u32);
    BCryptDecrypt(key: usize, input: *const u8, len: u32, padding: *const c_void, iv: *mut u8, iv_len: u32, out: *mut u8, out_len: u32, result: *mut u32, flags: u32);
    BCryptDestroyKey(key: usize);
    BCryptKeyDerivation(key: usize, params: *const c_void, out: *mut u8, len: u32, result: *mut u32, flags: u32);
    BCryptDeriveKeyPBKDF2(prf: usize, password: *const u8, password_len: u32, salt: *const u8, salt_len: u32, iterations: u64, out: *mut u8, len: u32, flags: u32);
    BCryptImportKey(h: usize, import: usize, blob: *const u16, key: *mut usize, object: *mut u8, object_len: u32, input: *const u8, len: u32, flags: u32);
    BCryptImportKeyPair(h: usize, import: usize, blob: *const u16, key: *mut usize, input: *const u8, len: u32, flags: u32);
    BCryptExportKey(key: usize, export: usize, blob: *const u16, out: *mut u8, len: u32, result: *mut u32, flags: u32);
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

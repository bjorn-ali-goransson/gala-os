//! gala's bcrypt.dll: the CNG primitives .NET uses most, as documented -
//! random numbers (BCryptGenRandom, from bcryptprimitives' ProcessPrng)
//! and hashes: MD5, SHA-1, SHA-256/384/512 and their HMACs, through
//! algorithm providers or the documented pseudo-handles, incremental
//! (create, hash data, finish, duplicate, reusable) or one shot. The
//! digests come from the RustCrypto crates (THIRD_PARTY.md); MD2 and MD4
//! too. Symmetric ciphers (AES, DES, 3DES, 3DES_112, RC2, RC4) with keys
//! (`cipher`). RSA key pairs, signatures and encryption (`rsa`). Key
//! derivation is not there: it fails with STATUS_NOT_SUPPORTED, recorded
//! in the ledger.
//!
//! Objects (providers, hashes, keys) live on the process heap and are
//! tagged; a hash's caller-supplied object buffer is not used.
//!
//! The `rsa` crate needs `alloc`: the global allocator is the process
//! heap (`gala_heap`). Its big-number arithmetic allocates infallibly, so
//! running out of memory there ends in the panic handler (the thread
//! spins) instead of an error status; everything else in this DLL checks
//! its allocations.

#![no_std]
#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

extern crate alloc;

mod cipher;
mod rsa;
mod unimplemented;

// The global allocator (for the `rsa` crate).
use gala_heap as _;

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
/// BCryptGetFipsAlgorithmMode: whether FIPS mode is enforced; gala has
/// no such policy (the registry's FipsAlgorithmPolicy is not set).
#[unsafe(no_mangle)]
pub extern "system" fn BCryptGetFipsAlgorithmMode(enabled: *mut u8) -> u32 {
    if enabled.is_null() {
        return 0xc000_000d; // STATUS_INVALID_PARAMETER
    }
    // SAFETY: the caller's output (a BOOLEAN).
    unsafe { enabled.write(0) };
    0
}

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

pub type Status = u32;
pub const STATUS_SUCCESS: Status = 0;
pub const STATUS_INVALID_HANDLE: Status = 0xc000_0008;
pub const STATUS_INVALID_PARAMETER: Status = 0xc000_000d;
pub const STATUS_NO_MEMORY: Status = 0xc000_0017;
pub const STATUS_BUFFER_TOO_SMALL: Status = 0xc000_0023;
pub const STATUS_NOT_SUPPORTED: Status = 0xc000_00bb;
const STATUS_NOT_FOUND: Status = 0xc000_0225;

const BCRYPT_ALG_HANDLE_HMAC_FLAG: u32 = 0x8;
const BCRYPT_HASH_REUSABLE_FLAG: u32 = 0x20;
const BCRYPT_USE_SYSTEM_PREFERRED_RNG: u32 = 0x2;

pub fn missing(function: &[u8], detail: u64) {
    // SAFETY: a valid buffer.
    unsafe { GalaRecordMissing(function.as_ptr(), function.len(), 2, detail) };
}

/// The algorithms gala provides.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Md5,
    Sha1,
    Sha256,
    Sha384,
    Sha512,
    Md2,
    Md4,
    Rng,
    Aes,
    Des,
    Des3,
    Des3_112,
    Rc2,
    Rc4,
    /// RSA: the provider opens; its key operations are not there yet
    /// (recorded).
    Rsa,
}

/// A block cipher's chaining mode (BCRYPT_CHAINING_MODE).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Cbc,
    Ecb,
}

impl Mode {
    fn name(self) -> &'static str {
        match self {
            Mode::Cbc => "ChainingModeCBC",
            Mode::Ecb => "ChainingModeECB",
        }
    }
}

impl Kind {
    const ALL: [Kind; 15] = [
        Kind::Md5,
        Kind::Sha1,
        Kind::Sha256,
        Kind::Sha384,
        Kind::Sha512,
        Kind::Md2,
        Kind::Md4,
        Kind::Rng,
        Kind::Aes,
        Kind::Des,
        Kind::Des3,
        Kind::Des3_112,
        Kind::Rc2,
        Kind::Rc4,
        Kind::Rsa,
    ];

    fn is_hash(self) -> bool {
        matches!(
            self,
            Kind::Md5
                | Kind::Sha1
                | Kind::Sha256
                | Kind::Sha384
                | Kind::Sha512
                | Kind::Md2
                | Kind::Md4
        )
    }

    pub fn is_symmetric(self) -> bool {
        matches!(self, Kind::Aes | Kind::Des | Kind::Des3 | Kind::Des3_112 | Kind::Rc2 | Kind::Rc4)
    }

    fn digest_len(self) -> u32 {
        match self {
            Kind::Md5 | Kind::Md2 | Kind::Md4 => 16,
            Kind::Sha1 => 20,
            Kind::Sha256 => 32,
            Kind::Sha384 => 48,
            Kind::Sha512 => 64,
            _ => 0,
        }
    }

    /// A hash's block length, or a cipher's (RC4: 1).
    pub fn block_len(self) -> u32 {
        match self {
            Kind::Sha384 | Kind::Sha512 => 128,
            Kind::Md2 | Kind::Aes => 16,
            Kind::Des | Kind::Des3 | Kind::Des3_112 | Kind::Rc2 => 8,
            Kind::Rc4 => 1,
            Kind::Rng | Kind::Rsa => 0,
            _ => 64,
        }
    }

    /// BCRYPT_KEY_LENGTHS_STRUCT (bits): min, max, increment.
    fn key_lengths(self) -> Option<[u32; 3]> {
        Some(match self {
            Kind::Aes => [128, 256, 64],
            Kind::Des => [64, 64, 0],
            Kind::Des3 => [192, 192, 0],
            Kind::Des3_112 => [128, 128, 0],
            Kind::Rc2 => [16, 128, 8],
            Kind::Rc4 => [8, 512, 8],
            Kind::Rsa => [512, 16384, 64],
            _ => return None,
        })
    }

    fn name(self) -> &'static str {
        match self {
            Kind::Md5 => "MD5",
            Kind::Sha1 => "SHA1",
            Kind::Sha256 => "SHA256",
            Kind::Sha384 => "SHA384",
            Kind::Sha512 => "SHA512",
            Kind::Md2 => "MD2",
            Kind::Md4 => "MD4",
            Kind::Rng => "RNG",
            Kind::Aes => "AES",
            Kind::Des => "DES",
            Kind::Des3 => "3DES",
            Kind::Des3_112 => "3DES_112",
            Kind::Rc2 => "RC2",
            Kind::Rc4 => "RC4",
            Kind::Rsa => "RSA",
        }
    }
}

/// An algorithm provider (or what a pseudo-handle stands for): for block
/// ciphers, the chaining mode and (RC2) effective key length new keys get.
#[derive(Clone, Copy)]
pub struct Algorithm {
    kind: Kind,
    hmac: bool,
    pub mode: Mode,
    pub effective_bits: u32,
}

impl Algorithm {
    fn new(kind: Kind, hmac: bool) -> Algorithm {
        // RC4 is a stream: no chaining mode. RC2's effective key length
        // defaults to 128 bits.
        Algorithm { kind, hmac, mode: Mode::Cbc, effective_bits: 128 }
    }
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
pub fn algorithm(h: usize) -> Option<Algorithm> {
    let pseudo = |kind, hmac| Some(Algorithm::new(kind, hmac));
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

pub fn new_object<T>(value: T) -> *mut T {
    // SAFETY: plain call.
    let p = unsafe { HeapAlloc(GetProcessHeap(), 0, core::mem::size_of::<T>()) }.cast::<T>();
    if !p.is_null() {
        // SAFETY: fresh memory of T's size (the heap aligns to 16).
        unsafe { p.write(value) };
    }
    p
}

pub fn free_object<T>(p: *mut T) {
    // SAFETY: an object of ours, dropped then freed.
    unsafe {
        core::ptr::drop_in_place(p);
        HeapFree(GetProcessHeap(), 0, p.cast());
    }
}

/// A UTF-16 string the caller passed, compared ASCII case-insensitively.
pub fn wide_is(p: *const u16, text: &str) -> bool {
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
    let Some(&kind) = Kind::ALL.iter().find(|k| wide_is(id, k.name())) else {
        // The ledger names the algorithm: `bcrypt.dll!algorithm <name>`.
        let mut text = [0u8; 64];
        let prefix = b"bcrypt.dll!algorithm ";
        text[..prefix.len()].copy_from_slice(prefix);
        let mut n = prefix.len();
        // SAFETY: the caller's NUL-terminated name, read up to its NUL.
        while n < text.len() && unsafe { *id.add(n - prefix.len()) } != 0 {
            text[n] = unsafe { *id.add(n - prefix.len()) } as u8;
            n += 1;
        }
        missing(&text[..n], u64::from(flags));
        return STATUS_NOT_FOUND;
    };
    let hmac = flags & BCRYPT_ALG_HANDLE_HMAC_FLAG != 0;
    if hmac && !kind.is_hash() {
        return STATUS_NOT_FOUND;
    }
    let p = new_object(Provider { magic: ALG_MAGIC, alg: Algorithm::new(kind, hmac) });
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
    Md2(md2::Md2),
    Md4(md4::Md4),
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
            (Kind::Md2, false) => State::Md2(Default::default()),
            (Kind::Md4, false) => State::Md4(Default::default()),
            (Kind::Md5, true) => State::HmacMd5(Hmac::new_from_slice(key).ok()?),
            (Kind::Sha1, true) => State::HmacSha1(Hmac::new_from_slice(key).ok()?),
            (Kind::Sha256, true) => State::HmacSha256(Hmac::new_from_slice(key).ok()?),
            (Kind::Sha384, true) => State::HmacSha384(Hmac::new_from_slice(key).ok()?),
            (Kind::Sha512, true) => State::HmacSha512(Hmac::new_from_slice(key).ok()?),
            _ => return None,
        })
    }

    fn update(&mut self, data: &[u8]) {
        match self {
            State::Md5(s) => s.update(data),
            State::Sha1(s) => s.update(data),
            State::Sha256(s) => s.update(data),
            State::Sha384(s) => s.update(data),
            State::Sha512(s) => s.update(data),
            State::Md2(s) => s.update(data),
            State::Md4(s) => s.update(data),
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
            State::Md2(s) => out.copy_from_slice(&s.finalize()),
            State::Md4(s) => out.copy_from_slice(&s.finalize()),
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
pub fn bytes<'a>(p: *const u8, len: u32) -> &'a [u8] {
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
    let Some(alg) = algorithm(h).filter(|a| a.kind.is_hash()) else {
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
    let Some(alg) = algorithm(h).filter(|a| a.kind.is_hash()) else {
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

/// What a handle names, for properties: a provider (or pseudo-handle), a
/// hash or a symmetric key.
enum Object<'a> {
    Algorithm(Algorithm),
    Hash(&'a mut Hash),
    Key(&'a mut cipher::Key),
    RsaKey(&'a mut rsa::RsaKey),
}

fn object<'a>(h: usize) -> Option<Object<'a>> {
    if let Some(a) = algorithm(h) {
        return Some(Object::Algorithm(a));
    }
    if let Some(x) = hash(h) {
        return Some(Object::Hash(x));
    }
    if let Some(k) = rsa::key(h) {
        return Some(Object::RsaKey(k));
    }
    cipher::key(h).map(Object::Key)
}

/// BCryptGetProperty of a provider, hash or key: ObjectLength,
/// HashDigestLength, HashBlockLength, BlockLength, KeyLength (keys),
/// KeyLengths, ChainingMode, EffectiveKeyLength (RC2), AlgorithmName.
#[unsafe(no_mangle)]
pub extern "system" fn BCryptGetProperty(
    h: usize,
    name: *const u16,
    out: *mut u8,
    len: u32,
    result: *mut u32,
    _flags: u32,
) -> Status {
    let Some(obj) = object(h) else { return STATUS_INVALID_HANDLE };
    if result.is_null() {
        return STATUS_INVALID_PARAMETER;
    }
    let (kind, mode, effective, key_bits) = match &obj {
        Object::Algorithm(a) => (a.kind, a.mode, a.effective_bits, None),
        Object::Hash(x) => (x.alg.kind, Mode::Cbc, 0, None),
        Object::Key(k) => (k.kind, k.mode, 0, Some(k.key_bits())),
        Object::RsaKey(k) => (Kind::Rsa, Mode::Cbc, 0, Some(k.bits())),
    };
    let mut text = [0u8; 64];
    let dword = |text: &mut [u8; 64], v: u32| {
        text[..4].copy_from_slice(&v.to_le_bytes());
        4
    };
    let string = |text: &mut [u8; 64], v: &str| {
        for (i, b) in v.bytes().enumerate() {
            text[2 * i] = b;
            text[2 * i + 1] = 0;
        }
        text[2 * v.len()] = 0;
        text[2 * v.len() + 1] = 0;
        2 * v.len() + 2
    };
    let n = if wide_is(name, "ObjectLength") {
        let size =
            if kind.is_symmetric() { cipher::object_length() } else { size_of::<Hash>() as u32 };
        dword(&mut text, size)
    } else if wide_is(name, "HashDigestLength") && kind.is_hash() {
        dword(&mut text, kind.digest_len())
    } else if (wide_is(name, "HashBlockLength") && kind.is_hash())
        || (wide_is(name, "BlockLength") && kind.is_symmetric())
    {
        dword(&mut text, kind.block_len())
    } else if (wide_is(name, "KeyLength") || (wide_is(name, "KeyStrength") && kind == Kind::Rsa))
        && key_bits.is_some()
    {
        dword(&mut text, key_bits.unwrap_or(0))
    } else if wide_is(name, "BlockLength") && kind == Kind::Rsa && key_bits.is_some() {
        dword(&mut text, key_bits.unwrap_or(0).div_ceil(8))
    } else if wide_is(name, "EffectiveKeyLength") && kind == Kind::Rc2 && key_bits.is_none() {
        dword(&mut text, effective)
    } else if wide_is(name, "KeyLengths") && kind.key_lengths().is_some() {
        let lens = kind.key_lengths().unwrap_or([0; 3]);
        for (i, v) in lens.iter().enumerate() {
            text[4 * i..4 * i + 4].copy_from_slice(&v.to_le_bytes());
        }
        12
    } else if wide_is(name, "ChainingMode") && kind.is_symmetric() && kind != Kind::Rc4 {
        string(&mut text, mode.name())
    } else if wide_is(name, "AlgorithmName") {
        string(&mut text, kind.name())
    } else {
        missing(b"bcrypt.dll!BCryptGetProperty(property)", 0);
        return STATUS_NOT_SUPPORTED;
    };
    let value = &text[..n];
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

/// BCryptSetProperty of a provider or key: ChainingMode (CBC or ECB; other
/// modes recorded) and EffectiveKeyLength (RC2).
#[unsafe(no_mangle)]
pub extern "system" fn BCryptSetProperty(
    h: usize,
    name: *const u16,
    input: *const u8,
    len: u32,
    _flags: u32,
) -> Status {
    let value = bytes(input, len);
    let mode = if wide_is(name, "ChainingMode") {
        let p = input.cast::<u16>();
        Some(if wide_is(p, "ChainingModeCBC") {
            Mode::Cbc
        } else if wide_is(p, "ChainingModeECB") {
            Mode::Ecb
        } else {
            missing(b"bcrypt.dll!BCryptSetProperty(chaining mode)", 0);
            return STATUS_NOT_SUPPORTED;
        })
    } else {
        None
    };
    let effective = if wide_is(name, "EffectiveKeyLength") {
        let Some(b) = value.get(..4) else { return STATUS_INVALID_PARAMETER };
        Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    } else {
        None
    };
    if mode.is_none() && effective.is_none() {
        missing(b"bcrypt.dll!BCryptSetProperty(property)", 0);
        return STATUS_NOT_SUPPORTED;
    }
    if let Some(k) = cipher::key(h) {
        if let Some(m) = mode {
            k.mode = m;
        }
        if let Some(bits) = effective {
            return k.set_effective_bits(bits);
        }
        return STATUS_SUCCESS;
    }
    if h < 0x1000 || algorithm(h).is_none() {
        return STATUS_INVALID_HANDLE;
    }
    // SAFETY: a provider of ours (checked).
    let p = unsafe { &mut *(h as *mut Provider) };
    if !p.alg.kind.is_symmetric() || (mode.is_some() && p.alg.kind == Kind::Rc4) {
        return STATUS_INVALID_PARAMETER;
    }
    if let Some(m) = mode {
        p.alg.mode = m;
    }
    if let Some(bits) = effective {
        if p.alg.kind != Kind::Rc2 || !(1..=1024).contains(&bits) {
            return STATUS_INVALID_PARAMETER;
        }
        p.alg.effective_bits = bits;
    }
    STATUS_SUCCESS
}

/// BCryptExportKey: symmetric keys as key data blobs; RSA keys are not
/// there yet.
#[unsafe(no_mangle)]
pub extern "system" fn BCryptExportKey(
    key: usize,
    export: usize,
    blob: *const u16,
    out: *mut u8,
    len: u32,
    result: *mut u32,
    _flags: u32,
) -> Status {
    if export != 0 {
        missing(b"bcrypt.dll!BCryptExportKey(wrapped)", 0);
        return STATUS_NOT_SUPPORTED;
    }
    if let Some(k) = rsa::key(key) {
        return rsa::export(k, blob, out, len, result);
    }
    let Some(k) = cipher::key(key) else { return STATUS_INVALID_HANDLE };
    cipher::export(k, blob, out, len, result)
}

/// A handle that is not a symmetric key: an RSA key, or invalid.
pub fn rsa_destroy(h: usize) -> Status {
    rsa::destroy(h)
}

#[allow(clippy::too_many_arguments)]
pub fn rsa_crypt(
    h: usize,
    encrypt: bool,
    input: *const u8,
    len: u32,
    padding: *const u8,
    out: *mut u8,
    out_len: u32,
    result: *mut u32,
    flags: u32,
) -> Status {
    rsa::crypt(h, encrypt, input, len, padding, out, out_len, result, flags)
}

/// Key derivation: not there yet (recorded).
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
    BCryptKeyDerivation(key: usize, params: *const c_void, out: *mut u8, len: u32, result: *mut u32, flags: u32);
    BCryptDeriveKeyPBKDF2(prf: usize, password: *const u8, password_len: u32, salt: *const u8, salt_len: u32, iterations: u64, out: *mut u8, len: u32, flags: u32);
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

//! Keys of the Microsoft Software Key Storage Provider: RSA and ECDSA
//! (P-256, P-384) private keys imported (NCryptImportKey: PKCS#8
//! PrivateKeyInfo, as NCRYPT_PKCS8_PRIVATE_KEY_BLOB, or bcrypt's
//! RSAPRIVATEBLOB / RSAFULLPRIVATEBLOB / ECCPRIVATEBLOB), named ones kept
//! under their name (NCRYPTBUFFER_PKCS_KEY_NAME) and opened again by it
//! (NCryptOpenKey), deleted (NCryptDeleteKey), and signing
//! (NCryptSignHash) with bcrypt's RSA or ECDSA. A key handle owns a
//! bcrypt key made from the stored blob (an RSAPRIVATEBLOB or an
//! ECCPRIVATEBLOB; its magic says which).
//!
//! TODO: named keys live as long as the process (Windows keeps them in
//! the user's profile, across processes); other algorithms (P-521, DSA,
//! DH), a PKCS#8 EC key without its public point, encrypted PKCS#8 and
//! export are not supported (recorded).

use core::sync::atomic::{AtomicBool, Ordering};

use super::{
    NTE_INVALID_HANDLE, NTE_INVALID_PARAMETER, NTE_NOT_SUPPORTED, software_provider, unsupported,
};

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetProcessHeap() -> usize;
    fn HeapAlloc(heap: usize, flags: u32, size: usize) -> *mut u8;
    fn HeapFree(heap: usize, flags: u32, block: *mut u8) -> i32;
}

const NTE_BAD_KEYSET: i32 = 0x8009_0016u32 as i32;
const NTE_EXISTS: i32 = 0x8009_000fu32 as i32;
const NTE_NO_MEMORY: i32 = 0x8009_000eu32 as i32;
const NTE_BAD_DATA: i32 = 0x8009_0005u32 as i32;
const NCRYPTBUFFER_PKCS_KEY_NAME: u32 = 45;
const NCRYPTBUFFER_PKCS_SECRET: u32 = 46;
const NCRYPT_OVERWRITE_KEY_FLAG: u32 = 0x80;

const KEY_MAGIC: u32 = 0x6761_6c6b; // "galk"
const NAME_MAX: usize = 128;
const SLOTS: usize = 32;

fn alloc(size: usize) -> *mut u8 {
    // SAFETY: plain allocation, zeroed.
    unsafe { HeapAlloc(GetProcessHeap(), 8, size) }
}

fn free(p: *mut u8) {
    if !p.is_null() {
        // SAFETY: a block from `alloc`.
        unsafe { HeapFree(GetProcessHeap(), 0, p) };
    }
}

/// A named key in the store: its name and private blob.
struct Entry {
    name: [u16; NAME_MAX],
    name_len: usize,
    blob: *mut u8,
    blob_len: usize,
}

struct Store {
    slots: [*mut Entry; SLOTS],
}

/// The store, behind `LOCK`.
struct Shared(core::cell::UnsafeCell<Store>);

// SAFETY: only reached through `with_store`, under the lock.
unsafe impl Sync for Shared {}

static LOCK: AtomicBool = AtomicBool::new(false);
static STORE: Shared =
    Shared(core::cell::UnsafeCell::new(Store { slots: [core::ptr::null_mut(); SLOTS] }));

/// Runs `f` with the store locked.
fn with_store<R>(f: impl FnOnce(&mut Store) -> R) -> R {
    while LOCK.compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        core::hint::spin_loop();
    }
    // SAFETY: the lock gives this thread the only reference.
    let r = f(unsafe { &mut *STORE.0.get() });
    LOCK.store(false, Ordering::Release);
    r
}

/// A key handle (NCRYPT_KEY_HANDLE points at it).
#[repr(C)]
struct Key {
    magic: u32,
    name: [u16; NAME_MAX],
    name_len: usize,
    /// bcrypt's algorithm and the key made from the blob.
    algorithm: usize,
    key: usize,
    bits: u32,
    kind: Algorithm,
}

/// A key's algorithm, from its blob's magic.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Algorithm {
    Rsa,
    EcdsaP256,
    EcdsaP384,
}

const RSA_PRIVATE_MAGIC: u32 = 0x3241_5352; // "RSA2"
const ECDSA_P256_PRIVATE_MAGIC: u32 = 0x3253_4345; // "ECS2"
const ECDSA_P384_PRIVATE_MAGIC: u32 = 0x3453_4345; // "ECS4"

impl Algorithm {
    /// The algorithm of a private blob, and its size in bits.
    fn of(blob: &[u8]) -> Option<(Algorithm, u32)> {
        let word = |i: usize| Some(u32::from_le_bytes(blob.get(i..i + 4)?.try_into().ok()?));
        match word(0)? {
            RSA_PRIVATE_MAGIC => Some((Algorithm::Rsa, word(4)?)),
            ECDSA_P256_PRIVATE_MAGIC => Some((Algorithm::EcdsaP256, 256)),
            ECDSA_P384_PRIVATE_MAGIC => Some((Algorithm::EcdsaP384, 384)),
            _ => None,
        }
    }

    /// bcrypt's algorithm id, NCRYPT_ALGORITHM_PROPERTY.
    fn name(self) -> &'static str {
        match self {
            Algorithm::Rsa => "RSA",
            Algorithm::EcdsaP256 => "ECDSA_P256",
            Algorithm::EcdsaP384 => "ECDSA_P384",
        }
    }

    /// NCRYPT_ALGORITHM_GROUP_PROPERTY.
    fn group(self) -> &'static str {
        match self {
            Algorithm::Rsa => "RSA",
            _ => "ECDSA",
        }
    }

    /// bcrypt's blob type for the stored blob.
    fn blob_type(self) -> &'static str {
        match self {
            Algorithm::Rsa => "RSAPRIVATEBLOB",
            _ => "ECCPRIVATEBLOB",
        }
    }
}

fn key<'a>(h: usize) -> Option<&'a mut Key> {
    if h == 0 || !h.is_multiple_of(8) || h == software_provider() {
        return None;
    }
    // SAFETY: a handle we gave out points at a Key; others fail the magic
    // check (callers pass handles from this provider).
    let k = unsafe { &mut *(h as *mut Key) };
    (k.magic == KEY_MAGIC).then_some(k)
}

/// Whether `h` is one of our key handles.
pub fn is_key(h: usize) -> bool {
    key(h).is_some()
}

type ImportFn =
    extern "system" fn(usize, usize, *const u16, *mut usize, *const u8, u32, u32) -> i32;

/// A bcrypt key of a private blob of `kind`: (algorithm, key).
fn bcrypt_key(kind: Algorithm, blob: &[u8]) -> Option<(usize, usize)> {
    type OpenFn = extern "system" fn(*mut usize, *const u16, *const u16, u32) -> i32;
    let open = super::bcrypt(b"BCryptOpenAlgorithmProvider\0");
    let import = super::bcrypt(b"BCryptImportKeyPair\0");
    if open == 0 || import == 0 {
        return None;
    }
    let name = wide::<16>(kind.name());
    let blob_type = wide::<16>(kind.blob_type());
    let (mut alg, mut k) = (0usize, 0usize);
    // SAFETY: bcrypt.dll's functions of these signatures; our buffers.
    unsafe {
        let open = core::mem::transmute::<usize, OpenFn>(open);
        if open(&mut alg, name.as_ptr(), core::ptr::null(), 0) != 0 {
            return None;
        }
        let import = core::mem::transmute::<usize, ImportFn>(import);
        if import(alg, 0, blob_type.as_ptr(), &mut k, blob.as_ptr(), blob.len() as u32, 0) != 0 {
            return None;
        }
    }
    Some((alg, k))
}

fn wide<const N: usize>(s: &str) -> [u16; N] {
    let mut w = [0u16; N];
    for (d, b) in w.iter_mut().zip(s.bytes()) {
        *d = b.into();
    }
    w
}

/// A new handle for a key of `blob` named `name`.
fn new_handle(name: &[u16], blob: &[u8]) -> Result<usize, i32> {
    let Some((kind, bits)) = Algorithm::of(blob) else { return Err(NTE_BAD_DATA) };
    let Some((algorithm, k)) = bcrypt_key(kind, blob) else { return Err(NTE_BAD_DATA) };
    let p = alloc(size_of::<Key>()).cast::<Key>();
    if p.is_null() {
        return Err(NTE_NO_MEMORY);
    }
    let mut n = [0u16; NAME_MAX];
    n[..name.len()].copy_from_slice(name);
    // SAFETY: a fresh block of a Key.
    unsafe {
        p.write(Key {
            magic: KEY_MAGIC,
            name: n,
            name_len: name.len(),
            algorithm,
            key: k,
            bits,
            kind,
        })
    };
    Ok(p as usize)
}

fn destroy(k: &mut Key) {
    type Destroy = extern "system" fn(usize) -> i32;
    type Close = extern "system" fn(usize, u32) -> i32;
    let (d, c) =
        (super::bcrypt(b"BCryptDestroyKey\0"), super::bcrypt(b"BCryptCloseAlgorithmProvider\0"));
    // SAFETY: bcrypt.dll's functions; our handles.
    unsafe {
        if d != 0 {
            core::mem::transmute::<usize, Destroy>(d)(k.key);
        }
        if c != 0 {
            core::mem::transmute::<usize, Close>(c)(k.algorithm, 0);
        }
    }
    k.magic = 0;
    free((k as *mut Key).cast());
}

/// Frees a key handle (NCryptFreeObject).
pub fn free_key(h: usize) -> i32 {
    match key(h) {
        Some(k) => {
            destroy(k);
            0
        }
        None => NTE_INVALID_HANDLE,
    }
}

/// A DER element: (tag, content, rest).
fn der(b: &[u8]) -> Option<(u8, &[u8], &[u8])> {
    let (&tag, b) = b.split_first()?;
    let (&first, mut b) = b.split_first()?;
    let len = if first < 0x80 {
        usize::from(first)
    } else {
        let n = usize::from(first & 0x7f);
        if n == 0 || n > 3 || b.len() < n {
            return None;
        }
        let mut len = 0;
        for &x in &b[..n] {
            len = len << 8 | usize::from(x);
        }
        b = &b[n..];
        len
    };
    (b.len() >= len).then(|| (tag, &b[..len], &b[len..]))
}

/// An INTEGER's magnitude, without leading zeros.
fn integer(b: &[u8]) -> Option<(&[u8], &[u8])> {
    let (tag, mut v, rest) = der(b)?;
    if tag != 2 {
        return None;
    }
    while v.len() > 1 && v[0] == 0 {
        v = &v[1..];
    }
    Some((v, rest))
}

const RSA_OID: [u8; 9] = [0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 1, 1, 1];
/// id-ecPublicKey (RFC 5480) and the named curves prime256v1, secp384r1.
const EC_OID: [u8; 7] = [0x2a, 0x86, 0x48, 0xce, 0x3d, 2, 1];
const P256_OID: [u8; 8] = [0x2a, 0x86, 0x48, 0xce, 0x3d, 3, 1, 7];
const P384_OID: [u8; 5] = [0x2b, 0x81, 0x04, 0, 34];

/// The private blob (RSAPRIVATEBLOB or ECCPRIVATEBLOB) of a PKCS#8
/// PrivateKeyInfo, into `out`; its length.
fn pkcs8_to_blob(p8: &[u8], out: &mut [u8]) -> Option<usize> {
    let (tag, info, _) = der(p8)?;
    if tag != 0x30 {
        return None;
    }
    let (_, rest) = integer(info)?; // version
    let (tag, alg, rest) = der(rest)?;
    let (oid_tag, oid, params) = der(alg)?;
    let (key_tag, key, _) = der(rest)?;
    if tag != 0x30 || oid_tag != 6 || key_tag != 4 {
        return None;
    }
    if oid == EC_OID {
        return ec_pkcs8_to_blob(params, key, out);
    }
    if oid != RSA_OID {
        unsupported(b"ncrypt.dll!NCryptImportKey(PKCS8 key other than RSA and ECDSA)");
        return None;
    }
    let (tag, seq, _) = der(key)?;
    if tag != 0x30 {
        return None;
    }
    let (_, r) = integer(seq)?; // version
    let (n, r) = integer(r)?;
    let (e, r) = integer(r)?;
    let (_d, r) = integer(r)?;
    let (p, r) = integer(r)?;
    let (q, _) = integer(r)?;
    let total = 24 + e.len() + n.len() + p.len() + q.len();
    if total > out.len() {
        return None;
    }
    let bits = (n.len() * 8) as u32 - n[0].leading_zeros();
    let header =
        [0x3241_5352u32, bits, e.len() as u32, n.len() as u32, p.len() as u32, q.len() as u32];
    for (i, v) in header.iter().enumerate() {
        out[4 * i..4 * i + 4].copy_from_slice(&v.to_le_bytes());
    }
    let mut at = 24;
    for part in [e, n, p, q] {
        out[at..at + part.len()].copy_from_slice(part);
        at += part.len();
    }
    Some(at)
}

/// The ECCPRIVATEBLOB of a PKCS#8 EC key: the curve from the
/// algorithm's parameters (a named curve), then RFC 5915's ECPrivateKey
/// (version 1, the private scalar, [0] parameters, [1] the public point,
/// which the blob needs: a key without it is recorded and refused).
fn ec_pkcs8_to_blob(params: &[u8], key: &[u8], out: &mut [u8]) -> Option<usize> {
    let (tag, curve, _) = der(params)?;
    let (magic, size) = match curve {
        c if tag == 6 && c == P256_OID => (ECDSA_P256_PRIVATE_MAGIC, 32),
        c if tag == 6 && c == P384_OID => (ECDSA_P384_PRIVATE_MAGIC, 48),
        _ => {
            unsupported(b"ncrypt.dll!NCryptImportKey(PKCS8 EC curve other than P-256 and P-384)");
            return None;
        }
    };
    let (tag, seq, _) = der(key)?;
    if tag != 0x30 {
        return None;
    }
    let (_, rest) = integer(seq)?; // version 1
    let (tag, d, mut rest) = der(rest)?;
    if tag != 4 || d.len() > size {
        return None;
    }
    let mut point = None;
    while !rest.is_empty() {
        let (tag, value, next) = der(rest)?;
        if tag == 0xa1 {
            // BIT STRING: no unused bits, then 04 || X || Y.
            let (tag, bits, _) = der(value)?;
            if tag != 3 || bits.len() != 2 + 2 * size || bits[0] != 0 || bits[1] != 4 {
                return None;
            }
            point = Some(&bits[2..]);
        }
        rest = next;
    }
    let Some(point) = point else {
        unsupported(b"ncrypt.dll!NCryptImportKey(PKCS8 EC key without its public key)");
        return None;
    };
    let total = 8 + 3 * size;
    if total > out.len() {
        return None;
    }
    out[..4].copy_from_slice(&magic.to_le_bytes());
    out[4..8].copy_from_slice(&(size as u32).to_le_bytes());
    out[8..8 + 2 * size].copy_from_slice(point);
    // d, left-padded to the curve's size.
    let at = 8 + 2 * size;
    out[at..total].fill(0);
    out[total - d.len()..total].copy_from_slice(d);
    Some(total)
}

/// The UTF-16 string of `size` bytes at `p` (without a trailing NUL).
fn name_of<'a>(p: *const u16, size: usize) -> &'a [u16] {
    if p.is_null() {
        return &[];
    }
    // SAFETY: the caller's buffer of `size` bytes.
    let s = unsafe { core::slice::from_raw_parts(p, size / 2) };
    let end = s.iter().position(|&c| c == 0).unwrap_or(s.len());
    &s[..end]
}

fn wide_eq(a: *const u16, want: &str) -> bool {
    if a.is_null() {
        return false;
    }
    let mut i = 0;
    for b in want.bytes() {
        // SAFETY: the caller's NUL-terminated string, read up to its NUL.
        if unsafe { *a.add(i) } != u16::from(b) {
            return false;
        }
        i += 1;
    }
    // SAFETY: as above.
    unsafe { *a.add(i) == 0 }
}

/// NCryptBufferDesc's buffers.
#[repr(C)]
pub struct NBuffer {
    size: u32,
    kind: u32,
    data: *const u8,
}

#[repr(C)]
pub struct NBufferDesc {
    version: u32,
    count: u32,
    buffers: *const NBuffer,
}

#[unsafe(no_mangle)]
pub extern "system" fn NCryptImportKey(
    provider: usize,
    import_key: usize,
    blob_type: *const u16,
    params: *const NBufferDesc,
    out: *mut usize,
    data: *const u8,
    size: u32,
    flags: u32,
) -> i32 {
    if provider != software_provider() {
        return NTE_INVALID_HANDLE;
    }
    if out.is_null() || data.is_null() || import_key != 0 {
        if import_key != 0 {
            unsupported(b"ncrypt.dll!NCryptImportKey(wrapped key)");
        }
        return NTE_INVALID_PARAMETER;
    }
    // SAFETY: the caller's blob.
    let input = unsafe { core::slice::from_raw_parts(data, size as usize) };
    let mut name: &[u16] = &[];
    if !params.is_null() {
        // SAFETY: the caller's descriptor and buffers.
        let list = unsafe {
            let d = &*params;
            if d.buffers.is_null() {
                &[][..]
            } else {
                core::slice::from_raw_parts(d.buffers, d.count as usize)
            }
        };
        for b in list {
            match b.kind {
                NCRYPTBUFFER_PKCS_KEY_NAME => name = name_of(b.data.cast(), b.size as usize),
                NCRYPTBUFFER_PKCS_SECRET => {
                    unsupported(b"ncrypt.dll!NCryptImportKey(encrypted PKCS8)");
                    return NTE_NOT_SUPPORTED;
                }
                _ => {}
            }
        }
    }
    if name.len() > NAME_MAX {
        return NTE_INVALID_PARAMETER;
    }
    let mut converted = [0u8; 2048];
    let blob: &[u8] = if wide_eq(blob_type, "PKCS8_PRIVATEKEY") {
        match pkcs8_to_blob(input, &mut converted) {
            Some(n) => &converted[..n],
            None => return NTE_BAD_DATA,
        }
    } else if wide_eq(blob_type, "RSAPRIVATEBLOB") || wide_eq(blob_type, "RSAFULLPRIVATEBLOB") {
        // The private blob's first part (header, e, n, p, q) is the
        // RSAPRIVATEBLOB of either.
        if input.len() < 24 {
            return NTE_BAD_DATA;
        }
        let f = |i: usize| {
            u32::from_le_bytes([input[i], input[i + 1], input[i + 2], input[i + 3]]) as usize
        };
        let n = 24 + f(8) + f(12) + f(16) + f(20);
        if n > input.len() || n > converted.len() {
            return NTE_BAD_DATA;
        }
        converted[..n].copy_from_slice(&input[..n]);
        converted[..4].copy_from_slice(&0x3241_5352u32.to_le_bytes());
        &converted[..n]
    } else if wide_eq(blob_type, "ECCPRIVATEBLOB") {
        // ECCPRIVATEBLOB: magic, cbKey, X, Y, d.
        let n = if input.len() >= 8 {
            8 + 3 * u32::from_le_bytes([input[4], input[5], input[6], input[7]]) as usize
        } else {
            0
        };
        if n < 8 || n != input.len() || n > converted.len() {
            return NTE_BAD_DATA;
        }
        converted[..n].copy_from_slice(input);
        &converted[..n]
    } else {
        unsupported(b"ncrypt.dll!NCryptImportKey(blob type)");
        return NTE_NOT_SUPPORTED;
    };
    if Algorithm::of(blob).is_none() {
        return NTE_BAD_DATA;
    }
    if !name.is_empty() {
        let stored = with_store(|s| {
            let at = s.slots.iter().position(|&e| {
                // SAFETY: entries of the store.
                !e.is_null() && unsafe { (&(*e).name)[..(*e).name_len] == *name }
            });
            if at.is_some() && flags & NCRYPT_OVERWRITE_KEY_FLAG == 0 {
                return Err(NTE_EXISTS);
            }
            let slot = match at {
                Some(i) => {
                    remove(s, i);
                    i
                }
                None => s.slots.iter().position(|e| e.is_null()).ok_or(NTE_NO_MEMORY)?,
            };
            let e = alloc(size_of::<Entry>()).cast::<Entry>();
            let b = alloc(blob.len());
            if e.is_null() || b.is_null() {
                free(e.cast());
                free(b);
                return Err(NTE_NO_MEMORY);
            }
            let mut n = [0u16; NAME_MAX];
            n[..name.len()].copy_from_slice(name);
            // SAFETY: fresh blocks of the right sizes.
            unsafe {
                core::ptr::copy_nonoverlapping(blob.as_ptr(), b, blob.len());
                e.write(Entry { name: n, name_len: name.len(), blob: b, blob_len: blob.len() });
            }
            s.slots[slot] = e;
            Ok(())
        });
        if let Err(e) = stored {
            return e;
        }
    }
    match new_handle(name, blob) {
        Ok(h) => {
            // SAFETY: the caller's output.
            unsafe { out.write(h) };
            0
        }
        Err(e) => e,
    }
}

fn remove(s: &mut Store, i: usize) {
    let e = s.slots[i];
    if !e.is_null() {
        // SAFETY: an entry of the store.
        free(unsafe { (*e).blob });
        free(e.cast());
        s.slots[i] = core::ptr::null_mut();
    }
}

/// NCryptOpenKey: a named key of the store.
pub fn open(out: *mut usize, name: *const u16) -> i32 {
    let mut n = 0;
    // SAFETY: the caller's NUL-terminated name.
    unsafe {
        while !name.is_null() && n < NAME_MAX && *name.add(n) != 0 {
            n += 1;
        }
    }
    let wanted = name_of(name, 2 * n);
    let mut blob = [0u8; 2048];
    let found = with_store(|s| {
        s.slots.iter().find_map(|&e| {
            // SAFETY: entries of the store.
            let entry = unsafe { e.as_ref()? };
            (&entry.name[..entry.name_len] == wanted).then(|| {
                // SAFETY: the entry's blob.
                let b = unsafe { core::slice::from_raw_parts(entry.blob, entry.blob_len) };
                blob[..b.len()].copy_from_slice(b);
                b.len()
            })
        })
    });
    let Some(len) = found else { return NTE_BAD_KEYSET };
    match new_handle(wanted, &blob[..len]) {
        Ok(h) => {
            // SAFETY: the caller's output.
            unsafe { out.write(h) };
            0
        }
        Err(e) => e,
    }
}

/// NCryptDeleteKey: removes a named key from the store and frees the
/// handle.
pub fn delete(h: usize) -> i32 {
    let Some(k) = key(h) else { return NTE_INVALID_HANDLE };
    let name = &k.name[..k.name_len];
    with_store(|s| {
        if let Some(i) = s.slots.iter().position(|&e| {
            // SAFETY: entries of the store.
            !e.is_null() && unsafe { (&(*e).name)[..(*e).name_len] == *name }
        }) {
            remove(s, i);
        }
    });
    destroy(k);
    0
}

/// NCryptSignHash: bcrypt's BCryptSignHash with the key (the same padding
/// structures and flags).
#[unsafe(no_mangle)]
pub extern "system" fn NCryptSignHash(
    h: usize,
    padding: *const u8,
    hash: *const u8,
    hash_size: u32,
    signature: *mut u8,
    capacity: u32,
    result: *mut u32,
    flags: u32,
) -> i32 {
    type Sign =
        extern "system" fn(usize, *const u8, *const u8, u32, *mut u8, u32, *mut u32, u32) -> i32;
    let Some(k) = key(h) else { return NTE_INVALID_HANDLE };
    let f = super::bcrypt(b"BCryptSignHash\0");
    if f == 0 {
        return NTE_NOT_SUPPORTED;
    }
    // NCRYPT_SILENT_FLAG (0x40) means nothing to bcrypt.
    // SAFETY: bcrypt.dll's function of this signature; the caller's buffers.
    let status = unsafe {
        core::mem::transmute::<usize, Sign>(f)(
            k.key,
            padding,
            hash,
            hash_size,
            signature,
            capacity,
            result,
            flags & !0x40,
        )
    };
    // NTSTATUS to SECURITY_STATUS.
    match status as u32 {
        0 => 0,
        0xc000_0023 => 0x8009_0028u32 as i32, // NTE_BUFFER_TOO_SMALL
        0xc000_000d => NTE_INVALID_PARAMETER,
        _ => NTE_BAD_DATA,
    }
}

/// A key's "Length" (bits), "Algorithm Group" ("RSA", "ECDSA") and
/// "Algorithm Name" ("RSA", "ECDSA_P256", "ECDSA_P384").
pub fn property(h: usize, name: *const u16, out: *mut u8, size: u32, result: *mut u32) -> i32 {
    let Some(k) = key(h) else { return NTE_INVALID_HANDLE };
    let mut value = [0u8; 32];
    let n = if wide_eq(name, "Length") {
        value[..4].copy_from_slice(&k.bits.to_le_bytes());
        4
    } else if wide_eq(name, "Algorithm Group") || wide_eq(name, "Algorithm Name") {
        let text = if wide_eq(name, "Algorithm Group") { k.kind.group() } else { k.kind.name() };
        for (i, b) in text.bytes().enumerate() {
            value[2 * i] = b;
        }
        2 * text.len() + 2
    } else {
        unsupported(b"ncrypt.dll!NCryptGetProperty(key property)");
        return NTE_NOT_SUPPORTED;
    };
    if !result.is_null() {
        // SAFETY: the caller's output.
        unsafe { result.write(n as u32) };
    }
    if out.is_null() {
        return 0;
    }
    if (size as usize) < n {
        return 0x8009_0028u32 as i32; // NTE_BUFFER_TOO_SMALL
    }
    // SAFETY: the caller's buffer of `size` bytes.
    unsafe { core::ptr::copy_nonoverlapping(value.as_ptr(), out, n) };
    0
}

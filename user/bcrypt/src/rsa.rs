//! RSA keys (BCRYPT_RSA_ALGORITHM): key pairs generated
//! (BCryptGenerateKeyPair + BCryptFinalizeKeyPair) or imported from the
//! documented BCRYPT_RSAKEY_BLOB forms (RSAPUBLICBLOB, RSAPRIVATEBLOB,
//! RSAFULLPRIVATEBLOB: the header, then big-endian PublicExponent,
//! Modulus, Prime1, Prime2 and, full, Exponent1, Exponent2, Coefficient,
//! PrivateExponent) and exported the same way; signatures with
//! BCRYPT_PAD_PKCS1 (the DigestInfo of BCRYPT_PKCS1_PADDING_INFO's
//! algorithm, none without one) and BCRYPT_PAD_PSS (its hash and salt
//! length); encryption with BCRYPT_PAD_PKCS1, BCRYPT_PAD_OAEP (hash and
//! a UTF-8 label) and BCRYPT_PAD_NONE. The arithmetic and paddings are
//! RustCrypto's `rsa` crate (THIRD_PARTY.md), which allocates through the
//! global allocator (the process heap; see lib.rs).

use alloc::boxed::Box;
use alloc::vec::Vec;
use num_bigint_dig::ModInverse;
use rsa::traits::{PrivateKeyParts, PublicKeyParts};
use rsa::{BigUint, Oaep, Pkcs1v15Encrypt, Pkcs1v15Sign, Pss, RsaPrivateKey, RsaPublicKey};

use super::{
    STATUS_BUFFER_TOO_SMALL, STATUS_INVALID_HANDLE, STATUS_INVALID_PARAMETER, STATUS_NO_MEMORY,
    STATUS_NOT_SUPPORTED, STATUS_SUCCESS, Status, bytes, missing, wide_is,
};

const RSA_MAGIC: u32 = 0x6761_6c72; // "galr"
const STATUS_INVALID_SIGNATURE: Status = 0xc000_a000;
const STATUS_INVALID_BUFFER_SIZE: Status = 0xc000_0206;
const STATUS_INTERNAL_ERROR: Status = 0xc000_00e5;
const STATUS_DATA_ERROR: Status = 0xc000_003e;

const BCRYPT_PAD_NONE: u32 = 1;
const BCRYPT_PAD_PKCS1: u32 = 2;
const BCRYPT_PAD_OAEP: u32 = 4;
const BCRYPT_PAD_PSS: u32 = 8;

const MAGIC_PUBLIC: u32 = 0x3141_5352; // "RSA1"
const MAGIC_PRIVATE: u32 = 0x3241_5352; // "RSA2"
const MAGIC_FULL: u32 = 0x3341_5352; // "RSA3"

/// An RSA key object (the handle points at it).
pub struct RsaKey {
    magic: u32,
    /// The key size BCryptGenerateKeyPair asked for, until finalized.
    pending_bits: u32,
    public: Option<RsaPublicKey>,
    private: Option<RsaPrivateKey>,
}

pub fn key<'a>(h: usize) -> Option<&'a mut RsaKey> {
    if h < 0x1000 {
        return None;
    }
    // SAFETY: a handle the caller got from us (tagged; every key object
    // starts with its magic).
    let p = unsafe { &mut *(h as *mut RsaKey) };
    (p.magic == RSA_MAGIC).then_some(p)
}

impl RsaKey {
    pub fn bits(&self) -> u32 {
        match &self.public {
            Some(k) => k.n().bits() as u32,
            None => self.pending_bits,
        }
    }
}

/// The process's random generator (bcryptprimitives' ProcessPrng).
pub struct Prng;

impl rand_core::RngCore for Prng {
    fn next_u32(&mut self) -> u32 {
        let mut b = [0u8; 4];
        self.fill_bytes(&mut b);
        u32::from_le_bytes(b)
    }
    fn next_u64(&mut self) -> u64 {
        let mut b = [0u8; 8];
        self.fill_bytes(&mut b);
        u64::from_le_bytes(b)
    }
    fn fill_bytes(&mut self, dest: &mut [u8]) {
        // SAFETY: our buffer; ProcessPrng always succeeds.
        unsafe { super::ProcessPrng(dest.as_mut_ptr(), dest.len()) };
    }
    fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), rand_core::Error> {
        self.fill_bytes(dest);
        Ok(())
    }
}

impl rand_core::CryptoRng for Prng {}

fn new_key(public: Option<RsaPublicKey>, private: Option<RsaPrivateKey>, bits: u32) -> usize {
    super::new_object(RsaKey { magic: RSA_MAGIC, pending_bits: bits, public, private }) as usize
}

/// Whether provider handle `alg` is RSA.
fn is_rsa(alg: usize) -> bool {
    super::algorithm(alg).is_some_and(|a| a.kind == super::Kind::Rsa)
}

#[unsafe(no_mangle)]
pub extern "system" fn BCryptGenerateKeyPair(
    alg: usize,
    out: *mut usize,
    bits: u32,
    _flags: u32,
) -> Status {
    if !is_rsa(alg) {
        return STATUS_INVALID_HANDLE;
    }
    if out.is_null() || !(512..=16384).contains(&bits) || !bits.is_multiple_of(64) {
        return STATUS_INVALID_PARAMETER;
    }
    let h = new_key(None, None, bits);
    if h == 0 {
        return STATUS_NO_MEMORY;
    }
    // SAFETY: the caller's output.
    unsafe { *out = h };
    STATUS_SUCCESS
}

/// BCryptFinalizeKeyPair: generates the key (public exponent 65537).
#[unsafe(no_mangle)]
pub extern "system" fn BCryptFinalizeKeyPair(h: usize, _flags: u32) -> Status {
    let Some(k) = key(h) else { return STATUS_INVALID_HANDLE };
    if k.public.is_some() {
        return STATUS_INVALID_HANDLE; // already finalized
    }
    let Ok(private) = RsaPrivateKey::new(&mut Prng, k.pending_bits as usize) else {
        return STATUS_INTERNAL_ERROR;
    };
    k.public = Some(private.to_public_key());
    k.private = Some(private);
    STATUS_SUCCESS
}

/// Reads `n` big-endian bytes at `*at` of `blob`.
fn take(blob: &[u8], at: &mut usize, n: usize) -> Option<BigUint> {
    let part = blob.get(*at..*at + n)?;
    *at += n;
    Some(BigUint::from_bytes_be(part))
}

#[unsafe(no_mangle)]
pub extern "system" fn BCryptImportKeyPair(
    alg: usize,
    import: usize,
    blob_type: *const u16,
    out: *mut usize,
    input: *const u8,
    len: u32,
    _flags: u32,
) -> Status {
    if !is_rsa(alg) {
        missing(b"bcrypt.dll!BCryptImportKeyPair(algorithm)", 0);
        return STATUS_NOT_SUPPORTED;
    }
    if import != 0 || out.is_null() || input.is_null() || len < 24 {
        return STATUS_INVALID_PARAMETER;
    }
    let blob = bytes(input, len);
    let word = |i: usize| {
        u32::from_le_bytes([blob[4 * i], blob[4 * i + 1], blob[4 * i + 2], blob[4 * i + 3]])
    };
    let (magic, bits) = (word(0), word(1));
    let (e_len, n_len, p_len, q_len) =
        (word(2) as usize, word(3) as usize, word(4) as usize, word(5) as usize);
    let wanted = if wide_is(blob_type, "RSAPUBLICBLOB") {
        MAGIC_PUBLIC
    } else if wide_is(blob_type, "RSAPRIVATEBLOB") {
        MAGIC_PRIVATE
    } else if wide_is(blob_type, "RSAFULLPRIVATEBLOB") {
        MAGIC_FULL
    } else {
        missing(b"bcrypt.dll!BCryptImportKeyPair(blob type)", 0);
        return STATUS_NOT_SUPPORTED;
    };
    // A private blob may be imported as public (its first part).
    let public_part = wanted == MAGIC_PUBLIC && (magic == MAGIC_PRIVATE || magic == MAGIC_FULL);
    if magic != wanted && !public_part {
        return STATUS_INVALID_PARAMETER;
    }
    let mut at = 24;
    let (Some(e), Some(n)) = (take(blob, &mut at, e_len), take(blob, &mut at, n_len)) else {
        return STATUS_INVALID_PARAMETER;
    };
    if n.bits() as u32 != bits && bits.div_ceil(8) as usize != n_len {
        return STATUS_INVALID_PARAMETER;
    }
    let Ok(public) = RsaPublicKey::new(n.clone(), e.clone()) else {
        return STATUS_INVALID_PARAMETER;
    };
    let private = if wanted == MAGIC_PUBLIC {
        None
    } else {
        let (Some(p), Some(q)) = (take(blob, &mut at, p_len), take(blob, &mut at, q_len)) else {
            return STATUS_INVALID_PARAMETER;
        };
        let d = if wanted == MAGIC_FULL {
            // Exponent1, Exponent2 (prime lengths), Coefficient, then the
            // private exponent (modulus length).
            at += 2 * p_len + q_len;
            match take(blob, &mut at, n_len) {
                Some(d) => d,
                None => return STATUS_INVALID_PARAMETER,
            }
        } else {
            let one = BigUint::from(1u32);
            let phi = (&p - &one) * (&q - &one);
            match e.clone().mod_inverse(&phi).and_then(|d| d.to_biguint()) {
                Some(d) => d,
                None => return STATUS_INVALID_PARAMETER,
            }
        };
        let mut primes = Vec::new();
        primes.push(p);
        primes.push(q);
        match RsaPrivateKey::from_components(n, e, d, primes) {
            Ok(mut k) => {
                if k.precompute().is_err() {
                    return STATUS_INVALID_PARAMETER;
                }
                Some(k)
            }
            Err(_) => return STATUS_INVALID_PARAMETER,
        }
    };
    let h = new_key(Some(public), private, bits);
    if h == 0 {
        return STATUS_NO_MEMORY;
    }
    // SAFETY: the caller's output.
    unsafe { *out = h };
    STATUS_SUCCESS
}

/// `v` as `len` big-endian bytes (left-padded).
fn put(out: &mut Vec<u8>, v: &BigUint, len: usize) {
    let b = v.to_bytes_be();
    out.extend(core::iter::repeat_n(0, len.saturating_sub(b.len())));
    out.extend_from_slice(&b[b.len().saturating_sub(len)..]);
}

/// BCryptExportKey of an RSA key.
pub fn export(
    k: &RsaKey,
    blob_type: *const u16,
    out: *mut u8,
    len: u32,
    result: *mut u32,
) -> Status {
    let Some(public) = &k.public else { return STATUS_INVALID_HANDLE };
    let magic = if wide_is(blob_type, "RSAPUBLICBLOB") {
        MAGIC_PUBLIC
    } else if wide_is(blob_type, "RSAPRIVATEBLOB") {
        MAGIC_PRIVATE
    } else if wide_is(blob_type, "RSAFULLPRIVATEBLOB") {
        MAGIC_FULL
    } else {
        missing(b"bcrypt.dll!BCryptExportKey(blob type)", 0);
        return STATUS_NOT_SUPPORTED;
    };
    if magic != MAGIC_PUBLIC && k.private.is_none() {
        return STATUS_INVALID_PARAMETER;
    }
    let e = public.e().to_bytes_be();
    let n_len = public.size();
    let mut b = Vec::new();
    let (p_len, q_len) = match (&k.private, magic) {
        (Some(private), MAGIC_PRIVATE | MAGIC_FULL) => {
            let primes = private.primes();
            ((primes[0].bits().div_ceil(8)), (primes[1].bits().div_ceil(8)))
        }
        _ => (0, 0),
    };
    for v in [magic, (n_len * 8) as u32, e.len() as u32, n_len as u32, p_len as u32, q_len as u32] {
        b.extend_from_slice(&v.to_le_bytes());
    }
    b.extend_from_slice(&e);
    put(&mut b, public.n(), n_len);
    if let (Some(private), true) = (&k.private, magic != MAGIC_PUBLIC) {
        let primes = private.primes();
        put(&mut b, &primes[0], p_len);
        put(&mut b, &primes[1], q_len);
        if magic == MAGIC_FULL {
            let (Some(dp), Some(dq), Some(qinv)) =
                (private.dp(), private.dq(), private.crt_coefficient())
            else {
                return STATUS_INTERNAL_ERROR;
            };
            put(&mut b, dp, p_len);
            put(&mut b, dq, q_len);
            put(&mut b, &qinv, p_len);
            put(&mut b, private.d(), n_len);
        }
    }
    copy_out(&b, out, len, result)
}

/// Copies `data` to the caller (the size alone without a buffer).
fn copy_out(data: &[u8], out: *mut u8, len: u32, result: *mut u32) -> Status {
    if result.is_null() {
        return STATUS_INVALID_PARAMETER;
    }
    // SAFETY: the caller's outputs.
    unsafe {
        *result = data.len() as u32;
        if out.is_null() {
            return STATUS_SUCCESS;
        }
        if (len as usize) < data.len() {
            return STATUS_BUFFER_TOO_SMALL;
        }
        core::ptr::copy_nonoverlapping(data.as_ptr(), out, data.len());
    }
    STATUS_SUCCESS
}

pub fn destroy(h: usize) -> Status {
    if key(h).is_none() {
        return STATUS_INVALID_HANDLE;
    }
    super::free_object(h as *mut RsaKey);
    STATUS_SUCCESS
}

/// The hashes a padding names.
#[derive(Clone, Copy)]
enum HashId {
    Md2,
    Md4,
    Md5,
    Sha1,
    Sha256,
    Sha384,
    Sha512,
}

fn hash_id(name: *const u16) -> Option<HashId> {
    [
        ("MD2", HashId::Md2),
        ("MD4", HashId::Md4),
        ("MD5", HashId::Md5),
        ("SHA1", HashId::Sha1),
        ("SHA256", HashId::Sha256),
        ("SHA384", HashId::Sha384),
        ("SHA512", HashId::Sha512),
    ]
    .into_iter()
    .find(|(n, _)| wide_is(name, n))
    .map(|(_, id)| id)
}

/// PKCS#1 v1.5 signature padding: the DigestInfo prefix of the hash
/// (RFC 8017, 9.2 note 1), or none (BCRYPT_PKCS1_PADDING_INFO without an
/// algorithm, or no padding information).
fn pkcs1_sign(info: *const u8) -> Option<Pkcs1v15Sign> {
    let alg = if info.is_null() {
        core::ptr::null()
    } else {
        // SAFETY: BCRYPT_PKCS1_PADDING_INFO { LPCWSTR pszAlgId }.
        unsafe { info.cast::<*const u16>().read_unaligned() }
    };
    if alg.is_null() {
        return Some(Pkcs1v15Sign::new_unprefixed());
    }
    let (prefix, len): (&[u8], usize) = match hash_id(alg)? {
        HashId::Md2 => (
            &[
                0x30, 0x20, 0x30, 0x0c, 0x06, 0x08, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x02, 0x02,
                0x05, 0x00, 0x04, 0x10,
            ],
            16,
        ),
        HashId::Md4 => (
            &[
                0x30, 0x20, 0x30, 0x0c, 0x06, 0x08, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x02, 0x04,
                0x05, 0x00, 0x04, 0x10,
            ],
            16,
        ),
        HashId::Md5 => (
            &[
                0x30, 0x20, 0x30, 0x0c, 0x06, 0x08, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x02, 0x05,
                0x05, 0x00, 0x04, 0x10,
            ],
            16,
        ),
        HashId::Sha1 => (
            &[
                0x30, 0x21, 0x30, 0x09, 0x06, 0x05, 0x2b, 0x0e, 0x03, 0x02, 0x1a, 0x05, 0x00, 0x04,
                0x14,
            ],
            20,
        ),
        HashId::Sha256 => (
            &[
                0x30, 0x31, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02,
                0x01, 0x05, 0x00, 0x04, 0x20,
            ],
            32,
        ),
        HashId::Sha384 => (
            &[
                0x30, 0x41, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02,
                0x02, 0x05, 0x00, 0x04, 0x30,
            ],
            48,
        ),
        HashId::Sha512 => (
            &[
                0x30, 0x51, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02,
                0x03, 0x05, 0x00, 0x04, 0x40,
            ],
            64,
        ),
    };
    Some(Pkcs1v15Sign { hash_len: Some(len), prefix: Box::from(prefix) })
}

/// PSS padding: BCRYPT_PSS_PADDING_INFO { pszAlgId, cbSalt }.
fn pss(info: *const u8) -> Option<Pss> {
    if info.is_null() {
        return None;
    }
    // SAFETY: the caller's padding information.
    let (alg, salt) = unsafe {
        (info.cast::<*const u16>().read_unaligned(), info.add(8).cast::<u32>().read_unaligned())
    };
    let salt = salt as usize;
    Some(match hash_id(alg)? {
        HashId::Md5 => Pss::new_with_salt::<md5::Md5>(salt),
        HashId::Sha1 => Pss::new_with_salt::<sha1::Sha1>(salt),
        HashId::Sha256 => Pss::new_with_salt::<sha2::Sha256>(salt),
        HashId::Sha384 => Pss::new_with_salt::<sha2::Sha384>(salt),
        HashId::Sha512 => Pss::new_with_salt::<sha2::Sha512>(salt),
        HashId::Md2 | HashId::Md4 => return None,
    })
}

/// OAEP padding: BCRYPT_OAEP_PADDING_INFO { pszAlgId, pbLabel, cbLabel }
/// (a label must be UTF-8 here).
fn oaep(info: *const u8) -> Option<Oaep> {
    if info.is_null() {
        return None;
    }
    // SAFETY: the caller's padding information.
    let (alg, label, label_len) = unsafe {
        (
            info.cast::<*const u16>().read_unaligned(),
            info.add(8).cast::<*const u8>().read_unaligned(),
            info.add(16).cast::<u32>().read_unaligned(),
        )
    };
    let label = if label.is_null() || label_len == 0 {
        None
    } else {
        Some(core::str::from_utf8(bytes(label, label_len)).ok()?)
    };
    macro_rules! with {
        ($d:ty) => {
            match label {
                Some(l) => Oaep::new_with_label::<$d, _>(l),
                None => Oaep::new::<$d>(),
            }
        };
    }
    Some(match hash_id(alg)? {
        HashId::Md5 => with!(md5::Md5),
        HashId::Sha1 => with!(sha1::Sha1),
        HashId::Sha256 => with!(sha2::Sha256),
        HashId::Sha384 => with!(sha2::Sha384),
        HashId::Sha512 => with!(sha2::Sha512),
        HashId::Md2 | HashId::Md4 => return None,
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn BCryptSignHash(
    h: usize,
    info: *const u8,
    hash: *const u8,
    hash_len: u32,
    out: *mut u8,
    out_len: u32,
    result: *mut u32,
    flags: u32,
) -> Status {
    let Some(k) = key(h) else { return STATUS_INVALID_HANDLE };
    let Some(public) = &k.public else { return STATUS_INVALID_HANDLE };
    let size = public.size();
    if result.is_null() {
        return STATUS_INVALID_PARAMETER;
    }
    if out.is_null() {
        // SAFETY: the caller's output.
        unsafe { *result = size as u32 };
        return STATUS_SUCCESS;
    }
    let Some(private) = &k.private else { return STATUS_INVALID_PARAMETER };
    let digest = bytes(hash, hash_len);
    let signature = match flags {
        BCRYPT_PAD_PKCS1 => match pkcs1_sign(info) {
            Some(p) => private.sign(p, digest),
            None => return STATUS_NOT_SUPPORTED,
        },
        BCRYPT_PAD_PSS => match pss(info) {
            Some(p) => private.sign_with_rng(&mut Prng, p, digest),
            None => return STATUS_NOT_SUPPORTED,
        },
        _ => {
            missing(b"bcrypt.dll!BCryptSignHash(padding)", u64::from(flags));
            return STATUS_INVALID_PARAMETER;
        }
    };
    match signature {
        Ok(s) => copy_out(&s, out, out_len, result),
        Err(_) => STATUS_INVALID_PARAMETER,
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn BCryptVerifySignature(
    h: usize,
    info: *const u8,
    hash: *const u8,
    hash_len: u32,
    sig: *const u8,
    sig_len: u32,
    flags: u32,
) -> Status {
    let Some(k) = key(h) else { return STATUS_INVALID_HANDLE };
    let Some(public) = &k.public else { return STATUS_INVALID_HANDLE };
    let digest = bytes(hash, hash_len);
    let signature = bytes(sig, sig_len);
    let ok = match flags {
        BCRYPT_PAD_PKCS1 => match pkcs1_sign(info) {
            Some(p) => public.verify(p, digest, signature).is_ok(),
            None => return STATUS_NOT_SUPPORTED,
        },
        BCRYPT_PAD_PSS => match pss(info) {
            Some(p) => public.verify(p, digest, signature).is_ok(),
            None => return STATUS_NOT_SUPPORTED,
        },
        _ => {
            missing(b"bcrypt.dll!BCryptVerifySignature(padding)", u64::from(flags));
            return STATUS_INVALID_PARAMETER;
        }
    };
    if ok { STATUS_SUCCESS } else { STATUS_INVALID_SIGNATURE }
}

/// BCryptEncrypt/BCryptDecrypt of an RSA key (no IV).
#[allow(clippy::too_many_arguments)]
pub fn crypt(
    h: usize,
    encrypt: bool,
    input: *const u8,
    len: u32,
    info: *const u8,
    out: *mut u8,
    out_len: u32,
    result: *mut u32,
    flags: u32,
) -> Status {
    let Some(k) = key(h) else { return STATUS_INVALID_HANDLE };
    let Some(public) = &k.public else { return STATUS_INVALID_HANDLE };
    let size = public.size();
    if result.is_null() {
        return STATUS_INVALID_PARAMETER;
    }
    let data = bytes(input, len);
    if encrypt && out.is_null() {
        // SAFETY: the caller's output.
        unsafe { *result = size as u32 };
        return STATUS_SUCCESS;
    }
    let output = if encrypt {
        match flags {
            BCRYPT_PAD_PKCS1 => public.encrypt(&mut Prng, Pkcs1v15Encrypt, data),
            BCRYPT_PAD_OAEP => match oaep(info) {
                Some(p) => public.encrypt(&mut Prng, p, data),
                None => return STATUS_NOT_SUPPORTED,
            },
            BCRYPT_PAD_NONE => {
                if data.len() > size {
                    return STATUS_INVALID_PARAMETER;
                }
                rsa::hazmat::rsa_encrypt(public, &BigUint::from_bytes_be(data)).map(|c| {
                    let mut v = Vec::new();
                    put(&mut v, &c, size);
                    v
                })
            }
            _ => return STATUS_INVALID_PARAMETER,
        }
    } else {
        let Some(private) = &k.private else { return STATUS_INVALID_PARAMETER };
        if data.len() != size {
            return STATUS_INVALID_BUFFER_SIZE;
        }
        match flags {
            BCRYPT_PAD_PKCS1 => private.decrypt(Pkcs1v15Encrypt, data),
            BCRYPT_PAD_OAEP => match oaep(info) {
                Some(p) => private.decrypt(p, data),
                None => return STATUS_NOT_SUPPORTED,
            },
            BCRYPT_PAD_NONE => rsa::hazmat::rsa_decrypt_and_check(
                private,
                Some(&mut Prng),
                &BigUint::from_bytes_be(data),
            )
            .map(|m| {
                let mut v = Vec::new();
                put(&mut v, &m, size);
                v
            }),
            _ => return STATUS_INVALID_PARAMETER,
        }
    };
    match output {
        Ok(v) => copy_out(&v, out, out_len, result),
        Err(_) if encrypt => STATUS_INVALID_PARAMETER,
        Err(_) => STATUS_DATA_ERROR,
    }
}

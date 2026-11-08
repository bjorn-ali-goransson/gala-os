//! ECDSA keys on the NIST curves P-256 and P-384
//! (BCRYPT_ECDSA_P256_ALGORITHM, BCRYPT_ECDSA_P384_ALGORITHM, and their
//! pseudo-handles), as crypt32 needs them to check certificate
//! signatures and ncrypt to sign with a server's or client's key:
//! BCryptImportKeyPair of a BCRYPT_ECCPUBLIC_BLOB (BCRYPT_ECCKEY_BLOB:
//! dwMagic ECS1/ECS3, cbKey, then X and Y, each cbKey bytes big-endian)
//! or a BCRYPT_ECCPRIVATE_BLOB (ECS2/ECS4: X, Y, then the private scalar
//! d; also importable as ECCPUBLICBLOB, its first part);
//! BCryptVerifySignature of a hash (no padding info; the signature is r
//! then s, each cbKey bytes), which gives STATUS_INVALID_SIGNATURE when it
//! does not verify; BCryptSignHash of a hash with a private key (r || s;
//! the nonce is RFC 6979's, derived from the key and the hash, where
//! Windows draws a random one: either is a valid signature);
//! BCryptExportKey of both blobs (Inno Setup's IDE and compiler export
//! the trusted keys they import). Generation and P-521 are not there
//! (recorded). The curve arithmetic is
//! RustCrypto's `p256`/`p384` crates (THIRD_PARTY.md).

use p256::ecdsa::signature::hazmat::{PrehashSigner, PrehashVerifier};

use super::{
    Kind, STATUS_BUFFER_TOO_SMALL, STATUS_INVALID_HANDLE, STATUS_INVALID_PARAMETER,
    STATUS_NO_MEMORY, STATUS_NOT_SUPPORTED, STATUS_SUCCESS, Status, bytes, missing, wide_is,
};

const EC_MAGIC: u32 = 0x6761_6c65; // "gale"
const STATUS_INVALID_SIGNATURE: Status = 0xc000_a000;

const MAGIC_PUBLIC_P256: u32 = 0x3153_4345; // "ECS1"
const MAGIC_PUBLIC_P384: u32 = 0x3353_4345; // "ECS3"
const MAGIC_PRIVATE_P256: u32 = 0x3253_4345; // "ECS2"
const MAGIC_PRIVATE_P384: u32 = 0x3453_4345; // "ECS4"

enum Verifier {
    P256(p256::ecdsa::VerifyingKey),
    P384(p384::ecdsa::VerifyingKey),
}

enum Signer {
    P256(p256::ecdsa::SigningKey),
    P384(p384::ecdsa::SigningKey),
}

/// An ECDSA key object (the handle points at it): the public key, and
/// the private one for a private blob.
/// `repr(C)`: the magic first, where `key` looks for it in any handle.
#[repr(C)]
pub struct EcKey {
    magic: u32,
    key: Verifier,
    private: Option<Signer>,
}

impl EcKey {
    pub fn bits(&self) -> u32 {
        match self.key {
            Verifier::P256(_) => 256,
            Verifier::P384(_) => 384,
        }
    }
}

pub fn key<'a>(h: usize) -> Option<&'a mut EcKey> {
    if h < 0x1000 {
        return None;
    }
    // SAFETY: a handle the caller got from us (tagged; every key object
    // starts with its magic).
    let p = unsafe { &mut *(h as *mut EcKey) };
    (p.magic == EC_MAGIC).then_some(p)
}

/// BCryptImportKeyPair on an ECDSA provider: a public or private key
/// blob.
pub fn import(
    kind: Kind,
    blob_type: *const u16,
    out: *mut usize,
    input: *const u8,
    len: u32,
) -> Status {
    if out.is_null() || input.is_null() || len < 8 {
        return STATUS_INVALID_PARAMETER;
    }
    let public_type = wide_is(blob_type, "ECCPUBLICBLOB");
    if !public_type && !wide_is(blob_type, "ECCPRIVATEBLOB") {
        missing(b"bcrypt.dll!BCryptImportKeyPair(ECDSA blob type)", 0);
        return STATUS_NOT_SUPPORTED;
    }
    let blob = bytes(input, len);
    let word = |i: usize| u32::from_le_bytes([blob[i], blob[i + 1], blob[i + 2], blob[i + 3]]);
    let (magic, size) = (word(0), word(4) as usize);
    let (public_magic, private_magic, want_size) = match kind {
        Kind::EcdsaP256 => (MAGIC_PUBLIC_P256, MAGIC_PRIVATE_P256, 32),
        Kind::EcdsaP384 => (MAGIC_PUBLIC_P384, MAGIC_PRIVATE_P384, 48),
        _ => return STATUS_INVALID_HANDLE,
    };
    // A private blob may be imported as public (its X and Y).
    let private = magic == private_magic;
    let parts = if private { 3 } else { 2 };
    let magic_ok = if public_type { magic == public_magic || private } else { private };
    if !magic_ok || size != want_size || blob.len() != 8 + parts * size {
        return STATUS_INVALID_PARAMETER;
    }
    // SEC 1's uncompressed point: 04 || X || Y.
    let mut point = [0u8; 1 + 2 * 48];
    point[0] = 4;
    point[1..1 + 2 * size].copy_from_slice(&blob[8..8 + 2 * size]);
    let point = &point[..1 + 2 * size];
    let key = match kind {
        Kind::EcdsaP256 => p256::ecdsa::VerifyingKey::from_sec1_bytes(point).map(Verifier::P256),
        _ => p384::ecdsa::VerifyingKey::from_sec1_bytes(point).map(Verifier::P384),
    };
    let Ok(key) = key else { return STATUS_INVALID_PARAMETER };
    let signer = if private && !public_type {
        let d = &blob[8 + 2 * size..];
        let signer = match kind {
            Kind::EcdsaP256 => p256::ecdsa::SigningKey::from_slice(d).map(Signer::P256),
            _ => p384::ecdsa::SigningKey::from_slice(d).map(Signer::P384),
        };
        let Ok(signer) = signer else { return STATUS_INVALID_PARAMETER };
        // d must be the private key of X and Y.
        let matches = match (&signer, &key) {
            (Signer::P256(s), Verifier::P256(v)) => s.verifying_key() == v,
            (Signer::P384(s), Verifier::P384(v)) => s.verifying_key() == v,
            _ => false,
        };
        if !matches {
            return STATUS_INVALID_PARAMETER;
        }
        Some(signer)
    } else {
        None
    };
    let p = super::new_object(EcKey { magic: EC_MAGIC, key, private: signer });
    if p.is_null() {
        return STATUS_NO_MEMORY;
    }
    // SAFETY: the caller's output.
    unsafe { *out = p as usize };
    STATUS_SUCCESS
}

/// BCryptExportKey of an ECDSA key: BCRYPT_ECCPUBLIC_BLOB (magic, cbKey,
/// X, Y) or, for a key with its private part, BCRYPT_ECCPRIVATE_BLOB (d
/// after them); without a buffer the size alone, a short one
/// STATUS_BUFFER_TOO_SMALL. A public key has no private blob
/// (STATUS_INVALID_PARAMETER).
pub fn export(
    k: &EcKey,
    blob_type: *const u16,
    out: *mut u8,
    len: u32,
    result: *mut u32,
) -> Status {
    let private = if wide_is(blob_type, "ECCPUBLICBLOB") {
        false
    } else if wide_is(blob_type, "ECCPRIVATEBLOB") {
        true
    } else {
        missing(b"bcrypt.dll!BCryptExportKey(ECDSA blob type)", 0);
        return STATUS_NOT_SUPPORTED;
    };
    if private && k.private.is_none() {
        return STATUS_INVALID_PARAMETER;
    }
    let mut b = [0u8; 8 + 3 * 48];
    let (size, magic) = match (&k.key, private) {
        (Verifier::P256(_), false) => (32, MAGIC_PUBLIC_P256),
        (Verifier::P256(_), true) => (32, MAGIC_PRIVATE_P256),
        (Verifier::P384(_), false) => (48, MAGIC_PUBLIC_P384),
        (Verifier::P384(_), true) => (48, MAGIC_PRIVATE_P384),
    };
    b[..4].copy_from_slice(&magic.to_le_bytes());
    b[4..8].copy_from_slice(&(size as u32).to_le_bytes());
    // SEC 1's uncompressed point: 04 || X || Y.
    match &k.key {
        Verifier::P256(v) => {
            b[8..8 + 2 * size].copy_from_slice(&v.to_encoded_point(false).as_bytes()[1..])
        }
        Verifier::P384(v) => {
            b[8..8 + 2 * size].copy_from_slice(&v.to_encoded_point(false).as_bytes()[1..])
        }
    }
    match &k.private {
        Some(Signer::P256(s)) if private => {
            b[8 + 2 * size..8 + 3 * size].copy_from_slice(&s.to_bytes())
        }
        Some(Signer::P384(s)) if private => {
            b[8 + 2 * size..8 + 3 * size].copy_from_slice(&s.to_bytes())
        }
        _ => {}
    }
    let n = 8 + if private { 3 } else { 2 } * size;
    if result.is_null() {
        return STATUS_INVALID_PARAMETER;
    }
    // SAFETY: the caller's outputs.
    unsafe {
        *result = n as u32;
        if out.is_null() {
            return STATUS_SUCCESS;
        }
        if (len as usize) < n {
            return STATUS_BUFFER_TOO_SMALL;
        }
        core::ptr::copy_nonoverlapping(b.as_ptr(), out, n);
    }
    STATUS_SUCCESS
}

/// BCryptVerifySignature of an ECDSA key: `hash` signed as r || s.
pub fn verify(k: &EcKey, hash: &[u8], signature: &[u8]) -> Status {
    let ok = match &k.key {
        Verifier::P256(v) => p256::ecdsa::Signature::from_slice(signature)
            .is_ok_and(|s| v.verify_prehash(hash, &s).is_ok()),
        Verifier::P384(v) => p384::ecdsa::Signature::from_slice(signature)
            .is_ok_and(|s| v.verify_prehash(hash, &s).is_ok()),
    };
    if ok { STATUS_SUCCESS } else { STATUS_INVALID_SIGNATURE }
}

/// BCryptSignHash of an ECDSA private key: `hash` signed as r || s
/// (each cbKey bytes; with no output buffer, the size).
pub fn sign(k: &EcKey, hash: &[u8], out: *mut u8, out_len: u32, result: *mut u32) -> Status {
    let size = if k.bits() == 256 { 64 } else { 96 };
    if result.is_null() {
        return STATUS_INVALID_PARAMETER;
    }
    if out.is_null() {
        // SAFETY: the caller's output.
        unsafe { *result = size };
        return STATUS_SUCCESS;
    }
    let Some(signer) = &k.private else {
        // A public key cannot sign.
        return STATUS_INVALID_PARAMETER;
    };
    let mut signature = [0u8; 96];
    let done = match signer {
        Signer::P256(s) => PrehashSigner::<p256::ecdsa::Signature>::sign_prehash(s, hash)
            .map(|sig| signature[..64].copy_from_slice(&sig.to_bytes())),
        Signer::P384(s) => PrehashSigner::<p384::ecdsa::Signature>::sign_prehash(s, hash)
            .map(|sig| signature[..96].copy_from_slice(&sig.to_bytes())),
    };
    if done.is_err() {
        // A hash shorter than half the curve's size.
        return STATUS_INVALID_PARAMETER;
    }
    if out_len < size {
        return STATUS_BUFFER_TOO_SMALL;
    }
    // SAFETY: the caller's outputs (`out_len` bytes at `out`).
    unsafe {
        core::ptr::copy_nonoverlapping(signature.as_ptr(), out, size as usize);
        *result = size;
    }
    STATUS_SUCCESS
}

pub fn destroy(h: usize) -> Status {
    let Some(k) = key(h) else { return STATUS_INVALID_HANDLE };
    k.magic = 0;
    super::free_object(h as *mut EcKey);
    STATUS_SUCCESS
}

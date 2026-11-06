//! ECDSA public keys on the NIST curves P-256 and P-384
//! (BCRYPT_ECDSA_P256_ALGORITHM, BCRYPT_ECDSA_P384_ALGORITHM, and their
//! pseudo-handles), as crypt32 needs them to check certificate
//! signatures: BCryptImportKeyPair of a BCRYPT_ECCPUBLIC_BLOB
//! (BCRYPT_ECCKEY_BLOB: dwMagic ECS1/ECS3, cbKey, then X and Y, each
//! cbKey bytes big-endian) and BCryptVerifySignature of a hash (no
//! padding info; the signature is r then s, each cbKey bytes), which
//! gives STATUS_INVALID_SIGNATURE when it does not verify. Private keys,
//! signing, generation, export and P-521 are not there (recorded). The
//! curve arithmetic is RustCrypto's `p256`/`p384` crates (THIRD_PARTY.md).

use p256::ecdsa::signature::hazmat::PrehashVerifier;

use super::{
    Kind, STATUS_INVALID_HANDLE, STATUS_INVALID_PARAMETER, STATUS_NO_MEMORY, STATUS_NOT_SUPPORTED,
    STATUS_SUCCESS, Status, bytes, missing, wide_is,
};

const EC_MAGIC: u32 = 0x6761_6c65; // "gale"
const STATUS_INVALID_SIGNATURE: Status = 0xc000_a000;

const MAGIC_PUBLIC_P256: u32 = 0x3153_4345; // "ECS1"
const MAGIC_PUBLIC_P384: u32 = 0x3353_4345; // "ECS3"

enum Verifier {
    P256(p256::ecdsa::VerifyingKey),
    P384(p384::ecdsa::VerifyingKey),
}

/// An ECDSA public key object (the handle points at it).
pub struct EcKey {
    magic: u32,
    key: Verifier,
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

/// BCryptImportKeyPair on an ECDSA provider: a public key blob.
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
    if !wide_is(blob_type, "ECCPUBLICBLOB") {
        missing(b"bcrypt.dll!BCryptImportKeyPair(ECDSA blob type)", 0);
        return STATUS_NOT_SUPPORTED;
    }
    let blob = bytes(input, len);
    let word = |i: usize| u32::from_le_bytes([blob[i], blob[i + 1], blob[i + 2], blob[i + 3]]);
    let (magic, size) = (word(0), word(4) as usize);
    let (want_magic, want_size) = match kind {
        Kind::EcdsaP256 => (MAGIC_PUBLIC_P256, 32),
        Kind::EcdsaP384 => (MAGIC_PUBLIC_P384, 48),
        _ => return STATUS_INVALID_HANDLE,
    };
    if magic != want_magic || size != want_size || blob.len() != 8 + 2 * size {
        return STATUS_INVALID_PARAMETER;
    }
    // SEC 1's uncompressed point: 04 || X || Y.
    let mut point = [0u8; 1 + 2 * 48];
    point[0] = 4;
    point[1..1 + 2 * size].copy_from_slice(&blob[8..]);
    let point = &point[..1 + 2 * size];
    let key = match kind {
        Kind::EcdsaP256 => p256::ecdsa::VerifyingKey::from_sec1_bytes(point).map(Verifier::P256),
        _ => p384::ecdsa::VerifyingKey::from_sec1_bytes(point).map(Verifier::P384),
    };
    let Ok(key) = key else { return STATUS_INVALID_PARAMETER };
    let p = super::new_object(EcKey { magic: EC_MAGIC, key });
    if p.is_null() {
        return STATUS_NO_MEMORY;
    }
    // SAFETY: the caller's output.
    unsafe { *out = p as usize };
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

pub fn destroy(h: usize) -> Status {
    let Some(k) = key(h) else { return STATUS_INVALID_HANDLE };
    k.magic = 0;
    super::free_object(h as *mut EcKey);
    STATUS_SUCCESS
}

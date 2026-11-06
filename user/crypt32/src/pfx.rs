//! PFXImportCertStore: a PKCS #12 PFX (RFC 7292) into a memory store, as
//! Microsoft documents it, with the details Wine's conformance tests
//! record on Windows (`dlls/crypt32/tests/store.c`).
//!
//! The PFX's authSafe must be `data` (password integrity; public-key
//! integrity, `signedData`, is recorded and fails). Its MAC (HMAC with
//! SHA-1 or SHA-2, the key from the PKCS #12 KDF) is checked first: a
//! wrong password is ERROR_INVALID_PASSWORD. The AuthenticatedSafe's
//! ContentInfos are `data` or `encryptedData`; bags are encrypted with
//! the PKCS #12 PBE schemes (pbeWithSHAAnd3-KeyTripleDES-CBC,
//! 2-KeyTripleDES, 128- and 40-bit RC2-CBC) or PBES2 (PBKDF2 with
//! HMAC-SHA1/SHA-256/384/512, AES-CBC or DES-EDE3-CBC). The ciphers,
//! hashes and HMACs are bcrypt's (loaded by name, as the certificate
//! hashes are). RC4 schemes are recorded and fail.
//!
//! Passwords: the PKCS #12 KDF takes the password as a big-endian
//! BMPString with its terminating NUL; PBES2 takes its UTF-8 bytes. An
//! empty or NULL password is tried both as the empty string with its NUL
//! and as no bytes at all (implementations differ; Windows reads both).
//!
//! Certificates (certBag, x509Certificate) are added to the store with
//! their friendly name (CERT_FRIENDLY_NAME_PROP_ID); each private key
//! (keyBag, pkcs8ShroudedKeyBag) goes to the certificate with the same
//! localKeyId (or, without ids, to the only certificate) as a CNG key of
//! the Microsoft Software Key Storage Provider (ncrypt): persisted under a
//! new container name (a GUID), named by CERT_KEY_PROV_INFO_PROP_ID
//! (dwProvType 0, dwKeySpec AT_KEYEXCHANGE), or with
//! PKCS12_NO_PERSIST_KEY held by the certificate
//! (CERT_KEY_CONTEXT_PROP_ID, freed with it). Windows puts keys in a
//! CryptoAPI provider unless PKCS12_PREFER_CNG_KSP or
//! PKCS12_ALWAYS_CNG_KSP is given; gala has no CSP keys, so it uses the
//! KSP either way and records that (`PFXImportCertStore(CSP key in the
//! KSP)`).

use core::ffi::c_void;

use crate::cert::{
    CertContext, CertFreeCertificateContext, CertSetCertificateContextProperty, Der, INTEGER,
    OCTET_STRING, OID, SEQUENCE, SET, Tlv, bcrypt, give_key, ncrypt,
};
use crate::misc::DataBlob;
use crate::store::{
    CertAddEncodedCertificateToStore, CertCloseStore, CertOpenStore, error, unsupported,
};

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetProcessHeap() -> usize;
    fn HeapAlloc(heap: usize, flags: u32, size: usize) -> *mut u8;
    fn HeapFree(heap: usize, flags: u32, p: *mut u8) -> i32;
    fn GetLastError() -> u32;
}

const ERROR_INVALID_PARAMETER: u32 = 87;
const ERROR_INVALID_PASSWORD: u32 = 86;
const ERROR_NOT_ENOUGH_MEMORY: u32 = 8;
const CRYPT_E_BAD_ENCODE: u32 = 0x8009_2002;
const NTE_NOT_SUPPORTED: u32 = 0x8009_0029;

const CRYPT_MACHINE_KEYSET: u32 = 0x20;
const PKCS12_PREFER_CNG_KSP: u32 = 0x100;
const PKCS12_ALWAYS_CNG_KSP: u32 = 0x200;
const PKCS12_NO_PERSIST_KEY: u32 = 0x8000;
const CERT_KEY_PROV_INFO_PROP_ID: u32 = 2;
const CERT_FRIENDLY_NAME_PROP_ID: u32 = 11;
const AT_KEYEXCHANGE: u32 = 1;
const CERT_STORE_PROV_MEMORY: usize = 2;
const CERT_STORE_ADD_ALWAYS: u32 = 4;
const X509_ASN_ENCODING: u32 = 1;

/// OID contents (DER, without tag and length).
const OID_DATA: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x07, 0x01];
const OID_ENCRYPTED_DATA: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x07, 0x06];
/// 1.2.840.113549.1.12.10.1.n: the bag types.
const OID_BAG: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x0c, 0x0a, 0x01];
const BAG_KEY: u8 = 1;
const BAG_SHROUDED_KEY: u8 = 2;
const BAG_CERT: u8 = 3;
const BAG_SAFE_CONTENTS: u8 = 6;
/// 1.2.840.113549.1.9.22.1: x509Certificate in a certBag.
const OID_X509_CERT: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x09, 0x16, 0x01];
/// 1.2.840.113549.1.9.20 / .21: friendlyName, localKeyId.
const OID_FRIENDLY_NAME: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x09, 0x14];
const OID_LOCAL_KEY_ID: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x09, 0x15];
/// 1.2.840.113549.1.12.1.n: the PKCS #12 PBE schemes.
const OID_PKCS12_PBE: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x0c, 0x01];
/// 1.2.840.113549.1.5.13 / .12: PBES2, PBKDF2.
const OID_PBES2: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x05, 0x0d];
const OID_PBKDF2: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x05, 0x0c];
/// 1.2.840.113549.2.n: hmacWithSHA1 (7), SHA256 (9), SHA384 (10), SHA512 (11).
const OID_HMAC: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x02];
/// 2.16.840.1.101.3.4.1.n: aes128-CBC (2), aes192-CBC (22), aes256-CBC (42).
const OID_AES: &[u8] = &[0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x01];
/// 1.2.840.113549.3.7: des-ede3-cbc.
const OID_DES_EDE3_CBC: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x03, 0x07];
/// 1.3.14.3.2.26 SHA-1; 2.16.840.1.101.3.4.2.n SHA-256 (1), 384 (2), 512 (3).
const OID_SHA1: &[u8] = &[0x2b, 0x0e, 0x03, 0x02, 0x1a];
const OID_SHA2: &[u8] = &[0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02];

/// A hash bcrypt has: its pseudo-handles (plain, HMAC), output and block
/// lengths.
#[derive(Clone, Copy)]
struct Hash {
    plain: usize,
    hmac: usize,
    len: usize,
    block: usize,
}

const SHA1: Hash = Hash { plain: 0x31, hmac: 0xa1, len: 20, block: 64 };
const SHA256: Hash = Hash { plain: 0x41, hmac: 0xb1, len: 32, block: 64 };
const SHA384: Hash = Hash { plain: 0x51, hmac: 0xc1, len: 48, block: 128 };
const SHA512: Hash = Hash { plain: 0x61, hmac: 0xd1, len: 64, block: 128 };

/// A heap buffer (freed when dropped).
struct Buf {
    p: *mut u8,
    len: usize,
}

impl Buf {
    fn new(len: usize) -> Result<Buf, u32> {
        // SAFETY: a zeroed heap block (at least one byte).
        let p = unsafe { HeapAlloc(GetProcessHeap(), 8, len.max(1)) };
        if p.is_null() { Err(ERROR_NOT_ENOUGH_MEMORY) } else { Ok(Buf { p, len }) }
    }

    fn bytes(&self) -> &[u8] {
        // SAFETY: our block of `len` bytes.
        unsafe { core::slice::from_raw_parts(self.p, self.len) }
    }

    fn bytes_mut(&mut self) -> &mut [u8] {
        // SAFETY: as above.
        unsafe { core::slice::from_raw_parts_mut(self.p, self.len) }
    }
}

impl Drop for Buf {
    fn drop(&mut self) {
        // SAFETY: our block; secrets are wiped first.
        unsafe {
            core::ptr::write_bytes(self.p, 0, self.len);
            HeapFree(GetProcessHeap(), 0, self.p);
        }
    }
}

/// BCryptHash: `hash` (or its HMAC under `key`) of the concatenation of
/// `parts` into `out` (the hash's length).
fn digest(hash: Hash, key: Option<&[u8]>, parts: &[&[u8]], out: &mut [u8]) -> Result<(), u32> {
    type HashFn =
        unsafe extern "system" fn(usize, *const u8, u32, *const u8, u32, *mut u8, u32) -> u32;
    let f = bcrypt(c"BCryptHash").ok_or(NTE_NOT_SUPPORTED)?;
    let total: usize = parts.iter().map(|p| p.len()).sum();
    let mut joined = Buf::new(total)?;
    let mut at = 0;
    for p in parts {
        joined.bytes_mut()[at..at + p.len()].copy_from_slice(p);
        at += p.len();
    }
    let (alg, secret) = match key {
        Some(k) => (hash.hmac, k),
        None => (hash.plain, &[][..]),
    };
    // SAFETY: BCryptHash's documented signature; our buffers.
    let status = unsafe {
        core::mem::transmute::<usize, HashFn>(f)(
            alg,
            secret.as_ptr(),
            secret.len() as u32,
            joined.p,
            total as u32,
            out.as_mut_ptr(),
            hash.len as u32,
        )
    };
    if status == 0 { Ok(()) } else { Err(NTE_NOT_SUPPORTED) }
}

/// The PKCS #12 key derivation (RFC 7292 appendix B.2): `out.len()`
/// bytes for purpose `id` (1 key, 2 IV, 3 MAC key) from the BMPString
/// password, the salt and the iteration count.
fn pkcs12_kdf(
    hash: Hash,
    id: u8,
    password: &[u8],
    salt: &[u8],
    iterations: u32,
    out: &mut [u8],
) -> Result<(), u32> {
    let v = hash.block;
    let fill = |src: &[u8], buf: &mut Buf| {
        if !src.is_empty() {
            for (i, b) in buf.bytes_mut().iter_mut().enumerate() {
                *b = src[i % src.len()];
            }
        }
    };
    let s_len = salt.len().div_ceil(v) * v;
    let p_len = password.len().div_ceil(v) * v;
    let mut i_buf = Buf::new(s_len + p_len)?;
    {
        let mut s = Buf::new(s_len)?;
        fill(salt, &mut s);
        let mut p = Buf::new(p_len)?;
        fill(password, &mut p);
        i_buf.bytes_mut()[..s_len].copy_from_slice(s.bytes());
        i_buf.bytes_mut()[s_len..].copy_from_slice(p.bytes());
    }
    let d = [id; 128];
    let mut a = [0u8; 64];
    let mut done = 0;
    while done < out.len() {
        digest(hash, None, &[&d[..v], i_buf.bytes()], &mut a[..hash.len])?;
        for _ in 1..iterations.max(1) {
            let prev = a;
            digest(hash, None, &[&prev[..hash.len]], &mut a[..hash.len])?;
        }
        let n = hash.len.min(out.len() - done);
        out[done..done + n].copy_from_slice(&a[..n]);
        done += n;
        if done >= out.len() {
            break;
        }
        // I_j = (I_j + B + 1) mod 2^(8v), B being A repeated to v bytes.
        let b: [u8; 128] = core::array::from_fn(|k| a[k % hash.len]);
        for block in i_buf.bytes_mut().chunks_exact_mut(v) {
            let mut carry = 1u16;
            for k in (0..v).rev() {
                let sum = u16::from(block[k]) + u16::from(b[k]) + carry;
                block[k] = sum as u8;
                carry = sum >> 8;
            }
        }
    }
    Ok(())
}

/// PBKDF2 (RFC 8018 5.2) with HMAC-`hash`.
fn pbkdf2(
    hash: Hash,
    password: &[u8],
    salt: &[u8],
    iterations: u32,
    out: &mut [u8],
) -> Result<(), u32> {
    let mut block = 1u32;
    let mut done = 0;
    while done < out.len() {
        let mut u = [0u8; 64];
        digest(hash, Some(password), &[salt, &block.to_be_bytes()], &mut u[..hash.len])?;
        let mut t = u;
        for _ in 1..iterations.max(1) {
            let prev = u;
            digest(hash, Some(password), &[&prev[..hash.len]], &mut u[..hash.len])?;
            for (x, y) in t.iter_mut().zip(&u) {
                *x ^= y;
            }
        }
        let n = hash.len.min(out.len() - done);
        out[done..done + n].copy_from_slice(&t[..n]);
        done += n;
        block += 1;
    }
    Ok(())
}

/// The password in the forms the schemes take.
struct Password {
    /// BMPString, big-endian, with its NUL (or without anything, for the
    /// empty password's other form).
    bmp: Buf,
    /// UTF-8, no NUL.
    utf8: Buf,
}

impl Password {
    /// `chars` (UTF-16, without NUL); `bare`: the empty password as no
    /// bytes at all.
    fn new(chars: &[u16], bare: bool) -> Result<Password, u32> {
        let mut bmp = Buf::new(if bare { 0 } else { 2 * chars.len() + 2 })?;
        if !bare {
            for (i, c) in chars.iter().enumerate() {
                bmp.bytes_mut()[2 * i..2 * i + 2].copy_from_slice(&c.to_be_bytes());
            }
        }
        let mut utf8 = Buf::new(3 * chars.len())?;
        let mut n = 0;
        for c in char::decode_utf16(chars.iter().copied()) {
            let c = c.unwrap_or(char::REPLACEMENT_CHARACTER);
            n += c.encode_utf8(&mut utf8.bytes_mut()[n..]).len();
        }
        utf8.len = n;
        Ok(Password { bmp, utf8 })
    }
}

fn int(t: Tlv<'_>) -> Result<u32, u32> {
    if t.tag != INTEGER || t.content.is_empty() || t.content.len() > 5 || t.content[0] & 0x80 != 0 {
        return Err(CRYPT_E_BAD_ENCODE);
    }
    let v = t.content.iter().fold(0u64, |a, &b| (a << 8) | u64::from(b));
    u32::try_from(v).map_err(|_| CRYPT_E_BAD_ENCODE)
}

/// The hash an AlgorithmIdentifier of a digest names.
fn digest_alg(oid: &[u8]) -> Option<Hash> {
    match oid {
        OID_SHA1 => Some(SHA1),
        [prefix @ .., n] if prefix == OID_SHA2 => match *n {
            1 => Some(SHA256),
            2 => Some(SHA384),
            3 => Some(SHA512),
            _ => None,
        },
        _ => None,
    }
}

/// An AlgorithmIdentifier: (OID contents, its parameters).
fn alg_id(t: Tlv<'_>) -> Result<(&[u8], Option<Tlv<'_>>), u32> {
    if t.tag != SEQUENCE {
        return Err(CRYPT_E_BAD_ENCODE);
    }
    let mut d = Der::new(t.content);
    let oid = d.expect(OID)?.content;
    let params = if d.peek_tag().is_some() { Some(d.next()?) } else { None };
    Ok((oid, params))
}

/// A block cipher in CBC mode, through bcrypt: the plaintext of `data`
/// (PKCS #7 padding removed); a bad padding is a wrong password.
fn cbc_decrypt(
    cipher: &str,
    key: &[u8],
    effective_bits: u32,
    iv: &[u8],
    data: &[u8],
) -> Result<Buf, u32> {
    type Open = unsafe extern "system" fn(*mut usize, *const u16, *const u16, u32) -> u32;
    type SetProperty = unsafe extern "system" fn(usize, *const u16, *const u8, u32, u32) -> u32;
    type Generate =
        unsafe extern "system" fn(usize, *mut usize, *mut u8, u32, *const u8, u32, u32) -> u32;
    type Decrypt = unsafe extern "system" fn(
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
    type Destroy = unsafe extern "system" fn(usize) -> u32;
    type Close = unsafe extern "system" fn(usize, u32) -> u32;
    let names = [
        c"BCryptOpenAlgorithmProvider",
        c"BCryptSetProperty",
        c"BCryptGenerateSymmetricKey",
        c"BCryptDecrypt",
        c"BCryptDestroyKey",
        c"BCryptCloseAlgorithmProvider",
    ];
    let mut f = [0usize; 6];
    for (slot, name) in f.iter_mut().zip(names) {
        *slot = bcrypt(name).ok_or(NTE_NOT_SUPPORTED)?;
    }
    let wide = |s: &str| -> [u16; 32] {
        let mut w = [0u16; 32];
        for (d, b) in w.iter_mut().zip(s.bytes()) {
            *d = b.into();
        }
        w
    };
    let (name, mode, chaining, effective) =
        (wide(cipher), wide("ChainingModeCBC"), wide("ChainingMode"), wide("EffectiveKeyLength"));
    let mut out = Buf::new(data.len())?;
    let mut iv_copy = [0u8; 16];
    iv_copy[..iv.len()].copy_from_slice(iv);
    // SAFETY: bcrypt's documented signatures; our buffers and handles.
    unsafe {
        let mut alg = 0usize;
        if core::mem::transmute::<usize, Open>(f[0])(&mut alg, name.as_ptr(), core::ptr::null(), 0)
            != 0
        {
            return Err(NTE_NOT_SUPPORTED);
        }
        let set = core::mem::transmute::<usize, SetProperty>(f[1]);
        let mut status = set(alg, chaining.as_ptr(), mode.as_ptr().cast(), 32, 0);
        let mut k = 0usize;
        if status == 0 {
            status = core::mem::transmute::<usize, Generate>(f[2])(
                alg,
                &mut k,
                core::ptr::null_mut(),
                0,
                key.as_ptr(),
                key.len() as u32,
                0,
            );
        }
        if status == 0 && effective_bits != 0 {
            status = set(k, effective.as_ptr(), effective_bits.to_le_bytes().as_ptr(), 4, 0);
        }
        let mut n = 0u32;
        if status == 0 {
            status = core::mem::transmute::<usize, Decrypt>(f[3])(
                k,
                data.as_ptr(),
                data.len() as u32,
                core::ptr::null(),
                iv_copy.as_mut_ptr(),
                iv.len() as u32,
                out.p,
                data.len() as u32,
                &mut n,
                1, // BCRYPT_BLOCK_PADDING
            );
        }
        if k != 0 {
            core::mem::transmute::<usize, Destroy>(f[4])(k);
        }
        core::mem::transmute::<usize, Close>(f[5])(alg, 0);
        if status != 0 {
            return Err(ERROR_INVALID_PASSWORD);
        }
        out.len = n as usize;
    }
    Ok(out)
}

/// Decrypts `data` encrypted under AlgorithmIdentifier `alg`.
fn decrypt(alg: Tlv<'_>, data: &[u8], password: &Password) -> Result<Buf, u32> {
    let (oid, params) = alg_id(alg)?;
    let params = params.ok_or(CRYPT_E_BAD_ENCODE)?;
    if let [prefix @ .., scheme] = oid
        && prefix == OID_PKCS12_PBE
    {
        // (cipher, key length, RC2's effective bits)
        let (cipher, key_len, bits) = match scheme {
            3 => ("3DES", 24, 0),
            4 => ("3DES_112", 16, 0),
            5 => ("RC2", 16, 128),
            6 => ("RC2", 5, 40),
            _ => {
                unsupported(b"crypt32.dll!PFXImportCertStore(PBE scheme)", (*scheme).into());
                return Err(NTE_NOT_SUPPORTED);
            }
        };
        let mut d = Der::new(params.content);
        let salt = d.expect(OCTET_STRING)?.content;
        let iterations = int(d.next()?)?;
        let (mut key, mut iv) = ([0u8; 24], [0u8; 8]);
        pkcs12_kdf(SHA1, 1, password.bmp.bytes(), salt, iterations, &mut key[..key_len])?;
        pkcs12_kdf(SHA1, 2, password.bmp.bytes(), salt, iterations, &mut iv)?;
        let r = cbc_decrypt(cipher, &key[..key_len], bits, &iv, data);
        key.fill(0);
        return r;
    }
    if oid != OID_PBES2 {
        unsupported(b"crypt32.dll!PFXImportCertStore(encryption scheme)", 0);
        return Err(NTE_NOT_SUPPORTED);
    }
    let mut d = Der::new(params.content);
    let (kdf, kdf_params) = alg_id(d.next()?)?;
    let (enc, enc_params) = alg_id(d.next()?)?;
    if kdf != OID_PBKDF2 {
        unsupported(b"crypt32.dll!PFXImportCertStore(PBES2 key derivation)", 0);
        return Err(NTE_NOT_SUPPORTED);
    }
    let mut k = Der::new(kdf_params.ok_or(CRYPT_E_BAD_ENCODE)?.content);
    let salt = k.expect(OCTET_STRING)?.content;
    let iterations = int(k.next()?)?;
    let mut hash = SHA1;
    let mut asked_len = None;
    while k.peek_tag().is_some() {
        let t = k.next()?;
        if t.tag == INTEGER {
            asked_len = Some(int(t)? as usize);
        } else {
            hash = match alg_id(t)?.0 {
                [prefix @ .., 7] if prefix == OID_HMAC => SHA1,
                [prefix @ .., 9] if prefix == OID_HMAC => SHA256,
                [prefix @ .., 10] if prefix == OID_HMAC => SHA384,
                [prefix @ .., 11] if prefix == OID_HMAC => SHA512,
                _ => {
                    unsupported(b"crypt32.dll!PFXImportCertStore(PBKDF2 PRF)", 0);
                    return Err(NTE_NOT_SUPPORTED);
                }
            };
        }
    }
    let (cipher, key_len) = match enc {
        [prefix @ .., 2] if prefix == OID_AES => ("AES", 16),
        [prefix @ .., 22] if prefix == OID_AES => ("AES", 24),
        [prefix @ .., 42] if prefix == OID_AES => ("AES", 32),
        OID_DES_EDE3_CBC => ("3DES", 24),
        _ => {
            unsupported(b"crypt32.dll!PFXImportCertStore(PBES2 cipher)", 0);
            return Err(NTE_NOT_SUPPORTED);
        }
    };
    if asked_len.is_some_and(|n| n != key_len) {
        return Err(CRYPT_E_BAD_ENCODE);
    }
    let iv = enc_params.filter(|t| t.tag == OCTET_STRING).ok_or(CRYPT_E_BAD_ENCODE)?.content;
    if iv.len() != if cipher == "AES" { 16 } else { 8 } {
        return Err(CRYPT_E_BAD_ENCODE);
    }
    let mut key = [0u8; 32];
    pbkdf2(hash, password.utf8.bytes(), salt, iterations, &mut key[..key_len])?;
    let r = cbc_decrypt(cipher, &key[..key_len], 0, iv, data);
    key.fill(0);
    r
}

/// Checks the PFX's MacData over `content`.
fn mac_matches(mac_data: Tlv<'_>, content: &[u8], password: &Password) -> Result<bool, u32> {
    let mut d = Der::new(mac_data.content);
    let mut info = Der::new(d.expect(SEQUENCE)?.content);
    let (oid, _) = alg_id(info.next()?)?;
    let expected = info.expect(OCTET_STRING)?.content;
    let salt = d.expect(OCTET_STRING)?.content;
    let iterations = if d.peek_tag().is_some() { int(d.next()?)? } else { 1 };
    let Some(hash) = digest_alg(oid) else {
        unsupported(b"crypt32.dll!PFXImportCertStore(MAC digest)", 0);
        return Err(NTE_NOT_SUPPORTED);
    };
    let mut key = [0u8; 64];
    pkcs12_kdf(hash, 3, password.bmp.bytes(), salt, iterations, &mut key[..hash.len])?;
    let mut mac = [0u8; 64];
    digest(hash, Some(&key[..hash.len]), &[content], &mut mac[..hash.len])?;
    key.fill(0);
    Ok(expected == &mac[..hash.len])
}

const MAX_CERTS: usize = 32;
const MAX_KEYS: usize = 8;
const MAX_ID: usize = 64;

/// A certificate added to the store, with its localKeyId.
struct Added {
    context: *const CertContext,
    id: [u8; MAX_ID],
    id_len: usize,
}

/// A private key (PKCS #8 PrivateKeyInfo), with its localKeyId.
struct Key {
    info: Buf,
    id: [u8; MAX_ID],
    id_len: usize,
}

struct Import<'a> {
    store: *mut c_void,
    password: &'a Password,
    certs: [Option<Added>; MAX_CERTS],
    keys: [Option<Key>; MAX_KEYS],
}

/// A bag's attributes: (localKeyId, friendlyName as UTF-16 with a NUL).
fn attributes<'a>(set: Option<Tlv<'a>>, name: &mut [u16; 128]) -> Result<(&'a [u8], usize), u32> {
    let (mut id, mut name_len) = (&[][..], 0);
    let Some(set) = set else { return Ok((id, 0)) };
    let mut d = Der::new(set.content);
    while d.peek_tag().is_some() {
        let mut a = Der::new(d.expect(SEQUENCE)?.content);
        let oid = a.expect(OID)?.content;
        let mut values = Der::new(a.expect(SET)?.content);
        let Ok(value) = values.next() else { continue };
        if oid == OID_LOCAL_KEY_ID && value.tag == OCTET_STRING {
            id = value.content;
        } else if oid == OID_FRIENDLY_NAME && value.tag == 0x1e {
            // BMPString: big-endian UTF-16.
            for (slot, pair) in name.iter_mut().zip(value.content.chunks_exact(2)).take(127) {
                *slot = u16::from_be_bytes([pair[0], pair[1]]);
                name_len += 1;
            }
            name[name_len] = 0;
        }
    }
    Ok((id, name_len))
}

impl Import<'_> {
    /// A SafeContents: its bags.
    fn safe_contents(&mut self, data: &[u8], depth: usize) -> Result<(), u32> {
        let mut bags = Der::new(Der::new(data).expect(SEQUENCE)?.content);
        while bags.peek_tag().is_some() {
            let mut bag = Der::new(bags.expect(SEQUENCE)?.content);
            let oid = bag.expect(OID)?.content;
            let value = bag.expect(0xa0)?;
            let attrs = if bag.peek_tag() == Some(SET) { Some(bag.next()?) } else { None };
            let kind = match oid {
                [prefix @ .., n] if prefix == OID_BAG => *n,
                _ => 0,
            };
            let mut name = [0u16; 128];
            let (id, name_len) = attributes(attrs, &mut name)?;
            let inner = Der::new(value.content).next()?;
            match kind {
                BAG_CERT => {
                    let mut c = Der::new(inner.content);
                    if c.expect(OID)?.content != OID_X509_CERT {
                        unsupported(b"crypt32.dll!PFXImportCertStore(certificate type)", 0);
                        continue;
                    }
                    let der = Der::new(c.expect(0xa0)?.content).expect(OCTET_STRING)?.content;
                    self.add_cert(der, id, &name[..name_len + usize::from(name_len > 0)])?;
                }
                BAG_KEY => self.add_key(Buf::copy(inner.whole)?, id)?,
                BAG_SHROUDED_KEY => {
                    let mut e = Der::new(inner.content);
                    let alg = e.next()?;
                    let data = e.expect(OCTET_STRING)?.content;
                    let info = decrypt(alg, data, self.password)?;
                    self.add_key(info, id)?;
                }
                BAG_SAFE_CONTENTS if depth < 4 => self.safe_contents(inner.whole, depth + 1)?,
                _ => unsupported(b"crypt32.dll!PFXImportCertStore(bag type)", kind.into()),
            }
        }
        Ok(())
    }

    fn add_cert(&mut self, der: &[u8], id: &[u8], name: &[u16]) -> Result<(), u32> {
        let Some(slot) = self.certs.iter_mut().find(|c| c.is_none()) else {
            unsupported(b"crypt32.dll!PFXImportCertStore(more certificates)", MAX_CERTS as u64);
            return Err(ERROR_NOT_ENOUGH_MEMORY);
        };
        let mut context = core::ptr::null();
        if CertAddEncodedCertificateToStore(
            self.store,
            X509_ASN_ENCODING,
            der.as_ptr(),
            der.len() as u32,
            CERT_STORE_ADD_ALWAYS,
            &mut context,
        ) == 0
        {
            // SAFETY: plain call.
            return Err(unsafe { GetLastError() });
        }
        if !name.is_empty() {
            let blob = DataBlob { size: (2 * name.len()) as u32, data: name.as_ptr().cast() };
            CertSetCertificateContextProperty(
                context,
                CERT_FRIENDLY_NAME_PROP_ID,
                0,
                (&blob as *const DataBlob).cast(),
            );
        }
        let mut added = Added { context, id: [0; MAX_ID], id_len: id.len().min(MAX_ID) };
        added.id[..added.id_len].copy_from_slice(&id[..added.id_len]);
        *slot = Some(added);
        Ok(())
    }

    fn add_key(&mut self, info: Buf, id: &[u8]) -> Result<(), u32> {
        let Some(slot) = self.keys.iter_mut().find(|k| k.is_none()) else {
            unsupported(b"crypt32.dll!PFXImportCertStore(more keys)", MAX_KEYS as u64);
            return Err(ERROR_NOT_ENOUGH_MEMORY);
        };
        let mut key = Key { info, id: [0; MAX_ID], id_len: id.len().min(MAX_ID) };
        key.id[..key.id_len].copy_from_slice(&id[..key.id_len]);
        *slot = Some(key);
        Ok(())
    }

    /// Gives each key to its certificate (same localKeyId; without ids,
    /// the only certificate).
    fn place_keys(&mut self, flags: u32) -> Result<(), u32> {
        let count = self.certs.iter().filter(|c| c.is_some()).count();
        for key in self.keys.iter().flatten() {
            let id = &key.id[..key.id_len];
            let owner = self.certs.iter().flatten().find(|c| {
                if id.is_empty() || c.id_len == 0 { count == 1 } else { &c.id[..c.id_len] == id }
            });
            match owner {
                Some(c) => store_key(c.context, key.info.bytes(), flags)?,
                None => {
                    unsupported(b"crypt32.dll!PFXImportCertStore(a key without its certificate)", 0)
                }
            }
        }
        Ok(())
    }
}

impl Buf {
    fn copy(data: &[u8]) -> Result<Buf, u32> {
        let mut b = Buf::new(data.len())?;
        b.bytes_mut().copy_from_slice(data);
        Ok(b)
    }
}

/// A container name for a persisted key: a new GUID in braces.
fn container_name(out: &mut [u16; 40]) -> Result<(), u32> {
    type Random = unsafe extern "system" fn(usize, *mut u8, u32, u32) -> u32;
    let f = bcrypt(c"BCryptGenRandom").ok_or(NTE_NOT_SUPPORTED)?;
    let mut g = [0u8; 16];
    // SAFETY: BCryptGenRandom's documented signature (the system RNG).
    if unsafe { core::mem::transmute::<usize, Random>(f)(0, g.as_mut_ptr(), 16, 2) } != 0 {
        return Err(NTE_NOT_SUPPORTED);
    }
    g[6] = (g[6] & 0x0f) | 0x40;
    g[8] = (g[8] & 0x3f) | 0x80;
    let hex = b"0123456789abcdef";
    let mut n = 0;
    let mut put = |c: u8| {
        out[n] = c.into();
        n += 1;
    };
    put(b'{');
    for (i, b) in g.iter().enumerate() {
        if matches!(i, 4 | 6 | 8 | 10) {
            put(b'-');
        }
        put(hex[usize::from(b >> 4)]);
        put(hex[usize::from(b & 15)]);
    }
    put(b'}');
    put(0);
    Ok(())
}

const KSP: &str = "Microsoft Software Key Storage Provider";

/// Imports PKCS #8 `info` into the software KSP for `cert`: persisted
/// and named by CERT_KEY_PROV_INFO_PROP_ID, or held by the context.
fn store_key(cert: *const CertContext, info: &[u8], flags: u32) -> Result<(), u32> {
    type Open = unsafe extern "system" fn(*mut usize, *const u16, u32) -> i32;
    type ImportKey = unsafe extern "system" fn(
        usize,
        usize,
        *const u16,
        *const NBufferDesc,
        *mut usize,
        *const u8,
        u32,
        u32,
    ) -> i32;
    let (Some(open), Some(import)) =
        (ncrypt(c"NCryptOpenStorageProvider"), ncrypt(c"NCryptImportKey"))
    else {
        return Err(NTE_NOT_SUPPORTED);
    };
    let mut provider_name = [0u16; 48];
    for (d, b) in provider_name.iter_mut().zip(KSP.bytes()) {
        *d = b.into();
    }
    let blob_type: [u16; 17] =
        core::array::from_fn(|i| b"PKCS8_PRIVATEKEY\0".get(i).copied().unwrap_or(0).into());
    let persist = flags & PKCS12_NO_PERSIST_KEY == 0;
    let mut container = [0u16; 40];
    if persist {
        container_name(&mut container)?;
    }
    let buffer = NBuffer {
        size: 2 * 39, // the GUID in braces and its NUL
        kind: 45,     // NCRYPTBUFFER_PKCS_KEY_NAME
        data: container.as_ptr().cast(),
    };
    let desc = NBufferDesc { version: 0, count: 1, buffers: &buffer };
    let (mut prov, mut key) = (0usize, 0usize);
    // SAFETY: ncrypt's documented signatures; our buffers.
    let r = unsafe {
        let r = core::mem::transmute::<usize, Open>(open)(&mut prov, provider_name.as_ptr(), 0);
        if r != 0 {
            return Err(r as u32);
        }
        let r = core::mem::transmute::<usize, ImportKey>(import)(
            prov,
            0,
            blob_type.as_ptr(),
            if persist { &desc } else { core::ptr::null() },
            &mut key,
            info.as_ptr(),
            info.len() as u32,
            0,
        );
        crate::cert::free_ncrypt_key(prov);
        r
    };
    if r != 0 {
        return Err(r as u32);
    }
    if !persist {
        return if give_key(cert, key) {
            Ok(())
        } else {
            crate::cert::free_ncrypt_key(key);
            Err(ERROR_NOT_ENOUGH_MEMORY)
        };
    }
    crate::cert::free_ncrypt_key(key);
    // CRYPT_KEY_PROV_INFO (x64): container, provider, dwProvType,
    // dwFlags, cProvParam, rgProvParam, dwKeySpec.
    #[repr(C)]
    struct ProvInfo {
        container: *const u16,
        provider: *const u16,
        provider_type: u32,
        flags: u32,
        params: u32,
        param_list: *const c_void,
        spec: u32,
    }
    let prov_info = ProvInfo {
        container: container.as_ptr(),
        provider: provider_name.as_ptr(),
        provider_type: 0,
        flags: flags & CRYPT_MACHINE_KEYSET,
        params: 0,
        param_list: core::ptr::null(),
        spec: AT_KEYEXCHANGE,
    };
    if CertSetCertificateContextProperty(
        cert,
        CERT_KEY_PROV_INFO_PROP_ID,
        0,
        (&prov_info as *const ProvInfo).cast(),
    ) == 0
    {
        return Err(ERROR_NOT_ENOUGH_MEMORY);
    }
    Ok(())
}

/// NCryptBuffer / NCryptBufferDesc.
#[repr(C)]
struct NBuffer {
    size: u32,
    kind: u32,
    data: *const c_void,
}

#[repr(C)]
struct NBufferDesc {
    version: u32,
    count: u32,
    buffers: *const NBuffer,
}

/// The PFX's parts: (the AuthenticatedSafe's encoding, the MacData).
fn outer(pfx: &[u8]) -> Result<(&[u8], Option<Tlv<'_>>), u32> {
    let mut d = Der::new(Der::new(pfx).expect(SEQUENCE)?.content);
    if int(d.next()?)? != 3 {
        return Err(CRYPT_E_BAD_ENCODE);
    }
    let mut info = Der::new(d.expect(SEQUENCE)?.content);
    if info.expect(OID)?.content != OID_DATA {
        unsupported(b"crypt32.dll!PFXImportCertStore(public-key integrity)", 0);
        return Err(NTE_NOT_SUPPORTED);
    }
    let content = Der::new(info.expect(0xa0)?.content).expect(OCTET_STRING)?.content;
    let mac = if d.peek_tag().is_some() { Some(d.expect(SEQUENCE)?) } else { None };
    Ok((content, mac))
}

/// The AuthenticatedSafe's ContentInfos into `import`.
fn contents(import: &mut Import<'_>, auth_safe: &[u8]) -> Result<(), u32> {
    let mut infos = Der::new(Der::new(auth_safe).expect(SEQUENCE)?.content);
    while infos.peek_tag().is_some() {
        let mut info = Der::new(infos.expect(SEQUENCE)?.content);
        let kind = info.expect(OID)?.content;
        let content = Der::new(info.expect(0xa0)?.content).next()?;
        if kind == OID_DATA && content.tag == OCTET_STRING {
            import.safe_contents(content.content, 0)?;
        } else if kind == OID_ENCRYPTED_DATA && content.tag == SEQUENCE {
            // EncryptedData: version, EncryptedContentInfo { contentType,
            // algorithm, [0] IMPLICIT encryptedContent }.
            let mut e = Der::new(content.content);
            int(e.next()?)?;
            let mut eci = Der::new(e.expect(SEQUENCE)?.content);
            eci.expect(OID)?;
            let alg = eci.next()?;
            let data = eci.expect(0x80)?.content;
            let plain = decrypt(alg, data, import.password)?;
            import.safe_contents(plain.bytes(), 0)?;
        } else {
            unsupported(b"crypt32.dll!PFXImportCertStore(content type)", 0);
            return Err(NTE_NOT_SUPPORTED);
        }
    }
    Ok(())
}

fn import_with(pfx: &[u8], password: &Password, flags: u32) -> Result<*mut c_void, u32> {
    let (auth_safe, _) = outer(pfx)?;
    let store = CertOpenStore(CERT_STORE_PROV_MEMORY as *const u8, 0, 0, 0, core::ptr::null());
    if store.is_null() {
        return Err(ERROR_NOT_ENOUGH_MEMORY);
    }
    let mut import = Import {
        store,
        password,
        certs: [const { None }; MAX_CERTS],
        keys: [const { None }; MAX_KEYS],
    };
    let r = contents(&mut import, auth_safe).and_then(|()| import.place_keys(flags));
    for c in import.certs.iter().flatten() {
        CertFreeCertificateContext(c.context);
    }
    match r {
        Ok(()) => Ok(store),
        Err(e) => {
            CertCloseStore(store, 0);
            Err(e)
        }
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn PFXImportCertStore(
    pfx: *const DataBlob,
    password: *const u16,
    flags: u32,
) -> *mut c_void {
    let null = core::ptr::null_mut();
    if pfx.is_null() {
        return error(ERROR_INVALID_PARAMETER, null);
    }
    // SAFETY: the caller's blob.
    let blob = unsafe { &*pfx };
    if blob.data.is_null() || blob.size == 0 {
        return error(ERROR_INVALID_PARAMETER, null);
    }
    // SAFETY: the blob's bytes.
    let data = unsafe { core::slice::from_raw_parts(blob.data, blob.size as usize) };
    let mut chars: &[u16] = &[];
    if !password.is_null() {
        let mut n = 0;
        // SAFETY: the caller's NUL-terminated password.
        while unsafe { *password.add(n) } != 0 {
            n += 1;
        }
        // SAFETY: as above.
        chars = unsafe { core::slice::from_raw_parts(password, n) };
    }
    if flags & (PKCS12_PREFER_CNG_KSP | PKCS12_ALWAYS_CNG_KSP) == 0 {
        unsupported(b"crypt32.dll!PFXImportCertStore(CSP key in the KSP)", flags.into());
    }
    let (auth_safe, mac) = match outer(data) {
        Ok(parts) => parts,
        Err(e) => return error(e, null),
    };
    // The empty password has two forms; the MAC tells which was used.
    let forms: &[bool] = if chars.is_empty() { &[false, true] } else { &[false] };
    let mut chosen = None;
    for &bare in forms {
        let p = match Password::new(chars, bare) {
            Ok(p) => p,
            Err(e) => return error(e, null),
        };
        let Some(mac) = mac else {
            chosen = Some(p);
            break;
        };
        match mac_matches(mac, auth_safe, &p) {
            Ok(true) => {
                chosen = Some(p);
                break;
            }
            Ok(false) => {}
            Err(e) => return error(e, null),
        }
    }
    let Some(password) = chosen else { return error(ERROR_INVALID_PASSWORD, null) };
    match import_with(data, &password, flags) {
        Ok(store) => store,
        Err(e) => error(e, null),
    }
}

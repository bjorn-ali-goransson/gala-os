//! bcrypt.dll (CNG) as .NET uses it, with the results real Windows gives:
//! the documented NTSTATUS codes and the published test vectors (FIPS 180
//! for SHA-1/SHA-2 of "abc", RFC 1321 for MD5, RFC 2202 and RFC 4231
//! test case 2 for HMAC-SHA1 and HMAC-SHA256):
//!
//! - BCryptGetFipsAlgorithmMode: not enforced.
//! - Symmetric ciphers with published test vectors: AES (FIPS-197, SP
//!   800-38A), DES, 3DES (SP 800-67), RC2 (RFC 2268), RC4; properties,
//!   ECB/CBC, block padding, key data blobs. MD2/MD4 (RFC 1319/1320).
//! - BCryptGenRandom with BCRYPT_USE_SYSTEM_PREFERRED_RNG, an RNG
//!   provider and the RNG pseudo-handle; STATUS_INVALID_HANDLE without one.
//! - BCryptOpenAlgorithmProvider / Close (STATUS_NOT_FOUND for an unknown
//!   algorithm), BCryptGetProperty (ObjectLength, HashDigestLength,
//!   HashBlockLength; STATUS_BUFFER_TOO_SMALL).
//! - BCryptCreateHash / HashData / FinishHash (in pieces, reusable,
//!   duplicated; STATUS_INVALID_PARAMETER for a wrong output size),
//!   BCryptDestroyHash; HMAC providers and secrets.
//! - BCryptHash (one shot) with algorithm pseudo-handles.
//! - RSA (rsa_vectors.rs): BCryptImportKeyPair of RSAFULLPRIVATEBLOB,
//!   RSAPRIVATEBLOB and RSAPUBLICBLOB; BCryptSignHash with PKCS#1 v1.5
//!   (SHA-256: OpenSSL's signature, the size query) and PSS;
//!   BCryptVerifySignature (STATUS_INVALID_SIGNATURE when altered);
//!   BCryptDecrypt of OpenSSL's OAEP and PKCS#1 ciphertexts, BCryptEncrypt
//!   round trips; BCryptExportKey of both blobs (the imported bytes);
//!   KeyLength; BCryptGenerateKeyPair/FinalizeKeyPair.
//!
//! Prints `bcrypt_test: ok` or exits with the number of the failing check.

#![no_std]
#![no_main]

mod rsa_vectors;

use core::ffi::c_void;

type Bool = i32;
type Handle = *mut c_void;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut c_void) -> Bool;
    fn ExitProcess(code: u32) -> !;
}

#[link(name = "bcrypt", kind = "raw-dylib")]
unsafe extern "system" {
    fn BCryptGenRandom(alg: Handle, buf: *mut u8, len: u32, flags: u32) -> u32;
    fn BCryptGetFipsAlgorithmMode(enabled: *mut u8) -> u32;
    fn BCryptOpenAlgorithmProvider(
        alg: *mut Handle,
        id: *const u16,
        imp: *const u16,
        flags: u32,
    ) -> u32;
    fn BCryptCloseAlgorithmProvider(alg: Handle, flags: u32) -> u32;
    fn BCryptGetProperty(
        obj: Handle,
        name: *const u16,
        out: *mut u8,
        len: u32,
        result: *mut u32,
        flags: u32,
    ) -> u32;
    fn BCryptCreateHash(
        alg: Handle,
        hash: *mut Handle,
        object: *mut u8,
        object_len: u32,
        secret: *const u8,
        secret_len: u32,
        flags: u32,
    ) -> u32;
    fn BCryptHashData(hash: Handle, data: *const u8, len: u32, flags: u32) -> u32;
    fn BCryptFinishHash(hash: Handle, out: *mut u8, len: u32, flags: u32) -> u32;
    fn BCryptDuplicateHash(
        hash: Handle,
        new: *mut Handle,
        object: *mut u8,
        len: u32,
        flags: u32,
    ) -> u32;
    fn BCryptDestroyHash(hash: Handle) -> u32;
    fn BCryptSetProperty(
        obj: Handle,
        name: *const u16,
        input: *const u8,
        len: u32,
        flags: u32,
    ) -> u32;
    fn BCryptGenerateSymmetricKey(
        alg: Handle,
        key: *mut Handle,
        object: *mut u8,
        object_len: u32,
        secret: *const u8,
        secret_len: u32,
        flags: u32,
    ) -> u32;
    fn BCryptEncrypt(
        key: Handle,
        input: *const u8,
        len: u32,
        padding: *const c_void,
        iv: *mut u8,
        iv_len: u32,
        out: *mut u8,
        out_len: u32,
        result: *mut u32,
        flags: u32,
    ) -> u32;
    fn BCryptDecrypt(
        key: Handle,
        input: *const u8,
        len: u32,
        padding: *const c_void,
        iv: *mut u8,
        iv_len: u32,
        out: *mut u8,
        out_len: u32,
        result: *mut u32,
        flags: u32,
    ) -> u32;
    fn BCryptExportKey(
        key: Handle,
        export: Handle,
        blob: *const u16,
        out: *mut u8,
        len: u32,
        result: *mut u32,
        flags: u32,
    ) -> u32;
    fn BCryptImportKey(
        alg: Handle,
        import: Handle,
        blob: *const u16,
        key: *mut Handle,
        object: *mut u8,
        object_len: u32,
        input: *const u8,
        len: u32,
        flags: u32,
    ) -> u32;
    fn BCryptDestroyKey(key: Handle) -> u32;
    fn BCryptImportKeyPair(
        alg: Handle,
        import: Handle,
        blob: *const u16,
        key: *mut Handle,
        input: *const u8,
        len: u32,
        flags: u32,
    ) -> u32;
    fn BCryptGenerateKeyPair(alg: Handle, key: *mut Handle, bits: u32, flags: u32) -> u32;
    fn BCryptFinalizeKeyPair(key: Handle, flags: u32) -> u32;
    fn BCryptSignHash(
        key: Handle,
        padding: *const c_void,
        hash: *const u8,
        hash_len: u32,
        out: *mut u8,
        out_len: u32,
        result: *mut u32,
        flags: u32,
    ) -> u32;
    fn BCryptVerifySignature(
        key: Handle,
        padding: *const c_void,
        hash: *const u8,
        hash_len: u32,
        sig: *const u8,
        sig_len: u32,
        flags: u32,
    ) -> u32;
    fn BCryptHash(
        alg: Handle,
        secret: *const u8,
        secret_len: u32,
        input: *const u8,
        input_len: u32,
        out: *mut u8,
        out_len: u32,
    ) -> u32;
}

#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
    fn __chkstk();
}

#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

#[unsafe(no_mangle)]
pub extern "C" fn __CxxFrameHandler3() -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdeac) }
}

const STATUS_INVALID_HANDLE: u64 = 0xc000_0008;
const STATUS_INVALID_PARAMETER: u64 = 0xc000_000d;
const STATUS_BUFFER_TOO_SMALL: u64 = 0xc000_0023;
const STATUS_NOT_FOUND: u64 = 0xc000_0225;
const BCRYPT_USE_SYSTEM_PREFERRED_RNG: u32 = 2;
const BCRYPT_ALG_HANDLE_HMAC_FLAG: u32 = 8;
const BCRYPT_HASH_REUSABLE_FLAG: u32 = 0x20;

/// Algorithm pseudo-handles ("CNG Algorithm Pseudo-handles").
fn pseudo(v: usize) -> Handle {
    core::ptr::without_provenance_mut(v)
}
const MD5: usize = 0x21;
const SHA1: usize = 0x31;
const SHA512: usize = 0x61;
const RNG: usize = 0x81;
const HMAC_SHA1: usize = 0xa1;

fn out(text: &[u8]) {
    let mut written = 0;
    // SAFETY: valid buffer and output pointer.
    unsafe {
        WriteFile(
            GetStdHandle(-11i32 as u32),
            text.as_ptr(),
            text.len() as u32,
            &mut written,
            core::ptr::null_mut(),
        )
    };
}

fn hex(v: u32) -> [u8; 8] {
    let mut s = [0u8; 8];
    for (i, c) in s.iter_mut().enumerate() {
        let d = (v >> (28 - 4 * i)) & 0xf;
        *c = if d < 10 { b'0' + d as u8 } else { b'a' + d as u8 - 10 };
    }
    s
}

struct Checks(u32);

impl Checks {
    fn ok(&mut self, cond: bool, got: u64) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        out(b"bcrypt_test: check ");
        out(&hex(self.0));
        out(b" got ");
        out(&hex(got as u32));
        out(b"\r\n");
        Err(self.0)
    }

    fn eq(&mut self, got: u64, want: u64) -> Result<(), u32> {
        self.ok(got == want, got)
    }

    /// `got` is the digest written in hexadecimal as `want`.
    fn digest(&mut self, got: &[u8], want: &str) -> Result<(), u32> {
        let same = got.len() * 2 == want.len()
            && got.iter().enumerate().all(|(i, &b)| {
                let byte = u8::from_str_radix(&want[2 * i..2 * i + 2], 16).unwrap_or(0);
                b == byte
            });
        self.ok(same, u64::from(got.first().copied().unwrap_or(0)))
    }
}

fn w(s: &str) -> [u16; 32] {
    let mut b = [0u16; 32];
    for (d, ch) in b.iter_mut().zip(s.bytes()) {
        *d = ch.into();
    }
    b
}

const SHA256_ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
const SHA1_ABC: &str = "a9993e364706816aba3e25717850c26c9cd0d89d";
const MD5_ABC: &str = "900150983cd24fb0d6963f7d28e17f72";
const SHA384_ABC: &str = "cb00753f45a35e8bb5a03d699ac65007272c32ab0eded1631a8b605a43ff5bed8086072ba1e7cc2358baeca134c825a7";
const SHA512_ABC: &str = "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f";
const HMAC_SHA256_JEFE: &str = "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843";
const HMAC_SHA1_JEFE: &str = "effcdf6ae5eb2fa2d27416d5f184df9c259a7c79";
const JEFE_DATA: &[u8] = b"what do ya want for nothing?";

/// The bytes written in hexadecimal as `text` (up to 64).
fn hex_bytes(text: &str) -> ([u8; 64], usize) {
    let mut out = [0u8; 64];
    let n = text.len() / 2;
    for (i, slot) in out.iter_mut().enumerate().take(n) {
        *slot = u8::from_str_radix(&text[2 * i..2 * i + 2], 16).unwrap_or(0);
    }
    (out, n)
}

/// Opens algorithm `name`.
fn open(c: &mut Checks, name: &str) -> Result<Handle, u32> {
    let mut alg = core::ptr::null_mut();
    // SAFETY: our outputs and name.
    let r =
        unsafe { BCryptOpenAlgorithmProvider(&mut alg, w(name).as_ptr(), core::ptr::null(), 0) };
    c.eq(r.into(), 0)?;
    Ok(alg)
}

/// Encrypts `plain` with `alg` (key and IV in hexadecimal; ECB when `iv` is
/// empty) and checks the ciphertext against `want`, then decrypts it back.
fn cipher_vector(
    c: &mut Checks,
    alg: Handle,
    key: &str,
    iv: &str,
    plain: &str,
    want: &str,
) -> Result<(), u32> {
    let (k, kn) = hex_bytes(key);
    let (mut v, vn) = hex_bytes(iv);
    let (p, pn) = hex_bytes(plain);
    let mut h = core::ptr::null_mut();
    // SAFETY: bcrypt calls with buffers of ours.
    unsafe {
        let r = BCryptGenerateSymmetricKey(
            alg,
            &mut h,
            core::ptr::null_mut(),
            0,
            k.as_ptr(),
            kn as u32,
            0,
        );
        c.eq(r.into(), 0)?;
        let mut out = [0u8; 64];
        let mut got = 0u32;
        let ivp = if vn == 0 { core::ptr::null_mut() } else { v.as_mut_ptr() };
        let r = BCryptEncrypt(
            h,
            p.as_ptr(),
            pn as u32,
            core::ptr::null(),
            ivp,
            vn as u32,
            out.as_mut_ptr(),
            64,
            &mut got,
            0,
        );
        c.eq(r.into(), 0)?;
        c.eq(got.into(), pn as u64)?;
        c.digest(&out[..pn], want)?;
        BCryptDestroyKey(h);
        // Decrypt with a fresh key and the original IV.
        let (mut v, _) = hex_bytes(iv);
        let ivp = if vn == 0 { core::ptr::null_mut() } else { v.as_mut_ptr() };
        BCryptGenerateSymmetricKey(alg, &mut h, core::ptr::null_mut(), 0, k.as_ptr(), kn as u32, 0);
        let mut back = [0u8; 64];
        let r = BCryptDecrypt(
            h,
            out.as_ptr(),
            pn as u32,
            core::ptr::null(),
            ivp,
            vn as u32,
            back.as_mut_ptr(),
            64,
            &mut got,
            0,
        );
        c.eq(r.into(), 0)?;
        c.digest(&back[..pn], plain)?;
        c.eq(BCryptDestroyKey(h).into(), 0)?;
    }
    Ok(())
}

/// Sets a provider's chaining mode.
fn chaining(c: &mut Checks, alg: Handle, mode: &str) -> Result<(), u32> {
    let m = w(mode);
    // SAFETY: our handle and string (with its NUL).
    let r = unsafe {
        BCryptSetProperty(
            alg,
            w("ChainingMode").as_ptr(),
            m.as_ptr().cast(),
            2 * (mode.len() as u32 + 1),
            0,
        )
    };
    c.eq(r.into(), 0)
}

/// Symmetric ciphers with the published test vectors: AES (FIPS-197
/// C.1/C.3 in ECB, SP 800-38A F.2.1 in CBC), DES (the classic
/// 133457799BBCDFF1 vector), 3DES (SP 800-67's three-key example), RC2
/// (RFC 2268, 64-bit effective keys), RC4; properties (BlockLength,
/// ChainingMode, KeyLengths), BCRYPT_BLOCK_PADDING, key data blobs.
fn ciphers(c: &mut Checks) -> Result<(), u32> {
    let aes = open(c, "AES")?;
    // SAFETY: bcrypt calls with buffers of ours.
    unsafe {
        let mut v = [0u8; 64];
        let mut got = 0u32;
        c.eq(
            BCryptGetProperty(aes, w("BlockLength").as_ptr(), v.as_mut_ptr(), 4, &mut got, 0)
                .into(),
            0,
        )?;
        c.eq(u32::from_le_bytes([v[0], v[1], v[2], v[3]]).into(), 16)?;
        c.eq(
            BCryptGetProperty(aes, w("ChainingMode").as_ptr(), v.as_mut_ptr(), 64, &mut got, 0)
                .into(),
            0,
        )?;
        let cbc = "ChainingModeCBC";
        c.ok(
            got as usize == 2 * (cbc.len() + 1)
                && cbc.bytes().enumerate().all(|(i, b)| v[2 * i] == b),
            got.into(),
        )?;
        c.eq(
            BCryptGetProperty(aes, w("KeyLengths").as_ptr(), v.as_mut_ptr(), 12, &mut got, 0)
                .into(),
            0,
        )?;
        let lens: [u32; 3] = core::array::from_fn(|i| {
            u32::from_le_bytes([v[4 * i], v[4 * i + 1], v[4 * i + 2], v[4 * i + 3]])
        });
        c.ok(lens == [128, 256, 64], lens[0].into())?;
    }
    cipher_vector(
        c,
        aes,
        "2b7e151628aed2a6abf7158809cf4f3c",
        "000102030405060708090a0b0c0d0e0f",
        "6bc1bee22e409f96e93d7e117393172a",
        "7649abac8119b246cee98e9b12e9197d",
    )?;
    chaining(c, aes, "ChainingModeECB")?;
    cipher_vector(
        c,
        aes,
        "000102030405060708090a0b0c0d0e0f",
        "",
        "00112233445566778899aabbccddeeff",
        "69c4e0d86a7b0430d8cdb78070b4c55a",
    )?;
    cipher_vector(
        c,
        aes,
        "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
        "",
        "00112233445566778899aabbccddeeff",
        "8ea2b7ca516745bfeafc49904b496089",
    )?;
    chaining(c, aes, "ChainingModeCBC")?;
    // Padding: 16 bytes become 32; decrypting strips it.
    // SAFETY: bcrypt calls with buffers of ours.
    unsafe {
        let (k, _) = hex_bytes("2b7e151628aed2a6abf7158809cf4f3c");
        let mut h = core::ptr::null_mut();
        BCryptGenerateSymmetricKey(aes, &mut h, core::ptr::null_mut(), 0, k.as_ptr(), 16, 0);
        let plain = *b"sixteen byte msg";
        let mut iv = [0u8; 16];
        let mut out = [0u8; 64];
        let mut got = 0u32;
        let r = BCryptEncrypt(
            h,
            plain.as_ptr(),
            16,
            core::ptr::null(),
            iv.as_mut_ptr(),
            16,
            core::ptr::null_mut(),
            0,
            &mut got,
            1,
        );
        c.eq(r.into(), 0)?;
        c.eq(got.into(), 32)?; // the size needed
        iv = [0; 16];
        let r = BCryptEncrypt(
            h,
            plain.as_ptr(),
            16,
            core::ptr::null(),
            iv.as_mut_ptr(),
            16,
            out.as_mut_ptr(),
            64,
            &mut got,
            1,
        );
        c.eq(r.into(), 0)?;
        c.eq(got.into(), 32)?;
        c.ok(iv == out[16..32], 0)?; // the IV is the last ciphertext block
        iv = [0; 16];
        let mut back = [0u8; 64];
        let r = BCryptDecrypt(
            h,
            out.as_ptr(),
            32,
            core::ptr::null(),
            iv.as_mut_ptr(),
            16,
            back.as_mut_ptr(),
            64,
            &mut got,
            1,
        );
        c.eq(r.into(), 0)?;
        c.eq(got.into(), 16)?;
        c.ok(back[..16] == plain, 0)?;
        // Not a whole block without padding: STATUS_INVALID_BUFFER_SIZE.
        let r = BCryptEncrypt(
            h,
            plain.as_ptr(),
            15,
            core::ptr::null(),
            iv.as_mut_ptr(),
            16,
            out.as_mut_ptr(),
            64,
            &mut got,
            0,
        );
        c.eq(r.into(), 0xc000_0206)?;
        // A key data blob: BCRYPT_KEY_DATA_BLOB_HEADER (magic KDBM,
        // version 1, size), then the key; imported back.
        let blob = w("KeyDataBlob");
        let r = BCryptExportKey(
            h,
            core::ptr::null_mut(),
            blob.as_ptr(),
            out.as_mut_ptr(),
            64,
            &mut got,
            0,
        );
        c.eq(r.into(), 0)?;
        c.eq(got.into(), 28)?;
        c.eq(u32::from_le_bytes([out[0], out[1], out[2], out[3]]).into(), 0x4d42_444b)?;
        c.ok(out[12..28] == k[..16], 0)?;
        let mut h2 = core::ptr::null_mut();
        let r = BCryptImportKey(
            aes,
            core::ptr::null_mut(),
            blob.as_ptr(),
            &mut h2,
            core::ptr::null_mut(),
            0,
            out.as_ptr(),
            28,
            0,
        );
        c.eq(r.into(), 0)?;
        BCryptDestroyKey(h2);
        BCryptDestroyKey(h);
        BCryptCloseAlgorithmProvider(aes, 0);
    }
    let des = open(c, "DES")?;
    chaining(c, des, "ChainingModeECB")?;
    cipher_vector(c, des, "133457799bbcdff1", "", "0123456789abcdef", "85e813540f0ab405")?;
    let tdes = open(c, "3DES")?;
    chaining(c, tdes, "ChainingModeECB")?;
    // SP 800-67's example, its first block.
    cipher_vector(
        c,
        tdes,
        "0123456789abcdef23456789abcdef01456789abcdef0123",
        "",
        "5468652071756663",
        "a826fd8ce53b855f",
    )?;
    let rc2 = open(c, "RC2")?;
    chaining(c, rc2, "ChainingModeECB")?;
    // RFC 2268's vectors use a 64-bit effective key length.
    let bits = 64u32.to_le_bytes();
    // SAFETY: our handle and value.
    let r =
        unsafe { BCryptSetProperty(rc2, w("EffectiveKeyLength").as_ptr(), bits.as_ptr(), 4, 0) };
    c.eq(r.into(), 0)?;
    cipher_vector(c, rc2, "ffffffffffffffff", "", "ffffffffffffffff", "278b27e42e2f0d49")?;
    cipher_vector(c, rc2, "3000000000000000", "", "1000000000000001", "30649edf9be7d2c2")?;
    let rc4 = open(c, "RC4")?;
    cipher_vector(c, rc4, "4b6579", "", "506c61696e74657874", "bbf316e8d940af0ad3")?;
    // Two-key 3DES is three-key 3DES with K3 = K1.
    let tdes112 = open(c, "3DES_112")?;
    chaining(c, tdes112, "ChainingModeECB")?;
    let (k1k2, _) = hex_bytes("0123456789abcdef23456789abcdef01");
    let (k1k2k1, _) = hex_bytes("0123456789abcdef23456789abcdef010123456789abcdef");
    let plain = *b"8 bytes!";
    let (mut a, mut b) = ([0u8; 8], [0u8; 8]);
    // SAFETY: bcrypt calls with buffers of ours.
    unsafe {
        let (mut h2, mut h3, mut got) = (core::ptr::null_mut(), core::ptr::null_mut(), 0u32);
        c.eq(
            BCryptGenerateSymmetricKey(
                tdes112,
                &mut h2,
                core::ptr::null_mut(),
                0,
                k1k2.as_ptr(),
                16,
                0,
            )
            .into(),
            0,
        )?;
        c.eq(
            BCryptGenerateSymmetricKey(
                tdes,
                &mut h3,
                core::ptr::null_mut(),
                0,
                k1k2k1.as_ptr(),
                24,
                0,
            )
            .into(),
            0,
        )?;
        BCryptEncrypt(
            h2,
            plain.as_ptr(),
            8,
            core::ptr::null(),
            core::ptr::null_mut(),
            0,
            a.as_mut_ptr(),
            8,
            &mut got,
            0,
        );
        BCryptEncrypt(
            h3,
            plain.as_ptr(),
            8,
            core::ptr::null(),
            core::ptr::null_mut(),
            0,
            b.as_mut_ptr(),
            8,
            &mut got,
            0,
        );
        c.ok(a == b && a != plain, a[0].into())?;
        BCryptDestroyKey(h2);
        BCryptDestroyKey(h3);
    }
    Ok(())
}

/// MD2 and MD4 of "abc" (RFC 1319, RFC 1320).
fn old_hashes(c: &mut Checks) -> Result<(), u32> {
    for (name, want) in
        [("MD2", "da853b0d3f88d99b30283a69e6ded6bb"), ("MD4", "a448017aaf21d8525fc10ae87aa6729d")]
    {
        let alg = open(c, name)?;
        let mut out = [0u8; 16];
        // SAFETY: bcrypt calls with buffers of ours.
        let r = unsafe {
            BCryptHash(alg, core::ptr::null(), 0, b"abc".as_ptr(), 3, out.as_mut_ptr(), 16)
        };
        c.eq(r.into(), 0)?;
        c.digest(&out, want)?;
    }
    Ok(())
}

fn random(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: bcrypt calls with buffers of ours.
    unsafe {
        let (mut a, mut b) = ([0u8; 32], [0u8; 32]);
        let null = core::ptr::null_mut();
        c.eq(BCryptGenRandom(null, a.as_mut_ptr(), 32, BCRYPT_USE_SYSTEM_PREFERRED_RNG).into(), 0)?;
        c.eq(BCryptGenRandom(null, b.as_mut_ptr(), 32, BCRYPT_USE_SYSTEM_PREFERRED_RNG).into(), 0)?;
        c.ok(a != b && a != [0; 32], 0)?;
        c.eq(BCryptGenRandom(null, a.as_mut_ptr(), 32, 0).into(), STATUS_INVALID_HANDLE)?;
        let mut rng = null;
        let name = w("RNG");
        c.eq(BCryptOpenAlgorithmProvider(&mut rng, name.as_ptr(), core::ptr::null(), 0).into(), 0)?;
        c.eq(BCryptGenRandom(rng, a.as_mut_ptr(), 32, 0).into(), 0)?;
        c.eq(BCryptCloseAlgorithmProvider(rng, 0).into(), 0)?;
        c.eq(BCryptGenRandom(pseudo(RNG), b.as_mut_ptr(), 32, 0).into(), 0)?;
        c.ok(a != b, 0)?;
    }
    Ok(())
}

/// BCryptGetFipsAlgorithmMode: FIPS mode is not enforced (no policy).
fn fips(c: &mut Checks) -> Result<(), u32> {
    let mut enabled = 0xffu8;
    // SAFETY: our output.
    c.eq(unsafe { BCryptGetFipsAlgorithmMode(&mut enabled) }.into(), 0)?;
    c.eq(enabled.into(), 0)
}

fn hashes(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: bcrypt calls with buffers and handles of ours.
    unsafe {
        let null = core::ptr::null_mut();
        let mut alg = null;
        let nope = w("NOPE");
        c.eq(
            BCryptOpenAlgorithmProvider(&mut alg, nope.as_ptr(), core::ptr::null(), 0).into(),
            STATUS_NOT_FOUND,
        )?;
        let sha256 = w("SHA256");
        c.eq(
            BCryptOpenAlgorithmProvider(&mut alg, sha256.as_ptr(), core::ptr::null(), 0).into(),
            0,
        )?;
        let (mut v, mut got) = (0u32, 0u32);
        let length = w("HashDigestLength");
        let p = (&mut v as *mut u32).cast();
        c.eq(BCryptGetProperty(alg, length.as_ptr(), p, 4, &mut got, 0).into(), 0)?;
        c.ok(v == 32 && got == 4, v.into())?;
        let block = w("HashBlockLength");
        c.eq(BCryptGetProperty(alg, block.as_ptr(), p, 4, &mut got, 0).into(), 0)?;
        c.eq(v.into(), 64)?;
        let object = w("ObjectLength");
        c.eq(BCryptGetProperty(alg, object.as_ptr(), p, 4, &mut got, 0).into(), 0)?;
        c.ok(v > 0, 0)?;
        c.eq(
            BCryptGetProperty(alg, object.as_ptr(), p, 2, &mut got, 0).into(),
            STATUS_BUFFER_TOO_SMALL,
        )?;

        // In pieces.
        let mut h = null;
        c.eq(BCryptCreateHash(alg, &mut h, null.cast(), 0, core::ptr::null(), 0, 0).into(), 0)?;
        c.eq(BCryptHashData(h, b"a".as_ptr(), 1, 0).into(), 0)?;
        // A duplicate carries the state so far.
        let mut d = null;
        c.eq(BCryptDuplicateHash(h, &mut d, null.cast(), 0, 0).into(), 0)?;
        c.eq(BCryptHashData(h, b"bc".as_ptr(), 2, 0).into(), 0)?;
        let mut digest = [0u8; 64];
        c.eq(BCryptFinishHash(h, digest.as_mut_ptr(), 31, 0).into(), STATUS_INVALID_PARAMETER)?;
        c.eq(BCryptFinishHash(h, digest.as_mut_ptr(), 32, 0).into(), 0)?;
        c.digest(&digest[..32], SHA256_ABC)?;
        c.eq(BCryptHashData(d, b"bc".as_ptr(), 2, 0).into(), 0)?;
        c.eq(BCryptFinishHash(d, digest.as_mut_ptr(), 32, 0).into(), 0)?;
        c.digest(&digest[..32], SHA256_ABC)?;
        c.eq(BCryptDestroyHash(h).into(), 0)?;
        c.eq(BCryptDestroyHash(d).into(), 0)?;

        // Reusable: finishing starts over.
        c.eq(
            BCryptCreateHash(
                alg,
                &mut h,
                null.cast(),
                0,
                core::ptr::null(),
                0,
                BCRYPT_HASH_REUSABLE_FLAG,
            )
            .into(),
            0,
        )?;
        for _ in 0..2 {
            c.eq(BCryptHashData(h, b"abc".as_ptr(), 3, 0).into(), 0)?;
            c.eq(BCryptFinishHash(h, digest.as_mut_ptr(), 32, 0).into(), 0)?;
            c.digest(&digest[..32], SHA256_ABC)?;
        }
        BCryptDestroyHash(h);
        c.eq(BCryptCloseAlgorithmProvider(alg, 0).into(), 0)?;

        // SHA-384 through a provider; one-shot hashes through pseudo-handles.
        let sha384 = w("SHA384");
        c.eq(
            BCryptOpenAlgorithmProvider(&mut alg, sha384.as_ptr(), core::ptr::null(), 0).into(),
            0,
        )?;
        c.eq(
            BCryptHash(alg, core::ptr::null(), 0, b"abc".as_ptr(), 3, digest.as_mut_ptr(), 48)
                .into(),
            0,
        )?;
        c.digest(&digest[..48], SHA384_ABC)?;
        BCryptCloseAlgorithmProvider(alg, 0);
        let one_shot = |c: &mut Checks, alg: usize, len: usize, want: &str| -> Result<(), u32> {
            let mut d = [0u8; 64];
            let r = BCryptHash(
                pseudo(alg),
                core::ptr::null(),
                0,
                b"abc".as_ptr(),
                3,
                d.as_mut_ptr(),
                len as u32,
            );
            c.eq(r.into(), 0)?;
            c.digest(&d[..len], want)
        };
        one_shot(c, SHA1, 20, SHA1_ABC)?;
        one_shot(c, MD5, 16, MD5_ABC)?;
        one_shot(c, SHA512, 64, SHA512_ABC)?;

        // HMAC: a provider with the HMAC flag and a secret; a pseudo-handle.
        c.eq(
            BCryptOpenAlgorithmProvider(
                &mut alg,
                sha256.as_ptr(),
                core::ptr::null(),
                BCRYPT_ALG_HANDLE_HMAC_FLAG,
            )
            .into(),
            0,
        )?;
        c.eq(BCryptCreateHash(alg, &mut h, null.cast(), 0, b"Jefe".as_ptr(), 4, 0).into(), 0)?;
        c.eq(BCryptHashData(h, JEFE_DATA.as_ptr(), JEFE_DATA.len() as u32, 0).into(), 0)?;
        c.eq(BCryptFinishHash(h, digest.as_mut_ptr(), 32, 0).into(), 0)?;
        c.digest(&digest[..32], HMAC_SHA256_JEFE)?;
        BCryptDestroyHash(h);
        BCryptCloseAlgorithmProvider(alg, 0);
        let r = BCryptHash(
            pseudo(HMAC_SHA1),
            b"Jefe".as_ptr(),
            4,
            JEFE_DATA.as_ptr(),
            JEFE_DATA.len() as u32,
            digest.as_mut_ptr(),
            20,
        );
        c.eq(r.into(), 0)?;
        c.digest(&digest[..20], HMAC_SHA1_JEFE)?;
    }
    Ok(())
}

const STATUS_INVALID_SIGNATURE: u64 = 0xc000_a000;
const BCRYPT_PAD_PKCS1: u32 = 2;
const BCRYPT_PAD_OAEP: u32 = 4;
const BCRYPT_PAD_PSS: u32 = 8;

/// BCRYPT_PKCS1_PADDING_INFO, BCRYPT_PSS_PADDING_INFO,
/// BCRYPT_OAEP_PADDING_INFO.
#[repr(C)]
struct Pkcs1Info {
    alg: *const u16,
}
#[repr(C)]
struct PssInfo {
    alg: *const u16,
    salt: u32,
}
#[repr(C)]
struct OaepInfo {
    alg: *const u16,
    label: *const u8,
    label_len: u32,
}

/// Imports key blob `data` of type `blob`.
fn import_pair(c: &mut Checks, alg: Handle, blob: &str, data: &[u8]) -> Result<Handle, u32> {
    let mut key = core::ptr::null_mut();
    let name = w(blob);
    // SAFETY: our blob and outputs.
    let r = unsafe {
        BCryptImportKeyPair(
            alg,
            core::ptr::null_mut(),
            name.as_ptr(),
            &mut key,
            data.as_ptr(),
            data.len() as u32,
            0,
        )
    };
    c.eq(r.into(), 0)?;
    Ok(key)
}

/// Exports `key` as `blob` and compares it with `want`.
fn export_is(c: &mut Checks, key: Handle, blob: &str, want: &[u8]) -> Result<(), u32> {
    let mut buf = [0u8; 1024];
    let mut n = 0;
    let name = w(blob);
    // SAFETY: our buffer and outputs.
    let r = unsafe {
        BCryptExportKey(
            key,
            core::ptr::null_mut(),
            name.as_ptr(),
            buf.as_mut_ptr(),
            1024,
            &mut n,
            0,
        )
    };
    c.eq(r.into(), 0)?;
    c.eq(n.into(), want.len() as u64)?;
    c.ok(&buf[..n as usize] == want, n.into())
}

fn rsa(c: &mut Checks) -> Result<(), u32> {
    use rsa_vectors::*;
    let alg = open(c, "RSA")?;
    let (hash, _) = hex_bytes(SHA256_ABC);
    let sha256 = w("SHA256");
    let sha1 = w("SHA1");
    let pkcs1 = Pkcs1Info { alg: sha256.as_ptr() };
    let pkcs1_info = (&pkcs1 as *const Pkcs1Info).cast::<c_void>();
    // SAFETY: bcrypt calls with buffers and handles of ours.
    unsafe {
        let full = import_pair(c, alg, "RSAFULLPRIVATEBLOB", RSA_FULL)?;
        let (mut v, mut got) = (0u32, 0u32);
        let length = w("KeyLength");
        let p = (&mut v as *mut u32).cast();
        c.eq(BCryptGetProperty(full, length.as_ptr(), p, 4, &mut got, 0).into(), 0)?;
        c.eq(v.into(), 1024)?;

        // PKCS#1 v1.5: OpenSSL's signature.
        let mut sig = [0u8; 128];
        let mut n = 0;
        let r = BCryptSignHash(
            full,
            pkcs1_info,
            hash.as_ptr(),
            32,
            core::ptr::null_mut(),
            0,
            &mut n,
            BCRYPT_PAD_PKCS1,
        );
        c.ok(r == 0 && n == 128, n.into())?;
        let r = BCryptSignHash(
            full,
            pkcs1_info,
            hash.as_ptr(),
            32,
            sig.as_mut_ptr(),
            128,
            &mut n,
            BCRYPT_PAD_PKCS1,
        );
        c.eq(r.into(), 0)?;
        c.ok(sig[..] == *RSA_SIG_ABC_SHA256, sig[0].into())?;
        let public = import_pair(c, alg, "RSAPUBLICBLOB", RSA_PUBLIC)?;
        let r = BCryptVerifySignature(
            public,
            pkcs1_info,
            hash.as_ptr(),
            32,
            sig.as_ptr(),
            128,
            BCRYPT_PAD_PKCS1,
        );
        c.eq(r.into(), 0)?;
        sig[5] ^= 1;
        let r = BCryptVerifySignature(
            public,
            pkcs1_info,
            hash.as_ptr(),
            32,
            sig.as_ptr(),
            128,
            BCRYPT_PAD_PKCS1,
        );
        c.eq(r.into(), STATUS_INVALID_SIGNATURE)?;

        // The same signature from the short private blob.
        let private = import_pair(c, alg, "RSAPRIVATEBLOB", RSA_PRIVATE)?;
        let r = BCryptSignHash(
            private,
            pkcs1_info,
            hash.as_ptr(),
            32,
            sig.as_mut_ptr(),
            128,
            &mut n,
            BCRYPT_PAD_PKCS1,
        );
        c.eq(r.into(), 0)?;
        c.ok(sig[..] == *RSA_SIG_ABC_SHA256, sig[0].into())?;

        // PSS (random salt): verified with the public key.
        let pss = PssInfo { alg: sha256.as_ptr(), salt: 32 };
        let pss_info = (&pss as *const PssInfo).cast::<c_void>();
        let r = BCryptSignHash(
            full,
            pss_info,
            hash.as_ptr(),
            32,
            sig.as_mut_ptr(),
            128,
            &mut n,
            BCRYPT_PAD_PSS,
        );
        c.ok(r == 0 && n == 128, r.into())?;
        let r = BCryptVerifySignature(
            public,
            pss_info,
            hash.as_ptr(),
            32,
            sig.as_ptr(),
            128,
            BCRYPT_PAD_PSS,
        );
        c.eq(r.into(), 0)?;
        let r = BCryptVerifySignature(
            public,
            pkcs1_info,
            hash.as_ptr(),
            32,
            sig.as_ptr(),
            128,
            BCRYPT_PAD_PKCS1,
        );
        c.eq(r.into(), STATUS_INVALID_SIGNATURE)?;

        // OpenSSL's ciphertexts.
        let mut plain = [0u8; 128];
        let oaep = OaepInfo { alg: sha1.as_ptr(), label: core::ptr::null(), label_len: 0 };
        let oaep_info = (&oaep as *const OaepInfo).cast::<c_void>();
        let r = BCryptDecrypt(
            full,
            RSA_OAEP_SHA1.as_ptr(),
            128,
            oaep_info,
            core::ptr::null_mut(),
            0,
            plain.as_mut_ptr(),
            128,
            &mut n,
            BCRYPT_PAD_OAEP,
        );
        c.eq(r.into(), 0)?;
        c.ok(&plain[..n as usize] == b"gala secret", n.into())?;
        let r = BCryptDecrypt(
            full,
            RSA_PKCS1.as_ptr(),
            128,
            core::ptr::null(),
            core::ptr::null_mut(),
            0,
            plain.as_mut_ptr(),
            128,
            &mut n,
            BCRYPT_PAD_PKCS1,
        );
        c.eq(r.into(), 0)?;
        c.ok(&plain[..n as usize] == b"gala secret", n.into())?;

        // Round trips through the public key.
        let mut cipher = [0u8; 128];
        for (info, flags) in [(core::ptr::null(), BCRYPT_PAD_PKCS1), (oaep_info, BCRYPT_PAD_OAEP)] {
            let r = BCryptEncrypt(
                public,
                b"round trip".as_ptr(),
                10,
                info,
                core::ptr::null_mut(),
                0,
                core::ptr::null_mut(),
                0,
                &mut n,
                flags,
            );
            c.ok(r == 0 && n == 128, n.into())?;
            let r = BCryptEncrypt(
                public,
                b"round trip".as_ptr(),
                10,
                info,
                core::ptr::null_mut(),
                0,
                cipher.as_mut_ptr(),
                128,
                &mut n,
                flags,
            );
            c.eq(r.into(), 0)?;
            let r = BCryptDecrypt(
                private,
                cipher.as_ptr(),
                128,
                info,
                core::ptr::null_mut(),
                0,
                plain.as_mut_ptr(),
                128,
                &mut n,
                flags,
            );
            c.eq(r.into(), 0)?;
            c.ok(&plain[..n as usize] == b"round trip", n.into())?;
        }

        // Exports give the imported bytes.
        export_is(c, full, "RSAPUBLICBLOB", RSA_PUBLIC)?;
        export_is(c, full, "RSAFULLPRIVATEBLOB", RSA_FULL)?;
        export_is(c, private, "RSAPRIVATEBLOB", RSA_PRIVATE)?;
        for k in [full, public, private] {
            c.eq(BCryptDestroyKey(k).into(), 0)?;
        }

        // A new key pair.
        let mut key = core::ptr::null_mut();
        c.eq(BCryptGenerateKeyPair(alg, &mut key, 1024, 0).into(), 0)?;
        c.eq(BCryptFinalizeKeyPair(key, 0).into(), 0)?;
        let mut blob = [0u8; 1024];
        let public_name = w("RSAPUBLICBLOB");
        let r = BCryptExportKey(
            key,
            core::ptr::null_mut(),
            public_name.as_ptr(),
            blob.as_mut_ptr(),
            1024,
            &mut n,
            0,
        );
        c.eq(r.into(), 0)?;
        let word = |i: usize| {
            u32::from_le_bytes([blob[4 * i], blob[4 * i + 1], blob[4 * i + 2], blob[4 * i + 3]])
        };
        c.ok(word(0) == 0x3141_5352 && word(1) == 1024 && word(3) == 128, word(1).into())?;
        let r = BCryptSignHash(
            key,
            pkcs1_info,
            hash.as_ptr(),
            32,
            sig.as_mut_ptr(),
            128,
            &mut n,
            BCRYPT_PAD_PKCS1,
        );
        c.eq(r.into(), 0)?;
        let r = BCryptVerifySignature(
            key,
            pkcs1_info,
            hash.as_ptr(),
            32,
            sig.as_ptr(),
            128,
            BCRYPT_PAD_PKCS1,
        );
        c.eq(r.into(), 0)?;
        c.eq(BCryptDestroyKey(key).into(), 0)?;
        BCryptCloseAlgorithmProvider(alg, 0);
    }
    Ok(())
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    rsa(&mut c)?;
    random(&mut c)?;
    fips(&mut c)?;
    ciphers(&mut c)?;
    old_hashes(&mut c)?;
    hashes(&mut c)
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = match run() {
        Ok(()) => {
            out(b"bcrypt_test: ok\r\n");
            0
        }
        Err(id) => id,
    };
    // SAFETY: ends the process.
    unsafe { ExitProcess(code) }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdead) }
}

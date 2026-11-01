//! bcrypt.dll (CNG) as .NET uses it, with the results real Windows gives:
//! the documented NTSTATUS codes and the published test vectors (FIPS 180
//! for SHA-1/SHA-2 of "abc", RFC 1321 for MD5, RFC 2202 and RFC 4231
//! test case 2 for HMAC-SHA1 and HMAC-SHA256):
//!
//! - BCryptGetFipsAlgorithmMode: not enforced.
//! - BCryptGenRandom with BCRYPT_USE_SYSTEM_PREFERRED_RNG, an RNG
//!   provider and the RNG pseudo-handle; STATUS_INVALID_HANDLE without one.
//! - BCryptOpenAlgorithmProvider / Close (STATUS_NOT_FOUND for an unknown
//!   algorithm), BCryptGetProperty (ObjectLength, HashDigestLength,
//!   HashBlockLength; STATUS_BUFFER_TOO_SMALL).
//! - BCryptCreateHash / HashData / FinishHash (in pieces, reusable,
//!   duplicated; STATUS_INVALID_PARAMETER for a wrong output size),
//!   BCryptDestroyHash; HMAC providers and secrets.
//! - BCryptHash (one shot) with algorithm pseudo-handles.
//!
//! Prints `bcrypt_test: ok` or exits with the number of the failing check.

#![no_std]
#![no_main]

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

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    random(&mut c)?;
    fips(&mut c)?;
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

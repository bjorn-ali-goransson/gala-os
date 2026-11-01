//! crypt32's certificate contexts, with the documented results for a
//! certificate made with OpenSSL (vectors.rs):
//! - CertCreateCertificateContext decodes it: a copy of the encoding,
//!   X509_ASN_ENCODING, and CERT_INFO's fields (version, the serial number
//!   little-endian, the signature algorithm's OID string, issuer and
//!   subject as encoded names, validity as FILETIMEs, the public key's
//!   algorithm and BIT STRING, the extensions with their OIDs and
//!   critical flags); bad encodings fail with a CRYPT_E_ASN1_* error and
//!   other encoding types with E_INVALIDARG;
//! - CertDuplicateCertificateContext gives the same context; each
//!   reference is freed with CertFreeCertificateContext;
//! - CertGetCertificateContextProperty computes CERT_SHA1_HASH_PROP_ID
//!   (and CERT_HASH_PROP_ID); CertSetCertificateContextProperty keeps a
//!   friendly name; an absent property is CRYPT_E_NOT_FOUND; sizes are
//!   queried with a null buffer (ERROR_MORE_DATA when short);
//! - CertGetNameStringW: the subject's common name
//!   (CERT_NAME_SIMPLE_DISPLAY_TYPE), the issuer's with CERT_NAME_ISSUER_FLAG,
//!   CERT_NAME_ATTR_TYPE of the organization OID;
//! - CertGetPublicKeyLength: 1024; CertFindExtension of basic constraints;
//! - CryptImportPublicKeyInfoEx2: a bcrypt RSA key that verifies the
//!   certificate's own signature (BCryptVerifySignature over the SHA-256
//!   of the TBSCertificate);
//! - base64: CryptBinaryToStringA (sizes with and without the NUL, lines of
//!   64 characters ending in CRLF unless CRYPT_STRING_NOCRLF,
//!   ERROR_MORE_DATA) and CryptStringToBinaryA (CRLFs skipped, the format
//!   found, ERROR_INVALID_DATA for what is not base64).
//!
//! Prints `cert_test: ok` or exits with the number of the failing check.

#![no_std]
#![no_main]

use core::ffi::c_void;

type Bool = i32;

type Handle = *mut c_void;

/// CRYPT_INTEGER_BLOB and friends.
#[repr(C)]
struct Blob {
    size: u32,
    data: *mut u8,
}

/// CRYPT_ALGORITHM_IDENTIFIER.
#[repr(C)]
struct Algorithm {
    oid: *const u8,
    parameters: Blob,
}

/// CRYPT_BIT_BLOB.
#[repr(C)]
struct BitBlob {
    size: u32,
    data: *mut u8,
    unused: u32,
}

/// CERT_PUBLIC_KEY_INFO.
#[repr(C)]
struct PublicKeyInfo {
    algorithm: Algorithm,
    key: BitBlob,
}

/// CERT_EXTENSION.
#[repr(C)]
struct Extension {
    oid: *const u8,
    critical: i32,
    value: Blob,
}

/// CERT_INFO.
#[repr(C)]
struct CertInfo {
    version: u32,
    serial: Blob,
    signature: Algorithm,
    issuer: Blob,
    not_before: u64,
    not_after: u64,
    subject: Blob,
    public_key: PublicKeyInfo,
    issuer_id: BitBlob,
    subject_id: BitBlob,
    extension_count: u32,
    extensions: *mut Extension,
}

/// CERT_CONTEXT.
#[repr(C)]
struct CertContext {
    encoding: u32,
    encoded: *mut u8,
    encoded_len: u32,
    info: *mut CertInfo,
    store: Handle,
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut c_void) -> i32;
    fn ExitProcess(code: u32) -> !;
    fn GetLastError() -> u32;
}

#[link(name = "crypt32", kind = "raw-dylib")]
unsafe extern "system" {
    fn CryptBinaryToStringA(
        data: *const u8,
        len: u32,
        flags: u32,
        text: *mut u8,
        chars: *mut u32,
    ) -> Bool;
    fn CryptStringToBinaryA(
        text: *const u8,
        chars: u32,
        flags: u32,
        out: *mut u8,
        len: *mut u32,
        skipped: *mut u32,
        format: *mut u32,
    ) -> Bool;
    fn CertCreateCertificateContext(encoding: u32, data: *const u8, len: u32)
    -> *const CertContext;
    fn CertDuplicateCertificateContext(cert: *const CertContext) -> *const CertContext;
    fn CertFreeCertificateContext(cert: *const CertContext) -> i32;
    fn CertGetCertificateContextProperty(
        cert: *const CertContext,
        id: u32,
        data: *mut u8,
        len: *mut u32,
    ) -> i32;
    fn CertSetCertificateContextProperty(
        cert: *const CertContext,
        id: u32,
        flags: u32,
        data: *const c_void,
    ) -> i32;
    fn CertGetNameStringW(
        cert: *const CertContext,
        kind: u32,
        flags: u32,
        kind_para: *const c_void,
        name: *mut u16,
        len: u32,
    ) -> u32;
    fn CertGetPublicKeyLength(encoding: u32, info: *const PublicKeyInfo) -> u32;
    fn CertFindExtension(
        oid: *const u8,
        count: u32,
        extensions: *const Extension,
    ) -> *const Extension;
    fn CryptImportPublicKeyInfoEx2(
        encoding: u32,
        info: *const PublicKeyInfo,
        flags: u32,
        aux: *const c_void,
        key: *mut Handle,
    ) -> i32;
}

#[link(name = "bcrypt", kind = "raw-dylib")]
unsafe extern "system" {
    fn BCryptHash(
        alg: usize,
        secret: *const u8,
        secret_len: u32,
        input: *const u8,
        input_len: u32,
        out: *mut u8,
        out_len: u32,
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
    fn BCryptDestroyKey(key: Handle) -> u32;
}

mod vectors;

#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
    fn strlen(s: *const u8) -> usize;
}

#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

#[unsafe(no_mangle)]
pub extern "C" fn __CxxFrameHandler3() -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdeac) }
}

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
        out(b"cert_test: check ");
        out(&hex(self.0));
        out(b" got ");
        out(&hex(got as u32));
        out(b"\r\n");
        Err(self.0)
    }

    fn eq(&mut self, got: u64, want: u64) -> Result<(), u32> {
        self.ok(got == want, got)
    }
}

const X509_ASN_ENCODING: u32 = 1;
const PKCS_7_ASN_ENCODING: u32 = 0x1_0000;
const E_INVALIDARG: u64 = 0x8007_0057;
const CRYPT_E_NOT_FOUND: u64 = 0x8009_2004;
const ERROR_MORE_DATA: u64 = 234;
const CERT_HASH_PROP_ID: u32 = 3;
const CERT_FRIENDLY_NAME_PROP_ID: u32 = 11;
const CERT_NAME_SIMPLE_DISPLAY_TYPE: u32 = 4;
const CERT_NAME_ATTR_TYPE: u32 = 3;
const CERT_NAME_ISSUER_FLAG: u32 = 1;

/// The NUL-terminated C string at `p`.
fn c_str<'a>(p: *const u8) -> &'a [u8] {
    let mut n = 0;
    // SAFETY: a NUL-terminated string from crypt32.
    unsafe {
        while *p.add(n) != 0 {
            n += 1;
        }
        core::slice::from_raw_parts(p, n)
    }
}

/// Whether the UTF-16 string at `p` (NUL-terminated) is `text`.
fn wide_is(p: *const u16, text: &str) -> bool {
    // SAFETY: a NUL-terminated string of ours.
    unsafe {
        text.bytes().enumerate().all(|(i, b)| *p.add(i) == u16::from(b)) && *p.add(text.len()) == 0
    }
}

fn blob<'a>(b: &Blob) -> &'a [u8] {
    // SAFETY: a blob from crypt32.
    unsafe { core::slice::from_raw_parts(b.data, b.size as usize) }
}

fn run() -> Result<(), u32> {
    use vectors::*;
    let mut c = Checks(0);
    let null: *mut c_void = core::ptr::null_mut();
    // SAFETY: crypt32 calls with buffers of ours and the contexts it
    // returns.
    unsafe {
        // Bad input.
        let bad = [0x31u8, 0x03, 0x02, 0x01, 0x00];
        let p = CertCreateCertificateContext(X509_ASN_ENCODING, bad.as_ptr(), 5);
        c.ok(p.is_null(), p as u64)?;
        c.ok(GetLastError() & 0xffff_ff00 == 0x8009_3100, GetLastError().into())?;
        let p = CertCreateCertificateContext(X509_ASN_ENCODING, CERT.as_ptr(), 100);
        c.ok(p.is_null(), p as u64)?;
        c.ok(GetLastError() & 0xffff_ff00 == 0x8009_3100, GetLastError().into())?;
        let p = CertCreateCertificateContext(0, CERT.as_ptr(), CERT.len() as u32);
        c.ok(p.is_null(), p as u64)?;
        c.eq(GetLastError().into(), E_INVALIDARG)?;

        let encoding = X509_ASN_ENCODING | PKCS_7_ASN_ENCODING;
        let cert = CertCreateCertificateContext(encoding, CERT.as_ptr(), CERT.len() as u32);
        c.ok(!cert.is_null(), GetLastError().into())?;
        let ctx = &*cert;
        c.eq(ctx.encoding.into(), encoding.into())?;
        c.ok(
            ctx.encoded_len as usize == CERT.len() && ctx.encoded.cast_const() != CERT.as_ptr(),
            ctx.encoded_len.into(),
        )?;
        c.ok(core::slice::from_raw_parts(ctx.encoded, CERT.len()) == CERT, 0)?;
        let info = &*ctx.info;
        c.eq(info.version.into(), 2)?; // CERT_V3
        c.ok(blob(&info.serial) == [0x89, 0x67, 0x45, 0x23, 0x01], info.serial.size.into())?;
        c.ok(c_str(info.signature.oid) == b"1.2.840.113549.1.1.11", 0)?;
        c.ok(
            blob(&info.signature.parameters) == [0x05, 0x00],
            info.signature.parameters.size.into(),
        )?;
        c.ok(blob(&info.issuer) == &CERT[NAME], info.issuer.size.into())?;
        c.ok(blob(&info.subject) == &CERT[NAME], info.subject.size.into())?;
        c.eq(info.not_before, NOT_BEFORE)?;
        c.eq(info.not_after, NOT_AFTER)?;
        c.ok(c_str(info.public_key.algorithm.oid) == b"1.2.840.113549.1.1.1", 0)?;
        let key = &info.public_key.key;
        c.ok(
            key.unused == 0
                && core::slice::from_raw_parts(key.data, key.size as usize) == &CERT[PUBLIC_KEY],
            key.size.into(),
        )?;
        c.eq(info.issuer_id.size.into(), 0)?;
        c.eq(info.extension_count.into(), 3)?;
        let ext = core::slice::from_raw_parts(info.extensions, 3);
        c.ok(c_str(ext[0].oid) == b"2.5.29.14" && ext[0].critical == 0, 0)?;
        c.ok(blob(&ext[0].value).len() == 22, ext[0].value.size.into())?;
        c.ok(c_str(ext[2].oid) == b"2.5.29.19" && ext[2].critical != 0, 0)?;
        let found = CertFindExtension(c"2.5.29.19".as_ptr().cast(), 3, info.extensions);
        c.ok(core::ptr::eq(found, &ext[2]), found as u64)?;
        c.ok(CertFindExtension(c"2.5.29.15".as_ptr().cast(), 3, info.extensions).is_null(), 0)?;
        c.eq(CertGetPublicKeyLength(X509_ASN_ENCODING, &info.public_key).into(), 1024)?;

        // References.
        let again = CertDuplicateCertificateContext(cert);
        c.ok(core::ptr::eq(again, cert), again as u64)?;
        c.ok(CertFreeCertificateContext(again) != 0, 0)?;

        // Properties.
        let mut hash = [0u8; 32];
        let mut n = 0u32;
        c.ok(
            CertGetCertificateContextProperty(cert, CERT_HASH_PROP_ID, null.cast(), &mut n) != 0,
            GetLastError().into(),
        )?;
        c.eq(n.into(), 20)?;
        n = 10;
        c.eq(
            CertGetCertificateContextProperty(cert, CERT_HASH_PROP_ID, hash.as_mut_ptr(), &mut n)
                as u64,
            0,
        )?;
        c.ok(GetLastError() as u64 == ERROR_MORE_DATA && n == 20, n.into())?;
        n = 32;
        c.ok(
            CertGetCertificateContextProperty(cert, CERT_HASH_PROP_ID, hash.as_mut_ptr(), &mut n)
                != 0,
            0,
        )?;
        c.ok(n == 20 && hash[..20] == CERT_SHA1, hash[0].into())?;
        n = 32;
        c.eq(
            CertGetCertificateContextProperty(
                cert,
                CERT_FRIENDLY_NAME_PROP_ID,
                hash.as_mut_ptr(),
                &mut n,
            ) as u64,
            0,
        )?;
        c.eq(GetLastError().into(), CRYPT_E_NOT_FOUND)?;
        let mut friendly: [u16; 5] = [0x67, 0x61, 0x6c, 0x61, 0];
        let value = Blob { size: 10, data: friendly.as_mut_ptr().cast() };
        c.ok(
            CertSetCertificateContextProperty(
                cert,
                CERT_FRIENDLY_NAME_PROP_ID,
                0,
                (&value as *const Blob).cast(),
            ) != 0,
            GetLastError().into(),
        )?;
        let mut name = [0u16; 32];
        n = 64;
        c.ok(
            CertGetCertificateContextProperty(
                cert,
                CERT_FRIENDLY_NAME_PROP_ID,
                name.as_mut_ptr().cast(),
                &mut n,
            ) != 0,
            0,
        )?;
        c.ok(n == 10 && wide_is(name.as_ptr(), "gala"), n.into())?;

        // Names.
        let got =
            CertGetNameStringW(cert, CERT_NAME_SIMPLE_DISPLAY_TYPE, 0, null, name.as_mut_ptr(), 32);
        c.ok(got == 10 && wide_is(name.as_ptr(), "gala test"), got.into())?;
        let got = CertGetNameStringW(cert, CERT_NAME_SIMPLE_DISPLAY_TYPE, 0, null, null.cast(), 0);
        c.eq(got.into(), 10)?;
        let got = CertGetNameStringW(
            cert,
            CERT_NAME_SIMPLE_DISPLAY_TYPE,
            CERT_NAME_ISSUER_FLAG,
            null,
            name.as_mut_ptr(),
            32,
        );
        c.ok(got == 10 && wide_is(name.as_ptr(), "gala test"), got.into())?;
        let got = CertGetNameStringW(
            cert,
            CERT_NAME_ATTR_TYPE,
            0,
            c"2.5.4.10".as_ptr().cast(),
            name.as_mut_ptr(),
            32,
        );
        c.ok(got == 5 && wide_is(name.as_ptr(), "gala"), got.into())?;

        // The public key verifies the certificate's own signature.
        let mut key = null;
        c.ok(
            CryptImportPublicKeyInfoEx2(X509_ASN_ENCODING, &info.public_key, 0, null, &mut key)
                != 0,
            GetLastError().into(),
        )?;
        let tbs = &CERT[TBS];
        let mut digest = [0u8; 32];
        c.eq(
            BCryptHash(
                0x41,
                core::ptr::null(),
                0,
                tbs.as_ptr(),
                tbs.len() as u32,
                digest.as_mut_ptr(),
                32,
            )
            .into(),
            0,
        )?; // SHA256 pseudo-handle
        let signature = &CERT[CERT.len() - 128..];
        let sha256: [u16; 7] = [0x53, 0x48, 0x41, 0x32, 0x35, 0x36, 0];
        let padding = sha256.as_ptr();
        let r = BCryptVerifySignature(
            key,
            (&padding as *const *const u16).cast(),
            digest.as_ptr(),
            32,
            signature.as_ptr(),
            128,
            2,
        );
        c.eq(r.into(), 0)?;
        c.eq(BCryptDestroyKey(key).into(), 0)?;

        c.ok(CertFreeCertificateContext(cert) != 0, 0)?;

        // Base64 (CryptBinaryToStringA, CryptStringToBinaryA).
        const BASE64: u32 = 1;
        const NOCRLF: u32 = 0x4000_0000;
        const STRICT: u32 = 0x2000_0000;
        let data = b"hello world!!";
        let want = b"aGVsbG8gd29ybGQhIQ==";
        let mut n = 0u32;
        let null = core::ptr::null_mut();
        // The size asked for counts the NUL; the length written does not.
        c.ok(CryptBinaryToStringA(data.as_ptr(), 13, BASE64 | NOCRLF, null, &mut n) != 0, 0)?;
        c.eq(n.into(), 21)?;
        let mut text = [0u8; 128];
        c.ok(
            CryptBinaryToStringA(data.as_ptr(), 13, BASE64 | NOCRLF, text.as_mut_ptr(), &mut n)
                != 0,
            GetLastError().into(),
        )?;
        c.eq(n.into(), 20)?;
        c.ok(&text[..21] == b"aGVsbG8gd29ybGQhIQ==\0", text[0].into())?;
        // Without NOCRLF every line (64 characters at most) ends in CRLF.
        let sixty = [0xffu8; 60];
        n = 128;
        c.ok(
            CryptBinaryToStringA(sixty.as_ptr(), 60, BASE64, text.as_mut_ptr(), &mut n) != 0,
            GetLastError().into(),
        )?;
        c.eq(n.into(), 84)?;
        c.ok(&text[62..68] == b"//\r\n//" && &text[80..85] == b"//\r\n\0", text[64].into())?;
        // Too small a buffer: ERROR_MORE_DATA.
        n = 10;
        c.eq(
            CryptBinaryToStringA(data.as_ptr(), 13, BASE64 | NOCRLF, text.as_mut_ptr(), &mut n)
                as u64,
            0,
        )?;
        c.eq(GetLastError().into(), 234)?;

        let (mut skipped, mut format) = (9u32, 9u32);
        c.ok(
            CryptStringToBinaryA(
                want.as_ptr(),
                20,
                BASE64 | STRICT,
                null,
                &mut n,
                &mut skipped,
                &mut format,
            ) != 0,
            GetLastError().into(),
        )?;
        c.eq(n.into(), 13)?;
        let mut back = [0u8; 32];
        // NUL-terminated (length 0); CRLFs are skipped.
        c.ok(
            CryptStringToBinaryA(
                c"aGVsbG8gd29y\r\nbGQhIQ==\r\n".as_ptr().cast(),
                0,
                BASE64,
                back.as_mut_ptr(),
                &mut n,
                &mut skipped,
                &mut format,
            ) != 0,
            GetLastError().into(),
        )?;
        c.eq(n.into(), 13)?;
        c.ok(&back[..13] == data && skipped == 0 && format == BASE64, format.into())?;
        // Not base64: ERROR_INVALID_DATA.
        n = 32;
        let bad = b"aGV$bG8=";
        let r = CryptStringToBinaryA(
            bad.as_ptr(),
            8,
            BASE64,
            back.as_mut_ptr(),
            &mut n,
            null.cast(),
            null.cast(),
        );
        c.eq(r as u64, 0)?;
        c.eq(GetLastError().into(), 13)?;
    }
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = match run() {
        Ok(()) => {
            out(b"cert_test: ok\r\n");
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

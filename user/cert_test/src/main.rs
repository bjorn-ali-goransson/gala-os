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
//! - What libcurl, GnuTLS, the JDK and SQL Server LocalDB import (ROADMAP
//!   AG; documented behaviour, no Windows run yet): CertGetNameStringA
//!   (the W form's name and size in bytes with the NUL; a short buffer
//!   gets a truncated, NUL-terminated name); CryptMemAlloc/Realloc/Free
//!   (contents kept); CryptProtectMemory/CryptUnprotectMemory (in place,
//!   the data changed and restored, for each scope;
//!   ERROR_INVALID_PARAMETER for a size that is not a multiple of
//!   CRYPTPROTECTMEMORY_BLOCK_SIZE, 16); PFXIsPFXBlob (a PFX's outer
//!   SEQUENCE { INTEGER 3, ContentInfo }, not another version or a
//!   certificate).
//! - Certificate stores (documented behaviour, no Windows run yet):
//!   CertOpenStore(CERT_STORE_PROV_MEMORY) and the "ROOT" system store
//!   (the machine's trusted roots: the schannel test CA found in it by
//!   subject, opened with CertOpenSystemStoreW and with
//!   CertOpenStore(CERT_STORE_PROV_SYSTEM_W, LOCAL_MACHINE));
//!   CertAddEncodedCertificateToStore and
//!   CertAddCertificateContextToStore with CERT_STORE_ADD_NEW (a second
//!   copy is CRYPT_E_EXISTS), _USE_EXISTING (the copy in the store) and
//!   _ALWAYS (a duplicate); the added contexts belong to the store
//!   (hCertStore); CertEnumCertificatesInStore (the end is
//!   CRYPT_E_NOT_FOUND); CertFindCertificateInStore by
//!   CERT_FIND_SHA1_HASH, CERT_FIND_SUBJECT_STR_W (a substring, case
//!   ignored), CERT_FIND_EXISTING and CERT_FIND_ANY;
//!   CertDeleteCertificateFromStore; CertDuplicateStore and
//!   CertCloseStore; CertEnumCRLsInStore of a store without CRLs.
//! - Collection stores (CERT_STORE_PROV_COLLECTION, as Chromium builds its
//!   roots; nested, as its issuer lookup is: a collection's members'
//!   members' certificates are its too): CertAddStoreToCollection adds a
//!   store whose certificates the
//!   collection then enumerates and finds (the contexts are the member's:
//!   hCertStore), in priority order; a certificate added to the collection
//!   goes to the member added with CERT_PHYSICAL_STORE_ADD_ENABLE_FLAG;
//!   CertRemoveStoreFromCollection takes a member out.
//! - Usage, names, time, signatures (what Chromium imports): with
//!   EKU_CERT (key usage digitalSignature | keyEncipherment, EKU
//!   serverAuth and clientAuth) CertGetIntendedKeyUsage gives the bits
//!   0xa0 and CertGetEnhancedKeyUsage the two OIDs (size first); with CERT
//!   (neither extension) the first is FALSE with last error 0 and the
//!   second an empty list with CRYPT_E_NOT_FOUND (every usage); with
//!   CERT_FIND_PROP_ONLY_ENHKEY_USAGE_FLAG and no usage property set, or
//!   CERT_FIND_EXT_ONLY_ENHKEY_USAGE_FLAG and no extension, it fails with
//!   CRYPT_E_NOT_FOUND (Python's enum_certificates: trusted for every
//!   purpose);
//!   CertCompareCertificateName of a name with itself and with another;
//!   CertVerifyTimeValidity before, inside and after the validity;
//!   CryptVerifyCertificateSignatureEx of each self-signed certificate by
//!   itself (TRUE) and of CERT by EKU_CERT's key (NTE_BAD_SIGNATURE);
//!   CertControlStore(CERT_STORE_CTRL_RESYNC) on a memory store;
//!   CertFindChainInStore finds no chain in a store without private keys;
//!   CryptAcquireCertificatePrivateKey of a certificate without a key is
//!   CRYPT_E_NO_KEY_PROPERTY.
//! - CryptQueryObject on a blob: the DER certificate (content
//!   CERT_QUERY_CONTENT_CERT, format BINARY) and its PEM text (format
//!   BASE64_ENCODED), with its context and a store holding it; bytes that
//!   are neither are CRYPT_E_NO_MATCH.
//! - CryptDecodeObjectEx of a subjectAltName (X509_ALTERNATE_NAME and
//!   szOID_SUBJECT_ALT_NAME2): a DNS name and an IP address into
//!   CERT_ALT_NAME_INFO, allocated (CRYPT_DECODE_ALLOC_FLAG, LocalFree) or
//!   into the caller's buffer after a size query; ERROR_MORE_DATA when
//!   the buffer is short.
//! - The chain engine and its policies (chain.rs: CertGetCertificateChain,
//!   CertVerifyCertificateChainPolicy, engines with an exclusive root,
//!   revocation from CRLs).
//! - CRLs (crl.rs: CRL contexts, CRL stores, CertFindCertificateInCRL,
//!   CertVerifyCRLRevocation, CertVerifyRevocation).
//! - PFXImportCertStore (pfx.rs: PKCS #12 files from OpenSSL, modern and
//!   legacy algorithms, the key kept or not).
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
    fn SetLastError(code: u32);
    fn LocalFree(p: *mut c_void) -> *mut c_void;
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
    fn CertGetNameStringA(
        cert: *const CertContext,
        kind: u32,
        flags: u32,
        kind_para: *const c_void,
        name: *mut u8,
        len: u32,
    ) -> u32;
    fn CryptMemAlloc(size: u32) -> *mut u8;
    fn CryptMemRealloc(p: *mut u8, size: u32) -> *mut u8;
    fn CryptMemFree(p: *mut u8);
    fn CryptProtectMemory(data: *mut u8, size: u32, flags: u32) -> Bool;
    fn CryptUnprotectMemory(data: *mut u8, size: u32, flags: u32) -> Bool;
    fn PFXIsPFXBlob(pfx: *const Blob) -> Bool;
    fn CertOpenStore(
        provider: *const u8,
        encoding: u32,
        prov: usize,
        flags: u32,
        para: *const c_void,
    ) -> Handle;
    fn CertOpenSystemStoreW(prov: usize, name: *const u16) -> Handle;
    fn CertCloseStore(store: Handle, flags: u32) -> Bool;
    fn CertDuplicateStore(store: Handle) -> Handle;
    fn CertGetIntendedKeyUsage(
        encoding: u32,
        info: *const CertInfo,
        usage: *mut u8,
        n: u32,
    ) -> Bool;
    fn CertGetEnhancedKeyUsage(
        cert: *const CertContext,
        flags: u32,
        usage: *mut u8,
        size: *mut u32,
    ) -> Bool;
    fn CertCompareCertificateName(encoding: u32, a: *const Blob, b: *const Blob) -> Bool;
    fn CertVerifyTimeValidity(time: *const u64, info: *const CertInfo) -> i32;
    fn CryptVerifyCertificateSignatureEx(
        prov: usize,
        encoding: u32,
        subject_type: u32,
        subject: *const c_void,
        issuer_type: u32,
        issuer: *const c_void,
        flags: u32,
        extra: *const c_void,
    ) -> Bool;
    fn CertControlStore(store: Handle, flags: u32, ctrl: u32, para: *const c_void) -> Bool;
    fn CertFindChainInStore(
        store: Handle,
        encoding: u32,
        flags: u32,
        kind: u32,
        para: *const c_void,
        prev: *const c_void,
    ) -> *const c_void;
    fn CryptAcquireCertificatePrivateKey(
        cert: *const CertContext,
        flags: u32,
        reserved: *const c_void,
        key: *mut usize,
        spec: *mut u32,
        free: *mut Bool,
    ) -> Bool;
    fn CertAddStoreToCollection(
        collection: Handle,
        sibling: Handle,
        flags: u32,
        priority: u32,
    ) -> Bool;
    fn CertRemoveStoreFromCollection(collection: Handle, sibling: Handle);
    fn CertAddEncodedCertificateToStore(
        store: Handle,
        encoding: u32,
        data: *const u8,
        len: u32,
        disposition: u32,
        out: *mut *const CertContext,
    ) -> Bool;
    fn CertAddCertificateContextToStore(
        store: Handle,
        cert: *const CertContext,
        disposition: u32,
        out: *mut *const CertContext,
    ) -> Bool;
    fn CertEnumCertificatesInStore(store: Handle, prev: *const CertContext) -> *const CertContext;
    fn CertFindCertificateInStore(
        store: Handle,
        encoding: u32,
        flags: u32,
        kind: u32,
        para: *const c_void,
        prev: *const CertContext,
    ) -> *const CertContext;
    fn CertDeleteCertificateFromStore(cert: *const CertContext) -> Bool;
    fn CertEnumCRLsInStore(store: Handle, prev: *const crl::CrlContext) -> *const crl::CrlContext;
    fn CryptQueryObject(
        object_type: u32,
        object: *const c_void,
        content_flags: u32,
        format_flags: u32,
        flags: u32,
        encoding: *mut u32,
        content: *mut u32,
        format: *mut u32,
        store: *mut Handle,
        msg: *mut Handle,
        context: *mut *const c_void,
    ) -> Bool;
    fn CryptDecodeObjectEx(
        encoding: u32,
        struct_type: *const u8,
        data: *const u8,
        len: u32,
        flags: u32,
        para: *const c_void,
        out: *mut c_void,
        out_len: *mut u32,
    ) -> Bool;
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

mod chain;
mod crl;
mod pfx;
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
    #[track_caller]
    fn ok(&mut self, cond: bool, got: u64) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        out(b"cert_test: check ");
        out(&hex(self.0));
        out(b" got ");
        out(&hex(got as u32));
        check_line(core::panic::Location::caller().line());
        out(b"\r\n");
        Err(self.0)
    }

    #[track_caller]
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
        // Only the certificate encoding (the low word) is kept (Windows 11
        // 25H2: 1 for X509_ASN_ENCODING | PKCS_7_ASN_ENCODING).
        c.eq(ctx.encoding.into(), X509_ASN_ENCODING.into())?;
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
        // The ANSI form.
        let mut narrow = [0u8; 32];
        let got = CertGetNameStringA(
            cert,
            CERT_NAME_SIMPLE_DISPLAY_TYPE,
            0,
            null,
            narrow.as_mut_ptr(),
            32,
        );
        c.ok(got == 10 && &narrow[..10] == b"gala test\0", got.into())?;
        let got = CertGetNameStringA(cert, CERT_NAME_SIMPLE_DISPLAY_TYPE, 0, null, null.cast(), 0);
        c.eq(got.into(), 10)?;
        let got = CertGetNameStringA(
            cert,
            CERT_NAME_SIMPLE_DISPLAY_TYPE,
            0,
            null,
            narrow.as_mut_ptr(),
            5,
        );
        c.ok(got == 5 && &narrow[..5] == b"gala\0", got.into())?;

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

        // CryptMemAlloc/Realloc/Free.
        let p = CryptMemAlloc(16);
        c.ok(!p.is_null(), 0)?;
        core::ptr::copy_nonoverlapping(b"0123456789abcdef".as_ptr(), p, 16);
        let p = CryptMemRealloc(p, 4096);
        c.ok(!p.is_null() && core::slice::from_raw_parts(p, 16) == b"0123456789abcdef", 0)?;
        CryptMemFree(p);
        CryptMemFree(null.cast());

        // CryptProtectMemory / CryptUnprotectMemory, each scope.
        let secret = *b"a secret of thirty-two bytes ..!";
        for flags in [0u32, 1, 2] {
            let mut buf = secret;
            c.ok(CryptProtectMemory(buf.as_mut_ptr(), 32, flags) != 0, flags.into())?;
            c.ok(buf != secret, flags.into())?;
            c.ok(CryptUnprotectMemory(buf.as_mut_ptr(), 32, flags) != 0, flags.into())?;
            c.ok(buf == secret, flags.into())?;
        }
        let mut odd = [0u8; 20];
        c.eq(CryptProtectMemory(odd.as_mut_ptr(), 20, 0) as u64, 0)?;
        c.eq(GetLastError().into(), 87)?;

        // PFXIsPFXBlob: SEQUENCE { INTEGER 3, SEQUENCE { OID data, [0] } }.
        let pfx = [
            0x30u8, 0x16, 0x02, 0x01, 0x03, 0x30, 0x11, 0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7,
            0x0d, 0x01, 0x07, 0x01, 0xa0, 0x04, 0x04, 0x02, 0x30, 0x00,
        ];
        let b = Blob { size: pfx.len() as u32, data: pfx.as_ptr().cast_mut() };
        c.eq(PFXIsPFXBlob(&b) as u64, 1)?;
        let mut v1 = pfx;
        v1[4] = 1;
        let b = Blob { size: v1.len() as u32, data: v1.as_mut_ptr() };
        c.eq(PFXIsPFXBlob(&b) as u64, 0)?;
        let b = Blob { size: CERT.len() as u32, data: CERT.as_ptr().cast_mut() };
        c.eq(PFXIsPFXBlob(&b) as u64, 0)?;

        stores(&mut c)?;
        collections(&mut c)?;
        usage(&mut c)?;
        query(&mut c)?;
        alt_names(&mut c)?;
        chain::chains(&mut c)?;
        crl::crls(&mut c)?;
        pfx::pfx(&mut c)?;
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

const CERT_STORE_PROV_MEMORY: usize = 2;
const CERT_STORE_ADD_NEW: u32 = 1;
const CERT_STORE_ADD_USE_EXISTING: u32 = 2;
const CERT_STORE_ADD_ALWAYS: u32 = 4;
const CERT_FIND_ANY: u32 = 0;
const CERT_FIND_SHA1_HASH: u32 = 0x1_0000;
const CERT_FIND_SUBJECT_STR_W: u32 = 0x8_0007;
const CERT_FIND_EXISTING: u32 = 0xd_0000;
const CRYPT_E_EXISTS: u64 = 0x8009_2005;
const CRYPT_E_NO_MATCH: u64 = 0x8009_2009;
const SHA1_HASH_PROP_ID: u32 = 3;

/// The certificates in `store`.
///
/// # Safety
/// A store handle from crypt32.
unsafe fn count(store: Handle) -> u32 {
    let mut n = 0;
    let mut p = core::ptr::null();
    loop {
        // SAFETY: the store and the previous context, freed by the call.
        p = unsafe { CertEnumCertificatesInStore(store, p) };
        if p.is_null() {
            return n;
        }
        n += 1;
    }
}

fn wide_text(s: &str, out: &mut [u16; 32]) -> *const u16 {
    for (slot, c) in out.iter_mut().zip(s.encode_utf16()) {
        *slot = c;
    }
    out.as_ptr()
}

/// Usage, names, time and signatures (see the module comment).
fn usage(c: &mut Checks) -> Result<(), u32> {
    use vectors::*;
    const CRYPT_E_NO_KEY_PROPERTY: u32 = 0x8009_200b;
    const NTE_BAD_SIGNATURE: u32 = 0x8009_0006;
    const SUBJECT_CERT: u32 = 2;
    const ISSUER_CERT: u32 = 2;
    let null: *mut c_void = core::ptr::null_mut();
    // SAFETY: crypt32 calls with our buffers and the contexts it returns.
    unsafe {
        let plain =
            CertCreateCertificateContext(X509_ASN_ENCODING, CERT.as_ptr(), CERT.len() as u32);
        let eku = CertCreateCertificateContext(
            X509_ASN_ENCODING,
            EKU_CERT.as_ptr(),
            EKU_CERT.len() as u32,
        );
        c.ok(!plain.is_null() && !eku.is_null(), GetLastError().into())?;
        let mut bits = [0xffu8; 2];
        c.ok(
            CertGetIntendedKeyUsage(X509_ASN_ENCODING, (*eku).info, bits.as_mut_ptr(), 2) != 0,
            0,
        )?;
        c.ok(bits == [0xa0, 0], u32::from(bits[0]).into())?;
        SetLastError(5);
        c.eq(
            CertGetIntendedKeyUsage(X509_ASN_ENCODING, (*plain).info, bits.as_mut_ptr(), 2) as u64,
            0,
        )?;
        c.eq(GetLastError().into(), 0)?;
        let mut size = 0u32;
        c.ok(CertGetEnhancedKeyUsage(eku, 0, core::ptr::null_mut(), &mut size) != 0, 0)?;
        c.ok(size >= 16, size.into())?;
        let mut buf = [0u64; 32];
        c.ok((size as usize) <= core::mem::size_of_val(&buf), size.into())?;
        c.ok(CertGetEnhancedKeyUsage(eku, 0, buf.as_mut_ptr().cast(), &mut size) != 0, 0)?;
        // CERT_ENHKEY_USAGE { cUsageIdentifier, rgpszUsageIdentifier }.
        c.eq(buf[0] as u32 as u64, 2)?;
        let ids = buf[1] as *const *const u8;
        c.ok(c_str(*ids) == b"1.3.6.1.5.5.7.3.1" && c_str(*ids.add(1)) == b"1.3.6.1.5.5.7.3.2", 0)?;
        size = core::mem::size_of_val(&buf) as u32;
        c.ok(CertGetEnhancedKeyUsage(plain, 0, buf.as_mut_ptr().cast(), &mut size) != 0, 0)?;
        c.eq(buf[0] as u32 as u64, 0)?;
        c.eq(GetLastError().into(), CRYPT_E_NOT_FOUND)?;
        // Asking for the property only (CERT_FIND_PROP_ONLY_ENHKEY_USAGE_FLAG,
        // Python's enum_certificates): none set fails with
        // CRYPT_E_NOT_FOUND, which callers read as "every usage".
        size = core::mem::size_of_val(&buf) as u32;
        c.eq(CertGetEnhancedKeyUsage(plain, 4, buf.as_mut_ptr().cast(), &mut size) as u64, 0)?;
        c.eq(GetLastError().into(), CRYPT_E_NOT_FOUND)?;
        let mut size = 0u32;
        c.eq(CertGetEnhancedKeyUsage(eku, 4, core::ptr::null_mut(), &mut size) as u64, 0)?;
        c.eq(GetLastError().into(), CRYPT_E_NOT_FOUND)?;
        // The extension alone (CERT_FIND_EXT_ONLY_ENHKEY_USAGE_FLAG): none
        // fails the same way; EKU_CERT's two OIDs.
        size = core::mem::size_of_val(&buf) as u32;
        c.eq(CertGetEnhancedKeyUsage(plain, 2, buf.as_mut_ptr().cast(), &mut size) as u64, 0)?;
        c.eq(GetLastError().into(), CRYPT_E_NOT_FOUND)?;
        size = core::mem::size_of_val(&buf) as u32;
        c.ok(CertGetEnhancedKeyUsage(eku, 2, buf.as_mut_ptr().cast(), &mut size) != 0, 0)?;
        c.eq(buf[0] as u32 as u64, 2)?;

        let info = &*(*plain).info;
        let other = &*(*eku).info;
        c.ok(CertCompareCertificateName(X509_ASN_ENCODING, &info.subject, &info.issuer) != 0, 0)?;
        c.eq(
            CertCompareCertificateName(X509_ASN_ENCODING, &info.subject, &other.subject) as u64,
            0,
        )?;
        let (before, inside, after) = (NOT_BEFORE - 1, NOT_BEFORE + 1, NOT_AFTER + 1);
        c.eq(CertVerifyTimeValidity(&before, info) as u32 as u64, u64::from(u32::MAX))?; // -1
        c.eq(CertVerifyTimeValidity(&inside, info) as u64, 0)?;
        c.eq(CertVerifyTimeValidity(&after, info) as u64, 1)?;

        for cert in [plain, eku] {
            let ok = CryptVerifyCertificateSignatureEx(
                0,
                X509_ASN_ENCODING,
                SUBJECT_CERT,
                cert.cast(),
                ISSUER_CERT,
                cert.cast(),
                0,
                null,
            );
            c.ok(ok != 0, GetLastError().into())?;
        }
        let ok = CryptVerifyCertificateSignatureEx(
            0,
            X509_ASN_ENCODING,
            SUBJECT_CERT,
            plain.cast(),
            ISSUER_CERT,
            eku.cast(),
            0,
            null,
        );
        c.eq(ok as u64, 0)?;
        c.eq(GetLastError().into(), NTE_BAD_SIGNATURE.into())?;

        let store = CertOpenStore(CERT_STORE_PROV_MEMORY as *const u8, 0, 0, 0, null);
        c.ok(CertControlStore(store, 0, 1, null) != 0, GetLastError().into())?; // RESYNC
        c.ok(
            CertAddCertificateContextToStore(
                store,
                plain,
                CERT_STORE_ADD_NEW,
                core::ptr::null_mut(),
            ) != 0,
            0,
        )?;
        let chain = CertFindChainInStore(store, X509_ASN_ENCODING, 0, 1, null, null);
        c.ok(chain.is_null(), 0)?;
        c.eq(GetLastError().into(), CRYPT_E_NOT_FOUND)?;
        CertCloseStore(store, 0);
        let (mut key, mut spec, mut free) = (0usize, 0u32, 0);
        let got = CryptAcquireCertificatePrivateKey(plain, 0, null, &mut key, &mut spec, &mut free);
        c.eq(got as u64, 0)?;
        c.eq(GetLastError().into(), CRYPT_E_NO_KEY_PROPERTY.into())?;
        CertFreeCertificateContext(plain);
        CertFreeCertificateContext(eku);
    }
    Ok(())
}

/// Collection stores (see the module comment).
fn collections(c: &mut Checks) -> Result<(), u32> {
    use vectors::*;
    const CERT_STORE_PROV_COLLECTION: usize = 11;
    const CERT_PHYSICAL_STORE_ADD_ENABLE_FLAG: u32 = 1;
    let null: *mut c_void = core::ptr::null_mut();
    // SAFETY: crypt32 calls with our buffers and the handles it returns.
    unsafe {
        let memory = CERT_STORE_PROV_MEMORY as *const u8;
        let (a, b) = (CertOpenStore(memory, 0, 0, 0, null), CertOpenStore(memory, 0, 0, 0, null));
        let all = CertOpenStore(CERT_STORE_PROV_COLLECTION as *const u8, 0, 0, 0, null);
        c.ok(!a.is_null() && !b.is_null() && !all.is_null(), 0)?;
        let mut in_a: *const CertContext = core::ptr::null();
        let ok = CertAddEncodedCertificateToStore(
            a,
            X509_ASN_ENCODING,
            CERT.as_ptr(),
            CERT.len() as u32,
            CERT_STORE_ADD_NEW,
            &mut in_a,
        );
        c.ok(ok != 0, GetLastError().into())?;
        c.eq(count(all).into(), 0)?;
        c.ok(CertAddStoreToCollection(all, a, 0, 1) != 0, GetLastError().into())?;
        c.ok(CertAddStoreToCollection(all, b, CERT_PHYSICAL_STORE_ADD_ENABLE_FLAG, 0) != 0, 0)?;
        c.eq(count(all).into(), 1)?;
        let first = CertEnumCertificatesInStore(all, core::ptr::null());
        c.ok(!first.is_null() && (*first).store == a, 0)?;
        CertFreeCertificateContext(first);
        // Added to the collection: into b (the member that takes adds).
        let mut added: *const CertContext = core::ptr::null();
        let ok = CertAddEncodedCertificateToStore(
            all,
            X509_ASN_ENCODING,
            CERT.as_ptr(),
            CERT.len() as u32,
            CERT_STORE_ADD_ALWAYS,
            &mut added,
        );
        c.ok(ok != 0 && !added.is_null() && (*added).store == b, GetLastError().into())?;
        CertFreeCertificateContext(added);
        c.eq(count(b).into(), 1)?;
        c.eq(count(all).into(), 2)?;
        // Found through the collection.
        let hash = Blob { size: 20, data: CERT_SHA1.as_ptr().cast_mut() };
        let found = CertFindCertificateInStore(
            all,
            X509_ASN_ENCODING,
            0,
            CERT_FIND_SHA1_HASH,
            (&raw const hash).cast(),
            core::ptr::null(),
        );
        c.ok(!found.is_null() && (*found).store == a, 0)?;
        CertFreeCertificateContext(found);
        // A collection of collections (Chromium's issuer lookup: all
        // certificates = {roots, intermediates}, each a collection of
        // system stores): the members' members' certificates count too.
        let outer = CertOpenStore(CERT_STORE_PROV_COLLECTION as *const u8, 0, 0, 0, null);
        c.ok(CertAddStoreToCollection(outer, all, 0, 0) != 0, GetLastError().into())?;
        c.eq(count(outer).into(), 2)?;
        let found = CertFindCertificateInStore(
            outer,
            X509_ASN_ENCODING,
            0,
            CERT_FIND_SHA1_HASH,
            (&raw const hash).cast(),
            core::ptr::null(),
        );
        c.ok(!found.is_null() && (*found).store == a, 0)?;
        CertFreeCertificateContext(found);
        c.ok(CertCloseStore(outer, 0) != 0, 0)?;
        CertRemoveStoreFromCollection(all, a);
        c.eq(count(all).into(), 1)?;
        CertFreeCertificateContext(in_a);
        c.ok(CertCloseStore(all, 0) != 0, 0)?;
        // The members outlive the collection.
        c.eq(count(a).into(), 1)?;
        CertCloseStore(a, 0);
        CertCloseStore(b, 0);
    }
    Ok(())
}

fn stores(c: &mut Checks) -> Result<(), u32> {
    use vectors::*;
    let null: *mut c_void = core::ptr::null_mut();
    // SAFETY: crypt32 calls with our buffers and the handles it returns.
    unsafe {
        let store = CertOpenStore(CERT_STORE_PROV_MEMORY as *const u8, 0, 0, 0, null);
        c.ok(!store.is_null(), 0)?;
        c.eq(count(store).into(), 0)?;
        c.eq(GetLastError().into(), CRYPT_E_NOT_FOUND)?;
        let mut added: *const CertContext = core::ptr::null();
        let ok = CertAddEncodedCertificateToStore(
            store,
            X509_ASN_ENCODING,
            CERT.as_ptr(),
            CERT.len() as u32,
            CERT_STORE_ADD_NEW,
            &mut added,
        );
        c.ok(ok != 0 && !added.is_null() && (*added).store == store, 0)?;
        let r = CertAddEncodedCertificateToStore(
            store,
            X509_ASN_ENCODING,
            CERT.as_ptr(),
            CERT.len() as u32,
            CERT_STORE_ADD_NEW,
            core::ptr::null_mut(),
        );
        c.eq(r as u64, 0)?;
        c.eq(GetLastError().into(), CRYPT_E_EXISTS)?;
        let outside =
            CertCreateCertificateContext(X509_ASN_ENCODING, CERT.as_ptr(), CERT.len() as u32);
        let mut existing: *const CertContext = core::ptr::null();
        c.ok(
            CertAddCertificateContextToStore(
                store,
                outside,
                CERT_STORE_ADD_USE_EXISTING,
                &mut existing,
            ) != 0,
            0,
        )?;
        c.ok(!existing.is_null() && (*existing).store == store, 0)?;
        CertFreeCertificateContext(existing);
        c.eq(count(store).into(), 1)?;
        c.ok(
            CertAddCertificateContextToStore(
                store,
                outside,
                CERT_STORE_ADD_ALWAYS,
                core::ptr::null_mut(),
            ) != 0,
            0,
        )?;
        c.eq(count(store).into(), 2)?;
        // Find by SHA-1 hash, subject text, the existing certificate, any.
        let mut hash = [0u8; 20];
        let mut n = 20;
        c.ok(
            CertGetCertificateContextProperty(
                outside,
                SHA1_HASH_PROP_ID,
                hash.as_mut_ptr(),
                &mut n,
            ) != 0,
            0,
        )?;
        let hash_blob = Blob { size: 20, data: hash.as_mut_ptr() };
        let f = CertFindCertificateInStore(
            store,
            X509_ASN_ENCODING,
            0,
            CERT_FIND_SHA1_HASH,
            (&raw const hash_blob).cast(),
            core::ptr::null(),
        );
        c.ok(!f.is_null() && (*f).store == store, 0)?;
        let f2 = CertFindCertificateInStore(
            store,
            X509_ASN_ENCODING,
            0,
            CERT_FIND_SHA1_HASH,
            (&raw const hash_blob).cast(),
            f,
        );
        c.ok(!f2.is_null() && f2 != f, 0)?;
        let f3 = CertFindCertificateInStore(
            store,
            X509_ASN_ENCODING,
            0,
            CERT_FIND_SHA1_HASH,
            (&raw const hash_blob).cast(),
            f2,
        );
        c.ok(f3.is_null(), 0)?;
        c.eq(GetLastError().into(), CRYPT_E_NOT_FOUND)?;
        let mut text = [0u16; 32];
        let f = CertFindCertificateInStore(
            store,
            X509_ASN_ENCODING,
            0,
            CERT_FIND_SUBJECT_STR_W,
            wide_text("GALA TEST", &mut text).cast(),
            core::ptr::null(),
        );
        c.ok(!f.is_null(), 0)?;
        CertFreeCertificateContext(f);
        let f = CertFindCertificateInStore(
            store,
            X509_ASN_ENCODING,
            0,
            CERT_FIND_SUBJECT_STR_W,
            wide_text("nobody", &mut text).cast(),
            core::ptr::null(),
        );
        c.ok(f.is_null(), 0)?;
        let f = CertFindCertificateInStore(
            store,
            X509_ASN_ENCODING,
            0,
            CERT_FIND_EXISTING,
            outside.cast(),
            core::ptr::null(),
        );
        c.ok(!f.is_null(), 0)?;
        // Delete it: one is left.
        c.ok(CertDeleteCertificateFromStore(f) != 0, 0)?;
        c.eq(count(store).into(), 1)?;
        let any = CertFindCertificateInStore(
            store,
            X509_ASN_ENCODING,
            0,
            CERT_FIND_ANY,
            null,
            core::ptr::null(),
        );
        c.ok(!any.is_null(), 0)?;
        CertFreeCertificateContext(any);
        CertFreeCertificateContext(added);
        CertFreeCertificateContext(outside);
        c.ok(CertEnumCRLsInStore(store, core::ptr::null()).is_null(), 0)?;
        c.eq(GetLastError().into(), CRYPT_E_NOT_FOUND)?;
        let dup = CertDuplicateStore(store);
        c.ok(dup == store, 0)?;
        c.ok(CertCloseStore(dup, 0) != 0 && CertCloseStore(store, 0) != 0, 0)?;
        // A system store: ROOT holds the machine's trusted roots, the
        // schannel test CA among them (installed by run-all.ps1 on
        // Windows, in gala's machine root store on gala), found by
        // subject; also through CertOpenStore(CERT_STORE_PROV_SYSTEM_W,
        // CERT_SYSTEM_STORE_LOCAL_MACHINE), as Chromium opens it.
        let mut root_text = [0u16; 32];
        let root = CertOpenSystemStoreW(0, wide_text("ROOT", &mut root_text));
        c.ok(!root.is_null(), 0)?;
        let mut name = [0u16; 32];
        let subject = wide_text("gala schannel test CA", &mut name);
        let found = CertFindCertificateInStore(
            root,
            X509_ASN_ENCODING,
            0,
            CERT_FIND_SUBJECT_STR_W,
            subject.cast(),
            core::ptr::null(),
        );
        c.ok(!found.is_null(), GetLastError().into())?;
        CertFreeCertificateContext(found);
        c.ok(CertCloseStore(root, 0) != 0, 0)?;
        const CERT_STORE_PROV_SYSTEM_W: usize = 10;
        const CERT_SYSTEM_STORE_LOCAL_MACHINE: u32 = 2 << 16;
        const CERT_STORE_READONLY_FLAG: u32 = 0x8000;
        let mut root_name = [0u16; 32];
        let machine = CertOpenStore(
            CERT_STORE_PROV_SYSTEM_W as *const u8,
            0,
            0,
            CERT_SYSTEM_STORE_LOCAL_MACHINE | CERT_STORE_READONLY_FLAG,
            wide_text("ROOT", &mut root_name).cast(),
        );
        c.ok(!machine.is_null(), GetLastError().into())?;
        let found = CertFindCertificateInStore(
            machine,
            X509_ASN_ENCODING,
            0,
            CERT_FIND_SUBJECT_STR_W,
            subject.cast(),
            core::ptr::null(),
        );
        c.ok(!found.is_null(), GetLastError().into())?;
        CertFreeCertificateContext(found);
        c.ok(CertCloseStore(machine, 0) != 0, 0)?;
    }
    Ok(())
}

const CERT_QUERY_OBJECT_BLOB: u32 = 2;
const CERT_QUERY_CONTENT_FLAG_CERT: u32 = 1 << 1;
const CERT_QUERY_FORMAT_FLAG_ALL: u32 = (1 << 1) | (1 << 2) | (1 << 3);

fn query(c: &mut Checks) -> Result<(), u32> {
    use vectors::*;
    // The PEM form of CERT.
    let mut pem = [0u8; 2048];
    let mut n = 0;
    for &b in b"-----BEGIN CERTIFICATE-----\r\n" {
        pem[n] = b;
        n += 1;
    }
    let mut room = (pem.len() - n) as u32;
    // SAFETY: our buffers.
    unsafe {
        c.ok(
            CryptBinaryToStringA(
                CERT.as_ptr(),
                CERT.len() as u32,
                1,
                pem.as_mut_ptr().add(n),
                &mut room,
            ) != 0,
            0,
        )?;
    }
    n += room as usize;
    for &b in b"-----END CERTIFICATE-----\r\n" {
        pem[n] = b;
        n += 1;
    }
    for (data, len, want_format) in [(CERT.as_ptr(), CERT.len(), 1u32), (pem.as_ptr(), n, 2)] {
        let blob = Blob { size: len as u32, data: data.cast_mut() };
        let (mut encoding, mut content, mut format) = (0u32, 0u32, 0u32);
        let mut store: Handle = core::ptr::null_mut();
        let mut context: *const c_void = core::ptr::null();
        // SAFETY: our blob and outputs.
        unsafe {
            let ok = CryptQueryObject(
                CERT_QUERY_OBJECT_BLOB,
                (&raw const blob).cast(),
                CERT_QUERY_CONTENT_FLAG_CERT,
                CERT_QUERY_FORMAT_FLAG_ALL,
                0,
                &mut encoding,
                &mut content,
                &mut format,
                &mut store,
                core::ptr::null_mut(),
                &mut context,
            );
            c.ok(ok != 0, want_format.into())?;
            c.ok(
                content == 1 && format == want_format && encoding & X509_ASN_ENCODING != 0,
                format.into(),
            )?;
            let cert = context as *const CertContext;
            c.ok(!cert.is_null() && (*cert).encoded_len as usize == CERT.len(), 0)?;
            c.eq(count(store).into(), 1)?;
            CertFreeCertificateContext(cert);
            CertCloseStore(store, 0);
        }
    }
    let junk = Blob { size: 4, data: b"junk".as_ptr().cast_mut() };
    // SAFETY: our blob.
    unsafe {
        let ok = CryptQueryObject(
            CERT_QUERY_OBJECT_BLOB,
            (&raw const junk).cast(),
            CERT_QUERY_CONTENT_FLAG_CERT,
            CERT_QUERY_FORMAT_FLAG_ALL,
            0,
            core::ptr::null_mut(),
            core::ptr::null_mut(),
            core::ptr::null_mut(),
            core::ptr::null_mut(),
            core::ptr::null_mut(),
            core::ptr::null_mut(),
        );
        c.eq(ok as u64, 0)?;
        c.eq(GetLastError().into(), CRYPT_E_NO_MATCH)?;
    }
    Ok(())
}

/// CERT_ALT_NAME_ENTRY: the choice, then a pointer-sized union (a
/// CRYPT_DATA_BLOB for IP addresses).
#[repr(C)]
struct AltEntry {
    choice: u32,
    a: usize,
    b: usize,
}

#[repr(C)]
struct AltInfo {
    count: u32,
    entries: *const AltEntry,
}

const CRYPT_DECODE_ALLOC_FLAG: u32 = 0x8000;
const X509_ALTERNATE_NAME: usize = 12;

fn check_alt(c: &mut Checks, info: *const AltInfo) -> Result<(), u32> {
    // SAFETY: crypt32's decoded structure.
    unsafe {
        let info = &*info;
        c.eq(info.count.into(), 2)?;
        let e = core::slice::from_raw_parts(info.entries, 2);
        c.eq(e[0].choice.into(), 3)?; // CERT_ALT_NAME_DNS_NAME
        c.ok(wide_is(e[0].a as *const u16, "a.example"), 0)?;
        c.eq(e[1].choice.into(), 8)?; // CERT_ALT_NAME_IP_ADDRESS
        let ip = core::slice::from_raw_parts(e[1].b as *const u8, e[1].a as u32 as usize);
        c.ok(ip == [127, 0, 0, 1], 0)?;
    }
    Ok(())
}

fn alt_names(c: &mut Checks) -> Result<(), u32> {
    // SEQUENCE { [2] "a.example", [7] 127.0.0.1 }
    let der = [
        0x30u8, 0x11, 0x82, 0x09, b'a', b'.', b'e', b'x', b'a', b'm', b'p', b'l', b'e', 0x87, 0x04,
        0x7f, 0x00, 0x00, 0x01,
    ];
    let null: *const c_void = core::ptr::null();
    // SAFETY: our encoding and outputs; the allocated result freed with
    // LocalFree.
    unsafe {
        let mut p: *mut AltInfo = core::ptr::null_mut();
        let mut size = 0u32;
        let ok = CryptDecodeObjectEx(
            X509_ASN_ENCODING,
            X509_ALTERNATE_NAME as *const u8,
            der.as_ptr(),
            der.len() as u32,
            CRYPT_DECODE_ALLOC_FLAG,
            null,
            (&raw mut p).cast(),
            &mut size,
        );
        c.ok(
            ok != 0 && !p.is_null() && size as usize >= core::mem::size_of::<AltInfo>(),
            size.into(),
        )?;
        check_alt(c, p)?;
        LocalFree(p.cast());
        // The OID form, into our own buffer after a size query.
        let mut size = 0u32;
        let oid = c"2.5.29.17";
        c.ok(
            CryptDecodeObjectEx(
                X509_ASN_ENCODING,
                oid.as_ptr().cast(),
                der.as_ptr(),
                der.len() as u32,
                0,
                null,
                core::ptr::null_mut(),
                &mut size,
            ) != 0,
            0,
        )?;
        let mut buf = [0u64; 64];
        c.ok(size as usize <= 512, size.into())?;
        let mut short = size - 1;
        c.eq(
            CryptDecodeObjectEx(
                X509_ASN_ENCODING,
                oid.as_ptr().cast(),
                der.as_ptr(),
                der.len() as u32,
                0,
                null,
                buf.as_mut_ptr().cast(),
                &mut short,
            ) as u64,
            0,
        )?;
        c.eq(GetLastError().into(), ERROR_MORE_DATA)?;
        c.ok(
            CryptDecodeObjectEx(
                X509_ASN_ENCODING,
                oid.as_ptr().cast(),
                der.as_ptr(),
                der.len() as u32,
                0,
                null,
                buf.as_mut_ptr().cast(),
                &mut size,
            ) != 0,
            0,
        )?;
        check_alt(c, buf.as_ptr().cast())?;
        // Not an alternative name.
        let bad = [0x04u8, 0x01, 0x00];
        let mut p: *mut AltInfo = core::ptr::null_mut();
        c.eq(
            CryptDecodeObjectEx(
                X509_ASN_ENCODING,
                X509_ALTERNATE_NAME as *const u8,
                bad.as_ptr(),
                3,
                CRYPT_DECODE_ALLOC_FLAG,
                null,
                (&raw mut p).cast(),
                &mut size,
            ) as u64,
            0,
        )?;
    }
    Ok(())
}

/// ` (line L)`: the source line of the failing check, from
/// `#[track_caller]` on Checks' methods (so a report names the line
/// without anyone counting checks).
fn check_line(line: u32) {
    let mut b = *b" (line 0000000000)";
    let mut digits = [0u8; 10];
    let (mut n, mut len) = (line, 0);
    loop {
        digits[len] = b'0' + (n % 10) as u8;
        len += 1;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    for i in 0..len {
        b[7 + i] = digits[len - 1 - i];
    }
    b[7 + len] = b')';
    out(&b[..8 + len]);
}

//! PFXImportCertStore (PKCS #12), with the results Microsoft documents
//! for it and Wine's conformance tests (dlls/crypt32/tests/store.c
//! test_PFXImportCertStore, checked on Windows) record. The files
//! (`pfx/make.sh`, OpenSSL): one RSA 2048 certificate, CN=pfx.gala.test,
//! with its key under OpenSSL 3's defaults (PBES2: PBKDF2-HMAC-SHA256,
//! AES-256-CBC; HMAC-SHA256 MAC) and under the legacy ones (RC2-40 and
//! 3DES, HMAC-SHA1 MAC, friendly name "gala pfx"), and the certificate
//! alone with an empty password.
//!
//! - A NULL blob: ERROR_INVALID_PARAMETER; a wrong password:
//!   ERROR_INVALID_PASSWORD.
//! - The certificate alone (L""): a store of one certificate (its
//!   hCertStore), no key properties (CRYPT_E_NOT_FOUND).
//! - With the key kept (PKCS12_ALWAYS_CNG_KSP): CERT_KEY_PROV_INFO_PROP_ID
//!   names the Microsoft Software Key Storage Provider (dwProvType 0)
//!   and a container, a new one each import; no CERT_KEY_CONTEXT_PROP_ID;
//!   CryptAcquireCertificatePrivateKey(CRYPT_ACQUIRE_ONLY_NCRYPT_KEY_FLAG)
//!   opens it (CERT_NCRYPT_KEY_SPEC, the caller frees it) and it signs
//!   what the certificate's public key verifies.
//! - With PKCS12_NO_PERSIST_KEY: CERT_KEY_CONTEXT_PROP_ID holds the key
//!   (CERT_NCRYPT_KEY_SPEC), no CERT_KEY_PROV_INFO_PROP_ID; the key from
//!   CryptAcquireCertificatePrivateKey is not the caller's to free; the
//!   friendly name becomes CERT_FRIENDLY_NAME_PROP_ID.

use super::{
    BCryptDestroyKey, BCryptHash, BCryptVerifySignature, Bool, CertCloseStore, CertContext,
    CertDuplicateCertificateContext, CertEnumCertificatesInStore, CertFreeCertificateContext,
    CertGetCertificateContextProperty, CertGetNameStringW, Checks,
    CryptAcquireCertificatePrivateKey, CryptImportPublicKeyInfoEx2, GetLastError, Handle,
    SetLastError, X509_ASN_ENCODING, wide_is,
};
use core::ffi::c_void;
use core::ptr::{null, null_mut};

const MODERN: &[u8] = include_bytes!("../pfx/modern.pfx");
const LEGACY: &[u8] = include_bytes!("../pfx/legacy.pfx");
const CERT_ONLY: &[u8] = include_bytes!("../pfx/certonly.pfx");

/// CRYPT_DATA_BLOB.
#[repr(C)]
struct DataBlob {
    size: u32,
    data: *const u8,
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "crypt32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "crypt32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn PFXImportCertStore(pfx: *const DataBlob, password: *const u16, flags: u32) -> Handle;
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "ncrypt", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "ncrypt", kind = "raw-dylib"))]
unsafe extern "system" {
    fn NCryptSignHash(
        key: usize,
        padding: *const c_void,
        hash: *const u8,
        hash_len: u32,
        sig: *mut u8,
        sig_len: u32,
        result: *mut u32,
        flags: u32,
    ) -> i32;
    fn NCryptFreeObject(object: usize) -> i32;
}

const ERROR_INVALID_PARAMETER: u64 = 87;
const ERROR_INVALID_PASSWORD: u64 = 86;
const CRYPT_E_NOT_FOUND: u64 = 0x8009_2004;
const CERT_KEY_PROV_INFO_PROP_ID: u32 = 2;
const CERT_KEY_CONTEXT_PROP_ID: u32 = 5;
const CERT_FRIENDLY_NAME_PROP_ID: u32 = 11;
const PKCS12_ALWAYS_CNG_KSP: u32 = 0x200;
const PKCS12_NO_PERSIST_KEY: u32 = 0x8000;
const CRYPT_EXPORTABLE: u32 = 0x1;
const CRYPT_ACQUIRE_ONLY_NCRYPT_KEY_FLAG: u32 = 0x4_0000;
const CERT_NCRYPT_KEY_SPEC: u32 = 0xffff_ffff;
const BCRYPT_PAD_PKCS1: u32 = 2;
const CERT_NAME_SIMPLE_DISPLAY_TYPE: u32 = 4;

fn wide<const N: usize>(s: &str) -> [u16; N] {
    let mut w = [0u16; N];
    for (d, b) in w.iter_mut().zip(s.bytes()) {
        *d = b.into();
    }
    w
}

fn import(pfx: &[u8], password: *const u16, flags: u32) -> Handle {
    let blob = DataBlob { size: pfx.len() as u32, data: pfx.as_ptr() };
    // SAFETY: our blob and password.
    unsafe { PFXImportCertStore(&blob, password, flags) }
}

/// The store's only certificate (checked: one, its hCertStore, its
/// subject).
fn only_cert(c: &mut Checks, store: Handle) -> Result<*const CertContext, u32> {
    // SAFETY: documented crypt32 calls on our store.
    unsafe {
        c.ok(!store.is_null(), GetLastError().into())?;
        let cert = CertEnumCertificatesInStore(store, null());
        c.ok(!cert.is_null(), GetLastError().into())?;
        c.eq((*cert).store as u64, store as u64)?;
        let mut name = [0u16; 32];
        CertGetNameStringW(cert, CERT_NAME_SIMPLE_DISPLAY_TYPE, 0, null(), name.as_mut_ptr(), 32);
        c.ok(wide_is(name.as_ptr(), "pfx.gala.test"), u64::from(name[0]))?;
        // The enumeration frees the context it continues from.
        let next = CertEnumCertificatesInStore(store, CertDuplicateCertificateContext(cert));
        c.ok(next.is_null(), 0)?;
        Ok(cert)
    }
}

/// The property into `buf`: its size, or the error.
/// CRYPT_KEY_PROV_INFO.
#[repr(C)]
struct KeyProvInfo {
    container: *const u16,
    provider: *const u16,
    provider_type: u32,
    flags: u32,
    param_count: u32,
    params: *const c_void,
    key_spec: u32,
}

/// CERT_KEY_CONTEXT.
#[repr(C)]
struct KeyContext {
    size: u32,
    key: usize,
    key_spec: u32,
}

fn property(cert: *const CertContext, id: u32, buf: &mut [u64; 128]) -> Result<u32, u32> {
    let mut size = 1024u32;
    // SAFETY: our buffer of `size` bytes.
    let ok =
        unsafe { CertGetCertificateContextProperty(cert, id, buf.as_mut_ptr().cast(), &mut size) };
    // SAFETY: plain call.
    if ok != 0 { Ok(size) } else { Err(unsafe { GetLastError() }) }
}

/// Whether two NUL-terminated UTF-16 strings are equal.
fn same_wide(a: *const u16, b: *const u16) -> bool {
    // SAFETY: NUL-terminated strings from crypt32.
    unsafe {
        let mut i = 0;
        loop {
            if *a.add(i) != *b.add(i) {
                return false;
            }
            if *a.add(i) == 0 {
                return true;
            }
            i += 1;
        }
    }
}

/// The certificate's private key signs a SHA-256 hash (PKCS #1) that its
/// public key verifies.
fn signs(c: &mut Checks, cert: *const CertContext, key: usize) -> Result<(), u32> {
    // SAFETY: documented calls with our buffers.
    unsafe {
        let mut hash = [0u8; 32];
        let text = b"gala pfx";
        c.eq(BCryptHash(0x41, null(), 0, text.as_ptr(), 8, hash.as_mut_ptr(), 32).into(), 0)?;
        let sha256: [u16; 7] = wide("SHA256");
        let padding = sha256.as_ptr();
        let padding = (&padding as *const *const u16).cast::<c_void>();
        let mut sig = [0u8; 256];
        let mut n = 0u32;
        let r = NCryptSignHash(
            key,
            padding,
            hash.as_ptr(),
            32,
            sig.as_mut_ptr(),
            256,
            &mut n,
            BCRYPT_PAD_PKCS1,
        );
        c.ok(r == 0 && n == 256, r as u32 as u64)?;
        let mut public: Handle = null_mut();
        let info = &(*(*cert).info).public_key;
        c.ok(CryptImportPublicKeyInfoEx2(X509_ASN_ENCODING, info, 0, null(), &mut public) != 0, 0)?;
        let r = BCryptVerifySignature(
            public,
            padding,
            hash.as_ptr(),
            32,
            sig.as_ptr(),
            n,
            BCRYPT_PAD_PKCS1,
        );
        c.eq(r.into(), 0)?;
        BCryptDestroyKey(public);
    }
    Ok(())
}

/// CryptAcquireCertificatePrivateKey(CRYPT_ACQUIRE_ONLY_NCRYPT_KEY_FLAG):
/// (key, key spec, whether the caller frees it).
fn acquire(c: &mut Checks, cert: *const CertContext) -> Result<(usize, u32, Bool), u32> {
    let (mut key, mut spec, mut free) = (0usize, 0u32, 7 as Bool);
    // SAFETY: our outputs.
    let ok = unsafe {
        CryptAcquireCertificatePrivateKey(
            cert,
            CRYPT_ACQUIRE_ONLY_NCRYPT_KEY_FLAG,
            null(),
            &mut key,
            &mut spec,
            &mut free,
        )
    };
    // SAFETY: plain call.
    c.ok(ok != 0 && key != 0, unsafe { GetLastError() }.into())?;
    Ok((key, spec, free))
}

pub fn pfx(c: &mut Checks) -> Result<(), u32> {
    let mut buf = [0u64; 128];
    let not_found = Err(CRYPT_E_NOT_FOUND as u32);
    // SAFETY: documented crypt32 calls with our buffers.
    unsafe {
        SetLastError(0xdead_beef);
        c.ok(PFXImportCertStore(null(), null(), 0).is_null(), 0)?;
        c.eq(GetLastError().into(), ERROR_INVALID_PARAMETER)?;
        let wrong: [u16; 5] = wide("nope");
        SetLastError(0xdead_beef);
        c.ok(import(MODERN, wrong.as_ptr(), 0).is_null(), 0)?;
        c.eq(GetLastError().into(), ERROR_INVALID_PASSWORD)?;

        // The certificate alone, an empty password.
        let empty = [0u16; 1];
        let store = import(CERT_ONLY, empty.as_ptr(), 0);
        let cert = only_cert(c, store)?;
        c.ok(property(cert, CERT_KEY_PROV_INFO_PROP_ID, &mut buf) == not_found, 0)?;
        c.ok(property(cert, CERT_KEY_CONTEXT_PROP_ID, &mut buf) == not_found, 0)?;
        CertFreeCertificateContext(cert);
        c.ok(CertCloseStore(store, 0) != 0, 0)?;

        // The key kept in the software KSP.
        let password: [u16; 5] = wide("gala");
        let store = import(MODERN, password.as_ptr(), CRYPT_EXPORTABLE | PKCS12_ALWAYS_CNG_KSP);
        let cert = only_cert(c, store)?;
        c.ok(property(cert, CERT_KEY_CONTEXT_PROP_ID, &mut buf) == not_found, 0)?;
        let size = property(cert, CERT_KEY_PROV_INFO_PROP_ID, &mut buf);
        c.ok(size.is_ok(), size.err().unwrap_or(0).into())?;
        // CRYPT_KEY_PROV_INFO: the container, the provider, dwProvType.
        let info = buf.as_ptr().cast::<KeyProvInfo>().read();
        let container = info.container;
        c.ok(!container.is_null() && *container != 0, 0)?;
        c.ok(wide_is(info.provider, "Microsoft Software Key Storage Provider"), 0)?;
        c.eq(info.provider_type.into(), 0)?;
        let mut first = [0u16; 128];
        for (i, slot) in first.iter_mut().enumerate().take(127) {
            *slot = *container.add(i);
            if *slot == 0 {
                break;
            }
        }
        let (key, spec, free) = acquire(c, cert)?;
        c.ok(spec == CERT_NCRYPT_KEY_SPEC && free != 0, spec.into())?;
        signs(c, cert, key)?;
        c.eq(NCryptFreeObject(key) as u64, 0)?;
        // A second import gets a container of its own.
        let again = import(MODERN, password.as_ptr(), PKCS12_ALWAYS_CNG_KSP);
        let other = only_cert(c, again)?;
        c.ok(property(other, CERT_KEY_PROV_INFO_PROP_ID, &mut buf).is_ok(), 0)?;
        let info = buf.as_ptr().cast::<KeyProvInfo>().read();
        c.ok(!same_wide(first.as_ptr(), info.container), 0)?;
        CertFreeCertificateContext(other);
        CertCloseStore(again, 0);
        CertFreeCertificateContext(cert);
        CertCloseStore(store, 0);

        // A key that is not kept: CERT_KEY_CONTEXT_PROP_ID; the legacy
        // algorithms and the friendly name.
        let flags = PKCS12_NO_PERSIST_KEY | PKCS12_ALWAYS_CNG_KSP;
        let store = import(LEGACY, password.as_ptr(), flags);
        let cert = only_cert(c, store)?;
        c.ok(property(cert, CERT_KEY_PROV_INFO_PROP_ID, &mut buf) == not_found, 0)?;
        let size = property(cert, CERT_KEY_CONTEXT_PROP_ID, &mut buf);
        c.ok(size.is_ok(), size.err().unwrap_or(0).into())?;
        // CERT_KEY_CONTEXT: cbSize (12 bytes on x86, 24 on x64), the key
        // handle, dwKeySpec.
        let context = buf.as_ptr().cast::<KeyContext>().read();
        c.eq(context.size.into(), super::by_width(12, 24))?;
        c.ok(context.key != 0, 0)?;
        c.eq(context.key_spec.into(), CERT_NCRYPT_KEY_SPEC.into())?;
        let size = property(cert, CERT_FRIENDLY_NAME_PROP_ID, &mut buf);
        c.ok(size == Ok(18), size.err().unwrap_or(0).into())?;
        c.ok(wide_is(buf.as_ptr().cast(), "gala pfx"), 0)?;
        let (key, spec, free) = acquire(c, cert)?;
        c.ok(spec == CERT_NCRYPT_KEY_SPEC && free == 0, spec.into())?;
        signs(c, cert, key)?;
        CertFreeCertificateContext(cert);
        c.ok(CertCloseStore(store, 0) != 0, 0)?;
    }
    Ok(())
}

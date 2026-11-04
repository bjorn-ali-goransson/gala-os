//! Credentials: SCHANNEL_CRED and SCH_CREDENTIALS as AcquireCredentialsHandle
//! gets them, the trust anchors of gala's machine root store, a server's
//! certificate and its CNG key (signing through ncrypt), the clock.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::ffi::c_void;

use schannel_tls::rustls::crypto::CryptoProvider;
use schannel_tls::rustls::pki_types::{CertificateDer, UnixTime};
use schannel_tls::rustls::sign::{Signer, SigningKey};
use schannel_tls::rustls::time_provider::TimeProvider;
use schannel_tls::rustls::{Error, SignatureAlgorithm, SignatureScheme};
use schannel_tls::{Protocols, sec};

use crate::ffi::{self, missing};

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetSystemTimeAsFileTime(time: *mut u64);
    fn GetSystemDirectoryW(buffer: *mut u16, size: u32) -> u32;
    fn FindFirstFileW(name: *const u16, data: *mut FindData) -> usize;
    fn FindNextFileW(find: usize, data: *mut FindData) -> i32;
    fn FindClose(find: usize) -> i32;
    fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        security: *const c_void,
        disposition: u32,
        flags: u32,
        template: usize,
    ) -> usize;
    fn ReadFile(file: usize, buffer: *mut u8, size: u32, read: *mut u32, overlapped: usize) -> i32;
    fn CloseHandle(handle: usize) -> i32;
}

#[link(name = "crypt32", kind = "raw-dylib")]
unsafe extern "system" {
    fn CertGetCertificateContextProperty(
        cert: *const c_void,
        id: u32,
        data: *mut u8,
        size: *mut u32,
    ) -> i32;
}

#[link(name = "ncrypt", kind = "raw-dylib")]
unsafe extern "system" {
    fn NCryptOpenStorageProvider(provider: *mut usize, name: *const u16, flags: u32) -> i32;
    fn NCryptOpenKey(
        provider: usize,
        key: *mut usize,
        name: *const u16,
        spec: u32,
        flags: u32,
    ) -> i32;
    fn NCryptSignHash(
        key: usize,
        padding: *const c_void,
        hash: *const u8,
        hash_size: u32,
        signature: *mut u8,
        capacity: u32,
        size: *mut u32,
        flags: u32,
    ) -> i32;
    fn NCryptFreeObject(object: usize) -> i32;
}

/// WIN32_FIND_DATAW.
#[repr(C)]
struct FindData {
    attributes: u32,
    times: [u32; 6],
    size_high: u32,
    size_low: u32,
    reserved: [u32; 2],
    name: [u16; 260],
    short_name: [u16; 14],
    _obsolete: [u32; 3],
}

/// The clock rustls checks certificates' validity with.
#[derive(Debug)]
pub struct Clock;

impl TimeProvider for Clock {
    fn current_time(&self) -> Option<UnixTime> {
        let mut t = 0u64;
        // SAFETY: an output of ours.
        unsafe { GetSystemTimeAsFileTime(&mut t) };
        // FILETIME: 100 ns since 1601; 11644473600 s to 1970.
        let secs = (t / 10_000_000).checked_sub(11_644_473_600)?;
        Some(UnixTime::since_unix_epoch(core::time::Duration::from_secs(secs)))
    }
}

/// gala's machine root store: the certificates (DER or PEM) in
/// %SystemRoot%\System32\gala\roots. (crypt32 has no certificate stores
/// yet; when it gets the ROOT store, this is where it reads it.)
pub fn machine_roots() -> Vec<CertificateDer<'static>> {
    let mut roots = Vec::new();
    let mut dir = [0u16; 300];
    // SAFETY: our buffer.
    let n = unsafe { GetSystemDirectoryW(dir.as_mut_ptr(), 260) } as usize;
    if n == 0 || n > 260 {
        return roots;
    }
    let mut path: Vec<u16> = dir[..n].to_vec();
    path.extend("\\gala\\roots\\".encode_utf16());
    let base = path.len();
    let mut pattern = path.clone();
    pattern.extend([u16::from(b'*'), 0]);
    // SAFETY: plain data, written by the call.
    let mut data: FindData = unsafe { core::mem::zeroed() };
    // SAFETY: a NUL-terminated pattern; our output.
    let find = unsafe { FindFirstFileW(pattern.as_ptr(), &mut data) };
    if find == usize::MAX {
        missing(b"schannel.dll!machine root store (no gala/roots directory)", 0);
        return roots;
    }
    loop {
        if data.attributes & 0x10 == 0 {
            let len = data.name.iter().position(|&c| c == 0).unwrap_or(0);
            path.truncate(base);
            path.extend_from_slice(&data.name[..len]);
            path.push(0);
            if let Some(bytes) = read_file(&path) {
                roots.extend(certificates(&bytes));
            }
        }
        // SAFETY: the search handle; our output.
        if unsafe { FindNextFileW(find, &mut data) } == 0 {
            break;
        }
    }
    // SAFETY: the search handle.
    unsafe { FindClose(find) };
    roots
}

fn read_file(path: &[u16]) -> Option<Vec<u8>> {
    // SAFETY: a NUL-terminated path; GENERIC_READ, FILE_SHARE_READ,
    // OPEN_EXISTING.
    let file = unsafe { CreateFileW(path.as_ptr(), 0x8000_0000, 1, core::ptr::null(), 3, 0, 0) };
    if file == usize::MAX {
        return None;
    }
    let mut out = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let mut n = 0u32;
        // SAFETY: our buffer and output.
        let ok = unsafe { ReadFile(file, chunk.as_mut_ptr(), 4096, &mut n, 0) };
        if ok == 0 || n == 0 || out.try_reserve(n as usize).is_err() || out.len() > 1 << 20 {
            break;
        }
        out.extend_from_slice(&chunk[..n as usize]);
    }
    // SAFETY: our handle.
    unsafe { CloseHandle(file) };
    Some(out)
}

/// The certificates of a file: DER (one), or PEM (any number).
fn certificates(bytes: &[u8]) -> Vec<CertificateDer<'static>> {
    let mut out = Vec::new();
    if bytes.first() == Some(&0x30) {
        out.push(CertificateDer::from(bytes.to_vec()));
        return out;
    }
    let text = core::str::from_utf8(bytes).unwrap_or("");
    let mut rest = text;
    while let Some(start) = rest.find("-----BEGIN CERTIFICATE-----") {
        let body = &rest[start + 27..];
        let Some(end) = body.find("-----END CERTIFICATE-----") else { break };
        if let Some(der) = base64(&body[..end]) {
            out.push(CertificateDer::from(der));
        }
        rest = &body[end..];
    }
    out
}

fn base64(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let (mut acc, mut bits) = (0u32, 0);
    for c in text.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' | b'\r' | b'\n' | b' ' | b'\t' => continue,
            _ => return None,
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

/// A server key in CNG (an NCRYPT_KEY_HANDLE): rustls signs through
/// NCryptSignHash, so the key stays in ncrypt.
#[derive(Debug)]
pub struct CngKey {
    handle: usize,
    owned: bool,
}

impl Drop for CngKey {
    fn drop(&mut self) {
        if self.owned {
            // SAFETY: the key handle we opened.
            unsafe { NCryptFreeObject(self.handle) };
        }
    }
}

/// RSA's schemes, best first (TLS 1.3 takes PSS only).
const RSA_SCHEMES: [SignatureScheme; 6] = [
    SignatureScheme::RSA_PSS_SHA256,
    SignatureScheme::RSA_PSS_SHA384,
    SignatureScheme::RSA_PSS_SHA512,
    SignatureScheme::RSA_PKCS1_SHA256,
    SignatureScheme::RSA_PKCS1_SHA384,
    SignatureScheme::RSA_PKCS1_SHA512,
];

impl SigningKey for CngKey {
    fn choose_scheme(&self, offered: &[SignatureScheme]) -> Option<Box<dyn Signer>> {
        let scheme = RSA_SCHEMES.into_iter().find(|s| offered.contains(s))?;
        Some(Box::new(CngSigner { handle: self.handle, scheme }))
    }

    fn algorithm(&self) -> SignatureAlgorithm {
        SignatureAlgorithm::RSA
    }
}

#[derive(Debug)]
struct CngSigner {
    handle: usize,
    scheme: SignatureScheme,
}

// SAFETY (Send/Sync): the handle is a plain value; ncrypt's calls are
// thread-safe.
unsafe impl Send for CngKey {}
unsafe impl Sync for CngKey {}

impl Signer for CngSigner {
    fn sign(&self, message: &[u8]) -> Result<Vec<u8>, Error> {
        use sha2::Digest;
        let (hash, alg): (Vec<u8>, &str) = match self.scheme {
            SignatureScheme::RSA_PSS_SHA256 | SignatureScheme::RSA_PKCS1_SHA256 => {
                (sha2::Sha256::digest(message).to_vec(), "SHA256")
            }
            SignatureScheme::RSA_PSS_SHA384 | SignatureScheme::RSA_PKCS1_SHA384 => {
                (sha2::Sha384::digest(message).to_vec(), "SHA384")
            }
            _ => (sha2::Sha512::digest(message).to_vec(), "SHA512"),
        };
        let mut alg_w = [0u16; 8];
        ffi::put_wide(&mut alg_w, alg);
        let pss = matches!(
            self.scheme,
            SignatureScheme::RSA_PSS_SHA256
                | SignatureScheme::RSA_PSS_SHA384
                | SignatureScheme::RSA_PSS_SHA512
        );
        // BCRYPT_PSS_PADDING_INFO { pszAlgId, cbSalt } (salt: the hash
        // length, as TLS requires) or BCRYPT_PKCS1_PADDING_INFO.
        #[repr(C)]
        struct Padding {
            alg: *const u16,
            salt: u32,
        }
        let padding = Padding { alg: alg_w.as_ptr(), salt: hash.len() as u32 };
        let flags = if pss { 8 } else { 2 }; // BCRYPT_PAD_PSS / BCRYPT_PAD_PKCS1
        let mut sig = alloc::vec![0u8; 1024];
        let mut n = 0u32;
        // SAFETY: the key handle; our buffers.
        let status = unsafe {
            NCryptSignHash(
                self.handle,
                (&padding as *const Padding).cast(),
                hash.as_ptr(),
                hash.len() as u32,
                sig.as_mut_ptr(),
                sig.len() as u32,
                &mut n,
                flags,
            )
        };
        if status != 0 {
            ffi::failed("NCryptSignHash", &status, status as u32);
            return Err(Error::General(String::from("NCryptSignHash failed")));
        }
        sig.truncate(n as usize);
        Ok(sig)
    }

    fn scheme(&self) -> SignatureScheme {
        self.scheme
    }
}

// SAFETY (Send/Sync): as for CngKey.
unsafe impl Send for CngSigner {}
unsafe impl Sync for CngSigner {}

/// CERT_CONTEXT's pbCertEncoded/cbCertEncoded.
fn encoded<'a>(cert: *const c_void) -> &'a [u8] {
    // SAFETY: the caller's CERT_CONTEXT.
    unsafe {
        let data = cert.cast::<u8>().add(8).cast::<*const u8>().read_unaligned();
        let size = cert.cast::<u8>().add(16).cast::<u32>().read_unaligned();
        if data.is_null() { &[] } else { core::slice::from_raw_parts(data, size as usize) }
    }
}

/// The CNG key of a server certificate: CERT_NCRYPT_KEY_HANDLE_PROP_ID,
/// or the key CERT_KEY_PROV_INFO_PROP_ID names (opened from its
/// provider).
fn certificate_key(cert: *const c_void) -> Result<CngKey, u32> {
    let mut handle = 0u64;
    let mut size = 8u32;
    // SAFETY: crypt32's property of the caller's context; our output.
    if unsafe {
        CertGetCertificateContextProperty(cert, 78, (&mut handle as *mut u64).cast(), &mut size)
    } != 0
    {
        return Ok(CngKey { handle: handle as usize, owned: false });
    }
    let mut info = [0u64; 300];
    let mut size = (info.len() * 8) as u32;
    // SAFETY: as above (CRYPT_KEY_PROV_INFO, with its strings).
    if unsafe { CertGetCertificateContextProperty(cert, 2, info.as_mut_ptr().cast(), &mut size) }
        == 0
    {
        missing(b"schannel.dll!server certificate without a private key", 0);
        return Err(sec::E_NO_CREDENTIALS);
    }
    let container = info[0] as *const u16;
    let provider = info[1] as *const u16;
    let provider_type = info[2] as u32;
    let spec = info[5] as u32;
    if provider_type != 0 {
        // A CryptoAPI (CSP) key container: not in gala.
        missing(b"schannel.dll!server key in a CryptoAPI provider", provider_type.into());
        return Err(sec::E_NO_CREDENTIALS);
    }
    let (mut p, mut key) = (0usize, 0usize);
    // SAFETY: NUL-terminated names from the property; our outputs.
    unsafe {
        if NCryptOpenStorageProvider(&mut p, provider, 0) != 0 {
            return Err(sec::E_NO_CREDENTIALS);
        }
        let r = NCryptOpenKey(p, &mut key, container, spec, 0);
        NCryptFreeObject(p);
        if r != 0 {
            ffi::failed("NCryptOpenKey", &ffi::wide(container), r as u32);
            return Err(sec::E_NO_CREDENTIALS);
        }
    }
    Ok(CngKey { handle: key, owned: true })
}

/// Whether a certificate's key is RSA (rsaEncryption in its
/// SubjectPublicKeyInfo).
fn is_rsa(der: &[u8]) -> bool {
    const RSA: [u8; 11] = [6, 9, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 1, 1, 1];
    der.windows(RSA.len()).any(|w| w == RSA)
}

/// A credential handle's state.
pub struct Credential {
    pub inbound: bool,
    pub protocols: Protocols,
    pub manual: bool,
    pub check_name: bool,
    pub roots: Vec<CertificateDer<'static>>,
    pub chain: Vec<CertificateDer<'static>>,
    pub key: Option<Arc<dyn SigningKey>>,
    pub provider: Arc<CryptoProvider>,
}

const SP_PROT_TLS1_2: u32 = 0xc00;
const SP_PROT_TLS1_3: u32 = 0x3000;
const SCH_CRED_NO_SERVERNAME_CHECK: u32 = 0x4;
const SCH_CRED_MANUAL_CRED_VALIDATION: u32 = 0x8;
const SCH_CRED_REVOCATION_CHECKS: u32 = 0x100 | 0x200 | 0x400;

fn read32(p: *const u8, at: usize) -> u32 {
    // SAFETY: the caller's structure, at a field of it.
    unsafe { p.add(at).cast::<u32>().read_unaligned() }
}

fn read_ptr(p: *const u8, at: usize) -> *const u8 {
    // SAFETY: as above.
    unsafe { p.add(at).cast::<*const u8>().read_unaligned() }
}

/// The credentials AcquireCredentialsHandle's pAuthData describes
/// (SCHANNEL_CRED version 4, SCH_CREDENTIALS version 5, or none).
pub fn acquire(inbound: bool, auth: *const u8) -> Result<Credential, u32> {
    let (mut enabled, mut disabled, mut flags) = (0u32, 0u32, 0u32);
    let (mut count, mut certs) = (0u32, core::ptr::null::<u8>());
    if !auth.is_null() {
        match read32(auth, 0) {
            4 => {
                count = read32(auth, 4);
                certs = read_ptr(auth, 8);
                enabled = read32(auth, 56);
                flags = read32(auth, 72);
                if read32(auth, 40) != 0 {
                    missing(
                        b"schannel.dll!SCHANNEL_CRED palgSupportedAlgs",
                        read32(auth, 40).into(),
                    );
                }
            }
            5 => {
                count = read32(auth, 8);
                certs = read_ptr(auth, 16);
                flags = read32(auth, 52);
                let tls = read_ptr(auth, 64);
                for i in 0..read32(auth, 56) as usize {
                    // TLS_PARAMETERS (40 bytes): only those for every ALPN
                    // id (cAlpnIds 0) say what is disabled overall.
                    let t = tls.wrapping_add(40 * i);
                    if !tls.is_null() && read32(t, 0) == 0 {
                        disabled |= read32(t, 16);
                    }
                    if !tls.is_null() && read32(t, 20) != 0 {
                        missing(
                            b"schannel.dll!TLS_PARAMETERS pDisabledCrypto",
                            read32(t, 20).into(),
                        );
                    }
                }
            }
            v => {
                missing(b"schannel.dll!AcquireCredentialsHandle(credential version)", v.into());
                return Err(0x8009_030d); // SEC_E_UNKNOWN_CREDENTIALS
            }
        }
    }
    if flags & SCH_CRED_REVOCATION_CHECKS != 0 {
        // No CRLs or OCSP here: certificates are not checked for revocation.
        missing(b"schannel.dll!revocation checking", flags.into());
    }
    let mut protocols = Protocols {
        tls12: enabled == 0 || enabled & SP_PROT_TLS1_2 != 0,
        tls13: enabled == 0 || enabled & SP_PROT_TLS1_3 != 0,
    };
    protocols.tls12 &= disabled & SP_PROT_TLS1_2 == 0;
    protocols.tls13 &= disabled & SP_PROT_TLS1_3 == 0;
    if !protocols.tls12 && !protocols.tls13 {
        // SSL 3.0, TLS 1.0 and 1.1 (or DTLS) only: not supported.
        missing(b"schannel.dll!protocols other than TLS 1.2 and 1.3", (enabled | disabled).into());
    }
    let manual = flags & SCH_CRED_MANUAL_CRED_VALIDATION != 0;
    let mut credential = Credential {
        inbound,
        protocols,
        manual,
        check_name: flags & SCH_CRED_NO_SERVERNAME_CHECK == 0,
        roots: Vec::new(),
        chain: Vec::new(),
        key: None,
        provider: Arc::new(schannel_tls::provider()),
    };
    if count > 0 && !certs.is_null() {
        // SAFETY: the caller's array of `count` PCCERT_CONTEXT.
        let cert = unsafe { certs.cast::<*const c_void>().read_unaligned() };
        if !cert.is_null() {
            let der = encoded(cert);
            credential.chain.push(CertificateDer::from(der.to_vec()));
            if inbound {
                if !is_rsa(der) {
                    missing(b"schannel.dll!server key other than RSA", 0);
                    return Err(sec::E_NO_CREDENTIALS);
                }
                credential.key = Some(Arc::new(certificate_key(cert)?));
            } else {
                // Client certificates (mutual TLS) are not sent yet.
                missing(b"schannel.dll!client certificate", 0);
                credential.chain.clear();
            }
        }
    }
    if inbound && credential.key.is_none() {
        return Err(sec::E_NO_CREDENTIALS);
    }
    if !inbound && !manual {
        credential.roots = machine_roots();
    }
    Ok(credential)
}

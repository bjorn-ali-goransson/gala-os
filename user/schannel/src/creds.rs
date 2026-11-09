//! Credentials: SCHANNEL_CRED and SCH_CREDENTIALS as AcquireCredentialsHandle
//! gets them, a server's or client's certificate and its CNG key (RSA or
//! ECDSA, signing through ncrypt), the handle's session caches, the
//! clock. Trust is crypt32's (`trust`).

use alloc::boxed::Box;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::ffi::c_void;

use schannel_tls::rustls::crypto::CryptoProvider;
use schannel_tls::rustls::pki_types::{CertificateDer, UnixTime};
use schannel_tls::rustls::sign::{CertifiedKey, Signer, SigningKey};
use schannel_tls::rustls::time_provider::TimeProvider;
use schannel_tls::rustls::{Error, SignatureAlgorithm, SignatureScheme};
use schannel_tls::{ClientCache, Protocols, ServerCache, sec};

use crate::ffi::{self, missing};
use crate::layout::{CertContext, KeyProvInfo, SchCredentials, SchannelCred, TlsParameters};

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GetSystemTimeAsFileTime(time: *mut u64);
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "crypt32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "crypt32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn CertGetCertificateContextProperty(
        cert: *const c_void,
        id: u32,
        data: *mut u8,
        size: *mut u32,
    ) -> i32;
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "ncrypt", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "ncrypt", kind = "raw-dylib"))]
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

/// A certificate's key in CNG (an NCRYPT_KEY_HANDLE): rustls signs
/// through NCryptSignHash, so the key stays in ncrypt.
#[derive(Debug)]
pub struct CngKey {
    handle: usize,
    owned: bool,
    kind: KeyKind,
}

/// A certificate's key algorithm (from its SubjectPublicKeyInfo).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyKind {
    Rsa,
    EcdsaP256,
    EcdsaP384,
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
        // ECDSA: the curve's own hash (TLS 1.3 binds them; Windows signs
        // so in TLS 1.2 too).
        let scheme = match self.kind {
            KeyKind::Rsa => RSA_SCHEMES.into_iter().find(|s| offered.contains(s))?,
            KeyKind::EcdsaP256 => {
                Some(SignatureScheme::ECDSA_NISTP256_SHA256).filter(|s| offered.contains(s))?
            }
            KeyKind::EcdsaP384 => {
                Some(SignatureScheme::ECDSA_NISTP384_SHA384).filter(|s| offered.contains(s))?
            }
        };
        Some(Box::new(CngSigner { handle: self.handle, scheme }))
    }

    fn algorithm(&self) -> SignatureAlgorithm {
        match self.kind {
            KeyKind::Rsa => SignatureAlgorithm::RSA,
            _ => SignatureAlgorithm::ECDSA,
        }
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
            SignatureScheme::RSA_PSS_SHA256
            | SignatureScheme::RSA_PKCS1_SHA256
            | SignatureScheme::ECDSA_NISTP256_SHA256 => {
                (sha2::Sha256::digest(message).to_vec(), "SHA256")
            }
            SignatureScheme::RSA_PSS_SHA384
            | SignatureScheme::RSA_PKCS1_SHA384
            | SignatureScheme::ECDSA_NISTP384_SHA384 => {
                (sha2::Sha384::digest(message).to_vec(), "SHA384")
            }
            _ => (sha2::Sha512::digest(message).to_vec(), "SHA512"),
        };
        let ecdsa = matches!(
            self.scheme,
            SignatureScheme::ECDSA_NISTP256_SHA256 | SignatureScheme::ECDSA_NISTP384_SHA384
        );
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
        // BCRYPT_PAD_PSS / BCRYPT_PAD_PKCS1; ECDSA has no padding.
        let flags = if ecdsa {
            0
        } else if pss {
            8
        } else {
            2
        };
        let padding: *const c_void =
            if ecdsa { core::ptr::null() } else { (&padding as *const Padding).cast() };
        let mut sig = alloc::vec![0u8; 1024];
        let mut n = 0u32;
        // SAFETY: the key handle; our buffers.
        let status = unsafe {
            NCryptSignHash(
                self.handle,
                padding,
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
        if ecdsa {
            // CNG gives r || s; TLS wants DER's Ecdsa-Sig-Value.
            return ecdsa_der(&sig).ok_or_else(|| Error::General(String::from("ECDSA signature")));
        }
        Ok(sig)
    }

    fn scheme(&self) -> SignatureScheme {
        self.scheme
    }
}

// SAFETY (Send/Sync): as for CngKey.
unsafe impl Send for CngSigner {}
unsafe impl Sync for CngSigner {}

/// An ECDSA signature r || s (two halves of equal size) as DER's
/// Ecdsa-Sig-Value: SEQUENCE { INTEGER r, INTEGER s }.
fn ecdsa_der(rs: &[u8]) -> Option<Vec<u8>> {
    if rs.is_empty() || !rs.len().is_multiple_of(2) || rs.len() > 132 {
        return None;
    }
    let mut body = Vec::new();
    for half in rs.chunks(rs.len() / 2) {
        // Minimal: no leading zeros, one zero before a high bit.
        let start = half.iter().position(|&b| b != 0).unwrap_or(half.len() - 1);
        let v = &half[start..];
        let pad = v[0] & 0x80 != 0;
        body.push(2);
        body.push((v.len() + usize::from(pad)) as u8);
        if pad {
            body.push(0);
        }
        body.extend_from_slice(v);
    }
    let mut der = Vec::new();
    der.push(0x30);
    if body.len() >= 0x80 {
        der.push(0x81);
    }
    der.push(body.len() as u8);
    der.extend_from_slice(&body);
    Some(der)
}

/// CERT_CONTEXT's pbCertEncoded/cbCertEncoded.
fn encoded<'a>(cert: *const c_void) -> &'a [u8] {
    // SAFETY: the caller's CERT_CONTEXT.
    unsafe {
        let cert = &*cert.cast::<CertContext>();
        if cert.encoded.is_null() {
            &[]
        } else {
            core::slice::from_raw_parts(cert.encoded, cert.encoded_size as usize)
        }
    }
}

/// The CNG key of a certificate (a server's, or a client's):
/// CERT_NCRYPT_KEY_HANDLE_PROP_ID, or the key CERT_KEY_PROV_INFO_PROP_ID
/// names (opened from its provider).
pub fn certificate_key(cert: *const c_void, kind: KeyKind) -> Result<CngKey, u32> {
    // NCRYPT_KEY_HANDLE: a ULONG_PTR.
    let mut handle = 0usize;
    let mut size = size_of::<usize>() as u32;
    // SAFETY: crypt32's property of the caller's context; our output.
    if unsafe { CertGetCertificateContextProperty(cert, 78, (&raw mut handle).cast(), &mut size) }
        != 0
    {
        return Ok(CngKey { handle, owned: false, kind });
    }
    // CRYPT_KEY_PROV_INFO with its strings after it (pointer-aligned).
    let mut block = [0usize; 600];
    let mut size = size_of_val(&block) as u32;
    // SAFETY: as above.
    if unsafe { CertGetCertificateContextProperty(cert, 2, block.as_mut_ptr().cast(), &mut size) }
        == 0
        || (size as usize) < size_of::<KeyProvInfo>()
    {
        missing(b"schannel.dll!certificate without a private key", 0);
        return Err(sec::E_NO_CREDENTIALS);
    }
    // SAFETY: crypt32 wrote a CRYPT_KEY_PROV_INFO at the block's start.
    let info = unsafe { &*block.as_ptr().cast::<KeyProvInfo>() };
    let (container, provider) = (info.container, info.provider);
    let (provider_type, spec) = (info.provider_type, info.key_spec);
    if provider_type != 0 {
        // A CryptoAPI (CSP) key container: not in gala.
        missing(b"schannel.dll!certificate key in a CryptoAPI provider", provider_type.into());
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
    Ok(CngKey { handle: key, owned: true, kind })
}

/// A certificate's key algorithm: rsaEncryption, or id-ecPublicKey on
/// P-256 or P-384, in its SubjectPublicKeyInfo (the OIDs as DER encodes
/// them; the signature algorithm's OIDs differ from these).
pub fn key_kind(der: &[u8]) -> Option<KeyKind> {
    const RSA: [u8; 11] = [6, 9, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 1, 1, 1];
    const EC: [u8; 9] = [6, 7, 0x2a, 0x86, 0x48, 0xce, 0x3d, 2, 1];
    const P256: [u8; 10] = [6, 8, 0x2a, 0x86, 0x48, 0xce, 0x3d, 3, 1, 7];
    const P384: [u8; 7] = [6, 5, 0x2b, 0x81, 0x04, 0, 34];
    let has = |oid: &[u8]| der.windows(oid.len()).any(|w| w == oid);
    if has(&RSA) {
        Some(KeyKind::Rsa)
    } else if has(&EC) && has(&P256) {
        Some(KeyKind::EcdsaP256)
    } else if has(&EC) && has(&P384) {
        Some(KeyKind::EcdsaP384)
    } else {
        None
    }
}

/// A credential handle's state.
pub struct Credential {
    pub inbound: bool,
    pub protocols: Protocols,
    pub manual: bool,
    pub check_name: bool,
    pub chain: Vec<CertificateDer<'static>>,
    pub key: Option<Arc<dyn SigningKey>>,
    pub provider: Arc<CryptoProvider>,
    /// The session caches of the handle (a client's or a server's):
    /// contexts of the same handle (and, for a client, target name)
    /// resume.
    pub client_cache: ClientCache,
    pub server_cache: Arc<ServerCache>,
    /// A client's certificate (paCred[0]) with its CNG key, sent when
    /// the server asks.
    pub client_cert: Option<Arc<CertifiedKey>>,
    /// SCH_CRED_NO_DEFAULT_CREDS: no automatic choice from the MY store.
    pub no_default_creds: bool,
    /// The SCH_CRED_REVOCATION_CHECK_* and SCH_CRED_IGNORE_* revocation
    /// flags, for automatic validation (trust.rs).
    pub revocation: u32,
}

const SP_PROT_TLS1_2: u32 = 0xc00;
const SP_PROT_TLS1_3: u32 = 0x3000;
/// The server and client bits of every SP_PROT_* protocol (PCT 1.0 to
/// TLS 1.3, DTLS 1.0 and 1.2: server even, client odd).
const SP_PROT_SERVER: u32 = 0x5_5555;
const SP_PROT_CLIENT: u32 = 0xa_aaaa;
const SCH_CRED_NO_SERVERNAME_CHECK: u32 = 0x4;
const SCH_CRED_MANUAL_CRED_VALIDATION: u32 = 0x8;
const SCH_CRED_NO_DEFAULT_CREDS: u32 = 0x10;
const SCH_CRED_DISABLE_RECONNECTS: u32 = 0x80;
/// SCH_CRED_REVOCATION_CHECK_END_CERT, _CHAIN, _CHAIN_EXCLUDE_ROOT,
/// SCH_CRED_IGNORE_NO_REVOCATION_CHECK, SCH_CRED_IGNORE_REVOCATION_OFFLINE,
/// SCH_CRED_REVOCATION_CHECK_CACHE_ONLY.
const SCH_CRED_REVOCATION_FLAGS: u32 = 0x100 | 0x200 | 0x400 | 0x800 | 0x1000 | 0x4000;

/// The credentials AcquireCredentialsHandle's pAuthData describes
/// (SCHANNEL_CRED version 4, SCH_CREDENTIALS version 5, or none).
pub fn acquire(inbound: bool, auth: *const u8) -> Result<Credential, u32> {
    let (mut enabled, mut disabled, mut flags) = (0u32, 0u32, 0u32);
    let (mut count, mut certs) = (0u32, core::ptr::null::<*const CertContext>());
    if !auth.is_null() {
        // SAFETY: the caller's pAuthData starts with its dwVersion.
        match unsafe { auth.cast::<u32>().read_unaligned() } {
            4 => {
                // SAFETY: a SCHANNEL_CRED (version 4), as documented.
                let cred = unsafe { auth.cast::<SchannelCred>().read_unaligned() };
                count = cred.cred_count;
                certs = cred.creds;
                enabled = cred.enabled_protocols;
                flags = cred.flags;
                if cred.alg_count != 0 {
                    missing(b"schannel.dll!SCHANNEL_CRED palgSupportedAlgs", cred.alg_count.into());
                }
            }
            5 => {
                // SAFETY: a SCH_CREDENTIALS (version 5), as documented.
                let cred = unsafe { auth.cast::<SchCredentials>().read_unaligned() };
                count = cred.cred_count;
                certs = cred.creds;
                flags = cred.flags;
                let tls = cred.tls_parameters;
                for i in 0..cred.tls_parameter_count as usize {
                    if tls.is_null() {
                        break;
                    }
                    // SAFETY: the caller's array of cTlsParameters.
                    let t = unsafe { tls.add(i).cast::<TlsParameters>().read_unaligned() };
                    // Only the parameters for every ALPN id (cAlpnIds 0)
                    // say what is disabled overall.
                    if t.alpn_id_count == 0 {
                        disabled |= t.disabled_protocols;
                    }
                    if t.disabled_crypto_count != 0 {
                        missing(
                            b"schannel.dll!TLS_PARAMETERS pDisabledCrypto",
                            t.disabled_crypto_count.into(),
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
    // Each protocol has a client and a server bit (SP_PROT_TLS1_2_CLIENT,
    // _SERVER): only this side's count (curl disables every protocol but
    // the client's TLS 1.0 to 1.3, the server halves included).
    let side = if inbound { SP_PROT_SERVER } else { SP_PROT_CLIENT };
    let (tls12, tls13) = (SP_PROT_TLS1_2 & side, SP_PROT_TLS1_3 & side);
    let mut protocols = Protocols {
        tls12: enabled == 0 || enabled & tls12 != 0,
        tls13: enabled == 0 || enabled & tls13 != 0,
    };
    protocols.tls12 &= disabled & tls12 == 0;
    protocols.tls13 &= disabled & tls13 == 0;
    if !protocols.tls12 && !protocols.tls13 {
        // SSL 3.0, TLS 1.0 and 1.1 (or DTLS) only: not supported.
        missing(b"schannel.dll!protocols other than TLS 1.2 and 1.3", (enabled | disabled).into());
    }
    let manual = flags & SCH_CRED_MANUAL_CRED_VALIDATION != 0;
    // SCH_CRED_DISABLE_RECONNECTS: full handshakes only.
    let reconnects = flags & SCH_CRED_DISABLE_RECONNECTS == 0;
    let mut credential = Credential {
        inbound,
        protocols,
        manual,
        check_name: flags & SCH_CRED_NO_SERVERNAME_CHECK == 0,
        chain: Vec::new(),
        key: None,
        provider: Arc::new(schannel_tls::provider()),
        client_cache: ClientCache::new(reconnects && !inbound),
        server_cache: Arc::new(ServerCache::new(reconnects && inbound)),
        client_cert: None,
        no_default_creds: flags & SCH_CRED_NO_DEFAULT_CREDS != 0,
        revocation: flags & SCH_CRED_REVOCATION_FLAGS,
    };
    if count > 0 && !certs.is_null() {
        // SAFETY: the caller's array of `count` PCCERT_CONTEXT.
        let cert: *const c_void = unsafe { certs.read_unaligned() }.cast();
        if !cert.is_null() {
            let der = encoded(cert);
            credential.chain.push(CertificateDer::from(der.to_vec()));
            if inbound {
                let Some(kind) = key_kind(der) else {
                    missing(b"schannel.dll!server key other than RSA, ECDSA P-256 and P-384", 0);
                    return Err(sec::E_NO_CREDENTIALS);
                };
                credential.key = Some(Arc::new(certificate_key(cert, kind)?));
            } else {
                // The client's certificate, for a server that asks (only
                // the certificate: no intermediates from the stores yet).
                let Some(kind) = key_kind(der) else {
                    missing(b"schannel.dll!client key other than RSA, ECDSA P-256 and P-384", 0);
                    return Err(sec::E_NO_CREDENTIALS);
                };
                let key: Arc<dyn SigningKey> = Arc::new(certificate_key(cert, kind)?);
                let chain = core::mem::take(&mut credential.chain);
                credential.client_cert = Some(Arc::new(CertifiedKey::new(chain, key)));
            }
        }
    }
    if inbound && credential.key.is_none() {
        return Err(sec::E_NO_CREDENTIALS);
    }
    Ok(credential)
}

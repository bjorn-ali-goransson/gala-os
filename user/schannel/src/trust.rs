//! Automatic validation through crypt32's chain engine: the server's
//! certificate, with the intermediates it sent in a memory store as
//! hAdditionalStore, chained by CertGetCertificateChain on the default
//! engine (gala's machine root store, System32\gala\roots) and checked by
//! CertVerifyCertificateChainPolicy(CERT_CHAIN_POLICY_SSL) with the
//! target name, as documented for Schannel's automatic validation. A
//! program that validates itself (curl's `--cacert`, Go, Chromium's
//! issuer lookup) gets the same decision from the same engine. The policy
//! error becomes rustls' certificate error and so the SSPI status:
//! CERT_E_UNTRUSTEDROOT (and a chain that reaches no root) is
//! SEC_E_UNTRUSTED_ROOT, CERT_E_EXPIRED SEC_E_CERT_EXPIRED,
//! CERT_E_CN_NO_MATCH SEC_E_WRONG_PRINCIPAL, CERT_E_WRONG_USAGE
//! SEC_E_CERT_WRONG_USAGE, the others SEC_E_CERT_UNKNOWN. crypt32 records
//! the policy failure (`crypt32.dll!SSL: CERT_E_...`, the chain's error
//! bits), schannel.dll the handshake's (`schannel.dll!ISC: ...`).

use alloc::vec::Vec;
use core::ffi::c_void;

use schannel_tls::Trust;
use schannel_tls::rustls::CertificateError;
use schannel_tls::rustls::pki_types::UnixTime;

#[link(name = "crypt32", kind = "raw-dylib")]
unsafe extern "system" {
    fn CertOpenStore(
        provider: usize,
        encoding: u32,
        prov: usize,
        flags: u32,
        para: *const c_void,
    ) -> *mut c_void;
    fn CertCloseStore(store: *mut c_void, flags: u32) -> i32;
    fn CertAddEncodedCertificateToStore(
        store: *mut c_void,
        encoding: u32,
        data: *const u8,
        size: u32,
        disposition: u32,
        out: *mut *const c_void,
    ) -> i32;
    fn CertCreateCertificateContext(encoding: u32, data: *const u8, size: u32) -> *const c_void;
    fn CertFreeCertificateContext(cert: *const c_void) -> i32;
    fn CertGetCertificateChain(
        engine: *mut c_void,
        cert: *const c_void,
        time: *const u64,
        additional: *mut c_void,
        para: *const ChainPara,
        flags: u32,
        reserved: *mut c_void,
        chain: *mut *const c_void,
    ) -> i32;
    fn CertVerifyCertificateChainPolicy(
        policy: usize,
        chain: *const c_void,
        para: *const PolicyPara,
        status: *mut PolicyStatus,
    ) -> i32;
    fn CertFreeCertificateChain(chain: *const c_void);
}

const ENCODING: u32 = 0x1_0001; // X509_ASN_ENCODING | PKCS_7_ASN_ENCODING
const CERT_STORE_PROV_MEMORY: usize = 2;
const CERT_STORE_ADD_ALWAYS: u32 = 4;
const CERT_CHAIN_POLICY_SSL: usize = 4;
const AUTHTYPE_SERVER: u32 = 2;
const SECURITY_FLAG_IGNORE_CERT_CN_INVALID: u32 = 0x1000;

const CERT_E_EXPIRED: u32 = 0x800b_0101;
const CERT_E_CRITICAL: u32 = 0x800b_0105;
const CERT_E_UNTRUSTEDROOT: u32 = 0x800b_0109;
const CERT_E_CHAINING: u32 = 0x800b_010a;
const CERT_E_CN_NO_MATCH: u32 = 0x800b_010f;
const CERT_E_WRONG_USAGE: u32 = 0x800b_0110;
const TRUST_E_CERT_SIGNATURE: u32 = 0x8009_6004;
const CRYPT_E_REVOKED: u32 = 0x8009_2010;

/// CERT_CHAIN_PARA without its later fields (no requested usage: the SSL
/// policy checks the server usage).
#[repr(C)]
struct ChainPara {
    size: u32,
    usage: [u64; 3],
}

/// CERT_CHAIN_POLICY_PARA.
#[repr(C)]
struct PolicyPara {
    size: u32,
    flags: u32,
    extra: *const SslPara,
}

/// SSL_EXTRA_CERT_CHAIN_POLICY_PARA.
#[repr(C)]
struct SslPara {
    size: u32,
    auth_type: u32,
    checks: u32,
    server_name: *const u16,
}

/// CERT_CHAIN_POLICY_STATUS.
#[repr(C)]
struct PolicyStatus {
    size: u32,
    error: u32,
    chain_index: i32,
    element_index: i32,
    extra: *mut c_void,
}

/// The machine's trust, as crypt32 decides it.
#[derive(Debug)]
pub struct Crypt32;

impl Trust for Crypt32 {
    fn check(
        &self,
        end_entity: &[u8],
        intermediates: &[&[u8]],
        name: Option<&str>,
        now: UnixTime,
    ) -> Result<(), CertificateError> {
        // SAFETY: crypt32's documented calls with buffers of ours; what
        // they make is freed below.
        unsafe {
            let store = CertOpenStore(CERT_STORE_PROV_MEMORY, 0, 0, 0, core::ptr::null());
            for der in intermediates {
                CertAddEncodedCertificateToStore(
                    store,
                    ENCODING,
                    der.as_ptr(),
                    der.len() as u32,
                    CERT_STORE_ADD_ALWAYS,
                    core::ptr::null_mut(),
                );
            }
            let leaf = CertCreateCertificateContext(
                ENCODING,
                end_entity.as_ptr(),
                end_entity.len() as u32,
            );
            if leaf.is_null() {
                CertCloseStore(store, 0);
                return Err(CertificateError::BadEncoding);
            }
            // FILETIME: 100 ns since 1601; 11644473600 s to 1970.
            let time = (now.as_secs() + 11_644_473_600) * 10_000_000;
            let para = ChainPara { size: core::mem::size_of::<ChainPara>() as u32, usage: [0; 3] };
            let mut chain = core::ptr::null();
            let built = CertGetCertificateChain(
                core::ptr::null_mut(),
                leaf,
                &time,
                store,
                &para,
                0,
                core::ptr::null_mut(),
                &mut chain,
            ) != 0;
            let mut wide: Vec<u16> = name.unwrap_or("").encode_utf16().collect();
            wide.push(0);
            let ssl = SslPara {
                size: core::mem::size_of::<SslPara>() as u32,
                auth_type: AUTHTYPE_SERVER,
                checks: if name.is_some() { 0 } else { SECURITY_FLAG_IGNORE_CERT_CN_INVALID },
                server_name: if name.is_some() { wide.as_ptr() } else { core::ptr::null() },
            };
            let policy = PolicyPara {
                size: core::mem::size_of::<PolicyPara>() as u32,
                flags: 0,
                extra: &ssl,
            };
            let mut status = PolicyStatus {
                size: core::mem::size_of::<PolicyStatus>() as u32,
                error: CERT_E_CHAINING,
                chain_index: -1,
                element_index: -1,
                extra: core::ptr::null_mut(),
            };
            if built {
                CertVerifyCertificateChainPolicy(
                    CERT_CHAIN_POLICY_SSL,
                    chain,
                    &policy,
                    &mut status,
                );
                CertFreeCertificateChain(chain);
            }
            CertFreeCertificateContext(leaf);
            CertCloseStore(store, 0);
            match status.error {
                0 => Ok(()),
                CERT_E_UNTRUSTEDROOT | CERT_E_CHAINING => Err(CertificateError::UnknownIssuer),
                CERT_E_EXPIRED => Err(CertificateError::Expired),
                CERT_E_CN_NO_MATCH => Err(CertificateError::NotValidForName),
                CERT_E_WRONG_USAGE => Err(CertificateError::InvalidPurpose),
                TRUST_E_CERT_SIGNATURE => Err(CertificateError::BadSignature),
                CRYPT_E_REVOKED => Err(CertificateError::Revoked),
                CERT_E_CRITICAL => Err(CertificateError::UnhandledCriticalExtension),
                _ => Err(CertificateError::ApplicationVerificationFailure),
            }
        }
    }
}

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
//! SEC_E_CERT_WRONG_USAGE, revocation's CRYPT_E_REVOKED,
//! CRYPT_E_NO_REVOCATION_CHECK and CRYPT_E_REVOCATION_OFFLINE themselves
//! (as Schannel returns them), the others SEC_E_CERT_UNKNOWN. crypt32
//! records the policy failure (`crypt32.dll!SSL: CERT_E_...`, the chain's
//! error bits), schannel.dll the handshake's (`schannel.dll!ISC: ...`).
//!
//! Revocation is checked as the credential's flags ask:
//! SCH_CRED_REVOCATION_CHECK_END_CERT, _CHAIN, _CHAIN_EXCLUDE_ROOT and
//! SCH_CRED_REVOCATION_CHECK_CACHE_ONLY become CertGetCertificateChain's
//! CERT_CHAIN_REVOCATION_CHECK_* flags (none: not checked, as on Windows);
//! SCH_CRED_IGNORE_NO_REVOCATION_CHECK and
//! SCH_CRED_IGNORE_REVOCATION_OFFLINE excuse the elements whose
//! revocation is unknown (CERT_TRUST_REVOCATION_STATUS_UNKNOWN), without
//! and with CERT_TRUST_IS_OFFLINE_REVOCATION: when every such element is
//! excused the policy is asked again with
//! CERT_CHAIN_POLICY_IGNORE_ALL_REV_UNKNOWN_FLAGS. A revoked certificate
//! is never excused. rustls has no kind for "offline": it travels as
//! CertificateError::ExpiredRevocationList (schannel_tls `code`).

use alloc::vec::Vec;
use core::ffi::c_void;

use schannel_tls::Trust;

use crate::layout::{
    ChainContext, ChainPara, EnhKeyUsage, PolicyPara, PolicyStatus, SslPara, UsageMatch,
};
use schannel_tls::rustls::CertificateError;
use schannel_tls::rustls::pki_types::UnixTime;

#[cfg_attr(
    target_arch = "x86",
    link(name = "crypt32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "crypt32", kind = "raw-dylib"))]
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
const CRYPT_E_NO_REVOCATION_CHECK: u32 = 0x8009_2012;
const CRYPT_E_REVOCATION_OFFLINE: u32 = 0x8009_2013;

// SCHANNEL_CRED dwFlags (revocation).
const SCH_CRED_REVOCATION_CHECK_END_CERT: u32 = 0x100;
const SCH_CRED_REVOCATION_CHECK_CHAIN: u32 = 0x200;
const SCH_CRED_REVOCATION_CHECK_CHAIN_EXCLUDE_ROOT: u32 = 0x400;
const SCH_CRED_IGNORE_NO_REVOCATION_CHECK: u32 = 0x800;
const SCH_CRED_IGNORE_REVOCATION_OFFLINE: u32 = 0x1000;
const SCH_CRED_REVOCATION_CHECK_CACHE_ONLY: u32 = 0x4000;

// CertGetCertificateChain dwFlags.
const CERT_CHAIN_REVOCATION_CHECK_END_CERT: u32 = 0x1000_0000;
const CERT_CHAIN_REVOCATION_CHECK_CHAIN: u32 = 0x2000_0000;
const CERT_CHAIN_REVOCATION_CHECK_CHAIN_EXCLUDE_ROOT: u32 = 0x4000_0000;
const CERT_CHAIN_REVOCATION_CHECK_CACHE_ONLY: u32 = 0x8000_0000;
const CERT_CHAIN_POLICY_IGNORE_ALL_REV_UNKNOWN_FLAGS: u32 = 0xf00;
const CERT_TRUST_REVOCATION_STATUS_UNKNOWN: u32 = 0x40;
const CERT_TRUST_IS_OFFLINE_REVOCATION: u32 = 0x100_0000;

/// The chain flags the credential's revocation flags ask for.
fn chain_flags(revocation: u32) -> u32 {
    let mut flags = 0;
    if revocation & SCH_CRED_REVOCATION_CHECK_END_CERT != 0 {
        flags |= CERT_CHAIN_REVOCATION_CHECK_END_CERT;
    }
    if revocation & SCH_CRED_REVOCATION_CHECK_CHAIN != 0 {
        flags |= CERT_CHAIN_REVOCATION_CHECK_CHAIN;
    }
    if revocation & SCH_CRED_REVOCATION_CHECK_CHAIN_EXCLUDE_ROOT != 0 {
        flags |= CERT_CHAIN_REVOCATION_CHECK_CHAIN_EXCLUDE_ROOT;
    }
    if flags != 0 && revocation & SCH_CRED_REVOCATION_CHECK_CACHE_ONLY != 0 {
        flags |= CERT_CHAIN_REVOCATION_CHECK_CACHE_ONLY;
    }
    flags
}

/// Whether the credential excuses every element of `chain` whose
/// revocation is unknown (SCH_CRED_IGNORE_NO_REVOCATION_CHECK for those
/// that could not be checked, SCH_CRED_IGNORE_REVOCATION_OFFLINE for those
/// whose CRL was out of reach).
fn excused(chain: *const c_void, revocation: u32) -> bool {
    // SAFETY: a chain crypt32 gave (one simple chain of `count` elements).
    unsafe {
        let chain = &*chain.cast::<ChainContext>();
        if chain.count == 0 {
            return false;
        }
        let simple = &**chain.chains;
        (0..simple.count as usize).all(|i| {
            let error = (**simple.elements.add(i)).status.error;
            if error & CERT_TRUST_REVOCATION_STATUS_UNKNOWN == 0 {
                true
            } else if error & CERT_TRUST_IS_OFFLINE_REVOCATION != 0 {
                revocation & SCH_CRED_IGNORE_REVOCATION_OFFLINE != 0
            } else {
                revocation & SCH_CRED_IGNORE_NO_REVOCATION_CHECK != 0
            }
        })
    }
}

/// The machine's trust, as crypt32 decides it, with the credential's
/// revocation flags (SCH_CRED_REVOCATION_CHECK_*, SCH_CRED_IGNORE_*).
#[derive(Debug)]
pub struct Crypt32 {
    pub revocation: u32,
}

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
            let para = ChainPara {
                size: core::mem::size_of::<ChainPara>() as u32,
                requested: UsageMatch {
                    kind: 0,
                    usage: EnhKeyUsage { count: 0, ids: core::ptr::null() },
                },
            };
            let mut chain = core::ptr::null();
            let built = CertGetCertificateChain(
                core::ptr::null_mut(),
                leaf,
                &time,
                store,
                &para,
                chain_flags(self.revocation),
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
            let mut policy = PolicyPara {
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
                // Unknown revocation the credential excuses: the rest of
                // the policy decides.
                if matches!(status.error, CRYPT_E_NO_REVOCATION_CHECK | CRYPT_E_REVOCATION_OFFLINE)
                    && excused(chain, self.revocation)
                {
                    policy.flags = CERT_CHAIN_POLICY_IGNORE_ALL_REV_UNKNOWN_FLAGS;
                    CertVerifyCertificateChainPolicy(
                        CERT_CHAIN_POLICY_SSL,
                        chain,
                        &policy,
                        &mut status,
                    );
                }
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
                CRYPT_E_NO_REVOCATION_CHECK => Err(CertificateError::UnknownRevocationStatus),
                CRYPT_E_REVOCATION_OFFLINE => Err(CertificateError::ExpiredRevocationList),
                CERT_E_CRITICAL => Err(CertificateError::UnhandledCriticalExtension),
                _ => Err(CertificateError::ApplicationVerificationFailure),
            }
        }
    }
}

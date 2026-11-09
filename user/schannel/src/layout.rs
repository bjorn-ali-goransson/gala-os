//! The caller's structures with pointer-sized fields that schannel.dll
//! reads and writes, typed (`repr(C)` with pointers): one definition gives
//! the x64 layout (System32's schannel.dll) and the x86 one (SysWOW64's),
//! as sspi.h, schannel.h and wincrypt.h lay them out (ARCHITECTURE "Plan:
//! WOW64"). The checks (`check!`) hold each to the sizes and offsets of
//! the SDK's headers for both targets (mingw-w64's, compiled for
//! i686-w64-mingw32 and x86_64-w64-mingw32; SCH_CREDENTIALS and
//! TLS_PARAMETERS, which mingw lacks, as learn.microsoft.com declares
//! them).

use core::ffi::c_void;
use core::mem::{offset_of, size_of};

use crate::ffi::{SecBuffer, SecBufferDesc, SecHandle};

/// SCHANNEL_CRED (dwVersion SCHANNEL_CRED_VERSION, 4).
#[repr(C)]
pub struct SchannelCred {
    pub version: u32,
    pub cred_count: u32,
    pub creds: *const *const CertContext,
    pub root_store: *mut c_void,
    pub mapper_count: u32,
    pub mappers: *const c_void,
    pub alg_count: u32,
    pub algs: *const u32,
    pub enabled_protocols: u32,
    pub minimum_strength: u32,
    pub maximum_strength: u32,
    pub session_lifespan: u32,
    pub flags: u32,
    pub cred_format: u32,
}

/// SCH_CREDENTIALS (dwVersion SCH_CREDENTIALS_VERSION, 5).
#[repr(C)]
pub struct SchCredentials {
    pub version: u32,
    pub cred_format: u32,
    pub cred_count: u32,
    pub creds: *const *const CertContext,
    pub root_store: *mut c_void,
    pub mapper_count: u32,
    pub mappers: *const c_void,
    pub session_lifespan: u32,
    pub flags: u32,
    pub tls_parameter_count: u32,
    pub tls_parameters: *const TlsParameters,
}

/// TLS_PARAMETERS.
#[repr(C)]
pub struct TlsParameters {
    pub alpn_id_count: u32,
    pub alpn_ids: *const c_void,
    pub disabled_protocols: u32,
    pub disabled_crypto_count: u32,
    pub disabled_crypto: *const c_void,
    pub flags: u32,
}

/// CERT_CONTEXT.
#[repr(C)]
pub struct CertContext {
    pub encoding: u32,
    pub encoded: *const u8,
    pub encoded_size: u32,
    pub info: *const c_void,
    pub store: *mut c_void,
}

/// CRYPT_KEY_PROV_INFO (the property CERT_KEY_PROV_INFO_PROP_ID, its
/// strings after it in the same block).
#[repr(C)]
pub struct KeyProvInfo {
    pub container: *const u16,
    pub provider: *const u16,
    pub provider_type: u32,
    pub flags: u32,
    pub param_count: u32,
    pub params: *const c_void,
    pub key_spec: u32,
}

/// SecPkgInfoW.
#[repr(C)]
pub struct SecPkgInfo {
    pub capabilities: u32,
    pub version: u16,
    pub rpc_id: u16,
    pub max_token: u32,
    pub name: *const u16,
    pub comment: *const u16,
}

/// SecPkgContext_NegotiationInfoW.
#[repr(C)]
pub struct NegotiationInfo {
    pub package_info: *mut SecPkgInfo,
    pub state: u32,
}

/// CERT_NAME_BLOB.
#[repr(C)]
pub struct NameBlob {
    pub size: u32,
    pub data: *mut u8,
}

/// SecPkgContext_IssuerListInfoEx.
#[repr(C)]
pub struct IssuerListEx {
    pub issuers: *mut NameBlob,
    pub count: u32,
}

/// CERT_CHAIN_PARA without its later fields (no requested usage):
/// cbSize, RequestedUsage (a CERT_USAGE_MATCH).
#[repr(C)]
pub struct ChainPara {
    pub size: u32,
    pub requested: UsageMatch,
}

/// CERT_USAGE_MATCH: dwType, then a CERT_ENHKEY_USAGE.
#[repr(C)]
pub struct UsageMatch {
    pub kind: u32,
    pub usage: EnhKeyUsage,
}

/// CERT_ENHKEY_USAGE.
#[repr(C)]
pub struct EnhKeyUsage {
    pub count: u32,
    pub ids: *const *const u8,
}

/// CERT_TRUST_STATUS.
#[repr(C)]
pub struct TrustStatus {
    pub error: u32,
    pub info: u32,
}

/// CERT_CHAIN_ELEMENT (its start).
#[repr(C)]
pub struct ChainElement {
    pub size: u32,
    pub cert: *const CertContext,
    pub status: TrustStatus,
}

/// CERT_SIMPLE_CHAIN (its start).
#[repr(C)]
pub struct SimpleChain {
    pub size: u32,
    pub status: TrustStatus,
    pub count: u32,
    pub elements: *const *const ChainElement,
}

/// CERT_CHAIN_CONTEXT (its start).
#[repr(C)]
pub struct ChainContext {
    pub size: u32,
    pub status: TrustStatus,
    pub count: u32,
    pub chains: *const *const SimpleChain,
}

/// CERT_CHAIN_POLICY_PARA.
#[repr(C)]
pub struct PolicyPara {
    pub size: u32,
    pub flags: u32,
    pub extra: *const SslPara,
}

/// SSL_EXTRA_CERT_CHAIN_POLICY_PARA.
#[repr(C)]
pub struct SslPara {
    pub size: u32,
    pub auth_type: u32,
    pub checks: u32,
    pub server_name: *const u16,
}

/// CERT_CHAIN_POLICY_STATUS.
#[repr(C)]
pub struct PolicyStatus {
    pub size: u32,
    pub error: u32,
    pub chain_index: i32,
    pub element_index: i32,
    pub extra: *mut c_void,
}

// ---- Checks: the sizes and offsets of the SDK's headers ----

/// `(x86, x64)`: the value for the target being built.
const fn by_width(x86: usize, x64: usize) -> usize {
    if size_of::<usize>() == 4 { x86 } else { x64 }
}

/// A compile-time check that `$value` is the header's for the target.
macro_rules! check {
    ($value:expr, $x86:expr, $x64:expr) => {
        const _: () = assert!($value == by_width($x86, $x64));
    };
}

check!(size_of::<SecHandle>(), 8, 16);
check!(size_of::<SecBuffer>(), 12, 16);
check!(size_of::<SecBufferDesc>(), 12, 16);
check!(size_of::<SchannelCred>(), 56, 80);
check!(offset_of!(SchannelCred, creds), 8, 8);
check!(offset_of!(SchannelCred, alg_count), 24, 40);
check!(offset_of!(SchannelCred, algs), 28, 48);
check!(offset_of!(SchannelCred, enabled_protocols), 32, 56);
check!(offset_of!(SchannelCred, flags), 48, 72);
check!(size_of::<SchCredentials>(), 44, 72);
check!(offset_of!(SchCredentials, cred_count), 8, 8);
check!(offset_of!(SchCredentials, creds), 12, 16);
check!(offset_of!(SchCredentials, flags), 32, 52);
check!(offset_of!(SchCredentials, tls_parameter_count), 36, 56);
check!(offset_of!(SchCredentials, tls_parameters), 40, 64);
check!(size_of::<TlsParameters>(), 24, 40);
check!(offset_of!(TlsParameters, disabled_protocols), 8, 16);
check!(offset_of!(TlsParameters, disabled_crypto_count), 12, 20);
check!(size_of::<CertContext>(), 20, 40);
check!(offset_of!(CertContext, encoded), 4, 8);
check!(offset_of!(CertContext, encoded_size), 8, 16);
check!(size_of::<KeyProvInfo>(), 28, 48);
check!(offset_of!(KeyProvInfo, provider_type), 8, 16);
check!(offset_of!(KeyProvInfo, key_spec), 24, 40);
check!(size_of::<SecPkgInfo>(), 20, 32);
check!(offset_of!(SecPkgInfo, name), 12, 16);
check!(offset_of!(SecPkgInfo, comment), 16, 24);
check!(size_of::<NegotiationInfo>(), 8, 16);
check!(offset_of!(NegotiationInfo, state), 4, 8);
check!(size_of::<NameBlob>(), 8, 16);
check!(size_of::<IssuerListEx>(), 8, 16);
check!(offset_of!(IssuerListEx, count), 4, 8);
check!(size_of::<ChainPara>(), 16, 32);
check!(size_of::<PolicyPara>(), 12, 16);
check!(size_of::<SslPara>(), 16, 24);
check!(size_of::<PolicyStatus>(), 20, 24);
check!(offset_of!(ChainElement, status), 8, 16);
check!(offset_of!(SimpleChain, elements), 16, 16);
check!(offset_of!(ChainContext, chains), 16, 16);

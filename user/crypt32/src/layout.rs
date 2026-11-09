//! The caller's structures with pointer-sized fields that crypt32 reads
//! and writes, typed (`repr(C)` with pointers): one definition gives the
//! x64 layout (System32's crypt32) and the x86 one (SysWOW64's), as
//! wincrypt.h lays them out (ARCHITECTURE "Plan: WOW64"). The structures
//! crypt32 builds itself (CERT_CONTEXT, CERT_INFO, CRL_*, the chain
//! contexts) are typed where they are defined (`cert`, `crl`, `chain`);
//! the checks (`check!`) hold every one of them to the sizes and offsets
//! mingw-w64's wincrypt.h gives for each target. A structure that starts
//! with its cbSize may be an older, shorter version: its fields are read
//! with `get!` only when `has!` says the size covers them.

use core::ffi::c_void;
use core::mem::{offset_of, size_of};

/// Whether a structure of `size` bytes (its cbSize) covers `$field`.
macro_rules! has {
    ($size:expr, $T:ty, $field:ident) => {
        ($size as usize)
            >= core::mem::offset_of!($T, $field) + {
                fn size_of_field<F>(_: impl Fn(&$T) -> &F) -> usize {
                    core::mem::size_of::<F>()
                }
                size_of_field(|t: &$T| &t.$field)
            }
    };
}
pub(crate) use has;

/// Field `$field` of the caller's `$T` at `$p` (a `*const u8`), read
/// unaligned.
macro_rules! get {
    ($p:expr, $T:ty, $field:ident) => {{
        let p = $p.cast::<$T>();
        // SAFETY: the caller's structure, whose size covers the field
        // (`has!`).
        unsafe { core::ptr::addr_of!((*p).$field).read_unaligned() }
    }};
}
pub(crate) use get;

/// CERT_CHAIN_ENGINE_CONFIG (the Windows 7 form: through
/// dwExclusiveFlags; Windows 8's adds hExclusiveRoot and
/// hExclusiveTrustedPeople, Windows 7's dwExclusiveFlags).
#[repr(C)]
pub struct EngineConfig {
    pub size: u32,
    pub restricted_root: *mut c_void,
    pub restricted_trust: *mut c_void,
    pub restricted_other: *mut c_void,
    pub additional_count: u32,
    pub additional: *const *mut c_void,
    pub flags: u32,
    pub url_retrieval_timeout: u32,
    pub max_cached_certificates: u32,
    pub cycle_detection_modulus: u32,
    pub exclusive_root: *mut c_void,
    pub exclusive_trusted_people: *mut c_void,
    pub exclusive_flags: u32,
}

/// The sizes CertCreateCertificateChainEngine accepts: up to
/// CycleDetectionModulus, up to hExclusiveTrustedPeople, the whole.
pub const ENGINE_CONFIG_SIZES: [usize; 3] = [
    offset_of!(EngineConfig, exclusive_root),
    offset_of!(EngineConfig, exclusive_flags),
    size_of::<EngineConfig>(),
];

/// CERT_CHAIN_PARA through its RequestedUsage (a CERT_USAGE_MATCH: the
/// match type, then a CERT_ENHKEY_USAGE).
#[repr(C)]
pub struct ChainPara {
    pub size: u32,
    pub requested: UsageMatch,
}

/// CERT_USAGE_MATCH (pointer-aligned as a whole: at 8 on x64).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct UsageMatch {
    pub kind: u32,
    pub usage: EnhKeyUsage,
}

/// CERT_ENHKEY_USAGE (CTL_USAGE): the count, then the OID strings.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct EnhKeyUsage {
    pub count: u32,
    pub ids: *mut *mut u8,
}

/// CERT_CHAIN_POLICY_PARA.
#[repr(C)]
pub struct PolicyPara {
    pub size: u32,
    pub flags: u32,
    pub extra: *const u8,
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

/// SSL_EXTRA_CERT_CHAIN_POLICY_PARA (HTTPSPolicyCallbackData).
#[repr(C)]
pub struct SslExtraPara {
    pub size: u32,
    pub auth_type: u32,
    pub checks: u32,
    pub server_name: *const u16,
}

/// CERT_REVOCATION_PARA through pftTimeToUse (the fields gala reads).
#[repr(C)]
pub struct RevocationPara {
    pub size: u32,
    pub issuer: *const c_void,
    pub store_count: u32,
    pub stores: *const *mut c_void,
    pub crl_store: *mut c_void,
    pub time: *const u64,
}

/// CERT_REVOCATION_STATUS (no pointers: alike in both).
#[repr(C)]
pub struct RevocationStatus {
    pub size: u32,
    pub index: u32,
    pub error: u32,
    pub reason: u32,
    pub has_freshness: i32,
    pub freshness: u32,
}

/// CRYPT_KEY_PROV_INFO.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct KeyProvInfo {
    pub container: *const u16,
    pub provider: *const u16,
    pub provider_type: u32,
    pub flags: u32,
    pub param_count: u32,
    pub params: *const c_void,
    pub key_spec: u32,
}

/// CERT_KEY_CONTEXT: cbSize, the key (an HCRYPTPROV or an
/// NCRYPT_KEY_HANDLE), its spec.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct KeyContext {
    pub size: u32,
    pub key: usize,
    pub key_spec: u32,
}

/// CERT_ALT_NAME_INFO.
#[repr(C)]
pub struct AltNameInfo {
    pub count: u32,
    pub entries: *mut AltNameEntry,
}

/// CERT_ALT_NAME_ENTRY: the choice, then a string or a blob.
#[repr(C)]
pub struct AltNameEntry {
    pub choice: u32,
    pub value: AltNameValue,
}

/// CERT_ALT_NAME_ENTRY's union (the members gala writes).
#[repr(C)]
pub union AltNameValue {
    pub text: *mut u16,
    pub blob: crate::cert::Blob,
}

// ---- Checks: the sizes and offsets of mingw-w64's wincrypt.h ----

/// `(x86, x64)`: the value for the target being built.
pub(crate) const fn by_width(x86: usize, x64: usize) -> usize {
    if size_of::<usize>() == 4 { x86 } else { x64 }
}

/// A compile-time check that `$value` is wincrypt.h's for the target.
macro_rules! check {
    ($value:expr, $x86:expr, $x64:expr) => {
        const _: () = assert!($value == $crate::layout::by_width($x86, $x64));
    };
}
pub(crate) use check;

use crate::cert::{Algorithm, BitBlob, Blob, CertContext, CertInfo, Extension, PublicKeyInfo};
use crate::crl::{CrlContext, CrlEntry, CrlInfo};

check!(size_of::<Blob>(), 8, 16);
check!(size_of::<Algorithm>(), 12, 24);
check!(size_of::<BitBlob>(), 12, 24);
check!(size_of::<PublicKeyInfo>(), 24, 48);
check!(size_of::<Extension>(), 16, 32);
check!(size_of::<CertInfo>(), 112, 208);
check!(offset_of!(CertInfo, not_before), 32, 64);
check!(offset_of!(CertInfo, not_after), 40, 72);
check!(offset_of!(CertInfo, subject), 48, 80);
check!(offset_of!(CertInfo, public_key), 56, 96);
check!(offset_of!(CertInfo, extension_count), 104, 192);
check!(offset_of!(CertInfo, extensions), 108, 200);
check!(size_of::<CertContext>(), 20, 40);
check!(offset_of!(CertContext, store), 16, 32);
check!(size_of::<CrlEntry>(), 24, 40);
check!(offset_of!(CrlEntry, revoked_at), 8, 16);
check!(size_of::<CrlInfo>(), 56, 96);
check!(offset_of!(CrlInfo, this_update), 24, 48);
check!(offset_of!(CrlInfo, extensions), 52, 88);
check!(size_of::<CrlContext>(), 20, 40);
check!(size_of::<EngineConfig>(), 52, 88);
check!(offset_of!(EngineConfig, restricted_root), 4, 8);
check!(offset_of!(EngineConfig, additional_count), 16, 32);
check!(offset_of!(EngineConfig, additional), 20, 40);
check!(offset_of!(EngineConfig, cycle_detection_modulus), 36, 60);
check!(offset_of!(EngineConfig, exclusive_root), 40, 64);
check!(offset_of!(EngineConfig, exclusive_trusted_people), 44, 72);
check!(offset_of!(EngineConfig, exclusive_flags), 48, 80);
check!(size_of::<ChainPara>(), 16, 32);
check!(offset_of!(ChainPara, requested), 4, 8);
check!(size_of::<UsageMatch>(), 12, 24);
check!(offset_of!(ChainPara, requested) + offset_of!(UsageMatch, usage), 8, 16);
check!(
    offset_of!(ChainPara, requested) + offset_of!(UsageMatch, usage) + offset_of!(EnhKeyUsage, ids),
    12,
    24
);
check!(size_of::<EnhKeyUsage>(), 8, 16);
check!(size_of::<PolicyPara>(), 12, 16);
check!(offset_of!(PolicyPara, extra), 8, 8);
check!(size_of::<PolicyStatus>(), 20, 24);
check!(size_of::<SslExtraPara>(), 16, 24);
check!(offset_of!(SslExtraPara, server_name), 12, 16);
check!(size_of::<RevocationStatus>(), 24, 24);
check!(size_of::<RevocationPara>(), 24, 48);
check!(offset_of!(RevocationPara, issuer), 4, 8);
check!(offset_of!(RevocationPara, store_count), 8, 16);
check!(offset_of!(RevocationPara, stores), 12, 24);
check!(offset_of!(RevocationPara, crl_store), 16, 32);
check!(offset_of!(RevocationPara, time), 20, 40);
check!(size_of::<KeyProvInfo>(), 28, 48);
check!(offset_of!(KeyProvInfo, provider_type), 8, 16);
check!(offset_of!(KeyProvInfo, flags), 12, 20);
check!(offset_of!(KeyProvInfo, param_count), 16, 24);
check!(offset_of!(KeyProvInfo, params), 20, 32);
check!(offset_of!(KeyProvInfo, key_spec), 24, 40);
check!(size_of::<KeyContext>(), 12, 24);
check!(offset_of!(KeyContext, key), 4, 8);
check!(offset_of!(KeyContext, key_spec), 8, 16);
check!(size_of::<AltNameInfo>(), 8, 16);
check!(size_of::<AltNameEntry>(), 12, 24);
check!(offset_of!(AltNameEntry, value), 4, 8);

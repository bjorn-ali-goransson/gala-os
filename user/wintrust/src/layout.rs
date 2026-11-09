//! The structures WinVerifyTrust reads (WINTRUST_DATA, WINTRUST_FILE_INFO)
//! and the provider data it hands out (CRYPT_PROVIDER_DATA and what it
//! points to), typed with pointer-sized fields so one definition is the
//! x64 layout and the x86 one (ARCHITECTURE "Plan: WOW64"). The checks
//! hold them to mingw-w64's wintrust.h for each target, plus the two
//! pointers Windows 8's SDK adds to WINTRUST_DATA (pSignatureSettings)
//! and CRYPT_PROVIDER_DATA (pSigState, pSigSettings), which mingw-w64's
//! header lacks.

use core::ffi::c_void;
use core::mem::{offset_of, size_of};

#[repr(C)]
#[derive(Clone, Copy, PartialEq)]
pub struct Guid(pub u32, pub u16, pub u16, pub [u8; 8]);

/// WINTRUST_DATA (through Windows 8's pSignatureSettings; a caller's
/// cbStruct may stop earlier: `has!`).
#[repr(C)]
pub struct WintrustData {
    pub size: u32,
    pub policy_callback: *mut c_void,
    pub sip_client: *mut c_void,
    pub ui_choice: u32,
    pub revocation: u32,
    pub union_choice: u32,
    pub file: *mut FileInfo,
    pub state_action: u32,
    pub state: *mut c_void,
    pub url_reference: *const u16,
    pub prov_flags: u32,
    pub ui_context: u32,
    pub signature_settings: *mut c_void,
}

/// WINTRUST_FILE_INFO.
#[repr(C)]
pub struct FileInfo {
    pub size: u32,
    pub path: *const u16,
    pub file: usize,
    pub known_subject: *const Guid,
}

/// CRYPT_PROVIDER_DATA (with Windows 8's pSigState and pSigSettings).
#[repr(C)]
pub struct ProvData {
    pub size: u32,
    pub wintrust_data: *mut WintrustData,
    pub opened_file: i32,
    pub window: usize,
    pub action: *const Guid,
    pub prov: usize,
    pub error: u32,
    pub reg_security: u32,
    pub reg_policy: u32,
    pub functions: *mut ProvFunctions,
    pub step_count: u32,
    pub step_errors: *mut u32,
    pub store_count: u32,
    pub stores: *mut *mut c_void,
    pub encoding: u32,
    pub msg: *mut c_void,
    pub signer_count: u32,
    pub signers: *mut Signer,
    pub private_count: u32,
    pub private_data: *mut c_void,
    pub subject_choice: u32,
    pub sip: *mut ProvSip,
    pub usage_oid: *const u8,
    pub recall_with_state: i32,
    pub system_time: [u32; 2],
    pub ctl_signer_usage_oid: *const u8,
    pub prov_flags: u32,
    pub final_error: u32,
    pub request_usage: *mut c_void,
    pub trust_pub_settings: u32,
    pub ui_state: u32,
    pub sig_state: *mut c_void,
    pub sig_settings: *mut c_void,
}

/// CRYPT_PROVIDER_FUNCTIONS: cbStruct and fifteen function pointers.
#[repr(C)]
pub struct ProvFunctions {
    pub size: u32,
    pub alloc: Option<extern "system" fn(u32) -> *mut c_void>,
    pub free: Option<extern "system" fn(*mut c_void)>,
    pub rest: [usize; 13],
}

/// PROVDATA_SIP.
#[repr(C)]
pub struct ProvSip {
    pub size: u32,
    pub subject: Guid,
    pub sip: *mut c_void,
    pub cat_sip: *mut c_void,
    pub subject_info: *mut c_void,
    pub cat_subject_info: *mut c_void,
    pub indirect_data: *mut IndirectData,
}

/// CRYPT_PROVIDER_SGNR.
#[repr(C)]
pub struct Signer {
    pub size: u32,
    pub verify_as_of: [u32; 2],
    pub cert_count: u32,
    pub certs: *mut ProvCert,
    pub signer_type: u32,
    pub info: *mut SignerInfo,
    pub error: u32,
    pub counter_count: u32,
    pub counters: *mut Signer,
    pub chain: *const ChainContext,
}

/// CRYPT_PROVIDER_CERT.
#[repr(C)]
pub struct ProvCert {
    pub size: u32,
    pub cert: *const CertContext,
    pub commercial: i32,
    pub trusted_root: i32,
    pub self_signed: i32,
    pub test_cert: i32,
    pub revoked_reason: u32,
    pub confidence: u32,
    pub error: u32,
    pub trust_list: *mut c_void,
    pub trust_list_signer: i32,
    pub ctl: *mut c_void,
    pub ctl_error: u32,
    pub cyclic: i32,
    pub element: *const ChainElement,
}

// ---- crypt32's structures wintrust reads ----

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Blob {
    pub size: u32,
    pub data: *mut u8,
}

impl Blob {
    pub fn bytes<'a>(&self) -> &'a [u8] {
        if self.data.is_null() {
            return &[];
        }
        // SAFETY: a blob of a structure crypt32 built.
        unsafe { core::slice::from_raw_parts(self.data, self.size as usize) }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Algorithm {
    pub oid: *const u8,
    pub parameters: Blob,
}

/// CRYPT_ATTRIBUTE and CRYPT_ATTRIBUTES.
#[repr(C)]
pub struct Attribute {
    pub oid: *const u8,
    pub count: u32,
    pub values: *const Blob,
}

#[repr(C)]
pub struct Attributes {
    pub count: u32,
    pub attrs: *const Attribute,
}

/// CMSG_SIGNER_INFO.
#[repr(C)]
pub struct SignerInfo {
    pub version: u32,
    pub issuer: Blob,
    pub serial: Blob,
    pub hash_algorithm: Algorithm,
    pub hash_encryption: Algorithm,
    pub encrypted_hash: Blob,
    pub auth: Attributes,
    pub unauth: Attributes,
}

/// SPC_INDIRECT_DATA_CONTENT.
#[repr(C)]
pub struct IndirectData {
    pub data_oid: *const u8,
    pub data_value: Blob,
    pub digest_algorithm: Algorithm,
    pub digest: Blob,
}

/// CERT_CONTEXT (wintrust reads pCertInfo's extensions through crypt32).
#[repr(C)]
pub struct CertContext {
    pub encoding: u32,
    pub encoded: *mut u8,
    pub encoded_len: u32,
    pub info: *mut c_void,
    pub store: *mut c_void,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct TrustStatus {
    pub error: u32,
    pub info: u32,
}

/// CERT_CHAIN_ELEMENT through TrustStatus.
#[repr(C)]
pub struct ChainElement {
    pub size: u32,
    pub cert: *const CertContext,
    pub status: TrustStatus,
}

/// CERT_SIMPLE_CHAIN through rgpElement.
#[repr(C)]
pub struct SimpleChain {
    pub size: u32,
    pub status: TrustStatus,
    pub count: u32,
    pub elements: *const *const ChainElement,
}

/// CERT_CHAIN_CONTEXT through rgpChain.
#[repr(C)]
pub struct ChainContext {
    pub size: u32,
    pub status: TrustStatus,
    pub count: u32,
    pub chains: *const *const SimpleChain,
}

/// CERT_CHAIN_PARA through RequestedUsage (CERT_USAGE_MATCH).
#[repr(C)]
pub struct ChainPara {
    pub size: u32,
    pub requested: UsageMatch,
}

/// CERT_USAGE_MATCH (pointer-aligned as a whole: at 8 on x64).
#[repr(C)]
pub struct UsageMatch {
    pub kind: u32,
    pub usage: EnhKeyUsage,
}

/// CERT_CHAIN_POLICY_PARA, _STATUS and AUTHENTICODE_EXTRA_CERT_CHAIN_POLICY_PARA.
#[repr(C)]
pub struct PolicyPara {
    pub size: u32,
    pub flags: u32,
    pub extra: *const c_void,
}

#[repr(C)]
pub struct PolicyStatus {
    pub size: u32,
    pub error: u32,
    pub chain_index: i32,
    pub element_index: i32,
    pub extra: *mut c_void,
}

#[repr(C)]
pub struct AuthenticodeExtra {
    pub size: u32,
    pub reg_policy: u32,
    pub signer_info: *const SignerInfo,
}

/// CERT_CTRL... CMSG_CTRL_VERIFY_SIGNATURE_EX_PARA.
#[repr(C)]
pub struct VerifyExPara {
    pub size: u32,
    pub prov: usize,
    pub signer_index: u32,
    pub signer_type: u32,
    pub signer: *const c_void,
}

/// CRYPT_TIMESTAMP_CONTEXT and CRYPT_TIMESTAMP_INFO through ftTime.
#[repr(C)]
pub struct TimestampContext {
    pub encoded_len: u32,
    pub encoded: *mut u8,
    pub info: *const TimestampInfo,
}

#[repr(C)]
pub struct TimestampInfo {
    pub version: u32,
    pub policy: *const u8,
    pub hash_algorithm: Algorithm,
    pub hashed_message: Blob,
    pub serial: Blob,
    pub time: [u32; 2],
}

/// CERT_ENHKEY_USAGE.
#[repr(C)]
pub struct EnhKeyUsage {
    pub count: u32,
    pub ids: *const *const u8,
}

/// Whether a structure of `size` bytes (its cbStruct) covers `$field`.
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

// ---- Checks: wintrust.h's and wincrypt.h's sizes and offsets ----

const fn by_width(x86: usize, x64: usize) -> usize {
    if size_of::<usize>() == 4 { x86 } else { x64 }
}

macro_rules! check {
    ($value:expr, $x86:expr, $x64:expr) => {
        const _: () = assert!($value == by_width($x86, $x64));
    };
}

// mingw-w64's WINTRUST_DATA is 48/80 without pSignatureSettings.
check!(offset_of!(WintrustData, signature_settings), 48, 80);
check!(size_of::<WintrustData>(), 52, 88);
check!(offset_of!(WintrustData, state_action), 28, 48);
check!(offset_of!(WintrustData, state), 32, 56);
check!(offset_of!(WintrustData, prov_flags), 40, 72);
check!(size_of::<FileInfo>(), 16, 32);
// mingw-w64's CRYPT_PROVIDER_DATA is 128/224 without pSigState and
// pSigSettings.
check!(offset_of!(ProvData, sig_state), 128, 224);
check!(size_of::<ProvData>(), 136, 240);
check!(offset_of!(ProvData, step_errors), 44, 80);
check!(offset_of!(ProvData, signers), 68, 128);
check!(offset_of!(ProvData, sip), 84, 160);
check!(offset_of!(ProvData, system_time), 96, 180);
check!(offset_of!(ProvData, request_usage), 116, 208);
check!(offset_of!(ProvData, ui_state), 124, 220);
check!(size_of::<ProvFunctions>(), 64, 128);
check!(size_of::<ProvSip>(), 40, 64);
check!(offset_of!(ProvSip, indirect_data), 36, 56);
check!(size_of::<Signer>(), 44, 64);
check!(offset_of!(Signer, certs), 16, 16);
check!(offset_of!(Signer, info), 24, 32);
check!(offset_of!(Signer, chain), 40, 56);
check!(size_of::<ProvCert>(), 60, 88);
check!(offset_of!(ProvCert, confidence), 28, 36);
check!(offset_of!(ProvCert, element), 56, 80);
check!(size_of::<SignerInfo>(), 68, 136);
check!(offset_of!(SignerInfo, encrypted_hash), 44, 88);
check!(offset_of!(SignerInfo, unauth), 60, 120);
check!(size_of::<IndirectData>(), 32, 64);
check!(size_of::<AuthenticodeExtra>(), 12, 16);
check!(size_of::<VerifyExPara>(), 20, 32);
check!(size_of::<TimestampContext>(), 12, 24);
check!(offset_of!(TimestampInfo, time), 36, 72);
check!(size_of::<ChainPara>(), 16, 32);
check!(offset_of!(ChainPara, requested) + offset_of!(UsageMatch, usage), 8, 16);
check!(size_of::<PolicyStatus>(), 20, 24);

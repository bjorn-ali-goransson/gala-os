//! WinVerifyTrust with WINTRUST_ACTION_GENERIC_VERIFY_V2 over a file, and
//! the WTHelper functions over the provider data it keeps.
//!
//! The steps are Windows' (as Microsoft documents CRYPT_PROVIDER_DATA's
//! padwTrustStepErrors and Wine's softpub.c orders them); each sets its
//! TRUSTERROR_STEP_FINAL_* entry and the first failing one is the result:
//! - object (OBJPROV): the file is a PE image with a certificate table
//!   (`pe::examine`), its WIN_CERTIFICATE decodes as a SignedData
//!   (CryptMsgOpenToDecode/Update), a store of its certificates
//!   (CERT_STORE_PROV_MSG), its content is an SpcIndirectDataContent for
//!   a PE image (CryptDecodeObjectEx) whose digest is the image hash
//!   (TRUST_E_BAD_DIGEST);
//! - signature (SIGPROV): the signer (CMSG_SIGNER_INFO), its certificate
//!   among the message's (TRUST_E_NO_SIGNER_CERT), its signature
//!   (CryptMsgControl, CMSG_CTRL_VERIFY_SIGNATURE_EX: crypt32's error, e.g.
//!   NTE_BAD_SIGNATURE), its RFC 3161 timestamp (unsigned attribute
//!   1.3.6.1.4.1.311.3.3.1: CryptVerifyTimeStampSignature over the
//!   signer's encrypted digest; TRUST_E_TIME_STAMP if it fails), whose time
//!   becomes the signer's sftVerifyAsOf and whose signer the counter signer;
//!   a signer whose signature does not verify is TRUST_E_NOSIGNATURE
//!   (crypt32's error in its dwError), unless the signer carries a nested
//!   signature (unauthenticated attribute 1.3.6.1.4.1.311.2.4.1,
//!   Authenticode's dual signing): then, as Windows does, that signature
//!   replaces the primary (its message, its digest against the same
//!   subject, its signer) and the steps go on with it, the primary's
//!   error standing if it fails too;
//! - certificate (CERTPROV): the signer's chain at sftVerifyAsOf with the
//!   code signing usage, the counter signer's at the same time with the
//!   time stamping usage (CertGetCertificateChain, the default engine,
//!   the message's certificates as the additional store), copied into the
//!   CRYPT_PROVIDER_CERTs;
//! - policy (POLICYPROV): CERT_CHAIN_POLICY_AUTHENTICODE on the signer's
//!   chain and AUTHENTICODE_TS on the counter signer's, with Windows'
//!   default Software Publishing state (0x23c00: an offline revocation
//!   check is accepted, revocation is not checked for the timestamp).
//!
//! WTD_HASH_ONLY_FLAG stops after the object step and answers anything
//! but a matching image hash with TRUST_E_NOSIGNATURE, as Windows does in
//! Wine's test_wintrust_digest. Every failed step is recorded
//! (`wintrust.dll!WinVerifyTrust: <step>`, the error as detail), so a
//! verification that fails says where.
//!
//! The state: one heap block per WTD_STATEACTION_VERIFY (`State`, the
//! CRYPT_PROVIDER_DATA first so its address is the hWVTStateData handle),
//! holding the provider data, the step errors, one signer, one counter
//! signer and up to 8 certificates of each chain; it owns the message,
//! the store, the chains, the certificate references, the decoded
//! structures and the file handle it opened, all freed by
//! WTD_STATEACTION_CLOSE (or at once with WTD_STATEACTION_IGNORE).
//!
//! Not here (recorded): other actions (TRUST_E_PROVIDER_UNKNOWN), other
//! subjects than a file (catalog members, blobs, signers, certificates)
//! and other files than PE images (cabinets, catalogs: no SIPs), more
//! than one signer or WIN_CERTIFICATE, the secondary signatures
//! (WINTRUST_SIGNATURE_SETTINGS: the nested signature serves only as
//! the fallback below),
//! legacy PKCS #9 countersignatures (the chain is checked at the current
//! time instead), the Disallowed store (gala's is empty), the registry's
//! Software Publishing state (its default is used), any UI.

use core::ffi::c_void;

use crate::cab;
use crate::layout::{
    AuthenticodeExtra, Blob, ChainContext, ChainElement, ChainPara, EnhKeyUsage, FileInfo, Guid,
    IndirectData, PolicyPara, PolicyStatus, ProvCert, ProvData, ProvFunctions, ProvSip, Signer,
    SignerInfo, TimestampContext, UsageMatch, VerifyExPara, WintrustData, has,
};
use crate::pe::{self, TRUST_E_NOSIGNATURE, TRUST_E_SUBJECT_FORM_UNKNOWN};
use crate::{observed, unsupported};

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        security: *const c_void,
        disposition: u32,
        flags: u32,
        template: usize,
    ) -> usize;
    fn CloseHandle(h: usize) -> i32;
    fn GetFileSizeEx(h: usize, size: *mut i64) -> i32;
    fn GetLastError() -> u32;
    fn SetLastError(code: u32);
    fn GetProcessHeap() -> usize;
    fn HeapAlloc(heap: usize, flags: u32, size: usize) -> *mut u8;
    fn HeapFree(heap: usize, flags: u32, p: *mut u8) -> i32;
    fn LocalFree(p: *mut c_void) -> *mut c_void;
    fn GetSystemTimeAsFileTime(time: *mut u64);
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "crypt32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "crypt32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn CryptMsgOpenToDecode(
        encoding: u32,
        flags: u32,
        kind: u32,
        prov: usize,
        recipient: *const c_void,
        stream: *const c_void,
    ) -> *mut c_void;
    fn CryptMsgUpdate(msg: *mut c_void, data: *const u8, len: u32, last: i32) -> i32;
    fn CryptMsgGetParam(
        msg: *mut c_void,
        param: u32,
        index: u32,
        out: *mut c_void,
        len: *mut u32,
    ) -> i32;
    fn CryptMsgControl(msg: *mut c_void, flags: u32, control: u32, para: *const c_void) -> i32;
    fn CryptMsgClose(msg: *mut c_void) -> i32;
    fn CertOpenStore(
        provider: *const u8,
        encoding: u32,
        prov: usize,
        flags: u32,
        para: *const c_void,
    ) -> *mut c_void;
    fn CertCloseStore(store: *mut c_void, flags: u32) -> i32;
    fn CertGetSubjectCertificateFromStore(
        store: *mut c_void,
        encoding: u32,
        info: *const c_void,
    ) -> *const crate::layout::CertContext;
    fn CertDuplicateCertificateContext(
        c: *const crate::layout::CertContext,
    ) -> *const crate::layout::CertContext;
    fn CertFreeCertificateContext(c: *const crate::layout::CertContext) -> i32;
    fn CertGetEnhancedKeyUsage(
        c: *const crate::layout::CertContext,
        flags: u32,
        usage: *mut u8,
        len: *mut u32,
    ) -> i32;
    fn CertGetCertificateChain(
        engine: *mut c_void,
        cert: *const crate::layout::CertContext,
        time: *const u64,
        additional: *mut c_void,
        para: *const ChainPara,
        flags: u32,
        reserved: *mut c_void,
        chain: *mut *const ChainContext,
    ) -> i32;
    fn CertVerifyCertificateChainPolicy(
        policy: usize,
        chain: *const ChainContext,
        para: *const PolicyPara,
        status: *mut PolicyStatus,
    ) -> i32;
    fn CertFreeCertificateChain(chain: *const ChainContext);
    fn CryptDecodeObjectEx(
        encoding: u32,
        kind: *const u8,
        data: *const u8,
        len: u32,
        flags: u32,
        para: *const c_void,
        out: *mut c_void,
        out_len: *mut u32,
    ) -> i32;
    fn CryptVerifyTimeStampSignature(
        token: *const u8,
        token_len: u32,
        data: *const u8,
        data_len: u32,
        additional: *mut c_void,
        context: *mut *mut TimestampContext,
        signer: *mut *const crate::layout::CertContext,
        store: *mut *mut c_void,
    ) -> i32;
    fn CryptMemFree(p: *mut c_void);
}

/// WINTRUST_ACTION_GENERIC_VERIFY_V2.
const GENERIC_VERIFY_V2: Guid =
    Guid(0x00aa_c56b, 0xcd44, 0x11d0, [0x8c, 0xc2, 0x00, 0xc0, 0x4f, 0xc2, 0x95, 0xee]);
/// The PE image SIP's subject GUID (C689AAB8-8E78-11D0-8C47-00C04FC295EE).
const PE_SUBJECT: Guid =
    Guid(0xc689_aab8, 0x8e78, 0x11d0, [0x8c, 0x47, 0x00, 0xc0, 0x4f, 0xc2, 0x95, 0xee]);
/// The cabinet SIP's subject GUID (C689AABA-8E78-11D0-8C47-00C04FC295EE,
/// Wine's sip.c tests).
const CAB_SUBJECT: Guid =
    Guid(0xc689_aaba, 0x8e78, 0x11d0, [0x8c, 0x47, 0x00, 0xc0, 0x4f, 0xc2, 0x95, 0xee]);

/// The file being verified: a PE image or a cabinet.
enum Subject {
    Pe(pe::Image),
    Cab(cab::Cabinet),
}

const ERROR_INVALID_PARAMETER: u32 = 87;
const ERROR_NOT_ENOUGH_MEMORY: u32 = 8;
const TRUST_E_PROVIDER_UNKNOWN: u32 = 0x800b_0001;
const TRUST_E_NO_SIGNER_CERT: u32 = 0x8009_6002;
const TRUST_E_TIME_STAMP: u32 = 0x8009_6005;
const TRUST_E_BAD_DIGEST: u32 = 0x8009_6010;
const TRUST_E_SYSTEM_ERROR: u32 = 0x8009_6001;
const NTE_BAD_ALGID: u32 = 0x8009_0008;

const X509_PKCS7: u32 = 0x1_0001;
const WTD_UI_NONE: u32 = 2;
const WTD_REVOKE_WHOLECHAIN: u32 = 1;
const WTD_CHOICE_FILE: u32 = 1;
const WTD_STATEACTION_IGNORE: u32 = 0;
const WTD_STATEACTION_VERIFY: u32 = 1;
const WTD_STATEACTION_CLOSE: u32 = 2;
const WTD_STATEACTION_AUTO_CACHE: u32 = 3;
const WTD_STATEACTION_AUTO_CACHE_FLUSH: u32 = 4;
const WTD_REVOCATION_CHECK_NONE: u32 = 0x10;
const WTD_REVOCATION_CHECK_END_CERT: u32 = 0x20;
const WTD_REVOCATION_CHECK_CHAIN: u32 = 0x40;
const WTD_REVOCATION_CHECK_CHAIN_EXCLUDE_ROOT: u32 = 0x80;
const WTD_HASH_ONLY_FLAG: u32 = 0x200;
const WTD_LIFETIME_SIGNING_FLAG: u32 = 0x800;

const TRUSTERROR_MAX_STEPS: usize = 38;
const STEP_OBJPROV: usize = 32;
const STEP_SIGPROV: usize = 33;
const STEP_CERTPROV: usize = 34;
const STEP_POLICYPROV: usize = 36;

/// Windows' default Software Publishing state (WTPF_OFFLINEOK_IND, _COM,
/// OFFLINEOKNBU_IND, _COM, WTPF_IGNOREREVOCATIONONTS).
const DEFAULT_POLICY_STATE: u32 = 0x23c00;

// CryptMsgGetParam's parameters.
const CMSG_INNER_CONTENT_TYPE_PARAM: u32 = 4;
const CMSG_CONTENT_PARAM: u32 = 2;
const CMSG_SIGNER_COUNT_PARAM: u32 = 5;
const CMSG_SIGNER_INFO_PARAM: u32 = 6;
const CMSG_SIGNER_CERT_INFO_PARAM: u32 = 7;
const CMSG_CTRL_VERIFY_SIGNATURE_EX: u32 = 19;
const CMSG_VERIFY_SIGNER_CERT: u32 = 2;

// CERT_TRUST_STATUS bits the provider certificates are made from.
const IS_NOT_TIME_VALID: u32 = 0x1;
const IS_NOT_TIME_NESTED: u32 = 0x2;
const IS_REVOKED: u32 = 0x4;
const IS_NOT_SIGNATURE_VALID: u32 = 0x8;
const IS_NOT_VALID_FOR_USAGE: u32 = 0x10;
const IS_UNTRUSTED_ROOT: u32 = 0x20;
const REVOCATION_UNKNOWN: u32 = 0x40;
const IS_CYCLIC: u32 = 0x80;
const IS_SELF_SIGNED: u32 = 0x8;

const MAX_CERTS: usize = 8;
const STATE_MAGIC: u32 = 0x6761_6c77; // "galw"

/// One verification's state (module comment).
#[repr(C)]
struct State {
    data: ProvData,
    magic: u32,
    steps: [u32; TRUSTERROR_MAX_STEPS],
    functions: ProvFunctions,
    sip: ProvSip,
    signer: Signer,
    counter: Signer,
    certs: [ProvCert; MAX_CERTS],
    counter_certs: [ProvCert; MAX_CERTS],
    store: *mut c_void,
    counter_chain: *const ChainContext,
    timestamp: *mut TimestampContext,
    timestamp_store: *mut c_void,
    counter_info: *mut SignerInfo,
    file: usize,
}

fn heap_alloc(size: usize) -> *mut u8 {
    // SAFETY: plain allocation, zeroed.
    unsafe { HeapAlloc(GetProcessHeap(), 8, size.max(1)) }
}

fn heap_free(p: *mut c_void) {
    if !p.is_null() {
        // SAFETY: a block of ours.
        unsafe { HeapFree(GetProcessHeap(), 0, p.cast()) };
    }
}

extern "system" fn provider_alloc(size: u32) -> *mut c_void {
    heap_alloc(size as usize).cast()
}

extern "system" fn provider_free(p: *mut c_void) {
    heap_free(p);
}

fn state<'a>(h: *mut c_void) -> Option<&'a mut State> {
    if h.is_null() {
        return None;
    }
    // SAFETY: a handle WinVerifyTrust gave starts with our State, whose
    // magic we check.
    let s = unsafe { &mut *h.cast::<State>() };
    (s.magic == STATE_MAGIC).then_some(s)
}

/// The ledger line of a failed step: `wintrust.dll!WinVerifyTrust: <what>`.
fn failed(what: &[u8], error: u32) -> u32 {
    let mut text = [0u8; 96];
    let mut n = 0;
    for part in [&b"wintrust.dll!WinVerifyTrust: "[..], what] {
        let part = &part[..part.len().min(text.len() - n)];
        text[n..n + part.len()].copy_from_slice(part);
        n += part.len();
    }
    observed(&text[..n], error.into());
    error
}

/// A NUL-terminated C string as bytes.
fn c_text<'a>(p: *const u8) -> &'a [u8] {
    if p.is_null() {
        return &[];
    }
    let mut n = 0;
    // SAFETY: a NUL-terminated string crypt32 built.
    unsafe {
        while *p.add(n) != 0 {
            n += 1;
        }
        core::slice::from_raw_parts(p, n)
    }
}

/// A message parameter in a new heap block (freed by the caller).
fn param(msg: *mut c_void, which: u32, index: u32) -> Result<*mut u8, u32> {
    let mut len = 0u32;
    // SAFETY: crypt32's documented size query; then our block.
    unsafe {
        if CryptMsgGetParam(msg, which, index, core::ptr::null_mut(), &mut len) == 0 {
            return Err(GetLastError());
        }
        let p = heap_alloc(len as usize);
        if p.is_null() {
            return Err(ERROR_NOT_ENOUGH_MEMORY);
        }
        if CryptMsgGetParam(msg, which, index, p.cast(), &mut len) == 0 {
            let e = GetLastError();
            heap_free(p.cast());
            return Err(e);
        }
        Ok(p)
    }
}

fn now() -> u64 {
    let mut t = 0u64;
    // SAFETY: our FILETIME.
    unsafe { GetSystemTimeAsFileTime(&mut t) };
    t
}

fn as_parts(t: u64) -> [u32; 2] {
    [t as u32, (t >> 32) as u32]
}

fn as_time(p: [u32; 2]) -> u64 {
    u64::from(p[0]) | u64::from(p[1]) << 32
}

/// WinVerifyTrust(window, action, data): GENERIC_VERIFY_V2 over a file
/// (module comment). The result is also the last error when it is not
/// S_OK; a close leaves the last error.
#[unsafe(no_mangle)]
pub extern "system" fn WinVerifyTrust(
    window: usize,
    action: *const Guid,
    data: *mut c_void,
) -> i32 {
    let data = data.cast::<WintrustData>();
    if action.is_null() || data.is_null() {
        // SAFETY: plain call.
        unsafe { SetLastError(ERROR_INVALID_PARAMETER) };
        return ERROR_INVALID_PARAMETER as i32;
    }
    // SAFETY: the caller's GUID.
    let action_guid = unsafe { action.read_unaligned() };
    if action_guid != GENERIC_VERIFY_V2 {
        unsupported(b"wintrust.dll!WinVerifyTrust(action)", action_guid.0.into());
        // SAFETY: plain call.
        unsafe { SetLastError(TRUST_E_PROVIDER_UNKNOWN) };
        return TRUST_E_PROVIDER_UNKNOWN as i32;
    }
    // SAFETY: the caller's WINTRUST_DATA (its cbStruct says how much).
    let size = unsafe { (*data).size };
    let state_action = if has!(size, WintrustData, state_action) {
        // SAFETY: as above.
        unsafe { (*data).state_action }
    } else {
        WTD_STATEACTION_IGNORE
    };
    let r = match state_action {
        WTD_STATEACTION_CLOSE => {
            // SAFETY: as above.
            unsafe {
                close(&mut *data);
            }
            return 0;
        }
        WTD_STATEACTION_AUTO_CACHE_FLUSH => {
            unsupported(b"wintrust.dll!WinVerifyTrust(WTD_STATEACTION_AUTO_CACHE_FLUSH)", 0);
            return 0;
        }
        WTD_STATEACTION_VERIFY | WTD_STATEACTION_IGNORE | WTD_STATEACTION_AUTO_CACHE => {
            if state_action == WTD_STATEACTION_AUTO_CACHE {
                unsupported(
                    b"wintrust.dll!WinVerifyTrust(WTD_STATEACTION_AUTO_CACHE: no cache)",
                    0,
                );
            }
            // SAFETY: as above.
            let data = unsafe { &mut *data };
            let keep = state_action == WTD_STATEACTION_VERIFY;
            run(window, action, data, keep)
        }
        _ => {
            unsupported(b"wintrust.dll!WinVerifyTrust(state action)", state_action.into());
            ERROR_INVALID_PARAMETER
        }
    };
    if r != 0 {
        // SAFETY: plain call.
        unsafe { SetLastError(r) };
    }
    r as i32
}

/// WinVerifyTrustEx: WinVerifyTrust.
#[unsafe(no_mangle)]
pub extern "system" fn WinVerifyTrustEx(
    window: usize,
    action: *const Guid,
    data: *mut c_void,
) -> i32 {
    WinVerifyTrust(window, action, data)
}

/// Frees a WTD_STATEACTION_VERIFY's state and clears hWVTStateData.
fn close(data: &mut WintrustData) {
    if !has!(data.size, WintrustData, state) {
        return;
    }
    if let Some(s) = state(data.state) {
        free_state(s);
    }
    data.state = core::ptr::null_mut();
}

fn free_certs(certs: &mut [ProvCert]) {
    for c in certs {
        if !c.cert.is_null() {
            // SAFETY: a reference we took.
            unsafe { CertFreeCertificateContext(c.cert) };
            c.cert = core::ptr::null();
        }
    }
}

/// Releases what the message, signature, certificate and policy steps
/// took (the message, its store, the signer and counter signer, their
/// chains and certificates, the timestamp) and clears them, so the state
/// can verify another signature (the nested one) or be freed.
fn reset_message(s: &mut State) {
    // SAFETY: everything below is ours (module comment); each pointer is
    // cleared after it is released.
    unsafe {
        free_certs(&mut s.certs);
        free_certs(&mut s.counter_certs);
        if !s.signer.chain.is_null() {
            CertFreeCertificateChain(s.signer.chain);
        }
        if !s.counter_chain.is_null() {
            CertFreeCertificateChain(s.counter_chain);
        }
        heap_free(s.signer.info.cast());
        heap_free(s.counter_info.cast());
        if !s.sip.indirect_data.is_null() {
            LocalFree(s.sip.indirect_data.cast());
        }
        if !s.timestamp.is_null() {
            CryptMemFree(s.timestamp.cast());
        }
        if !s.timestamp_store.is_null() {
            CertCloseStore(s.timestamp_store, 0);
        }
        if !s.store.is_null() {
            CertCloseStore(s.store, 0);
        }
        if !s.data.msg.is_null() {
            CryptMsgClose(s.data.msg);
        }
        // All-zero is a valid value of each (integers and raw pointers).
        core::ptr::write_bytes(&raw mut s.signer, 0, 1);
        core::ptr::write_bytes(&raw mut s.counter, 0, 1);
        core::ptr::write_bytes(s.certs.as_mut_ptr(), 0, MAX_CERTS);
        core::ptr::write_bytes(s.counter_certs.as_mut_ptr(), 0, MAX_CERTS);
    }
    s.counter_chain = core::ptr::null();
    s.counter_info = core::ptr::null_mut();
    s.sip.indirect_data = core::ptr::null_mut();
    s.timestamp = core::ptr::null_mut();
    s.timestamp_store = core::ptr::null_mut();
    s.store = core::ptr::null_mut();
    s.data.msg = core::ptr::null_mut();
    s.data.store_count = 0;
    s.data.stores = core::ptr::null_mut();
    s.data.signer_count = 0;
    s.data.signers = core::ptr::null_mut();
}

fn free_state(s: &mut State) {
    reset_message(s);
    // SAFETY: our file handle and block (module comment).
    unsafe {
        if s.file != 0 {
            CloseHandle(s.file);
        }
        s.magic = 0;
        heap_free((s as *mut State).cast());
    }
}

/// The primary signer's nested signature (an unauthenticated attribute
/// szOID_NESTED_SIGNATURE, 1.3.6.1.4.1.311.2.4.1, whose value is a
/// ContentInfo holding a SignedData: Authenticode's dual signing), copied
/// out of the signer info the reset frees.
fn nested_signature(s: &State) -> Option<pe::Heap> {
    if s.signer.info.is_null() {
        return None;
    }
    // SAFETY: the CMSG_SIGNER_INFO crypt32 built.
    let info = unsafe { &*s.signer.info };
    for i in 0..info.unauth.count as usize {
        // SAFETY: its attribute array.
        let a = unsafe { &*info.unauth.attrs.add(i) };
        if c_text(a.oid) == b"1.3.6.1.4.1.311.2.4.1" && a.count > 0 {
            // SAFETY: its value array.
            let value = unsafe { &*a.values }.bytes();
            let mut copy = pe::Heap::new(value.len())?;
            copy.bytes().copy_from_slice(value);
            return Some(copy);
        }
    }
    None
}

/// One verification: a new state, its steps, kept (VERIFY: hWVTStateData)
/// or freed.
fn run(window: usize, action: *const Guid, data: &mut WintrustData, keep: bool) -> u32 {
    let p = heap_alloc(core::mem::size_of::<State>()).cast::<State>();
    if p.is_null() {
        return ERROR_NOT_ENOUGH_MEMORY;
    }
    // SAFETY: our zeroed block; the pointers set point into it.
    let s = unsafe { &mut *p };
    s.magic = STATE_MAGIC;
    s.functions.size = core::mem::size_of::<ProvFunctions>() as u32;
    s.functions.alloc = Some(provider_alloc);
    s.functions.free = Some(provider_free);
    s.sip.size = core::mem::size_of::<ProvSip>() as u32;
    let d = &mut s.data;
    d.size = core::mem::size_of::<ProvData>() as u32;
    d.wintrust_data = data;
    d.window = window;
    d.action = action;
    d.reg_policy = DEFAULT_POLICY_STATE;
    d.functions = &raw mut s.functions;
    d.step_count = TRUSTERROR_MAX_STEPS as u32;
    d.step_errors = s.steps.as_mut_ptr();
    d.encoding = X509_PKCS7;
    d.subject_choice = WTD_CHOICE_FILE;
    d.sip = &raw mut s.sip;
    d.system_time = as_parts(now());
    d.prov_flags = if has!(data.size, WintrustData, prov_flags) { data.prov_flags } else { 0 };
    if has!(data.size, WintrustData, ui_choice) && data.ui_choice != WTD_UI_NONE {
        unsupported(b"wintrust.dll!WinVerifyTrust(UI: none shown)", data.ui_choice.into());
    }
    if has!(data.size, WintrustData, signature_settings) && !data.signature_settings.is_null() {
        unsupported(b"wintrust.dll!WinVerifyTrust(signature settings: primary only)", 0);
    }
    let hash_only = d.prov_flags & WTD_HASH_ONLY_FLAG != 0;
    let subject = object(s, data);
    let mut result = subject.as_ref().map(|_| ()).map_err(|&e| e);
    if let Err(e) = result {
        s.steps[STEP_OBJPROV] = e;
    }
    if hash_only {
        result = result.map_err(|_| TRUST_E_NOSIGNATURE);
    } else if let Ok((h, subject)) = &subject {
        result = signature(s);
        if result.is_err()
            && let Some(mut nested) = nested_signature(s)
        {
            // Windows falls back to the nested signature when the primary
            // signer's does not verify (issue #57): its message replaces
            // the primary's and is checked in full; if it fails too, the
            // primary's error stands.
            observed(b"wintrust.dll!WinVerifyTrust: nested signature tried", 0);
            reset_message(s);
            if let Err(e) = message(s, *h, subject, nested.bytes()).and_then(|()| signature(s)) {
                failed(b"nested signature", e);
            } else {
                result = Ok(());
            }
        }
        if let Err(e) = result {
            s.steps[STEP_SIGPROV] = e;
        }
        for (step, f) in [
            (STEP_CERTPROV, certificate as fn(&mut State) -> Result<(), u32>),
            (STEP_POLICYPROV, policy),
        ] {
            if result.is_err() {
                break;
            }
            result = f(s);
            if let Err(e) = result {
                s.steps[step] = e;
            }
        }
    }
    let r = result.err().unwrap_or(0);
    s.data.final_error = r;
    if keep && has!(data.size, WintrustData, state) {
        data.state = p.cast();
    } else {
        free_state(s);
    }
    r
}

/// The object step (module comment).
fn object(s: &mut State, data: &mut WintrustData) -> Result<(usize, Subject), u32> {
    if !has!(data.size, WintrustData, file) || data.union_choice != WTD_CHOICE_FILE {
        // The detail: cbStruct in the high half, dwUnionChoice (when the
        // structure has it) in the low.
        let choice =
            if has!(data.size, WintrustData, union_choice) { data.union_choice } else { 0 };
        unsupported(
            b"wintrust.dll!WinVerifyTrust(subject choice)",
            u64::from(data.size) << 32 | u64::from(choice),
        );
        return Err(failed(b"subject choice", TRUST_E_SUBJECT_FORM_UNKNOWN));
    }
    let f: &FileInfo = match unsafe { data.file.as_ref() } {
        // SAFETY: the caller's WINTRUST_FILE_INFO, if any (above).
        Some(f) => f,
        None => return Err(failed(b"no file", ERROR_INVALID_PARAMETER)),
    };
    if has!(f.size, FileInfo, known_subject) && !f.known_subject.is_null() {
        // SAFETY: the caller's GUID.
        let subject = unsafe { f.known_subject.read_unaligned() };
        if subject != PE_SUBJECT && subject != CAB_SUBJECT {
            unsupported(b"wintrust.dll!WinVerifyTrust(known subject)", subject.0.into());
            return Err(failed(b"known subject", TRUST_E_SUBJECT_FORM_UNKNOWN));
        }
    }
    let mut h = if has!(f.size, FileInfo, file) { f.file } else { 0 };
    if h == 0 || h == usize::MAX {
        // SAFETY: the caller's NUL-terminated path; GENERIC_READ,
        // FILE_SHARE_READ, OPEN_EXISTING.
        h = unsafe { CreateFileW(f.path, 0x8000_0000, 1, core::ptr::null(), 3, 0, 0) };
        if h == usize::MAX {
            // SAFETY: plain call.
            return Err(failed(b"open", unsafe { GetLastError() }));
        }
        s.file = h;
        s.data.opened_file = 1;
    }
    let mut size = 0i64;
    // SAFETY: the file handle.
    if unsafe { GetFileSizeEx(h, &mut size) } == 0 {
        // SAFETY: plain call.
        return Err(failed(b"file size", unsafe { GetLastError() }));
    }
    let size = size as u64;
    let subject = if cab::is_cabinet(h, size) {
        s.sip.subject = CAB_SUBJECT;
        Subject::Cab(cab::examine(h, size).map_err(|e| failed(b"cabinet not signed", e))?)
    } else {
        let image = pe::examine(h, size).map_err(|e| failed(b"subject or certificate table", e))?;
        s.sip.subject = PE_SUBJECT;
        Subject::Pe(image)
    };
    let (mut table, at, len) = match &subject {
        Subject::Pe(image) => pe::signature(h, image).map_err(|e| failed(b"WIN_CERTIFICATE", e))?,
        Subject::Cab(c) => cab::signature(h, c).map_err(|e| failed(b"cabinet signature", e))?,
    };
    message(s, h, &subject, &table.bytes()[at..at + len])?;
    Ok((h, subject))
}

/// The object step's message part: `der` (a PKCS #7 ContentInfo holding
/// a SignedData) decoded into the state's message and store, its
/// SpcIndirectDataContent's digest compared with the subject's hash.
fn message(s: &mut State, h: usize, subject: &Subject, der: &[u8]) -> Result<(), u32> {
    // SAFETY: crypt32's documented calls; our buffers.
    unsafe {
        let msg = CryptMsgOpenToDecode(X509_PKCS7, 0, 0, 0, core::ptr::null(), core::ptr::null());
        if msg.is_null() {
            return Err(failed(b"CryptMsgOpenToDecode", GetLastError()));
        }
        s.data.msg = msg;
        if CryptMsgUpdate(msg, der.as_ptr(), der.len() as u32, 1) == 0 {
            return Err(failed(b"signature does not decode", GetLastError()));
        }
        // CERT_STORE_PROV_MSG.
        let store = CertOpenStore(core::ptr::without_provenance(1), X509_PKCS7, 0, 0, msg);
        if store.is_null() {
            return Err(failed(b"message store", GetLastError()));
        }
        s.store = store;
        s.data.store_count = 1;
        s.data.stores = &raw mut s.store;
        let kind =
            param(msg, CMSG_INNER_CONTENT_TYPE_PARAM, 0).map_err(|e| failed(b"content type", e))?;
        let spc = c_text(kind) == b"1.3.6.1.4.1.311.2.1.4";
        heap_free(kind.cast());
        if !spc {
            unsupported(b"wintrust.dll!WinVerifyTrust(content not SpcIndirectDataContent)", 0);
            return Err(failed(b"content type", TRUST_E_NOSIGNATURE));
        }
        let mut len = 0u32;
        let content = param(msg, CMSG_CONTENT_PARAM, 0).map_err(|e| failed(b"content", e))?;
        let mut clen = 0u32;
        CryptMsgGetParam(msg, CMSG_CONTENT_PARAM, 0, core::ptr::null_mut(), &mut clen);
        let mut indirect: *mut IndirectData = core::ptr::null_mut();
        let ok = CryptDecodeObjectEx(
            X509_PKCS7,
            c"1.3.6.1.4.1.311.2.1.4".as_ptr().cast(),
            content,
            clen,
            0x8000, // CRYPT_DECODE_ALLOC_FLAG
            core::ptr::null(),
            (&raw mut indirect).cast(),
            &mut len,
        );
        heap_free(content.cast());
        if ok == 0 {
            return Err(failed(b"SpcIndirectDataContent", GetLastError()));
        }
        s.sip.indirect_data = indirect;
        let ind = &*indirect;
        let data_oid = c_text(ind.data_oid);
        let expected = match *subject {
            // SpcPeImageData.
            Subject::Pe(_) => data_oid == b"1.3.6.1.4.1.311.2.1.15",
            // SpcLink (Microsoft's signed cabinets) or SPC_CAB_DATA_OBJID.
            Subject::Cab(_) => {
                data_oid == b"1.3.6.1.4.1.311.2.1.28" || data_oid == b"1.3.6.1.4.1.311.2.1.25"
            }
        };
        if !expected {
            unsupported(b"wintrust.dll!WinVerifyTrust(indirect data of another subject)", 0);
            return Err(failed(b"indirect data type", TRUST_E_NOSIGNATURE));
        }
        let Some((alg, n)) = pe::digest_algorithm(c_text(ind.digest_algorithm.oid)) else {
            unsupported(b"wintrust.dll!WinVerifyTrust(image hash algorithm)", 0);
            return Err(failed(b"image hash algorithm", NTE_BAD_ALGID));
        };
        let mut hash = [0u8; 64];
        match subject {
            Subject::Pe(image) => pe::image_hash(h, image, alg, &mut hash[..n])
                .map_err(|e| failed(b"image hash", e))?,
            Subject::Cab(c) => {
                cab::hash(h, c, alg, &mut hash[..n]).map_err(|e| failed(b"cabinet hash", e))?
            }
        }
        if ind.digest.bytes() != &hash[..n] {
            let what: &[u8] = match *subject {
                Subject::Pe(_) => b"image hash differs (TRUST_E_BAD_DIGEST)",
                Subject::Cab(_) => b"cabinet hash differs (TRUST_E_BAD_DIGEST)",
            };
            return Err(failed(what, TRUST_E_BAD_DIGEST));
        }
    }
    Ok(())
}

/// Whether the certificate's extended key usage extension includes
/// lifetime signing (1.3.6.1.4.1.311.10.3.13).
fn lifetime_signing(cert: *const crate::layout::CertContext) -> bool {
    let mut buf = [0usize; 64];
    let mut len = core::mem::size_of_val(&buf) as u32;
    // SAFETY: CERT_FIND_EXT_ONLY_ENHKEY_USAGE_FLAG (2); our buffer.
    if unsafe { CertGetEnhancedKeyUsage(cert, 2, buf.as_mut_ptr().cast(), &mut len) } == 0 {
        return false;
    }
    // SAFETY: the CERT_ENHKEY_USAGE crypt32 wrote.
    let usage = unsafe { &*buf.as_ptr().cast::<EnhKeyUsage>() };
    (0..usage.count as usize).any(|i| {
        // SAFETY: its array of OID strings.
        c_text(unsafe { *usage.ids.add(i) }) == b"1.3.6.1.4.1.311.10.3.13"
    })
}

/// The signature step (module comment).
fn signature(s: &mut State) -> Result<(), u32> {
    let msg = s.data.msg;
    let mut count = 0u32;
    let mut len = 4u32;
    // SAFETY: crypt32's documented call; our DWORD.
    if unsafe {
        CryptMsgGetParam(msg, CMSG_SIGNER_COUNT_PARAM, 0, (&raw mut count).cast(), &mut len)
    } == 0
        || count == 0
    {
        return Err(failed(b"no signer", TRUST_E_NOSIGNATURE));
    }
    if count > 1 {
        unsupported(b"wintrust.dll!WinVerifyTrust(more than one signer: the first)", count.into());
    }
    let info = param(msg, CMSG_SIGNER_INFO_PARAM, 0).map_err(|e| failed(b"signer info", e))?;
    s.signer.size = core::mem::size_of::<Signer>() as u32;
    s.signer.info = info.cast();
    s.signer.verify_as_of = s.data.system_time;
    s.signer.certs = s.certs.as_mut_ptr();
    s.data.signer_count = 1;
    s.data.signers = &raw mut s.signer;
    let cert_info =
        param(msg, CMSG_SIGNER_CERT_INFO_PARAM, 0).map_err(|e| failed(b"signer id", e))?;
    // SAFETY: the store; the CERT_INFO crypt32 built.
    let cert = unsafe { CertGetSubjectCertificateFromStore(s.store, X509_PKCS7, cert_info.cast()) };
    heap_free(cert_info.cast());
    if cert.is_null() {
        return Err(failed(b"signer certificate not in the signature", TRUST_E_NO_SIGNER_CERT));
    }
    s.certs[0].size = core::mem::size_of::<ProvCert>() as u32;
    s.certs[0].cert = cert;
    s.signer.cert_count = 1;
    let para = VerifyExPara {
        size: core::mem::size_of::<VerifyExPara>() as u32,
        prov: 0,
        signer_index: 0,
        signer_type: CMSG_VERIFY_SIGNER_CERT,
        signer: cert.cast(),
    };
    // SAFETY: crypt32's documented call; our structure.
    if unsafe { CryptMsgControl(msg, 0, CMSG_CTRL_VERIFY_SIGNATURE_EX, (&raw const para).cast()) }
        == 0
    {
        // crypt32's error is the signer's; the verification's is
        // TRUST_E_NOSIGNATURE, as Windows answers (issue #57).
        // SAFETY: plain call.
        let e = unsafe { GetLastError() };
        s.signer.error = e;
        failed(b"signer's signature", e);
        return Err(TRUST_E_NOSIGNATURE);
    }
    timestamp(s)?;
    let lifetime = s.data.prov_flags & WTD_LIFETIME_SIGNING_FLAG != 0 || lifetime_signing(cert);
    if lifetime && s.signer.counter_count > 0 {
        // A lifetime signing certificate is valid only at the current time.
        s.signer.verify_as_of = s.data.system_time;
    }
    Ok(())
}

/// The signer's RFC 3161 timestamp, if any (module comment).
fn timestamp(s: &mut State) -> Result<(), u32> {
    // SAFETY: the CMSG_SIGNER_INFO crypt32 built.
    let info = unsafe { &*s.signer.info };
    let mut token = None;
    for i in 0..info.unauth.count as usize {
        // SAFETY: its attribute array.
        let a = unsafe { &*info.unauth.attrs.add(i) };
        match c_text(a.oid) {
            b"1.3.6.1.4.1.311.3.3.1" if a.count > 0 => {
                // SAFETY: its value array.
                token = Some(unsafe { *a.values });
            }
            b"1.2.840.113549.1.9.6" => unsupported(
                b"wintrust.dll!WinVerifyTrust(PKCS #9 countersignature: not used, current time)",
                0,
            ),
            _ => {}
        }
    }
    let Some(token): Option<Blob> = token else { return Ok(()) };
    let digest = info.encrypted_hash.bytes();
    let mut context: *mut TimestampContext = core::ptr::null_mut();
    let mut signer = core::ptr::null();
    let mut store = core::ptr::null_mut();
    // SAFETY: crypt32's documented call; the token and digest are blobs
    // of the signer info; our outputs.
    let ok = unsafe {
        CryptVerifyTimeStampSignature(
            token.data,
            token.size,
            digest.as_ptr(),
            digest.len() as u32,
            s.store,
            &mut context,
            &mut signer,
            &mut store,
        )
    };
    if ok == 0 {
        // SAFETY: plain call.
        let e = unsafe { GetLastError() };
        failed(b"timestamp", e);
        return Err(TRUST_E_TIME_STAMP);
    }
    s.timestamp = context;
    s.timestamp_store = store;
    // SAFETY: the context crypt32 made.
    let time = unsafe { (*(*context).info).time };
    s.signer.verify_as_of = time;
    // The counter signer: the timestamp's signer, its CMSG_SIGNER_INFO
    // from the token decoded again.
    s.counter.size = core::mem::size_of::<Signer>() as u32;
    s.counter.verify_as_of = time;
    s.counter.certs = s.counter_certs.as_mut_ptr();
    s.counter_certs[0].size = core::mem::size_of::<ProvCert>() as u32;
    s.counter_certs[0].cert = signer;
    s.counter.cert_count = 1;
    // SAFETY: crypt32's documented calls; the token's blob.
    unsafe {
        let m = CryptMsgOpenToDecode(X509_PKCS7, 0, 0, 0, core::ptr::null(), core::ptr::null());
        if !m.is_null() {
            if CryptMsgUpdate(m, token.data, token.size, 1) != 0
                && let Ok(p) = param(m, CMSG_SIGNER_INFO_PARAM, 0)
            {
                s.counter_info = p.cast();
                s.counter.info = p.cast();
            }
            CryptMsgClose(m);
        }
    }
    s.signer.counter_count = 1;
    s.signer.counters = &raw mut s.counter;
    Ok(())
}

/// The confidence of a chain element's status (Wine's softpub.c).
fn confidence(error: u32) -> u32 {
    let mut c = 0;
    if error & IS_NOT_SIGNATURE_VALID == 0 {
        c |= 0x1000_0000; // CERT_CONFIDENCE_SIG
    }
    if error & IS_NOT_TIME_VALID == 0 {
        c |= 0x0100_0000; // CERT_CONFIDENCE_TIME
    }
    if error & IS_NOT_TIME_NESTED == 0 {
        c |= 0x0010_0000; // CERT_CONFIDENCE_TIMENEST
    }
    c
}

/// An element's error as WinVerifyTrust's codes (Wine's softpub.c).
fn element_error(error: u32) -> u32 {
    const MAP: [(u32, u32); 8] = [
        (IS_NOT_SIGNATURE_VALID, 0x8009_6004), // TRUST_E_CERT_SIGNATURE
        (IS_UNTRUSTED_ROOT, 0x800b_0109),      // CERT_E_UNTRUSTEDROOT
        (IS_NOT_TIME_VALID, 0x800b_0101),      // CERT_E_EXPIRED
        (IS_NOT_TIME_NESTED, 0x800b_0102),     // CERT_E_VALIDITYPERIODNESTING
        (IS_REVOKED, 0x800b_010c),             // CERT_E_REVOKED
        (REVOCATION_UNKNOWN, 0x800b_010e),     // CERT_E_REVOCATION_FAILURE
        (IS_NOT_VALID_FOR_USAGE, 0x800b_0110), // CERT_E_WRONG_USAGE
        (IS_CYCLIC, 0x800b_010a),              // CERT_E_CHAINING
    ];
    MAP.iter()
        .find(|(bit, _)| error & bit != 0)
        .map_or(if error != 0 { TRUST_E_SYSTEM_ERROR } else { 0 }, |&(_, code)| code)
}

/// Builds `cert`'s chain at `time` for `usage`, into `certs` (from the
/// second; the first is the end certificate already there).
fn chain_into(
    store: *mut c_void,
    time: u64,
    usage: &'static core::ffi::CStr,
    flags: u32,
    certs: &mut [ProvCert; MAX_CERTS],
    count: &mut u32,
) -> Result<*const ChainContext, u32> {
    let ids = [usage.as_ptr().cast::<u8>()];
    let para = ChainPara {
        size: core::mem::size_of::<ChainPara>() as u32,
        requested: UsageMatch { kind: 0, usage: EnhKeyUsage { count: 1, ids: ids.as_ptr() } },
    };
    let mut chain = core::ptr::null();
    // SAFETY: crypt32's documented call; the default engine (NULL).
    if unsafe {
        CertGetCertificateChain(
            core::ptr::null_mut(),
            certs[0].cert,
            &time,
            store,
            &para,
            flags,
            core::ptr::null_mut(),
            &mut chain,
        )
    } == 0
    {
        // SAFETY: plain call.
        return Err(unsafe { GetLastError() });
    }
    // SAFETY: the chain crypt32 built: one simple chain.
    unsafe {
        let simple = &**(*chain).chains;
        let n = (simple.count as usize).min(MAX_CERTS);
        for (i, pc) in certs.iter_mut().enumerate().take(n) {
            let el: &ChainElement = &**simple.elements.add(i);
            pc.size = core::mem::size_of::<ProvCert>() as u32;
            if i > 0 {
                pc.cert = CertDuplicateCertificateContext(el.cert);
            }
            pc.element = el;
            pc.confidence = confidence(el.status.error);
            pc.error = element_error(el.status.error);
            pc.self_signed = i32::from(el.status.info & IS_SELF_SIGNED != 0);
            pc.trusted_root = i32::from(
                i == n - 1 && pc.self_signed != 0 && el.status.error & IS_UNTRUSTED_ROOT == 0,
            );
            pc.cyclic = i32::from(el.status.error & IS_CYCLIC != 0);
        }
        *count = n as u32;
    }
    Ok(chain)
}

/// The certificate step (module comment).
fn certificate(s: &mut State) -> Result<(), u32> {
    // SAFETY: the WINTRUST_DATA the state points to (the caller's, for
    // the length of this call).
    let data = unsafe { &*s.data.wintrust_data };
    let prov = s.data.prov_flags;
    let flags = if prov & WTD_REVOCATION_CHECK_NONE != 0 {
        0
    } else if prov & WTD_REVOCATION_CHECK_END_CERT != 0 {
        0x1000_0000
    } else if prov & WTD_REVOCATION_CHECK_CHAIN != 0 {
        0x2000_0000
    } else if prov & WTD_REVOCATION_CHECK_CHAIN_EXCLUDE_ROOT != 0
        || (has!(data.size, WintrustData, revocation) && data.revocation == WTD_REVOKE_WHOLECHAIN)
    {
        0x4000_0000
    } else {
        0
    };
    let chain = chain_into(
        s.store,
        as_time(s.signer.verify_as_of),
        c"1.3.6.1.5.5.7.3.3", // code signing
        flags,
        &mut s.certs,
        &mut s.signer.cert_count,
    )
    .map_err(|e| failed(b"CertGetCertificateChain", e))?;
    s.signer.chain = chain;
    if s.signer.counter_count > 0 {
        // The timestamp's chain at its own time, without revocation
        // (WTPF_IGNOREREVOCATIONONTS).
        let store = if s.timestamp_store.is_null() { s.store } else { s.timestamp_store };
        let chain = chain_into(
            store,
            as_time(s.counter.verify_as_of),
            c"1.3.6.1.5.5.7.3.8", // time stamping
            0,
            &mut s.counter_certs,
            &mut s.counter.cert_count,
        )
        .map_err(|e| failed(b"timestamp chain", e))?;
        s.counter.chain = chain;
        s.counter_chain = chain;
    }
    Ok(())
}

/// The policy step (module comment).
fn policy(s: &mut State) -> Result<(), u32> {
    let check = |chain: *const ChainContext, policy: usize, info: *const SignerInfo| {
        let extra = AuthenticodeExtra {
            size: core::mem::size_of::<AuthenticodeExtra>() as u32,
            reg_policy: DEFAULT_POLICY_STATE,
            signer_info: info,
        };
        let para = PolicyPara {
            size: core::mem::size_of::<PolicyPara>() as u32,
            flags: 0,
            extra: (&raw const extra).cast(),
        };
        let mut status = PolicyStatus {
            size: core::mem::size_of::<PolicyStatus>() as u32,
            error: 0,
            chain_index: -1,
            element_index: -1,
            extra: core::ptr::null_mut(),
        };
        // SAFETY: crypt32's documented call; our structures.
        if unsafe { CertVerifyCertificateChainPolicy(policy, chain, &para, &mut status) } == 0 {
            // SAFETY: plain call.
            return unsafe { GetLastError() };
        }
        status.error
    };
    // CERT_CHAIN_POLICY_AUTHENTICODE (2).
    let e = check(s.signer.chain, 2, s.signer.info);
    if e != 0 {
        s.signer.error = e;
        return Err(failed(b"Authenticode policy", e));
    }
    if s.signer.counter_count > 0 {
        // CERT_CHAIN_POLICY_AUTHENTICODE_TS (3).
        let e = check(s.counter.chain, 3, s.counter.info);
        if e != 0 {
            s.counter.error = e;
            return Err(failed(b"timestamp policy", e));
        }
    }
    Ok(())
}

/// WTHelperProvDataFromStateData: the provider data a
/// WTD_STATEACTION_VERIFY call kept (its hWVTStateData).
#[unsafe(no_mangle)]
pub extern "system" fn WTHelperProvDataFromStateData(h: *mut c_void) -> *mut ProvData {
    h.cast()
}

/// WTHelperGetProvSignerFromChain: signer `index` of the provider data,
/// or its counter signer `counter_index`; NULL past the end.
#[unsafe(no_mangle)]
pub extern "system" fn WTHelperGetProvSignerFromChain(
    data: *mut ProvData,
    index: u32,
    counter: i32,
    counter_index: u32,
) -> *mut Signer {
    // SAFETY: the caller's provider data, if any.
    let Some(d) = (unsafe { data.as_ref() }) else { return core::ptr::null_mut() };
    if index >= d.signer_count || d.signers.is_null() {
        return core::ptr::null_mut();
    }
    // SAFETY: within its signer array.
    let s = unsafe { d.signers.add(index as usize) };
    if counter == 0 {
        return s;
    }
    // SAFETY: as above.
    let sg = unsafe { &*s };
    if counter_index >= sg.counter_count || sg.counters.is_null() {
        return core::ptr::null_mut();
    }
    // SAFETY: within its counter signer array.
    unsafe { sg.counters.add(counter_index as usize) }
}

/// WTHelperGetProvCertFromChain: certificate `index` of a signer's chain;
/// NULL past the end.
#[unsafe(no_mangle)]
pub extern "system" fn WTHelperGetProvCertFromChain(
    signer: *mut Signer,
    index: u32,
) -> *mut ProvCert {
    // SAFETY: the caller's signer, if any.
    let Some(s) = (unsafe { signer.as_ref() }) else { return core::ptr::null_mut() };
    if index >= s.cert_count || s.certs.is_null() {
        return core::ptr::null_mut();
    }
    // SAFETY: within its certificate array.
    unsafe { s.certs.add(index as usize) }
}

//! gala's schannel.dll: the Schannel security package (TLS 1.2 and 1.3)
//! behind SSPI. secur32 forwards credentials and contexts of this package
//! here (handles tagged `galaSCHC`/`galaSCHX`); the functions are also
//! exported under their SSPI names, as Windows' schannel.dll does.
//!
//! - `creds`: AcquireCredentialsHandle's SCHANNEL_CRED / SCH_CREDENTIALS,
//!   a server's certificate and CNG key.
//! - `trust`: automatic validation, by crypt32's chain engine and SSL
//!   policy (gala's machine root store), the decision programs that
//!   validate themselves get too.
//! - here: InitializeSecurityContext, AcceptSecurityContext,
//!   EncryptMessage, DecryptMessage, ApplyControlToken, contexts, with the
//!   SecBuffer rules (learn.microsoft.com: the Schannel pages of those
//!   functions) over the engine in `schannel_tls`, which holds the TLS
//!   state machine and rustls.
//! - `attrs`: QueryContextAttributes.
//!
//! What a failure leaves: a failed handshake or message records
//! `schannel.dll!<function>: <rustls error>` in the ledger with the SSPI
//! status; unsupported requests record what was asked.

#![no_std]
#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]
#![allow(clippy::too_many_arguments)]

extern crate alloc;

// The global allocator for rustls and the RustCrypto crates.
use gala_heap as _;

mod attrs;
mod creds;
mod ffi;
mod layout;
mod trust;

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

use ffi::{
    CONTEXT_TAG, CREDENTIAL_TAG, SECBUFFER_ALERT, SECBUFFER_APPLICATION_PROTOCOLS, SECBUFFER_DATA,
    SECBUFFER_EMPTY, SECBUFFER_EXTRA, SECBUFFER_MISSING, SECBUFFER_STREAM_HEADER,
    SECBUFFER_STREAM_TRAILER, SECBUFFER_TOKEN, SecBuffer, SecBufferDesc, SecHandle, buffers, bytes,
    find, kind, missing, state,
};
use schannel_tls::{ClientSettings, ServerSettings, Session, Validation, sec};

// The 32-bit build (SysWOW64) takes the memory routines, 64-bit
// division helpers and unwind personality from ntrt's x86 runtime
// (ARCHITECTURE "Plan: WOW64").
#[cfg(target_arch = "x86")]
use ntrt as _;

#[cfg(target_arch = "x86_64")]
#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memmove(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
    fn strlen(s: *const u8) -> usize;
    // MSVC-target unwind tables reference it (gala's ntdll provides it).
    fn __CxxFrameHandler3();
    // The stack probe for frames over a page (ntdll exports it).
    fn __chkstk();
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn ExitProcess(code: u32) -> !;
    fn GetCurrentThreadId() -> u32;
}

/// MSVC-target codegen references this marker symbol directly.
#[cfg(target_arch = "x86_64")]
#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

/// UNISP_NAME.
pub const PACKAGE: &str = "Microsoft Unified Security Protocol Provider";

pub const SEC_E_INSUFFICIENT_MEMORY: u32 = 0x8009_0300;
pub const SEC_E_INVALID_HANDLE: u32 = 0x8009_0301;
pub const SEC_E_UNSUPPORTED_FUNCTION: u32 = 0x8009_0302;
pub const SEC_E_INVALID_TOKEN: u32 = 0x8009_0308;
pub const SEC_E_BUFFER_TOO_SMALL: u32 = 0x8009_0321;
const SEC_E_SECPKG_NOT_FOUND: u32 = 0x8009_0305;

/// ISC_REQ_/ASC_REQ_ flags.
const ALLOCATE_MEMORY: u32 = 0x100;
const ISC_REQ_USE_SUPPLIED_CREDS: u32 = 0x80;
const ISC_REQ_EXTENDED_ERROR: u32 = 0x4000;
const ISC_REQ_MANUAL_CRED_VALIDATION: u32 = 0x8_0000;
const ASC_REQ_EXTENDED_ERROR: u32 = 0x8000;
const ASC_REQ_MUTUAL_AUTH: u32 = 0x2;
/// What a client may ask for and get (ISC_RET_ has the same bits):
/// REPLAY_DETECT, SEQUENCE_DETECT, CONFIDENTIALITY, USE_SUPPLIED_CREDS,
/// EXTENDED_ERROR, STREAM, INTEGRITY, MANUAL_CRED_VALIDATION.
const ISC_GRANTED: u32 = 0x4 | 0x8 | 0x10 | 0x80 | 0x4000 | 0x8000 | 0x1_0000 | 0x8_0000;
/// The same for a server: REPLAY_DETECT, SEQUENCE_DETECT,
/// CONFIDENTIALITY, EXTENDED_ERROR, STREAM, MUTUAL_AUTH (asked: no
/// client certificate is requested yet).
const ASC_GRANTED: u32 = 0x4 | 0x8 | 0x10 | 0x8000 | 0x1_0000;

/// A context handle's state.
pub struct Context {
    pub session: Session,
    pub server: bool,
    /// Whether the first call offered ALPN protocols.
    pub alpn_offered: bool,
    /// A server's certificate (SECPKG_ATTR_LOCAL_CERT_CONTEXT).
    pub local: Option<Vec<u8>>,
    /// A client's target name (pszTargetName of the first call).
    pub target: String,
}

/// AcquireCredentialsHandleW with UNISP_NAME or SCHANNEL_NAME.
#[unsafe(no_mangle)]
pub extern "system" fn AcquireCredentialsHandleW(
    _principal: *const u16,
    package: *const u16,
    usage: u32,
    _logon: usize,
    auth: *const u8,
    _get_key: usize,
    _arg: usize,
    credential: *mut SecHandle,
    expiry: *mut i64,
) -> u32 {
    acquire(&ffi::wide(package), usage, auth, credential, expiry)
}

/// AcquireCredentialsHandleA: the same with an ANSI package name.
#[unsafe(no_mangle)]
pub extern "system" fn AcquireCredentialsHandleA(
    _principal: *const u8,
    package: *const u8,
    usage: u32,
    _logon: usize,
    auth: *const u8,
    _get_key: usize,
    _arg: usize,
    credential: *mut SecHandle,
    expiry: *mut i64,
) -> u32 {
    acquire(&ffi::ansi(package), usage, auth, credential, expiry)
}

fn acquire(
    package: &str,
    usage: u32,
    auth: *const u8,
    out: *mut SecHandle,
    expiry: *mut i64,
) -> u32 {
    if !ffi::is_schannel(package) {
        return SEC_E_SECPKG_NOT_FOUND;
    }
    if out.is_null() {
        return SEC_E_INVALID_HANDLE;
    }
    // SECPKG_CRED_INBOUND (1) is a server; OUTBOUND (2) a client.
    let inbound = usage & 3 == 1;
    let credential = match creds::acquire(inbound, auth) {
        Ok(c) => c,
        Err(e) => return e,
    };
    let p = ffi::boxed(credential);
    if p.is_null() {
        return SEC_E_INSUFFICIENT_MEMORY;
    }
    // SAFETY: the caller's outputs.
    unsafe {
        out.write(SecHandle { lower: p as usize, upper: CREDENTIAL_TAG });
        if !expiry.is_null() {
            expiry.write(i64::MAX);
        }
    }
    sec::E_OK
}

#[unsafe(no_mangle)]
pub extern "system" fn FreeCredentialsHandle(credential: *mut SecHandle) -> u32 {
    let Some(c) = state::<creds::Credential>(credential, CREDENTIAL_TAG) else {
        return SEC_E_INVALID_HANDLE;
    };
    ffi::unbox(c as *mut creds::Credential);
    // SAFETY: the caller's handle, now cleared.
    unsafe { credential.write(SecHandle { lower: 0, upper: 0 }) };
    sec::E_OK
}

/// QueryCredentialsAttributesW: SECPKG_ATTR_SUPPORTED_PROTOCOLS (0x58).
#[unsafe(no_mangle)]
pub extern "system" fn QueryCredentialsAttributesW(
    credential: *mut SecHandle,
    attribute: u32,
    buffer: *mut u32,
) -> u32 {
    let Some(c) = state::<creds::Credential>(credential, CREDENTIAL_TAG) else {
        return SEC_E_INVALID_HANDLE;
    };
    if attribute != 0x58 || buffer.is_null() {
        missing(b"schannel.dll!QueryCredentialsAttributesW(attribute)", attribute.into());
        return SEC_E_UNSUPPORTED_FUNCTION;
    }
    let side = if c.inbound { 0x400 } else { 0x800 };
    let bits =
        if c.protocols.tls12 { side } else { 0 } | if c.protocols.tls13 { side << 2 } else { 0 };
    // SAFETY: the caller's SecPkgCred_SupportedProtocols.
    unsafe { buffer.write(bits) };
    sec::E_OK
}

/// The ALPN protocol names of a SECBUFFER_APPLICATION_PROTOCOLS buffer
/// (SEC_APPLICATION_PROTOCOLS: ProtocolListsSize, then lists of
/// ProtoNegoExt, ProtocolListSize and the protocols as TLS encodes them);
/// only the ALPN list (ProtoNegoExt 2).
fn alpn_list(input: *mut SecBufferDesc) -> Option<Vec<Vec<u8>>> {
    let list = buffers(input);
    let b = bytes(&list[find(list, SECBUFFER_APPLICATION_PROTOCOLS)?]);
    let total = (u32::from_le_bytes(b.get(..4)?.try_into().ok()?) as usize).min(b.len() - 4);
    let mut lists = &b[4..4 + total];
    let mut names = Vec::new();
    while lists.len() >= 6 {
        let ext = u32::from_le_bytes(lists[..4].try_into().ok()?);
        let size = usize::from(u16::from_le_bytes([lists[4], lists[5]])).min(lists.len() - 6);
        let mut protocols = &lists[6..6 + size];
        if ext == 2 {
            while let Some((&n, rest)) = protocols.split_first() {
                let n = usize::from(n).min(rest.len());
                names.push(rest[..n].to_vec());
                protocols = &rest[n..];
            }
        } else {
            missing(b"schannel.dll!application protocol extension", ext.into());
        }
        lists = &lists[6 + size..];
    }
    Some(names)
}

/// The input token of a call (the first SECBUFFER_TOKEN), if any.
fn input_token<'a>(input: *mut SecBufferDesc) -> &'a [u8] {
    let list = buffers(input);
    match find(list, SECBUFFER_TOKEN) {
        Some(i) => bytes(&list[i]),
        None => &[],
    }
}

/// Reports SECBUFFER_EXTRA or SECBUFFER_MISSING in the first empty input
/// buffer after the token, as the documented loop reads them.
fn report_input(input: *mut SecBufferDesc, extra: usize, missing_bytes: usize) {
    let list = buffers(input);
    let Some(t) = find(list, SECBUFFER_TOKEN) else { return };
    let (token_data, token_size) = (list[t].data, list[t].size as usize);
    let Some(b) = list[t + 1..].iter_mut().find(|b| kind(b) == SECBUFFER_EMPTY) else { return };
    if extra > 0 {
        b.kind = SECBUFFER_EXTRA;
        b.size = extra as u32;
        b.data = token_data.wrapping_add(token_size - extra);
    } else if missing_bytes > 0 {
        b.kind = SECBUFFER_MISSING;
        b.size = missing_bytes as u32;
        b.data = core::ptr::null_mut();
    }
}

/// The common part of InitializeSecurityContext and
/// AcceptSecurityContext once the context exists.
fn step(
    ctx: &mut Context,
    function: &str,
    input: *mut SecBufferDesc,
    flags: u32,
    output: *mut SecBufferDesc,
    attributes: *mut u32,
) -> u32 {
    let token = ctx.session.token(input_token(input));
    let (granted, extended) = if ctx.server {
        (ASC_GRANTED, flags & ASC_REQ_EXTENDED_ERROR != 0)
    } else {
        (ISC_GRANTED, flags & ISC_REQ_EXTENDED_ERROR != 0)
    };
    let failed = token.status & 0x8000_0000 != 0 && token.status != sec::E_INCOMPLETE_MESSAGE;
    let mut allocated = false;
    if !failed || extended {
        // On a failure the alert goes out only with EXTENDED_ERROR (in an
        // ALERT buffer if there is one).
        let list = buffers(output);
        if failed && let Some(i) = find(list, SECBUFFER_ALERT) {
            list[i].kind = SECBUFFER_TOKEN;
            let r = ffi::output_token(output, &token.output, flags & ALLOCATE_MEMORY != 0);
            list[i].kind = SECBUFFER_ALERT;
            allocated = r.unwrap_or(false);
        } else {
            match ffi::output_token(output, &token.output, flags & ALLOCATE_MEMORY != 0) {
                Ok(a) => allocated = a,
                Err(e) if !failed => return e,
                Err(_) => {}
            }
        }
    } else {
        let _ = ffi::output_token(output, &[], flags & ALLOCATE_MEMORY != 0);
    }
    report_input(input, token.extra, token.missing);
    if !attributes.is_null() {
        let mut a = flags & granted;
        if allocated || flags & ALLOCATE_MEMORY != 0 {
            a |= ALLOCATE_MEMORY; // ISC_RET_/ASC_RET_ALLOCATED_MEMORY
        }
        // SAFETY: the caller's output.
        unsafe { attributes.write(a) };
    }
    if failed {
        match ctx.session.failure() {
            Some(reason) => ffi::failed(function, reason, token.status),
            None => ffi::failed(function, &FailedStatus(token.status), token.status),
        }
    }
    token.status
}

/// A status as text for the ledger line.
struct FailedStatus(u32);

impl core::fmt::Debug for FailedStatus {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let name = match self.0 {
            sec::E_UNTRUSTED_ROOT => "SEC_E_UNTRUSTED_ROOT",
            sec::E_WRONG_PRINCIPAL => "SEC_E_WRONG_PRINCIPAL",
            sec::E_CERT_EXPIRED => "SEC_E_CERT_EXPIRED",
            sec::E_CERT_UNKNOWN => "SEC_E_CERT_UNKNOWN",
            sec::E_ILLEGAL_MESSAGE => "SEC_E_ILLEGAL_MESSAGE",
            sec::E_ALGORITHM_MISMATCH => "SEC_E_ALGORITHM_MISMATCH",
            sec::E_DECRYPT_FAILURE => "SEC_E_DECRYPT_FAILURE",
            sec::E_INTERNAL_ERROR => "SEC_E_INTERNAL_ERROR",
            sec::CRYPT_E_REVOKED => "CRYPT_E_REVOKED",
            sec::CRYPT_E_NO_REVOCATION_CHECK => "CRYPT_E_NO_REVOCATION_CHECK",
            sec::CRYPT_E_REVOCATION_OFFLINE => "CRYPT_E_REVOCATION_OFFLINE",
            _ => "failed",
        };
        f.write_str(name)
    }
}

/// Stores a new context and gives its handle.
fn store_context(ctx: Context, out: *mut SecHandle) -> Result<*mut Context, u32> {
    if out.is_null() {
        return Err(SEC_E_INVALID_HANDLE);
    }
    let p = ffi::boxed(ctx);
    if p.is_null() {
        return Err(SEC_E_INSUFFICIENT_MEMORY);
    }
    // SAFETY: the caller's handle.
    unsafe { out.write(SecHandle { lower: p as usize, upper: CONTEXT_TAG }) };
    Ok(p)
}

/// Gives the same context in phNewContext on later calls.
fn same_context(context: *mut SecHandle, new: *mut SecHandle) {
    if !new.is_null() && new != context {
        // SAFETY: the caller's handles.
        unsafe { new.write(*context) };
    }
}

fn set_expiry(expiry: *mut i64) {
    if !expiry.is_null() {
        // SAFETY: the caller's output.
        unsafe { expiry.write(i64::MAX) };
    }
}

/// InitializeSecurityContextW (Schannel): the client's handshake.
#[unsafe(no_mangle)]
pub extern "system" fn InitializeSecurityContextW(
    credential: *mut SecHandle,
    context: *mut SecHandle,
    target: *const u16,
    flags: u32,
    _reserved: u32,
    _representation: u32,
    input: *mut SecBufferDesc,
    _reserved2: u32,
    new_context: *mut SecHandle,
    output: *mut SecBufferDesc,
    attributes: *mut u32,
    expiry: *mut i64,
) -> u32 {
    initialize(
        credential,
        context,
        &ffi::wide(target),
        flags,
        input,
        new_context,
        output,
        attributes,
        expiry,
    )
}

/// InitializeSecurityContextA: the same with an ANSI target name.
#[unsafe(no_mangle)]
pub extern "system" fn InitializeSecurityContextA(
    credential: *mut SecHandle,
    context: *mut SecHandle,
    target: *const u8,
    flags: u32,
    _reserved: u32,
    _representation: u32,
    input: *mut SecBufferDesc,
    _reserved2: u32,
    new_context: *mut SecHandle,
    output: *mut SecBufferDesc,
    attributes: *mut u32,
    expiry: *mut i64,
) -> u32 {
    initialize(
        credential,
        context,
        &ffi::ansi(target),
        flags,
        input,
        new_context,
        output,
        attributes,
        expiry,
    )
}

/// How a client context of `cred` validates the server's certificate:
/// manually (SCH_CRED_MANUAL_CRED_VALIDATION, ISC_REQ_MANUAL_CRED_VALIDATION)
/// or by crypt32.
fn validation(cred: &creds::Credential, flags: u32) -> Validation {
    if cred.manual || flags & ISC_REQ_MANUAL_CRED_VALIDATION != 0 {
        Validation::Manual
    } else {
        let trust = trust::Crypt32 { revocation: cred.revocation };
        Validation::Auto { trust: Arc::new(trust), check_name: cred.check_name }
    }
}

fn initialize(
    credential: *mut SecHandle,
    context: *mut SecHandle,
    target: &str,
    flags: u32,
    input: *mut SecBufferDesc,
    new_context: *mut SecHandle,
    output: *mut SecBufferDesc,
    attributes: *mut u32,
    expiry: *mut i64,
) -> u32 {
    set_expiry(expiry);
    if let Some(ctx) = state::<Context>(context, CONTEXT_TAG) {
        if ctx.server {
            return sec::E_OUT_OF_SEQUENCE;
        }
        if ctx.session.awaiting_credentials() {
            // After SEC_I_INCOMPLETE_CREDENTIALS: this call's credential
            // (a new one with a certificate, or the same to go on without).
            let mut supplied = None;
            if let Some(cred) = state::<creds::Credential>(credential, CREDENTIAL_TAG)
                && let Some(cert) = cred.client_cert.clone()
            {
                // The session then belongs to this credential (its cache).
                match cred.client_cache.config_for(
                    &cred.provider,
                    Arc::new(creds::Clock),
                    cred.protocols,
                    validation(cred, flags),
                    &ctx.target,
                ) {
                    Ok((_, config)) => supplied = Some((cert, config)),
                    Err(e) => {
                        ffi::failed("ISC", &FailedStatus(e), e);
                        return e;
                    }
                }
            }
            if let Err(e) = ctx.session.supply(supplied) {
                match ctx.session.failure() {
                    Some(reason) => ffi::failed("ISC", reason, e),
                    None => ffi::failed("ISC", &FailedStatus(e), e),
                }
                return e;
            }
        }
        let r = step(ctx, "ISC", input, flags, output, attributes);
        if r == sec::I_INCOMPLETE_CREDENTIALS
            && state::<creds::Credential>(credential, CREDENTIAL_TAG)
                .is_some_and(|c| !c.no_default_creds)
        {
            // Windows would first look for a certificate in the MY store.
            missing(b"schannel.dll!client certificate chosen from the MY store", 0);
        }
        same_context(context, new_context);
        return r;
    }
    let Some(cred) = state::<creds::Credential>(credential, CREDENTIAL_TAG) else {
        return SEC_E_INVALID_HANDLE;
    };
    if cred.inbound {
        return SEC_E_INVALID_HANDLE;
    }
    let alpn = alpn_list(input);
    let settings = ClientSettings {
        protocols: cred.protocols,
        validation: validation(cred, flags),
        target,
        alpn: alpn.clone().unwrap_or_default(),
        certificate: cred.client_cert.clone(),
        // ISC_REQ_USE_SUPPLIED_CREDS: send what the credential has, never
        // stop for more.
        ask: flags & ISC_REQ_USE_SUPPLIED_CREDS == 0,
    };
    let session =
        match Session::client(&cred.provider, Arc::new(creds::Clock), settings, &cred.client_cache)
        {
            Ok(s) => s,
            Err(e) => {
                ffi::failed("ISC", &FailedStatus(e), e);
                return e;
            }
        };
    let ctx = Context {
        session,
        server: false,
        alpn_offered: alpn.is_some(),
        local: None,
        target: String::from(target),
    };
    let p = match store_context(ctx, new_context) {
        Ok(p) => p,
        Err(e) => return e,
    };
    // SAFETY: the context just stored.
    let ctx = unsafe { &mut *p };
    // The first call's input holds no token (only, perhaps, the ALPN list).
    step(ctx, "ISC", core::ptr::null_mut(), flags, output, attributes)
}

/// AcceptSecurityContext (Schannel): the server's handshake.
#[unsafe(no_mangle)]
pub extern "system" fn AcceptSecurityContext(
    credential: *mut SecHandle,
    context: *mut SecHandle,
    input: *mut SecBufferDesc,
    flags: u32,
    _representation: u32,
    new_context: *mut SecHandle,
    output: *mut SecBufferDesc,
    attributes: *mut u32,
    expiry: *mut i64,
) -> u32 {
    set_expiry(expiry);
    if let Some(ctx) = state::<Context>(context, CONTEXT_TAG) {
        if !ctx.server {
            return sec::E_OUT_OF_SEQUENCE;
        }
        let r = step(ctx, "ASC", input, flags, output, attributes);
        same_context(context, new_context);
        return r;
    }
    let Some(cred) = state::<creds::Credential>(credential, CREDENTIAL_TAG) else {
        return SEC_E_INVALID_HANDLE;
    };
    let Some(key) = cred.key.clone().filter(|_| cred.inbound) else {
        return SEC_E_INVALID_HANDLE;
    };
    let alpn = alpn_list(input);
    if flags & ASC_REQ_MUTUAL_AUTH != 0 {
        missing(b"schannel.dll!ASC_REQ_MUTUAL_AUTH (client certificates)", 0);
    }
    let settings = ServerSettings {
        protocols: cred.protocols,
        chain: cred.chain.clone(),
        key,
        alpn: alpn.clone().unwrap_or_default(),
        cache: cred.server_cache.clone(),
        // ASC_REQ_MUTUAL_AUTH is not asked of clients yet (recorded).
        client_auth: None,
    };
    let session = match Session::server(&cred.provider, Arc::new(creds::Clock), settings) {
        Ok(s) => s,
        Err(e) => {
            ffi::failed("ASC", &FailedStatus(e), e);
            return e;
        }
    };
    let local = cred.chain.first().map(|c| c.as_ref().to_vec());
    let ctx = Context {
        session,
        server: true,
        alpn_offered: alpn.is_some(),
        local,
        target: String::new(),
    };
    let mut handle = SecHandle { lower: 0, upper: 0 };
    let p = match store_context(ctx, &mut handle) {
        Ok(p) => p,
        Err(e) => return e,
    };
    // SAFETY: the context just stored.
    let r = step(unsafe { &mut *p }, "ASC", input, flags, output, attributes);
    if r & 0x8000_0000 != 0 {
        // No context yet (SEC_E_INCOMPLETE_MESSAGE: the caller calls again
        // from the start with more).
        ffi::unbox(p);
        return r;
    }
    if new_context.is_null() {
        ffi::unbox(p);
        return SEC_E_INVALID_HANDLE;
    }
    // SAFETY: the caller's handle.
    unsafe { new_context.write(handle) };
    r
}

#[unsafe(no_mangle)]
pub extern "system" fn DeleteSecurityContext(context: *mut SecHandle) -> u32 {
    let Some(c) = state::<Context>(context, CONTEXT_TAG) else { return SEC_E_INVALID_HANDLE };
    ffi::unbox(c as *mut Context);
    // SAFETY: the caller's handle, now cleared.
    unsafe { context.write(SecHandle { lower: 0, upper: 0 }) };
    sec::E_OK
}

/// ApplyControlToken: SCHANNEL_SHUTDOWN (the next token call gives
/// close_notify); SCHANNEL_ALERT is accepted (the alert itself is not
/// sent: recorded).
#[unsafe(no_mangle)]
pub extern "system" fn ApplyControlToken(
    context: *mut SecHandle,
    input: *mut SecBufferDesc,
) -> u32 {
    let Some(ctx) = state::<Context>(context, CONTEXT_TAG) else { return SEC_E_INVALID_HANDLE };
    let token = input_token(input);
    let Some(control) = token.get(..4).map(|b| u32::from_le_bytes(b.try_into().unwrap_or([0; 4])))
    else {
        return SEC_E_INVALID_TOKEN;
    };
    match control {
        1 => {
            ctx.session.shutdown();
            sec::E_OK
        }
        17 => {
            missing(b"schannel.dll!ApplyControlToken(SCHANNEL_ALERT)", 0);
            sec::E_OK
        }
        c => {
            missing(b"schannel.dll!ApplyControlToken(control)", c.into());
            SEC_E_UNSUPPORTED_FUNCTION
        }
    }
}

/// EncryptMessage (Schannel): the DATA buffer as one record, in place:
/// its header into STREAM_HEADER, the encrypted data over DATA, the rest
/// into STREAM_TRAILER (cbBuffer set to what was used).
#[unsafe(no_mangle)]
pub extern "system" fn EncryptMessage(
    context: *mut SecHandle,
    _qop: u32,
    desc: *mut SecBufferDesc,
    _seq: u32,
) -> u32 {
    let Some(ctx) = state::<Context>(context, CONTEXT_TAG) else { return SEC_E_INVALID_HANDLE };
    let list = buffers(desc);
    let (Some(h), Some(d), Some(t)) = (
        find(list, SECBUFFER_STREAM_HEADER),
        find(list, SECBUFFER_DATA),
        find(list, SECBUFFER_STREAM_TRAILER),
    ) else {
        return SEC_E_INVALID_TOKEN;
    };
    let [header, trailer, max, ..] = ctx.session.stream_sizes();
    let plain = bytes(&list[d]);
    if plain.len() > max as usize {
        return SEC_E_BUFFER_TOO_SMALL;
    }
    if list[h].size < header || list[t].size < trailer && list[t].size < 16 {
        return SEC_E_BUFFER_TOO_SMALL;
    }
    let mut record = Vec::new();
    if let Err(e) = ctx.session.encrypt(plain, &mut record) {
        ffi::failed("EncryptMessage", &FailedStatus(e), e);
        return e;
    }
    let (header, n) = (header as usize, plain.len());
    let rest = record.len() - header - n;
    if record.len() < header + n || rest > list[t].size as usize {
        return SEC_E_BUFFER_TOO_SMALL;
    }
    bytes(&list[h])[..header].copy_from_slice(&record[..header]);
    plain.copy_from_slice(&record[header..header + n]);
    bytes(&list[t])[..rest].copy_from_slice(&record[header + n..]);
    list[h].size = header as u32;
    list[t].size = rest as u32;
    sec::E_OK
}

/// DecryptMessage (Schannel): the first record of the DATA buffer,
/// decrypted in place; the buffers then describe it (STREAM_HEADER, DATA,
/// STREAM_TRAILER, and EXTRA for what follows), or SECBUFFER_MISSING with
/// SEC_E_INCOMPLETE_MESSAGE.
#[unsafe(no_mangle)]
pub extern "system" fn DecryptMessage(
    context: *mut SecHandle,
    desc: *mut SecBufferDesc,
    _seq: u32,
    qop: *mut u32,
) -> u32 {
    let Some(ctx) = state::<Context>(context, CONTEXT_TAG) else { return SEC_E_INVALID_HANDLE };
    if !qop.is_null() {
        // SAFETY: the caller's output.
        unsafe { qop.write(0) };
    }
    let list = buffers(desc);
    let Some(d) = find(list, SECBUFFER_DATA) else { return SEC_E_INVALID_TOKEN };
    let (base, len) = (list[d].data, list[d].size as usize);
    let r = ctx.session.decrypt(bytes(&list[d]));
    if r.status == sec::E_INCOMPLETE_MESSAGE {
        if let Some(b) = list.iter_mut().find(|b| kind(b) == SECBUFFER_EMPTY) {
            b.kind = SECBUFFER_MISSING;
            b.size = r.missing as u32;
        }
        return r.status;
    }
    if r.status & 0x8000_0000 != 0 {
        match ctx.session.failure() {
            Some(reason) => ffi::failed("DecryptMessage", reason, r.status),
            None => ffi::failed("DecryptMessage", &FailedStatus(r.status), r.status),
        }
        return r.status;
    }
    // The buffers in the documented order, as many as the caller gave.
    let at = |o: usize| base.wrapping_add(o);
    let consumed = len - r.extra;
    let parts = [
        SecBuffer { size: r.header as u32, kind: SECBUFFER_STREAM_HEADER, data: at(0) },
        SecBuffer { size: r.data as u32, kind: SECBUFFER_DATA, data: at(r.header) },
        SecBuffer {
            size: r.trailer as u32,
            kind: SECBUFFER_STREAM_TRAILER,
            data: at(r.header + r.data),
        },
        SecBuffer {
            size: r.extra as u32,
            kind: if r.extra > 0 { SECBUFFER_EXTRA } else { SECBUFFER_EMPTY },
            data: if r.extra > 0 { at(consumed) } else { core::ptr::null_mut() },
        },
    ];
    for (slot, part) in list.iter_mut().zip(parts) {
        *slot = part;
    }
    r.status
}

/// FreeContextBuffer: a buffer this package allocated (process heap).
#[unsafe(no_mangle)]
pub extern "system" fn FreeContextBuffer(buffer: *mut u8) -> u32 {
    ffi::free(buffer);
    sec::E_OK
}

/// DllMain (the entry point our link flags name): names threads for the
/// engine's replay registry (schannel_tls replay.rs).
#[unsafe(no_mangle)]
pub extern "system" fn mainCRTStartup(_instance: usize, reason: u32, _reserved: usize) -> i32 {
    if reason == 1 {
        schannel_tls::set_thread_id(|| {
            // SAFETY: plain call.
            u64::from(unsafe { GetCurrentThreadId() })
        });
    }
    1
}

/// A panic (a bug in this package or a crate it uses) ends the process
/// with STATUS_STACK_BUFFER_OVERRUN's fail-fast code, after recording
/// where it happened.
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    use core::fmt::Write;
    let mut text = String::new();
    if let Some(l) = info.location() {
        let _ = write!(text, "{}:{}", l.file(), l.line());
    }
    ffi::failed("panic", &text, 0xc000_0409);
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xc000_0409) }
}

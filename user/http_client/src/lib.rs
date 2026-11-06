//! The HTTP(S) client engine behind gala's winhttp.dll and wininet.dll:
//! one implementation of what both APIs do the same way, each DLL
//! wrapping it in its own functions, flags and error codes.
//!
//! - `net`: the connection: TCP through ws2_32, TLS through SSPI's
//!   Schannel (secur32).
//! - `request`: a request and its response: headers, the body's framing,
//!   the header queries (WinHttpQueryHeaders and HttpQueryInfo take the
//!   same levels and flags, WinHTTP having kept WinINet's numbers).
//! - `url`: URL_COMPONENTS cracking (one layout in both APIs; the scheme
//!   numbers differ and are the caller's).
//! - `handles`: the live-handle list for validating HINTERNETs.
//!
//! Errors are `u32` codes WinHTTP and WinINet share (ERROR_WINHTTP_* has
//! ERROR_INTERNET_* / ERROR_HTTP_*'s numbers, `err`), except two the DLL
//! translates: a certificate failure is the SSPI or CERT_E_* status
//! itself (an HRESULT, never a Win32 code: `is_certificate`), and
//! `err::HEADER_SIZE_OVERFLOW` is WinHTTP's alone.
//!
//! What a failure leaves: every failed step records `<dll>!<step>`
//! (kind "other") with the Winsock or SSPI status as detail; an
//! unsupported path records `<dll>!<what>` (kind "unsupported"), `<dll>`
//! being the DLL the request belongs to (`Dll`).

#![no_std]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

extern crate alloc;

pub mod handles;
pub mod net;
pub mod request;
pub mod url;

use alloc::vec::Vec;

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn GalaRecordMissing(name: *const u8, length: usize, what: u32, detail: u64) -> u32;
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn ExitProcess(code: u32) -> !;
}

/// The codes both APIs share (WinINet's names in the comments).
pub mod err {
    pub const NOT_ENOUGH_MEMORY: u32 = 8;
    pub const INVALID_HANDLE: u32 = 6;
    pub const NOT_SUPPORTED: u32 = 50;
    pub const INVALID_PARAMETER: u32 = 87;
    pub const INSUFFICIENT_BUFFER: u32 = 122;
    /// ERROR_INTERNET_TIMEOUT.
    pub const TIMEOUT: u32 = 12002;
    /// ERROR_INTERNET_INVALID_URL.
    pub const INVALID_URL: u32 = 12005;
    /// ERROR_INTERNET_UNRECOGNIZED_SCHEME.
    pub const UNRECOGNIZED_SCHEME: u32 = 12006;
    /// ERROR_INTERNET_NAME_NOT_RESOLVED.
    pub const NAME_NOT_RESOLVED: u32 = 12007;
    /// ERROR_INTERNET_INVALID_OPTION.
    pub const INVALID_OPTION: u32 = 12009;
    /// ERROR_INTERNET_INCORRECT_HANDLE_TYPE.
    pub const INCORRECT_HANDLE_TYPE: u32 = 12018;
    /// ERROR_INTERNET_INCORRECT_HANDLE_STATE.
    pub const INCORRECT_HANDLE_STATE: u32 = 12019;
    /// ERROR_INTERNET_CANNOT_CONNECT.
    pub const CANNOT_CONNECT: u32 = 12029;
    /// ERROR_INTERNET_CONNECTION_ABORTED (WinHTTP: _CONNECTION_ERROR).
    pub const CONNECTION_ERROR: u32 = 12030;
    /// ERROR_HTTP_HEADER_NOT_FOUND.
    pub const HEADER_NOT_FOUND: u32 = 12150;
    /// ERROR_HTTP_INVALID_SERVER_RESPONSE.
    pub const INVALID_SERVER_RESPONSE: u32 = 12152;
    /// ERROR_HTTP_INVALID_HEADER.
    pub const INVALID_HEADER: u32 = 12153;
    /// ERROR_HTTP_INVALID_QUERY_REQUEST.
    pub const INVALID_QUERY_REQUEST: u32 = 12154;
    /// ERROR_HTTP_HEADER_ALREADY_EXISTS.
    pub const HEADER_ALREADY_EXISTS: u32 = 12155;
    /// ERROR_HTTP_REDIRECT_FAILED: a redirect the policy refuses (https
    /// to http, another scheme) or one more than the maximum.
    pub const REDIRECT_FAILED: u32 = 12156;
    /// ERROR_INTERNET_SECURITY_CHANNEL_ERROR.
    pub const SECURE_CHANNEL_ERROR: u32 = 12157;
    /// ERROR_WINHTTP_HEADER_SIZE_OVERFLOW (WinHTTP only).
    pub const HEADER_SIZE_OVERFLOW: u32 = 12181;
}

/// SECURITY_FLAG_IGNORE_UNKNOWN_CA, _WRONG_USAGE, _CERT_CN_INVALID,
/// _CERT_DATE_INVALID: the same bits in both APIs (WinINet's
/// INTERNET_FLAG_IGNORE_CERT_CN_INVALID / _DATE_INVALID too).
pub const IGNORE_UNKNOWN_CA: u32 = 0x100;
pub const IGNORE_WRONG_USAGE: u32 = 0x200;
pub const IGNORE_CN_INVALID: u32 = 0x1000;
pub const IGNORE_DATE_INVALID: u32 = 0x2000;

/// SSPI and CERT_E_* statuses a failed handshake returns for the server's
/// certificate (`is_certificate`), for the DLL's own codes.
pub mod cert {
    pub const SEC_E_WRONG_PRINCIPAL: u32 = 0x8009_0322;
    pub const SEC_E_UNTRUSTED_ROOT: u32 = 0x8009_0325;
    pub const SEC_E_CERT_UNKNOWN: u32 = 0x8009_0327;
    pub const SEC_E_CERT_EXPIRED: u32 = 0x8009_0328;
    pub const SEC_E_CERT_WRONG_USAGE: u32 = 0x8009_0349;
    pub const CERT_E_EXPIRED: u32 = 0x800b_0101;
    pub const CERT_E_UNTRUSTEDROOT: u32 = 0x800b_0109;
    pub const CERT_E_CHAINING: u32 = 0x800b_010a;
    pub const CERT_E_REVOKED: u32 = 0x800b_010c;
    pub const CERT_E_CN_NO_MATCH: u32 = 0x800b_010f;
    pub const CERT_E_UNTRUSTEDCA: u32 = 0x800b_0112;
}

/// An error that is a certificate's SSPI / CERT_E_* status.
pub fn is_certificate(e: u32) -> bool {
    e >= 0x8000_0000
}

/// The DLL a request belongs to, for its ledger records ("winhttp.dll").
pub type Dll = &'static str;

fn record(dll: Dll, what: &[u8], kind: u32, detail: u64) {
    let mut text = [0u8; 160];
    let mut n = 0;
    for &b in dll.as_bytes().iter().chain(b"!").chain(what) {
        if n < text.len() {
            text[n] = b;
            n += 1;
        }
    }
    // SAFETY: a valid buffer of `n` bytes.
    unsafe { GalaRecordMissing(text.as_ptr(), n, kind, detail) };
}

/// Records a gap (`<dll>!<what>`, "unsupported") in the ledger.
pub fn missing(dll: Dll, what: &[u8], detail: u64) {
    record(dll, what, 2, detail);
}

/// Records why a call failed (`<dll>!<step>`, kind "other", the Winsock
/// or SSPI status as detail): what a failed request leaves behind.
pub fn failed(dll: Dll, step: &[u8], detail: u64) {
    record(dll, step, 5, detail);
}

/// Fallible Vec growth (the heap's failure must not end the process).
pub fn put<T: Copy>(v: &mut Vec<T>, items: &[T]) -> Result<(), u32> {
    v.try_reserve(items.len()).map_err(|_| err::NOT_ENOUGH_MEMORY)?;
    v.extend_from_slice(items);
    Ok(())
}

/// The caller's UTF-16 string: NUL-terminated (`len` None) or `len` units.
pub fn wide(p: *const u16, len: Option<usize>) -> Result<Vec<u16>, u32> {
    let mut v = Vec::new();
    if p.is_null() {
        return Ok(v);
    }
    // SAFETY: the caller's string, as documented for the parameter.
    let n = len.unwrap_or_else(|| unsafe {
        let mut n = 0;
        while *p.add(n) != 0 {
            n += 1;
        }
        n
    });
    // SAFETY: `n` units of the caller's string.
    put(&mut v, unsafe { core::slice::from_raw_parts(p, n) })?;
    Ok(v)
}

/// UTF-16 to bytes for the wire: Latin-1 code points as themselves (what
/// HTTP headers carry); others become '?' (recorded).
pub fn narrow(dll: Dll, w: &[u16]) -> Result<Vec<u8>, u32> {
    let mut v = Vec::new();
    v.try_reserve(w.len()).map_err(|_| err::NOT_ENOUGH_MEMORY)?;
    for &c in w {
        if c > 0xff {
            missing(dll, b"non-Latin-1 header text", c.into());
        }
        v.push(if c > 0xff { b'?' } else { c as u8 });
    }
    Ok(v)
}

/// Time-outs in milliseconds; -1 waits forever.
#[derive(Clone, Copy)]
pub struct Timeouts {
    pub resolve: i32,
    pub connect: i32,
    pub send: i32,
    pub receive: i32,
    /// For the response's headers.
    pub response: i32,
}

/// What a session passes on to the handles made from it.
#[derive(Clone)]
pub struct Settings {
    pub dll: Dll,
    pub agent: Vec<u8>,
    pub timeouts: Timeouts,
    /// SP_PROT_* (0: Schannel's defaults).
    pub protocols: u32,
    /// Which redirects are followed, in WinHTTP's numbers
    /// (WINHTTP_OPTION_REDIRECT_POLICY): `REDIRECT_NEVER`,
    /// `REDIRECT_DISALLOW_HTTPS_TO_HTTP` or `REDIRECT_ALWAYS`.
    pub redirect_policy: u32,
    /// How many redirects one request follows at most
    /// (WINHTTP_OPTION_MAX_HTTP_AUTOMATIC_REDIRECTS).
    pub max_redirects: u32,
    /// Which statuses redirect and what they do to the verb (the APIs
    /// differ).
    pub redirect_rules: RedirectRules,
}

/// The redirect rules of each API (as Wine's conformance tests, run on
/// Windows, show them).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RedirectRules {
    /// WinHTTP: 301, 302, 307 and 308 (not 303); a 301 or 302 turns a
    /// POST into a GET without its body.
    WinHttp,
    /// WinINet: 301, 302, 303, 307 and 308; any verb but GET and HEAD
    /// becomes a GET without its body, except for 307 and 308.
    WinInet,
}

/// WINHTTP_OPTION_REDIRECT_POLICY_NEVER: a 3xx response is returned.
pub const REDIRECT_NEVER: u32 = 0;
/// WINHTTP_OPTION_REDIRECT_POLICY_DISALLOW_HTTPS_TO_HTTP: every redirect
/// is followed except one from https to http (`err::REDIRECT_FAILED`).
pub const REDIRECT_DISALLOW_HTTPS_TO_HTTP: u32 = 1;
/// WINHTTP_OPTION_REDIRECT_POLICY_ALWAYS.
pub const REDIRECT_ALWAYS: u32 = 2;

/// A DLL's panic handler: a panic is a bug, so its location goes to the
/// ledger (`<dll>!panic <file>`, the line as detail) and the process ends
/// with STATUS_STACK_BUFFER_OVERRUN (fail-fast), as schannel.dll's do.
pub fn fail_fast(dll: Dll, info: &core::panic::PanicInfo) -> ! {
    let mut text = [0u8; 96];
    let mut n = 0;
    for &b in b"panic " {
        text[n] = b;
        n += 1;
    }
    let line = info.location().map_or(0, |l| {
        let file = l.file().as_bytes();
        for &b in &file[file.len().saturating_sub(60)..] {
            if n < text.len() {
                text[n] = if b == b'\\' { b'/' } else { b };
                n += 1;
            }
        }
        l.line()
    });
    failed(dll, &text[..n], line.into());
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xc000_0409) }
}

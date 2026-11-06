//! gala's wininet.dll: WinINet's HTTP functions, synchronous, as
//! Microsoft documents them (learn.microsoft.com, "WinINet functions",
//! "HttpQueryInfo", "Query Info Flags", "Option Flags", "WinINet
//! errors"): InternetOpenW, InternetConnectW (INTERNET_SERVICE_HTTP),
//! HttpOpenRequestW, HttpAddRequestHeadersW, HttpSendRequestW (the
//! request and its response's headers), HttpQueryInfoW,
//! InternetQueryDataAvailable, InternetReadFile, InternetOpenUrlW (http
//! and https), InternetCrackUrlW, InternetSetOptionW and
//! InternetCloseHandle; redirects are followed (301, 302, 303, 307, 308)
//! unless INTERNET_FLAG_NO_AUTO_REDIRECT. The engine is
//! `user/http_client`, shared with
//! winhttp.dll (the same connection, request, framing and header
//! queries); this crate is WinINet's functions, flags and error codes
//! over it.
//!
//! Limits (each recorded in the ledger where a program meets it):
//! asynchronous operation (INTERNET_FLAG_ASYNC: ERROR_NOT_SUPPORTED),
//! proxies (INTERNET_OPEN_TYPE_PROXY), FTP and Gopher, authentication (user names
//! and passwords are not sent), the cache (every request goes to the
//! server, as with INTERNET_FLAG_RELOAD), cookies, decompression,
//! connection reuse and the connect time-out (the stack's own applies).
//! InternetTimeFromSystemTimeW / InternetTimeToSystemTimeW are
//! `gala_stubs` stubs. A handle must not be closed while another thread
//! uses it; closing a parent leaves its children working.

#![no_std]
#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

extern crate alloc;
use gala_heap as _;

mod unimplemented;

use alloc::vec::Vec;
use http_client::cert::{
    CERT_E_CHAINING, CERT_E_CN_NO_MATCH, CERT_E_EXPIRED, CERT_E_REVOKED, CERT_E_UNTRUSTEDCA,
    CERT_E_UNTRUSTEDROOT, SEC_E_CERT_EXPIRED, SEC_E_UNTRUSTED_ROOT, SEC_E_WRONG_PRINCIPAL,
};
use http_client::handles::Handles;
use http_client::request::{ADDREQ_FLAG_ADD, Open, QUERY_CUSTOM, Request};
use http_client::url::UrlComponents;
use http_client::{
    IGNORE_CN_INVALID, IGNORE_DATE_INVALID, REDIRECT_ALWAYS, REDIRECT_NEVER, RedirectRules,
    Settings, Timeouts, err, narrow, wide,
};

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

/// MSVC-target codegen references this marker symbol directly, so it cannot
/// come from an import.
#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn SetLastError(e: u32);
}

/// This DLL, in ledger records.
const DLL: http_client::Dll = "wininet.dll";

const ERROR_INVALID_PARAMETER: u32 = err::INVALID_PARAMETER;
const ERROR_NOT_SUPPORTED: u32 = err::NOT_SUPPORTED;
const ERROR_INSUFFICIENT_BUFFER: u32 = err::INSUFFICIENT_BUFFER;
const ERROR_INTERNET_INCORRECT_HANDLE_TYPE: u32 = err::INCORRECT_HANDLE_TYPE;
const ERROR_INTERNET_SEC_CERT_DATE_INVALID: u32 = 12037;
const ERROR_INTERNET_SEC_CERT_CN_INVALID: u32 = 12038;
const ERROR_INTERNET_INVALID_CA: u32 = 12045;
const ERROR_INTERNET_SEC_INVALID_CERT: u32 = 12169;
const ERROR_INTERNET_SEC_CERT_REVOKED: u32 = 12170;

const INTERNET_OPEN_TYPE_PRECONFIG: u32 = 0;
const INTERNET_OPEN_TYPE_DIRECT: u32 = 1;
const INTERNET_OPEN_TYPE_PRECONFIG_WITH_NO_AUTOPROXY: u32 = 4;
const INTERNET_SERVICE_HTTP: u32 = 3;

const INTERNET_FLAG_ASYNC: u32 = 0x1000_0000;
const INTERNET_FLAG_FROM_CACHE: u32 = 0x0100_0000;
const INTERNET_FLAG_SECURE: u32 = 0x0080_0000;
const INTERNET_FLAG_NO_AUTO_REDIRECT: u32 = 0x0020_0000;

const INTERNET_OPTION_CONNECT_TIMEOUT: u32 = 2;
const INTERNET_OPTION_CONNECT_RETRIES: u32 = 3;
const INTERNET_OPTION_SEND_TIMEOUT: u32 = 5;
const INTERNET_OPTION_RECEIVE_TIMEOUT: u32 = 6;
const INTERNET_OPTION_DATA_SEND_TIMEOUT: u32 = 7;
const INTERNET_OPTION_DATA_RECEIVE_TIMEOUT: u32 = 8;
const INTERNET_OPTION_SECURITY_FLAGS: u32 = 31;
const INTERNET_OPTION_USER_AGENT: u32 = 41;
const INTERNET_OPTION_CONTEXT_VALUE: u32 = 45;

/// INTERNET_SCHEME_HTTP and _HTTPS in WinINet's numbering.
const SCHEMES: [u32; 2] = [3, 4];

/// gala's defaults: what WinHTTP documents for the connect, send and
/// receive time-outs; the response's headers wait as long as a receive.
const DEFAULT_TIMEOUTS: Timeouts =
    Timeouts { resolve: -1, connect: 60_000, send: 30_000, receive: 30_000, response: 30_000 };

fn missing(what: &[u8], detail: u64) {
    http_client::missing(DLL, what, detail);
}

/// The engine's error as WinINet's code: a certificate failure by its
/// kind ("WinINet errors": an unknown issuer is ERROR_INTERNET_INVALID_CA,
/// an expired certificate _SEC_CERT_DATE_INVALID, a wrong name
/// _SEC_CERT_CN_INVALID, a revoked one _SEC_CERT_REVOKED, other problems
/// _SEC_INVALID_CERT); WinHTTP's header overflow has no WinINet code and
/// is ERROR_HTTP_INVALID_SERVER_RESPONSE (the engine recorded the step).
fn code(e: u32) -> u32 {
    if http_client::is_certificate(e) {
        return match e {
            SEC_E_UNTRUSTED_ROOT | CERT_E_UNTRUSTEDROOT | CERT_E_UNTRUSTEDCA | CERT_E_CHAINING => {
                ERROR_INTERNET_INVALID_CA
            }
            SEC_E_CERT_EXPIRED | CERT_E_EXPIRED => ERROR_INTERNET_SEC_CERT_DATE_INVALID,
            SEC_E_WRONG_PRINCIPAL | CERT_E_CN_NO_MATCH => ERROR_INTERNET_SEC_CERT_CN_INVALID,
            CERT_E_REVOKED => ERROR_INTERNET_SEC_CERT_REVOKED,
            _ => ERROR_INTERNET_SEC_INVALID_CERT,
        };
    }
    if e == err::HEADER_SIZE_OVERFLOW { err::INVALID_SERVER_RESPONSE } else { e }
}

fn set_error(e: u32) {
    // SAFETY: a plain call.
    unsafe { SetLastError(code(e)) };
}

/// Ends a BOOL entry point: TRUE, or FALSE with the error set.
fn done(r: Result<(), u32>) -> i32 {
    match r {
        Ok(()) => {
            set_error(0);
            1
        }
        Err(e) => {
            set_error(e);
            0
        }
    }
}

/// Ends an HINTERNET entry point: the handle, or NULL with the error set.
fn handle(r: Result<Kind, u32>, context: usize) -> usize {
    match r.and_then(|kind| HANDLES.insert(Object { kind, context })) {
        Ok(h) => {
            set_error(0);
            h
        }
        Err(e) => {
            set_error(e);
            0
        }
    }
}

struct Connect {
    settings: Settings,
    host: Vec<u16>,
    port: u16,
}

// Each object is a heap block of its own: boxing the request would only
// add a second one.
#[allow(clippy::large_enum_variant)]
enum Kind {
    Session(Settings),
    Connect(Connect),
    Request(Request),
}

/// What an HINTERNET points to.
struct Object {
    kind: Kind,
    /// dwContext (INTERNET_OPTION_CONTEXT_VALUE).
    context: usize,
}

impl Object {
    fn settings(&mut self) -> &mut Settings {
        match &mut self.kind {
            Kind::Session(s) => s,
            Kind::Connect(c) => &mut c.settings,
            Kind::Request(r) => &mut r.settings,
        }
    }
}

static HANDLES: Handles<Object> = Handles::new();

fn request_of(h: usize) -> Result<&'static mut Request, u32> {
    match &mut HANDLES.get(h)?.kind {
        Kind::Request(r) => Ok(r),
        _ => Err(ERROR_INTERNET_INCORRECT_HANDLE_TYPE),
    }
}

/// The caller's header text: `len` characters, or NUL-terminated when it
/// is -1 (HttpSendRequestW, HttpAddRequestHeadersW, InternetOpenUrlW).
fn headers_text(p: *const u16, len: u32) -> Result<Vec<u16>, u32> {
    if p.is_null() || len == 0 {
        return Ok(Vec::new());
    }
    wide(p, if len == u32::MAX { None } else { Some(len as usize) })
}

#[unsafe(no_mangle)]
pub extern "system" fn InternetOpenW(
    agent: *const u16,
    access: u32,
    _proxy: *const u16,
    _bypass: *const u16,
    flags: u32,
) -> usize {
    let r = (|| {
        if flags & INTERNET_FLAG_ASYNC != 0 {
            missing(b"InternetOpenW(INTERNET_FLAG_ASYNC)", flags.into());
            return Err(ERROR_NOT_SUPPORTED);
        }
        if flags & INTERNET_FLAG_FROM_CACHE != 0 {
            // No cache: every request goes to the server.
            missing(b"InternetOpenW(INTERNET_FLAG_OFFLINE)", flags.into());
        }
        match access {
            // No proxy is configured on gala: the preconfigured settings
            // are direct connections.
            INTERNET_OPEN_TYPE_PRECONFIG
            | INTERNET_OPEN_TYPE_DIRECT
            | INTERNET_OPEN_TYPE_PRECONFIG_WITH_NO_AUTOPROXY => {}
            _ => {
                missing(b"InternetOpenW(proxy)", access.into());
                return Err(ERROR_NOT_SUPPORTED);
            }
        }
        let agent = narrow(DLL, &wide(agent, None)?)?;
        Ok(Kind::Session(Settings {
            dll: DLL,
            agent,
            timeouts: DEFAULT_TIMEOUTS,
            protocols: 0,
            // WinINet follows every redirect (https to http included)
            // unless INTERNET_FLAG_NO_AUTO_REDIRECT; the engine's limit
            // stands for its "all attempts made to redirect failed".
            redirect_policy: REDIRECT_ALWAYS,
            max_redirects: 10,
            redirect_rules: RedirectRules::WinInet,
        }))
    })();
    handle(r, 0)
}

#[unsafe(no_mangle)]
pub extern "system" fn InternetConnectW(
    session: usize,
    server: *const u16,
    port: u16,
    user: *const u16,
    password: *const u16,
    service: u32,
    _flags: u32,
    context: usize,
) -> usize {
    let r = (|| {
        let settings = match &HANDLES.get(session)?.kind {
            Kind::Session(s) => s.clone(),
            _ => return Err(ERROR_INTERNET_INCORRECT_HANDLE_TYPE),
        };
        if service != INTERNET_SERVICE_HTTP {
            // FTP, Gopher.
            missing(b"InternetConnectW(service)", service.into());
            return Err(ERROR_NOT_SUPPORTED);
        }
        if server.is_null() {
            return Err(ERROR_INVALID_PARAMETER);
        }
        if !user.is_null() || !password.is_null() {
            // Not sent: no authentication yet.
            missing(b"InternetConnectW(credentials)", 0);
        }
        let host = wide(server, None)?;
        if host.is_empty() {
            return Err(ERROR_INVALID_PARAMETER);
        }
        Ok(Kind::Connect(Connect { settings, host, port }))
    })();
    handle(r, context)
}

/// A request from HttpOpenRequestW's or InternetOpenUrlW's flags.
fn open_request(
    c: &Connect,
    verb: *const u16,
    object: *const u16,
    version: *const u16,
    referrer: *const u16,
    accept: *const *const u16,
    flags: u32,
) -> Result<Request, u32> {
    let mut settings = c.settings.clone();
    if flags & INTERNET_FLAG_NO_AUTO_REDIRECT != 0 {
        settings.redirect_policy = REDIRECT_NEVER;
    }
    let mut r = Request::new(
        settings,
        &Open {
            host: &c.host,
            port: c.port,
            verb,
            object,
            version,
            referrer,
            accept,
            secure: flags & INTERNET_FLAG_SECURE != 0,
            escape: true,
        },
    )?;
    // INTERNET_FLAG_IGNORE_CERT_CN_INVALID / _DATE_INVALID are the
    // security flags' bits.
    r.security_flags = flags & (IGNORE_CN_INVALID | IGNORE_DATE_INVALID);
    Ok(r)
}

#[unsafe(no_mangle)]
pub extern "system" fn HttpOpenRequestW(
    connect: usize,
    verb: *const u16,
    object: *const u16,
    version: *const u16,
    referrer: *const u16,
    accept: *const *const u16,
    flags: u32,
    context: usize,
) -> usize {
    let r = (|| {
        let c = match &HANDLES.get(connect)?.kind {
            Kind::Connect(c) => c,
            _ => return Err(ERROR_INTERNET_INCORRECT_HANDLE_TYPE),
        };
        open_request(c, verb, object, version, referrer, accept, flags).map(Kind::Request)
    })();
    handle(r, context)
}

#[unsafe(no_mangle)]
pub extern "system" fn HttpAddRequestHeadersW(
    h: usize,
    headers: *const u16,
    len: u32,
    modifiers: u32,
) -> i32 {
    done((|| {
        let r = request_of(h)?;
        if headers.is_null() {
            return Err(ERROR_INVALID_PARAMETER);
        }
        r.add_headers(&headers_text(headers, len)?, modifiers)
    })())
}

#[unsafe(no_mangle)]
pub extern "system" fn HttpSendRequestW(
    h: usize,
    headers: *const u16,
    headers_length: u32,
    optional: *const u8,
    optional_length: u32,
) -> i32 {
    done((|| {
        let r = request_of(h)?;
        let headers = headers_text(headers, headers_length)?;
        let optional: &[u8] = if optional.is_null() || optional_length == 0 {
            &[]
        } else {
            // SAFETY: the caller's buffer of `optional_length` bytes.
            unsafe { core::slice::from_raw_parts(optional, optional_length as usize) }
        };
        // WinINet's send includes the response's status line and headers.
        r.send(&headers, optional, optional.len() as u64)?;
        r.receive()
    })())
}

#[unsafe(no_mangle)]
pub extern "system" fn HttpQueryInfoW(
    h: usize,
    level: u32,
    buffer: *mut u8,
    length: *mut u32,
    index: *mut u32,
) -> i32 {
    done((|| {
        let r = request_of(h)?;
        if length.is_null() {
            return Err(ERROR_INVALID_PARAMETER);
        }
        // HTTP_QUERY_CUSTOM: the header's name is in the buffer.
        let name = if level & 0xffff == QUERY_CUSTOM {
            if buffer.is_null() {
                return Err(ERROR_INVALID_PARAMETER);
            }
            let p = buffer.cast::<u16>().cast_const();
            // SAFETY: the caller's DWORD.
            let most = unsafe { *length } as usize / 2;
            // SAFETY: the caller's buffer of `*length` bytes.
            let n = (0..most).position(|i| unsafe { p.add(i).read_unaligned() } == 0);
            let n = n.ok_or(ERROR_INVALID_PARAMETER)?;
            let mut name = Vec::new();
            name.try_reserve(n).map_err(|_| err::NOT_ENOUGH_MEMORY)?;
            // SAFETY: `n` characters of the caller's buffer.
            name.extend((0..n).map(|i| unsafe { p.add(i).read_unaligned() }));
            name
        } else {
            Vec::new()
        };
        // SAFETY: the caller's length, buffer and index, as documented.
        unsafe { r.query_info(level, &name, buffer, length, index) }
    })())
}

#[unsafe(no_mangle)]
pub extern "system" fn InternetQueryDataAvailable(
    h: usize,
    available: *mut u32,
    _flags: u32,
    _context: usize,
) -> i32 {
    done((|| {
        let n = request_of(h)?.available()?;
        if !available.is_null() {
            // SAFETY: the caller's DWORD.
            unsafe { *available = n.min(u32::MAX as usize) as u32 };
        }
        Ok(())
    })())
}

/// Reads until `len` bytes or the end of the body (0 bytes read: the end).
#[unsafe(no_mangle)]
pub extern "system" fn InternetReadFile(
    h: usize,
    buffer: *mut u8,
    len: u32,
    read: *mut u32,
) -> i32 {
    done((|| {
        let r = request_of(h)?;
        if read.is_null() || (buffer.is_null() && len > 0) {
            return Err(ERROR_INVALID_PARAMETER);
        }
        // SAFETY: the caller's DWORD.
        unsafe { *read = 0 };
        let out: &mut [u8] = if len == 0 {
            &mut []
        } else {
            // SAFETY: the caller's buffer of `len` bytes.
            unsafe { core::slice::from_raw_parts_mut(buffer, len as usize) }
        };
        let mut total = 0;
        while total < out.len() {
            let n = r.read_body(&mut out[total..])?;
            if n == 0 {
                break;
            }
            total += n;
            // SAFETY: the caller's DWORD (what was read before an error).
            unsafe { *read = total as u32 };
        }
        Ok(())
    })())
}

#[unsafe(no_mangle)]
pub extern "system" fn InternetCloseHandle(h: usize) -> i32 {
    // Dropping a request closes its connection.
    done(HANDLES.close(h))
}

#[unsafe(no_mangle)]
pub extern "system" fn InternetCrackUrlW(
    url: *const u16,
    length: u32,
    flags: u32,
    components: *mut UrlComponents,
) -> i32 {
    done(http_client::url::crack(DLL, url, length, flags, components, SCHEMES).inspect_err(|&e| {
        if e == err::UNRECOGNIZED_SCHEME {
            // WinINet also cracks ftp, file, ... URLs; gala http(s) only.
            missing(b"InternetCrackUrlW(scheme)", 0);
        }
    }))
}

/// GET of an http or https URL: InternetConnectW, HttpOpenRequestW and
/// HttpSendRequestW in one call; the request's handle.
#[unsafe(no_mangle)]
pub extern "system" fn InternetOpenUrlW(
    session: usize,
    url: *const u16,
    headers: *const u16,
    headers_length: u32,
    flags: u32,
    context: usize,
) -> usize {
    let r = (|| {
        let settings = match &HANDLES.get(session)?.kind {
            Kind::Session(s) => s.clone(),
            _ => return Err(ERROR_INTERNET_INCORRECT_HANDLE_TYPE),
        };
        if url.is_null() {
            return Err(ERROR_INVALID_PARAMETER);
        }
        let text = wide(url, None)?;
        let p = http_client::url::parse(&text).inspect_err(|&e| {
            if e == err::UNRECOGNIZED_SCHEME {
                // ftp, file, ...: http(s) only.
                missing(b"InternetOpenUrlW(scheme)", 0);
            }
        })?;
        if p.user.1 > p.user.0 {
            missing(b"InternetOpenUrlW(credentials)", 0);
        }
        let mut host = Vec::new();
        http_client::put(&mut host, &text[p.host.0..p.host.1])?;
        let c = Connect { settings, host, port: p.port };
        // The object: the path (or "/") and the extra info, NUL-terminated.
        let mut object = Vec::new();
        if p.path.0 == p.path.1 {
            http_client::put(&mut object, &[u16::from(b'/')])?;
        }
        http_client::put(&mut object, &text[p.path.0..p.extra.1])?;
        http_client::put(&mut object, &[0])?;
        let flags = if p.secure { flags | INTERNET_FLAG_SECURE } else { flags };
        let mut r = open_request(
            &c,
            core::ptr::null(),
            object.as_ptr(),
            core::ptr::null(),
            core::ptr::null(),
            core::ptr::null(),
            flags,
        )?;
        let headers = headers_text(headers, headers_length)?;
        if !headers.is_empty() {
            r.add_headers(&headers, ADDREQ_FLAG_ADD)?;
        }
        r.send(&[], &[], 0)?;
        r.receive()?;
        Ok(Kind::Request(r))
    })();
    handle(r, context)
}

/// A WinINet time-out value (milliseconds; 0 and 0xFFFFFFFF: no limit).
fn timeout(v: u32) -> i32 {
    match v {
        0 | u32::MAX => -1,
        v => v.min(i32::MAX as u32) as i32,
    }
}

/// The caller's DWORD option value.
fn dword(buffer: *const u8, len: u32) -> Result<u32, u32> {
    if buffer.is_null() || len < 4 {
        return Err(ERROR_INSUFFICIENT_BUFFER);
    }
    // SAFETY: the caller's buffer of `len` (>= 4) bytes.
    Ok(unsafe { buffer.cast::<u32>().read_unaligned() })
}

#[unsafe(no_mangle)]
pub extern "system" fn InternetSetOptionW(
    h: usize,
    option: u32,
    buffer: *const u8,
    len: u32,
) -> i32 {
    done((|| {
        if h == 0 {
            // The process-wide defaults: not kept (each session has the
            // documented ones); recorded and accepted.
            missing(b"InternetSetOptionW(global)", option.into());
            return Ok(());
        }
        let o = HANDLES.get(h)?;
        match option {
            INTERNET_OPTION_CONNECT_TIMEOUT => {
                let v = timeout(dword(buffer, len)?);
                if v != DEFAULT_TIMEOUTS.connect {
                    // Not applied (the stack's own applies).
                    missing(b"connect timeout", v as u64);
                }
                o.settings().timeouts.connect = v;
            }
            INTERNET_OPTION_SEND_TIMEOUT | INTERNET_OPTION_DATA_SEND_TIMEOUT => {
                o.settings().timeouts.send = timeout(dword(buffer, len)?);
            }
            INTERNET_OPTION_RECEIVE_TIMEOUT | INTERNET_OPTION_DATA_RECEIVE_TIMEOUT => {
                let v = timeout(dword(buffer, len)?);
                let t = &mut o.settings().timeouts;
                t.receive = v;
                t.response = v;
            }
            // One connection attempt per address.
            INTERNET_OPTION_CONNECT_RETRIES => {
                dword(buffer, len)?;
            }
            INTERNET_OPTION_SECURITY_FLAGS => {
                let v = dword(buffer, len)?;
                match &mut o.kind {
                    Kind::Request(r) => r.security_flags = v,
                    _ => return Err(ERROR_INTERNET_INCORRECT_HANDLE_TYPE),
                }
            }
            INTERNET_OPTION_USER_AGENT => {
                // `len` is in characters for string options.
                let agent = narrow(DLL, &wide(buffer.cast(), Some(len as usize))?)?;
                let mut agent = agent;
                if let Some(n) = agent.iter().position(|&b| b == 0) {
                    agent.truncate(n);
                }
                o.settings().agent = agent;
            }
            INTERNET_OPTION_CONTEXT_VALUE => {
                if buffer.is_null() || (len as usize) < size_of::<usize>() {
                    return Err(ERROR_INSUFFICIENT_BUFFER);
                }
                // SAFETY: the caller's pointer-sized value.
                o.context = unsafe { buffer.cast::<usize>().read_unaligned() };
            }
            _ => {
                // Not applied; recorded, and accepted as the many tuning
                // options are (the request still works without it).
                missing(b"InternetSetOptionW", option.into());
            }
        }
        Ok(())
    })())
}

/// No dial-up on gala: the network is there (ERROR_SUCCESS).
#[unsafe(no_mangle)]
pub extern "system" fn InternetAttemptConnect(_reserved: u32) -> u32 {
    0
}

/// DllMain (the entry point our link flags name).
#[unsafe(no_mangle)]
pub extern "system" fn mainCRTStartup(_instance: usize, _reason: u32, _reserved: usize) -> i32 {
    1
}

/// A panic is a bug: its location goes to the ledger, and the process
/// ends with STATUS_STACK_BUFFER_OVERRUN (fail-fast).
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    http_client::fail_fast(DLL, info)
}

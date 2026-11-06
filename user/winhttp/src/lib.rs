//! gala's winhttp.dll: WinHTTP's synchronous client as Microsoft
//! documents it (learn.microsoft.com, "WinHTTP Functions", "URL_COMPONENTS",
//! "Query Info Flags", "Option Flags", "Error Messages"): sessions,
//! connections and requests (heap objects behind validated handles),
//! WinHttpCrackUrl, and HTTP/1.1 over TCP (ws2_32) or TLS (SSPI's
//! Schannel through secur32): requests with added headers and a body,
//! the response's status line and headers (WinHttpQueryHeaders), and the
//! body by Content-Length, chunked transfer coding or until the server
//! closes. The engine is `user/http_client`, shared with wininet.dll;
//! this crate is WinHTTP's functions, options and error codes over it
//! (`request.rs`: the request's functions).
//!
//! Limits (each recorded in the ledger where a program meets it):
//! asynchronous sessions (WINHTTP_FLAG_ASYNC: ERROR_NOT_SUPPORTED),
//! proxies (named proxies, WinHttpGetProxyForUrl), automatic redirects
//! (the 3xx response is returned), authentication, cookies,
//! decompression, HTTP/2, connection reuse (each request has its own
//! connection) and the connect and resolve time-outs (the stack's own
//! apply). There are no Internet Explorer proxy settings on gala
//! (WinHttpGetIEProxyConfigForCurrentUser: ERROR_FILE_NOT_FOUND, as
//! documented for that case). A handle must not be closed while another
//! thread uses it.

#![no_std]
#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

extern crate alloc;
use gala_heap as _;

mod request;
mod unimplemented;

use alloc::vec::Vec;
use http_client::handles::Handles;
use http_client::request::{Open, Request};
use http_client::url::UrlComponents;
use http_client::{Settings, Timeouts, err, narrow, wide};

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
/// come from an import. (It becomes a harmless extra data export.)
#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn SetLastError(e: u32);
}

/// This DLL, in ledger records.
const DLL: http_client::Dll = "winhttp.dll";

const ERROR_FILE_NOT_FOUND: u32 = 2;
const ERROR_NOT_SUPPORTED: u32 = err::NOT_SUPPORTED;
const ERROR_INVALID_PARAMETER: u32 = err::INVALID_PARAMETER;
const ERROR_INSUFFICIENT_BUFFER: u32 = err::INSUFFICIENT_BUFFER;
const ERROR_WINHTTP_INVALID_OPTION: u32 = err::INVALID_OPTION;
const ERROR_WINHTTP_INCORRECT_HANDLE_TYPE: u32 = err::INCORRECT_HANDLE_TYPE;
const ERROR_WINHTTP_SECURE_FAILURE: u32 = 12175;

const WINHTTP_ACCESS_TYPE_DEFAULT_PROXY: u32 = 0;
const WINHTTP_ACCESS_TYPE_NO_PROXY: u32 = 1;
const WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY: u32 = 4;
const WINHTTP_FLAG_ASYNC: u32 = 0x1000_0000;
const WINHTTP_FLAG_SECURE: u32 = 0x0080_0000;
const WINHTTP_FLAG_ESCAPE_DISABLE: u32 = 0x40;

const WINHTTP_OPTION_RESOLVE_TIMEOUT: u32 = 2;
const WINHTTP_OPTION_CONNECT_TIMEOUT: u32 = 3;
const WINHTTP_OPTION_CONNECT_RETRIES: u32 = 4;
const WINHTTP_OPTION_SEND_TIMEOUT: u32 = 5;
const WINHTTP_OPTION_RECEIVE_TIMEOUT: u32 = 6;
const WINHTTP_OPTION_RECEIVE_RESPONSE_TIMEOUT: u32 = 7;
const WINHTTP_OPTION_HANDLE_TYPE: u32 = 9;
const WINHTTP_OPTION_SECURITY_FLAGS: u32 = 31;
const WINHTTP_OPTION_USER_AGENT: u32 = 41;
const WINHTTP_OPTION_CONTEXT_VALUE: u32 = 45;
const WINHTTP_OPTION_SECURE_PROTOCOLS: u32 = 84;
const WINHTTP_OPTION_REDIRECT_POLICY: u32 = 88;

/// WINHTTP_HANDLE_TYPE_SESSION, _CONNECT, _REQUEST.
const HANDLE_TYPE_SESSION: u32 = 1;
const HANDLE_TYPE_CONNECT: u32 = 2;
const HANDLE_TYPE_REQUEST: u32 = 3;

/// WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_2 and _TLS1_3 (the same bits as
/// SP_PROT_TLS1_2_CLIENT and SP_PROT_TLS1_3_CLIENT).
const PROTOCOLS_SUPPORTED: u32 = 0x800 | 0x2000;

/// Records a gap (`winhttp.dll!...`, "unsupported") in the ledger.
fn missing(what: &[u8], detail: u64) {
    http_client::missing(DLL, what, detail);
}

/// The engine's error as WinHTTP's code: every certificate failure is
/// ERROR_WINHTTP_SECURE_FAILURE (as the error list documents it).
fn code(e: u32) -> u32 {
    if http_client::is_certificate(e) { ERROR_WINHTTP_SECURE_FAILURE } else { e }
}

fn set_error(e: u32) {
    // SAFETY: a plain call.
    unsafe { SetLastError(code(e)) };
}

/// Ends an entry point: TRUE, or FALSE with the error set.
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

/// The documented defaults (WinHttpSetTimeouts; RECEIVE_RESPONSE_TIMEOUT).
const DEFAULT_TIMEOUTS: Timeouts =
    Timeouts { resolve: -1, connect: 60_000, send: 30_000, receive: 30_000, response: 90_000 };

pub struct Connect {
    pub settings: Settings,
    pub host: Vec<u16>,
    pub port: u16,
}

// Each object is a heap block of its own: boxing the request would only
// add a second one.
#[allow(clippy::large_enum_variant)]
pub enum Kind {
    Session(Settings),
    Connect(Connect),
    Request(Request),
}

/// What an HINTERNET points to.
pub struct Object {
    pub kind: Kind,
    pub context: usize,
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

/// The live handles.
static HANDLES: Handles<Object> = Handles::new();

/// A new handle for `kind` (0 with ERROR_NOT_ENOUGH_MEMORY).
fn new_handle(kind: Kind) -> usize {
    match HANDLES.insert(Object { kind, context: 0 }) {
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

/// The object behind a live handle (ERROR_INVALID_HANDLE otherwise).
fn object(h: usize) -> Result<&'static mut Object, u32> {
    HANDLES.get(h)
}

fn request_of(h: usize) -> Result<&'static mut Request, u32> {
    match &mut object(h)?.kind {
        Kind::Request(r) => Ok(r),
        _ => Err(ERROR_WINHTTP_INCORRECT_HANDLE_TYPE),
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn WinHttpOpen(
    agent: *const u16,
    access: u32,
    _proxy: *const u16,
    _bypass: *const u16,
    flags: u32,
) -> usize {
    if flags & WINHTTP_FLAG_ASYNC != 0 {
        missing(b"WinHttpOpen(WINHTTP_FLAG_ASYNC)", flags.into());
        set_error(ERROR_NOT_SUPPORTED);
        return 0;
    }
    match access {
        // No proxy is configured on gala: the default and the automatic
        // configuration are direct connections.
        WINHTTP_ACCESS_TYPE_DEFAULT_PROXY
        | WINHTTP_ACCESS_TYPE_NO_PROXY
        | WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY => {}
        _ => {
            missing(b"WinHttpOpen(proxy)", access.into());
            set_error(ERROR_NOT_SUPPORTED);
            return 0;
        }
    }
    let agent = match wide(agent, None).and_then(|a| narrow(DLL, &a)) {
        Ok(a) => a,
        Err(e) => {
            set_error(e);
            return 0;
        }
    };
    new_handle(Kind::Session(Settings {
        dll: DLL,
        agent,
        timeouts: DEFAULT_TIMEOUTS,
        protocols: 0,
        redirect_policy: 1,
    }))
}

#[unsafe(no_mangle)]
pub extern "system" fn WinHttpConnect(
    session: usize,
    server: *const u16,
    port: u16,
    reserved: u32,
) -> usize {
    let r = (|| {
        let settings = match &object(session)?.kind {
            Kind::Session(s) => s.clone(),
            _ => return Err(ERROR_WINHTTP_INCORRECT_HANDLE_TYPE),
        };
        if server.is_null() || reserved != 0 {
            return Err(ERROR_INVALID_PARAMETER);
        }
        let host = wide(server, None)?;
        if host.is_empty() {
            return Err(ERROR_INVALID_PARAMETER);
        }
        Ok(Connect { settings, host, port })
    })();
    match r {
        Ok(c) => new_handle(Kind::Connect(c)),
        Err(e) => {
            set_error(e);
            0
        }
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn WinHttpOpenRequest(
    connect: usize,
    verb: *const u16,
    object_name: *const u16,
    version: *const u16,
    referrer: *const u16,
    accept_types: *const *const u16,
    flags: u32,
) -> usize {
    let r = (|| {
        let c = match &object(connect)?.kind {
            Kind::Connect(c) => c,
            _ => return Err(ERROR_WINHTTP_INCORRECT_HANDLE_TYPE),
        };
        Request::new(
            c.settings.clone(),
            &Open {
                host: &c.host,
                port: c.port,
                verb,
                object: object_name,
                version,
                referrer,
                accept: accept_types,
                secure: flags & WINHTTP_FLAG_SECURE != 0,
                escape: flags & WINHTTP_FLAG_ESCAPE_DISABLE == 0,
            },
        )
    })();
    match r {
        Ok(r) => new_handle(Kind::Request(r)),
        Err(e) => {
            set_error(e);
            0
        }
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn WinHttpCloseHandle(h: usize) -> i32 {
    // Dropping a request closes its connection.
    done(HANDLES.close(h))
}

/// A documented time-out value: -1 or 0 wait forever; below -1 is invalid.
fn timeout(v: i32) -> Result<i32, u32> {
    match v {
        0 | -1 => Ok(-1),
        v if v < -1 => Err(ERROR_INVALID_PARAMETER),
        v => Ok(v),
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn WinHttpSetTimeouts(
    h: usize,
    resolve: i32,
    connect: i32,
    send: i32,
    receive: i32,
) -> i32 {
    done((|| {
        let o = object(h)?;
        let (resolve, connect, send, receive) =
            (timeout(resolve)?, timeout(connect)?, timeout(send)?, timeout(receive)?);
        note_unapplied(resolve, connect);
        let t = &mut o.settings().timeouts;
        t.resolve = resolve;
        t.connect = connect;
        t.send = send;
        t.receive = receive;
        Ok(())
    })())
}

/// The resolve and connect time-outs are not applied (the stack's own
/// apply): recorded when a caller sets one other than the default.
fn note_unapplied(resolve: i32, connect: i32) {
    if resolve != DEFAULT_TIMEOUTS.resolve {
        missing(b"resolve timeout", resolve as u64);
    }
    if connect != DEFAULT_TIMEOUTS.connect {
        missing(b"connect timeout", connect as u64);
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
pub extern "system" fn WinHttpSetOption(h: usize, option: u32, buffer: *const u8, len: u32) -> i32 {
    done((|| {
        let o = object(h)?;
        match option {
            WINHTTP_OPTION_RESOLVE_TIMEOUT
            | WINHTTP_OPTION_CONNECT_TIMEOUT
            | WINHTTP_OPTION_SEND_TIMEOUT
            | WINHTTP_OPTION_RECEIVE_TIMEOUT
            | WINHTTP_OPTION_RECEIVE_RESPONSE_TIMEOUT => {
                let v = timeout(dword(buffer, len)? as i32)?;
                let t = &mut o.settings().timeouts;
                match option {
                    WINHTTP_OPTION_RESOLVE_TIMEOUT => {
                        note_unapplied(v, DEFAULT_TIMEOUTS.connect);
                        t.resolve = v
                    }
                    WINHTTP_OPTION_CONNECT_TIMEOUT => {
                        note_unapplied(DEFAULT_TIMEOUTS.resolve, v);
                        t.connect = v
                    }
                    WINHTTP_OPTION_SEND_TIMEOUT => t.send = v,
                    WINHTTP_OPTION_RECEIVE_TIMEOUT => t.receive = v,
                    _ => t.response = v,
                }
            }
            // One connection attempt per address.
            WINHTTP_OPTION_CONNECT_RETRIES => {
                dword(buffer, len)?;
            }
            WINHTTP_OPTION_CONTEXT_VALUE => {
                if buffer.is_null() || (len as usize) < size_of::<usize>() {
                    return Err(ERROR_INSUFFICIENT_BUFFER);
                }
                // SAFETY: the caller's pointer-sized value.
                o.context = unsafe { buffer.cast::<usize>().read_unaligned() };
            }
            WINHTTP_OPTION_USER_AGENT => {
                // `len` is in characters for string options.
                let agent = narrow(DLL, &wide(buffer.cast(), Some(len as usize))?)?;
                o.settings().agent = agent;
            }
            WINHTTP_OPTION_SECURE_PROTOCOLS => {
                let v = dword(buffer, len)?;
                if v & PROTOCOLS_SUPPORTED == 0 {
                    // SSL 2/3, TLS 1.0/1.1 only: Schannel has TLS 1.2 and 1.3.
                    missing(b"WINHTTP_OPTION_SECURE_PROTOCOLS", v.into());
                }
                o.settings().protocols = v & PROTOCOLS_SUPPORTED;
            }
            WINHTTP_OPTION_REDIRECT_POLICY => {
                let v = dword(buffer, len)?;
                if v > 2 {
                    return Err(ERROR_INVALID_PARAMETER);
                }
                o.settings().redirect_policy = v;
            }
            WINHTTP_OPTION_SECURITY_FLAGS => {
                let v = dword(buffer, len)?;
                match &mut o.kind {
                    Kind::Request(r) => r.security_flags = v,
                    _ => return Err(ERROR_WINHTTP_INCORRECT_HANDLE_TYPE),
                }
            }
            _ => {
                // Not applied; recorded, and accepted as the many tuning
                // options are (the request still works without it).
                missing(b"WinHttpSetOption", option.into());
            }
        }
        Ok(())
    })())
}

#[unsafe(no_mangle)]
pub extern "system" fn WinHttpQueryOption(
    h: usize,
    option: u32,
    buffer: *mut u8,
    len: *mut u32,
) -> i32 {
    done((|| {
        let o = object(h)?;
        if len.is_null() {
            return Err(ERROR_INVALID_PARAMETER);
        }
        let t = o.settings().timeouts;
        let value: usize = match option {
            WINHTTP_OPTION_RESOLVE_TIMEOUT => t.resolve as u32 as usize,
            WINHTTP_OPTION_CONNECT_TIMEOUT => t.connect as u32 as usize,
            WINHTTP_OPTION_SEND_TIMEOUT => t.send as u32 as usize,
            WINHTTP_OPTION_RECEIVE_TIMEOUT => t.receive as u32 as usize,
            WINHTTP_OPTION_RECEIVE_RESPONSE_TIMEOUT => t.response as u32 as usize,
            WINHTTP_OPTION_SECURE_PROTOCOLS => o.settings().protocols as usize,
            WINHTTP_OPTION_REDIRECT_POLICY => o.settings().redirect_policy as usize,
            WINHTTP_OPTION_HANDLE_TYPE => match o.kind {
                Kind::Session(_) => HANDLE_TYPE_SESSION as usize,
                Kind::Connect(_) => HANDLE_TYPE_CONNECT as usize,
                Kind::Request(_) => HANDLE_TYPE_REQUEST as usize,
            },
            WINHTTP_OPTION_SECURITY_FLAGS => match &o.kind {
                Kind::Request(r) => r.security_flags as usize,
                _ => return Err(ERROR_WINHTTP_INCORRECT_HANDLE_TYPE),
            },
            WINHTTP_OPTION_CONTEXT_VALUE => {
                // SAFETY: the caller's length and buffer.
                unsafe {
                    if buffer.is_null() || (*len as usize) < size_of::<usize>() {
                        *len = size_of::<usize>() as u32;
                        return Err(ERROR_INSUFFICIENT_BUFFER);
                    }
                    buffer.cast::<usize>().write_unaligned(o.context);
                    *len = size_of::<usize>() as u32;
                }
                return Ok(());
            }
            _ => {
                missing(b"WinHttpQueryOption", option.into());
                return Err(ERROR_WINHTTP_INVALID_OPTION);
            }
        };
        // SAFETY: the caller's length and buffer.
        unsafe {
            if buffer.is_null() || *len < 4 {
                *len = 4;
                return Err(ERROR_INSUFFICIENT_BUFFER);
            }
            buffer.cast::<u32>().write_unaligned(value as u32);
            *len = 4;
        }
        Ok(())
    })())
}

#[unsafe(no_mangle)]
pub extern "system" fn WinHttpAddRequestHeaders(
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
        let text = wide(headers, if len == u32::MAX { None } else { Some(len as usize) })?;
        r.add_headers(&text, modifiers)
    })())
}

#[unsafe(no_mangle)]
pub extern "system" fn WinHttpGetIEProxyConfigForCurrentUser(_config: usize) -> i32 {
    set_error(ERROR_FILE_NOT_FOUND);
    0
}

#[unsafe(no_mangle)]
pub extern "system" fn WinHttpGetProxyForUrl(
    _session: usize,
    _url: usize,
    _options: usize,
    _info: usize,
) -> i32 {
    missing(b"WinHttpGetProxyForUrl", 0);
    set_error(ERROR_NOT_SUPPORTED);
    0
}

#[unsafe(no_mangle)]
pub extern "system" fn WinHttpCrackUrl(
    url: *const u16,
    length: u32,
    flags: u32,
    components: *mut UrlComponents,
) -> i32 {
    // INTERNET_SCHEME_HTTP and _HTTPS in WinHTTP's numbering.
    done(http_client::url::crack(DLL, url, length, flags, components, [1, 2]))
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

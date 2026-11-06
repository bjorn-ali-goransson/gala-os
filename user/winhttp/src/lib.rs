//! gala's winhttp.dll: WinHTTP's synchronous client as Microsoft
//! documents it (learn.microsoft.com, "WinHTTP Functions", "URL_COMPONENTS",
//! "Query Info Flags", "Option Flags", "Error Messages"): sessions,
//! connections and requests (heap objects behind validated handles),
//! WinHttpCrackUrl, and HTTP/1.1 over TCP (ws2_32) or TLS (SSPI's
//! Schannel through secur32, `net.rs`): requests with added headers and a
//! body, the response's status line and headers (WinHttpQueryHeaders),
//! and the body by Content-Length, chunked transfer coding or until the
//! server closes (`request.rs`).
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

mod net;
mod request;
mod unimplemented;
mod url;

use alloc::vec::Vec;
use core::cell::UnsafeCell;

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

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn GalaRecordMissing(name: *const u8, length: usize, what: u32, detail: u64) -> u32;
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn SetLastError(e: u32);
    fn ExitProcess(code: u32) -> !;
    fn AcquireSRWLockExclusive(lock: *mut usize);
    fn ReleaseSRWLockExclusive(lock: *mut usize);
}

pub const ERROR_FILE_NOT_FOUND: u32 = 2;
pub const ERROR_INVALID_HANDLE: u32 = 6;
pub const ERROR_NOT_ENOUGH_MEMORY: u32 = 8;
pub const ERROR_NOT_SUPPORTED: u32 = 50;
pub const ERROR_INVALID_PARAMETER: u32 = 87;
pub const ERROR_INSUFFICIENT_BUFFER: u32 = 122;
pub const ERROR_WINHTTP_TIMEOUT: u32 = 12002;
pub const ERROR_WINHTTP_INVALID_URL: u32 = 12005;
pub const ERROR_WINHTTP_UNRECOGNIZED_SCHEME: u32 = 12006;
pub const ERROR_WINHTTP_NAME_NOT_RESOLVED: u32 = 12007;
pub const ERROR_WINHTTP_INVALID_OPTION: u32 = 12009;
pub const ERROR_WINHTTP_INCORRECT_HANDLE_TYPE: u32 = 12018;
pub const ERROR_WINHTTP_INCORRECT_HANDLE_STATE: u32 = 12019;
pub const ERROR_WINHTTP_CANNOT_CONNECT: u32 = 12029;
pub const ERROR_WINHTTP_CONNECTION_ERROR: u32 = 12030;
pub const ERROR_WINHTTP_HEADER_NOT_FOUND: u32 = 12150;
pub const ERROR_WINHTTP_INVALID_SERVER_RESPONSE: u32 = 12152;
pub const ERROR_WINHTTP_INVALID_HEADER: u32 = 12153;
pub const ERROR_WINHTTP_INVALID_QUERY_REQUEST: u32 = 12154;
pub const ERROR_WINHTTP_HEADER_ALREADY_EXISTS: u32 = 12155;
pub const ERROR_WINHTTP_SECURE_CHANNEL_ERROR: u32 = 12157;
pub const ERROR_WINHTTP_SECURE_FAILURE: u32 = 12175;
pub const ERROR_WINHTTP_HEADER_SIZE_OVERFLOW: u32 = 12181;

const WINHTTP_ACCESS_TYPE_DEFAULT_PROXY: u32 = 0;
const WINHTTP_ACCESS_TYPE_NO_PROXY: u32 = 1;
const WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY: u32 = 4;
const WINHTTP_FLAG_ASYNC: u32 = 0x1000_0000;
pub const WINHTTP_FLAG_SECURE: u32 = 0x0080_0000;
pub const WINHTTP_FLAG_ESCAPE_DISABLE: u32 = 0x40;

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
pub const PROTOCOLS_SUPPORTED: u32 = 0x800 | 0x2000;

/// Records a gap (`winhttp.dll!...`, "unsupported") in the ledger.
pub fn missing(name: &[u8], detail: u64) {
    // SAFETY: a valid buffer.
    unsafe { GalaRecordMissing(name.as_ptr(), name.len(), 2, detail) };
}

/// Records why a call failed (`winhttp.dll!<step>`, kind "other", the
/// Winsock or SSPI status as detail): what a failed request leaves behind.
pub fn failed(name: &[u8], detail: u64) {
    // SAFETY: a valid buffer.
    unsafe { GalaRecordMissing(name.as_ptr(), name.len(), 5, detail) };
}

pub fn set_error(e: u32) {
    // SAFETY: a plain call.
    unsafe { SetLastError(e) };
}

/// Ends an entry point: TRUE, or FALSE with the error set.
pub fn done(r: Result<(), u32>) -> i32 {
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

/// Fallible Vec growth (the heap's failure must not end the process).
pub fn put<T: Copy>(v: &mut Vec<T>, items: &[T]) -> Result<(), u32> {
    v.try_reserve(items.len()).map_err(|_| ERROR_NOT_ENOUGH_MEMORY)?;
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
pub fn narrow(w: &[u16]) -> Result<Vec<u8>, u32> {
    let mut v = Vec::new();
    v.try_reserve(w.len()).map_err(|_| ERROR_NOT_ENOUGH_MEMORY)?;
    for &c in w {
        if c > 0xff {
            missing(b"winhttp.dll!non-Latin-1 header text", c.into());
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
    pub response: i32,
}

/// The documented defaults (WinHttpSetTimeouts; RECEIVE_RESPONSE_TIMEOUT).
const DEFAULT_TIMEOUTS: Timeouts =
    Timeouts { resolve: -1, connect: 60_000, send: 30_000, receive: 30_000, response: 90_000 };

/// What a session passes on to the handles made from it.
#[derive(Clone)]
pub struct Settings {
    pub agent: Vec<u8>,
    pub timeouts: Timeouts,
    pub protocols: u32,
    pub redirect_policy: u32,
}

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
    Request(request::Request),
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

/// The live handles (the objects' addresses), for validation.
struct Table {
    lock: UnsafeCell<usize>,
    live: UnsafeCell<Vec<usize>>,
}

// SAFETY: `live` is only touched with `lock` (an SRW lock) held.
unsafe impl Sync for Table {}

static TABLE: Table = Table { lock: UnsafeCell::new(0), live: UnsafeCell::new(Vec::new()) };

fn with_table<R>(f: impl FnOnce(&mut Vec<usize>) -> R) -> R {
    // SAFETY: the SRW lock serialises access to the list.
    unsafe {
        AcquireSRWLockExclusive(TABLE.lock.get());
        let r = f(&mut *TABLE.live.get());
        ReleaseSRWLockExclusive(TABLE.lock.get());
        r
    }
}

/// A new handle for `kind` (0 with ERROR_NOT_ENOUGH_MEMORY).
fn new_handle(kind: Kind) -> usize {
    let layout = core::alloc::Layout::new::<Object>();
    // SAFETY: a non-zero-sized layout.
    let p = unsafe { alloc::alloc::alloc(layout) }.cast::<Object>();
    if p.is_null() {
        set_error(ERROR_NOT_ENOUGH_MEMORY);
        return 0;
    }
    // SAFETY: a fresh block for one Object.
    unsafe { p.write(Object { kind, context: 0 }) };
    let added = with_table(|t| {
        if t.try_reserve(1).is_err() {
            return false;
        }
        t.push(p as usize);
        true
    });
    if !added {
        free_object(p);
        set_error(ERROR_NOT_ENOUGH_MEMORY);
        return 0;
    }
    set_error(0);
    p as usize
}

fn free_object(p: *mut Object) {
    // SAFETY: a block from `new_handle`, no longer listed.
    unsafe {
        p.drop_in_place();
        alloc::alloc::dealloc(p.cast(), core::alloc::Layout::new::<Object>());
    }
}

/// The object behind a live handle (ERROR_INVALID_HANDLE otherwise).
pub fn object(h: usize) -> Result<&'static mut Object, u32> {
    if h == 0 || !with_table(|t| t.contains(&h)) {
        return Err(ERROR_INVALID_HANDLE);
    }
    // SAFETY: a live handle's object (closing it is the caller's business,
    // as on Windows: not while it is in use).
    Ok(unsafe { &mut *(h as *mut Object) })
}

pub fn request_of(h: usize) -> Result<&'static mut request::Request, u32> {
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
        missing(b"winhttp.dll!WinHttpOpen(WINHTTP_FLAG_ASYNC)", flags.into());
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
            missing(b"winhttp.dll!WinHttpOpen(proxy)", access.into());
            set_error(ERROR_NOT_SUPPORTED);
            return 0;
        }
    }
    let agent = match wide(agent, None).and_then(|a| narrow(&a)) {
        Ok(a) => a,
        Err(e) => {
            set_error(e);
            return 0;
        }
    };
    new_handle(Kind::Session(Settings {
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
        request::Request::new(c, verb, object_name, version, referrer, accept_types, flags)
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
    let removed = with_table(|t| match t.iter().position(|&x| x == h) {
        Some(i) => {
            t.swap_remove(i);
            true
        }
        None => false,
    });
    if !removed || h == 0 {
        set_error(ERROR_INVALID_HANDLE);
        return 0;
    }
    // Dropping a request closes its connection.
    free_object(h as *mut Object);
    set_error(0);
    1
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
        missing(b"winhttp.dll!resolve timeout", resolve as u64);
    }
    if connect != DEFAULT_TIMEOUTS.connect {
        missing(b"winhttp.dll!connect timeout", connect as u64);
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
                let agent = narrow(&wide(buffer.cast(), Some(len as usize))?)?;
                o.settings().agent = agent;
            }
            WINHTTP_OPTION_SECURE_PROTOCOLS => {
                let v = dword(buffer, len)?;
                if v & PROTOCOLS_SUPPORTED == 0 {
                    // SSL 2/3, TLS 1.0/1.1 only: Schannel has TLS 1.2 and 1.3.
                    missing(b"winhttp.dll!WINHTTP_OPTION_SECURE_PROTOCOLS", v.into());
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
                missing(b"winhttp.dll!WinHttpSetOption", option.into());
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
                missing(b"winhttp.dll!WinHttpQueryOption", option.into());
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
    missing(b"winhttp.dll!WinHttpGetProxyForUrl", 0);
    set_error(ERROR_NOT_SUPPORTED);
    0
}

/// DllMain (the entry point our link flags name).
#[unsafe(no_mangle)]
pub extern "system" fn mainCRTStartup(_instance: usize, _reason: u32, _reserved: usize) -> i32 {
    1
}

/// A panic is a bug: its location goes to the ledger, and the process
/// ends with STATUS_STACK_BUFFER_OVERRUN (fail-fast), as schannel.dll's do.
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    let mut text = [0u8; 96];
    let mut n = 0;
    for &b in b"winhttp.dll!panic " {
        text[n] = b;
        n += 1;
    }
    if let Some(l) = info.location() {
        let file = l.file().as_bytes();
        for &b in &file[file.len().saturating_sub(60)..] {
            if n < text.len() {
                text[n] = if b == b'\\' { b'/' } else { b };
                n += 1;
            }
        }
        failed(&text[..n], l.line().into());
    } else {
        failed(&text[..n], 0);
    }
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xc000_0409) }
}

//! WinHTTP's status callbacks on a synchronous session (ROADMAP OF: ODT's
//! setup.exe sets one before fetching its configuration), against Nginx
//! in the guest (user/schannel_test/nginx.conf: plain HTTP on
//! 127.0.0.1:8080, TLS on 8444 with a certificate from an untrusted
//! issuer). Expected values: Microsoft's WinHttpSetStatusCallback and
//! WINHTTP_STATUS_CALLBACK pages, and the sequences of Wine's
//! dlls/winhttp/tests/notification.c (cache_test, redirect_test), which
//! run against Windows:
//!
//! 1. WinHttpSetStatusCallback on a handle that is not one:
//!    WINHTTP_INVALID_STATUS_CALLBACK, ERROR_INVALID_HANDLE (assumed:
//!    Wine's answer). On a session: NULL (no previous callback) with the
//!    last error ERROR_SUCCESS, then the previous callback.
//! 2. A connection and a request made from the session inherit its
//!    callback (WinHttpSetStatusCallback on them returns it) and its
//!    WINHTTP_OPTION_CONTEXT_VALUE; HANDLE_CREATED tells each, its
//!    information the new handle.
//! 3. A GET to 127.0.0.1:8080 notifies, in this order, on the calling
//!    thread, with the request's handle and the context:
//!    RESOLVING_NAME (the host), NAME_RESOLVED and CONNECTING_TO_SERVER
//!    and CONNECTED_TO_SERVER (the address "127.0.0.1"), SENDING_REQUEST,
//!    REQUEST_SENT (the bytes sent, a DWORD), then in
//!    WinHttpReceiveResponse RECEIVING_RESPONSE and RESPONSE_RECEIVED (the
//!    bytes received); closing the request, then the connection, gives
//!    HANDLE_CLOSING with the handle closed (waited for: Windows may tell
//!    it from another thread).
//! 4. WinHttpSendRequest's context is the one the request's callbacks
//!    get from then on.
//! 5. The notification flags: with WINHTTP_CALLBACK_FLAG_HANDLES only
//!    HANDLE_CREATED and HANDLE_CLOSING come; a request's callback set to
//!    NULL stops its notifications.
//! 6. A redirect (GET /redirect-rel, 302 to /): RESPONSE_RECEIVED, then
//!    REDIRECT, then SENDING_REQUEST, REQUEST_SENT, RECEIVING_RESPONSE,
//!    RESPONSE_RECEIVED for the new request (the resolving and connecting
//!    ones may come between); the final status 200.
//! 7. (x64) TLS to an untrusted issuer, the callback set on the request
//!    for WINHTTP_CALLBACK_STATUS_SECURE_FAILURE only (as Wine's
//!    test_secure_connection does): SECURE_FAILURE with
//!    WINHTTP_CALLBACK_STATUS_FLAG_INVALID_CA, and WinHttpSendRequest
//!    fails with ERROR_WINHTTP_SECURE_FAILURE.
//! 8. WINHTTP_OPTION_CONFIGURE_PASSPORT_AUTH on a session takes
//!    WINHTTP_ENABLE_PASSPORT_AUTH (Wine's test_passport_auth).
//! 9. A connect time-out (WINHTTP_OPTION_CONNECT_TIMEOUT 30000, read back
//!    as set): a request still connects, and a port nobody listens on is
//!    still ERROR_WINHTTP_CANNOT_CONNECT.
//!
//! The same source builds `wow64_winhttp_cb_test`, the 32-bit (x86)
//! program (without 7: no 32-bit Schannel yet). Prints `winhttp_cb_test:
//! ok` (`wow64_winhttp_cb_test: ok`), or `<name>: check N got X (line
//! L)` and exits with N. Retries a first request (to "localhost") while
//! Nginx starts.

#![no_std]
#![no_main]

use core::ffi::c_void;
use core::ptr::null;
use core::sync::atomic::{AtomicUsize, Ordering};

use ntrt as _;

#[cfg(target_arch = "x86_64")]
const NAME: &str = "winhttp_cb_test";
#[cfg(target_arch = "x86")]
const NAME: &str = "wow64_winhttp_cb_test";

type Handle = usize;
type Callback = extern "system" fn(Handle, usize, u32, *mut c_void, u32);

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, overlapped: usize) -> i32;
    fn ExitProcess(code: u32) -> !;
    fn GetLastError() -> u32;
    fn SetLastError(e: u32);
    fn GetCurrentThreadId() -> u32;
    fn Sleep(ms: u32);
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "winhttp", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "winhttp", kind = "raw-dylib"))]
unsafe extern "system" {
    fn WinHttpOpen(
        agent: *const u16,
        access: u32,
        proxy: *const u16,
        bypass: *const u16,
        flags: u32,
    ) -> Handle;
    fn WinHttpConnect(session: Handle, server: *const u16, port: u16, reserved: u32) -> Handle;
    fn WinHttpOpenRequest(
        connect: Handle,
        verb: *const u16,
        object: *const u16,
        version: *const u16,
        referrer: *const u16,
        accept: *const *const u16,
        flags: u32,
    ) -> Handle;
    fn WinHttpSendRequest(
        request: Handle,
        headers: *const u16,
        headers_length: u32,
        optional: *const c_void,
        optional_length: u32,
        total_length: u32,
        context: usize,
    ) -> i32;
    fn WinHttpReceiveResponse(request: Handle, reserved: usize) -> i32;
    fn WinHttpQueryHeaders(
        request: Handle,
        level: u32,
        name: *const u16,
        buffer: *mut c_void,
        length: *mut u32,
        index: *mut u32,
    ) -> i32;
    fn WinHttpCloseHandle(h: Handle) -> i32;
    fn WinHttpSetStatusCallback(
        h: Handle,
        callback: Option<Callback>,
        flags: u32,
        reserved: usize,
    ) -> usize;
    fn WinHttpSetOption(h: Handle, option: u32, buffer: *const c_void, length: u32) -> i32;
    fn WinHttpQueryOption(h: Handle, option: u32, buffer: *mut c_void, length: *mut u32) -> i32;
}

const ERROR_SUCCESS: u32 = 0;
const ERROR_INVALID_HANDLE: u32 = 6;
const ERROR_WINHTTP_CANNOT_CONNECT: u32 = 12029;
#[cfg(target_arch = "x86_64")]
const ERROR_WINHTTP_SECURE_FAILURE: u32 = 12175;

const WINHTTP_ACCESS_TYPE_NO_PROXY: u32 = 1;
#[cfg(target_arch = "x86_64")]
const WINHTTP_FLAG_SECURE: u32 = 0x0080_0000;
const WINHTTP_QUERY_STATUS_CODE: u32 = 19;
const WINHTTP_QUERY_FLAG_NUMBER: u32 = 0x2000_0000;
const WINHTTP_OPTION_CONNECT_TIMEOUT: u32 = 3;
const WINHTTP_OPTION_CONTEXT_VALUE: u32 = 45;
const WINHTTP_OPTION_CONFIGURE_PASSPORT_AUTH: u32 = 83;
const WINHTTP_ENABLE_PASSPORT_AUTH: u32 = 0x1000_0000;
const WINHTTP_INVALID_STATUS_CALLBACK: usize = usize::MAX;

const RESOLVING_NAME: u32 = 0x1;
const NAME_RESOLVED: u32 = 0x2;
const CONNECTING_TO_SERVER: u32 = 0x4;
const CONNECTED_TO_SERVER: u32 = 0x8;
const SENDING_REQUEST: u32 = 0x10;
const REQUEST_SENT: u32 = 0x20;
const RECEIVING_RESPONSE: u32 = 0x40;
const RESPONSE_RECEIVED: u32 = 0x80;
const HANDLE_CREATED: u32 = 0x400;
const HANDLE_CLOSING: u32 = 0x800;
const REDIRECT: u32 = 0x4000;
#[cfg(target_arch = "x86_64")]
const SECURE_FAILURE: u32 = 0x1_0000;
#[cfg(target_arch = "x86_64")]
const FLAG_INVALID_CA: u32 = 0x8;
const ALL_NOTIFICATIONS: u32 = 0xffff_ffff;
const FLAG_HANDLES: u32 = HANDLE_CREATED | HANDLE_CLOSING;

/// The context the session is given (any recognisable value).
const CONTEXT: usize = 0x5a5a_1234;

struct Line {
    buf: [u8; 160],
    n: usize,
}

impl Line {
    fn new() -> Self {
        Self { buf: [0; 160], n: 0 }
    }

    fn s(mut self, s: &[u8]) -> Self {
        for &b in s {
            if self.n < self.buf.len() {
                self.buf[self.n] = b;
                self.n += 1;
            }
        }
        self
    }

    fn dec(self, v: u64) -> Self {
        let mut d = [0u8; 20];
        let mut i = d.len();
        let mut v = v;
        loop {
            i -= 1;
            d[i] = b'0' + (v % 10) as u8;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        self.s(&d[i..])
    }

    fn hex(self, v: u64) -> Self {
        let mut d = [0u8; 18];
        d[0] = b'0';
        d[1] = b'x';
        for i in 0..16 {
            d[2 + i] = b"0123456789abcdef"[((v >> (60 - 4 * i)) & 0xf) as usize];
        }
        self.s(&d)
    }

    fn print(self) {
        let mut written = 0;
        // SAFETY: our buffer; the standard output handle.
        unsafe {
            WriteFile(
                GetStdHandle(-11i32 as u32),
                self.buf.as_ptr(),
                self.n as u32,
                &mut written,
                0,
            )
        };
    }
}

struct Checks(u32);

impl Checks {
    #[track_caller]
    fn ok(&mut self, cond: bool, got: u64) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        Line::new()
            .s(NAME.as_bytes())
            .s(b": check ")
            .dec(u64::from(self.0))
            .s(b" got ")
            .hex(got)
            .s(b" (line ")
            .dec(u64::from(core::panic::Location::caller().line()))
            .s(b")\r\n")
            .print();
        Err(self.0)
    }

    #[track_caller]
    fn eq(&mut self, got: u64, want: u64) -> Result<(), u32> {
        self.ok(got == want, got)
    }
}

/// `text` as a NUL-terminated UTF-16 string.
fn wide<const N: usize>(text: &str) -> [u16; N] {
    let mut out = [0u16; N];
    for (slot, b) in out.iter_mut().zip(text.bytes()) {
        *slot = u16::from(b);
    }
    out
}

fn last_error() -> u32 {
    // SAFETY: a plain call.
    unsafe { GetLastError() }
}

/// One notification as the callback saw it.
#[derive(Clone, Copy)]
struct Note {
    handle: Handle,
    context: usize,
    status: u32,
    length: u32,
    /// The DWORD or handle the information points to, if it is one.
    value: usize,
    /// The string it points to, if it is one (NUL-terminated, cut).
    text: [u16; 48],
    thread: u32,
}

const EMPTY: Note =
    Note { handle: 0, context: 0, status: 0, length: 0, value: 0, text: [0; 48], thread: 0 };

const LOG_SIZE: usize = 64;

/// The notifications so far (`LOGGED` of them; a slot is taken by
/// `fetch_add`, so a callback on another thread does not clash).
struct Log(core::cell::UnsafeCell<[Note; LOG_SIZE]>);
// SAFETY: each slot is written once by the callback that took it, read
// by the main thread after.
unsafe impl Sync for Log {}
static LOG: Log = Log(core::cell::UnsafeCell::new([EMPTY; LOG_SIZE]));
static LOGGED: AtomicUsize = AtomicUsize::new(0);

extern "system" fn callback(
    h: Handle,
    context: usize,
    status: u32,
    info: *mut c_void,
    length: u32,
) {
    let i = LOGGED.fetch_add(1, Ordering::SeqCst);
    if i >= LOG_SIZE {
        return;
    }
    let mut n = Note {
        handle: h,
        context,
        status,
        length,
        // SAFETY: a plain call.
        thread: unsafe { GetCurrentThreadId() },
        ..EMPTY
    };
    if !info.is_null() {
        // SAFETY: the information each status documents: a handle, a
        // DWORD, or a NUL-terminated string.
        unsafe {
            match status {
                HANDLE_CREATED | HANDLE_CLOSING => n.value = info.cast::<usize>().read_unaligned(),
                RESOLVING_NAME | NAME_RESOLVED | CONNECTING_TO_SERVER | CONNECTED_TO_SERVER
                | REDIRECT => {
                    let s = info.cast::<u16>();
                    for j in 0..n.text.len() - 1 {
                        let c = s.add(j).read_unaligned();
                        if c == 0 {
                            break;
                        }
                        n.text[j] = c;
                    }
                }
                _ => n.value = info.cast::<u32>().read_unaligned() as usize,
            }
        }
    }
    // SAFETY: slot `i` is this call's alone.
    unsafe { (*LOG.0.get())[i] = n };
}

/// The notifications logged since `from`.
fn notes(from: usize) -> &'static [Note] {
    let to = LOGGED.load(Ordering::SeqCst).min(LOG_SIZE);
    // SAFETY: the slots below `to` are written (single-threaded callers
    // here, or waited for).
    unsafe { &(&*LOG.0.get())[from.min(to)..to] }
}

fn mark() -> usize {
    LOGGED.load(Ordering::SeqCst)
}

/// Waits (up to 5 s) until a HANDLE_CLOSING for `h` is logged since
/// `from`: Windows may tell it from another thread.
fn wait_closing(from: usize, h: Handle) -> bool {
    for _ in 0..500 {
        if notes(from).iter().any(|n| n.status == HANDLE_CLOSING && n.value == h) {
            return true;
        }
        // SAFETY: a plain call.
        unsafe { Sleep(10) };
    }
    false
}

fn text_is(n: &Note, s: &str) -> bool {
    let len = n.text.iter().position(|&c| c == 0).unwrap_or(n.text.len());
    len == s.len() && n.text.iter().zip(s.bytes()).all(|(&a, b)| a == u16::from(b))
}

fn cb_addr() -> usize {
    callback as Callback as usize
}

fn connect(c: &mut Checks, s: Handle, host: &str, port: u16) -> Result<Handle, u32> {
    // SAFETY: a NUL-terminated host.
    let h = unsafe { WinHttpConnect(s, wide::<32>(host).as_ptr(), port, 0) };
    c.ok(h != 0, last_error().into())?;
    Ok(h)
}

fn open(c: &mut Checks, con: Handle, path: &str, flags: u32) -> Result<Handle, u32> {
    // SAFETY: NUL-terminated strings; no accept types.
    let h = unsafe {
        WinHttpOpenRequest(
            con,
            wide::<4>("GET").as_ptr(),
            wide::<32>(path).as_ptr(),
            null(),
            null(),
            null(),
            flags,
        )
    };
    c.ok(h != 0, last_error().into())?;
    Ok(h)
}

/// Sends `r` with `context`; the error, or 0.
fn send(r: Handle, context: usize) -> u32 {
    // SAFETY: no headers, no data.
    if unsafe { WinHttpSendRequest(r, null(), 0, null(), 0, 0, context) } != 0 {
        0
    } else {
        last_error()
    }
}

fn status_code(c: &mut Checks, r: Handle) -> Result<u32, u32> {
    let mut status = 0u32;
    let mut len = 4u32;
    // SAFETY: a DWORD buffer.
    let ok = unsafe {
        WinHttpQueryHeaders(
            r,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            null(),
            (&raw mut status).cast(),
            &mut len,
            null::<u32>() as *mut u32,
        )
    };
    c.ok(ok != 0, last_error().into())?;
    Ok(status)
}

fn close(c: &mut Checks, h: Handle) -> Result<(), u32> {
    // SAFETY: a plain call.
    let ok = unsafe { WinHttpCloseHandle(h) };
    c.ok(ok != 0, last_error().into())
}

fn set_callback(h: Handle, cb: Option<Callback>, flags: u32) -> usize {
    // SAFETY: a plain call.
    unsafe { WinHttpSetStatusCallback(h, cb, flags, 0) }
}

/// The session, once Nginx answers.
fn session(c: &mut Checks) -> Result<Handle, u32> {
    // SAFETY: a NUL-terminated agent.
    let s = unsafe {
        WinHttpOpen(
            wide::<24>("gala winhttp_cb_test").as_ptr(),
            WINHTTP_ACCESS_TYPE_NO_PROXY,
            null(),
            null(),
            0,
        )
    };
    c.ok(s != 0, last_error().into())?;
    let mut error = ERROR_WINHTTP_CANNOT_CONNECT;
    for _ in 0..100 {
        // "localhost": 127.0.0.1's first connection is the sequence's
        // (Windows keeps connections for reuse, even across sessions).
        let con = connect(c, s, "localhost", 8080)?;
        let r = open(c, con, "/", 0)?;
        error = send(r, 0);
        close(c, r)?;
        close(c, con)?;
        if error != ERROR_WINHTTP_CANNOT_CONNECT {
            break;
        }
        // SAFETY: a plain call.
        unsafe { Sleep(200) };
    }
    c.eq(error.into(), 0)?;
    Ok(s)
}

/// 1, 2, 3 and 4: the callback's setting and inheritance, and the
/// sequence of one GET.
fn sequence(c: &mut Checks, s: Handle) -> Result<(), u32> {
    // SAFETY: plain calls.
    let main = unsafe { GetCurrentThreadId() };
    // 1.
    // SAFETY: a plain call.
    unsafe { SetLastError(0xdead_beef) };
    c.eq(
        set_callback(0x1234, Some(callback), ALL_NOTIFICATIONS) as u64,
        WINHTTP_INVALID_STATUS_CALLBACK as u64,
    )?;
    c.eq(last_error().into(), ERROR_INVALID_HANDLE.into())?;
    // SAFETY: a plain call.
    unsafe { SetLastError(0xdead_beef) };
    c.eq(set_callback(s, Some(callback), ALL_NOTIFICATIONS) as u64, 0)?;
    c.eq(last_error().into(), ERROR_SUCCESS.into())?;
    c.eq(set_callback(s, Some(callback), ALL_NOTIFICATIONS) as u64, cb_addr() as u64)?;
    let context = CONTEXT;
    // SAFETY: a pointer-sized value.
    let ok = unsafe {
        WinHttpSetOption(
            s,
            WINHTTP_OPTION_CONTEXT_VALUE,
            (&raw const context).cast(),
            size_of::<usize>() as u32,
        )
    };
    c.ok(ok != 0, last_error().into())?;
    // 2.
    let from = mark();
    let con = connect(c, s, "127.0.0.1", 8080)?;
    let n = notes(from);
    c.eq(n.len() as u64, 1)?;
    c.eq(n[0].status.into(), HANDLE_CREATED.into())?;
    c.eq(n[0].value as u64, con as u64)?;
    c.eq(n[0].length.into(), size_of::<usize>() as u64)?;
    c.eq(n[0].context as u64, CONTEXT as u64)?;
    c.eq(n[0].thread.into(), main.into())?;
    let mut got = 0usize;
    let mut len = size_of::<usize>() as u32;
    // SAFETY: a pointer-sized buffer.
    let ok = unsafe {
        WinHttpQueryOption(con, WINHTTP_OPTION_CONTEXT_VALUE, (&raw mut got).cast(), &mut len)
    };
    c.ok(ok != 0, last_error().into())?;
    c.eq(got as u64, CONTEXT as u64)?;
    let from = mark();
    let r = open(c, con, "/", 0)?;
    let n = notes(from);
    c.eq(n.len() as u64, 1)?;
    c.eq(n[0].status.into(), HANDLE_CREATED.into())?;
    c.eq(n[0].value as u64, r as u64)?;
    c.eq(n[0].context as u64, CONTEXT as u64)?;
    // 3.
    let from = mark();
    c.eq(send(r, 0).into(), 0)?;
    // SAFETY: a plain call.
    let ok = unsafe { WinHttpReceiveResponse(r, 0) };
    c.ok(ok != 0, last_error().into())?;
    let status = status_code(c, r)?;
    c.eq(status.into(), 200)?;
    let n = notes(from);
    let want = [
        RESOLVING_NAME,
        NAME_RESOLVED,
        CONNECTING_TO_SERVER,
        CONNECTED_TO_SERVER,
        SENDING_REQUEST,
        REQUEST_SENT,
        RECEIVING_RESPONSE,
        RESPONSE_RECEIVED,
    ];
    c.eq(n.len() as u64, want.len() as u64)?;
    for (note, &status) in n.iter().zip(want.iter()) {
        c.eq(note.status.into(), status.into())?;
        c.eq(note.handle as u64, r as u64)?;
        c.eq(note.context as u64, CONTEXT as u64)?;
        c.eq(note.thread.into(), main.into())?;
    }
    c.ok(text_is(&n[0], "127.0.0.1"), n[0].text[0].into())?;
    c.ok(text_is(&n[1], "127.0.0.1"), n[1].text[0].into())?;
    c.ok(text_is(&n[2], "127.0.0.1"), n[2].text[0].into())?;
    c.ok(text_is(&n[3], "127.0.0.1"), n[3].text[0].into())?;
    c.eq(n[5].length.into(), 4)?;
    c.ok(n[5].value > 0, n[5].value as u64)?;
    c.eq(n[7].length.into(), 4)?;
    c.ok(n[7].value > 0, n[7].value as u64)?;
    let from = mark();
    close(c, r)?;
    c.ok(wait_closing(from, r), 0)?;
    let n = notes(from);
    c.eq(n.len() as u64, 1)?;
    c.eq(n[0].handle as u64, r as u64)?;
    c.eq(n[0].length.into(), size_of::<usize>() as u64)?;
    c.eq(n[0].context as u64, CONTEXT as u64)?;
    // 4.
    let r = open(c, con, "/", 0)?;
    let from = mark();
    c.eq(send(r, 0x77).into(), 0)?;
    let n = notes(from);
    c.ok(n.iter().any(|n| n.status == REQUEST_SENT), n.len() as u64)?;
    for note in n {
        c.eq(note.context as u64, 0x77)?;
    }
    let from = mark();
    close(c, r)?;
    c.ok(wait_closing(from, r), 0)?;
    let from = mark();
    close(c, con)?;
    c.ok(wait_closing(from, con), 0)?;
    c.eq(notes(from).len() as u64, 1)?;
    Ok(())
}

/// 5. The notification flags, and a request's callback taken away.
fn flags(c: &mut Checks, s: Handle) -> Result<(), u32> {
    c.eq(set_callback(s, Some(callback), FLAG_HANDLES) as u64, cb_addr() as u64)?;
    let from = mark();
    let con = connect(c, s, "127.0.0.1", 8080)?;
    let r = open(c, con, "/", 0)?;
    c.eq(send(r, 0).into(), 0)?;
    // SAFETY: a plain call.
    let ok = unsafe { WinHttpReceiveResponse(r, 0) };
    c.ok(ok != 0, last_error().into())?;
    close(c, r)?;
    c.ok(wait_closing(from, r), 0)?;
    let n = notes(from);
    c.eq(n.len() as u64, 3)?;
    c.eq(n[0].status.into(), HANDLE_CREATED.into())?;
    c.eq(n[1].status.into(), HANDLE_CREATED.into())?;
    c.eq(n[2].status.into(), HANDLE_CLOSING.into())?;
    // A request's own callback: NULL, the previous one returned.
    c.eq(set_callback(s, Some(callback), ALL_NOTIFICATIONS) as u64, cb_addr() as u64)?;
    let r = open(c, con, "/", 0)?;
    c.eq(set_callback(r, None, ALL_NOTIFICATIONS) as u64, cb_addr() as u64)?;
    let from = mark();
    c.eq(send(r, 0).into(), 0)?;
    close(c, r)?;
    // SAFETY: a plain call.
    unsafe { Sleep(100) };
    c.eq(notes(from).len() as u64, 0)?;
    // The connection keeps the callback it was made with.
    c.eq(set_callback(con, Some(callback), ALL_NOTIFICATIONS) as u64, cb_addr() as u64)?;
    let from = mark();
    close(c, con)?;
    c.ok(wait_closing(from, con), 0)
}

/// 6. A redirect's notifications.
fn redirect(c: &mut Checks, s: Handle) -> Result<(), u32> {
    let con = connect(c, s, "127.0.0.1", 8080)?;
    let r = open(c, con, "/redirect-rel", 0)?;
    c.eq(send(r, 0).into(), 0)?;
    let from = mark();
    // SAFETY: a plain call.
    let ok = unsafe { WinHttpReceiveResponse(r, 0) };
    c.ok(ok != 0, last_error().into())?;
    let status = status_code(c, r)?;
    c.eq(status.into(), 200)?;
    // The order, the resolving and connecting ones left out (they may
    // or may not come).
    let mut seen = [0u32; 16];
    let mut k = 0;
    for n in notes(from) {
        if matches!(
            n.status,
            RESOLVING_NAME | NAME_RESOLVED | CONNECTING_TO_SERVER | CONNECTED_TO_SERVER
        ) {
            continue;
        }
        if k < seen.len() {
            seen[k] = n.status;
            k += 1;
        }
    }
    let want = [
        RECEIVING_RESPONSE,
        RESPONSE_RECEIVED,
        REDIRECT,
        SENDING_REQUEST,
        REQUEST_SENT,
        RECEIVING_RESPONSE,
        RESPONSE_RECEIVED,
    ];
    c.eq(k as u64, want.len() as u64)?;
    for (&got, &status) in seen.iter().zip(want.iter()) {
        c.eq(got.into(), status.into())?;
    }
    close(c, r)?;
    close(c, con)
}

/// 7. A certificate from an untrusted issuer: SECURE_FAILURE.
#[cfg(target_arch = "x86_64")]
fn secure_failure(c: &mut Checks, s: Handle) -> Result<(), u32> {
    c.eq(set_callback(s, None, ALL_NOTIFICATIONS) as u64, cb_addr() as u64)?;
    let con = connect(c, s, "localhost", 8444)?;
    let r = open(c, con, "/", WINHTTP_FLAG_SECURE)?;
    c.eq(set_callback(r, Some(callback), SECURE_FAILURE) as u64, 0)?;
    let from = mark();
    c.eq(send(r, 0).into(), ERROR_WINHTTP_SECURE_FAILURE.into())?;
    let n = notes(from);
    c.eq(n.len() as u64, 1)?;
    c.eq(n[0].status.into(), SECURE_FAILURE.into())?;
    c.eq(n[0].handle as u64, r as u64)?;
    c.eq(n[0].length.into(), 4)?;
    c.ok(n[0].value as u32 & FLAG_INVALID_CA != 0, n[0].value as u64)?;
    close(c, r)?;
    close(c, con)
}

/// 8 and 9: Passport's option and the connect time-out.
fn options(c: &mut Checks, s: Handle) -> Result<(), u32> {
    let passport = WINHTTP_ENABLE_PASSPORT_AUTH;
    // SAFETY: a DWORD.
    let ok = unsafe {
        WinHttpSetOption(s, WINHTTP_OPTION_CONFIGURE_PASSPORT_AUTH, (&raw const passport).cast(), 4)
    };
    c.ok(ok != 0, last_error().into())?;
    for (port, error) in [(8080u16, 0u32), (8090, ERROR_WINHTTP_CANNOT_CONNECT)] {
        let con = connect(c, s, "127.0.0.1", port)?;
        let timeout = 30_000u32;
        // SAFETY: a DWORD.
        let ok = unsafe {
            WinHttpSetOption(con, WINHTTP_OPTION_CONNECT_TIMEOUT, (&raw const timeout).cast(), 4)
        };
        c.ok(ok != 0, last_error().into())?;
        let mut got = 0u32;
        let mut len = 4u32;
        // SAFETY: a DWORD buffer.
        let ok = unsafe {
            WinHttpQueryOption(con, WINHTTP_OPTION_CONNECT_TIMEOUT, (&raw mut got).cast(), &mut len)
        };
        c.ok(ok != 0, last_error().into())?;
        c.eq(got.into(), 30_000)?;
        let r = open(c, con, "/", 0)?;
        c.eq(send(r, 0).into(), error.into())?;
        close(c, r)?;
        close(c, con)?;
    }
    Ok(())
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    let s = session(&mut c)?;
    sequence(&mut c, s)?;
    flags(&mut c, s)?;
    redirect(&mut c, s)?;
    #[cfg(target_arch = "x86_64")]
    secure_failure(&mut c, s)?;
    options(&mut c, s)?;
    set_callback(s, None, ALL_NOTIFICATIONS);
    close(&mut c, s)
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = match run() {
        Ok(()) => {
            Line::new().s(NAME.as_bytes()).s(b": ok\r\n").print();
            0
        }
        Err(n) => n,
    };
    // SAFETY: ends the process.
    unsafe { ExitProcess(code) }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xc000_0409) }
}

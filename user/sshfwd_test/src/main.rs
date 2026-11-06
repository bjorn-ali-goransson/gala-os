//! The far ends of ssh's port forwarding inside gala (ROADMAP SSH, scenario
//! `sshd_forward`): plain Winsock over the loopback address, with the
//! results real Windows gives.
//!
//! - `sshfwd_test serve <port>`: listens on 127.0.0.1:<port> (backlog 1),
//!   prints `sshfwd_test: listening on 127.0.0.1:<port>`, accepts one
//!   connection (`ssh -L`: sshd's direct-tcpip channel connecting out over
//!   loopback), echoes it until the peer shuts down its sending side,
//!   closes it and prints `sshfwd_test: served <n> bytes to 127.0.0.1`
//!   (the peer's address: sshd connected from the loopback address).
//! - `sshfwd_test connect <port> <bytes>`: connects to 127.0.0.1:<port>
//!   (`ssh -R`: sshd's tcpip-forward listener), sends <bytes> bytes of a
//!   pattern, shuts down its sending side, reads until end of stream (the
//!   far side echoes) and compares; prints `sshfwd_test: forwarded <n>
//!   bytes through 127.0.0.1:<port>`.
//! - Without arguments (`run-all.ps1`, `cargo xtask report`): both ends in
//!   one process: a listener on 127.0.0.1 port 0, a connection to it, a
//!   few bytes each way, the half-closes seen as end of stream.
//!
//! Exits with 0, or the number of the failing check (printed with what it
//! got: `sshfwd_test: check <n> got <value>`).

#![no_std]
#![no_main]

use core::ffi::c_void;

type Bool = i32;
type Handle = *mut c_void;
type Socket = usize;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut c_void) -> Bool;
    fn GetCommandLineW() -> *const u16;
    fn ExitProcess(code: u32) -> !;
}

#[link(name = "ws2_32", kind = "raw-dylib")]
unsafe extern "system" {
    fn WSAStartup(version: u16, data: *mut WsaData) -> i32;
    fn WSACleanup() -> i32;
    fn WSAGetLastError() -> i32;
    fn socket(af: i32, kind: i32, protocol: i32) -> Socket;
    fn bind(s: Socket, addr: *const SockaddrIn, len: i32) -> i32;
    fn listen(s: Socket, backlog: i32) -> i32;
    fn accept(s: Socket, addr: *mut SockaddrIn, len: *mut i32) -> Socket;
    fn connect(s: Socket, addr: *const SockaddrIn, len: i32) -> i32;
    fn send(s: Socket, buf: *const u8, len: i32, flags: i32) -> i32;
    fn recv(s: Socket, buf: *mut u8, len: i32, flags: i32) -> i32;
    fn shutdown(s: Socket, how: i32) -> i32;
    fn getsockname(s: Socket, addr: *mut SockaddrIn, len: *mut i32) -> i32;
    fn closesocket(s: Socket) -> i32;
}

#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
    // Stack probe for the buffers (ntdll exports it, as on Windows).
    fn __chkstk();
}

#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

#[unsafe(no_mangle)]
pub extern "C" fn __CxxFrameHandler3() -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdeac) }
}

/// WSADATA (x64 layout: the version fields, then the pointers).
#[repr(C)]
struct WsaData {
    version: u16,
    high_version: u16,
    max_sockets: u16,
    max_udp_dg: u16,
    vendor_info: *mut u8,
    description: [u8; 257],
    system_status: [u8; 129],
}

/// SOCKADDR_IN: family, port and address in network byte order.
#[repr(C)]
struct SockaddrIn {
    family: u16,
    port: u16,
    addr: [u8; 4],
    zero: [u8; 8],
}

impl SockaddrIn {
    fn new(addr: [u8; 4], port: u16) -> Self {
        Self { family: AF_INET as u16, port: port.to_be(), addr, zero: [0; 8] }
    }
}

const AF_INET: i32 = 2;
const SOCK_STREAM: i32 = 1;
const IPPROTO_TCP: i32 = 6;
const INVALID_SOCKET: Socket = !0;
const SD_SEND: i32 = 1;
const LOOPBACK: [u8; 4] = [127, 0, 0, 1];
const ADDR_LEN: i32 = size_of::<SockaddrIn>() as i32;
/// The most `connect` mode sends (and so keeps to compare).
const MAX_BYTES: usize = 1 << 20;

fn out(text: &[u8]) {
    let mut written = 0;
    // SAFETY: valid buffer and output pointer.
    unsafe {
        WriteFile(
            GetStdHandle(-11i32 as u32),
            text.as_ptr(),
            text.len() as u32,
            &mut written,
            core::ptr::null_mut(),
        )
    };
}

fn decimal(mut v: u64, buf: &mut [u8; 20]) -> &[u8] {
    let mut i = buf.len();
    loop {
        i -= 1;
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
        if v == 0 {
            return &buf[i..];
        }
    }
}

fn out_number(v: u64) {
    let mut digits = [0u8; 20];
    out(decimal(v, &mut digits));
}

struct Checks(u32);

impl Checks {
    fn ok(&mut self, cond: bool, got: u64) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        out(b"sshfwd_test: check ");
        out_number(self.0.into());
        out(b" got ");
        out_number(got);
        out(b"\r\n");
        Err(self.0)
    }

    fn eq(&mut self, got: u64, want: u64) -> Result<(), u32> {
        self.ok(got == want, got)
    }
}

fn pattern(i: usize) -> u8 {
    (i * 31 + i / 7) as u8
}

/// The stream offset of the first byte of `data` (which starts at offset
/// `at`) that is not the pattern's, or u64::MAX.
fn mismatch(data: &[u8], at: usize) -> u64 {
    data.iter()
        .enumerate()
        .find(|&(i, &b)| b != pattern(at + i))
        .map_or(u64::MAX, |(i, _)| (at + i) as u64)
}

fn last_error() -> u64 {
    // SAFETY: plain Winsock call.
    unsafe { WSAGetLastError() as u64 }
}

/// `serve <port>`: echo one connection on 127.0.0.1:<port>.
fn serve(c: &mut Checks, port: u16) -> Result<(), u32> {
    // SAFETY: documented Winsock calls with valid arguments.
    unsafe {
        let listener = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
        c.ok(listener != INVALID_SOCKET, last_error())?;
        c.eq(bind(listener, &SockaddrIn::new(LOOPBACK, port), ADDR_LEN) as u64, 0)?;
        c.eq(listen(listener, 1) as u64, 0)?;
        out(b"sshfwd_test: listening on 127.0.0.1:");
        out_number(port.into());
        out(b"\r\n");
        let mut peer = SockaddrIn::new([0; 4], 0);
        let mut len = ADDR_LEN;
        let conn = accept(listener, &mut peer, &mut len);
        c.ok(conn != INVALID_SOCKET, last_error())?;
        c.eq(len as u64, ADDR_LEN as u64)?;
        c.ok(peer.addr == LOOPBACK, u32::from_be_bytes(peer.addr).into())?;
        let mut buf = [0u8; 4096];
        let mut total = 0u64;
        loop {
            let n = recv(conn, buf.as_mut_ptr(), buf.len() as i32, 0);
            c.ok(n >= 0, last_error())?;
            if n == 0 {
                break;
            }
            let mut sent = 0;
            while sent < n {
                let m = send(conn, buf[sent as usize..].as_ptr(), n - sent, 0);
                c.ok(m > 0, last_error())?;
                sent += m;
            }
            total += n as u64;
        }
        c.eq(shutdown(conn, SD_SEND) as u64, 0)?;
        c.eq(closesocket(conn) as u64, 0)?;
        c.eq(closesocket(listener) as u64, 0)?;
        out(b"sshfwd_test: served ");
        out_number(total);
        out(b" bytes to 127.0.0.1\r\n");
    }
    Ok(())
}

/// `connect <port> <bytes>`: a round trip through 127.0.0.1:<port>.
fn connect_to(c: &mut Checks, port: u16, bytes: usize) -> Result<(), u32> {
    c.ok(bytes > 0 && bytes <= MAX_BYTES, bytes as u64)?;
    // SAFETY: documented Winsock calls with valid arguments.
    unsafe {
        let s = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
        c.ok(s != INVALID_SOCKET, last_error())?;
        c.eq(connect(s, &SockaddrIn::new(LOOPBACK, port), ADDR_LEN) as u64, 0)?;
        // Sends and receives alternate in chunks, so neither side's
        // buffers have to hold everything.
        let mut buf = [0u8; 4096];
        let (mut sent, mut got) = (0usize, 0usize);
        while sent < bytes {
            let n = (bytes - sent).min(buf.len());
            for (i, b) in buf[..n].iter_mut().enumerate() {
                *b = pattern(sent + i);
            }
            let m = send(s, buf.as_ptr(), n as i32, 0);
            c.ok(m > 0, last_error())?;
            sent += m as usize;
            // Takes what has come back so far without waiting for all.
            while got + buf.len() <= sent {
                let r = recv(s, buf.as_mut_ptr(), buf.len() as i32, 0);
                c.ok(r > 0, last_error())?;
                c.eq(mismatch(&buf[..r as usize], got), u64::MAX)?;
                got += r as usize;
            }
        }
        c.eq(shutdown(s, SD_SEND) as u64, 0)?;
        loop {
            let r = recv(s, buf.as_mut_ptr(), buf.len() as i32, 0);
            c.ok(r >= 0, last_error())?;
            if r == 0 {
                break;
            }
            c.eq(mismatch(&buf[..r as usize], got), u64::MAX)?;
            got += r as usize;
        }
        c.eq(got as u64, bytes as u64)?;
        c.eq(closesocket(s) as u64, 0)?;
        out(b"sshfwd_test: forwarded ");
        out_number(got as u64);
        out(b" bytes through 127.0.0.1:");
        out_number(port.into());
        out(b"\r\n");
    }
    Ok(())
}

/// No arguments: both ends of a loopback connection in this process.
fn self_test(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: documented Winsock calls with valid arguments.
    unsafe {
        let listener = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
        c.ok(listener != INVALID_SOCKET, last_error())?;
        c.eq(bind(listener, &SockaddrIn::new(LOOPBACK, 0), ADDR_LEN) as u64, 0)?;
        c.eq(listen(listener, 1) as u64, 0)?;
        let mut at = SockaddrIn::new([0; 4], 0);
        let mut len = ADDR_LEN;
        c.eq(getsockname(listener, &mut at, &mut len) as u64, 0)?;
        c.ok(at.addr == LOOPBACK && at.port != 0, u16::from_be(at.port).into())?;
        // The connection completes before accept (the backlog holds it).
        let client = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
        c.ok(client != INVALID_SOCKET, last_error())?;
        c.eq(connect(client, &at, ADDR_LEN) as u64, 0)?;
        let server = accept(listener, core::ptr::null_mut(), core::ptr::null_mut());
        c.ok(server != INVALID_SOCKET, last_error())?;
        let mut buf = [0u8; 16];
        for (from, to, text) in [(client, server, b"ping"), (server, client, b"pong")] {
            c.eq(send(from, text.as_ptr(), 4, 0) as u64, 4)?;
            c.eq(shutdown(from, SD_SEND) as u64, 0)?;
            c.eq(recv(to, buf.as_mut_ptr(), 16, 0) as u64, 4)?;
            c.ok(&buf[..4] == text, buf[0].into())?;
            c.eq(recv(to, buf.as_mut_ptr(), 16, 0) as u64, 0)?;
        }
        for s in [client, server, listener] {
            c.eq(closesocket(s) as u64, 0)?;
        }
    }
    Ok(())
}

/// The command line's words after the program's name (at most 4; quotes
/// are not needed by this program's arguments).
fn words(line: &[u16]) -> ([&[u16]; 4], usize) {
    let blank = |c: &u16| *c == u16::from(b' ') || *c == u16::from(b'\t');
    // The program's name ends at its closing quote or at a blank.
    let start = if line.first() == Some(&u16::from(b'"')) {
        line[1..].iter().position(|&c| c == u16::from(b'"')).map_or(line.len(), |i| i + 2)
    } else {
        line.iter().position(blank).unwrap_or(line.len())
    };
    let mut words: [&[u16]; 4] = [&[]; 4];
    let mut count = 0;
    for part in line[start.min(line.len())..].split(blank).filter(|p| !p.is_empty()) {
        if count == words.len() {
            break;
        }
        words[count] = part;
        count += 1;
    }
    (words, count)
}

fn number(word: &[u16]) -> Option<usize> {
    if word.is_empty() || word.len() > 9 {
        return None;
    }
    word.iter().try_fold(0usize, |v, &c| match c {
        0x30..=0x39 => Some(v * 10 + usize::from(c - 0x30)),
        _ => None,
    })
}

fn run() -> Result<(), u32> {
    // SAFETY: GetCommandLineW returns a NUL-terminated string that lives
    // as long as the process.
    let line = unsafe {
        let p = GetCommandLineW();
        let mut n = 0;
        while *p.add(n) != 0 {
            n += 1;
        }
        core::slice::from_raw_parts(p, n)
    };
    let (args, count) = words(line);
    let mut c = Checks(0);
    // SAFETY: WSADATA is plain data; WSAStartup fills it.
    unsafe {
        let mut data: WsaData = core::mem::zeroed();
        c.eq(WSAStartup(0x0202, &mut data) as u64, 0)?;
    }
    let port = if count >= 2 { number(args[1]) } else { None };
    let port = port.filter(|&p| p > 0 && p < 65536).map(|p| p as u16);
    let serve_word: [u16; 5] = b"serve".map(u16::from);
    let connect_word: [u16; 7] = b"connect".map(u16::from);
    match (count, port) {
        (2, Some(port)) if args[0] == serve_word => serve(&mut c, port)?,
        (3, Some(port)) if args[0] == connect_word => {
            connect_to(&mut c, port, number(args[2]).unwrap_or(0))?
        }
        (0, _) => self_test(&mut c)?,
        _ => {
            out(b"usage: sshfwd_test serve <port> | connect <port> <bytes>; got: ");
            // The command line as given (ASCII; '?' for anything else).
            for chunk in line.chunks(64) {
                let mut text = [0u8; 64];
                for (t, &u) in text.iter_mut().zip(chunk) {
                    *t = if (0x20..0x7f).contains(&u) { u as u8 } else { b'?' };
                }
                out(&text[..chunk.len()]);
            }
            out(b"\r\n");
            return Err(0xff);
        }
    }
    // SAFETY: plain Winsock call.
    c.eq(unsafe { WSACleanup() } as u64, 0)?;
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = match run() {
        Ok(()) => {
            out(b"sshfwd_test: ok\r\n");
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
    unsafe { ExitProcess(0xdead) }
}

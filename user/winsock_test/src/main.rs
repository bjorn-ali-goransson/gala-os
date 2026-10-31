//! Winsock TCP as programs use it, with the results real Windows gives:
//! WSAStartup (version 2.2), getaddrinfo of the machine's own host name
//! and of "localhost" (IPv4 addresses), socket/bind/listen/accept/connect on the
//! loopback address, getsockname (an ephemeral port after binding port 0;
//! accept reports the client's address), send/recv round trips in both
//! directions (with MSG_PEEK), a 16 KiB transfer, a graceful close (recv
//! returns 0), WSAECONNREFUSED for a port nobody listens on, WSAENOTSOCK
//! for a closed socket, and non-blocking sockets (WSAEWOULDBLOCK from
//! accept and recv; an accepted socket inherits the listener's mode).
//! Prints `winsock_test: ok` or exits with the number of the failing check.
//!
//! With the argument `serve` it then listens on port 7 of every address,
//! prints `winsock_test: listening on port 7`, echoes one connection until
//! the peer closes it and prints `winsock_test: echoed <n> bytes` (the
//! `winsock_test` boot scenario connects from the host).

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
    fn Sleep(ms: u32);
}

#[link(name = "ws2_32", kind = "raw-dylib")]
unsafe extern "system" {
    fn WSAStartup(version: u16, data: *mut WsaData) -> i32;
    fn WSACleanup() -> i32;
    fn gethostname(name: *mut u8, len: i32) -> i32;
    fn getaddrinfo(
        node: *const u8,
        service: *const u8,
        hints: *const i32,
        out: *mut *mut u8,
    ) -> i32;
    fn freeaddrinfo(ai: *mut u8);
    fn WSAGetLastError() -> i32;
    fn socket(af: i32, kind: i32, protocol: i32) -> Socket;
    fn bind(s: Socket, addr: *const SockaddrIn, len: i32) -> i32;
    fn listen(s: Socket, backlog: i32) -> i32;
    fn accept(s: Socket, addr: *mut SockaddrIn, len: *mut i32) -> Socket;
    fn connect(s: Socket, addr: *const SockaddrIn, len: i32) -> i32;
    fn send(s: Socket, buf: *const u8, len: i32, flags: i32) -> i32;
    fn recv(s: Socket, buf: *mut u8, len: i32, flags: i32) -> i32;
    fn getsockname(s: Socket, addr: *mut SockaddrIn, len: *mut i32) -> i32;
    fn closesocket(s: Socket) -> i32;
    fn ioctlsocket(s: Socket, cmd: i32, arg: *mut u32) -> i32;
}

#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
    // Stack probe for the 16 KiB buffers (ntdll exports it, as on Windows).
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
#[derive(Clone, Copy)]
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
const SOCKET_ERROR: i32 = -1;
const MSG_PEEK: i32 = 2;
const WSAENOTSOCK: i32 = 10038;
const WSAEWOULDBLOCK: i32 = 10035;
const FIONBIO: i32 = 0x8004_667eu32 as i32;
const WSAECONNREFUSED: i32 = 10061;
const LOOPBACK: [u8; 4] = [127, 0, 0, 1];
const ADDR_LEN: i32 = size_of::<SockaddrIn>() as i32;

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

fn hex(v: u32) -> [u8; 8] {
    let mut s = [0u8; 8];
    for (i, c) in s.iter_mut().enumerate() {
        let d = (v >> (28 - 4 * i)) & 0xf;
        *c = if d < 10 { b'0' + d as u8 } else { b'a' + d as u8 - 10 };
    }
    s
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

struct Checks(u32);

impl Checks {
    fn ok(&mut self, cond: bool, got: u64) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        out(b"winsock_test: check ");
        out(&hex(self.0));
        out(b" got ");
        out(&hex(got as u32));
        out(b"\r\n");
        Err(self.0)
    }

    fn eq(&mut self, got: u64, want: u64) -> Result<(), u32> {
        self.ok(got == want, got)
    }
}

fn pattern(i: usize) -> u8 {
    (i * 13 + i / 256) as u8
}

/// Receives until `buf` is full or the peer closes; returns the count.
fn recv_all(s: Socket, buf: &mut [u8]) -> usize {
    let mut got = 0;
    while got < buf.len() {
        let chunk = (buf.len() - got).min(i32::MAX as usize) as i32;
        // SAFETY: the rest of `buf` is writable.
        let n = unsafe { recv(s, buf[got..].as_mut_ptr(), chunk, 0) };
        if n <= 0 {
            break;
        }
        got += n as usize;
    }
    got
}

fn loopback(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: documented Winsock calls with valid arguments.
    unsafe {
        let mut data: WsaData = core::mem::zeroed();
        c.eq(WSAStartup(0x0202, &mut data) as u64, 0)?;
        c.eq(data.version.into(), 0x0202)?;

        // Name resolution of this machine's own host name and of
        // "localhost": IPv4 addresses (ADDRINFOA: family at 4, addr at 32).
        let mut host = [0u8; 256];
        c.eq(gethostname(host.as_mut_ptr(), 256) as u64, 0)?;
        let hints = [0i32, AF_INET, SOCK_STREAM, IPPROTO_TCP, 0, 0, 0, 0, 0, 0, 0, 0];
        for node in [host.as_ptr(), c"localhost".as_ptr().cast()] {
            let mut ai = core::ptr::null_mut();
            c.eq(getaddrinfo(node, c"80".as_ptr().cast(), hints.as_ptr(), &mut ai) as u64, 0)?;
            c.eq((*ai.add(4).cast::<i32>()) as u64, AF_INET as u64)?;
            let sa = *ai.add(32).cast::<*const u8>();
            c.eq(u16::from_be_bytes([*sa.add(2), *sa.add(3)]).into(), 80)?;
            freeaddrinfo(ai);
        }

        // A listener on an ephemeral loopback port.
        let listener = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
        c.ok(listener != INVALID_SOCKET, WSAGetLastError() as u64)?;
        let any_port = SockaddrIn::new(LOOPBACK, 0);
        c.eq(bind(listener, &any_port, ADDR_LEN) as u64, 0)?;
        let mut bound = SockaddrIn::new([0; 4], 0);
        let mut len = ADDR_LEN;
        c.eq(getsockname(listener, &mut bound, &mut len) as u64, 0)?;
        c.eq(len as u64, ADDR_LEN as u64)?;
        c.eq(bound.family.into(), AF_INET as u64)?;
        c.ok(bound.addr == LOOPBACK, u32::from_le_bytes(bound.addr).into())?;
        let port = u16::from_be(bound.port);
        c.ok(port != 0, 0)?;
        c.eq(listen(listener, 5) as u64, 0)?;

        // Connect, accept; both ends agree on the client's address.
        let client = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
        c.ok(client != INVALID_SOCKET, WSAGetLastError() as u64)?;
        let target = SockaddrIn::new(LOOPBACK, port);
        c.eq(connect(client, &target, ADDR_LEN) as u64, 0)?;
        let mut client_name = SockaddrIn::new([0; 4], 0);
        len = ADDR_LEN;
        c.eq(getsockname(client, &mut client_name, &mut len) as u64, 0)?;
        c.ok(client_name.port != 0 && client_name.addr == LOOPBACK, client_name.port.into())?;
        let mut peer = SockaddrIn::new([0; 4], 0);
        len = ADDR_LEN;
        let server = accept(listener, &mut peer, &mut len);
        c.ok(server != INVALID_SOCKET, WSAGetLastError() as u64)?;
        c.eq(len as u64, ADDR_LEN as u64)?;
        c.ok(peer.addr == LOOPBACK && peer.port == client_name.port, peer.port.into())?;

        // Round trips.
        c.eq(send(client, b"ping".as_ptr(), 4, 0) as u64, 4)?;
        let mut buf = [0u8; 64];
        // MSG_PEEK returns the bytes and leaves them for the next recv.
        c.eq(recv(server, buf.as_mut_ptr(), 64, MSG_PEEK) as u64, 4)?;
        c.ok(&buf[..4] == b"ping", buf[0].into())?;
        buf[..4].copy_from_slice(b"xxxx");
        c.eq(recv(server, buf.as_mut_ptr(), 64, 0) as u64, 4)?;
        c.ok(&buf[..4] == b"ping", buf[0].into())?;
        c.eq(send(server, b"pong!".as_ptr(), 5, 0) as u64, 5)?;
        c.eq(recv(client, buf.as_mut_ptr(), 64, 0) as u64, 5)?;
        c.ok(&buf[..5] == b"pong!", buf[0].into())?;

        // A bigger transfer arrives whole and in order.
        let mut big = [0u8; 16384];
        for (i, b) in big.iter_mut().enumerate() {
            *b = pattern(i);
        }
        c.eq(send(client, big.as_ptr(), big.len() as i32, 0) as u64, big.len() as u64)?;
        let mut back = [0u8; 16384];
        c.eq(recv_all(server, &mut back) as u64, back.len() as u64)?;
        let first_bad = back.iter().enumerate().position(|(i, &b)| b != pattern(i));
        c.ok(first_bad.is_none(), first_bad.unwrap_or(0) as u64)?;

        // Graceful close: the other end reads end-of-stream.
        c.eq(closesocket(server) as u64, 0)?;
        c.eq(recv(client, buf.as_mut_ptr(), 64, 0) as u64, 0)?;
        c.eq(closesocket(client) as u64, 0)?;
        c.eq(closesocket(listener) as u64, 0)?;

        // Non-blocking mode: accept and recv do not wait, and a socket
        // accepted from a non-blocking listener is non-blocking too.
        let listener2 = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
        c.eq(bind(listener2, &any_port, ADDR_LEN) as u64, 0)?;
        c.eq(listen(listener2, 5) as u64, 0)?;
        let mut on = 1u32;
        c.eq(ioctlsocket(listener2, FIONBIO, &mut on) as u64, 0)?;
        len = ADDR_LEN;
        c.eq(getsockname(listener2, &mut bound, &mut len) as u64, 0)?;
        c.eq(
            accept(listener2, core::ptr::null_mut(), core::ptr::null_mut()) as u64,
            INVALID_SOCKET as u64,
        )?;
        c.eq(WSAGetLastError() as u64, WSAEWOULDBLOCK as u64)?;
        let client2 = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
        c.eq(
            connect(client2, &SockaddrIn::new(LOOPBACK, u16::from_be(bound.port)), ADDR_LEN) as u64,
            0,
        )?;
        let mut accepted2 = INVALID_SOCKET;
        for _ in 0..200 {
            accepted2 = accept(listener2, core::ptr::null_mut(), core::ptr::null_mut());
            if accepted2 != INVALID_SOCKET {
                break;
            }
            Sleep(10);
        }
        c.ok(accepted2 != INVALID_SOCKET, WSAGetLastError() as u64)?;
        c.eq(recv(accepted2, buf.as_mut_ptr(), 64, 0) as u64, SOCKET_ERROR as u64)?;
        c.eq(WSAGetLastError() as u64, WSAEWOULDBLOCK as u64)?;
        for s in [accepted2, client2, listener2] {
            c.eq(closesocket(s) as u64, 0)?;
        }

        // Nobody listens on the port any more.
        let refused = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
        c.ok(refused != INVALID_SOCKET, WSAGetLastError() as u64)?;
        c.eq(connect(refused, &target, ADDR_LEN) as u64, SOCKET_ERROR as u64)?;
        c.eq(WSAGetLastError() as u64, WSAECONNREFUSED as u64)?;
        c.eq(closesocket(refused) as u64, 0)?;

        // A closed socket is no socket.
        c.eq(send(refused, b"x".as_ptr(), 1, 0) as u64, SOCKET_ERROR as u64)?;
        c.eq(WSAGetLastError() as u64, WSAENOTSOCK as u64)?;
    }
    Ok(())
}

/// `serve`: echo one connection on port 7 for the host.
fn serve(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: documented Winsock calls with valid arguments.
    unsafe {
        let listener = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
        c.ok(listener != INVALID_SOCKET, WSAGetLastError() as u64)?;
        c.eq(bind(listener, &SockaddrIn::new([0; 4], 7), ADDR_LEN) as u64, 0)?;
        c.eq(listen(listener, 1) as u64, 0)?;
        out(b"winsock_test: listening on port 7\r\n");
        let conn = accept(listener, core::ptr::null_mut(), core::ptr::null_mut());
        c.ok(conn != INVALID_SOCKET, WSAGetLastError() as u64)?;
        let mut buf = [0u8; 4096];
        let mut total = 0u64;
        loop {
            let n = recv(conn, buf.as_mut_ptr(), buf.len() as i32, 0);
            c.ok(n >= 0, WSAGetLastError() as u64)?;
            if n == 0 {
                break;
            }
            c.eq(send(conn, buf.as_ptr(), n, 0) as u64, n as u64)?;
            total += n as u64;
        }
        c.eq(closesocket(conn) as u64, 0)?;
        c.eq(closesocket(listener) as u64, 0)?;
        let mut digits = [0u8; 20];
        out(b"winsock_test: echoed ");
        out(decimal(total, &mut digits));
        out(b" bytes\r\n");
    }
    Ok(())
}

/// Whether the command line contains the word `serve`.
fn wants_serve() -> bool {
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
    let word: [u16; 5] = b"serve".map(u16::from);
    line.windows(5).any(|w| w == word)
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    loopback(&mut c)?;
    if wants_serve() {
        serve(&mut c)?;
    }
    // SAFETY: plain Winsock call.
    c.eq(unsafe { WSACleanup() } as u64, 0)?;
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = match run() {
        Ok(()) => {
            out(b"winsock_test: ok\r\n");
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

//! IPv6 sockets over the loopback address ::1, with the results real
//! Windows gives (ROADMAP N3 step 1): socket(AF_INET6) for TCP and UDP;
//! IPV6_V6ONLY on by default; bind to ::1 port 0 and getsockname (family
//! 23, an ephemeral port, ::1); listen, connect, accept (the peer ::1),
//! getpeername; a send/recv round trip both ways; a datagram to itself
//! with recvfrom's sender; getaddrinfo("localhost") with AF_INET6 hints
//! (::1); a dual-stack socket (IPV6_V6ONLY 0) bound to ::ffff:127.0.0.1
//! taking an IPv4 client (seen IPv4-mapped); a v6-only listener on [::]
//! refusing IPv4 clients, 0.0.0.0 of the same port bindable beside it;
//! getaddrinfo("localhost") with AF_UNSPEC (::1, then 127.0.0.1);
//! inet_pton/inet_ntop of "::1"
//! and "fe80::1:2". Prints
//! `ipv6_test: ok` or exits with the number of the failing check.

#![no_std]
#![no_main]

use core::ffi::c_void;
use core::ptr::null;

type Bool = i32;
type Handle = *mut c_void;
type Socket = usize;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut c_void) -> Bool;
    fn ExitProcess(code: u32) -> !;
}

#[link(name = "ws2_32", kind = "raw-dylib")]
unsafe extern "system" {
    fn WSAStartup(version: u16, data: *mut u8) -> i32;
    fn socket(af: i32, kind: i32, protocol: i32) -> Socket;
    fn bind(s: Socket, addr: *const Sockaddr6, len: i32) -> i32;
    fn listen(s: Socket, backlog: i32) -> i32;
    fn accept(s: Socket, addr: *mut Sockaddr6, len: *mut i32) -> Socket;
    fn connect(s: Socket, addr: *const Sockaddr6, len: i32) -> i32;
    fn send(s: Socket, buf: *const u8, len: i32, flags: i32) -> i32;
    fn recv(s: Socket, buf: *mut u8, len: i32, flags: i32) -> i32;
    fn sendto(
        s: Socket,
        buf: *const u8,
        len: i32,
        flags: i32,
        to: *const Sockaddr6,
        tolen: i32,
    ) -> i32;
    fn recvfrom(
        s: Socket,
        buf: *mut u8,
        len: i32,
        flags: i32,
        from: *mut Sockaddr6,
        fromlen: *mut i32,
    ) -> i32;
    fn getsockname(s: Socket, addr: *mut Sockaddr6, len: *mut i32) -> i32;
    fn getpeername(s: Socket, addr: *mut Sockaddr6, len: *mut i32) -> i32;
    fn getsockopt(s: Socket, level: i32, name: i32, value: *mut u8, len: *mut i32) -> i32;
    fn setsockopt(s: Socket, level: i32, name: i32, value: *const u8, len: i32) -> i32;
    #[link_name = "connect"]
    fn connect4(s: Socket, addr: *const Sockaddr4, len: i32) -> i32;
    #[link_name = "bind"]
    fn bind4(s: Socket, addr: *const Sockaddr4, len: i32) -> i32;
    fn WSAGetLastError() -> i32;
    fn closesocket(s: Socket) -> i32;
    fn getaddrinfo(
        node: *const u8,
        service: *const u8,
        hints: *const i32,
        out: *mut *mut AddrInfo,
    ) -> i32;
    fn freeaddrinfo(ai: *mut AddrInfo);
    fn inet_pton(family: i32, text: *const u8, out: *mut u8) -> i32;
    fn inet_ntop(family: i32, addr: *const u8, out: *mut u8, size: usize) -> *const u8;
}

#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
    fn __chkstk();
}

#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

#[unsafe(no_mangle)]
pub extern "C" fn __CxxFrameHandler3() -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdeac) }
}

/// SOCKADDR_IN6.
#[repr(C)]
#[derive(Clone, Copy)]
struct Sockaddr6 {
    family: u16,
    port: u16,
    flowinfo: u32,
    addr: [u8; 16],
    scope_id: u32,
}

impl Sockaddr6 {
    fn new(addr: [u8; 16], port: u16) -> Self {
        Self { family: AF_INET6 as u16, port: port.to_be(), flowinfo: 0, addr, scope_id: 0 }
    }
    fn empty() -> Self {
        Self::new([0; 16], 0)
    }
}

/// SOCKADDR_IN.
#[repr(C)]
struct Sockaddr4 {
    family: u16,
    port: u16,
    addr: [u8; 4],
    zero: [u8; 8],
}

/// ADDRINFOA (x64).
#[repr(C)]
struct AddrInfo {
    flags: i32,
    family: i32,
    socktype: i32,
    protocol: i32,
    addrlen: usize,
    canonname: *mut u8,
    addr: *const Sockaddr6,
    next: *mut AddrInfo,
}

const AF_INET6: i32 = 23;
const SOCK_STREAM: i32 = 1;
const SOCK_DGRAM: i32 = 2;
const IPPROTO_IPV6: i32 = 41;
const IPV6_V6ONLY: i32 = 27;
const LEN: i32 = 28;
const WSAECONNREFUSED: i32 = 10061;
const LOOPBACK: [u8; 16] = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1];
/// ::ffff:127.0.0.1 (IPv4-mapped).
const MAPPED_LOOPBACK: [u8; 16] = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff, 127, 0, 0, 1];

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
        );
    }
}

/// Numbered checks: the first that fails ends the process with its number.
struct Checks(u32);

impl Checks {
    #[track_caller]
    fn ok(&mut self, cond: bool) {
        self.0 += 1;
        if !cond {
            out(b"ipv6_test: check ");
            let n = self.0;
            out(&[b'0' + (n / 10 % 10) as u8, b'0' + (n % 10) as u8]);
            let (at, at_len) = line_text(core::panic::Location::caller().line());
            out(&at[..at_len]);
            out(b"\n");
            // SAFETY: ends the process.
            unsafe { ExitProcess(n) }
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let mut c = Checks(0);
    let mut wsa = [0u8; 512];
    // SAFETY: plain Winsock calls with valid buffers.
    unsafe {
        c.ok(WSAStartup(0x0202, wsa.as_mut_ptr()) == 0);
        // TCP over ::1.
        let l = socket(AF_INET6, SOCK_STREAM, 6);
        c.ok(l != !0);
        let (mut v6only, mut vlen) = (-1i32, 4i32);
        c.ok(getsockopt(l, IPPROTO_IPV6, IPV6_V6ONLY, (&raw mut v6only).cast(), &mut vlen) == 0);
        c.ok(v6only == 1);
        c.ok(bind(l, &Sockaddr6::new(LOOPBACK, 0), LEN) == 0);
        let mut name = Sockaddr6::empty();
        let mut nlen = LEN;
        c.ok(getsockname(l, &mut name, &mut nlen) == 0);
        c.ok(nlen == LEN && name.family == AF_INET6 as u16 && name.port != 0);
        c.ok(name.addr == LOOPBACK);
        c.ok(listen(l, 4) == 0);
        let client = socket(AF_INET6, SOCK_STREAM, 6);
        c.ok(client != !0);
        c.ok(connect(client, &name, LEN) == 0);
        let mut peer = Sockaddr6::empty();
        let mut plen = LEN;
        let server = accept(l, &mut peer, &mut plen);
        c.ok(server != !0);
        c.ok(plen == LEN && peer.family == AF_INET6 as u16 && peer.addr == LOOPBACK);
        let mut theirs = Sockaddr6::empty();
        let mut tlen = LEN;
        c.ok(getpeername(client, &mut theirs, &mut tlen) == 0);
        c.ok(theirs.port == name.port && theirs.addr == LOOPBACK);
        c.ok(send(client, b"ping".as_ptr(), 4, 0) == 4);
        let mut buf = [0u8; 16];
        c.ok(recv(server, buf.as_mut_ptr(), 16, 0) == 4 && &buf[..4] == b"ping");
        c.ok(send(server, b"pong!".as_ptr(), 5, 0) == 5);
        c.ok(recv(client, buf.as_mut_ptr(), 16, 0) == 5 && &buf[..5] == b"pong!");
        closesocket(client);
        closesocket(server);
        closesocket(l);
        // Dual stack (IPV6_V6ONLY 0, as Java's sockets): a v6 socket bound to
        // ::ffff:127.0.0.1 takes an IPv4 client, seen as ::ffff:127.0.0.1.
        let d = socket(AF_INET6, SOCK_STREAM, 6);
        c.ok(d != !0);
        let zero = 0i32;
        c.ok(setsockopt(d, IPPROTO_IPV6, IPV6_V6ONLY, (&raw const zero).cast(), 4) == 0);
        c.ok(bind(d, &Sockaddr6::new(MAPPED_LOOPBACK, 0), LEN) == 0);
        let mut dname = Sockaddr6::empty();
        let mut dlen = LEN;
        c.ok(getsockname(d, &mut dname, &mut dlen) == 0 && dname.addr == MAPPED_LOOPBACK);
        c.ok(listen(d, 4) == 0);
        let v4 = socket(2, SOCK_STREAM, 6);
        let to = Sockaddr4 { family: 2, port: dname.port, addr: [127, 0, 0, 1], zero: [0; 8] };
        c.ok(connect4(v4, &to, 16) == 0);
        let mut dpeer = Sockaddr6::empty();
        let mut dplen = LEN;
        let ds = accept(d, &mut dpeer, &mut dplen);
        c.ok(ds != !0 && dplen == LEN && dpeer.addr == MAPPED_LOOPBACK);
        closesocket(ds);
        closesocket(v4);
        closesocket(d);
        // IPV6_V6ONLY (on by default): a v6-only listener on [::]:p takes
        // no IPv4 client, and 0.0.0.0:p can be bound beside it (each
        // family has its own ports).
        let only = socket(AF_INET6, SOCK_STREAM, 6);
        c.ok(bind(only, &Sockaddr6::new([0; 16], 0), LEN) == 0);
        let mut oname = Sockaddr6::empty();
        let mut olen = LEN;
        c.ok(getsockname(only, &mut oname, &mut olen) == 0);
        c.ok(listen(only, 4) == 0);
        let v4b = socket(2, SOCK_STREAM, 6);
        let any4 = Sockaddr4 { family: 2, port: oname.port, addr: [0; 4], zero: [0; 8] };
        c.ok(bind4(v4b, &any4, 16) == 0);
        closesocket(v4b);
        let v4c = socket(2, SOCK_STREAM, 6);
        let to4 = Sockaddr4 { family: 2, port: oname.port, addr: [127, 0, 0, 1], zero: [0; 8] };
        c.ok(connect4(v4c, &to4, 16) == -1 && WSAGetLastError() == WSAECONNREFUSED);
        closesocket(v4c);
        closesocket(only);
        // UDP over ::1: a datagram to itself.
        let u = socket(AF_INET6, SOCK_DGRAM, 17);
        c.ok(u != !0);
        c.ok(bind(u, &Sockaddr6::new(LOOPBACK, 0), LEN) == 0);
        let mut uname = Sockaddr6::empty();
        let mut ulen = LEN;
        c.ok(getsockname(u, &mut uname, &mut ulen) == 0 && uname.port != 0);
        c.ok(sendto(u, b"datagram".as_ptr(), 8, 0, &uname, LEN) == 8);
        let mut from = Sockaddr6::empty();
        let mut flen = LEN;
        let n = recvfrom(u, buf.as_mut_ptr(), 16, 0, &mut from, &mut flen);
        c.ok(n == 8 && &buf[..8] == b"datagram");
        c.ok(flen == LEN && from.addr == LOOPBACK && from.port == uname.port);
        closesocket(u);
        // Names and text.
        let hints = [0i32, AF_INET6, SOCK_STREAM, 6, 0, 0, 0, 0, 0, 0, 0, 0];
        let mut list: *mut AddrInfo = core::ptr::null_mut();
        c.ok(getaddrinfo(b"localhost\0".as_ptr(), null(), hints.as_ptr(), &mut list) == 0);
        c.ok(!list.is_null() && (*list).family == AF_INET6 && (*list).addrlen == LEN as usize);
        c.ok((*(*list).addr).addr == LOOPBACK);
        freeaddrinfo(list);
        // AF_UNSPEC: ::1 first, then 127.0.0.1 (as Windows lists them).
        let unspec = [0i32, 0, SOCK_STREAM, 6, 0, 0, 0, 0, 0, 0, 0, 0];
        let mut both: *mut AddrInfo = core::ptr::null_mut();
        c.ok(getaddrinfo(b"localhost\0".as_ptr(), null(), unspec.as_ptr(), &mut both) == 0);
        c.ok(!both.is_null() && (*both).family == AF_INET6);
        let second = (*both).next;
        c.ok(!second.is_null() && (*second).family == 2 && (*second).addrlen == 16);
        c.ok((*(*second).addr.cast::<Sockaddr4>()).addr == [127, 0, 0, 1]);
        freeaddrinfo(both);
        let mut bin = [0u8; 16];
        c.ok(inet_pton(AF_INET6, b"::1\0".as_ptr(), bin.as_mut_ptr()) == 1 && bin == LOOPBACK);
        c.ok(inet_pton(AF_INET6, b"fe80::1:2\0".as_ptr(), bin.as_mut_ptr()) == 1);
        c.ok(bin == [0xfe, 0x80, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 2]);
        let mut text = [0u8; 64];
        c.ok(!inet_ntop(AF_INET6, bin.as_ptr(), text.as_mut_ptr(), 64).is_null());
        c.ok(text.starts_with(b"fe80::1:2\0"));
        c.ok(inet_pton(AF_INET6, b"not an address\0".as_ptr(), bin.as_mut_ptr()) == 0);
    }
    out(b"ipv6_test: ok\n");
    // SAFETY: ends the process.
    unsafe { ExitProcess(0) }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdead) }
}

/// ` (line L)` for the failing check's source line (`#[track_caller]` on
/// Checks' methods), as bytes and their length.
fn line_text(line: u32) -> ([u8; 18], usize) {
    let mut b = *b" (line 0000000000)";
    let mut digits = [0u8; 10];
    let (mut n, mut len) = (line, 0);
    loop {
        digits[len] = b'0' + (n % 10) as u8;
        len += 1;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    for i in 0..len {
        b[7 + i] = digits[len - 1 - i];
    }
    b[7 + len] = b')';
    (b, 8 + len)
}

//! Winsock UDP as programs use it, with the results real Windows gives
//! (Microsoft's documentation of each function): socket(AF_INET,
//! SOCK_DGRAM, IPPROTO_UDP) and WSASocketW; SO_TYPE (SOCK_DGRAM);
//! getsockname before bind (WSAEINVAL), recvfrom before bind (WSAEINVAL);
//! bind to port 0 (an ephemeral port, 49152-65535, reported by
//! getsockname), bind to a port in use (WSAEADDRINUSE); getpeername of an
//! unconnected socket (WSAENOTCONN); sendto/recvfrom over loopback between
//! two sockets (each datagram whole and on its own, the sender's address
//! reported, MSG_PEEK leaving it queued, a 4000-byte datagram);
//! a datagram larger than the buffer (WSAEMSGSIZE, the buffer filled, the
//! rest discarded); select and WSAPoll readiness (readable once a
//! datagram is queued, always writable); non-blocking recvfrom
//! (WSAEWOULDBLOCK); sendto on an unbound socket (bound implicitly to an
//! ephemeral port); connect() on UDP (a default peer: send/recv,
//! getpeername, datagrams from other addresses discarded) and connect to a
//! zeroed address (AF_UNSPEC, as libuv disconnects: the association
//! dissolved, WSAENOTCONN again);
//! SO_RCVBUF and SO_BROADCAST (set and read back); overlapped
//! WSARecvFrom/WSASendTo with an I/O completion port (key, OVERLAPPED,
//! byte count, the sender's address written at completion); closing a
//! socket with a receive pending (WSA_OPERATION_ABORTED, 995, through the
//! port). Prints `udp_test: ok` or exits with the number of the failing
//! check.
//!
//! With the argument `serve` it then binds port 7 of every address,
//! prints `udp_test: listening on port 7` and echoes datagrams to their
//! sender until one says `bye` (answered `bye`); a datagram `dns <name>`
//! is answered `dns <a.b.c.d>` with the first IPv4 address getaddrinfo
//! gives for the name, or `dns error <code>`. Then it prints
//! `udp_test: echoed <n> datagram(s)` (the `udp_echo` boot scenario plays
//! the other side from the host, through QEMU's network).

#![no_std]
#![no_main]

use core::ffi::c_void;
use core::ptr::{null, null_mut};

type Bool = i32;
type Handle = *mut c_void;
type Socket = usize;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut c_void) -> Bool;
    fn GetCommandLineW() -> *const u16;
    fn GetLastError() -> u32;
    fn CreateIoCompletionPort(file: Handle, port: Handle, key: usize, threads: u32) -> Handle;
    fn GetQueuedCompletionStatus(
        port: Handle,
        bytes: *mut u32,
        key: *mut usize,
        ov: *mut *mut Overlapped,
        ms: u32,
    ) -> Bool;
    fn CloseHandle(h: Handle) -> Bool;
    fn ExitProcess(code: u32) -> !;
}

#[link(name = "ws2_32", kind = "raw-dylib")]
unsafe extern "system" {
    fn WSAStartup(version: u16, data: *mut u8) -> i32;
    fn WSACleanup() -> i32;
    fn WSAGetLastError() -> i32;
    fn socket(af: i32, kind: i32, protocol: i32) -> Socket;
    fn WSASocketW(
        af: i32,
        kind: i32,
        protocol: i32,
        info: *mut c_void,
        g: u32,
        flags: u32,
    ) -> Socket;
    fn bind(s: Socket, addr: *const SockaddrIn, len: i32) -> i32;
    fn connect(s: Socket, addr: *const SockaddrIn, len: i32) -> i32;
    fn send(s: Socket, buf: *const u8, len: i32, flags: i32) -> i32;
    fn recv(s: Socket, buf: *mut u8, len: i32, flags: i32) -> i32;
    fn sendto(
        s: Socket,
        buf: *const u8,
        len: i32,
        flags: i32,
        to: *const SockaddrIn,
        to_len: i32,
    ) -> i32;
    fn recvfrom(
        s: Socket,
        buf: *mut u8,
        len: i32,
        flags: i32,
        from: *mut SockaddrIn,
        from_len: *mut i32,
    ) -> i32;
    fn WSARecvFrom(
        s: Socket,
        bufs: *mut WsaBuf,
        count: u32,
        received: *mut u32,
        flags: *mut u32,
        from: *mut SockaddrIn,
        from_len: *mut i32,
        ov: *mut Overlapped,
        routine: usize,
    ) -> i32;
    fn WSASendTo(
        s: Socket,
        bufs: *mut WsaBuf,
        count: u32,
        sent: *mut u32,
        flags: u32,
        to: *const SockaddrIn,
        to_len: i32,
        ov: *mut Overlapped,
        routine: usize,
    ) -> i32;
    fn getsockname(s: Socket, addr: *mut SockaddrIn, len: *mut i32) -> i32;
    fn getpeername(s: Socket, addr: *mut SockaddrIn, len: *mut i32) -> i32;
    fn getsockopt(s: Socket, level: i32, name: i32, value: *mut u8, len: *mut i32) -> i32;
    fn setsockopt(s: Socket, level: i32, name: i32, value: *const u8, len: i32) -> i32;
    fn ioctlsocket(s: Socket, cmd: i32, arg: *mut u32) -> i32;
    fn select(
        n: i32,
        read: *mut FdSet,
        write: *mut FdSet,
        except: *mut FdSet,
        timeout: *const Timeval,
    ) -> i32;
    fn WSAPoll(fds: *mut PollFd, count: u32, timeout: i32) -> i32;
    fn getaddrinfo(
        node: *const u8,
        service: *const u8,
        hints: *const i32,
        result: *mut *mut u8,
    ) -> i32;
    fn freeaddrinfo(ai: *mut u8);
    fn closesocket(s: Socket) -> i32;
}

#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
    // The stack probe for the large buffers (ntdll exports it).
    fn __chkstk();
}

#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

#[unsafe(no_mangle)]
pub extern "C" fn __CxxFrameHandler3() -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdeac) }
}

#[repr(C)]
struct Overlapped {
    internal: usize,
    internal_high: usize,
    offset: u64,
    event: Handle,
}

impl Overlapped {
    const fn new() -> Self {
        Self { internal: 0, internal_high: 0, offset: 0, event: null_mut() }
    }
}

#[repr(C)]
struct WsaBuf {
    len: u32,
    buf: *mut u8,
}

/// SOCKADDR_IN: family, port and address in network byte order.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq)]
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

    const fn empty() -> Self {
        Self { family: 0, port: 0, addr: [0; 4], zero: [0; 8] }
    }

    fn host_port(&self) -> u16 {
        u16::from_be(self.port)
    }
}

/// fd_set (x64): the count, then the sockets at offset 8.
#[repr(C)]
struct FdSet {
    count: u32,
    sockets: [Socket; 8],
}

impl FdSet {
    fn of(s: Socket) -> Self {
        let mut set = Self { count: 1, sockets: [0; 8] };
        set.sockets[0] = s;
        set
    }
}

#[repr(C)]
struct Timeval {
    sec: i32,
    usec: i32,
}

/// WSAPOLLFD.
#[repr(C)]
struct PollFd {
    fd: Socket,
    events: i16,
    revents: i16,
}

const AF_INET: i32 = 2;
const SOCK_DGRAM: i32 = 2;
const IPPROTO_UDP: i32 = 17;
const SOL_SOCKET: i32 = 0xffff;
const SO_TYPE: i32 = 0x1008;
const SO_RCVBUF: i32 = 0x1002;
const SO_BROADCAST: i32 = 0x20;
const SO_RANDOMIZE_PORT: i32 = 0x3005;
const IPPROTO_IP: i32 = 0;
const IP_DONTFRAGMENT: i32 = 14;
const IP_RECVECN: i32 = 50;
const INVALID_SOCKET: Socket = !0;
const SOCKET_ERROR: i32 = -1;
const MSG_PEEK: i32 = 2;
const FIONBIO: i32 = 0x8004_667eu32 as i32;
const WSA_FLAG_OVERLAPPED: u32 = 1;
const WSAEINVAL: i32 = 10022;
const WSAEWOULDBLOCK: i32 = 10035;
const WSAEMSGSIZE: i32 = 10040;
const WSAEADDRINUSE: i32 = 10048;
const WSAENOTCONN: i32 = 10057;
const WSA_IO_PENDING: i32 = 997;
const ERROR_OPERATION_ABORTED: u32 = 995;
const POLLRDNORM: i16 = 0x100;
const POLLWRNORM: i16 = 0x10;
const LOOPBACK: [u8; 4] = [127, 0, 0, 1];
const ADDR_LEN: i32 = size_of::<SockaddrIn>() as i32;
const KEY: usize = 0x75;

fn out(text: &[u8]) {
    let mut written = 0;
    // SAFETY: valid buffer and output pointer.
    unsafe {
        WriteFile(
            GetStdHandle(-11i32 as u32),
            text.as_ptr(),
            text.len() as u32,
            &mut written,
            null_mut(),
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

/// The decimal digits of `v` at the end of `buf`.
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
        out(b"udp_test: check ");
        out(&hex(self.0));
        out(b" got ");
        out(&hex(got as u32));
        out(b"\r\n");
        Err(self.0)
    }

    fn eq(&mut self, got: u64, want: u64) -> Result<(), u32> {
        self.ok(got == want, got)
    }

    /// A call failed with SOCKET_ERROR and this WSA error.
    fn error(&mut self, result: i32, want: i32) -> Result<(), u32> {
        self.eq(result as u64, SOCKET_ERROR as u64)?;
        // SAFETY: plain Winsock call.
        self.eq(unsafe { WSAGetLastError() } as u64, want as u64)
    }
}

fn udp() -> Socket {
    // SAFETY: plain Winsock call.
    unsafe { socket(AF_INET, SOCK_DGRAM, IPPROTO_UDP) }
}

fn name_of(s: Socket) -> SockaddrIn {
    let mut sa = SockaddrIn::empty();
    let mut len = ADDR_LEN;
    // SAFETY: valid output buffer and length.
    unsafe { getsockname(s, &mut sa, &mut len) };
    sa
}

/// Whether `s` is readable within `ms` (select).
fn readable(s: Socket, ms: i32) -> i32 {
    let mut set = FdSet::of(s);
    let t = Timeval { sec: ms / 1000, usec: ms % 1000 * 1000 };
    // SAFETY: valid fd_set and timeval.
    unsafe { select(0, &mut set, null_mut(), null_mut(), &t) }
}

fn get_int(s: Socket, name: i32) -> (i32, i32, i32) {
    let mut v = -1i32;
    let mut len = 4i32;
    // SAFETY: a 4-byte value buffer.
    let r = unsafe { getsockopt(s, SOL_SOCKET, name, (&raw mut v).cast(), &mut len) };
    (r, v, len)
}

/// Sends `data` from `from` to `to`; the byte count.
fn send_to(from: Socket, data: &[u8], to: &SockaddrIn) -> i32 {
    // SAFETY: valid buffers.
    unsafe { sendto(from, data.as_ptr(), data.len() as i32, 0, to, ADDR_LEN) }
}

/// One datagram into `buf` (`flags`); (result, sender).
fn recv_from(s: Socket, buf: &mut [u8], flags: i32) -> (i32, SockaddrIn) {
    let mut from = SockaddrIn::empty();
    let mut len = ADDR_LEN;
    // SAFETY: valid buffers.
    let r = unsafe { recvfrom(s, buf.as_mut_ptr(), buf.len() as i32, flags, &mut from, &mut len) };
    (r, from)
}

fn loopback(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: documented Winsock calls with valid arguments.
    unsafe {
        let mut data = [0u8; 408];
        c.eq(WSAStartup(0x0202, data.as_mut_ptr()) as u64, 0)?;
        let (a, b) = (udp(), udp());
        c.ok(a != INVALID_SOCKET && b != INVALID_SOCKET, WSAGetLastError() as u64)?;
        let (r, kind, len) = get_int(a, SO_TYPE);
        c.eq(r as u64, 0)?;
        c.eq(kind as u64, SOCK_DGRAM as u64)?;
        c.eq(len as u64, 4)?;

        // Not bound yet.
        let mut sa = SockaddrIn::empty();
        let mut len = ADDR_LEN;
        c.error(getsockname(a, &mut sa, &mut len), WSAEINVAL)?;
        let mut buf = [0u8; 64];
        c.error(recv_from(a, &mut buf, 0).0, WSAEINVAL)?;

        // Port 0: an ephemeral port (Windows' dynamic range).
        c.eq(bind(a, &SockaddrIn::new(LOOPBACK, 0), ADDR_LEN) as u64, 0)?;
        c.eq(bind(b, &SockaddrIn::new(LOOPBACK, 0), ADDR_LEN) as u64, 0)?;
        let (na, nb) = (name_of(a), name_of(b));
        c.eq(na.family.into(), AF_INET as u64)?;
        c.ok(na.addr == LOOPBACK, u32::from_le_bytes(na.addr).into())?;
        c.ok(na.host_port() >= 49152, na.host_port().into())?;
        c.ok(nb.host_port() >= 49152 && nb.port != na.port, nb.host_port().into())?;
        // A UDP port in use (no SO_REUSEADDR).
        let other = udp();
        c.error(bind(other, &na, ADDR_LEN), WSAEADDRINUSE)?;
        c.eq(closesocket(other) as u64, 0)?;
        // Not connected.
        len = ADDR_LEN;
        c.error(getpeername(a, &mut sa, &mut len), WSAENOTCONN)?;

        // Nothing queued yet: not readable, but writable.
        c.eq(readable(b, 0) as u64, 0)?;
        let mut fds = [PollFd { fd: b, events: POLLRDNORM | POLLWRNORM, revents: 0 }];
        c.eq(WSAPoll(fds.as_mut_ptr(), 1, 0) as u64, 1)?;
        c.eq(fds[0].revents as u64, POLLWRNORM as u64)?;

        // Datagrams keep their boundaries and their sender.
        c.eq(send_to(a, b"one", &nb) as u64, 3)?;
        c.eq(send_to(a, b"two!", &nb) as u64, 4)?;
        let big = [0x5au8; 4000];
        c.eq(send_to(a, &big, &nb) as u64, 4000)?;
        c.eq(readable(b, 5000) as u64, 1)?;
        fds[0].revents = 0;
        c.eq(WSAPoll(fds.as_mut_ptr(), 1, 1000) as u64, 1)?;
        c.eq(fds[0].revents as u64, (POLLRDNORM | POLLWRNORM) as u64)?;
        let (n, from) = recv_from(b, &mut buf, MSG_PEEK);
        c.eq(n as u64, 3)?;
        c.ok(&buf[..3] == b"one", buf[0].into())?;
        c.ok(from == na, from.host_port().into())?;
        buf[..3].fill(0);
        let (n, from) = recv_from(b, &mut buf, 0);
        c.eq(n as u64, 3)?;
        c.ok(&buf[..3] == b"one" && from == na, buf[0].into())?;
        c.eq(readable(b, 5000) as u64, 1)?;
        let (n, _) = recv_from(b, &mut buf, 0);
        c.eq(n as u64, 4)?;
        c.ok(&buf[..4] == b"two!", buf[0].into())?;
        let mut large = [0u8; 8192];
        c.eq(readable(b, 5000) as u64, 1)?;
        let (n, _) = recv_from(b, &mut large, 0);
        c.eq(n as u64, 4000)?;
        c.ok(large[..4000].iter().all(|&x| x == 0x5a), 0)?;

        // Too large for the buffer: WSAEMSGSIZE, the buffer filled with
        // its start, the rest of the datagram gone.
        c.eq(send_to(a, b"0123456789", &nb) as u64, 10)?;
        c.eq(send_to(a, b"next", &nb) as u64, 4)?;
        c.eq(readable(b, 5000) as u64, 1)?;
        let mut small = [0u8; 4];
        c.error(recv_from(b, &mut small, 0).0, WSAEMSGSIZE)?;
        c.ok(&small == b"0123", small[0].into())?;
        c.eq(readable(b, 5000) as u64, 1)?;
        let (n, _) = recv_from(b, &mut buf, 0);
        c.eq(n as u64, 4)?;
        c.ok(&buf[..4] == b"next", buf[0].into())?;

        // Non-blocking: nothing queued is WSAEWOULDBLOCK.
        let mut on = 1u32;
        c.eq(ioctlsocket(b, FIONBIO, &mut on) as u64, 0)?;
        c.error(recv_from(b, &mut buf, 0).0, WSAEWOULDBLOCK)?;

        // sendto binds an unbound socket to an ephemeral port.
        let s = udp();
        c.eq(send_to(s, b"implicit", &nb) as u64, 8)?;
        let ns = name_of(s);
        c.ok(ns.host_port() >= 49152, ns.host_port().into())?;
        c.eq(readable(b, 5000) as u64, 1)?;
        let (n, from) = recv_from(b, &mut buf, 0);
        c.eq(n as u64, 8)?;
        c.ok(from.port == ns.port && from.addr == LOOPBACK, from.host_port().into())?;

        // connect: a default peer for send and recv; others are discarded.
        c.eq(connect(a, &nb, ADDR_LEN) as u64, 0)?;
        len = ADDR_LEN;
        c.eq(getpeername(a, &mut sa, &mut len) as u64, 0)?;
        c.ok(sa == nb, sa.host_port().into())?;
        c.eq(send(a, b"conn".as_ptr(), 4, 0) as u64, 4)?;
        c.eq(readable(b, 5000) as u64, 1)?;
        let (n, from) = recv_from(b, &mut buf, 0);
        c.eq(n as u64, 4)?;
        c.ok(&buf[..4] == b"conn" && from == na, from.host_port().into())?;
        c.eq(send_to(s, b"stranger", &na) as u64, 8)?;
        c.eq(send_to(b, b"friend", &na) as u64, 6)?;
        c.eq(readable(a, 5000) as u64, 1)?;
        let n = recv(a, buf.as_mut_ptr(), 64, 0);
        c.eq(n as u64, 6)?;
        c.ok(&buf[..6] == b"friend", buf[0].into())?;
        c.eq(readable(a, 300) as u64, 0)?;
        // A zeroed address (here AF_UNSPEC, as libuv disconnects) dissolves
        // the association.
        c.eq(connect(a, &SockaddrIn::empty(), ADDR_LEN) as u64, 0)?;
        len = ADDR_LEN;
        c.error(getpeername(a, &mut sa, &mut len), WSAENOTCONN)?;
        c.eq(send_to(s, b"again", &na) as u64, 5)?;
        c.eq(readable(a, 5000) as u64, 1)?;
        let (n, from) = recv_from(a, &mut buf, 0);
        c.eq(n as u64, 5)?;
        c.ok(from.port == ns.port && from.addr == LOOPBACK, from.host_port().into())?;

        // Options kept per socket.
        let (r, v, _) = get_int(b, SO_BROADCAST);
        c.eq(r as u64, 0)?;
        c.eq(v as u64, 0)?;
        let one = 1i32;
        c.eq(setsockopt(b, SOL_SOCKET, SO_BROADCAST, (&raw const one).cast(), 4) as u64, 0)?;
        c.eq(get_int(b, SO_BROADCAST).1 as u64, 1)?;
        let size = 32768i32;
        c.eq(setsockopt(b, SOL_SOCKET, SO_RCVBUF, (&raw const size).cast(), 4) as u64, 0)?;
        c.eq(get_int(b, SO_RCVBUF).1 as u64, 32768)?;
        // SO_RANDOMIZE_PORT and IP_DONTFRAGMENT (Chromium's UDP sockets
        // set both): accepted, read back.
        c.eq(setsockopt(b, SOL_SOCKET, SO_RANDOMIZE_PORT, (&raw const one).cast(), 4) as u64, 0)?;
        c.eq(get_int(b, SO_RANDOMIZE_PORT).1 as u64, 1)?;
        c.eq(setsockopt(b, IPPROTO_IP, IP_DONTFRAGMENT, (&raw const one).cast(), 4) as u64, 0)?;
        let (mut df, mut df_len) = (-1i32, 4i32);
        c.eq(
            getsockopt(b, IPPROTO_IP, IP_DONTFRAGMENT, (&raw mut df).cast(), &mut df_len) as u64,
            0,
        )?;
        c.eq(df as u64, 1)?;
        // IP_RECVECN (Chromium's QUIC sockets): accepted, read back.
        c.eq(setsockopt(b, IPPROTO_IP, IP_RECVECN, (&raw const one).cast(), 4) as u64, 0)?;
        let (mut ecn, mut ecn_len) = (-1i32, 4i32);
        c.eq(getsockopt(b, IPPROTO_IP, IP_RECVECN, (&raw mut ecn).cast(), &mut ecn_len) as u64, 0)?;
        c.eq(ecn as u64, 1)?;

        // Overlapped, through a completion port.
        let d = WSASocketW(AF_INET, SOCK_DGRAM, IPPROTO_UDP, null_mut(), 0, WSA_FLAG_OVERLAPPED);
        c.ok(d != INVALID_SOCKET, WSAGetLastError() as u64)?;
        c.eq(bind(d, &SockaddrIn::new(LOOPBACK, 0), ADDR_LEN) as u64, 0)?;
        let nd = name_of(d);
        let port = CreateIoCompletionPort(d as Handle, null_mut(), KEY, 0);
        c.ok(!port.is_null(), GetLastError().into())?;
        let mut ov = Overlapped::new();
        let mut rbuf = [0u8; 32];
        let mut wb = WsaBuf { len: 32, buf: rbuf.as_mut_ptr() };
        let (mut got, mut flags) = (0u32, 0u32);
        let mut from = SockaddrIn::empty();
        let mut from_len = ADDR_LEN;
        let r =
            WSARecvFrom(d, &mut wb, 1, &mut got, &mut flags, &mut from, &mut from_len, &mut ov, 0);
        c.error(r, WSA_IO_PENDING)?;
        c.eq(send_to(a, b"async", &nd) as u64, 5)?;
        let (mut bytes, mut key, mut done) = (0u32, 0usize, null_mut());
        c.eq(GetQueuedCompletionStatus(port, &mut bytes, &mut key, &mut done, 5000) as u64, 1)?;
        c.eq(bytes.into(), 5)?;
        c.eq(key as u64, KEY as u64)?;
        c.ok(done == &raw mut ov, done as u64)?;
        c.ok(&rbuf[..5] == b"async", rbuf[0].into())?;
        c.ok(from == na, from.host_port().into())?;
        c.eq(from_len as u64, ADDR_LEN as u64)?;
        // A send: done at once or pending, a packet either way.
        let mut payload = *b"overlapped";
        let mut sb = WsaBuf { len: 10, buf: payload.as_mut_ptr() };
        let mut ov2 = Overlapped::new();
        let mut sent = 0u32;
        let r = WSASendTo(d, &mut sb, 1, &mut sent, 0, &nb, ADDR_LEN, &mut ov2, 0);
        c.ok(r == 0 || WSAGetLastError() == WSA_IO_PENDING, WSAGetLastError() as u64)?;
        c.eq(GetQueuedCompletionStatus(port, &mut bytes, &mut key, &mut done, 5000) as u64, 1)?;
        c.eq(bytes.into(), 10)?;
        c.ok(done == &raw mut ov2, done as u64)?;
        c.eq(readable(b, 5000) as u64, 1)?;
        let (n, mut from) = recv_from(b, &mut buf, 0);
        c.eq(n as u64, 10)?;
        c.ok(&buf[..10] == b"overlapped" && from == nd, from.host_port().into())?;
        // Closing the socket aborts the receive still pending.
        let mut ov3 = Overlapped::new();
        let r =
            WSARecvFrom(d, &mut wb, 1, &mut got, &mut flags, &mut from, &mut from_len, &mut ov3, 0);
        c.error(r, WSA_IO_PENDING)?;
        c.eq(closesocket(d) as u64, 0)?;
        let ok = GetQueuedCompletionStatus(port, &mut bytes, &mut key, &mut done, 5000);
        c.eq(ok as u64, 0)?;
        c.eq(GetLastError().into(), ERROR_OPERATION_ABORTED.into())?;
        c.ok(done == &raw mut ov3, done as u64)?;
        CloseHandle(port);

        for s in [a, b, s] {
            c.eq(closesocket(s) as u64, 0)?;
        }
    }
    Ok(())
}

/// The first IPv4 address of `name` (NUL-terminated), as text, or the
/// error.
fn resolve(name: &[u8], text: &mut [u8; 64]) -> usize {
    let hints = [0i32, AF_INET, SOCK_DGRAM, IPPROTO_UDP, 0, 0, 0, 0, 0, 0, 0, 0];
    let mut ai = null_mut();
    // SAFETY: a NUL-terminated name, valid hints and output.
    let r = unsafe { getaddrinfo(name.as_ptr(), null(), hints.as_ptr(), &mut ai) };
    let mut n = 0;
    let mut put = |s: &[u8]| {
        text[n..n + s.len()].copy_from_slice(s);
        n += s.len();
    };
    put(b"dns ");
    let mut digits = [0u8; 20];
    if r != 0 {
        put(b"error ");
        put(decimal(r as u64, &mut digits));
        return n;
    }
    // SAFETY: getaddrinfo's ADDRINFOA: the SOCKADDR_IN pointer at 32.
    let ip = unsafe {
        let sa = *ai.cast::<u8>().add(32).cast::<*const u8>();
        let ip = [*sa.add(4), *sa.add(5), *sa.add(6), *sa.add(7)];
        freeaddrinfo(ai);
        ip
    };
    for (i, b) in ip.iter().enumerate() {
        if i > 0 {
            put(b".");
        }
        put(decimal((*b).into(), &mut digits));
    }
    n
}

/// `serve`: echo datagrams on port 7 for the host until `bye`.
fn serve(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: documented Winsock calls with valid arguments.
    unsafe {
        let s = udp();
        c.ok(s != INVALID_SOCKET, WSAGetLastError() as u64)?;
        c.eq(bind(s, &SockaddrIn::new([0; 4], 7), ADDR_LEN) as u64, 0)?;
        out(b"udp_test: listening on port 7\r\n");
        let mut buf = [0u8; 2048];
        let mut echoed = 0u64;
        loop {
            let (n, from) = recv_from(s, &mut buf, 0);
            c.ok(n >= 0, WSAGetLastError() as u64)?;
            let data = &buf[..n as usize];
            if data == b"bye" {
                c.eq(send_to(s, b"bye", &from) as u64, 3)?;
                break;
            }
            if let Some(name) = data.strip_prefix(b"dns ") {
                let mut z = [0u8; 256];
                z[..name.len().min(255)].copy_from_slice(&name[..name.len().min(255)]);
                let mut text = [0u8; 64];
                let len = resolve(&z, &mut text);
                c.eq(send_to(s, &text[..len], &from) as u64, len as u64)?;
                // One line, one write.
                let mut line = [0u8; 96];
                line[..10].copy_from_slice(b"udp_test: ");
                line[10..10 + len].copy_from_slice(&text[..len]);
                line[10 + len..12 + len].copy_from_slice(b"\r\n");
                out(&line[..12 + len]);
                continue;
            }
            c.eq(send_to(s, data, &from) as u64, n as u64)?;
            echoed += 1;
        }
        c.eq(closesocket(s) as u64, 0)?;
        let mut digits = [0u8; 20];
        out(b"udp_test: echoed ");
        out(decimal(echoed, &mut digits));
        out(b" datagram(s)\r\n");
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
            out(b"udp_test: ok\r\n");
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

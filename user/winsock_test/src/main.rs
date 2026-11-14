//! Winsock TCP as programs use it, with the results real Windows gives:
//! WSAStartup (version 2.2), TCP keep-alive socket options (defaults,
//! set and read back), WSAPoll (readable, writable, hung up), WSAIoctl's SIO_BASE_HANDLE and
//! SIO_BSP_HANDLE(_SELECT/_POLL) (the socket itself), getaddrinfo (and GetAddrInfoW) of the machine's own host name
//! and of "localhost" (IPv4 addresses), socket/bind/listen/accept/connect on the
//! loopback address, getsockname (an ephemeral port after binding port 0;
//! accept reports the client's address), send/recv round trips in both
//! directions (with MSG_PEEK), a 16 KiB transfer, a graceful close (recv
//! returns 0), WSAECONNREFUSED for a port nobody listens on (a
//! non-blocking connect's in select's except set and SO_ERROR), WSAENOTSOCK
//! for a closed socket, WSADuplicateSocketW and WSASocketW from its
//! protocol information (a second socket for the connection; also
//! WSADuplicateSocketA with WSASocketA), WSAConnect
//! and WSAAccept, and non-blocking sockets (WSAEWOULDBLOCK from
//! accept and recv; an accepted socket inherits the listener's mode),
//! many listeners at once (twelve at SOMAXCONN, each accepting a
//! connection: a listener holds no fixed share of the stack's sockets),
//! the extension functions asyncio's _overlapped asks for (DisconnectEx:
//! a graceful disconnect, overlapped; TransmitFile's pointer),
//! WSAStringToAddressA/W (IPv4 and IPv6, ports, scopes, refusals),
//! getprotobyname/getprotobynumber by their ordinals (53, 54)
//! and a full backlog (listen's documented WSAECONNREFUSED once `backlog`
//! connections wait unaccepted; room again after an accept).
//! Prints `winsock_test: ok` or exits with the number of the failing check.
//!
//! With the argument `serve` it then listens on port 7 of every address,
//! prints `winsock_test: listening on port 7`, echoes one connection until
//! the peer closes it and prints `winsock_test: echoed <n> bytes` (the
//! `winsock_test` boot scenario connects from the host).

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
    fn ExitProcess(code: u32) -> !;
    fn Sleep(ms: u32);
    fn GetCurrentProcessId() -> u32;
    fn CreateEventW(attrs: *const c_void, manual: Bool, initial: Bool, name: *const u16) -> Handle;
    fn GetOverlappedResult(h: Handle, ov: *mut Overlapped, done: *mut u32, wait: Bool) -> Bool;
    fn CloseHandle(h: Handle) -> Bool;
    fn GetLastError() -> u32;
}

// The protocol database, imported by ordinal as programs do (Python's
// _socket.pyd imports getprotobyname as ws2_32 #53).
#[link(name = "ws2_32", kind = "raw-dylib")]
unsafe extern "system" {
    #[link_ordinal(53)]
    fn getprotobyname(name: *const u8) -> *const ProtoEnt;
    #[link_ordinal(54)]
    fn getprotobynumber(number: i32) -> *const ProtoEnt;
}

/// PROTOENT (winsock2.h).
#[repr(C)]
struct ProtoEnt {
    name: *const u8,
    aliases: *const *const u8,
    proto: i16,
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
    fn GetAddrInfoW(
        node: *const u16,
        service: *const u16,
        hints: *const i32,
        out: *mut *mut u8,
    ) -> i32;
    fn FreeAddrInfoW(ai: *mut u8);
    fn WSAGetLastError() -> i32;
    fn socket(af: i32, kind: i32, protocol: i32) -> Socket;
    fn bind(s: Socket, addr: *const SockaddrIn, len: i32) -> i32;
    fn listen(s: Socket, backlog: i32) -> i32;
    fn accept(s: Socket, addr: *mut SockaddrIn, len: *mut i32) -> Socket;
    fn connect(s: Socket, addr: *const SockaddrIn, len: i32) -> i32;
    fn send(s: Socket, buf: *const u8, len: i32, flags: i32) -> i32;
    fn WSAPoll(fds: *mut PollFd, count: u32, timeout: i32) -> i32;
    fn recv(s: Socket, buf: *mut u8, len: i32, flags: i32) -> i32;
    fn getsockname(s: Socket, addr: *mut SockaddrIn, len: *mut i32) -> i32;
    fn WSAIoctl(
        s: Socket,
        code: u32,
        input: *const u8,
        input_len: u32,
        output: *mut u8,
        output_len: u32,
        returned: *mut u32,
        overlapped: *mut c_void,
        routine: usize,
    ) -> i32;
    fn closesocket(s: Socket) -> i32;
    fn WSADuplicateSocketW(s: Socket, process: u32, info: *mut u8) -> i32;
    fn WSADuplicateSocketA(s: Socket, process: u32, info: *mut u8) -> i32;
    fn WSASocketA(
        af: i32,
        kind: i32,
        protocol: i32,
        info: *const u8,
        group: u32,
        flags: u32,
    ) -> Socket;
    fn WSAConnect(
        s: Socket,
        addr: *const SockaddrIn,
        len: i32,
        caller: *const u8,
        callee: *mut u8,
        sqos: *const u8,
        gqos: *const u8,
    ) -> i32;
    fn WSAAccept(
        s: Socket,
        addr: *mut SockaddrIn,
        len: *mut i32,
        condition: usize,
        data: usize,
    ) -> Socket;
    fn WSASocketW(
        af: i32,
        kind: i32,
        protocol: i32,
        info: *const u8,
        group: u32,
        flags: u32,
    ) -> Socket;
    fn setsockopt(s: Socket, level: i32, name: i32, value: *const u8, len: i32) -> i32;
    fn getsockopt(s: Socket, level: i32, name: i32, value: *mut u8, len: *mut i32) -> i32;
    fn ioctlsocket(s: Socket, cmd: i32, arg: *mut u32) -> i32;
    fn select(
        n: i32,
        read: *mut FdSet,
        write: *mut FdSet,
        except: *mut FdSet,
        timeout: *const [i32; 2],
    ) -> i32;
    fn getpeername(s: Socket, addr: *mut SockaddrIn, len: *mut i32) -> i32;
    fn WSAStringToAddressA(
        text: *const u8,
        af: i32,
        info: *const u8,
        addr: *mut u8,
        len: *mut i32,
    ) -> i32;
    fn WSAStringToAddressW(
        text: *const u16,
        af: i32,
        info: *const u8,
        addr: *mut u8,
        len: *mut i32,
    ) -> i32;
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

/// fd_set (winsock2.h): a count and up to FD_SETSIZE (64) sockets.
#[repr(C)]
struct FdSet {
    count: u32,
    sockets: [Socket; 64],
}

impl FdSet {
    fn one(s: Socket) -> FdSet {
        let mut f = FdSet { count: 1, sockets: [0; 64] };
        f.sockets[0] = s;
        f
    }
}

/// WSAPOLLFD.
#[repr(C)]
struct PollFd {
    fd: Socket,
    events: i16,
    revents: i16,
}

const POLLRDNORM: i16 = 0x100;
const POLLWRNORM: i16 = 0x10;
const POLLHUP: i16 = 0x2;

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

/// OVERLAPPED.
#[repr(C)]
struct Overlapped {
    internal: usize,
    internal_high: usize,
    offset: u64,
    event: Handle,
}

/// DisconnectEx(s, lpOverlapped, dwFlags, dwReserved) (mswsock.h).
type DisconnectEx = unsafe extern "system" fn(Socket, *mut Overlapped, u32, u32) -> Bool;

const SIO_GET_EXTENSION_FUNCTION_POINTER: u32 = 0xc800_0006;
/// WSAID_DISCONNECTEX and WSAID_TRANSMITFILE (mswsock.h) as GUID bytes.
const WSAID_DISCONNECTEX: [u8; 16] =
    guid(0x7fda_2e11, 0x8630, 0x436f, [0xa0, 0x31, 0xf5, 0x36, 0xa6, 0xee, 0xc1, 0x57]);
const WSAID_TRANSMITFILE: [u8; 16] =
    guid(0xb536_7df0, 0xcbac, 0x11cf, [0x95, 0xca, 0x00, 0x80, 0x5f, 0x48, 0xa1, 0x92]);
const WSA_IO_PENDING: u32 = 997;
const WSAEINVAL: i32 = 10022;

const fn guid(a: u32, b: u16, c: u16, d: [u8; 8]) -> [u8; 16] {
    let (a, b, c) = (a.to_le_bytes(), b.to_le_bytes(), c.to_le_bytes());
    [a[0], a[1], a[2], a[3], b[0], b[1], c[0], c[1], d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7]]
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
const SOMAXCONN: i32 = 0x7fff_ffff;
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
    #[track_caller]
    fn ok(&mut self, cond: bool, got: u64) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        out(b"winsock_test: check ");
        out(&hex(self.0));
        out(b" got ");
        out(&hex(got as u32));
        check_line(core::panic::Location::caller().line());
        out(b"\r\n");
        Err(self.0)
    }

    #[track_caller]
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
        // The same in UTF-16 (ADDRINFOW has the same layout).
        let (node, service) = (b"localhost\0".map(u16::from), b"22\0".map(u16::from));
        let mut ai = core::ptr::null_mut();
        c.eq(GetAddrInfoW(node.as_ptr(), service.as_ptr(), hints.as_ptr(), &mut ai) as u64, 0)?;
        c.eq((*ai.add(4).cast::<i32>()) as u64, AF_INET as u64)?;
        let sa = *ai.add(32).cast::<*const u8>();
        c.eq(u16::from_be_bytes([*sa.add(2), *sa.add(3)]).into(), 22)?;
        c.ok(*sa.add(4) == 127 && *sa.add(7) == 1, 0)?;
        FreeAddrInfoW(ai);

        // A listener on an ephemeral loopback port.
        let listener = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
        c.ok(listener != INVALID_SOCKET, WSAGetLastError() as u64)?;
        let any_port = SockaddrIn::new(LOOPBACK, 0);
        c.eq(bind(listener, &any_port, ADDR_LEN) as u64, 0)?;
        // TCP keep-alive options (ws2ipdef.h; Netty and Java set and read
        // them): Windows 10's defaults (count 10, interval 1 s, idle
        // 7200 s), then what was set.
        for (option, default, new) in [(16, 10, 5), (17, 1, 2), (3, 7200, 60)] {
            let mut value = 0i32;
            let mut len = 4i32;
            let got = getsockopt(listener, IPPROTO_TCP, option, (&raw mut value).cast(), &mut len);
            c.eq(got as u64, 0)?;
            c.eq(value as u64, default)?;
            c.eq(len as u64, 4)?;
            let v: i32 = new;
            c.eq(setsockopt(listener, IPPROTO_TCP, option, (&raw const v).cast(), 4) as u64, 0)?;
            len = 4;
            getsockopt(listener, IPPROTO_TCP, option, (&raw mut value).cast(), &mut len);
            c.eq(value as u64, new as u64)?;
        }
        // The base provider's handle (SIO_BASE_HANDLE and the BSP forms,
        // as wepoll asks for it): with no layered providers, the socket.
        for code in [0x4800_0022u32, 0x4800_001b, 0x4800_001c, 0x4800_001d] {
            let mut base: Socket = 0;
            let mut returned = 0u32;
            let r = WSAIoctl(
                listener,
                code,
                core::ptr::null(),
                0,
                (&raw mut base).cast(),
                8,
                &mut returned,
                core::ptr::null_mut(),
                0,
            );
            c.eq(r as u64, 0)?;
            c.eq(base as u64, listener as u64)?;
            c.eq(returned.into(), 8)?;
        }
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

        // WSAPoll: nothing to read yet on the server, the client can
        // write (at once, timeout 0); then data arrives.
        let mut fds = [
            PollFd { fd: server, events: POLLRDNORM, revents: -1 },
            PollFd { fd: client, events: POLLWRNORM, revents: -1 },
        ];
        c.eq(WSAPoll(fds.as_mut_ptr(), 2, 0) as u64, 1)?;
        c.eq(fds[0].revents as u64, 0)?;
        c.eq(fds[1].revents as u64, POLLWRNORM as u64)?;
        c.eq(send(client, b"p".as_ptr(), 1, 0) as u64, 1)?;
        let mut fds = [PollFd { fd: server, events: POLLRDNORM, revents: 0 }];
        c.eq(WSAPoll(fds.as_mut_ptr(), 1, 5000) as u64, 1)?;
        c.eq(fds[0].revents as u64, POLLRDNORM as u64)?;
        let mut one = [0u8; 1];
        c.eq(recv(server, one.as_mut_ptr(), 1, 0) as u64, 1)?;

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

        // WSADuplicateSocketW for this process, then WSASocketW from the
        // protocol information: a second socket for the same connection
        // (Win32-OpenSSH's dup2 of a socket); closing it leaves the first.
        let mut info = [0u8; 628];
        c.ok(
            WSADuplicateSocketW(server, GetCurrentProcessId(), info.as_mut_ptr()) == 0,
            WSAGetLastError() as u64,
        )?;
        let field = |at: usize| i32::from_le_bytes(info[at..at + 4].try_into().unwrap_or([0; 4]));
        c.eq(field(76) as u64, AF_INET as u64)?;
        c.eq(field(88) as u64, SOCK_STREAM as u64)?;
        c.eq(field(92) as u64, IPPROTO_TCP as u64)?;
        let dup = WSASocketW(-1, -1, -1, info.as_ptr(), 0, 0);
        c.ok(dup != INVALID_SOCKET && dup != server, WSAGetLastError() as u64)?;
        c.eq(send(dup, b"dup".as_ptr(), 3, 0) as u64, 3)?;
        c.eq(recv(client, buf.as_mut_ptr(), 64, 0) as u64, 3)?;
        c.ok(&buf[..3] == b"dup", buf[0].into())?;
        c.eq(closesocket(dup) as u64, 0)?;
        c.ok(send(server, b"!".as_ptr(), 1, 0) == 1, WSAGetLastError() as u64)?;
        c.eq(recv(client, buf.as_mut_ptr(), 64, 0) as u64, 1)?;
        // The ANSI form (WSAPROTOCOL_INFOA: the same fields, an ANSI name)
        // with WSASocketA, as PostgreSQL's postmaster passes connections.
        let mut info_a = [0u8; 372];
        c.eq(WSADuplicateSocketA(server, GetCurrentProcessId(), info_a.as_mut_ptr()) as u64, 0)?;
        let dup_a = WSASocketA(-1, -1, -1, info_a.as_ptr(), 0, 0);
        c.ok(dup_a != INVALID_SOCKET && dup_a != server, WSAGetLastError() as u64)?;
        c.eq(send(dup_a, b"a".as_ptr(), 1, 0) as u64, 1)?;
        c.eq(recv(client, buf.as_mut_ptr(), 64, 0) as u64, 1)?;
        c.eq(closesocket(dup_a) as u64, 0)?;
        // A process id that names no process: WSAEINVAL.
        c.eq(
            WSADuplicateSocketW(server, 0x7fff_fff0, info.as_mut_ptr()) as u64,
            SOCKET_ERROR as u64,
        )?;
        c.eq(WSAGetLastError() as u64, 10022)?;

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

        // Graceful close: the other end reads end-of-stream (WSAPoll: the
        // socket is readable, and hung up).
        c.eq(closesocket(server) as u64, 0)?;
        let mut fds = [PollFd { fd: client, events: POLLRDNORM, revents: 0 }];
        c.eq(WSAPoll(fds.as_mut_ptr(), 1, 5000) as u64, 1)?;
        c.ok(fds[0].revents & (POLLRDNORM | POLLHUP) != 0, fds[0].revents as u64)?;
        c.eq(recv(client, buf.as_mut_ptr(), 64, 0) as u64, 0)?;
        // WSAConnect and WSAAccept (no condition function, no QoS):
        // connect and accept (PostgreSQL's postmaster accepts so).
        let second = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
        c.eq(WSAConnect(second, &target, ADDR_LEN, null(), null_mut(), null(), null()) as u64, 0)?;
        len = ADDR_LEN;
        let taken = WSAAccept(listener, &mut peer, &mut len, 0, 0);
        c.ok(taken != INVALID_SOCKET, WSAGetLastError() as u64)?;
        c.ok(peer.addr == LOOPBACK, peer.port.into())?;
        c.eq(send(second, b"w".as_ptr(), 1, 0) as u64, 1)?;
        c.eq(recv(taken, buf.as_mut_ptr(), 64, 0) as u64, 1)?;
        c.eq(closesocket(taken) as u64, 0)?;
        c.eq(closesocket(second) as u64, 0)?;
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

        // A non-blocking socket's connect never completes in the call
        // (Windows: WSAEWOULDBLOCK, then writable when connected), even
        // over loopback where the handshake can finish at once (Chromium
        // treats a 0 as unreachable: tcp_socket_win.cc's NOTREACHED).
        let listener3 = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
        c.eq(bind(listener3, &any_port, ADDR_LEN) as u64, 0)?;
        c.eq(listen(listener3, 64) as u64, 0)?;
        // Non-blocking: its accept below only drains (a client closed
        // before its handshake finished leaves nothing to accept).
        let mut on3 = 1u32;
        c.eq(ioctlsocket(listener3, FIONBIO, &mut on3) as u64, 0)?;
        len = ADDR_LEN;
        c.eq(getsockname(listener3, &mut bound, &mut len) as u64, 0)?;
        let to3 = SockaddrIn::new(LOOPBACK, u16::from_be(bound.port));
        for _ in 0..40 {
            let nb = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
            let mut on = 1u32;
            c.eq(ioctlsocket(nb, FIONBIO, &mut on) as u64, 0)?;
            c.eq(connect(nb, &to3, ADDR_LEN) as u64, SOCKET_ERROR as u64)?;
            c.eq(WSAGetLastError() as u64, WSAEWOULDBLOCK as u64)?;
            c.eq(closesocket(nb) as u64, 0)?;
            let a = accept(listener3, core::ptr::null_mut(), core::ptr::null_mut());
            if a != INVALID_SOCKET {
                closesocket(a);
            }
        }
        c.eq(closesocket(listener3) as u64, 0)?;

        // Nobody listens on the port any more.
        let refused = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
        c.ok(refused != INVALID_SOCKET, WSAGetLastError() as u64)?;
        c.eq(connect(refused, &target, ADDR_LEN) as u64, SOCKET_ERROR as u64)?;
        c.eq(WSAGetLastError() as u64, WSAECONNREFUSED as u64)?;
        c.eq(closesocket(refused) as u64, 0)?;

        // The same refusal on a non-blocking connect (Python's
        // create_connection with a timeout): select reports it in the
        // except set, not the write set; SO_ERROR holds WSAECONNREFUSED;
        // the socket has no peer (WSAENOTCONN).
        const SOL_SOCKET: i32 = 0xffff;
        const SO_ERROR: i32 = 0x1007;
        const WSAENOTCONN: i32 = 10057;
        let nb = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
        let mut on = 1u32;
        c.eq(ioctlsocket(nb, FIONBIO, &mut on) as u64, 0)?;
        c.eq(connect(nb, &target, ADDR_LEN) as u64, SOCKET_ERROR as u64)?;
        c.eq(WSAGetLastError() as u64, WSAEWOULDBLOCK as u64)?;
        let (mut w, mut e) = (FdSet::one(nb), FdSet::one(nb));
        c.eq(select(0, null_mut(), &mut w, &mut e, &[10, 0]) as u64, 1)?;
        c.eq(w.count.into(), 0)?;
        c.eq(e.count.into(), 1)?;
        let (mut error, mut elen) = (0i32, 4i32);
        c.eq(getsockopt(nb, SOL_SOCKET, SO_ERROR, (&raw mut error).cast(), &mut elen) as u64, 0)?;
        c.eq(error as u64, WSAECONNREFUSED as u64)?;
        let mut peer = SockaddrIn::new([0; 4], 0);
        len = ADDR_LEN;
        c.eq(getpeername(nb, &mut peer, &mut len) as u64, SOCKET_ERROR as u64)?;
        c.eq(WSAGetLastError() as u64, WSAENOTCONN as u64)?;
        c.eq(closesocket(nb) as u64, 0)?;

        // A closed socket is no socket.
        c.eq(send(refused, b"x".as_ptr(), 1, 0) as u64, SOCKET_ERROR as u64)?;
        c.eq(WSAGetLastError() as u64, WSAENOTSOCK as u64)?;
    }
    Ok(())
}

/// A listener on an ephemeral loopback port with `backlog`; its port.
fn listener_on_loopback(c: &mut Checks, backlog: i32) -> Result<(Socket, u16), u32> {
    // SAFETY: documented Winsock calls with valid arguments.
    unsafe {
        let s = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
        c.ok(s != INVALID_SOCKET, WSAGetLastError() as u64)?;
        c.eq(bind(s, &SockaddrIn::new(LOOPBACK, 0), ADDR_LEN) as u64, 0)?;
        c.ok(listen(s, backlog) == 0, WSAGetLastError() as u64)?;
        let mut bound = SockaddrIn::new([0; 4], 0);
        let mut len = ADDR_LEN;
        c.eq(getsockname(s, &mut bound, &mut len) as u64, 0)?;
        Ok((s, u16::from_be(bound.port)))
    }
}

/// Many listeners at once, and a full backlog.
fn listeners(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: documented Winsock calls with valid arguments.
    unsafe {
        // Twelve listeners at SOMAXCONN (Nginx listens at 511 on each of
        // its ports), all open together; each takes a connection and the
        // bytes go both ways.
        const MANY: usize = 12;
        let mut many = [(INVALID_SOCKET, 0u16); MANY];
        for l in many.iter_mut() {
            *l = listener_on_loopback(c, SOMAXCONN)?;
        }
        for (i, &(l, port)) in many.iter().enumerate() {
            let client = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
            c.ok(client != INVALID_SOCKET, WSAGetLastError() as u64)?;
            c.ok(
                connect(client, &SockaddrIn::new(LOOPBACK, port), ADDR_LEN) == 0,
                WSAGetLastError() as u64,
            )?;
            let server = accept(l, null_mut(), null_mut());
            c.ok(server != INVALID_SOCKET, WSAGetLastError() as u64)?;
            let byte = [i as u8];
            let mut got = [0xffu8; 1];
            c.eq(send(client, byte.as_ptr(), 1, 0) as u64, 1)?;
            c.eq(recv(server, got.as_mut_ptr(), 1, 0) as u64, 1)?;
            c.eq(got[0].into(), i as u64)?;
            c.eq(send(server, byte.as_ptr(), 1, 0) as u64, 1)?;
            c.eq(recv(client, got.as_mut_ptr(), 1, 0) as u64, 1)?;
            c.eq(got[0].into(), i as u64)?;
            c.eq(closesocket(server) as u64, 0)?;
            c.eq(closesocket(client) as u64, 0)?;
        }
        for &(l, _) in &many {
            c.eq(closesocket(l) as u64, 0)?;
        }

        // A full backlog (listen on Microsoft Learn: "If a connection
        // request arrives and the queue is full, the client will receive
        // an error with an indication of WSAECONNREFUSED"): two
        // connections wait unaccepted at backlog 2, the third is refused;
        // once one is accepted there is room for another.
        let (l, port) = listener_on_loopback(c, 2)?;
        let to = SockaddrIn::new(LOOPBACK, port);
        let mut waiting = [INVALID_SOCKET; 2];
        for w in waiting.iter_mut() {
            *w = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
            c.ok(connect(*w, &to, ADDR_LEN) == 0, WSAGetLastError() as u64)?;
        }
        let refused = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
        c.eq(connect(refused, &to, ADDR_LEN) as u64, SOCKET_ERROR as u64)?;
        c.eq(WSAGetLastError() as u64, WSAECONNREFUSED as u64)?;
        c.eq(closesocket(refused) as u64, 0)?;
        let first = accept(l, null_mut(), null_mut());
        c.ok(first != INVALID_SOCKET, WSAGetLastError() as u64)?;
        let late = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
        c.ok(connect(late, &to, ADDR_LEN) == 0, WSAGetLastError() as u64)?;
        // The rest wait in order: the second client, then the late one.
        let mut accepted = [first, INVALID_SOCKET, INVALID_SOCKET];
        for a in accepted[1..].iter_mut() {
            *a = accept(l, null_mut(), null_mut());
            c.ok(*a != INVALID_SOCKET, WSAGetLastError() as u64)?;
        }
        let mut got = [0u8; 1];
        for (i, &client) in [waiting[0], waiting[1], late].iter().enumerate() {
            c.eq(send(client, [b'a' + i as u8].as_ptr(), 1, 0) as u64, 1)?;
            c.eq(recv(accepted[i], got.as_mut_ptr(), 1, 0) as u64, 1)?;
            c.eq(got[0].into(), (b'a' + i as u8).into())?;
        }
        for s in accepted.into_iter().chain(waiting).chain([late, l]) {
            c.eq(closesocket(s) as u64, 0)?;
        }
    }
    Ok(())
}

/// An extension function's pointer from WSAIoctl.
fn extension(c: &mut Checks, s: Socket, id: &[u8; 16]) -> Result<usize, u32> {
    let mut function = 0usize;
    let mut returned = 0u32;
    // SAFETY: a 16-byte GUID in, an 8-byte pointer out.
    let r = unsafe {
        WSAIoctl(
            s,
            SIO_GET_EXTENSION_FUNCTION_POINTER,
            id.as_ptr(),
            16,
            (&raw mut function).cast(),
            8,
            &mut returned,
            null_mut(),
            0,
        )
    };
    c.ok(r == 0, unsafe { WSAGetLastError() } as u64)?;
    c.eq(returned.into(), 8)?;
    c.ok(function != 0, 0)?;
    Ok(function)
}

/// The extension functions asyncio's _overlapped asks for at import
/// (AcceptEx, ConnectEx, DisconnectEx, TransmitFile), and DisconnectEx:
/// a graceful disconnect, overlapped, after which the peer reads the end
/// of the stream; a nonzero dwReserved is WSAEINVAL.
fn disconnect_ex(c: &mut Checks) -> Result<(), u32> {
    let (l, port) = listener_on_loopback(c, 1)?;
    // SAFETY: documented Winsock and kernel32 calls with valid arguments;
    // the function pointer is DisconnectEx's, as WSAIoctl handed it out.
    unsafe {
        let client = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
        c.ok(
            connect(client, &SockaddrIn::new(LOOPBACK, port), ADDR_LEN) == 0,
            WSAGetLastError() as u64,
        )?;
        let server = accept(l, null_mut(), null_mut());
        c.ok(server != INVALID_SOCKET, WSAGetLastError() as u64)?;
        extension(c, client, &WSAID_TRANSMITFILE)?;
        let disconnect: DisconnectEx =
            core::mem::transmute(extension(c, client, &WSAID_DISCONNECTEX)?);
        let mut ov = Overlapped { internal: 0, internal_high: 0, offset: 0, event: null_mut() };
        c.eq(disconnect(client, &mut ov, 0, 1) as u64, 0)?;
        c.eq(WSAGetLastError() as u64, WSAEINVAL as u64)?;
        c.eq(send(client, b"last".as_ptr(), 4, 0) as u64, 4)?;
        ov.event = CreateEventW(null(), 1, 0, null());
        c.ok(!ov.event.is_null(), GetLastError().into())?;
        if disconnect(client, &mut ov, 0, 0) == 0 {
            c.eq(WSAGetLastError() as u64, WSA_IO_PENDING.into())?;
        }
        let mut done = 0u32;
        c.ok(
            GetOverlappedResult(client as Handle, &mut ov, &mut done, 1) != 0,
            GetLastError().into(),
        )?;
        // The peer reads what was sent, then the end of the stream.
        let mut buf = [0u8; 8];
        c.eq(recv_all(server, &mut buf) as u64, 4)?;
        c.ok(&buf[..4] == b"last", buf[0].into())?;
        c.eq(CloseHandle(ov.event) as u64, 1)?;
        for s in [client, server, l] {
            c.eq(closesocket(s) as u64, 0)?;
        }
    }
    Ok(())
}

/// Whether the NUL-terminated string at `p` is `want` (compared byte by
/// byte up to `want`'s length and its NUL, so no strlen is needed).
///
/// # Safety
/// `p` is a NUL-terminated string.
unsafe fn c_str_is(p: *const u8, want: &[u8]) -> bool {
    for (i, w) in want.iter().enumerate() {
        // SAFETY: bytes before the string's NUL (a mismatch stops first).
        if unsafe { *p.add(i) } != *w {
            return false;
        }
    }
    // SAFETY: the byte after `want.len()` matching ones is within the
    // string or its NUL.
    unsafe { *p.add(want.len()) == 0 }
}

/// getprotobyname and getprotobynumber over the protocol database
/// (`%SystemRoot%\System32\drivers\etc\protocol`): tcp is 6 with the
/// alias TCP, 17 is udp, and an unknown name or number is NULL with
/// WSANO_DATA.
fn protocols(c: &mut Checks) -> Result<(), u32> {
    const WSANO_DATA: u64 = 11004;
    // SAFETY: NUL-terminated names; the PROTOENTs Winsock returns.
    unsafe {
        let tcp = getprotobyname(c"tcp".as_ptr().cast());
        c.ok(!tcp.is_null(), WSAGetLastError() as u64)?;
        c.eq((*tcp).proto as u64, 6)?;
        c.ok(c_str_is((*tcp).name, b"tcp"), 0)?;
        let alias = *(*tcp).aliases;
        c.ok(!alias.is_null() && c_str_is(alias, b"TCP"), 0)?;
        let udp = getprotobynumber(17);
        c.ok(!udp.is_null(), WSAGetLastError() as u64)?;
        c.ok(c_str_is((*udp).name, b"udp"), 0)?;
        c.eq((*udp).proto as u64, 17)?;
        c.ok(getprotobyname(c"nosuchprotocol".as_ptr().cast()).is_null(), 0)?;
        c.eq(WSAGetLastError() as u64, WSANO_DATA)?;
        c.ok(getprotobynumber(250).is_null(), 0)?;
        c.eq(WSAGetLastError() as u64, WSANO_DATA)?;
    }
    Ok(())
}

/// WSAStringToAddressA/W (asyncio's _overlapped parses addresses with
/// it): IPv4 with and without a port, IPv6 bracketed with a port and with
/// a scope; WSAEINVAL for text that is no address of the family,
/// WSAEFAULT for a buffer smaller than the family's SOCKADDR.
fn string_to_address(c: &mut Checks) -> Result<(), u32> {
    const AF_INET6: i32 = 23;
    const WSAEFAULT: i32 = 10014;
    // SAFETY: NUL-terminated strings and buffers of the given lengths.
    unsafe {
        let mut sa = [0xffu8; 28];
        let mut len = 28i32;
        c.eq(
            WSAStringToAddressA(
                c"127.0.0.1:8080".as_ptr().cast(),
                AF_INET,
                null(),
                sa.as_mut_ptr(),
                &mut len,
            ) as u64,
            0,
        )?;
        c.eq(len as u64, 16)?;
        c.ok(sa[..8] == [2, 0, 0x1f, 0x90, 127, 0, 0, 1], sa[2].into())?;
        len = 16;
        c.eq(
            WSAStringToAddressA(
                c"10.1.2.3".as_ptr().cast(),
                AF_INET,
                null(),
                sa.as_mut_ptr(),
                &mut len,
            ) as u64,
            0,
        )?;
        c.ok(sa[..8] == [2, 0, 0, 0, 10, 1, 2, 3], sa[2].into())?;
        let text = b"[::1]:443\0".map(u16::from);
        len = 28;
        c.eq(
            WSAStringToAddressW(text.as_ptr(), AF_INET6, null(), sa.as_mut_ptr(), &mut len) as u64,
            0,
        )?;
        c.eq(len as u64, 28)?;
        // SOCKADDR_IN6: family, port, flow info, address, scope id.
        c.ok(sa[..8] == [23, 0, 1, 0xbb, 0, 0, 0, 0], sa[2].into())?;
        c.ok(sa[8..24] == [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1], sa[23].into())?;
        c.eq(u32::from_le_bytes([sa[24], sa[25], sa[26], sa[27]]).into(), 0)?;
        len = 28;
        c.eq(
            WSAStringToAddressA(
                c"fe80::1%3".as_ptr().cast(),
                AF_INET6,
                null(),
                sa.as_mut_ptr(),
                &mut len,
            ) as u64,
            0,
        )?;
        c.ok(sa[2..4] == [0, 0] && sa[8..10] == [0xfe, 0x80] && sa[23] == 1, sa[8].into())?;
        c.eq(u32::from_le_bytes([sa[24], sa[25], sa[26], sa[27]]).into(), 3)?;
        for (text, af) in [
            (c"bogus", AF_INET),
            (c"1.2.3.4", AF_INET6),
            (c"::1", AF_INET),
            (c"1.2.3.4:99999", AF_INET),
        ] {
            len = 28;
            c.eq(
                WSAStringToAddressA(text.as_ptr().cast(), af, null(), sa.as_mut_ptr(), &mut len)
                    as u64,
                SOCKET_ERROR as u64,
            )?;
            c.eq(WSAGetLastError() as u64, WSAEINVAL as u64)?;
        }
        len = 8;
        c.eq(
            WSAStringToAddressA(
                c"127.0.0.1".as_ptr().cast(),
                AF_INET,
                null(),
                sa.as_mut_ptr(),
                &mut len,
            ) as u64,
            SOCKET_ERROR as u64,
        )?;
        c.eq(WSAGetLastError() as u64, WSAEFAULT as u64)?;
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
    listeners(&mut c)?;
    disconnect_ex(&mut c)?;
    string_to_address(&mut c)?;
    protocols(&mut c)?;
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

/// ` (line L)`: the source line of the failing check, from
/// `#[track_caller]` on Checks' methods (so a report names the line
/// without anyone counting checks).
fn check_line(line: u32) {
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
    out(&b[..8 + len]);
}

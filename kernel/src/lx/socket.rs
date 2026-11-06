//! TCP sockets for the Linux personality (ROADMAP LX, H11; user/lx/
//! socket_test is the spec, recorded on Linux) on the kernel's own TCP
//! stack (`net::tcp`, which NT's AFD uses too): socket, bind, listen,
//! accept/accept4, connect, the data calls (read/write and readv/writev
//! through `lx::dispatch`, send/recv/sendto/recvfrom here), shutdown,
//! getsockname/getpeername, setsockopt/getsockopt, and readiness for
//! poll and select.
//!
//! - AF_INET and AF_INET6 stream sockets; an AF_INET6 one takes IPv4
//!   peers too unless IPV6_V6ONLY is set (Linux's default: dual stack;
//!   `net::Scope`), and sees them as IPv4-mapped addresses. Datagram,
//!   Unix and other sockets are refused and ledgered (`linux option
//!   missing`).
//! - Waiting (accept, a blocking connect, recv, send while the send
//!   buffer is full) polls every millisecond, as pipes do (no wait queue
//!   on the stack's notify hook yet: it is AFD's); it ends with EINTR when
//!   a signal is to be delivered (restarted by `sigcalls::deliver` for
//!   read/write after SA_RESTART).
//! - A connect that is refused leaves the socket unconnected (it can
//!   connect again); its error is SO_ERROR's once for a non-blocking one.
//!   A connect on a socket whose non-blocking connect has completed since
//!   returns 0 once, then EISCONN (socket_test: Linux's).
//! - Options are kept and read back (SO_REUSEADDR, SO_KEEPALIVE,
//!   SO_REUSEPORT, SO_BROADCAST, buffer sizes, SO_LINGER, the timeouts,
//!   TCP_NODELAY, IPV6_V6ONLY) but change nothing on the stack yet except
//!   IPV6_V6ONLY; others are ENOPROTOOPT, ledgered.
//!
//! Observability: the stack's own `tcp accept`/`tcp connect`/`tcp close`
//! events (`events=net`) name each connection; refused or unsupported
//! cases are in the ledger; a socket the stack cannot make logs nothing
//! more than its errno in the flight recorder.

use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

use super::dispatch::{Call, Result};
use super::errno::{
    EADDRINUSE, EADDRNOTAVAIL, EAFNOSUPPORT, EAGAIN, EALREADY, EBADF, ECONNREFUSED, ECONNRESET,
    EFAULT, EINPROGRESS, EINVAL, EISCONN, ENETUNREACH, ENOMEM, ENOPROTOOPT, ENOTCONN, ENOTSOCK,
    EPIPE, EPROTONOSUPPORT,
};
use super::sigcalls;
use super::state::{Fd, Object};
use crate::mm::heap::KArc;
use crate::net::tcp::{self, Addr, NetError, SocketId};
use crate::net::{Ip, Scope};
use crate::nt::ledger;
use crate::sync::IrqMutex;

pub const AF_INET: u16 = 2;
pub const AF_INET6: u16 = 10;
const SOCK_STREAM: u64 = 1;
const SOCK_NONBLOCK: u64 = 0o4000;
const SOCK_CLOEXEC: u64 = 0o2000000;
const IPPROTO_TCP: u64 = 6;
const IPPROTO_IPV6: u64 = 41;
const SOL_SOCKET: u64 = 1;
const SO_REUSEADDR: u64 = 2;
const SO_TYPE: u64 = 3;
const SO_ERROR: u64 = 4;
const SO_BROADCAST: u64 = 6;
const SO_SNDBUF: u64 = 7;
const SO_RCVBUF: u64 = 8;
const SO_KEEPALIVE: u64 = 9;
const SO_LINGER: u64 = 13;
const SO_REUSEPORT: u64 = 15;
const SO_RCVTIMEO: u64 = 20;
const SO_SNDTIMEO: u64 = 21;
const SO_ACCEPTCONN: u64 = 30;
const TCP_NODELAY: u64 = 1;
const IPV6_V6ONLY: u64 = 26;
pub const MSG_PEEK: u64 = 2;
pub const MSG_DONTWAIT: u64 = 0x40;
const MSG_WAITALL: u64 = 0x100;
pub const MSG_NOSIGNAL: u64 = 0x4000;
/// The token the stack's notify hook gets for Linux sockets: none of
/// AFD's (it ignores tokens it does not know); Linux sockets poll.
const TOKEN: u64 = 1 << 63;
/// The most bytes one call moves through the kernel at a time.
const CHUNK: usize = 64 * 1024;
/// The buffer sizes SO_SNDBUF/SO_RCVBUF report until set (the stack's
/// 64 KiB per direction).
const BUFFER: u32 = 64 * 1024;

/// Where a socket is in its life.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SockState {
    Fresh,
    Bound(SocketId),
    Listening(SocketId),
    /// A non-blocking connect under way (or an interrupted blocking one).
    Connecting(SocketId),
    Connected(SocketId),
}

impl SockState {
    fn id(self) -> Option<SocketId> {
        match self {
            SockState::Fresh => None,
            SockState::Bound(id)
            | SockState::Listening(id)
            | SockState::Connecting(id)
            | SockState::Connected(id) => Some(id),
        }
    }
}

/// The options kept for getsockopt (see the module doc).
#[derive(Clone, Copy, Default)]
struct Options {
    reuseaddr: u32,
    keepalive: u32,
    reuseport: u32,
    broadcast: u32,
    nodelay: u32,
    v6only: u32,
    sndbuf: Option<u32>,
    rcvbuf: Option<u32>,
    linger: [u8; 8],
    rcvtimeo: [u8; 16],
    sndtimeo: [u8; 16],
}

/// A Linux socket (one open file description: dup'ed and inherited fds
/// share it; the connection closes with the last).
pub struct Socket {
    pub family: u16,
    pub state: IrqMutex<SockState>,
    options: IrqMutex<Options>,
    /// SO_ERROR: a non-blocking connect's failure, reported once.
    error: IrqMutex<u16>,
    shut_read: AtomicBool,
    shut_write: AtomicBool,
}

impl Drop for Socket {
    fn drop(&mut self) {
        if let Some(id) = self.state.lock().id() {
            tcp::close(id, false);
            tcp::kick();
        }
    }
}

fn errno_of(e: NetError) -> u16 {
    match e {
        NetError::WouldBlock => EAGAIN,
        NetError::InvalidState => EINVAL,
        NetError::AddressInUse => EADDRINUSE,
        NetError::AddressNotAvailable => EADDRNOTAVAIL,
        NetError::Refused => ECONNREFUSED,
        NetError::Reset => ECONNRESET,
        NetError::NotConnected | NetError::NoSocket => ENOTCONN,
        NetError::Unreachable => ENETUNREACH,
        NetError::NoMemory => ENOMEM,
        NetError::TooLarge => super::errno::EMSGSIZE,
    }
}

/// The socket behind `fd`: EBADF if it is not open, ENOTSOCK if it is
/// not a socket.
pub fn socket_of(call: &Call<'_>, fd: u64) -> core::result::Result<(KArc<Fd>, KArc<Socket>), u16> {
    let f = call.lx.fd(fd).ok_or(EBADF)?;
    let s = match &f.object {
        Object::Socket(s) => s.clone(),
        _ => return Err(ENOTSOCK),
    };
    Ok((f, s))
}

/// socket(2).
pub fn socket(call: &Call<'_>, domain: u64, kind: u64, protocol: u64) -> Result {
    let flags = kind & (SOCK_NONBLOCK | SOCK_CLOEXEC);
    let base = kind & !(SOCK_NONBLOCK | SOCK_CLOEXEC);
    let family = match domain {
        2 => AF_INET,
        10 => AF_INET6,
        _ => {
            if domain == 1 || domain == 16 {
                // AF_UNIX, AF_NETLINK: Linux has them; gala not yet.
                ledger::record("socket", "linux option missing", domain);
            }
            return Err(EAFNOSUPPORT);
        }
    };
    // SOCK_DGRAM, SOCK_RAW, SOCK_RDM, SOCK_SEQPACKET, SOCK_DCCP, SOCK_PACKET.
    if base != SOCK_STREAM {
        if matches!(base, 2 | 3 | 4 | 5 | 6 | 10) {
            ledger::record("socket", "linux option missing", domain << 8 | base);
            return Err(EPROTONOSUPPORT);
        }
        return Err(EINVAL);
    }
    if protocol != 0 && protocol != IPPROTO_TCP {
        return Err(EPROTONOSUPPORT);
    }
    let sock = KArc::try_new(Socket {
        family,
        state: IrqMutex::new(SockState::Fresh),
        options: IrqMutex::new(Options::default()),
        error: IrqMutex::new(0),
        shut_read: AtomicBool::new(false),
        shut_write: AtomicBool::new(false),
    })
    .map_err(|_| ENOMEM)?;
    let fd = KArc::try_new(Fd::new(Object::Socket(sock), flags & SOCK_NONBLOCK != 0))
        .map_err(|_| ENOMEM)?;
    call.lx.install_from(fd, flags & SOCK_CLOEXEC != 0, 0)
}

/// A `sockaddr_in`/`sockaddr_in6` from user memory, for a socket of
/// `family`: EINVAL if too short, `wrong_family` for another family.
fn read_addr(
    call: &Call<'_>,
    family: u16,
    addr: u64,
    len: u64,
    wrong_family: u16,
) -> core::result::Result<Addr, u16> {
    let len = u64::from(len as u32);
    let need = if family == AF_INET { 16 } else { 24 };
    if len < need {
        return Err(EINVAL);
    }
    let mut b = [0u8; 24];
    call.process.read(addr, &mut b[..need as usize]).map_err(|_| EFAULT)?;
    if u16::from_le_bytes([b[0], b[1]]) != family {
        return Err(wrong_family);
    }
    let port = u16::from_be_bytes([b[2], b[3]]);
    let ip = if family == AF_INET {
        Ip::V4([b[4], b[5], b[6], b[7]])
    } else {
        let mut a = [0u8; 16];
        a.copy_from_slice(&b[8..24]);
        Ip::V6(a).unmapped()
    };
    Ok(Addr { ip, port })
}

/// Writes `a` as a `sockaddr` of `family` at `addr` (at most `*len`
/// bytes; `*len` set to its whole size), if `addr` is not NULL.
fn write_addr(
    call: &Call<'_>,
    family: u16,
    a: Addr,
    addr: u64,
    len: u64,
) -> core::result::Result<(), u16> {
    if addr == 0 {
        return Ok(());
    }
    let mut lb = [0u8; 4];
    call.process.read(len, &mut lb).map_err(|_| EFAULT)?;
    let room = u32::from_le_bytes(lb) as i32;
    if room < 0 {
        return Err(EINVAL);
    }
    let mut b = [0u8; 28];
    b[..2].copy_from_slice(&family.to_le_bytes());
    b[2..4].copy_from_slice(&a.port.to_be_bytes());
    let size = if family == AF_INET {
        if let Ip::V4(ip) = a.ip.unmapped() {
            b[4..8].copy_from_slice(&ip);
        }
        16
    } else {
        if let Ip::V6(ip) = a.ip.mapped() {
            b[8..24].copy_from_slice(&ip);
        }
        28
    };
    let n = (room as usize).min(size);
    call.process.write(addr, &b[..n]).map_err(|_| EFAULT)?;
    call.process.write(len, &(size as u32).to_le_bytes()).map_err(|_| EFAULT)?;
    Ok(())
}

/// The families a socket of `family` takes.
fn scope(sock: &Socket) -> Scope {
    match sock.family {
        AF_INET => Scope::V4,
        _ if sock.options.lock().v6only != 0 => Scope::V6,
        _ => Scope::Dual,
    }
}

pub fn bind(call: &Call<'_>, fd: u64, addr: u64, len: u64) -> Result {
    let (_, sock) = socket_of(call, fd)?;
    let local = read_addr(call, sock.family, addr, len, EINVAL)?;
    let mut state = sock.state.lock();
    if *state != SockState::Fresh {
        return Err(EINVAL);
    }
    let id = tcp::bind_scoped(local, scope(&sock), TOKEN).map_err(errno_of)?;
    *state = SockState::Bound(id);
    Ok(0)
}

pub fn listen(call: &Call<'_>, fd: u64, backlog: u64) -> Result {
    let (_, sock) = socket_of(call, fd)?;
    let mut state = sock.state.lock();
    let id = match *state {
        // Not bound yet: an ephemeral port on every address.
        SockState::Fresh => {
            let any = if sock.family == AF_INET { Ip::V4([0; 4]) } else { Ip::V6([0; 16]) };
            tcp::bind_scoped(Addr { ip: any, port: 0 }, scope(&sock), TOKEN).map_err(errno_of)?
        }
        SockState::Bound(id) | SockState::Listening(id) => id,
        SockState::Connecting(_) | SockState::Connected(_) => return Err(EINVAL),
    };
    // An int; negative or huge is the most the stack takes.
    let backlog = (backlog as i32).clamp(1, i32::MAX) as usize;
    if let Err(e) = tcp::listen(id, backlog) {
        if *state == SockState::Fresh {
            tcp::close(id, false);
        }
        return Err(errno_of(e));
    }
    *state = SockState::Listening(id);
    tcp::kick();
    Ok(0)
}

use super::dispatch::pause;

/// accept and accept4.
pub fn accept4(call: &Call<'_>, fd: u64, addr: u64, len: u64, flags: u64) -> Result {
    if flags & !(SOCK_NONBLOCK | SOCK_CLOEXEC) != 0 {
        return Err(EINVAL);
    }
    let (f, sock) = socket_of(call, fd)?;
    let SockState::Listening(id) = *sock.state.lock() else { return Err(EINVAL) };
    let (sequence, remote) = loop {
        match tcp::pending(id) {
            Ok(p) => break p,
            Err(NetError::WouldBlock) if f.nonblock() => return Err(EAGAIN),
            Err(NetError::WouldBlock) => pause(call)?,
            Err(e) => return Err(errno_of(e)),
        }
    };
    let new = tcp::accept(id, sequence, TOKEN).map_err(errno_of)?;
    let options = *sock.options.lock();
    let accepted = KArc::try_new(Socket {
        family: sock.family,
        state: IrqMutex::new(SockState::Connected(new)),
        options: IrqMutex::new(options),
        error: IrqMutex::new(0),
        shut_read: AtomicBool::new(false),
        shut_write: AtomicBool::new(false),
    });
    let Ok(accepted) = accepted else {
        tcp::close(new, true);
        return Err(ENOMEM);
    };
    // From here on dropping `accepted` closes the connection.
    let fd = KArc::try_new(Fd::new(Object::Socket(accepted), flags & SOCK_NONBLOCK != 0))
        .map_err(|_| ENOMEM)?;
    let newfd = call.lx.install_from(fd, flags & SOCK_CLOEXEC != 0, 0)?;
    if let Err(e) = write_addr(call, sock.family, remote, addr, len) {
        call.lx.close(newfd);
        return Err(e);
    }
    Ok(newfd)
}

/// A connecting socket's progress: Ok(true) once established, Ok(false)
/// while under way, the error (and the socket unconnected again) if
/// refused. Only connect itself makes it Connected (`promote`): until
/// then the next connect returns 0 once (socket_test: Linux's socket
/// state moves on in connect, the connection's own earlier).
fn progress(
    sock: &Socket,
    state: &mut SockState,
    promote: bool,
) -> core::result::Result<bool, u16> {
    let SockState::Connecting(id) = *state else { return Ok(true) };
    match tcp::connected(id) {
        Ok(()) => {
            if promote {
                *state = SockState::Connected(id);
            }
            Ok(true)
        }
        Err(NetError::WouldBlock) => Ok(false),
        Err(e) => {
            tcp::close(id, true);
            *state = SockState::Fresh;
            let errno = errno_of(e);
            *sock.error.lock() = errno;
            Err(errno)
        }
    }
}

pub fn connect(call: &Call<'_>, fd: u64, addr: u64, len: u64) -> Result {
    let (f, sock) = socket_of(call, fd)?;
    let mut remote = read_addr(call, sock.family, addr, len, EAFNOSUPPORT)?;
    if remote.ip.is_unspecified() {
        // Linux connects to the local host.
        remote.ip = remote.ip.loopback_like();
    }
    {
        let mut state = sock.state.lock();
        match *state {
            SockState::Connected(_) => return Err(EISCONN),
            SockState::Listening(_) => return Err(EINVAL),
            SockState::Connecting(_) => {
                return match progress(&sock, &mut state, true) {
                    // Completed since: 0 this once (then EISCONN).
                    Ok(true) => {
                        *sock.error.lock() = 0;
                        Ok(0)
                    }
                    Ok(false) => Err(EALREADY),
                    Err(e) => {
                        *sock.error.lock() = 0;
                        Err(e)
                    }
                };
            }
            SockState::Fresh | SockState::Bound(_) => {}
        }
        if sock.family == AF_INET6 && scope(&sock) == Scope::V6 && !remote.ip.is_v6() {
            return Err(ENETUNREACH);
        }
        let id = tcp::connect(state.id(), remote, TOKEN).map_err(errno_of)?;
        *state = SockState::Connecting(id);
        tcp::kick();
    }
    if f.nonblock() {
        // The handshake goes on without the caller, even on loopback
        // (socket_test: Linux always answers EINPROGRESS here; the result
        // comes through poll and SO_ERROR, or the next connect).
        return Err(EINPROGRESS);
    }
    loop {
        {
            let mut state = sock.state.lock();
            match progress(&sock, &mut state, true) {
                Ok(true) => return Ok(0),
                Ok(false) => {}
                Err(e) => {
                    *sock.error.lock() = 0;
                    return Err(e);
                }
            }
        }
        // Interrupted, the connect goes on (EINTR; the next connect is
        // EALREADY until it finishes).
        pause(call)?;
    }
}

/// The connection of a socket that should have one (a non-blocking
/// connect's once it is established).
fn connection(sock: &Socket) -> core::result::Result<SocketId, u16> {
    let mut state = sock.state.lock();
    let now = *state;
    match now {
        SockState::Connected(id) => Ok(id),
        SockState::Connecting(id) if progress(sock, &mut state, false)? => Ok(id),
        _ => Err(ENOTCONN),
    }
}

/// Receives up to `count` bytes into user memory at `buf` (read, recv,
/// recvfrom).
pub fn recv(call: &Call<'_>, f: &Fd, sock: &Socket, buf: u64, count: u64, flags: u64) -> Result {
    let id = connection(sock)?;
    if sock.shut_read.load(Ordering::Relaxed) || count == 0 {
        return Ok(0);
    }
    let nonblock = f.nonblock() || flags & MSG_DONTWAIT != 0;
    let want = count.min(CHUNK as u64) as usize;
    let mut data = Vec::new();
    data.try_reserve_exact(want).map_err(|_| ENOMEM)?;
    data.resize(want, 0);
    let mut done = 0usize;
    loop {
        let got = if flags & MSG_PEEK != 0 {
            tcp::peek(id, &mut data[done..])
        } else {
            tcp::recv(id, &mut data[done..])
        };
        match got {
            Ok(n) => {
                if n > 0 {
                    tcp::kick();
                }
                let more = n > 0 && flags & MSG_WAITALL != 0 && flags & MSG_PEEK == 0;
                done += n;
                if !more || done == want {
                    break;
                }
            }
            Err(NetError::WouldBlock) if done > 0 => break,
            Err(NetError::WouldBlock) if nonblock => return Err(EAGAIN),
            Err(NetError::WouldBlock) => pause(call)?,
            Err(e) if done > 0 => {
                let _ = e;
                break;
            }
            Err(e) => return Err(errno_of(e)),
        }
    }
    call.process.write(buf, &data[..done]).map_err(|_| EFAULT)?;
    Ok(done as u64)
}

/// Sends `count` bytes, fetched by `fetch(offset, out)` (user memory or
/// a kernel buffer); blocking, all of them (a signal ends it with what
/// was sent, or EINTR). A connection that cannot send any more is EPIPE
/// with SIGPIPE (unless MSG_NOSIGNAL).
pub fn send(
    call: &Call<'_>,
    f: &Fd,
    sock: &Socket,
    count: usize,
    flags: u64,
    mut fetch: impl FnMut(usize, &mut [u8]) -> core::result::Result<(), u16>,
) -> Result {
    let id = connection(sock)?;
    let nonblock = f.nonblock() || flags & MSG_DONTWAIT != 0;
    let partial = |done: usize, e: u16| if done > 0 { Ok(done as u64) } else { Err(e) };
    let broken = |done: usize| {
        if flags & MSG_NOSIGNAL == 0 {
            sigcalls::sigpipe(call);
        }
        partial(done, EPIPE)
    };
    if sock.shut_write.load(Ordering::Relaxed) {
        return broken(0);
    }
    let mut done = 0usize;
    let mut chunk = Vec::new();
    while done < count {
        let n = (count - done).min(CHUNK);
        chunk.clear();
        if chunk.try_reserve_exact(n).is_err() {
            return partial(done, ENOMEM);
        }
        chunk.resize(n, 0);
        if let Err(e) = fetch(done, &mut chunk) {
            return partial(done, e);
        }
        let mut at = 0;
        while at < n {
            match tcp::send(id, &chunk[at..]) {
                Ok(k) => {
                    at += k;
                    done += k;
                    tcp::kick();
                }
                Err(NetError::WouldBlock) if nonblock => return partial(done, EAGAIN),
                Err(NetError::WouldBlock) => {
                    if let Err(e) = pause(call) {
                        return partial(done, e);
                    }
                }
                Err(NetError::Reset | NetError::InvalidState) => return broken(done),
                Err(e) => return partial(done, errno_of(e)),
            }
        }
    }
    Ok(done as u64)
}

/// sendto (and send): the address is ignored on a connected stream.
pub fn sendto(call: &Call<'_>, fd: u64, buf: u64, len: u64, flags: u64) -> Result {
    let (f, sock) = socket_of(call, fd)?;
    let len = len.min(0x7fff_f000) as usize;
    send(call, &f, &sock, len, flags, |at, out| {
        call.process.read(buf + at as u64, out).map_err(|_| EFAULT)
    })
}

/// recvfrom (and recv): the peer's address too if asked.
pub fn recvfrom(
    call: &Call<'_>,
    fd: u64,
    buf: u64,
    len: u64,
    flags: u64,
    addr: u64,
    addrlen: u64,
) -> Result {
    let (f, sock) = socket_of(call, fd)?;
    let n = recv(call, &f, &sock, buf, len, flags)?;
    if addr != 0 {
        let id = connection(&sock)?;
        let remote = tcp::remote(id).map_err(errno_of)?;
        write_addr(call, sock.family, remote, addr, addrlen)?;
    }
    Ok(n)
}

pub fn shutdown(call: &Call<'_>, fd: u64, how: u64) -> Result {
    let (_, sock) = socket_of(call, fd)?;
    if how > 2 {
        return Err(EINVAL);
    }
    let id = connection(&sock)?;
    if how != 1 {
        sock.shut_read.store(true, Ordering::Relaxed);
    }
    if how != 0 && !sock.shut_write.swap(true, Ordering::Relaxed) {
        tcp::shutdown_send(id).map_err(errno_of)?;
        tcp::kick();
    }
    Ok(0)
}

pub fn getsockname(call: &Call<'_>, fd: u64, addr: u64, len: u64) -> Result {
    let (_, sock) = socket_of(call, fd)?;
    let id = sock.state.lock().id();
    let local = match id {
        Some(id) => tcp::local(id).map_err(errno_of)?,
        None => {
            let ip = if sock.family == AF_INET { Ip::V4([0; 4]) } else { Ip::V6([0; 16]) };
            Addr { ip, port: 0 }
        }
    };
    write_addr(call, sock.family, local, addr, len)?;
    Ok(0)
}

pub fn getpeername(call: &Call<'_>, fd: u64, addr: u64, len: u64) -> Result {
    let (_, sock) = socket_of(call, fd)?;
    let id = connection(&sock)?;
    let remote = tcp::remote(id).map_err(errno_of)?;
    write_addr(call, sock.family, remote, addr, len)?;
    Ok(0)
}

pub fn setsockopt(call: &Call<'_>, fd: u64, level: u64, name: u64, val: u64, len: u64) -> Result {
    let (_, sock) = socket_of(call, fd)?;
    let len = u64::from(len as u32) as usize;
    let mut b = [0u8; 16];
    let int = |b: &[u8; 16]| u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
    let need = match (level, name) {
        (SOL_SOCKET, SO_LINGER) => 8,
        (SOL_SOCKET, SO_RCVTIMEO | SO_SNDTIMEO) => 16,
        _ => 4,
    };
    if len < need {
        return Err(EINVAL);
    }
    call.process.read(val, &mut b[..need]).map_err(|_| EFAULT)?;
    let mut o = sock.options.lock();
    let flag = u32::from(int(&b) != 0);
    match (level, name) {
        (SOL_SOCKET, SO_REUSEADDR) => o.reuseaddr = flag,
        (SOL_SOCKET, SO_KEEPALIVE) => o.keepalive = flag,
        (SOL_SOCKET, SO_REUSEPORT) => o.reuseport = flag,
        (SOL_SOCKET, SO_BROADCAST) => o.broadcast = flag,
        // Linux doubles the size asked for (socket(7)).
        (SOL_SOCKET, SO_SNDBUF) => o.sndbuf = Some(int(&b).saturating_mul(2)),
        (SOL_SOCKET, SO_RCVBUF) => o.rcvbuf = Some(int(&b).saturating_mul(2)),
        (SOL_SOCKET, SO_LINGER) => o.linger.copy_from_slice(&b[..8]),
        (SOL_SOCKET, SO_RCVTIMEO) => o.rcvtimeo = b,
        (SOL_SOCKET, SO_SNDTIMEO) => o.sndtimeo = b,
        (IPPROTO_TCP, TCP_NODELAY) => o.nodelay = flag,
        (IPPROTO_IPV6, IPV6_V6ONLY) if sock.family == AF_INET6 => {
            if *sock.state.lock() != SockState::Fresh {
                return Err(EINVAL);
            }
            o.v6only = flag;
        }
        _ => {
            ledger::record("setsockopt", "linux option missing", level << 32 | name);
            return Err(ENOPROTOOPT);
        }
    }
    Ok(0)
}

pub fn getsockopt(call: &Call<'_>, fd: u64, level: u64, name: u64, val: u64, len: u64) -> Result {
    let (_, sock) = socket_of(call, fd)?;
    let mut lb = [0u8; 4];
    call.process.read(len, &mut lb).map_err(|_| EFAULT)?;
    let room = u32::from_le_bytes(lb) as i32;
    if room < 0 {
        return Err(EINVAL);
    }
    let o = *sock.options.lock();
    let int = |v: u32| {
        let mut b = [0u8; 16];
        b[..4].copy_from_slice(&v.to_le_bytes());
        (b, 4usize)
    };
    let (bytes, size) = match (level, name) {
        (SOL_SOCKET, SO_TYPE) => int(SOCK_STREAM as u32),
        (SOL_SOCKET, SO_ERROR) => {
            {
                let mut state = sock.state.lock();
                let _ = progress(&sock, &mut state, false);
            }
            int(u32::from(core::mem::take(&mut *sock.error.lock())))
        }
        (SOL_SOCKET, SO_ACCEPTCONN) => {
            int(u32::from(matches!(*sock.state.lock(), SockState::Listening(_))))
        }
        (SOL_SOCKET, SO_REUSEADDR) => int(o.reuseaddr),
        (SOL_SOCKET, SO_KEEPALIVE) => int(o.keepalive),
        (SOL_SOCKET, SO_REUSEPORT) => int(o.reuseport),
        (SOL_SOCKET, SO_BROADCAST) => int(o.broadcast),
        (SOL_SOCKET, SO_SNDBUF) => int(o.sndbuf.unwrap_or(BUFFER)),
        (SOL_SOCKET, SO_RCVBUF) => int(o.rcvbuf.unwrap_or(BUFFER)),
        (SOL_SOCKET, SO_LINGER) => {
            let mut b = [0u8; 16];
            b[..8].copy_from_slice(&o.linger);
            (b, 8)
        }
        (SOL_SOCKET, SO_RCVTIMEO) => (o.rcvtimeo, 16),
        (SOL_SOCKET, SO_SNDTIMEO) => (o.sndtimeo, 16),
        (IPPROTO_TCP, TCP_NODELAY) => int(o.nodelay),
        (IPPROTO_IPV6, IPV6_V6ONLY) if sock.family == AF_INET6 => int(o.v6only),
        _ => {
            ledger::record("getsockopt", "linux option missing", level << 32 | name);
            return Err(ENOPROTOOPT);
        }
    };
    let n = (room as usize).min(size);
    call.process.write(val, &bytes[..n]).map_err(|_| EFAULT)?;
    call.process.write(len, &(n as u32).to_le_bytes()).map_err(|_| EFAULT)?;
    Ok(0)
}

// poll(2)'s event bits.
pub const POLLIN: u16 = 0x1;
pub const POLLOUT: u16 = 0x4;
pub const POLLERR: u16 = 0x8;
pub const POLLHUP: u16 = 0x10;
pub const POLLRDNORM: u16 = 0x40;
pub const POLLWRNORM: u16 = 0x100;
pub const POLLRDHUP: u16 = 0x2000;

/// What poll would report for the socket now (socket_test on Linux: a
/// listener is readable with a connection to accept; a connection
/// readable with bytes or at the end of the stream, writable with room,
/// HUP once both directions are done; a refused connect ERR|HUP).
pub fn readiness(sock: &Socket) -> u16 {
    let mut state = sock.state.lock();
    let established = match *state {
        SockState::Connecting(_) => match progress(sock, &mut state, false) {
            Ok(done) => done,
            // Refused (the error is kept for SO_ERROR).
            Err(_) => return POLLOUT | POLLERR | POLLHUP,
        },
        _ => true,
    };
    let state = match *state {
        SockState::Connecting(id) if established => SockState::Connected(id),
        s => s,
    };
    match state {
        SockState::Fresh => POLLOUT | POLLHUP,
        SockState::Bound(_) => POLLOUT | POLLHUP,
        SockState::Listening(id) => match tcp::readiness(id) {
            Ok(r) if r.acceptable => POLLIN | POLLRDNORM,
            _ => 0,
        },
        SockState::Connecting(_) => 0,
        SockState::Connected(id) => {
            let Ok(r) = tcp::readiness(id) else { return POLLERR | POLLHUP };
            let shut_read = sock.shut_read.load(Ordering::Relaxed);
            let shut_write = sock.shut_write.load(Ordering::Relaxed);
            let mut ev = 0;
            if r.readable || r.hangup || shut_read {
                ev |= POLLIN | POLLRDNORM;
            }
            if r.hangup {
                ev |= POLLRDHUP;
            }
            if r.writable && !shut_write {
                ev |= POLLOUT | POLLWRNORM;
            }
            if r.hangup && shut_write {
                ev |= POLLHUP;
            }
            if r.refused {
                ev |= POLLERR | POLLHUP;
            }
            ev
        }
    }
}

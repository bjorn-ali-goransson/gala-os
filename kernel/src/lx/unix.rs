//! AF_UNIX sockets for the Linux personality (ROADMAP LD step 2: the
//! Wayland socket, D-Bus; user/lx/unix_test is the spec, recorded on
//! Linux): the Linux calls on the core's sockets (`crate::unix`, seam
//! 5), and sendmsg/recvmsg for every socket.
//!
//! - **socket** (SOCK_STREAM, SOCK_SEQPACKET, SOCK_DGRAM, and SOCK_RAW
//!   as a datagram socket, as Linux takes it; protocol 0 or PF_UNIX;
//!   SOCK_NONBLOCK, SOCK_CLOEXEC) and **socketpair** (each end's peer
//!   credentials the caller's).
//! - **Names**: a path (the last component made a socket node of the
//!   shared file system, mode 0777 less the umask, `RamFs::create_socket`;
//!   an existing name, even its own, EADDRINUSE; kept open while bound,
//!   left behind when the socket closes, as Linux leaves it; unlink
//!   removes it, and the bound socket keeps its name), an abstract name
//!   (a leading NUL: no file), or autobind (only the family: five hex
//!   digits). connect follows the path (ENOENT; a node that is not a
//!   socket, or one nothing listens on, ECONNREFUSED; another kind
//!   EPROTOTYPE); a full backlog (backlog + 1 waiting, as Linux) is
//!   EAGAIN, or blocks.
//! - **Data**: read/write, readv/writev, send/recv/sendto/recvfrom,
//!   sendmsg/recvmsg (several iovecs; MSG_PEEK, MSG_DONTWAIT,
//!   MSG_WAITALL, MSG_NOSIGNAL, MSG_TRUNC, MSG_CMSG_CLOEXEC). EPIPE on a
//!   stream or seqpacket raises SIGPIPE unless MSG_NOSIGNAL. A datagram
//!   to a pair whose peer went is ECONNREFUSED.
//! - **SCM_RIGHTS**: the fds named are checked (EBADF; more than 253
//!   EINVAL) and their open file descriptions travel with the message
//!   (`Right`: a `KArc<Fd>`), so the receiver's fds are the same open
//!   files (offset, status flags, a memfd's pages). recvmsg installs as
//!   many as its control buffer holds (MSG_CMSG_CLOEXEC: close-on-exec),
//!   the rest are closed and MSG_CTRUNC set; msg_controllen is the space
//!   used (at most what was given), as Linux reports it. A read without
//!   a control buffer closes them.
//! - **Options**: SO_TYPE, SO_DOMAIN, SO_PROTOCOL, SO_ACCEPTCONN,
//!   SO_ERROR (0), SO_PEERCRED (pid, uid, gid: the peer's, see
//!   `crate::unix::Cred`), SO_SNDBUF/SO_RCVBUF (kept, doubled, as Linux
//!   reports them; the queues stay `unix::CAPACITY`), SO_PASSCRED (kept;
//!   SCM_CREDENTIALS are not delivered: ledgered); others ENOPROTOOPT,
//!   ledgered.
//! - **Waiting**: on the core socket's waiters (`wait_on`); poll and
//!   epoll wait on them too (`poll::Watch`).
//!
//! Not yet (ledgered `linux option missing`): a datagram socket's
//! connect or sendto by name (only pairs carry datagrams), SCM_CREDENTIALS,
//! rights on a peek (the bytes are peeked, the rights stay queued).
//!
//! Observability (`events=ipc`): `lx unix` events for socketpair, bind,
//! listen, connect, accept and the close of a named or connected socket
//! (pid, fd, op, the name, errno), `lx unix rights` for each message
//! carrying descriptors (sent, received, dropped: how many); a refused
//! bind, listen or connect is in its event with its errno. The state
//! dump lists every named socket (`unix::dump`).

use alloc::vec::Vec;
use core::fmt;

use super::dispatch::{Call, Result};
use super::errno::{
    EADDRINUSE, EAGAIN, EBADF, ECONNREFUSED, EFAULT, EINVAL, EISCONN, EMSGSIZE, ENOBUFS, ENOENT,
    ENOMEM, ENOPROTOOPT, ENOTCONN, EOPNOTSUPP, EPIPE, EPROTONOSUPPORT, EPROTOTYPE, ESOCKTNOSUPPORT,
};
use super::file;
use super::sigcalls;
use super::state::{Fd, Object};
use super::wait;
use crate::fs::ramfs::{Access, FsError, NodeId, Share};
use crate::mm::heap::{KArc, TryVec, try_box};
use crate::nt::file::with_fs;
use crate::nt::{ledger, platform};
use crate::sync::IrqMutex;
use crate::unix::{self, Address, Cred, Error, Kind, Right, Socket, Target, Waiters};

pub const AF_UNIX: u64 = 1;
const SOCK_STREAM: u64 = 1;
const SOCK_DGRAM: u64 = 2;
const SOCK_RAW: u64 = 3;
const SOCK_SEQPACKET: u64 = 5;
const SOCK_NONBLOCK: u64 = 0o4000;
const SOCK_CLOEXEC: u64 = 0o2000000;
const SOL_SOCKET: u64 = 1;
const SO_TYPE: u64 = 3;
const SO_ERROR: u64 = 4;
const SO_SNDBUF: u64 = 7;
const SO_RCVBUF: u64 = 8;
const SO_PASSCRED: u64 = 16;
const SO_PEERCRED: u64 = 17;
const SO_RCVTIMEO: u64 = 20;
const SO_SNDTIMEO: u64 = 21;
const SO_ACCEPTCONN: u64 = 30;
const SO_PROTOCOL: u64 = 38;
const SO_DOMAIN: u64 = 39;
const SCM_RIGHTS: u32 = 1;
const SCM_CREDENTIALS: u32 = 2;
pub const MSG_OOB: u64 = 1;
pub const MSG_PEEK: u64 = 2;
pub const MSG_CTRUNC: u64 = 8;
pub const MSG_TRUNC: u64 = 0x20;
pub const MSG_DONTWAIT: u64 = 0x40;
pub const MSG_EOR: u64 = 0x80;
pub const MSG_WAITALL: u64 = 0x100;
pub const MSG_NOSIGNAL: u64 = 0x4000;
pub const MSG_CMSG_CLOEXEC: u64 = 0x4000_0000;
/// sizeof(struct sockaddr_un): the family and 108 bytes of path.
const ADDR_MAX: u64 = 110;
/// How many bytes one stream call moves through the kernel at a time.
const CHUNK: usize = 64 * 1024;
/// The control data a sendmsg may carry (Linux's optmem_max).
const CONTROL_MAX: u64 = 20480;
/// sendmsg/recvmsg's iovcnt limit (UIO_MAXIOV).
const IOV_MAX: u64 = 1024;
/// Linux caps one read or write at this many bytes (MAX_RW_COUNT).
const MAX_RW: u64 = 0x7fff_f000;

/// The options kept per socket (see the module's doc).
#[derive(Default, Clone, Copy)]
struct Options {
    passcred: u32,
    sndbuf: Option<u32>,
    rcvbuf: Option<u32>,
    rcvtimeo: [u8; 16],
    sndtimeo: [u8; 16],
}

/// A Linux view of a core socket (`Object::Unix`): its options.
pub struct UnixSocket {
    pub core: KArc<Socket>,
    options: IrqMutex<Options>,
}

impl UnixSocket {
    pub fn new(core: KArc<Socket>) -> core::result::Result<KArc<UnixSocket>, u16> {
        KArc::try_new(UnixSocket { core, options: IrqMutex::new(Options::default()) })
            .map_err(|_| ENOMEM)
    }

    /// The description's last close (`Fd`'s drop): the core socket is
    /// released, its node's hold closed.
    pub fn release(&self) {
        let name = if self.core.is_bound() { self.core.address().ok().flatten() } else { None };
        if let Some(node) = self.core.release() {
            with_fs(|fs| fs.close(node, Access::default(), Share::default()));
        }
        if let Some(name) = name {
            event(self.core.cred.pid, None, "close", &name, 0);
        }
    }
}

fn errno_of(e: &Error) -> u16 {
    match e {
        Error::WouldBlock | Error::Full(_) => EAGAIN,
        Error::Invalid => EINVAL,
        Error::NotConnected => ENOTCONN,
        Error::IsConnected => EISCONN,
        Error::AddressInUse => EADDRINUSE,
        Error::Refused => ECONNREFUSED,
        Error::Pipe => EPIPE,
        Error::WrongType => EPROTOTYPE,
        Error::TooLong => EMSGSIZE,
        Error::NoBuffers => ENOBUFS,
        Error::NotSupported => EOPNOTSUPP,
        Error::NoMemory => ENOMEM,
    }
}

/// The calling process as a socket's credentials see it.
fn cred(call: &Call<'_>) -> Cred {
    let ids = super::identity::of_process(&call.process);
    Cred { pid: call.process.id, uid: ids.uid, gid: ids.gid }
}

/// A name as events show it: a path as is, an abstract name after `@`.
struct Name<'a>(&'a [u8]);

impl fmt::Display for Name<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (prefix, bytes) = match self.0.split_first() {
            Some((0, rest)) => ("@", rest),
            _ => ("", self.0.strip_suffix(&[0]).unwrap_or(self.0)),
        };
        f.write_str(prefix)?;
        // Inside a JSON string: other bytes, quotes and backslashes as
        // \u escapes.
        for &b in bytes {
            if (b.is_ascii_graphic() || b == b' ') && b != b'"' && b != b'\\' {
                fmt::Write::write_char(f, b as char)?;
            } else {
                write!(f, "\\u{:04x}", b)?;
            }
        }
        Ok(())
    }
}

/// An `lx unix` event (`events=ipc`).
fn event(pid: u64, fd: Option<u64>, op: &str, name: &[u8], errno: u16) {
    crate::events::emit(
        crate::events::Category::Ipc,
        "lx unix",
        format_args!(
            r#""pid":{pid},"fd":{},"op":"{op}","name":"{}","errno":{errno}"#,
            fd.map_or(-1, |f| f as i64),
            Name(name)
        ),
    );
}

fn rights_event(call: &Call<'_>, fd: u64, what: &str, count: usize) {
    crate::events::emit(
        crate::events::Category::Ipc,
        "lx unix rights",
        format_args!(r#""pid":{},"fd":{fd},"what":"{what}","count":{count}"#, call.process.id),
    );
}

/// The unix socket behind `fd`, if it is one.
pub fn of(call: &Call<'_>, fd: u64) -> Option<(KArc<Fd>, KArc<UnixSocket>)> {
    let f = call.lx.fd(fd)?;
    let Object::Unix(u) = &f.object else { return None };
    let u = u.clone();
    Some((f, u))
}

fn install(call: &Call<'_>, sock: KArc<Socket>, flags: u64) -> Result {
    let u = UnixSocket::new(sock)?;
    let fd =
        KArc::try_new(Fd::new(Object::Unix(u), flags & SOCK_NONBLOCK != 0)).map_err(|_| ENOMEM)?;
    call.lx.install_from(fd, flags & SOCK_CLOEXEC != 0, 0)
}

/// The kind a socket(2) type asks for.
fn kind_of(base: u64) -> core::result::Result<Kind, u16> {
    match base {
        SOCK_STREAM => Ok(Kind::Stream),
        // SOCK_RAW is a datagram socket for AF_UNIX (unix_test on Linux).
        SOCK_DGRAM | SOCK_RAW => Ok(Kind::Dgram),
        SOCK_SEQPACKET => Ok(Kind::SeqPacket),
        _ => Err(ESOCKTNOSUPPORT),
    }
}

fn check_type(kind: u64, protocol: u64) -> core::result::Result<(Kind, u64), u16> {
    let flags = kind & (SOCK_NONBLOCK | SOCK_CLOEXEC);
    let kind = kind_of(kind & !(SOCK_NONBLOCK | SOCK_CLOEXEC))?;
    // 0, or PF_UNIX itself.
    if protocol != 0 && protocol != AF_UNIX {
        return Err(EPROTONOSUPPORT);
    }
    Ok((kind, flags))
}

/// socket(AF_UNIX, ...).
pub fn socket(call: &Call<'_>, kind: u64, protocol: u64) -> Result {
    let (kind, flags) = check_type(kind, protocol)?;
    let sock = Socket::new(kind, cred(call)).map_err(|_| ENOMEM)?;
    install(call, sock, flags)
}

/// socketpair(2): AF_UNIX only (another family EOPNOTSUPP, as Linux).
pub fn socketpair(call: &Call<'_>, domain: u64, kind: u64, protocol: u64, sv: u64) -> Result {
    if domain != AF_UNIX {
        if domain == 2 || domain == 10 {
            return Err(EOPNOTSUPP);
        }
        return Err(super::errno::EAFNOSUPPORT);
    }
    let (kind, flags) = check_type(kind, protocol)?;
    let (a, b) = Socket::pair(kind, cred(call)).map_err(|_| ENOMEM)?;
    let fa = install(call, a, flags)?;
    let fb = match install(call, b, flags) {
        Ok(fb) => fb,
        Err(e) => {
            call.lx.close(fa);
            return Err(e);
        }
    };
    let mut out = [0u8; 8];
    out[..4].copy_from_slice(&(fa as u32).to_le_bytes());
    out[4..].copy_from_slice(&(fb as u32).to_le_bytes());
    if call.process.write(sv, &out).is_err() {
        call.lx.close(fa);
        call.lx.close(fb);
        return Err(EFAULT);
    }
    event(call.process.id, Some(fa), "socketpair", &[], 0);
    Ok(0)
}

/// A sockaddr_un from user memory.
enum Parsed {
    /// Only the family (bind: autobind).
    Unnamed,
    /// A path, without its NUL.
    Path(Vec<u8>),
    /// An abstract name, its leading NUL included.
    Abstract(Vec<u8>),
}

fn read_addr(call: &Call<'_>, addr: u64, len: u64) -> core::result::Result<Parsed, u16> {
    let len = u64::from(len as u32);
    if !(2..=ADDR_MAX).contains(&len) {
        return Err(EINVAL);
    }
    let mut b = [0u8; ADDR_MAX as usize];
    call.process.read(addr, &mut b[..len as usize]).map_err(|_| EFAULT)?;
    if u16::from_le_bytes([b[0], b[1]]) != AF_UNIX as u16 {
        return Err(EINVAL);
    }
    let path = &b[2..len as usize];
    let mut v = Vec::new();
    match path.first() {
        None => Ok(Parsed::Unnamed),
        Some(0) => {
            v.try_extend_from_slice(path).map_err(|_| ENOMEM)?;
            Ok(Parsed::Abstract(v))
        }
        Some(_) => {
            let end = path.iter().position(|&c| c == 0).unwrap_or(path.len());
            v.try_extend_from_slice(&path[..end]).map_err(|_| ENOMEM)?;
            Ok(Parsed::Path(v))
        }
    }
}

/// Writes a name (`bytes`: sun_path as reported; None: unnamed, the
/// family alone) at `addr`, at most `*len` bytes, and its whole length
/// at `len` (getsockname, getpeername, accept, recvfrom).
fn write_addr(
    call: &Call<'_>,
    bytes: Option<&[u8]>,
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
    let mut b = [0u8; ADDR_MAX as usize];
    b[..2].copy_from_slice(&(AF_UNIX as u16).to_le_bytes());
    let name = bytes.unwrap_or(&[]);
    let size = 2 + name.len().min(108);
    b[2..size].copy_from_slice(&name[..size - 2]);
    let n = (room as usize).min(size);
    call.process.write(addr, &b[..n]).map_err(|_| EFAULT)?;
    call.process.write(len, &(size as u32).to_le_bytes()).map_err(|_| EFAULT)?;
    Ok(())
}

pub fn bind(call: &Call<'_>, fd: u64, u: &UnixSocket, addr: u64, len: u64) -> Result {
    let parsed = read_addr(call, addr, len)?;
    let result = match &parsed {
        Parsed::Unnamed => Socket::autobind(&u.core).map_err(|e| errno_of(&e)),
        Parsed::Abstract(name) => {
            let mut bytes = Vec::new();
            bytes.try_extend_from_slice(name).map_err(|_| ENOMEM)?;
            Socket::bind(&u.core, Address { bytes, node: None }).map_err(|e| errno_of(&e))
        }
        Parsed::Path(path) => bind_path(call, u, path),
    };
    // The name asked for, or the one autobind chose.
    let chosen = u.core.address().ok().flatten();
    let name = match &parsed {
        Parsed::Unnamed => chosen.as_deref().unwrap_or(&[]),
        Parsed::Abstract(n) | Parsed::Path(n) => n.as_slice(),
    };
    event(call.process.id, Some(fd), "bind", name, result.err().unwrap_or(0));
    result.map(|()| 0)
}

/// Binds to a new socket node at `path` (see the module's doc).
fn bind_path(call: &Call<'_>, u: &UnixSocket, path: &[u8]) -> core::result::Result<(), u16> {
    let found = file::resolve(call, file::AT_FDCWD, path, false)?;
    if found.node.is_some() {
        return Err(EADDRINUSE);
    }
    let name = found.name.as_deref().ok_or(EADDRINUSE)?;
    let mode = 0o777 & !(*call.lx.umask.lock() as u16);
    let now = platform::system_time();
    let dir = found.dir;
    let made = with_fs(|fs| -> core::result::Result<NodeId, FsError> {
        let node = fs.create_socket(dir, name, mode, now)?;
        // Held open while bound: its id is not reused (the core's key).
        if let Err(e) = fs.open(node, Access::default(), Share::default()) {
            let _ = fs.unlink(dir, name);
            return Err(e);
        }
        Ok(node)
    });
    let node = match made {
        Ok(node) => node,
        Err(FsError::Exists) => return Err(EADDRINUSE),
        Err(FsError::NoMemory) => return Err(ENOMEM),
        Err(FsError::NotFound | FsError::PathNotFound) => return Err(ENOENT),
        Err(_) => return Err(EINVAL),
    };
    let mut bytes = Vec::new();
    let copied = bytes.try_extend_from_slice(path).and_then(|()| bytes.try_push(0));
    let bound = match copied {
        Ok(()) => Socket::bind(&u.core, Address { bytes, node: Some(node) }),
        Err(_) => Err(Error::NoMemory),
    };
    if let Err(e) = bound {
        // Bound already (Linux made the node first too, then refused):
        // the node goes again.
        with_fs(|fs| {
            let _ = fs.unlink(dir, name);
            fs.close(node, Access::default(), Share::default());
        });
        return Err(errno_of(&e));
    }
    Ok(())
}

/// /proc/sys/net/core/somaxconn's default: the most a backlog asks for.
const SOMAXCONN: i32 = 4096;

pub fn listen(call: &Call<'_>, fd: u64, u: &UnixSocket, backlog: u64) -> Result {
    let backlog = match backlog as i32 {
        b @ 0..=SOMAXCONN => b as usize,
        _ => SOMAXCONN as usize,
    };
    let result = u.core.listen(backlog, cred(call)).map_err(|e| errno_of(&e));
    let name = u.core.address().ok().flatten();
    event(
        call.process.id,
        Some(fd),
        "listen",
        name.as_deref().unwrap_or(&[]),
        result.err().unwrap_or(0),
    );
    result.map(|()| 0)
}

/// Runs `attempt` until it is done, waiting on the socket's waiters while
/// it would block (or a full listener's, `Error::Full`): EAGAIN without
/// waiting when `nonblock`; EINTR when a signal is to be delivered.
fn wait_on<T>(
    call: &Call<'_>,
    nonblock: bool,
    waiters: &KArc<Waiters>,
    mut attempt: impl FnMut() -> core::result::Result<T, Error>,
) -> core::result::Result<T, u16> {
    let me = platform::current();
    loop {
        let on = match attempt() {
            Ok(v) => return Ok(v),
            Err(Error::WouldBlock) => waiters.clone(),
            Err(Error::Full(w)) => w,
            Err(e) => return Err(errno_of(&e)),
        };
        if nonblock {
            return Err(EAGAIN);
        }
        on.add(me).map_err(|_| ENOMEM)?;
        // From here a change wakes the thread: look once more, then block.
        wait::prepare(call, None);
        match attempt() {
            Err(Error::WouldBlock | Error::Full(_)) => {}
            r => {
                wait::abandon();
                on.remove(me);
                return r.map_err(|e| errno_of(&e));
            }
        }
        let r = wait::block(call);
        on.remove(me);
        r?;
    }
}

pub fn accept4(
    call: &Call<'_>,
    fd: u64,
    f: &Fd,
    u: &UnixSocket,
    addr: u64,
    len: u64,
    flags: u64,
) -> Result {
    if flags & !(SOCK_NONBLOCK | SOCK_CLOEXEC) != 0 {
        return Err(EINVAL);
    }
    let sock = wait_on(call, f.nonblock(), &u.core.waiters, || u.core.accept())?;
    let peer = match sock.peer_address() {
        Ok(p) => p,
        Err(e) => return Err(errno_of(&e)),
    };
    let peer_pid = sock.peer_cred().map_or(0, |c| c.pid);
    // From here on dropping it releases the connection.
    let release = sock.clone();
    let newfd = match install(call, sock, flags) {
        Ok(n) => n,
        Err(e) => {
            release.release();
            return Err(e);
        }
    };
    if let Err(e) = write_addr(call, peer.as_deref(), addr, len) {
        call.lx.close(newfd);
        return Err(e);
    }
    crate::events::emit(
        crate::events::Category::Ipc,
        "lx unix",
        format_args!(
            r#""pid":{},"fd":{fd},"op":"accept","new_fd":{newfd},"peer_pid":{peer_pid},"errno":0"#,
            call.process.id
        ),
    );
    Ok(newfd)
}

pub fn connect(call: &Call<'_>, fd: u64, f: &Fd, u: &UnixSocket, addr: u64, len: u64) -> Result {
    let parsed = read_addr(call, addr, len)?;
    let result = connect_to(call, f, u, &parsed);
    let name = match &parsed {
        Parsed::Unnamed => &[][..],
        Parsed::Abstract(n) | Parsed::Path(n) => n.as_slice(),
    };
    if result != Err(EAGAIN) {
        event(call.process.id, Some(fd), "connect", name, result.err().unwrap_or(0));
    }
    result.map(|()| 0)
}

fn connect_to(
    call: &Call<'_>,
    f: &Fd,
    u: &UnixSocket,
    parsed: &Parsed,
) -> core::result::Result<(), u16> {
    let node;
    let target = match parsed {
        Parsed::Unnamed => return Err(EINVAL),
        Parsed::Abstract(name) => Target::Abstract(name),
        Parsed::Path(path) => {
            node = file::resolve(call, file::AT_FDCWD, path, true)?.existing()?;
            if !with_fs(|fs| fs.node(node).is_some_and(crate::fs::ramfs::Node::is_socket)) {
                return Err(ECONNREFUSED);
            }
            Target::Node(node)
        }
    };
    if u.core.kind == Kind::Dgram {
        // A datagram socket's default peer by name: not yet.
        ledger::record("connect", "linux option missing", SOCK_DGRAM);
        return Err(EOPNOTSUPP);
    }
    let me = cred(call);
    wait_on(call, f.nonblock(), &u.core.waiters, || u.core.connect(target, me))
}

/// User memory as the pieces of an iovec array: `fetch(at, out)` copies
/// the bytes from offset `at` on.
struct Pieces(Vec<(u64, u64)>);

impl Pieces {
    /// Reads `count` iovecs at `iov`; EINVAL past IOV_MAX or a total
    /// beyond ssize_t.
    fn read(call: &Call<'_>, iov: u64, count: u64) -> core::result::Result<(Pieces, u64), u16> {
        if count > IOV_MAX {
            return Err(EINVAL);
        }
        let mut pieces = Vec::new();
        let mut total = 0u64;
        for i in 0..count {
            let base = call.process.read_u64(iov + i * 16).map_err(|_| EFAULT)?;
            let len = call.process.read_u64(iov + i * 16 + 8).map_err(|_| EFAULT)?;
            total = total.checked_add(len).filter(|&t| t <= i64::MAX as u64).ok_or(EINVAL)?;
            if len > 0 {
                pieces.try_push((base, len)).map_err(|_| ENOMEM)?;
            }
        }
        Ok((Pieces(pieces), total.min(MAX_RW)))
    }

    fn one(base: u64, len: u64) -> core::result::Result<Pieces, u16> {
        let mut v = Vec::new();
        v.try_push((base, len)).map_err(|_| ENOMEM)?;
        Ok(Pieces(v))
    }

    /// Copies user bytes from offset `at` into `out`.
    fn fetch(&self, call: &Call<'_>, mut at: u64, out: &mut [u8]) -> core::result::Result<(), u16> {
        let mut done = 0usize;
        for &(base, len) in &self.0 {
            if done == out.len() {
                break;
            }
            if at >= len {
                at -= len;
                continue;
            }
            let k = ((len - at) as usize).min(out.len() - done);
            call.process.read(base + at, &mut out[done..done + k]).map_err(|_| EFAULT)?;
            done += k;
            at = 0;
        }
        Ok(())
    }

    /// Copies `data` out over the pieces from their start.
    fn scatter(&self, call: &Call<'_>, data: &[u8]) -> core::result::Result<(), u16> {
        let mut done = 0usize;
        for &(base, len) in &self.0 {
            if done == data.len() {
                break;
            }
            let k = (len as usize).min(data.len() - done);
            call.process.write(base, &data[done..done + k]).map_err(|_| EFAULT)?;
            done += k;
        }
        Ok(())
    }
}

/// Sends `count` bytes fetched by `fetch` and `rights` (taken with the
/// first bytes queued): a stream piece by piece, blocking for room unless
/// non-blocking (then what fits, or EAGAIN); a message whole. EPIPE
/// raises SIGPIPE unless MSG_NOSIGNAL.
fn send_from(
    call: &Call<'_>,
    f: &Fd,
    u: &UnixSocket,
    count: usize,
    flags: u64,
    rights: &mut Vec<Right>,
    mut fetch: impl FnMut(usize, &mut [u8]) -> core::result::Result<(), u16>,
) -> Result {
    let sock = &u.core;
    let nonblock = f.nonblock() || flags & MSG_DONTWAIT != 0;
    let partial = |done: usize, e: u16| if done > 0 { Ok(done as u64) } else { Err(e) };
    let stream = sock.kind == Kind::Stream;
    if !stream && count + unix::RECORD_OVERHEAD > unix::CAPACITY {
        return Err(EMSGSIZE);
    }
    let purse = call.process.quota.clone();
    let mut done = 0usize;
    let mut chunk = Vec::new();
    loop {
        let n = if stream { (count - done).min(CHUNK) } else { count };
        chunk.clear();
        if chunk.try_reserve_exact(n).is_err() {
            return partial(done, ENOMEM);
        }
        chunk.resize(n, 0);
        if let Err(e) = fetch(done, &mut chunk) {
            return partial(done, e);
        }
        let mut at = 0;
        loop {
            let sent = wait_on(call, nonblock, &sock.waiters, || {
                sock.send(&chunk[at..], rights, Some(&purse))
            });
            match sent {
                Ok(k) => {
                    at += k;
                    done += k;
                }
                Err(EPIPE) => {
                    if flags & MSG_NOSIGNAL == 0 {
                        sigcalls::sigpipe(call);
                    }
                    return partial(done, EPIPE);
                }
                Err(e) => return partial(done, e),
            }
            if at >= n || !stream {
                break;
            }
        }
        if done >= count || !stream {
            return Ok(done as u64);
        }
    }
}

/// What a receive took: the bytes, the message's whole length, rights.
struct Got {
    data: Vec<u8>,
    full: usize,
    rights: Vec<Right>,
}

/// Receives up to `want` bytes (see `unix::Socket::recv`), blocking
/// unless non-blocking; MSG_WAITALL on a stream goes on until `want`
/// bytes, the end, rights or a signal.
fn recv_got(
    call: &Call<'_>,
    f: &Fd,
    u: &UnixSocket,
    want: usize,
    flags: u64,
) -> core::result::Result<Got, u16> {
    let sock = &u.core;
    let nonblock = f.nonblock() || flags & MSG_DONTWAIT != 0;
    let peek = flags & MSG_PEEK != 0;
    let cap = want.min(if sock.kind == Kind::Stream { 4 * CHUNK } else { unix::CAPACITY });
    let mut data = Vec::new();
    data.try_reserve_exact(cap).map_err(|_| ENOMEM)?;
    data.resize(cap, 0);
    let mut len = 0usize;
    let mut full = 0usize;
    let mut rights = Vec::new();
    loop {
        let got = wait_on(call, nonblock, &sock.waiters, || sock.recv(&mut data[len..], peek));
        match got {
            Ok(r) => {
                let ended = r.len == 0;
                len += r.len;
                full = if sock.kind == Kind::Stream { len } else { r.full };
                let carried = !r.rights.is_empty();
                rights = r.rights;
                let more = sock.kind == Kind::Stream
                    && flags & MSG_WAITALL != 0
                    && !peek
                    && !ended
                    && !carried
                    && len < cap;
                if !more {
                    break;
                }
            }
            Err(_) if len > 0 => break,
            Err(e) => return Err(e),
        }
    }
    data.truncate(len);
    Ok(Got { data, full, rights })
}

/// read(2) and recv/recvfrom: rights that came are closed.
pub fn recv(
    call: &Call<'_>,
    fd: u64,
    f: &Fd,
    u: &UnixSocket,
    buf: u64,
    count: u64,
    flags: u64,
) -> Result {
    let got = recv_got(call, f, u, count.min(MAX_RW) as usize, flags)?;
    if !got.rights.is_empty() {
        rights_event(call, fd, "dropped", got.rights.len());
    }
    drop(got.rights);
    call.process.write(buf, &got.data).map_err(|_| EFAULT)?;
    if flags & MSG_TRUNC != 0 && u.core.kind != Kind::Stream {
        return Ok(got.full as u64);
    }
    Ok(got.data.len() as u64)
}

pub fn recvfrom(
    call: &Call<'_>,
    fd: u64,
    buf: u64,
    len: u64,
    flags: u64,
    addr: u64,
    addrlen: u64,
) -> Result {
    let (f, u) = of(call, fd).ok_or(super::errno::ENOTSOCK)?;
    let n = recv(call, fd, &f, &u, buf, len, flags)?;
    if addr != 0 {
        let peer = u.core.peer_address().ok().flatten();
        write_addr(call, peer.as_deref(), addr, addrlen)?;
    }
    Ok(n)
}

/// write(2) and send/sendto (an address on a connected socket: EISCONN;
/// a datagram's destination by name: not yet).
pub fn send(
    call: &Call<'_>,
    f: &Fd,
    u: &UnixSocket,
    buf: u64,
    count: u64,
    flags: u64,
    addr: u64,
) -> Result {
    if addr != 0 {
        return Err(if u.core.kind == Kind::Dgram {
            ledger::record("sendto", "linux option missing", SOCK_DGRAM);
            EOPNOTSUPP
        } else if u.core.peer_address().is_ok() {
            EISCONN
        } else {
            EOPNOTSUPP
        });
    }
    let pieces = Pieces::one(buf, count)?;
    let mut rights = Vec::new();
    send_from(call, f, u, count.min(MAX_RW) as usize, flags, &mut rights, |at, out| {
        pieces.fetch(call, at as u64, out)
    })
}

/// Kernel bytes written to a socket (writev's gathered pieces).
pub fn send_bytes(call: &Call<'_>, f: &Fd, u: &UnixSocket, data: &[u8]) -> Result {
    let mut rights = Vec::new();
    send_from(call, f, u, data.len(), 0, &mut rights, |at, out| {
        out.copy_from_slice(&data[at..at + out.len()]);
        Ok(())
    })
}

/// struct msghdr as user memory holds it (56 bytes).
struct MsgHdr {
    name: u64,
    namelen: u32,
    iov: u64,
    iovlen: u64,
    control: u64,
    controllen: u64,
}

fn read_msghdr(call: &Call<'_>, msg: u64) -> core::result::Result<MsgHdr, u16> {
    let mut b = [0u8; 56];
    call.process.read(msg, &mut b).map_err(|_| EFAULT)?;
    let q = |at: usize| u64::from_le_bytes(b[at..at + 8].try_into().unwrap_or([0; 8]));
    Ok(MsgHdr {
        name: q(0),
        namelen: u32::from_le_bytes([b[8], b[9], b[10], b[11]]),
        iov: q(16),
        iovlen: q(24),
        control: q(32),
        controllen: q(40),
    })
}

/// The rights a sendmsg's control data names (SCM_RIGHTS: each fd's open
/// file description).
fn read_rights(call: &Call<'_>, m: &MsgHdr) -> core::result::Result<Vec<Right>, u16> {
    let mut rights: Vec<Right> = Vec::new();
    if m.control == 0 || m.controllen == 0 {
        return Ok(rights);
    }
    if m.controllen > CONTROL_MAX {
        return Err(ENOBUFS);
    }
    let mut control = Vec::new();
    control.try_reserve_exact(m.controllen as usize).map_err(|_| ENOMEM)?;
    control.resize(m.controllen as usize, 0);
    call.process.read(m.control, &mut control).map_err(|_| EFAULT)?;
    let mut at = 0usize;
    while at + 16 <= control.len() {
        let h = &control[at..];
        let len = u64::from_le_bytes(h[..8].try_into().unwrap_or([0; 8])) as usize;
        let level = u32::from_le_bytes([h[8], h[9], h[10], h[11]]);
        let kind = u32::from_le_bytes([h[12], h[13], h[14], h[15]]);
        if len < 16 || len > control.len() - at {
            return Err(EINVAL);
        }
        match (u64::from(level), kind) {
            (SOL_SOCKET, SCM_RIGHTS) => {
                for raw in control[at + 16..at + len].chunks_exact(4) {
                    let n = u64::from(u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]));
                    let fd = call.lx.fd(n).ok_or(EBADF)?;
                    if rights.len() >= unix::MAX_RIGHTS {
                        return Err(EINVAL);
                    }
                    let boxed = try_box(fd).map_err(|_| ENOMEM)?;
                    rights.try_push(boxed).map_err(|_| ENOMEM)?;
                }
            }
            (SOL_SOCKET, SCM_CREDENTIALS) => {
                // Checked and delivered by Linux (SO_PASSCRED): not yet.
                ledger::record("sendmsg", "linux option missing", u64::from(SCM_CREDENTIALS));
            }
            _ => return Err(EINVAL),
        }
        at += len.next_multiple_of(8);
    }
    Ok(rights)
}

/// sendmsg(2) on any socket: a unix socket's rights travel with the
/// bytes; a TCP socket's control data is refused (EINVAL, ledgered).
pub fn sendmsg(call: &Call<'_>, fd: u64, msg: u64, flags: u64) -> Result {
    let f = call.lx.fd(fd).ok_or(EBADF)?;
    let m = read_msghdr(call, msg)?;
    let (pieces, total) = Pieces::read(call, m.iov, m.iovlen)?;
    match &f.object {
        Object::Unix(u) => {
            if m.name != 0 && m.namelen != 0 {
                return send(call, &f, u, 0, 0, flags, m.name);
            }
            let mut rights = read_rights(call, &m)?;
            let carried = rights.len();
            let r = send_from(call, &f, u, total as usize, flags, &mut rights, |at, out| {
                pieces.fetch(call, at as u64, out)
            });
            if carried > 0 && r.is_ok() && rights.is_empty() {
                rights_event(call, fd, "sent", carried);
            }
            r
        }
        Object::Socket(sock) => {
            if m.control != 0 && m.controllen != 0 {
                ledger::record("sendmsg", "linux option missing", 2);
                return Err(EINVAL);
            }
            super::socket::send(call, &f, sock, total as usize, flags, |at, out| {
                pieces.fetch(call, at as u64, out)
            })
        }
        _ => Err(super::errno::ENOTSOCK),
    }
}

/// recvmsg(2) on any socket (see the module's doc for the control data).
pub fn recvmsg(call: &Call<'_>, fd: u64, msg: u64, flags: u64) -> Result {
    let f = call.lx.fd(fd).ok_or(EBADF)?;
    let m = read_msghdr(call, msg)?;
    let (pieces, total) = Pieces::read(call, m.iov, m.iovlen)?;
    let (got, peer, message) = match &f.object {
        Object::Unix(u) => {
            let got = recv_got(call, &f, u, total as usize, flags)?;
            let peer = u.core.peer_address().ok().flatten();
            (got, peer, u.core.kind != Kind::Stream)
        }
        Object::Socket(sock) => {
            let data = super::socket::recv_bytes(call, &f, sock, total, flags)?;
            let full = data.len();
            (Got { data, full, rights: Vec::new() }, None, false)
        }
        _ => return Err(super::errno::ENOTSOCK),
    };
    pieces.scatter(call, &got.data)?;
    let mut out_flags = flags & MSG_CMSG_CLOEXEC;
    if message && got.full > got.data.len() {
        out_flags |= MSG_TRUNC;
    }
    // The rights: as many as the control buffer holds become fds.
    let offered = got.rights.len();
    let mut fds: Vec<u32> = Vec::new();
    let mut controllen = 0u64;
    if offered > 0 {
        let room = if m.control == 0 { 0 } else { m.controllen };
        let max = if room >= 16 { ((room - 16) / 4) as usize } else { 0 };
        let cloexec = flags & MSG_CMSG_CLOEXEC != 0;
        for right in got.rights {
            if fds.len() >= max || fds.try_reserve(1).is_err() {
                // Closed (dropped here, no lock held).
                continue;
            }
            // Out of fds (EMFILE), it is closed as the rest.
            if let Ok(desc) = right.downcast::<KArc<Fd>>()
                && let Ok(n) = call.lx.install_from(*desc, cloexec, 0)
            {
                let _ = fds.try_push(n as u32);
            }
        }
        if fds.len() < offered {
            out_flags |= MSG_CTRUNC;
        }
        if !fds.is_empty() {
            let cmsg_len = 16 + 4 * fds.len();
            let mut c = Vec::new();
            c.try_reserve_exact(cmsg_len).map_err(|_| ENOMEM)?;
            c.try_extend_from_slice(&(cmsg_len as u64).to_le_bytes()).map_err(|_| ENOMEM)?;
            c.try_extend_from_slice(&(SOL_SOCKET as u32).to_le_bytes()).map_err(|_| ENOMEM)?;
            c.try_extend_from_slice(&SCM_RIGHTS.to_le_bytes()).map_err(|_| ENOMEM)?;
            for n in &fds {
                c.try_extend_from_slice(&n.to_le_bytes()).map_err(|_| ENOMEM)?;
            }
            if call.process.write(m.control, &c).is_err() {
                for &n in &fds {
                    call.lx.close(u64::from(n));
                }
                return Err(EFAULT);
            }
            // CMSG_SPACE, at most what was given (unix_test on Linux).
            controllen = (cmsg_len as u64).next_multiple_of(8).min(room);
        }
        rights_event(call, fd, "received", fds.len());
        if fds.len() < offered {
            rights_event(call, fd, "dropped", offered - fds.len());
        }
    }
    let namelen = match (&peer, m.name) {
        (Some(p), name) if name != 0 => {
            let size = (2 + p.len()).min(ADDR_MAX as usize);
            let mut b = [0u8; ADDR_MAX as usize];
            b[..2].copy_from_slice(&(AF_UNIX as u16).to_le_bytes());
            b[2..size].copy_from_slice(&p[..size - 2]);
            let n = size.min(m.namelen as usize);
            call.process.write(name, &b[..n]).map_err(|_| EFAULT)?;
            size as u32
        }
        _ => 0,
    };
    call.process.write(msg + 8, &namelen.to_le_bytes()).map_err(|_| EFAULT)?;
    call.process.write(msg + 40, &controllen.to_le_bytes()).map_err(|_| EFAULT)?;
    call.process.write(msg + 48, &(out_flags as u32).to_le_bytes()).map_err(|_| EFAULT)?;
    if flags & MSG_TRUNC != 0 && message {
        return Ok(got.full as u64);
    }
    Ok(got.data.len() as u64)
}

pub fn shutdown(u: &UnixSocket, how: u64) -> Result {
    let flags = match how {
        0 => unix::RCV,
        1 => unix::SEND,
        2 => unix::BOTH,
        _ => return Err(EINVAL),
    };
    u.core.shutdown(flags);
    Ok(0)
}

pub fn getsockname(call: &Call<'_>, u: &UnixSocket, addr: u64, len: u64) -> Result {
    let name = u.core.address().map_err(|_| ENOMEM)?;
    write_addr(call, name.as_deref(), addr, len)?;
    Ok(0)
}

pub fn getpeername(call: &Call<'_>, u: &UnixSocket, addr: u64, len: u64) -> Result {
    let name = u.core.peer_address().map_err(|e| errno_of(&e))?;
    write_addr(call, name.as_deref(), addr, len)?;
    Ok(0)
}

pub fn getsockopt(
    call: &Call<'_>,
    u: &UnixSocket,
    level: u64,
    name: u64,
    val: u64,
    len: u64,
) -> Result {
    let mut lb = [0u8; 4];
    call.process.read(len, &mut lb).map_err(|_| EFAULT)?;
    let room = u32::from_le_bytes(lb) as i32;
    if room < 0 {
        return Err(EINVAL);
    }
    let o = *u.options.lock();
    let mut b = [0u8; 16];
    let int = |b: &mut [u8; 16], v: u32| {
        b[..4].copy_from_slice(&v.to_le_bytes());
        4usize
    };
    let size = match (level, name) {
        (SOL_SOCKET, SO_TYPE) => int(&mut b, u.core.kind as u32),
        (SOL_SOCKET, SO_DOMAIN) => int(&mut b, AF_UNIX as u32),
        (SOL_SOCKET, SO_PROTOCOL) => int(&mut b, 0),
        (SOL_SOCKET, SO_ERROR) => int(&mut b, 0),
        (SOL_SOCKET, SO_ACCEPTCONN) => int(&mut b, u32::from(u.core.is_listening())),
        (SOL_SOCKET, SO_PASSCRED) => int(&mut b, o.passcred),
        (SOL_SOCKET, SO_SNDBUF) => int(&mut b, o.sndbuf.unwrap_or(unix::CAPACITY as u32)),
        (SOL_SOCKET, SO_RCVBUF) => int(&mut b, o.rcvbuf.unwrap_or(unix::CAPACITY as u32)),
        (SOL_SOCKET, SO_RCVTIMEO) => {
            b = o.rcvtimeo;
            16
        }
        (SOL_SOCKET, SO_SNDTIMEO) => {
            b = o.sndtimeo;
            16
        }
        (SOL_SOCKET, SO_PEERCRED) => {
            // Not connected: Linux's "nobody" (pid 0, ids -1).
            let c = u.core.peer_cred().unwrap_or(Cred { pid: 0, uid: u32::MAX, gid: u32::MAX });
            b[..4].copy_from_slice(&(c.pid as u32).to_le_bytes());
            b[4..8].copy_from_slice(&c.uid.to_le_bytes());
            b[8..12].copy_from_slice(&c.gid.to_le_bytes());
            12
        }
        _ => {
            ledger::record("getsockopt", "linux option missing", level << 32 | name);
            return Err(ENOPROTOOPT);
        }
    };
    let n = (room as usize).min(size);
    call.process.write(val, &b[..n]).map_err(|_| EFAULT)?;
    call.process.write(len, &(n as u32).to_le_bytes()).map_err(|_| EFAULT)?;
    Ok(0)
}

pub fn setsockopt(
    call: &Call<'_>,
    u: &UnixSocket,
    level: u64,
    name: u64,
    val: u64,
    len: u64,
) -> Result {
    let len = u64::from(len as u32) as usize;
    let need = match (level, name) {
        (SOL_SOCKET, SO_RCVTIMEO | SO_SNDTIMEO) => 16,
        _ => 4,
    };
    if len < need {
        return Err(EINVAL);
    }
    let mut b = [0u8; 16];
    call.process.read(val, &mut b[..need]).map_err(|_| EFAULT)?;
    let int = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
    let mut o = u.options.lock();
    match (level, name) {
        (SOL_SOCKET, SO_PASSCRED) => {
            o.passcred = u32::from(int != 0);
            if int != 0 {
                ledger::record("setsockopt", "linux option missing", level << 32 | name);
            }
        }
        // Linux doubles the size asked for (socket(7)).
        (SOL_SOCKET, SO_SNDBUF) => o.sndbuf = Some(int.saturating_mul(2)),
        (SOL_SOCKET, SO_RCVBUF) => o.rcvbuf = Some(int.saturating_mul(2)),
        (SOL_SOCKET, SO_RCVTIMEO) => o.rcvtimeo = b,
        (SOL_SOCKET, SO_SNDTIMEO) => o.sndtimeo = b,
        _ => {
            ledger::record("setsockopt", "linux option missing", level << 32 | name);
            return Err(ENOPROTOOPT);
        }
    }
    Ok(0)
}

// poll(2)'s bits (the same as `lx::socket`'s).
use super::socket::{POLLHUP, POLLIN, POLLOUT, POLLRDHUP, POLLRDNORM, POLLWRNORM};

/// What poll reports for the socket (unix_test on Linux: a fresh
/// connection-kind socket OUT|HUP; a listener IN with a connection
/// waiting; a connection IN with bytes or the peer's shutdown (RDHUP
/// then), OUT while a quarter of the buffer or less waits, HUP once both
/// directions are shut).
pub fn readiness(u: &UnixSocket) -> u16 {
    let r = u.core.readiness();
    let mut ev = 0;
    if r.readable {
        ev |= POLLIN | POLLRDNORM;
    }
    if r.writable {
        ev |= POLLOUT | POLLWRNORM;
    }
    if r.read_hangup {
        ev |= POLLRDHUP;
    }
    if r.hangup {
        ev |= POLLHUP;
    }
    ev
}

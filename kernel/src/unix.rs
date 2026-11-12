//! AF_UNIX sockets as core objects (ARCHITECTURE "The seams", seam 5:
//! shared core objects; "AF_UNIX sockets"): what a socket is, its name,
//! its connection and the bytes, messages and passed rights queued on
//! it, below every personality. The Linux personality's calls
//! (`lx::unix`) are a view of these; Windows' AF_UNIX (afd) can be
//! another, on the same names and connections.
//!
//! - **Kinds**: stream (bytes; a read takes what is queued, across
//!   writes, but stops after a write that carried rights, so they come
//!   with their bytes), seqpacket and datagram (one message per read,
//!   the rest of a longer one discarded and reported as truncated).
//! - **Names** (`Address`): a file-system node (the personality creates
//!   it, `RamFs::create_socket`, and keeps it open while bound, so its id
//!   is not reused) or an abstract name (bytes starting with a NUL, in
//!   no file system). `BOUND` maps names to their sockets; `release`
//!   takes a socket out (the node stays: Linux leaves a socket's file
//!   when it closes).
//! - **Connections** (`Conn`): two sides, a queue toward each, each
//!   side's shutdown flags as Linux keeps them (RCV and SEND: a side's
//!   shutdown(SHUT_WR) sets its SEND and the peer's RCV; a stream or
//!   seqpacket side that goes sets both on the peer). `connect` makes the
//!   listener's side at once and queues it for `accept` (a full backlog:
//!   `Error::Full`, with the listener's waiters to block on).
//! - **Rights** (`Right`): what a personality passes along a message
//!   (Linux's SCM_RIGHTS: open file descriptions), opaque here. They are
//!   dropped outside every lock here: dropping one may release another
//!   socket.
//! - **Credentials** (`Cred`): the peer's, as SO_PEERCRED reads them:
//!   the connector's for an accepted socket, the listener's at listen()
//!   for a connected one, the creator's for a pair.
//! - **Waiting** is the personality's: each socket has `waiters`, woken
//!   by every change of it, its connection or its listener's queue; a
//!   waiter adds itself, prepares, looks again, then blocks.
//! - **Buffers** are bounded (`CAPACITY` per direction, counting
//!   `RECORD_OVERHEAD` per message, as Linux counts each buffer's size)
//!   and charged to the sending process's nonpaged pool quota
//!   (`quota::Held`): over it, `Error::NoBuffers`.
//!
//! Lock order: a listener's state before a connector's (connect), each
//! socket's state before its connection's; `BOUND` alone. Nothing here
//! sleeps or touches user memory.
//!
//! Known limit: rights in flight that refer, through a socket, back to
//! their own queue (a socket sent over itself) keep each other alive
//! until the kernel ends (Linux collects such cycles); never seen from
//! the programs gala runs.

use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::vec::Vec;
use core::any::Any;
use core::fmt;

use crate::fs::ramfs::NodeId;
use crate::mm::heap::{AllocError, KArc, TryVec};
use crate::quota::{Block, Held, Pool};
use crate::runtime::{self, ThreadId};
use crate::sync::IrqMutex;

/// How many bytes may wait toward one side of a connection (Linux's
/// default socket send buffer, net.core.wmem_default).
pub const CAPACITY: usize = 212_992;
/// What each queued message counts beyond its bytes (Linux counts each
/// buffer's whole size): many empty messages fill a queue too.
pub const RECORD_OVERHEAD: usize = 256;
/// The most rights one message carries (Linux's SCM_MAX_FD).
pub const MAX_RIGHTS: usize = 253;
/// shutdown's flags of a side, as Linux keeps them.
pub const RCV: u8 = 1;
pub const SEND: u8 = 2;
pub const BOTH: u8 = RCV | SEND;

/// A socket's kind, by Linux's SOCK_* number.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Stream = 1,
    Dgram = 2,
    SeqPacket = 5,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Stream => "stream",
            Kind::Dgram => "dgram",
            Kind::SeqPacket => "seqpacket",
        }
    }

    /// Whether it has connections (a listener, accept).
    pub fn connected(self) -> bool {
        self != Kind::Dgram
    }
}

/// Who made a socket or connected it (SO_PEERCRED).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Cred {
    pub pid: u64,
    pub uid: u32,
    pub gid: u32,
}

/// What a message carries besides its bytes (a personality's open file
/// descriptions).
pub type Right = Box<dyn Any + Send>;

/// Why a call failed.
#[derive(Clone, Debug)]
pub enum Error {
    /// Nothing to take, or no room: wait on the socket's waiters.
    WouldBlock,
    /// A listener's backlog is full: wait on its waiters.
    Full(KArc<Waiters>),
    /// Not in a state the call works in (bound twice, listen unbound,
    /// accept on what does not listen, read of an unconnected stream).
    Invalid,
    NotConnected,
    IsConnected,
    AddressInUse,
    /// No socket listens on the name.
    Refused,
    /// The connection cannot carry what was sent any more.
    Pipe,
    /// A connect to a socket of another kind.
    WrongType,
    /// A message larger than a queue holds.
    TooLong,
    /// Over the sender's pool quota.
    NoBuffers,
    /// The kind cannot do it (listen on a datagram socket).
    NotSupported,
    NoMemory,
}

impl From<AllocError> for Error {
    fn from(_: AllocError) -> Error {
        Error::NoMemory
    }
}

impl From<alloc::collections::TryReserveError> for Error {
    fn from(_: alloc::collections::TryReserveError) -> Error {
        Error::NoMemory
    }
}

/// The threads waiting on a socket (see the module's doc).
#[derive(Default)]
pub struct Waiters(IrqMutex<Vec<ThreadId>>);

impl fmt::Debug for Waiters {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Waiters({})", self.0.lock().len())
    }
}

impl Waiters {
    /// Adds `t` (once).
    pub fn add(&self, t: ThreadId) -> Result<(), AllocError> {
        let mut w = self.0.lock();
        if !w.contains(&t) {
            w.try_push(t)?;
        }
        Ok(())
    }

    pub fn remove(&self, t: ThreadId) {
        self.0.lock().retain(|&w| w != t);
    }

    pub fn wake_all(&self) {
        for &t in self.0.lock().iter() {
            runtime::wake(t);
        }
    }

    pub fn len(&self) -> usize {
        self.0.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.lock().is_empty()
    }
}

/// A socket's name: the bytes of `sun_path` as Linux reports them (a
/// path and its NUL, or a NUL and the abstract name), and the node a path
/// names.
#[derive(Debug)]
pub struct Address {
    pub bytes: Vec<u8>,
    pub node: Option<NodeId>,
}

impl Address {
    fn try_clone(&self) -> Result<Address, AllocError> {
        let mut bytes = Vec::new();
        bytes.try_extend_from_slice(&self.bytes)?;
        Ok(Address { bytes, node: self.node })
    }

    fn is_abstract(&self) -> bool {
        self.node.is_none()
    }
}

/// What `connect` looks for.
#[derive(Clone, Copy, Debug)]
pub enum Target<'a> {
    Node(NodeId),
    Abstract(&'a [u8]),
}

/// One write's bytes (a stream's partly read: from `at`) and what came
/// with them.
struct Record {
    data: Vec<u8>,
    at: usize,
    rights: Vec<Right>,
    /// The bytes it holds in its queue, given back to the sender's quota
    /// when it goes.
    _charge: Option<Held>,
}

impl Record {
    fn size(&self) -> usize {
        self.data.len() + RECORD_OVERHEAD
    }
}

struct ConnState {
    /// queue[i]: what side i reads.
    queue: [VecDeque<Record>; 2],
    /// The bytes queue[i] counts against `CAPACITY`.
    queued: [usize; 2],
    shutdown: [u8; 2],
    open: [bool; 2],
    /// Grows with each thing that arrives for side i (epoll's EPOLLET).
    arrivals: [u64; 2],
}

/// A connection (or a pair's link): no references to its sockets, only to
/// their waiters.
struct Conn {
    kind: Kind,
    state: IrqMutex<ConnState>,
    waiters: [KArc<Waiters>; 2],
}

impl Conn {
    fn new(kind: Kind, a: &Socket, b: &Socket) -> Result<KArc<Conn>, AllocError> {
        KArc::try_new(Conn {
            kind,
            state: IrqMutex::new(ConnState {
                queue: [VecDeque::new(), VecDeque::new()],
                queued: [0; 2],
                shutdown: [0; 2],
                open: [true; 2],
                arrivals: [0; 2],
            }),
            waiters: [a.waiters.clone(), b.waiters.clone()],
        })
    }

    fn wake(&self) {
        self.waiters[0].wake_all();
        self.waiters[1].wake_all();
    }

    /// Side `side` is gone: its queue goes (returned, to be dropped after
    /// the lock), a connected peer sees both directions shut.
    fn close_side(&self, side: usize) -> VecDeque<Record> {
        let gone = {
            let mut st = self.state.lock();
            st.open[side] = false;
            st.queued[side] = 0;
            if self.kind.connected() {
                st.shutdown[1 - side] = BOTH;
                st.arrivals[1 - side] += 1;
            }
            core::mem::take(&mut st.queue[side])
        };
        self.wake();
        gone
    }
}

enum Phase {
    Fresh,
    Listening { backlog: usize, queue: VecDeque<KArc<Socket>>, cred: Cred, arrivals: u64 },
    Connected { conn: KArc<Conn>, side: usize },
}

struct State {
    phase: Phase,
    address: Option<Address>,
    /// The peer's name (getpeername) and credentials (SO_PEERCRED).
    peer_address: Option<Address>,
    peer_cred: Option<Cred>,
    /// shutdown flags of an unconnected socket (Linux keeps them).
    shutdown: u8,
    /// It bound `address` itself (an accepted socket only shows its
    /// listener's): its name is in `BOUND`, its node held for it.
    bound: bool,
    released: bool,
}

/// A socket.
pub struct Socket {
    pub kind: Kind,
    /// Its creator's credentials.
    pub cred: Cred,
    pub waiters: KArc<Waiters>,
    state: IrqMutex<State>,
}

/// The names bound to sockets.
enum Key {
    Node(NodeId),
    Abstract(Vec<u8>),
}

static BOUND: IrqMutex<Vec<(Key, KArc<Socket>)>> = IrqMutex::new(Vec::new());

fn find_bound(target: Target<'_>) -> Option<KArc<Socket>> {
    let bound = BOUND.lock();
    bound
        .iter()
        .find(|(k, _)| match (k, target) {
            (Key::Node(a), Target::Node(b)) => *a == b,
            (Key::Abstract(a), Target::Abstract(b)) => a.as_slice() == b,
            _ => false,
        })
        .map(|(_, s)| s.clone())
}

/// What a read took.
pub struct Received {
    /// Bytes copied out.
    pub len: usize,
    /// The whole message's length (larger than `len`: truncated).
    pub full: usize,
    pub rights: Vec<Right>,
}

/// What poll would report (the personality maps it to its bits).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Ready {
    pub readable: bool,
    pub writable: bool,
    /// Both directions shut (or never connected, for a connection kind).
    pub hangup: bool,
    /// The peer will send no more.
    pub read_hangup: bool,
}

impl Socket {
    fn make(kind: Kind, cred: Cred) -> Result<Socket, AllocError> {
        Ok(Socket {
            kind,
            cred,
            waiters: KArc::try_new(Waiters::default())?,
            state: IrqMutex::new(State {
                phase: Phase::Fresh,
                address: None,
                peer_address: None,
                peer_cred: None,
                shutdown: 0,
                bound: false,
                released: false,
            }),
        })
    }

    pub fn new(kind: Kind, cred: Cred) -> Result<KArc<Socket>, AllocError> {
        KArc::try_new(Socket::make(kind, cred)?)
    }

    /// socketpair: two sockets connected to each other, each with the
    /// creator's credentials as its peer's.
    pub fn pair(kind: Kind, cred: Cred) -> Result<(KArc<Socket>, KArc<Socket>), AllocError> {
        let a = Socket::new(kind, cred)?;
        let b = Socket::new(kind, cred)?;
        let conn = Conn::new(kind, &a, &b)?;
        for (side, s) in [&a, &b].into_iter().enumerate() {
            let mut st = s.state.lock();
            st.phase = Phase::Connected { conn: conn.clone(), side };
            st.peer_cred = Some(cred);
        }
        Ok((a, b))
    }

    /// Its name (getsockname): None while unnamed.
    pub fn address(&self) -> Result<Option<Vec<u8>>, AllocError> {
        let st = self.state.lock();
        match &st.address {
            Some(a) => Ok(Some(a.try_clone()?.bytes)),
            None => Ok(None),
        }
    }

    /// The peer's name (getpeername; None: an unnamed peer), NotConnected
    /// if it has none.
    pub fn peer_address(&self) -> Result<Option<Vec<u8>>, Error> {
        let st = self.state.lock();
        if !matches!(st.phase, Phase::Connected { .. }) {
            return Err(Error::NotConnected);
        }
        match &st.peer_address {
            Some(a) => Ok(Some(a.try_clone()?.bytes)),
            None => Ok(None),
        }
    }

    pub fn peer_cred(&self) -> Option<Cred> {
        self.state.lock().peer_cred
    }

    /// Whether it bound its name itself (an accepted socket shows its
    /// listener's).
    pub fn is_bound(&self) -> bool {
        self.state.lock().bound
    }

    pub fn is_listening(&self) -> bool {
        matches!(self.state.lock().phase, Phase::Listening { .. })
    }

    /// Gives it `address`: Invalid if it has a name, AddressInUse for an
    /// abstract name another socket has (a path's node is new: the
    /// personality made it).
    pub fn bind(this: &KArc<Socket>, address: Address) -> Result<(), Error> {
        let mut st = this.state.lock();
        if st.address.is_some() || st.released {
            return Err(Error::Invalid);
        }
        let key = match address.node {
            Some(n) => Key::Node(n),
            None => {
                let mut b = Vec::new();
                b.try_extend_from_slice(&address.bytes)?;
                Key::Abstract(b)
            }
        };
        {
            let mut bound = BOUND.lock();
            if let Key::Abstract(name) = &key
                && bound.iter().any(|(k, _)| matches!(k, Key::Abstract(b) if b == name))
            {
                return Err(Error::AddressInUse);
            }
            bound.try_push((key, this.clone()))?;
        }
        st.address = Some(address);
        st.bound = true;
        Ok(())
    }

    /// Gives a bound socket another node (`node`, a socket node the
    /// caller made and holds): a connect to it reaches this socket, as a
    /// connect to a socket file seen through a bind mount does. Its name
    /// stays the first one; the node is dropped from the names when the
    /// socket is released. Invalid if it is not bound by itself.
    pub fn alias(this: &KArc<Socket>, node: NodeId) -> Result<(), Error> {
        let st = this.state.lock();
        if !st.bound || st.released || st.address.as_ref().is_none_or(Address::is_abstract) {
            return Err(Error::Invalid);
        }
        BOUND.lock().try_push((Key::Node(node), this.clone()))?;
        Ok(())
    }

    /// Binds an unused abstract name of five hex digits (bind with only
    /// the family; Linux's autobind).
    pub fn autobind(this: &KArc<Socket>) -> Result<(), Error> {
        use core::sync::atomic::{AtomicU32, Ordering};
        static NEXT: AtomicU32 = AtomicU32::new(0);
        for _ in 0..0x10_0000 {
            let n = NEXT.fetch_add(1, Ordering::Relaxed) & 0xf_ffff;
            let mut name = [0u8; 6];
            for (i, shift) in [16, 12, 8, 4, 0].into_iter().enumerate() {
                name[i + 1] = b"0123456789abcdef"[(n >> shift) as usize & 0xf];
            }
            let mut bytes = Vec::new();
            bytes.try_extend_from_slice(&name)?;
            match Socket::bind(this, Address { bytes, node: None }) {
                Err(Error::AddressInUse) => continue,
                r => return r,
            }
        }
        Err(Error::AddressInUse)
    }

    /// listen(2): `backlog` connections may wait for accept (one more, as
    /// Linux takes them); `cred` is what connectors see.
    pub fn listen(&self, backlog: usize, cred: Cred) -> Result<(), Error> {
        if !self.kind.connected() {
            return Err(Error::NotSupported);
        }
        let mut st = self.state.lock();
        if st.address.is_none() || st.released {
            return Err(Error::Invalid);
        }
        match &mut st.phase {
            Phase::Fresh => {
                st.phase = Phase::Listening { backlog, queue: VecDeque::new(), cred, arrivals: 0 };
            }
            Phase::Listening { backlog: b, .. } => *b = backlog,
            Phase::Connected { .. } => return Err(Error::Invalid),
        }
        Ok(())
    }

    /// connect(2) to the socket listening on `target`, as `cred`.
    pub fn connect(&self, target: Target<'_>, cred: Cred) -> Result<(), Error> {
        let Some(listener) = find_bound(target) else { return Err(Error::Refused) };
        if core::ptr::eq(&*listener, self) {
            // Its own name: it does not listen (a listener cannot connect).
            return Err(if self.is_listening() { Error::Invalid } else { Error::Refused });
        }
        if listener.kind != self.kind {
            return Err(Error::WrongType);
        }
        if !self.kind.connected() {
            return Err(Error::NotSupported);
        }
        // The side that will be accepted, made before the locks.
        let server = Socket::new(self.kind, listener.cred)?;
        let conn = Conn::new(self.kind, self, &server)?;
        let mut l = listener.state.lock();
        let (full, listen_cred) = match &l.phase {
            Phase::Listening { backlog, queue, cred, .. } => (queue.len() > *backlog, *cred),
            _ => return Err(Error::Refused),
        };
        let mut me = self.state.lock();
        match me.phase {
            Phase::Connected { .. } => return Err(Error::IsConnected),
            Phase::Listening { .. } => return Err(Error::Invalid),
            Phase::Fresh => {}
        }
        if full {
            return Err(Error::Full(listener.waiters.clone()));
        }
        let copy = |a: &Option<Address>| -> Result<Option<Address>, AllocError> {
            match a {
                Some(a) => Ok(Some(a.try_clone()?)),
                None => Ok(None),
            }
        };
        let my_address = copy(&me.address)?;
        let their_address = copy(&l.address)?;
        let peer_address = copy(&l.address)?;
        {
            let mut s = server.state.lock();
            s.phase = Phase::Connected { conn: conn.clone(), side: 1 };
            s.address = their_address;
            s.peer_address = my_address;
            s.peer_cred = Some(cred);
        }
        let Phase::Listening { queue, arrivals, .. } = &mut l.phase else {
            return Err(Error::Refused);
        };
        // Reserved, then grown without allocating (alloc-audit's rule).
        queue.try_reserve(1)?;
        queue.extend(core::iter::once(server));
        *arrivals += 1;
        me.phase = Phase::Connected { conn, side: 0 };
        me.peer_address = peer_address;
        me.peer_cred = Some(listen_cred);
        drop(me);
        drop(l);
        listener.waiters.wake_all();
        Ok(())
    }

    /// accept(2): the oldest connection waiting (WouldBlock if none,
    /// Invalid if it does not listen).
    pub fn accept(&self) -> Result<KArc<Socket>, Error> {
        let taken = {
            let mut st = self.state.lock();
            let Phase::Listening { queue, .. } = &mut st.phase else {
                return Err(Error::Invalid);
            };
            queue.pop_front().ok_or(Error::WouldBlock)?
        };
        // Room for a connector waiting on a full backlog.
        self.waiters.wake_all();
        Ok(taken)
    }

    /// The connection and side of a connected socket.
    fn connection(&self) -> Option<(KArc<Conn>, usize)> {
        match &self.state.lock().phase {
            Phase::Connected { conn, side } => Some((conn.clone(), *side)),
            _ => None,
        }
    }

    /// Sends `data` (a stream takes what fits, at least a byte, or
    /// WouldBlock; a message is whole or WouldBlock), the rights in
    /// `rights` taken with the first bytes queued; charged to `purse`.
    pub fn send(
        &self,
        data: &[u8],
        rights: &mut Vec<Right>,
        purse: Option<&KArc<Block>>,
    ) -> Result<usize, Error> {
        let Some((conn, side)) = self.connection() else {
            return Err(Error::NotConnected);
        };
        let peer = 1 - side;
        let message = self.kind != Kind::Stream;
        if message && data.len() + RECORD_OVERHEAD > CAPACITY {
            return Err(Error::TooLong);
        }
        if !message && data.is_empty() {
            // Nothing to queue: rights alone are not sent on a stream.
            return Ok(0);
        }
        let mut st = conn.state.lock();
        if st.shutdown[side] & SEND != 0 {
            return Err(Error::Pipe);
        }
        if !st.open[peer] {
            return Err(if self.kind == Kind::Dgram { Error::Refused } else { Error::Pipe });
        }
        if st.shutdown[peer] & RCV != 0 && self.kind != Kind::Dgram {
            return Err(Error::Pipe);
        }
        let room = CAPACITY.saturating_sub(st.queued[peer]);
        let take = if message {
            if room < data.len() + RECORD_OVERHEAD {
                return Err(Error::WouldBlock);
            }
            data.len()
        } else {
            if room <= RECORD_OVERHEAD {
                return Err(Error::WouldBlock);
            }
            data.len().min(room - RECORD_OVERHEAD)
        };
        let size = take + RECORD_OVERHEAD;
        let charge = match purse {
            Some(p) => {
                Some(Held::try_new(p, Pool::NonPaged, size as u64).map_err(|_| Error::NoBuffers)?)
            }
            None => None,
        };
        let mut bytes = Vec::new();
        bytes.try_extend_from_slice(&data[..take])?;
        st.queue[peer].try_reserve(1)?;
        st.queue[peer].extend(core::iter::once(Record {
            data: bytes,
            at: 0,
            rights: core::mem::take(rights),
            _charge: charge,
        }));
        st.queued[peer] += size;
        st.arrivals[peer] += 1;
        drop(st);
        conn.wake();
        Ok(take)
    }

    /// Takes (or with `peek` copies) what is queued for it into `out`:
    /// a stream's bytes across writes up to a write that carried rights,
    /// or one message. End of stream: Ok with nothing. WouldBlock when
    /// nothing is there yet; a stream that is not connected is Invalid,
    /// a seqpacket NotConnected. Peeking takes no rights.
    pub fn recv(&self, out: &mut [u8], peek: bool) -> Result<Received, Error> {
        let Some((conn, side)) = self.connection() else {
            return Err(match self.kind {
                Kind::Stream => Error::Invalid,
                Kind::SeqPacket => Error::NotConnected,
                Kind::Dgram => Error::WouldBlock,
            });
        };
        let mut gone = Vec::new();
        let mut rights = Vec::new();
        let (len, full) = {
            let mut st = conn.state.lock();
            if st.queue[side].is_empty() {
                let ended = self.kind != Kind::Dgram && st.shutdown[side] & RCV != 0;
                return if ended || out.is_empty() && self.kind == Kind::Stream {
                    Ok(Received { len: 0, full: 0, rights })
                } else {
                    Err(Error::WouldBlock)
                };
            }
            let st = &mut *st;
            if self.kind == Kind::Stream {
                let mut n = 0;
                let mut i = 0;
                while n < out.len() {
                    let Some(r) = st.queue[side].get_mut(i) else { break };
                    let k = (out.len() - n).min(r.data.len() - r.at);
                    out[n..n + k].copy_from_slice(&r.data[r.at..r.at + k]);
                    n += k;
                    let carried = !r.rights.is_empty();
                    if peek {
                        i += 1;
                    } else {
                        rights = core::mem::take(&mut r.rights);
                        r.at += k;
                        if r.at == r.data.len() {
                            st.queued[side] -= r.size();
                            if let Some(r) = st.queue[side].pop_front() {
                                // Dropped after the lock (its charge).
                                let _ = gone.try_push(r);
                            }
                        }
                    }
                    if carried {
                        break;
                    }
                }
                (n, n)
            } else {
                let Some(r) = st.queue[side].front() else { return Err(Error::WouldBlock) };
                let full = r.data.len();
                let k = out.len().min(full);
                out[..k].copy_from_slice(&r.data[..k]);
                if !peek && let Some(mut r) = st.queue[side].pop_front() {
                    st.queued[side] -= r.size();
                    rights = core::mem::take(&mut r.rights);
                    let _ = gone.try_push(r);
                }
                (k, full)
            }
        };
        drop(gone);
        if !peek {
            // Room for the sender.
            conn.wake();
        }
        Ok(Received { len, full, rights })
    }

    /// shutdown(2) with Linux's flags (`RCV`, `SEND`): a connected
    /// stream's or seqpacket's peer gets the mirror of them.
    pub fn shutdown(&self, how: u8) {
        let Some((conn, side)) = self.connection() else {
            self.state.lock().shutdown |= how;
            return;
        };
        {
            let mut st = conn.state.lock();
            st.shutdown[side] |= how;
            if conn.kind.connected() {
                let mut mirror = 0;
                if how & RCV != 0 {
                    mirror |= SEND;
                }
                if how & SEND != 0 {
                    mirror |= RCV;
                }
                st.shutdown[1 - side] |= mirror;
                st.arrivals[1 - side] += 1;
            }
            st.arrivals[side] += 1;
        }
        conn.wake();
    }

    /// What poll would report now (see `Ready`).
    pub fn readiness(&self) -> Ready {
        let (conn, side) = {
            let st = self.state.lock();
            match &st.phase {
                Phase::Fresh => {
                    return Ready {
                        readable: st.shutdown & RCV != 0,
                        writable: true,
                        hangup: self.kind.connected() || st.shutdown == BOTH,
                        read_hangup: st.shutdown & RCV != 0,
                    };
                }
                Phase::Listening { queue, .. } => {
                    return Ready { readable: !queue.is_empty(), ..Ready::default() };
                }
                Phase::Connected { conn, side } => (conn.clone(), *side),
            }
        };
        let st = conn.state.lock();
        let shut = st.shutdown[side];
        // Linux: writable while what it sent and is not read yet is at
        // most a quarter of the buffer.
        let writable = st.queued[1 - side] * 4 <= CAPACITY;
        Ready {
            readable: !st.queue[side].is_empty() || shut & RCV != 0,
            writable,
            hangup: shut == BOTH,
            read_hangup: shut & RCV != 0,
        }
    }

    /// A count that grows with each arrival (bytes, a message, a
    /// connection, a shutdown): epoll's EPOLLET reports again only after
    /// it moved.
    pub fn arrivals(&self) -> u64 {
        let (conn, side) = {
            let st = self.state.lock();
            match &st.phase {
                Phase::Fresh => return 0,
                Phase::Listening { arrivals, .. } => return *arrivals,
                Phase::Connected { conn, side } => (conn.clone(), *side),
            }
        };
        conn.state.lock().arrivals[side]
    }

    /// The last reference of its personality went: its name is free
    /// again, a listener's waiting connections go, a connection's side
    /// closes. Returns the node it bound itself to (the personality closes
    /// its hold on it); an accepted socket shows its listener's name but
    /// holds nothing.
    pub fn release(&self) -> Option<NodeId> {
        let (phase, address) = {
            let mut st = self.state.lock();
            if st.released {
                return None;
            }
            st.released = true;
            let bound = st.bound.then(|| st.address.as_ref().and_then(|a| a.node));
            (core::mem::replace(&mut st.phase, Phase::Fresh), bound)
        };
        if address.is_some() {
            BOUND.lock().retain(|(_, s)| !core::ptr::eq(&**s, self));
        }
        match phase {
            Phase::Fresh => {}
            Phase::Listening { queue, .. } => {
                for s in queue {
                    s.release();
                }
            }
            Phase::Connected { conn, side } => drop(conn.close_side(side)),
        }
        self.waiters.wake_all();
        address.flatten()
    }

    /// One line for the state dump.
    fn describe(&self, out: &mut dyn fmt::Write) -> fmt::Result {
        let st = self.state.lock();
        write!(out, "{} by pid {}", self.kind.name(), self.cred.pid)?;
        if let Some(a) = &st.address {
            if a.is_abstract() {
                write!(out, ", abstract ")?;
                for &b in a.bytes.iter().skip(1) {
                    write!(out, "{}", Byte(b))?;
                }
            } else {
                write!(out, ", node {}", a.node.unwrap_or(0))?;
            }
        }
        match &st.phase {
            Phase::Fresh => write!(out, ", unconnected"),
            Phase::Listening { backlog, queue, .. } => {
                write!(out, ", listening ({} of {} waiting)", queue.len(), backlog + 1)
            }
            Phase::Connected { conn, side } => {
                let c = conn.state.lock();
                write!(
                    out,
                    ", connected (side {side}: {} queued, shutdown {}, peer {})",
                    c.queue[*side].len(),
                    c.shutdown[*side],
                    if c.open[1 - side] { "open" } else { "gone" }
                )
            }
        }?;
        write!(out, ", {} waiting", self.waiters.len())
    }
}

/// A byte of a name as the dump shows it.
struct Byte(u8);

impl fmt::Display for Byte {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0.is_ascii_graphic() {
            write!(f, "{}", self.0 as char)
        } else {
            write!(f, "\\x{:02x}", self.0)
        }
    }
}

/// The state dump: every socket with a name (listening or not).
pub fn dump() {
    let bound = {
        let b = BOUND.lock();
        let mut v = Vec::new();
        if v.try_reserve_exact(b.len()).is_err() {
            crate::info!("dump: unix: {} bound sockets (no memory to list them)", b.len());
            return;
        }
        for (_, s) in b.iter() {
            // Room reserved above.
            let _ = v.try_push(s.clone());
        }
        v
    };
    for s in bound {
        crate::info!("dump: unix: {}", Described(&s));
    }
}

struct Described<'a>(&'a Socket);

impl fmt::Display for Described<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.describe(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ME: Cred = Cred { pid: 7, uid: 1000, gid: 1000 };
    const OTHER: Cred = Cred { pid: 9, uid: 1000, gid: 1000 };

    fn abstract_name(name: &[u8]) -> Address {
        let mut bytes = Vec::new();
        bytes.push(0);
        bytes.extend_from_slice(name);
        Address { bytes, node: None }
    }

    fn recv(s: &Socket, n: usize) -> Result<(Vec<u8>, usize, usize), Error> {
        let mut buf = alloc::vec![0u8; n];
        let r = s.recv(&mut buf, false)?;
        buf.truncate(r.len);
        Ok((buf, r.full, r.rights.len()))
    }

    fn right(n: u32) -> Right {
        Box::new(n)
    }

    #[test]
    fn a_pair_carries_bytes_both_ways_and_ends_like_linux() {
        let (a, b) = Socket::pair(Kind::Stream, ME).unwrap();
        assert_eq!(a.peer_cred(), Some(ME));
        assert_eq!(a.send(b"po", &mut Vec::new(), None).unwrap(), 2);
        assert_eq!(a.send(b"ng", &mut Vec::new(), None).unwrap(), 2);
        assert!(b.readiness().readable);
        assert_eq!(recv(&b, 16).unwrap().0, b"pong");
        assert!(matches!(recv(&b, 16), Err(Error::WouldBlock)));
        a.shutdown(SEND);
        assert_eq!(
            b.readiness(),
            Ready { readable: true, writable: true, hangup: false, read_hangup: true }
        );
        assert_eq!(recv(&b, 16).unwrap().0, b"");
        assert!(matches!(a.send(b"x", &mut Vec::new(), None), Err(Error::Pipe)));
        a.release();
        assert_eq!(
            b.readiness(),
            Ready { readable: true, writable: true, hangup: true, read_hangup: true }
        );
        assert!(matches!(b.send(b"x", &mut Vec::new(), None), Err(Error::Pipe)));
    }

    #[test]
    fn a_stream_read_stops_after_rights() {
        let (a, b) = Socket::pair(Kind::Stream, ME).unwrap();
        a.send(b"first", &mut Vec::new(), None).unwrap();
        let mut r = alloc::vec![right(1), right(2)];
        a.send(b"second", &mut r, None).unwrap();
        assert!(r.is_empty());
        a.send(b"third", &mut Vec::new(), None).unwrap();
        assert_eq!(recv(&b, 64).unwrap(), (b"firstsecond".to_vec(), 11, 2));
        assert_eq!(recv(&b, 64).unwrap(), (b"third".to_vec(), 5, 0));
        // A partial read of a record with rights takes them with its
        // first bytes.
        let mut r = alloc::vec![right(3)];
        a.send(b"abcdef", &mut r, None).unwrap();
        assert_eq!(recv(&b, 2).unwrap(), (b"ab".to_vec(), 2, 1));
        assert_eq!(recv(&b, 64).unwrap(), (b"cdef".to_vec(), 4, 0));
    }

    #[test]
    fn messages_keep_their_boundaries() {
        let (a, b) = Socket::pair(Kind::SeqPacket, ME).unwrap();
        a.send(b"abc", &mut Vec::new(), None).unwrap();
        a.send(b"", &mut Vec::new(), None).unwrap();
        assert_eq!(recv(&b, 2).unwrap(), (b"ab".to_vec(), 3, 0));
        assert_eq!(recv(&b, 8).unwrap(), (b"".to_vec(), 0, 0));
        assert!(matches!(recv(&b, 8), Err(Error::WouldBlock)));
        a.release();
        assert_eq!(recv(&b, 8).unwrap().1, 0);
        let (c, d) = Socket::pair(Kind::Dgram, ME).unwrap();
        c.release();
        assert!(matches!(d.send(b"x", &mut Vec::new(), None), Err(Error::Refused)));
        assert!(matches!(recv(&d, 8), Err(Error::WouldBlock)));
        assert_eq!(d.readiness(), Ready { writable: true, ..Ready::default() });
    }

    #[test]
    fn a_full_queue_refuses_until_read() {
        let (a, b) = Socket::pair(Kind::Stream, ME).unwrap();
        let chunk = alloc::vec![7u8; 4096];
        let mut sent = 0;
        loop {
            match a.send(&chunk, &mut Vec::new(), None) {
                Ok(n) => sent += n,
                Err(Error::WouldBlock) => break,
                Err(e) => panic!("{e:?}"),
            }
        }
        assert!(sent > 0 && !a.readiness().writable);
        let mut got = 0;
        while let Ok((d, ..)) = recv(&b, 4096) {
            got += d.len();
        }
        assert_eq!(got, sent);
        assert!(a.readiness().writable);
        let m = Socket::pair(Kind::Dgram, ME).unwrap().0;
        assert!(matches!(
            m.send(&alloc::vec![0; CAPACITY], &mut Vec::new(), None),
            Err(Error::TooLong)
        ));
    }

    #[test]
    fn names_listen_connect_accept() {
        let l = Socket::new(Kind::Stream, ME).unwrap();
        assert!(matches!(l.listen(1, ME), Err(Error::Invalid)));
        Socket::bind(&l, abstract_name(b"unit-l")).unwrap();
        assert!(matches!(Socket::bind(&l, abstract_name(b"other")), Err(Error::Invalid)));
        let twin = Socket::new(Kind::Stream, ME).unwrap();
        assert!(matches!(Socket::bind(&twin, abstract_name(b"unit-l")), Err(Error::AddressInUse)));
        let c = Socket::new(Kind::Stream, OTHER).unwrap();
        let target = Target::Abstract(b"\0unit-l");
        assert!(matches!(c.connect(target, OTHER), Err(Error::Refused)));
        l.listen(1, ME).unwrap();
        c.connect(target, OTHER).unwrap();
        assert!(matches!(c.connect(target, OTHER), Err(Error::IsConnected)));
        assert_eq!(c.peer_cred(), Some(ME));
        assert_eq!(c.peer_address().unwrap().unwrap(), b"\0unit-l");
        let d = Socket::new(Kind::Stream, OTHER).unwrap();
        d.connect(target, OTHER).unwrap();
        let e = Socket::new(Kind::Stream, OTHER).unwrap();
        assert!(matches!(e.connect(target, OTHER), Err(Error::Full(_))));
        let dg = Socket::new(Kind::Dgram, OTHER).unwrap();
        assert!(matches!(dg.connect(target, OTHER), Err(Error::WrongType)));
        assert!(l.readiness().readable);
        let s = l.accept().unwrap();
        assert_eq!(s.peer_cred(), Some(OTHER));
        assert_eq!(s.address().unwrap().unwrap(), b"\0unit-l");
        assert_eq!(s.peer_address().unwrap(), None);
        c.send(b"hi", &mut Vec::new(), None).unwrap();
        assert_eq!(recv(&s, 8).unwrap().0, b"hi");
        assert_eq!(s.release(), None);
        // The listener closes: the one still waiting ends, the name is free.
        l.release();
        assert_eq!(recv(&d, 8).unwrap().0, b"");
        assert!(matches!(e.connect(target, OTHER), Err(Error::Refused)));
        Socket::bind(&twin, abstract_name(b"unit-l")).unwrap();
        twin.release();
        let auto = Socket::new(Kind::Stream, ME).unwrap();
        Socket::autobind(&auto).unwrap();
        let name = auto.address().unwrap().unwrap();
        assert!(name.len() == 6 && name[0] == 0 && name[1..].iter().all(u8::is_ascii_hexdigit));
        auto.release();
    }

    /// A listener bound to a node reached through another node too (a
    /// WSL distribution's view of the Wayland socket); the alias goes
    /// with the listener.
    #[test]
    fn an_alias_reaches_the_listener_until_it_is_released() {
        let (first, second) = (1_000_001, 1_000_002);
        let l = Socket::new(Kind::Stream, ME).unwrap();
        assert!(matches!(Socket::alias(&l, second), Err(Error::Invalid)));
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"/run/x\0");
        Socket::bind(&l, Address { bytes, node: Some(first) }).unwrap();
        l.listen(4, ME).unwrap();
        Socket::alias(&l, second).unwrap();
        let c = Socket::new(Kind::Stream, OTHER).unwrap();
        c.connect(Target::Node(second), OTHER).unwrap();
        // Its name stays the first.
        assert_eq!(c.peer_address().unwrap().unwrap(), b"/run/x\0");
        let a = l.accept().unwrap();
        c.send(b"hi", &mut Vec::new(), None).unwrap();
        assert_eq!(recv(&a, 8).unwrap().0, b"hi");
        assert_eq!(l.release(), Some(first));
        let d = Socket::new(Kind::Stream, OTHER).unwrap();
        assert!(matches!(d.connect(Target::Node(second), OTHER), Err(Error::Refused)));
        let abs = Socket::new(Kind::Stream, ME).unwrap();
        Socket::bind(&abs, abstract_name(b"unit-alias")).unwrap();
        assert!(matches!(Socket::alias(&abs, second), Err(Error::Invalid)));
        abs.release();
    }

    #[test]
    fn a_closed_side_drops_what_was_queued_for_it() {
        let (a, b) = Socket::pair(Kind::Stream, ME).unwrap();
        let mut r = alloc::vec![right(5)];
        a.send(b"x", &mut r, None).unwrap();
        b.release();
        assert!(matches!(a.send(b"y", &mut Vec::new(), None), Err(Error::Pipe)));
        assert_eq!(a.connection().unwrap().0.state.lock().queue[1].len(), 0);
    }
}

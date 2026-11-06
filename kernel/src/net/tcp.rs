//! The kernel's TCP socket API on smoltcp, independent of NT (`nt::afd`
//! builds Windows sockets on it). Design: docs/ARCHITECTURE.md,
//! "Networking".
//!
//! Two interfaces, each with its own socket set: `lo` (127.0.0.1/8, always)
//! and `eth` (the card, when there is one). The sets grow fallibly up to
//! the stack's socket limit (a share of memory, `socket_limit`). Our own
//! table of entries maps `SocketId`s to smoltcp sockets: a bound address, a
//! listener (one listening socket per interface, its connections being
//! opened and those waiting to be accepted) or a connection.
//! The `net` thread polls; after each poll every entry whose readiness
//! changed is reported to the notify hook with the entry's token.

use super::loopback::Loopback;
use super::virtio_net::VirtioNet;
use crate::mm::heap::TryVec;
use crate::sync::IrqMutex;
// Threads and time through the core's runtime (not `nt::platform`: `net`
// is core, below every personality), so the stack also runs in gala-um
// (loopback only there).
use crate::runtime as sched;
use crate::runtime as time;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use smoltcp::iface::{
    Config, Interface, PollIngressSingleResult, PollResult, SocketHandle, SocketSet, SocketStorage,
};
use smoltcp::phy::Device;
use smoltcp::socket::tcp::{self, State};
use smoltcp::wire::{
    HardwareAddress, IpAddress, IpCidr, IpEndpoint, IpListenEndpoint, Ipv4Address,
};

/// Bytes buffered per direction per connection.
const BUFFER: usize = 64 * 1024;
/// What one socket's buffers take (both directions; a UDP socket's too).
const SOCKET_COST: u64 = 2 * BUFFER as u64;
/// The share of memory the stack's sockets may take: a quarter
/// (`socket_limit`).
const MEMORY_SHARE: u64 = 4;
/// The fewest sockets the limit allows, however little memory (and in
/// host tests, which have no frame allocator).
const MIN_SOCKETS: usize = 64;
/// Slots a socket set starts with; it doubles as it fills.
const INITIAL_SLOTS: usize = 16;
/// The most connections a listener lets wait (opened or established, not
/// accepted): a bound on what callers ask, not storage (each connection
/// is a socket of its own when it comes). 65535 is the top of Windows'
/// SOMAXCONN_HINT range.
pub const MAX_BACKLOG: usize = 65535;
const EPHEMERAL: core::ops::RangeInclusive<u16> = 49152..=65535;

use super::{Ip, Scope};

pub const LOOPBACK: Ipv4Address = Ipv4Address::new(127, 0, 0, 1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetError {
    /// Not possible yet; wait for the notify hook and retry.
    WouldBlock,
    InvalidState,
    AddressInUse,
    AddressNotAvailable,
    Refused,
    Reset,
    NotConnected,
    Unreachable,
    NoMemory,
    /// The id names no live socket.
    NoSocket,
    /// A datagram larger than the interface carries.
    TooLarge,
}

/// An IPv4 or IPv6 address and port.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Addr {
    pub ip: Ip,
    pub port: u16,
}

impl Addr {
    /// An IPv4 address and port.
    pub const fn v4(ip: [u8; 4], port: u16) -> Self {
        Self { ip: Ip::V4(ip), port }
    }

    fn from_endpoint(e: IpEndpoint) -> Self {
        Self { ip: Ip::from_smoltcp(e.addr), port: e.port }
    }

    pub fn endpoint(self) -> IpEndpoint {
        IpEndpoint::new(self.ip.to_smoltcp(), self.port)
    }
}

/// `"a.b.c.d:port"` (a JSON string, for the event stream).
impl core::fmt::Display for Addr {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self.ip {
            Ip::V4(_) => write!(f, "\"{}:{}\"", self.ip, self.port),
            Ip::V6(_) => write!(f, "\"[{}]:{}\"", self.ip, self.port),
        }
    }
}

/// A connection event (`events=net`): accepted, connecting, closed.
fn net_event(what: &str, id: SocketId, local: Addr, remote: Option<Addr>) {
    crate::events::emit(
        crate::events::Category::Net,
        what,
        format_args!(
            r#""socket":{},"local":{local},"remote":{}"#,
            id.index,
            remote.map_or(Addr::default(), |r| r)
        ),
    );
}

/// The end of a connection's sending side or of the connection
/// (`events=net`: "tcp shutdown", "tcp close"), with its byte counts: what
/// its owner queued and took, what was still unsent (queued, not yet
/// acknowledged) and unread (received, never taken). A stream that came
/// up short is found by comparing one end's `sent` with the other's
/// `received`.
fn bytes_event(what: &str, id: SocketId, local: Addr, remote: Option<Addr>, b: Bytes) {
    crate::events::emit(
        crate::events::Category::Net,
        what,
        format_args!(
            r#""socket":{},"local":{local},"remote":{},"sent":{},"received":{},"unsent":{},"unread":{}"#,
            id.index,
            remote.map_or(Addr::default(), |r| r),
            b.sent,
            b.received,
            b.unsent,
            b.unread
        ),
    );
}

/// A connection's byte counts (`bytes_event`): `sent` and `received` by
/// its owner since the socket was made (or adopted after a handover);
/// `unsent` and `unread` are the queues' lengths when reported.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Bytes {
    sent: u64,
    received: u64,
    unsent: usize,
    unread: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SocketId {
    pub(super) index: u32,
    generation: u32,
}

impl SocketId {
    /// The number events name it by (`"socket"`).
    pub fn number(self) -> u32 {
        self.index
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Side {
    Lo,
    Eth,
}

impl Side {
    fn index(self) -> usize {
        self as usize
    }

    fn name(self) -> &'static str {
        match self {
            Side::Lo => "loopback",
            Side::Eth => "card",
        }
    }
}

pub(super) type Sock = (Side, SocketHandle);

/// What a listener does with a connection request (a SYN) that finds its
/// backlog full.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WhenFull {
    /// Refuse it (the stack answers with RST: the client's connect fails
    /// with connection refused), as Windows documents for listen.
    #[default]
    Refuse,
    /// Ignore it (no answer: the client sends its SYN again later and gets
    /// in once an accept made room), as Linux does (tcp(7)'s
    /// tcp_abort_on_overflow off).
    Ignore,
}

/// A listener: one socket in LISTEN per interface (the one smoltcp hands
/// the next SYN to; smoltcp has no accept queue, a listening socket
/// *becomes* the connection), and the connections it took. After every
/// packet the stack moves a socket that took a SYN to `opening` and puts
/// a fresh one in its place while the backlog has room (`Stack::tend`),
/// so a listener holds two sockets at most besides its connections.
#[derive(Default)]
pub(super) struct Listener {
    /// The most connections waiting (`opening` and `ready` together).
    backlog: usize,
    when_full: WhenFull,
    /// The socket in LISTEN on each interface (`Side::index`): None while
    /// the backlog is full (WhenFull::Refuse: the stack's own RST answers
    /// the SYN) or when no socket could be had (the limit, memory).
    catchers: [Option<Sock>; 2],
    /// Connections being opened (SYN received, the handshake not done).
    opening: Vec<Sock>,
    /// Established connections waiting to be accepted, with their
    /// sequence numbers (AFD's WAIT_FOR_LISTEN / ACCEPT name them).
    ready: Vec<(u32, Sock)>,
    next_sequence: u32,
    /// The backlog was full at the last look (`events=net` "tcp backlog
    /// full" once each time it fills).
    full: bool,
}

impl Listener {
    fn waiting(&self) -> usize {
        self.opening.len() + self.ready.len()
    }
}

pub(super) enum Kind {
    Bound,
    Listener(Listener),
    Conn {
        sock: Sock,
        connected: bool,
    },
    /// A UDP socket (`net::udp`).
    Udp(super::udp::Udp),
}

impl Kind {
    fn is_udp(&self) -> bool {
        matches!(self, Kind::Udp(_))
    }
}

pub(super) struct Entry {
    generation: u32,
    token: u64,
    pub(super) local: Addr,
    /// The families it takes (ports and listening addresses follow it).
    pub(super) scope: Scope,
    pub(super) kind: Kind,
    /// Readiness at the last report (see `signature`); STALE after the
    /// owner sent, received or accepted (the next poll reports it: a queue
    /// emptied and refilled to the same length looks unchanged).
    pub(super) last: u64,
    /// What its owner sent and received (a connection; `bytes_event`).
    pub(super) bytes: Bytes,
}

/// No signature: the next poll reports the entry.
pub(super) const STALE: u64 = u64::MAX;

pub(super) struct Iface<D: Device> {
    pub(super) dev: D,
    iface: Interface,
    pub(super) sockets: SocketSet<'static>,
    /// Sockets in the set.
    count: usize,
}

impl<D: Device> Iface<D> {
    fn new(mut dev: D, hw: HardwareAddress, cidr: IpCidr) -> Option<Self> {
        let mut config = Config::new(hw);
        config.random_seed = time::monotonic_ns() ^ 0x9e37_79b9_7f4a_7c15;
        let mut iface = Interface::new(config, &mut dev, now());
        iface.update_ip_addrs(|addrs| {
            let _ = addrs.push(cidr);
        });
        // Empty slots reserved, not made: `Stack::add` grows the set
        // (fallibly) before smoltcp would push.
        let mut storage: Vec<SocketStorage<'static>> = Vec::new();
        storage.try_reserve_exact(INITIAL_SLOTS).ok()?;
        Some(Self { dev, iface, sockets: SocketSet::new(storage), count: 0 })
    }

    /// Takes one received packet; false when there was none.
    fn ingress_one(&mut self, now: smoltcp::time::Instant) -> bool {
        self.iface.poll_ingress_single(now, &mut self.dev, &mut self.sockets)
            != PollIngressSingleResult::None
    }

    /// Sends what the sockets queued; whether a socket may have changed.
    fn egress(&mut self, now: smoltcp::time::Instant) -> bool {
        self.iface.poll_egress(now, &mut self.dev, &mut self.sockets) != PollResult::None
    }
}

/// The most sockets the stack holds, every interface's together: their
/// buffers (SOCKET_COST each) may take a quarter of the machine's memory,
/// at least MIN_SOCKETS of them. Every listener holds one per interface
/// and every connection one, so this is what bounds listeners and
/// connections alike (`Stack::admit`). TODO (ROADMAP R6): charge the
/// buffers to the owning process's quota as well.
fn socket_limit() -> usize {
    let (_, frames) = crate::mm::frame_stats();
    let share = frames.saturating_mul(4096) / MEMORY_SHARE / SOCKET_COST;
    usize::try_from(share).unwrap_or(usize::MAX).max(MIN_SOCKETS)
}

pub(super) struct Stack {
    pub(super) lo: Iface<Loopback>,
    pub(super) eth: Option<Iface<VirtioNet>>,
    pub(super) eth_ip: Option<[u8; 4]>,
    eth_mac: [u8; 6],
    pub(super) entries: Vec<Option<Entry>>,
    /// Closed by their owner, still finishing (FIN, TIME-WAIT); removed
    /// from their set when done.
    lingering: Vec<Sock>,
    next_port: u16,
    next_generation: u32,
    /// `socket_limit` when the stack started.
    limit: usize,
    /// A socket was refused and it was logged (once until a socket goes).
    refused_logged: bool,
}

static STACK: IrqMutex<Option<Stack>> = IrqMutex::new(None);
static NET_THREAD: AtomicU64 = AtomicU64::new(u64::MAX);
static KICKED: AtomicBool = AtomicBool::new(false);
/// A kernel handover stopped the stack (`stop`); the net thread parks.
static FROZEN: AtomicBool = AtomicBool::new(false);
/// The net thread saw FROZEN and is parked (polls nothing).
static PARKED: AtomicBool = AtomicBool::new(false);
static NOTIFY: IrqMutex<Option<fn(u64)>> = IrqMutex::new(None);

pub static POLLS: AtomicU64 = AtomicU64::new(0);

fn now() -> smoltcp::time::Instant {
    smoltcp::time::Instant::from_micros((time::monotonic_ns() / 1000) as i64)
}

fn new_socket() -> Option<tcp::Socket<'static>> {
    let (mut rx, mut tx) = (Vec::new(), Vec::new());
    rx.try_reserve_exact(BUFFER).ok()?;
    tx.try_reserve_exact(BUFFER).ok()?;
    rx.resize(BUFFER, 0);
    tx.resize(BUFFER, 0);
    let mut socket = tcp::Socket::new(tcp::SocketBuffer::new(rx), tcp::SocketBuffer::new(tx));
    // Small writes go out at once, as with TCP_NODELAY (request/response
    // round trips over loopback would otherwise wait for delayed ACKs).
    socket.set_nagle_enabled(false);
    socket.set_ack_delay(None);
    Some(socket)
}

impl Stack {
    pub(super) fn sockets(&mut self, side: Side) -> Option<&mut SocketSet<'static>> {
        match side {
            Side::Lo => Some(&mut self.lo.sockets),
            Side::Eth => self.eth.as_mut().map(|e| &mut e.sockets),
        }
    }

    fn tcp(&mut self, (side, handle): Sock) -> Option<&mut tcp::Socket<'static>> {
        Some(self.sockets(side)?.get_mut::<tcp::Socket>(handle))
    }

    /// Sockets in every set.
    fn in_use(&self) -> usize {
        self.lo.count + self.eth.as_ref().map_or(0, |e| e.count)
    }

    /// Logs why a socket is refused, once until a socket goes.
    fn refused(&mut self, side: Side, why: core::fmt::Arguments<'_>) {
        if !self.refused_logged {
            self.refused_logged = true;
            let (free, total) = crate::mm::frame_stats();
            crate::warn!(
                "net: a {} socket is refused: {why} ({} sockets in use: loopback {}, card {}; \
                 limit {}, a quarter of memory at {} KiB each; frames free {free} of {total})",
                side.name(),
                self.in_use(),
                self.lo.count,
                self.eth.as_ref().map_or(0, |e| e.count),
                self.limit,
                SOCKET_COST / 1024,
            );
        }
    }

    /// Whether one more socket may go into `side`'s set: under the limit,
    /// and room in the set (grown fallibly here, so smoltcp's `add` never
    /// allocates). Callers ask before allocating a socket's buffers.
    fn admit(&mut self, side: Side) -> Result<(), NetError> {
        if self.in_use() >= self.limit {
            let limit = self.limit;
            self.refused(side, format_args!("all {limit} sockets are in use"));
            return Err(NetError::NoMemory);
        }
        let set = self.sockets(side).ok_or(NetError::Unreachable)?;
        if !set.has_room() {
            let grow = set.slots().max(INITIAL_SLOTS);
            if set.try_reserve(grow).is_err() {
                self.refused(side, format_args!("out of memory growing the socket set"));
                return Err(NetError::NoMemory);
            }
        }
        Ok(())
    }

    /// A TCP socket for `side` with its buffers (`admit` first).
    fn new_tcp(&mut self, side: Side) -> Result<tcp::Socket<'static>, NetError> {
        self.admit(side)?;
        new_socket().ok_or_else(|| {
            self.refused(side, format_args!("out of memory for its buffers"));
            NetError::NoMemory
        })
    }

    pub(super) fn add<S: smoltcp::socket::AnySocket<'static>>(
        &mut self,
        side: Side,
        socket: S,
    ) -> Result<Sock, NetError> {
        self.admit(side)?;
        let handle = self.sockets(side).ok_or(NetError::Unreachable)?.add(socket);
        match side {
            Side::Lo => self.lo.count += 1,
            Side::Eth => {
                if let Some(e) = self.eth.as_mut() {
                    e.count += 1;
                }
            }
        }
        Ok((side, handle))
    }

    pub(super) fn remove(&mut self, (side, handle): Sock) {
        if let Some(set) = self.sockets(side) {
            set.remove(handle);
            match side {
                Side::Lo => self.lo.count -= 1,
                Side::Eth => {
                    if let Some(e) = self.eth.as_mut() {
                        e.count -= 1;
                    }
                }
            }
            self.refused_logged = false;
        }
    }

    pub(super) fn entry(&mut self, id: SocketId) -> Result<&mut Entry, NetError> {
        match self.entries.get_mut(id.index as usize) {
            Some(Some(e)) if e.generation == id.generation => Ok(e),
            _ => Err(NetError::NoSocket),
        }
    }

    /// Whether a TCP (or, with `udp`, a UDP) socket has `port`: the two
    /// protocols' ports are apart.
    pub(super) fn port_in_use(&self, port: u16, udp: bool, scope: Scope) -> bool {
        self.entries
            .iter()
            .flatten()
            .any(|e| e.local.port == port && e.kind.is_udp() == udp && e.scope.overlaps(scope))
    }

    pub(super) fn ephemeral_port(&mut self, udp: bool, scope: Scope) -> Result<u16, NetError> {
        for _ in EPHEMERAL {
            let port = self.next_port;
            self.next_port = if port == *EPHEMERAL.end() { *EPHEMERAL.start() } else { port + 1 };
            if !self.port_in_use(port, udp, scope) {
                return Ok(port);
            }
        }
        Err(NetError::AddressInUse)
    }

    pub(super) fn insert(
        &mut self,
        token: u64,
        local: Addr,
        scope: Scope,
        kind: Kind,
    ) -> Result<SocketId, NetError> {
        let generation = self.next_generation;
        self.next_generation = self.next_generation.wrapping_add(1);
        let entry =
            Entry { generation, token, local, scope, kind, last: 0, bytes: Bytes::default() };
        let index = match self.entries.iter().position(Option::is_none) {
            Some(i) => {
                self.entries[i] = Some(entry);
                i
            }
            None => {
                self.entries.try_push(Some(entry)).map_err(|_| NetError::NoMemory)?;
                self.entries.len() - 1
            }
        };
        Ok(SocketId { index: index as u32, generation })
    }

    /// The interfaces a local address listens on.
    pub(super) fn sides_for(&self, ip: Ip) -> &'static [Side] {
        match (ip.is_unspecified(), ip.is_loopback(), self.eth.is_some()) {
            (true, _, true) => &[Side::Lo, Side::Eth],
            (true, _, false) | (_, true, _) => &[Side::Lo],
            _ => &[Side::Eth],
        }
    }

    pub(super) fn side_to(&self, ip: Ip) -> Result<Side, NetError> {
        match ip {
            ip if ip.is_loopback() => Ok(Side::Lo),
            // The card has no IPv6 address yet (N3 step 3).
            Ip::V6(_) => Err(NetError::Unreachable),
            _ if self.eth.is_some() => Ok(Side::Eth),
            _ => Err(NetError::Unreachable),
        }
    }

    /// Whether a socket may bind to `ip`: unspecified, loopback or the
    /// card's (IPv4) address.
    pub(super) fn may_bind(&self, ip: Ip) -> bool {
        ip.is_unspecified() || ip.is_loopback() || matches!(ip, Ip::V4(a) if Some(a) == self.eth_ip)
    }

    /// The address a socket of `scope` bound to `ip` takes on `side`: the
    /// address itself, or for an unspecified one the side's address of
    /// the scope's family (no address: any family, dual stack); None: it
    /// takes nothing on that side (no IPv6 address on the card yet).
    pub(super) fn side_address(
        &self,
        side: Side,
        ip: Ip,
        scope: Scope,
    ) -> Option<Option<IpAddress>> {
        if !ip.is_unspecified() {
            return Some(Some(ip.to_smoltcp()));
        }
        match (scope, side) {
            (Scope::Dual, _) => Some(None),
            (Scope::V4, Side::Lo) => Some(Some(Ip::V4([127, 0, 0, 1]).to_smoltcp())),
            (Scope::V4, Side::Eth) => self.eth_ip.map(|a| Some(Ip::V4(a).to_smoltcp())),
            (Scope::V6, Side::Lo) => Some(Some(Ip::V6(Ip::LOOPBACK6).to_smoltcp())),
            (Scope::V6, Side::Eth) => None,
        }
    }

    fn listening_socket(
        &mut self,
        side: Side,
        local: Addr,
        scope: Scope,
    ) -> Result<Sock, NetError> {
        let addr = self.side_address(side, local.ip, scope).ok_or(NetError::AddressNotAvailable)?;
        let mut socket = self.new_tcp(side)?;
        let endpoint = IpListenEndpoint { addr, port: local.port };
        socket.listen(endpoint).map_err(|_| NetError::InvalidState)?;
        self.add(side, socket)
    }

    /// Puts every listener's sockets in order (`tend`).
    fn tend_all(&mut self) {
        for i in 0..self.entries.len() {
            if matches!(self.entries[i], Some(Entry { kind: Kind::Listener(_), .. })) {
                self.tend(i);
            }
        }
    }

    /// Puts listener `i`'s sockets in order after smoltcp moved some: a
    /// listening socket that took a SYN joins the connections being opened
    /// (or, with the backlog full and WhenFull::Ignore, is dropped before
    /// it answered: the SYN goes unanswered); an opened connection joins
    /// the ready queue; one that went back to LISTEN (reset during the
    /// handshake) or closed goes; then each interface gets a fresh
    /// listening socket where it has none, while the backlog has room
    /// (always with WhenFull::Ignore). With WhenFull::Refuse and the
    /// backlog full, the listening sockets go: a SYN then finds none and
    /// the stack answers it with RST. Runs after every received packet,
    /// so the next SYN always meets a listener in order.
    fn tend(&mut self, i: usize) {
        let Some(Some(entry)) = self.entries.get_mut(i) else { return };
        let Kind::Listener(listener) = &mut entry.kind else { return };
        let mut l = core::mem::take(listener);
        let (local, scope) = (entry.local, entry.scope);
        let id = SocketId { index: i as u32, generation: entry.generation };
        // Listening sockets that took a SYN.
        for slot in 0..l.catchers.len() {
            let Some(sock) = l.catchers[slot] else { continue };
            let state = self.tcp(sock).map(|t| t.state());
            if state == Some(State::Listen) {
                continue;
            }
            l.catchers[slot] = None;
            if state.is_some() && l.waiting() < l.backlog && l.opening.try_push(sock).is_ok() {
                continue;
            }
            // Over the backlog (WhenFull::Ignore, or both interfaces'
            // sockets took one at once): forgotten before smoltcp sent
            // its SYN-ACK, as if the SYN never came.
            let remote = self.tcp(sock).and_then(|t| t.remote_endpoint()).map(Addr::from_endpoint);
            self.remove(sock);
            net_event("tcp syn ignored", id, local, remote);
        }
        // Connections being opened.
        let mut n = 0;
        while n < l.opening.len() {
            let sock = l.opening[n];
            match self.tcp(sock).map(|t| t.state()) {
                Some(State::SynReceived) => n += 1,
                Some(State::Established | State::CloseWait) => {
                    if l.ready.try_push((l.next_sequence, sock)).is_err() {
                        // No memory to queue it: it stays here, counted
                        // in the backlog, until the next look.
                        n += 1;
                        continue;
                    }
                    l.next_sequence = l.next_sequence.wrapping_add(1);
                    l.opening.remove(n);
                }
                // Back to LISTEN (a reset during the handshake), closed,
                // or gone: no connection came of it.
                _ => {
                    l.opening.remove(n);
                    self.remove(sock);
                }
            }
        }
        // Listening sockets where there is room.
        let room = l.waiting() < l.backlog || l.when_full == WhenFull::Ignore;
        for &side in self.sides_for(local.ip) {
            let slot = side.index();
            match l.catchers[slot] {
                None if room && self.side_address(side, local.ip, scope).is_some() => {
                    // On failure (the limit, memory: logged) the interface
                    // refuses connections until a later look finds room.
                    l.catchers[slot] = self.listening_socket(side, local, scope).ok();
                }
                Some(sock) if !room => {
                    self.remove(sock);
                    l.catchers[slot] = None;
                }
                _ => {}
            }
        }
        let full = l.waiting() >= l.backlog;
        if full && !l.full {
            net_event("tcp backlog full", id, local, None);
        }
        l.full = full;
        if let Some(Some(Entry { kind: Kind::Listener(listener), .. })) = self.entries.get_mut(i) {
            *listener = l;
        }
    }

    /// Removes lingering sockets that have finished closing.
    fn reap(&mut self) {
        let mut i = 0;
        while i < self.lingering.len() {
            let sock = self.lingering[i];
            let done =
                self.tcp(sock).is_none_or(|s| matches!(s.state(), State::Closed | State::TimeWait));
            if done {
                self.lingering.swap_remove(i);
                self.remove(sock);
            } else {
                i += 1;
            }
        }
    }

    /// A summary of what an entry's owner could do now; a change wakes it.
    fn signature(&mut self, index: usize) -> u64 {
        let Some(entry) = self.entries[index].as_ref() else { return 0 };
        match entry.kind {
            Kind::Bound => 1,
            Kind::Listener(ref l) => 2 | (l.ready.len() as u64) << 8,
            Kind::Conn { sock, .. } => match self.tcp(sock) {
                Some(s) => {
                    3 | (s.state() as u64) << 4
                        | (s.recv_queue() as u64) << 12
                        | (s.send_queue() as u64) << 36
                }
                None => 0,
            },
            Kind::Udp(_) => super::udp::signature(self, index),
        }
    }

    /// One interface's turn: every received packet, each followed by the
    /// listeners' `tend` (so a SYN that a listening socket took is
    /// replaced before the next packet), then what the sockets send.
    /// Whether anything happened.
    fn poll_side(&mut self, side: Side) -> bool {
        let now = now();
        let mut busy = false;
        loop {
            let took = match side {
                Side::Lo => self.lo.ingress_one(now),
                Side::Eth => self.eth.as_mut().is_some_and(|e| e.ingress_one(now)),
            };
            if !took {
                break;
            }
            busy = true;
            self.tend_all();
        }
        busy |= match side {
            Side::Lo => self.lo.egress(now),
            Side::Eth => self.eth.as_mut().is_some_and(|e| e.egress(now)),
        };
        busy
    }

    /// Polls both interfaces; returns the tokens of changed entries.
    fn poll(&mut self, changed: &mut Vec<u64>) {
        POLLS.fetch_add(1, Ordering::Relaxed);
        // Loopback traffic answers itself: poll until quiet (bounded).
        for _ in 0..64 {
            let mut busy = self.poll_side(Side::Lo);
            if self.eth.is_some() {
                busy |= self.poll_side(Side::Eth);
            }
            if !busy {
                break;
            }
        }
        // What timers did (a handshake given up, a socket closed).
        self.tend_all();
        self.reap();
        super::udp::drain(self);
        // Room for every entry's report (the table grows with the sockets).
        let _ = changed.try_reserve(self.entries.len());
        for i in 0..self.entries.len() {
            let signature = self.signature(i);
            if let Some(entry) = self.entries[i].as_mut()
                && entry.last != signature
            {
                // Recorded only once reported: without memory for the
                // report, the next poll reports it.
                if changed.try_push(entry.token).is_ok() {
                    entry.last = signature;
                }
            }
        }
    }

    fn poll_delay_ms(&mut self) -> Option<u64> {
        let now = now();
        let lo = self.lo.iface.poll_delay(now, &self.lo.sockets);
        // The card has no interrupts: poll it every millisecond.
        let eth = self.eth.as_ref().map(|_| smoltcp::time::Duration::from_millis(1));
        match (lo, eth) {
            (Some(a), Some(b)) => Some(a.min(b).total_millis()),
            (a, b) => a.or(b).map(|d| d.total_millis()),
        }
    }
}

pub(super) fn with_stack<R>(
    f: impl FnOnce(&mut Stack) -> Result<R, NetError>,
) -> Result<R, NetError> {
    let result = with_stack_quiet(f);
    kick();
    result
}

/// `with_stack` for calls that queue nothing (no reason to poll).
pub(super) fn with_stack_quiet<R>(
    f: impl FnOnce(&mut Stack) -> Result<R, NetError>,
) -> Result<R, NetError> {
    match STACK.lock().as_mut() {
        Some(stack) => f(stack),
        None => Err(NetError::Unreachable),
    }
}

/// Wakes the net thread to poll now (something was queued).
pub fn kick() {
    KICKED.store(true, Ordering::Release);
    let id = NET_THREAD.load(Ordering::Acquire);
    if id != u64::MAX {
        sched::wake(sched::ThreadId::from_raw(id as usize));
    }
}

/// The card's IPv4 address, prefix length and MAC, if there is a card.
pub fn ethernet() -> Option<([u8; 4], u8, [u8; 6])> {
    let stack = STACK.lock();
    let s = stack.as_ref()?;
    s.eth_ip.map(|ip| (ip, super::PREFIX, s.eth_mac))
}

/// Sets the function told about readiness changes (with the token given
/// when the socket was made).
pub fn set_notify(hook: fn(u64)) {
    *NOTIFY.lock() = Some(hook);
}

/// Starts the stack: loopback always, the card if `card` is given, and the
/// `net` thread.
pub fn start(card: Option<VirtioNet>) -> Result<(), NetError> {
    let lo =
        Iface::new(Loopback::new(), HardwareAddress::Ip, IpCidr::new(IpAddress::Ipv4(LOOPBACK), 8))
            .map(|mut lo| {
                // ::1 beside 127.0.0.1 (N3 step 1).
                lo.iface.update_ip_addrs(|addrs| {
                    let _ = addrs.push(IpCidr::new(Ip::V6(Ip::LOOPBACK6).to_smoltcp(), 128));
                });
                lo
            })
            .ok_or(NetError::NoMemory)?;
    let (mut eth, mut eth_ip, mut eth_mac) = (None, None, [0; 6]);
    if let Some(card) = card {
        eth_mac = card.mac;
        let mac = smoltcp::wire::EthernetAddress(card.mac);
        let mut iface =
            Iface::new(card, mac.into(), IpCidr::new(super::ADDRESS.into(), super::PREFIX))
                .ok_or(NetError::NoMemory)?;
        let _ = iface.iface.routes_mut().add_default_ipv4_route(super::GATEWAY);
        eth = Some(iface);
        eth_ip = Some(super::ADDRESS.octets());
    }
    *STACK.lock() = Some(Stack {
        lo,
        eth,
        eth_ip,
        eth_mac,
        entries: Vec::new(),
        lingering: Vec::new(),
        next_port: *EPHEMERAL.start(),
        next_generation: 1,
        limit: socket_limit(),
        refused_logged: false,
    });
    let id = sched::spawn("net", net_thread, 0).map_err(|_| NetError::NoMemory)?;
    NET_THREAD.store(id.as_usize() as u64, Ordering::Release);
    Ok(())
}

fn net_thread(_: usize) {
    let mut changed: Vec<u64> = Vec::new();
    if changed.try_reserve(MIN_SOCKETS * 2).is_err() {
        crate::warn!("net: out of memory; the network thread stops");
        return;
    }
    loop {
        if FROZEN.load(Ordering::SeqCst) {
            // Stopped for a kernel handover (`stop`): no frame is taken
            // and nothing is acknowledged until `restart` (or the jump).
            PARKED.store(true, Ordering::SeqCst);
            sched::prepare_wait(Some(time::ticks() + (10 * time::TICK_HZ / 1000).max(1)));
            sched::wait();
            continue;
        }
        KICKED.store(false, Ordering::Release);
        changed.clear();
        let delay = {
            let mut guard = STACK.lock();
            let Some(stack) = guard.as_mut() else { return };
            stack.poll(&mut changed);
            stack.poll_delay_ms()
        };
        let hook = *NOTIFY.lock();
        if let Some(hook) = hook {
            for &token in &changed {
                hook(token);
            }
        }
        let deadline = delay.map(|ms| time::ticks() + ms.max(1) * time::TICK_HZ / 1000);
        sched::prepare_wait(deadline);
        if KICKED.load(Ordering::Acquire) {
            // Kicked since the poll started: poll again at once.
            sched::wake(sched::current());
        }
        sched::wait();
    }
}

/// Binds a socket to `local` (port 0: an ephemeral port). `token` is
/// passed to the notify hook.
pub fn bind(local: Addr, token: u64) -> Result<SocketId, NetError> {
    bind_scoped(local, Scope::of(local.ip), token)
}

/// The same for a socket of `scope` (an IPv6 one with or without
/// IPV6_V6ONLY: `nt::afd`).
pub fn bind_scoped(local: Addr, scope: Scope, token: u64) -> Result<SocketId, NetError> {
    with_stack(|s| {
        if !s.may_bind(local.ip) {
            return Err(NetError::AddressNotAvailable);
        }
        let port = match local.port {
            0 => s.ephemeral_port(false, scope)?,
            p if s.port_in_use(p, false, scope) => return Err(NetError::AddressInUse),
            p => p,
        };
        s.insert(token, Addr { ip: local.ip, port }, scope, Kind::Bound)
    })
}

/// Turns a bound socket into a listener that lets up to `backlog`
/// connections wait (1 to MAX_BACKLOG; the personality maps what its
/// programs ask: `nt::afd`, `lx::socket`), and does `when_full` with a
/// request beyond them. A listener already listening is left as it is
/// (Windows keeps the first backlog).
pub fn listen(id: SocketId, backlog: usize, when_full: WhenFull) -> Result<(), NetError> {
    with_stack(|s| {
        let entry = s.entry(id)?;
        match entry.kind {
            Kind::Bound => {}
            Kind::Listener(_) => return Ok(()),
            Kind::Conn { .. } | Kind::Udp(_) => return Err(NetError::InvalidState),
        }
        let backlog = backlog.clamp(1, MAX_BACKLOG);
        entry.kind = Kind::Listener(Listener {
            backlog,
            when_full,
            next_sequence: 1,
            ..Listener::default()
        });
        s.tend(id.index as usize);
        let Kind::Listener(l) = &mut s.entry(id)?.kind else { return Err(NetError::NoSocket) };
        if l.catchers.iter().all(Option::is_none) {
            // Not one listening socket (the limit or memory, logged by
            // `Stack::refused`): the listen fails, the socket stays bound.
            s.entry(id)?.kind = Kind::Bound;
            return Err(NetError::NoMemory);
        }
        Ok(())
    })
}

/// The oldest established connection waiting on a listener: its sequence
/// number and remote address.
pub fn pending(id: SocketId) -> Result<(u32, Addr), NetError> {
    with_stack(|s| {
        let Kind::Listener(l) = &s.entry(id)?.kind else {
            return Err(NetError::InvalidState);
        };
        let &(sequence, sock) = l.ready.first().ok_or(NetError::WouldBlock)?;
        let remote = s.tcp(sock).and_then(|t| t.remote_endpoint()).ok_or(NetError::Reset)?;
        Ok((sequence, Addr::from_endpoint(remote)))
    })
}

/// Takes the pending connection `sequence` off a listener as a new socket.
pub fn accept(id: SocketId, sequence: u32, token: u64) -> Result<SocketId, NetError> {
    with_stack(|s| {
        let Kind::Listener(l) = &mut s.entry(id)?.kind else {
            return Err(NetError::InvalidState);
        };
        let pos = l.ready.iter().position(|&(q, _)| q == sequence).ok_or(NetError::InvalidState)?;
        let (_, sock) = l.ready.remove(pos);
        s.entry(id)?.last = STALE;
        // Room in the backlog: a listening socket again before the next
        // SYN comes (WhenFull::Refuse took them away while it was full).
        s.tend(id.index as usize);
        let local = s.tcp(sock).and_then(|t| t.local_endpoint()).map(Addr::from_endpoint);
        let remote = s.tcp(sock).and_then(|t| t.remote_endpoint()).map(Addr::from_endpoint);
        let local = local.unwrap_or_default();
        match s.insert(token, local, Scope::of(local.ip), Kind::Conn { sock, connected: true }) {
            Ok(new) => {
                net_event("tcp accept", new, local, remote);
                Ok(new)
            }
            Err(e) => {
                s.remove(sock);
                Err(e)
            }
        }
    })
}

/// Starts connecting to `remote` (binding first if needed). Completion:
/// `connected`.
pub fn connect(id: Option<SocketId>, remote: Addr, token: u64) -> Result<SocketId, NetError> {
    let id = match id {
        Some(id) => id,
        // The unspecified address of the remote's family.
        None => bind(Addr { ip: remote.ip.unspecified_like(), port: 0 }, token)?,
    };
    with_stack(|s| {
        let side = s.side_to(remote.ip)?;
        let entry = s.entry(id)?;
        if !matches!(entry.kind, Kind::Bound) {
            return Err(NetError::InvalidState);
        }
        let mut local = entry.local;
        if local.ip.is_unspecified() {
            local.ip = match side {
                Side::Lo => remote.ip.loopback_like(),
                Side::Eth => Ip::V4(s.eth_ip.ok_or(NetError::Unreachable)?),
            };
        }
        let mut socket = s.new_tcp(side)?;
        let context = match side {
            Side::Lo => s.lo.iface.context(),
            Side::Eth => s.eth.as_mut().ok_or(NetError::Unreachable)?.iface.context(),
        };
        socket
            .connect(context, remote.endpoint(), local.endpoint())
            .map_err(|_| NetError::InvalidState)?;
        let sock = s.add(side, socket)?;
        let entry = s.entry(id)?;
        entry.local = local;
        entry.kind = Kind::Conn { sock, connected: false };
        net_event("tcp connect", id, local, Some(remote));
        Ok(id)
    })
}

/// Whether a connection attempt finished: Ok once established,
/// WouldBlock while in progress, Refused if the peer said no.
pub fn connected(id: SocketId) -> Result<(), NetError> {
    with_stack(|s| {
        let Kind::Conn { sock, connected } = s.entry(id)?.kind else {
            return Err(NetError::InvalidState);
        };
        if connected {
            return Ok(());
        }
        let state = s.tcp(sock).ok_or(NetError::NoSocket)?.state();
        match state {
            State::SynSent | State::SynReceived => Err(NetError::WouldBlock),
            State::Closed => Err(NetError::Refused),
            _ => {
                if let Kind::Conn { connected, .. } = &mut s.entry(id)?.kind {
                    *connected = true;
                }
                Ok(())
            }
        }
    })
}

/// Runs `f` on a connection's socket and its byte counts.
fn conn<R>(
    id: SocketId,
    f: impl FnOnce(&mut tcp::Socket<'static>, &mut Bytes) -> Result<R, NetError>,
) -> Result<R, NetError> {
    with_stack(|s| {
        let entry = s.entry(id)?;
        let Kind::Conn { sock, connected } = entry.kind else {
            return Err(NetError::NotConnected);
        };
        // What the owner does to the socket is not seen by the net thread:
        // the next poll must report the socket whatever it finds.
        entry.last = STALE;
        let mut bytes = entry.bytes;
        let socket = s.tcp(sock).ok_or(NetError::NoSocket)?;
        if !connected && matches!(socket.state(), State::SynSent | State::SynReceived) {
            return Err(NetError::NotConnected);
        }
        let r = f(socket, &mut bytes);
        s.entry(id)?.bytes = bytes;
        r
    })
}

/// Queues bytes for sending; returns how many fit (WouldBlock if none).
pub fn send(id: SocketId, data: &[u8]) -> Result<usize, NetError> {
    conn(id, |t, bytes| {
        if !t.may_send() {
            return Err(if t.state() == State::Closed {
                NetError::Reset
            } else {
                NetError::InvalidState
            });
        }
        match t.send_slice(data) {
            Ok(0) if !data.is_empty() => Err(NetError::WouldBlock),
            Ok(n) => {
                bytes.sent += n as u64;
                Ok(n)
            }
            Err(_) => Err(NetError::InvalidState),
        }
    })
}

/// Takes received bytes; Ok(0) at end of stream, WouldBlock if none yet.
pub fn recv(id: SocketId, out: &mut [u8]) -> Result<usize, NetError> {
    conn(id, |t, bytes| {
        if t.can_recv() {
            let n = t.recv_slice(out).map_err(|_| NetError::Reset)?;
            bytes.received += n as u64;
            return Ok(n);
        }
        if t.may_recv() {
            return Err(NetError::WouldBlock);
        }
        // Nothing buffered and nothing more coming: end of stream.
        // TODO: tell a reset (WSAECONNRESET on Windows) from a FIN; smoltcp
        // ends both in CLOSED.
        Ok(0)
    })
}

/// `recv` that leaves the bytes in the socket (MSG_PEEK).
pub fn peek(id: SocketId, out: &mut [u8]) -> Result<usize, NetError> {
    conn(id, |t, _| {
        if t.can_recv() {
            return t.peek_slice(out).map_err(|_| NetError::Reset);
        }
        if t.may_recv() { Err(NetError::WouldBlock) } else { Ok(0) }
    })
}

/// Stops sending (FIN after the queued bytes); `events=net` "tcp
/// shutdown" with the byte counts (`bytes_event`).
pub fn shutdown_send(id: SocketId) -> Result<(), NetError> {
    let (local, remote, bytes) = conn(id, |t, bytes| {
        t.close();
        let local = t.local_endpoint().map(Addr::from_endpoint).unwrap_or_default();
        let remote = t.remote_endpoint().map(Addr::from_endpoint);
        let b = Bytes { unsent: t.send_queue(), unread: t.recv_queue(), ..*bytes };
        Ok((local, remote, b))
    })?;
    bytes_event("tcp shutdown", id, local, remote, bytes);
    Ok(())
}

pub fn local(id: SocketId) -> Result<Addr, NetError> {
    with_stack(|s| Ok(s.entry(id)?.local))
}

pub fn remote(id: SocketId) -> Result<Addr, NetError> {
    with_stack(|s| {
        let sock = match &s.entry(id)?.kind {
            Kind::Conn { sock, .. } => *sock,
            Kind::Udp(u) => return u.peer.ok_or(NetError::NotConnected),
            _ => return Err(NetError::NotConnected),
        };
        let t = s.tcp(sock).ok_or(NetError::NoSocket)?;
        t.remote_endpoint().map(Addr::from_endpoint).ok_or(NetError::NotConnected)
    })
}

/// Closes a socket: gracefully (FIN, data already queued is still sent),
/// or with `abort` by reset. Pending connections of a listener are reset.
pub fn close(id: SocketId, abort: bool) {
    let _ = with_stack(|s| {
        let entry = s.entries.get_mut(id.index as usize).and_then(Option::take);
        let Some(entry) = entry.filter(|e| e.generation == id.generation) else {
            return Ok(());
        };
        if let Kind::Conn { sock, .. } = entry.kind {
            let t = s.tcp(sock);
            let remote = t.as_ref().and_then(|t| t.remote_endpoint()).map(Addr::from_endpoint);
            let bytes = Bytes {
                unsent: t.as_ref().map_or(0, |t| t.send_queue()),
                unread: t.as_ref().map_or(0, |t| t.recv_queue()),
                ..entry.bytes
            };
            let what = if abort { "tcp reset" } else { "tcp close" };
            bytes_event(what, id, entry.local, remote, bytes);
        }
        let finish = |s: &mut Stack, sock: Sock, abort: bool| {
            if let Some(t) = s.tcp(sock) {
                if abort { t.abort() } else { t.close() }
            }
            if s.lingering.try_push(sock).is_err() {
                s.remove(sock);
            }
        };
        match entry.kind {
            Kind::Bound => {}
            Kind::Listener(l) => {
                for sock in l.catchers.into_iter().flatten() {
                    s.remove(sock);
                }
                // Connections not accepted are reset (half-open ones too).
                for sock in l.opening.into_iter().chain(l.ready.into_iter().map(|(_, sock)| sock)) {
                    finish(s, sock, true);
                }
            }
            Kind::Conn { sock, .. } => finish(s, sock, abort),
            Kind::Udp(u) => {
                for sock in u.socks.into_iter().flatten() {
                    s.remove(sock);
                }
            }
        }
        Ok(())
    });
}

/// What a socket's owner could do now (for select/poll).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Readiness {
    /// Bytes to receive.
    pub readable: bool,
    /// Room to send.
    pub writable: bool,
    /// The peer closed its side (or the connection is gone).
    pub hangup: bool,
    /// A listener has a connection to accept.
    pub acceptable: bool,
    /// A connection is established.
    pub connected: bool,
    /// A connection attempt failed.
    pub refused: bool,
    /// Bytes buffered to receive (a connection), connections waiting to
    /// be accepted (a listener): Linux's epoll tells new arrivals by them
    /// (EPOLLET, `lx::socket::arrivals`).
    pub queued: usize,
}

pub fn readiness(id: SocketId) -> Result<Readiness, NetError> {
    with_stack_quiet(|s| {
        let entry = s.entry(id)?;
        Ok(match entry.kind {
            Kind::Bound => Readiness::default(),
            Kind::Listener(ref l) => Readiness {
                acceptable: !l.ready.is_empty(),
                queued: l.ready.len(),
                ..Readiness::default()
            },
            Kind::Conn { sock, connected } => {
                let t = s.tcp(sock).ok_or(NetError::NoSocket)?;
                let state = t.state();
                let opening = matches!(state, State::SynSent | State::SynReceived);
                let refused = !connected && state == State::Closed;
                Readiness {
                    readable: t.can_recv(),
                    writable: t.may_send() && t.can_send(),
                    hangup: !opening && !refused && !t.may_recv(),
                    acceptable: false,
                    connected: !opening && !refused,
                    refused,
                    queued: t.recv_queue(),
                }
            }
            // Datagrams: readable while one is queued; always writable (a
            // send that finds the queue full waits, as on Windows).
            Kind::Udp(ref u) => {
                Readiness { readable: !u.queue.is_empty(), writable: true, ..Readiness::default() }
            }
        })
    })
}

/// Counters for the log: (polls, loopback packets, loopback drops,
/// sockets in use).
pub fn stats() -> (u64, u64, u64, usize) {
    let guard = STACK.lock();
    let Some(s) = guard.as_ref() else { return (0, 0, 0, 0) };
    let sockets = s.lo.count + s.eth.as_ref().map_or(0, |e| e.count);
    (POLLS.load(Ordering::Relaxed), s.lo.dev.packets, s.lo.dev.dropped, sockets)
}

// Kernel handover (ROADMAP R9 (e), ARCHITECTURE "Sockets across a
// handover"): the stack is stopped, every socket is written down in gala's
// own terms (`ConnState`, never smoltcp's memory layout) and the next
// kernel rebuilds the smoltcp sockets from that (through the `carried` /
// `restore` patch of the vendored smoltcp, THIRD_PARTY.md).

/// TCP states as the handover record numbers them (RFC 793's names).
pub mod tcp_state {
    pub const CLOSED: u32 = 0;
    pub const LISTEN: u32 = 1;
    pub const SYN_SENT: u32 = 2;
    pub const SYN_RECEIVED: u32 = 3;
    pub const ESTABLISHED: u32 = 4;
    pub const FIN_WAIT_1: u32 = 5;
    pub const FIN_WAIT_2: u32 = 6;
    pub const CLOSE_WAIT: u32 = 7;
    pub const CLOSING: u32 = 8;
    pub const LAST_ACK: u32 = 9;
    pub const TIME_WAIT: u32 = 10;
}

/// A connection's timer as the handover record numbers it.
pub mod timer_kind {
    /// Nothing in flight (a keep-alive deadline, maybe).
    pub const IDLE: u32 = 0;
    /// Bytes in flight: retransmit at the deadline (then double the delay).
    pub const RETRANSMIT: u32 = 1;
    /// Duplicate acknowledgements came: retransmit at once.
    pub const FAST_RETRANSMIT: u32 = 2;
    /// TIME-WAIT: gone at the deadline.
    pub const CLOSE: u32 = 3;
}

/// A TCP connection in gala's own terms, as a kernel handover carries it
/// (sequence numbers, windows, options as negotiated, the timer's absolute
/// deadline on the kernel's monotonic clock, the socket's own options and
/// its buffers' sizes). The queued bytes are carried next to it
/// (`CarriedConn`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ConnState {
    /// On the card's interface (else loopback).
    pub on_card: bool,
    /// `tcp_state`.
    pub state: u32,
    /// The connection's endpoints (None while listening or closed).
    pub tuple: Option<(Addr, Addr)>,
    /// The address it listened on (port 0 and 0.0.0.0: none).
    pub listen: Addr,
    /// SND.UNA: the first byte of the send queue.
    pub send_unacked: u32,
    /// SND.NXT - 1: the last sequence number sent.
    pub send_last: u32,
    /// The sequence number of the first unread byte of the receive queue
    /// (RCV.NXT is it plus the queue's length).
    pub recv_first: u32,
    /// The last acknowledgement number sent.
    pub ack_sent: Option<u32>,
    /// The last window sent (as sent, scaled down).
    pub window_sent: u16,
    /// The window scale this side advertised.
    pub window_shift: u8,
    /// The peer's window, in bytes from SND.UNA.
    pub peer_window: u32,
    /// The peer's window scale (None: it did not offer scaling).
    pub peer_window_scale: Option<u8>,
    /// The peer allows selective acknowledgements.
    pub peer_sack: bool,
    /// The peer's maximum segment size.
    pub peer_mss: u32,
    /// When the last segment came (monotonic ns).
    pub last_received_ns: Option<u64>,
    pub fin_received: bool,
    /// `timer_kind`, its deadline (monotonic ns) and its current delay.
    pub timer: u32,
    pub timer_at_ns: u64,
    pub timer_delay_ns: u64,
    /// The round-trip estimate and its deviation (ms).
    pub rtt_ms: u32,
    pub rtt_dev_ms: u32,
    /// The peer's last timestamp value (0: timestamps not used).
    pub peer_tsval: u32,
    /// Socket options: Nagle's algorithm, delayed ACKs, keep-alive and
    /// idle timeout (ms), hop limit.
    pub nagle: bool,
    pub ack_delay_ms: Option<u32>,
    pub keep_alive_ms: Option<u32>,
    pub timeout_ms: Option<u32>,
    pub hop_limit: Option<u8>,
    /// The buffers' sizes (the receive window was advertised from it).
    pub recv_capacity: u32,
    pub send_capacity: u32,
}

/// A connection with its queued bytes: received and not read; sent and
/// not acknowledged, then not sent yet.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct CarriedConn {
    pub state: ConnState,
    pub received: Vec<u8>,
    pub unsent: Vec<u8>,
}

/// What a socket entry is, carried.
#[derive(Debug, PartialEq, Eq)]
pub enum CarriedKind {
    Bound,
    /// A listener: its backlog, the next sequence number, the established
    /// connections not accepted yet (with their sequence numbers) and the
    /// listening sockets that are not plain LISTEN (half-open, or
    /// established while the backlog was full).
    Listener {
        backlog: u32,
        next_sequence: u32,
        ready: Vec<(u32, CarriedConn)>,
        opening: Vec<CarriedConn>,
    },
    Conn {
        conn: CarriedConn,
        connected: bool,
    },
    /// A UDP socket: its default peer (connect) and the datagrams
    /// received and not read, with their senders.
    Udp {
        peer: Option<Addr>,
        datagrams: Vec<(Addr, Vec<u8>)>,
    },
}

/// One socket of the kernel's API, carried.
#[derive(Debug, PartialEq, Eq)]
pub struct CarriedEntry {
    pub local: Addr,
    pub kind: CarriedKind,
}

/// The card's address configuration (gala's static one stands for a
/// DHCP lease: the next kernel keeps it).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CardConfig {
    pub ip: [u8; 4],
    pub prefix: u8,
    pub gateway: [u8; 4],
    pub mac: [u8; 6],
}

/// The stack's own state, carried.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct CarriedStack {
    pub card: Option<CardConfig>,
    /// The next ephemeral port to try.
    pub next_port: u16,
    /// When the stack stopped (monotonic ns): the next kernel says how
    /// long the network was gone.
    pub stopped_ns: u64,
    /// Connections their owner closed, still finishing (FIN, TIME-WAIT).
    pub lingering: Vec<CarriedConn>,
}

fn state_number(s: State) -> u32 {
    use tcp_state::*;
    match s {
        State::Closed => CLOSED,
        State::Listen => LISTEN,
        State::SynSent => SYN_SENT,
        State::SynReceived => SYN_RECEIVED,
        State::Established => ESTABLISHED,
        State::FinWait1 => FIN_WAIT_1,
        State::FinWait2 => FIN_WAIT_2,
        State::CloseWait => CLOSE_WAIT,
        State::Closing => CLOSING,
        State::LastAck => LAST_ACK,
        State::TimeWait => TIME_WAIT,
    }
}

fn state_of(n: u32) -> Option<State> {
    use tcp_state::*;
    Some(match n {
        CLOSED => State::Closed,
        LISTEN => State::Listen,
        SYN_SENT => State::SynSent,
        SYN_RECEIVED => State::SynReceived,
        ESTABLISHED => State::Established,
        FIN_WAIT_1 => State::FinWait1,
        FIN_WAIT_2 => State::FinWait2,
        CLOSE_WAIT => State::CloseWait,
        CLOSING => State::Closing,
        LAST_ACK => State::LastAck,
        TIME_WAIT => State::TimeWait,
        _ => return None,
    })
}

fn ns_of(i: smoltcp::time::Instant) -> u64 {
    (i.total_micros().max(0) as u64).saturating_mul(1000)
}

fn instant_of(ns: u64) -> smoltcp::time::Instant {
    smoltcp::time::Instant::from_micros((ns / 1000) as i64)
}

fn ms_of(d: Option<smoltcp::time::Duration>) -> Option<u32> {
    d.map(|d| d.total_millis().min(u64::from(u32::MAX)) as u32)
}

fn duration_of(ms: Option<u32>) -> Option<smoltcp::time::Duration> {
    ms.map(|ms| smoltcp::time::Duration::from_millis(u64::from(ms)))
}

fn ip_endpoint(a: Addr) -> IpEndpoint {
    a.endpoint()
}

/// A smoltcp socket in gala's terms.
fn conn_state(on_card: bool, t: &tcp::Socket<'static>) -> ConnState {
    use smoltcp::socket::tcp::CarriedTimer;
    let c = t.carried();
    let (timer, timer_at_ns, timer_delay_ns) = match c.timer {
        CarriedTimer::Idle { keep_alive_at } => {
            (timer_kind::IDLE, keep_alive_at.map_or(0, ns_of), 0)
        }
        CarriedTimer::Retransmit { expires_at, delay } => {
            (timer_kind::RETRANSMIT, ns_of(expires_at), delay.total_micros().saturating_mul(1000))
        }
        CarriedTimer::FastRetransmit => (timer_kind::FAST_RETRANSMIT, 0, 0),
        CarriedTimer::Close { expires_at } => (timer_kind::CLOSE, ns_of(expires_at), 0),
    };
    let listen = Addr {
        ip: c.listen_endpoint.addr.map_or(Ip::V4([0; 4]), Ip::from_smoltcp),
        port: c.listen_endpoint.port,
    };
    ConnState {
        on_card,
        state: state_number(c.state),
        tuple: c.local.zip(c.remote).map(|(l, r)| (Addr::from_endpoint(l), Addr::from_endpoint(r))),
        listen,
        send_unacked: c.local_seq_no.0 as u32,
        send_last: c.remote_last_seq.0 as u32,
        recv_first: c.remote_seq_no.0 as u32,
        ack_sent: c.remote_last_ack.map(|a| a.0 as u32),
        window_sent: c.remote_last_win,
        window_shift: c.remote_win_shift,
        peer_window: c.remote_win_len.min(u32::MAX as usize) as u32,
        peer_window_scale: c.remote_win_scale,
        peer_sack: c.remote_has_sack,
        peer_mss: c.remote_mss.min(u32::MAX as usize) as u32,
        last_received_ns: c.remote_last_ts.map(ns_of),
        fin_received: c.rx_fin_received,
        timer,
        timer_at_ns,
        timer_delay_ns,
        rtt_ms: c.rtt,
        rtt_dev_ms: c.rtt_deviation,
        peer_tsval: c.last_remote_tsval,
        nagle: t.nagle_enabled(),
        ack_delay_ms: ms_of(t.ack_delay()),
        keep_alive_ms: ms_of(t.keep_alive()),
        timeout_ms: ms_of(t.timeout()),
        hop_limit: t.hop_limit(),
        recv_capacity: t.recv_capacity().min(u32::MAX as usize) as u32,
        send_capacity: t.send_capacity().min(u32::MAX as usize) as u32,
    }
}

/// gala's description back in smoltcp's terms.
fn smoltcp_state(s: &ConnState) -> Result<smoltcp::socket::tcp::Carried, NetError> {
    use smoltcp::socket::tcp::{Carried, CarriedTimer};
    use smoltcp::wire::TcpSeqNumber;
    let timer = match s.timer {
        timer_kind::IDLE => CarriedTimer::Idle {
            keep_alive_at: (s.timer_at_ns != 0).then(|| instant_of(s.timer_at_ns)),
        },
        timer_kind::RETRANSMIT => CarriedTimer::Retransmit {
            expires_at: instant_of(s.timer_at_ns),
            delay: smoltcp::time::Duration::from_micros(s.timer_delay_ns / 1000),
        },
        timer_kind::FAST_RETRANSMIT => CarriedTimer::FastRetransmit,
        timer_kind::CLOSE => CarriedTimer::Close { expires_at: instant_of(s.timer_at_ns) },
        _ => return Err(NetError::InvalidState),
    };
    let listen = IpListenEndpoint { addr: s.listen.ip.listen_addr(), port: s.listen.port };
    Ok(Carried {
        state: state_of(s.state).ok_or(NetError::InvalidState)?,
        listen_endpoint: listen,
        local: s.tuple.map(|(l, _)| ip_endpoint(l)),
        remote: s.tuple.map(|(_, r)| ip_endpoint(r)),
        local_seq_no: TcpSeqNumber(s.send_unacked as i32),
        remote_seq_no: TcpSeqNumber(s.recv_first as i32),
        remote_last_seq: TcpSeqNumber(s.send_last as i32),
        remote_last_ack: s.ack_sent.map(|a| TcpSeqNumber(a as i32)),
        remote_last_win: s.window_sent,
        remote_win_shift: s.window_shift,
        remote_win_len: s.peer_window as usize,
        remote_win_scale: s.peer_window_scale,
        remote_has_sack: s.peer_sack,
        remote_mss: s.peer_mss as usize,
        remote_last_ts: s.last_received_ns.map(instant_of),
        rx_fin_received: s.fin_received,
        timer,
        rtt: s.rtt_ms,
        rtt_deviation: s.rtt_dev_ms,
        last_remote_tsval: s.peer_tsval,
    })
}

fn queued(t: &tcp::Socket<'static>, send: bool) -> Result<Vec<u8>, NetError> {
    let len = if send { t.send_queue() } else { t.recv_queue() };
    let mut v = Vec::new();
    v.try_reserve_exact(len).map_err(|_| NetError::NoMemory)?;
    v.resize(len, 0);
    let n = t.queued_bytes(send, 0, &mut v);
    v.truncate(n);
    Ok(v)
}

impl Stack {
    fn carry_conn(&mut self, sock: Sock) -> Result<CarriedConn, NetError> {
        let t = self.tcp(sock).ok_or(NetError::NoSocket)?;
        Ok(CarriedConn {
            state: conn_state(sock.0 == Side::Eth, t),
            received: queued(t, false)?,
            unsent: queued(t, true)?,
        })
    }

    /// A connection rebuilt from its carried state (added to its set).
    fn adopt_conn(&mut self, c: &CarriedConn) -> Result<Sock, NetError> {
        let s = &c.state;
        if s.recv_capacity as usize > BUFFER || s.send_capacity as usize > BUFFER {
            // A smaller buffer than the window advertised from it.
            return Err(NetError::NoMemory);
        }
        let mut socket = new_socket().ok_or(NetError::NoMemory)?;
        socket.set_nagle_enabled(s.nagle);
        socket.set_ack_delay(duration_of(s.ack_delay_ms));
        socket.set_keep_alive(duration_of(s.keep_alive_ms));
        socket.set_timeout(duration_of(s.timeout_ms));
        socket.set_hop_limit(s.hop_limit);
        socket
            .restore(&smoltcp_state(s)?, &c.received, &c.unsent)
            .map_err(|_| NetError::InvalidState)?;
        self.add(if s.on_card { Side::Eth } else { Side::Lo }, socket)
    }
}

/// Stops the stack for a kernel handover: the net thread finishes its
/// poll (and the notify hooks it calls) and then polls nothing, so no
/// frame is taken and nothing is acknowledged that the record would not
/// hold. False if it did not stop within `ms`. `restart` undoes it (a
/// refused handover).
pub fn stop(ms: u64) -> bool {
    PARKED.store(false, Ordering::SeqCst);
    FROZEN.store(true, Ordering::SeqCst);
    if NET_THREAD.load(Ordering::Acquire) == u64::MAX {
        return true;
    }
    kick();
    let until = time::monotonic_ns() + ms * 1_000_000;
    while !PARKED.load(Ordering::SeqCst) {
        if time::monotonic_ns() >= until {
            return false;
        }
        sched::sleep_ms(1);
    }
    true
}

/// Lets the net thread poll again after `stop`.
pub fn restart() {
    PARKED.store(false, Ordering::SeqCst);
    FROZEN.store(false, Ordering::SeqCst);
    kick();
}

/// Socket `id` as carried (the stack stopped).
pub fn carry_entry(id: SocketId) -> Result<CarriedEntry, NetError> {
    with_stack_quiet(|s| {
        let entry = s.entry(id)?;
        let local = entry.local;
        let kind = match &entry.kind {
            Kind::Bound => CarriedKind::Bound,
            Kind::Conn { sock, connected } => {
                let (sock, connected) = (*sock, *connected);
                CarriedKind::Conn { conn: s.carry_conn(sock)?, connected }
            }
            Kind::Udp(_) => super::udp::carry(s, id)?,
            Kind::Listener(l) => {
                let (backlog, next_sequence) = (l.backlog as u32, l.next_sequence);
                // The listening sockets are not carried: the next kernel
                // makes its own (`tend`).
                let (mut ready_socks, mut opening_socks) = (Vec::new(), Vec::new());
                ready_socks.try_reserve(l.ready.len()).map_err(|_| NetError::NoMemory)?;
                opening_socks.try_reserve(l.opening.len()).map_err(|_| NetError::NoMemory)?;
                ready_socks.extend(l.ready.iter().copied());
                opening_socks.extend(l.opening.iter().copied());
                let (mut ready, mut opening) = (Vec::new(), Vec::new());
                for (q, sock) in ready_socks {
                    ready.try_push((q, s.carry_conn(sock)?)).map_err(|_| NetError::NoMemory)?;
                }
                for sock in opening_socks {
                    opening.try_push(s.carry_conn(sock)?).map_err(|_| NetError::NoMemory)?;
                }
                CarriedKind::Listener { backlog, next_sequence, ready, opening }
            }
        };
        Ok(CarriedEntry { local, kind })
    })
}

/// The stack's own state (the stack stopped): the card's configuration,
/// the ephemeral port counter, the closing connections.
pub fn carry_stack(stopped_ns: u64) -> Result<CarriedStack, NetError> {
    with_stack_quiet(|s| {
        let card = s.eth_ip.map(|ip| CardConfig {
            ip,
            prefix: super::PREFIX,
            gateway: super::GATEWAY.octets(),
            mac: s.eth_mac,
        });
        let mut lingering = Vec::new();
        for i in 0..s.lingering.len() {
            let sock = s.lingering[i];
            let closed =
                s.tcp(sock).is_none_or(|t| matches!(t.state(), State::Closed | State::Listen));
            if !closed {
                let c = s.carry_conn(sock)?;
                lingering.try_push(c).map_err(|_| NetError::NoMemory)?;
            }
        }
        Ok(CarriedStack { card, next_port: s.next_port, stopped_ns, lingering })
    })
}

/// In the next kernel, before the carried sockets: the card's carried
/// configuration (its address kept), the port counter, the closing
/// connections.
pub fn adopt_stack(c: &CarriedStack) -> Result<(), NetError> {
    with_stack(|s| {
        if c.next_port != 0 {
            s.next_port = c.next_port.clamp(*EPHEMERAL.start(), *EPHEMERAL.end());
        }
        if let Some(card) = c.card {
            let eth = s.eth.as_mut().ok_or(NetError::Unreachable)?;
            if card.mac != s.eth_mac {
                warn!("net: the card's MAC changed across the handover; keeping the address");
            }
            if Some(card.ip) != s.eth_ip {
                let cidr = IpCidr::new(IpAddress::Ipv4(Ipv4Address::from(card.ip)), card.prefix);
                eth.iface.update_ip_addrs(|addrs| {
                    addrs.clear();
                    let _ = addrs.push(cidr);
                });
                eth.iface.routes_mut().remove_default_ipv4_route();
                let _ =
                    eth.iface.routes_mut().add_default_ipv4_route(Ipv4Address::from(card.gateway));
                s.eth_ip = Some(card.ip);
            }
        }
        for conn in &c.lingering {
            let sock = s.adopt_conn(conn)?;
            if s.lingering.try_push(sock).is_err() {
                s.remove(sock);
                return Err(NetError::NoMemory);
            }
        }
        Ok(())
    })
}

/// In the next kernel: socket `c` again, reported to the notify hook with
/// `token`.
pub fn adopt_entry(c: &CarriedEntry, token: u64) -> Result<SocketId, NetError> {
    with_stack(|s| {
        s.entries.try_reserve(1).map_err(|_| NetError::NoMemory)?;
        let kind = match &c.kind {
            CarriedKind::Bound => Kind::Bound,
            CarriedKind::Udp { peer, datagrams } => {
                Kind::Udp(super::udp::adopt(s, c.local, *peer, datagrams)?)
            }
            CarriedKind::Conn { conn, connected } => {
                Kind::Conn { sock: s.adopt_conn(conn)?, connected: *connected }
            }
            CarriedKind::Listener { backlog, next_sequence, ready, opening } => {
                let mut l = Listener {
                    backlog: (*backlog as usize).clamp(1, MAX_BACKLOG),
                    // Only AFD's endpoints are carried (Windows' rule).
                    when_full: WhenFull::Refuse,
                    next_sequence: *next_sequence,
                    ..Listener::default()
                };
                let built = (|| {
                    l.ready.try_reserve(ready.len()).map_err(|_| NetError::NoMemory)?;
                    l.opening.try_reserve(opening.len()).map_err(|_| NetError::NoMemory)?;
                    for (q, conn) in ready {
                        l.ready
                            .try_push((*q, s.adopt_conn(conn)?))
                            .map_err(|_| NetError::NoMemory)?;
                    }
                    for conn in opening {
                        l.opening.try_push(s.adopt_conn(conn)?).map_err(|_| NetError::NoMemory)?;
                    }
                    Ok(())
                })();
                if let Err(e) = built {
                    for &sock in l.opening.iter().chain(l.ready.iter().map(|(_, sock)| sock)) {
                        s.remove(sock);
                    }
                    return Err(e);
                }
                Kind::Listener(l)
            }
        };
        // Room was reserved: this cannot fail.
        let id = s.insert(token, c.local, Scope::of(c.local.ip), kind)?;
        // A listener's listening sockets, where its backlog has room.
        s.tend(id.index as usize);
        Ok(id)
    })
}

/// The stack in host tests (`net::tcp`'s and `net::udp`'s).
#[cfg(test)]
pub(super) mod testing {
    use super::*;

    /// The stack is global: tests that use it take turns.
    static TURN: std::sync::Mutex<()> = std::sync::Mutex::new(());

    pub fn turn() -> std::sync::MutexGuard<'static, ()> {
        TURN.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A stack with only the loopback interface, as `start` makes it.
    pub fn loopback_stack() {
        let lo = Iface::new(
            Loopback::new(),
            HardwareAddress::Ip,
            IpCidr::new(IpAddress::Ipv4(LOOPBACK), 8),
        )
        .unwrap();
        *STACK.lock() = Some(Stack {
            lo,
            eth: None,
            eth_ip: None,
            eth_mac: [0; 6],
            entries: Vec::new(),
            lingering: Vec::new(),
            next_port: *EPHEMERAL.start(),
            next_generation: 1,
            limit: socket_limit(),
            refused_logged: false,
        });
    }

    /// One poll of the net thread: the tokens it would report.
    pub fn poll() -> Vec<u64> {
        let mut changed = Vec::new();
        with_stack_quiet(|s| {
            s.poll(&mut changed);
            Ok(())
        })
        .unwrap();
        changed
    }

    /// The test's stack is gone.
    pub fn end() {
        *STACK.lock() = None;
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{loopback_stack, poll, turn};
    use super::*;

    #[test]
    fn a_reader_is_told_of_the_next_message_of_the_same_size() {
        let _turn = turn();
        loopback_stack();
        let addr = Addr::v4([127, 0, 0, 1], 4000);
        let listener = bind(addr, 1).unwrap();
        listen(listener, 4, WhenFull::Refuse).unwrap();
        let client = connect(None, addr, 2).unwrap();
        for _ in 0..4 {
            poll();
        }
        let (sequence, _) = pending(listener).unwrap();
        let server = accept(listener, sequence, 3).unwrap();
        poll();
        let mut buf = [0u8; 64];
        for round in 0..3 {
            assert_eq!(send(client, &[round; 24]).unwrap(), 24);
            let changed = poll();
            // The server's socket has 24 bytes queued: its owner is told,
            // every round, though the queue looked the same in the last
            // report (24 bytes, consumed since).
            assert!(changed.contains(&3), "round {round}: {changed:?}");
            assert_eq!(recv(server, &mut buf).unwrap(), 24);
            assert_eq!(buf[0], round);
        }
        *STACK.lock() = None;
    }

    /// N3 step 1: a connection over ::1, both ends seeing IPv6 addresses.
    #[test]
    fn connections_over_ipv6_loopback() {
        let _turn = turn();
        loopback_stack();
        let addr = Addr { ip: Ip::V6(Ip::LOOPBACK6), port: 4200 };
        let listener = bind(addr, 1).unwrap();
        listen(listener, 4, WhenFull::Refuse).unwrap();
        let client = connect(None, addr, 2).unwrap();
        polls(4);
        let (sequence, remote) = pending(listener).unwrap();
        assert_eq!(remote.ip, Ip::V6(Ip::LOOPBACK6));
        let server = accept(listener, sequence, 3).unwrap();
        polls(2);
        assert_eq!(local(client).unwrap().ip, Ip::V6(Ip::LOOPBACK6));
        assert_eq!(super::remote(client).unwrap(), addr);
        assert_eq!(send(client, b"over ::1").unwrap(), 8);
        polls(2);
        assert_eq!(recv_all(server, 8), b"over ::1");
        // An IPv6 address off the loopback has no route yet (step 3).
        let far =
            Addr { ip: Ip::V6([0x20, 1, 0xd, 0xb8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]), port: 80 };
        assert_eq!(connect(None, far, 4), Err(NetError::Unreachable));
        *STACK.lock() = None;
    }

    /// More listeners than a fixed share of the stack's sockets allowed
    /// (Nginx's ports at backlog 511): each takes its connection.
    #[test]
    fn many_listeners_each_take_a_connection() {
        let _turn = turn();
        loopback_stack();
        let mut listeners = Vec::new();
        for i in 0..20u16 {
            let addr = Addr::v4([127, 0, 0, 1], 4300 + i);
            let l = bind(addr, 100 + u64::from(i)).unwrap();
            listen(l, 511, WhenFull::Refuse).unwrap();
            listeners.push((l, addr));
        }
        for (i, &(l, addr)) in listeners.iter().enumerate() {
            let client = connect(None, addr, 200 + i as u64).unwrap();
            polls(4);
            assert_eq!(connected(client), Ok(()), "listener {i}");
            let (sequence, _) = pending(l).unwrap();
            let server = accept(l, sequence, 300 + i as u64).unwrap();
            assert_eq!(send(client, b"hi").unwrap(), 2);
            assert_eq!(recv_all(server, 2), b"hi");
        }
        *STACK.lock() = None;
    }

    /// Windows' listen: once `backlog` connections wait unaccepted, the
    /// next is refused (WSAECONNREFUSED); an accept makes room again.
    #[test]
    fn a_full_backlog_refuses_until_an_accept() {
        let _turn = turn();
        loopback_stack();
        let addr = Addr::v4([127, 0, 0, 1], 4400);
        let listener = bind(addr, 1).unwrap();
        listen(listener, 2, WhenFull::Refuse).unwrap();
        let a = connect(None, addr, 2).unwrap();
        let b = connect(None, addr, 3).unwrap();
        polls(4);
        assert_eq!((connected(a), connected(b)), (Ok(()), Ok(())));
        let c = connect(None, addr, 4).unwrap();
        polls(4);
        assert_eq!(connected(c), Err(NetError::Refused));
        let (first, _) = pending(listener).unwrap();
        accept(listener, first, 5).unwrap();
        let d = connect(None, addr, 6).unwrap();
        polls(4);
        assert_eq!(connected(d), Ok(()));
        *STACK.lock() = None;
    }

    /// Linux's way (`lx::socket`): a SYN beyond the backlog goes
    /// unanswered (no refusal); once an accept made room, a SYN gets in.
    #[test]
    fn a_full_backlog_ignores_until_an_accept() {
        let _turn = turn();
        loopback_stack();
        let addr = Addr::v4([127, 0, 0, 1], 4500);
        let listener = bind(addr, 1).unwrap();
        listen(listener, 1, WhenFull::Ignore).unwrap();
        let a = connect(None, addr, 2).unwrap();
        polls(4);
        assert_eq!(connected(a), Ok(()));
        let b = connect(None, addr, 3).unwrap();
        polls(4);
        // Not refused, not connected: its SYN went unanswered.
        assert_eq!(connected(b), Err(NetError::WouldBlock));
        let (first, _) = pending(listener).unwrap();
        accept(listener, first, 4).unwrap();
        // Room again: the next SYN gets in (b's own retransmission needs
        // the clock, which host tests lack; lx_socket boots it).
        let c = connect(None, addr, 5).unwrap();
        polls(4);
        assert_eq!(connected(c), Ok(()));
        assert!(pending(listener).is_ok());
        *STACK.lock() = None;
    }

    /// The socket sets grow past their first slots, and a closed listener
    /// leaves nothing behind.
    #[test]
    fn listeners_hold_one_socket_each_and_free_it() {
        let _turn = turn();
        loopback_stack();
        let before = stats().3;
        let mut ids = Vec::new();
        for i in 0..(INITIAL_SLOTS as u16 + 8) {
            let l = bind(Addr::v4([127, 0, 0, 1], 4600 + i), 1).unwrap();
            listen(l, 511, WhenFull::Refuse).unwrap();
            ids.push(l);
        }
        assert_eq!(stats().3 - before, INITIAL_SLOTS + 8);
        for l in ids {
            close(l, false);
        }
        assert_eq!(stats().3, before);
        *STACK.lock() = None;
    }

    fn polls(n: usize) {
        for _ in 0..n {
            poll();
        }
    }

    fn recv_all(id: SocketId, want: usize) -> Vec<u8> {
        let mut got = Vec::new();
        let mut buf = [0u8; 4096];
        for _ in 0..64 {
            match recv(id, &mut buf) {
                Ok(n) => got.extend_from_slice(&buf[..n]),
                Err(NetError::WouldBlock) => polls(1),
                Err(e) => panic!("recv: {e:?}"),
            }
            if got.len() >= want {
                break;
            }
        }
        got
    }

    /// R9 (e): connections written down in gala's terms and rebuilt in a
    /// new stack go on: bytes received and not read, bytes not sent yet,
    /// a connection waiting to be accepted (with its sequence number), the
    /// listener accepting new ones, a graceful close.
    #[test]
    fn connections_go_on_in_a_new_stack() {
        let _turn = turn();
        loopback_stack();
        let addr = Addr::v4([127, 0, 0, 1], 4100);
        let listener = bind(addr, 1).unwrap();
        listen(listener, 2, WhenFull::Refuse).unwrap();
        let client = connect(None, addr, 2).unwrap();
        polls(4);
        let (sequence, _) = pending(listener).unwrap();
        let server = accept(listener, sequence, 3).unwrap();
        // Received, not read yet.
        assert_eq!(send(client, &[7; 100]).unwrap(), 100);
        polls(4);
        // A second client, established, not accepted.
        let waiting = connect(None, addr, 4).unwrap();
        polls(4);
        assert_eq!(send(waiting, b"early").unwrap(), 5);
        polls(4);
        let (waiting_sequence, _) = pending(listener).unwrap();
        // Queued, not sent yet (no poll before the handover).
        assert_eq!(send(client, &[9; 50]).unwrap(), 50);
        assert_eq!(send(server, b"reply").unwrap(), 5);

        assert!(stop(100));
        let ids = [listener, client, server, waiting];
        let carried: Vec<CarriedEntry> = ids.iter().map(|&id| carry_entry(id).unwrap()).collect();
        let stack = carry_stack(1).unwrap();
        match &carried[0].kind {
            CarriedKind::Listener { backlog: 2, ready, .. } => {
                assert_eq!(ready.len(), 1);
                assert_eq!(ready[0].0, waiting_sequence);
            }
            other => panic!("listener carried as {other:?}"),
        }
        match &carried[2].kind {
            CarriedKind::Conn { conn, connected: true } => {
                assert_eq!(conn.state.state, tcp_state::ESTABLISHED);
                assert_eq!(conn.received.len(), 100);
                assert_eq!(conn.unsent, b"reply");
                assert_eq!(conn.state.tuple.map(|t| t.0), Some(addr));
            }
            other => panic!("server carried as {other:?}"),
        }
        match &carried[1].kind {
            CarriedKind::Conn { conn, .. } => assert_eq!(conn.unsent.len(), 50),
            other => panic!("client carried as {other:?}"),
        }
        restart();

        // The next kernel: a new stack.
        loopback_stack();
        adopt_stack(&stack).unwrap();
        let ids: Vec<SocketId> = carried
            .iter()
            .enumerate()
            .map(|(i, c)| adopt_entry(c, 10 + i as u64).unwrap())
            .collect();
        let [listener, client, server, waiting] = ids[..] else { unreachable!() };
        assert_eq!(local(server).unwrap(), addr);
        assert_eq!(recv_all(server, 150), [[7u8; 100].as_slice(), &[9; 50]].concat());
        assert_eq!(recv_all(client, 5), b"reply");
        // The connection that waited is accepted by its old number.
        let accepted = accept(listener, waiting_sequence, 20).unwrap();
        assert_eq!(recv_all(accepted, 5), b"early");
        assert_eq!(send(accepted, b"late").unwrap(), 4);
        assert_eq!(recv_all(waiting, 4), b"late");
        // And a new one.
        let third = connect(None, addr, 21).unwrap();
        polls(4);
        let (q, _) = pending(listener).unwrap();
        assert!(q > waiting_sequence);
        let fresh = accept(listener, q, 22).unwrap();
        assert_eq!(send(third, b"new").unwrap(), 3);
        assert_eq!(recv_all(fresh, 3), b"new");
        // Graceful close of a carried connection.
        close(client, false);
        polls(4);
        let mut buf = [0u8; 8];
        assert_eq!(recv(server, &mut buf), Ok(0));
        *STACK.lock() = None;
    }
}

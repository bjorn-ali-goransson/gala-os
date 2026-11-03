//! The kernel's TCP socket API on smoltcp, independent of NT (`nt::afd`
//! builds Windows sockets on it). Design: docs/ARCHITECTURE.md,
//! "Networking".
//!
//! Two interfaces, each with its own socket set: `lo` (127.0.0.1/8, always)
//! and `eth` (the card, when there is one). Our own table of entries maps
//! `SocketId`s to smoltcp sockets: a bound address, a listener (with its
//! listening and established-but-not-accepted sockets) or a connection.
//! The `net` thread polls; after each poll every entry whose readiness
//! changed is reported to the notify hook with the entry's token.

use super::loopback::Loopback;
use super::virtio_net::VirtioNet;
use crate::mm::heap::TryVec;
use crate::sync::IrqMutex;
// Threads and time through the NT layer's platform, so the stack also runs
// in gala-um (loopback only there).
use crate::nt::platform as sched;
use crate::nt::platform as time;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use smoltcp::iface::{Config, Interface, SocketHandle, SocketSet, SocketStorage};
use smoltcp::phy::Device;
use smoltcp::socket::tcp::{self, State};
use smoltcp::wire::{
    HardwareAddress, IpAddress, IpCidr, IpEndpoint, IpListenEndpoint, Ipv4Address,
};

/// Sockets per interface (smoltcp sockets, including listening ones).
const MAX_SOCKETS: usize = 64;
/// Bytes buffered per direction per connection.
const BUFFER: usize = 64 * 1024;
const MAX_BACKLOG: usize = 8;
const EPHEMERAL: core::ops::RangeInclusive<u16> = 49152..=65535;

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

/// An IPv4 address and port.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Addr {
    pub ip: [u8; 4],
    pub port: u16,
}

impl Addr {
    fn ip(&self) -> Ipv4Address {
        Ipv4Address::from(self.ip)
    }

    fn from_endpoint(e: IpEndpoint) -> Self {
        let IpAddress::Ipv4(ip) = e.addr;
        Self { ip: ip.octets(), port: e.port }
    }
}

/// `"a.b.c.d:port"` (a JSON string, for the event stream).
impl core::fmt::Display for Addr {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let [a, b, c, d] = self.ip;
        write!(f, "\"{a}.{b}.{c}.{d}:{}\"", self.port)
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SocketId {
    pub(super) index: u32,
    generation: u32,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Side {
    Lo,
    Eth,
}

pub(super) type Sock = (Side, SocketHandle);

pub(super) enum Kind {
    Bound,
    Listener {
        backlog: usize,
        listening: Vec<Sock>,
        ready: Vec<(u32, Sock)>,
        next_sequence: u32,
    },
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
    pub(super) kind: Kind,
    /// Readiness at the last report (see `signature`); STALE after the
    /// owner sent, received or accepted (the next poll reports it: a queue
    /// emptied and refilled to the same length looks unchanged).
    pub(super) last: u64,
}

/// No signature: the next poll reports the entry.
pub(super) const STALE: u64 = u64::MAX;

pub(super) struct Iface<D: Device> {
    pub(super) dev: D,
    iface: Interface,
    pub(super) sockets: SocketSet<'static>,
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
        let mut storage = Vec::new();
        storage.try_reserve_exact(MAX_SOCKETS).ok()?;
        for _ in 0..MAX_SOCKETS {
            storage.try_push(SocketStorage::EMPTY).ok()?;
        }
        Some(Self { dev, iface, sockets: SocketSet::new(storage), count: 0 })
    }

    fn poll(&mut self) -> bool {
        self.iface.poll(now(), &mut self.dev, &mut self.sockets) != smoltcp::iface::PollResult::None
    }
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

    pub(super) fn add<S: smoltcp::socket::AnySocket<'static>>(
        &mut self,
        side: Side,
        socket: S,
    ) -> Result<Sock, NetError> {
        let count = match side {
            Side::Lo => &mut self.lo.count,
            Side::Eth => &mut self.eth.as_mut().ok_or(NetError::Unreachable)?.count,
        };
        if *count == MAX_SOCKETS {
            return Err(NetError::NoMemory);
        }
        *count += 1;
        let handle = self.sockets(side).ok_or(NetError::Unreachable)?.add(socket);
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
    pub(super) fn port_in_use(&self, port: u16, udp: bool) -> bool {
        self.entries.iter().flatten().any(|e| e.local.port == port && e.kind.is_udp() == udp)
    }

    pub(super) fn ephemeral_port(&mut self, udp: bool) -> Result<u16, NetError> {
        for _ in EPHEMERAL {
            let port = self.next_port;
            self.next_port = if port == *EPHEMERAL.end() { *EPHEMERAL.start() } else { port + 1 };
            if !self.port_in_use(port, udp) {
                return Ok(port);
            }
        }
        Err(NetError::AddressInUse)
    }

    pub(super) fn insert(
        &mut self,
        token: u64,
        local: Addr,
        kind: Kind,
    ) -> Result<SocketId, NetError> {
        let generation = self.next_generation;
        self.next_generation = self.next_generation.wrapping_add(1);
        let entry = Entry { generation, token, local, kind, last: 0 };
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
    pub(super) fn sides_for(&self, ip: [u8; 4]) -> &'static [Side] {
        match (ip, self.eth.is_some()) {
            ([0, 0, 0, 0], true) => &[Side::Lo, Side::Eth],
            ([0, 0, 0, 0], false) | ([127, ..], _) => &[Side::Lo],
            _ => &[Side::Eth],
        }
    }

    pub(super) fn side_to(&self, ip: [u8; 4]) -> Result<Side, NetError> {
        match ip {
            [127, ..] => Ok(Side::Lo),
            _ if self.eth.is_some() => Ok(Side::Eth),
            _ => Err(NetError::Unreachable),
        }
    }

    fn listening_socket(&mut self, side: Side, local: Addr) -> Result<Sock, NetError> {
        let mut socket = new_socket().ok_or(NetError::NoMemory)?;
        let endpoint = IpListenEndpoint {
            addr: (local.ip != [0; 4]).then(|| IpAddress::Ipv4(local.ip())),
            port: local.port,
        };
        socket.listen(endpoint).map_err(|_| NetError::InvalidState)?;
        self.add(side, socket)
    }

    /// Moves established connections of every listener to its ready
    /// queue, replacing them with fresh listening sockets.
    fn promote(&mut self) {
        for i in 0..self.entries.len() {
            let mut n = 0;
            // The n-th listening socket of entry i, if any.
            let nth = |entries: &[Option<Entry>], n: usize| match &entries[i] {
                Some(Entry { kind: Kind::Listener { listening, .. }, local, .. }) => {
                    listening.get(n).map(|&sock| (sock, *local))
                }
                _ => None,
            };
            while let Some((sock, local)) = nth(&self.entries, n) {
                let state = self.tcp(sock).map(|t| t.state());
                if !matches!(state, Some(State::Established | State::CloseWait)) {
                    n += 1;
                    continue;
                }
                let fresh = self.listening_socket(sock.0, local);
                let Some(Some(Entry {
                    kind: Kind::Listener { listening, ready, next_sequence, backlog },
                    ..
                })) = self.entries.get_mut(i)
                else {
                    break;
                };
                // A full backlog keeps the connection where it is until
                // the owner accepts (it is established; data waits in it).
                if ready.len() >= *backlog || ready.try_push((*next_sequence, sock)).is_err() {
                    // No room to queue it: leave it listening-side for now.
                    if let Ok(f) = fresh {
                        self.remove(f);
                    }
                    n += 1;
                    continue;
                }
                *next_sequence = next_sequence.wrapping_add(1);
                match fresh {
                    Ok(f) => {
                        listening[n] = f;
                        n += 1;
                    }
                    Err(_) => {
                        listening.swap_remove(n);
                    }
                }
            }
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
            Kind::Listener { ref ready, .. } => 2 | (ready.len() as u64) << 8,
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

    /// Polls both interfaces; returns the tokens of changed entries.
    fn poll(&mut self, changed: &mut Vec<u64>) {
        POLLS.fetch_add(1, Ordering::Relaxed);
        // Loopback traffic answers itself: poll until quiet (bounded).
        for _ in 0..64 {
            let mut busy = self.lo.poll();
            if let Some(eth) = self.eth.as_mut() {
                busy |= eth.poll();
            }
            if !busy {
                break;
            }
        }
        self.promote();
        self.reap();
        super::udp::drain(self);
        for i in 0..self.entries.len() {
            let signature = self.signature(i);
            if let Some(entry) = self.entries[i].as_mut()
                && entry.last != signature
            {
                entry.last = signature;
                let _ = changed.try_push(entry.token);
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
    });
    let id = sched::spawn("net", net_thread, 0).map_err(|_| NetError::NoMemory)?;
    NET_THREAD.store(id.as_usize() as u64, Ordering::Release);
    Ok(())
}

fn net_thread(_: usize) {
    let mut changed: Vec<u64> = Vec::new();
    if changed.try_reserve(MAX_SOCKETS * 2).is_err() {
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
    with_stack(|s| {
        let ok_ip = local.ip == [0; 4] || local.ip[0] == 127 || Some(local.ip) == s.eth_ip;
        if !ok_ip {
            return Err(NetError::AddressNotAvailable);
        }
        let port = match local.port {
            0 => s.ephemeral_port(false)?,
            p if s.port_in_use(p, false) => return Err(NetError::AddressInUse),
            p => p,
        };
        s.insert(token, Addr { ip: local.ip, port }, Kind::Bound)
    })
}

/// Turns a bound socket into a listener.
pub fn listen(id: SocketId, backlog: usize) -> Result<(), NetError> {
    with_stack(|s| {
        let entry = s.entry(id)?;
        match entry.kind {
            Kind::Bound => {}
            Kind::Listener { .. } => return Ok(()),
            Kind::Conn { .. } | Kind::Udp(_) => return Err(NetError::InvalidState),
        }
        let local = entry.local;
        let backlog = backlog.clamp(1, MAX_BACKLOG);
        let mut listening = Vec::new();
        listening.try_reserve(backlog * 2).map_err(|_| NetError::NoMemory)?;
        let mut ready = Vec::new();
        ready.try_reserve(backlog * 2).map_err(|_| NetError::NoMemory)?;
        for &side in s.sides_for(local.ip) {
            for _ in 0..backlog {
                match s.listening_socket(side, local) {
                    Ok(sock) => listening.try_push(sock).map_err(|_| NetError::NoMemory)?,
                    Err(e) => {
                        let (free, total) = crate::mm::frame_stats();
                        warn!(
                            "net: listen on port {}: {e:?} (sockets: loopback {}, card {}; \
                             frames free {free} of {total})",
                            local.port,
                            s.lo.count,
                            s.eth.as_ref().map_or(0, |e| e.count)
                        );
                        for &sock in &listening {
                            s.remove(sock);
                        }
                        return Err(e);
                    }
                }
            }
        }
        s.entry(id)?.kind = Kind::Listener { backlog, listening, ready, next_sequence: 1 };
        Ok(())
    })
}

/// The oldest established connection waiting on a listener: its sequence
/// number and remote address.
pub fn pending(id: SocketId) -> Result<(u32, Addr), NetError> {
    with_stack(|s| {
        let Kind::Listener { ready, .. } = &s.entry(id)?.kind else {
            return Err(NetError::InvalidState);
        };
        let &(sequence, sock) = ready.first().ok_or(NetError::WouldBlock)?;
        let remote = s.tcp(sock).and_then(|t| t.remote_endpoint()).ok_or(NetError::Reset)?;
        Ok((sequence, Addr::from_endpoint(remote)))
    })
}

/// Takes the pending connection `sequence` off a listener as a new socket.
pub fn accept(id: SocketId, sequence: u32, token: u64) -> Result<SocketId, NetError> {
    with_stack(|s| {
        let Kind::Listener { ready, .. } = &mut s.entry(id)?.kind else {
            return Err(NetError::InvalidState);
        };
        let pos = ready.iter().position(|&(q, _)| q == sequence).ok_or(NetError::InvalidState)?;
        let (_, sock) = ready.remove(pos);
        s.entry(id)?.last = STALE;
        let local = s.tcp(sock).and_then(|t| t.local_endpoint()).map(Addr::from_endpoint);
        let remote = s.tcp(sock).and_then(|t| t.remote_endpoint()).map(Addr::from_endpoint);
        match s.insert(token, local.unwrap_or_default(), Kind::Conn { sock, connected: true }) {
            Ok(new) => {
                net_event("tcp accept", new, local.unwrap_or_default(), remote);
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
        None => bind(Addr::default(), token)?,
    };
    with_stack(|s| {
        let side = s.side_to(remote.ip)?;
        let entry = s.entry(id)?;
        if !matches!(entry.kind, Kind::Bound) {
            return Err(NetError::InvalidState);
        }
        let mut local = entry.local;
        if local.ip == [0; 4] {
            local.ip = match side {
                Side::Lo => LOOPBACK.octets(),
                Side::Eth => s.eth_ip.ok_or(NetError::Unreachable)?,
            };
        }
        let mut socket = new_socket().ok_or(NetError::NoMemory)?;
        let context = match side {
            Side::Lo => s.lo.iface.context(),
            Side::Eth => s.eth.as_mut().ok_or(NetError::Unreachable)?.iface.context(),
        };
        socket
            .connect(
                context,
                IpEndpoint::new(IpAddress::Ipv4(remote.ip()), remote.port),
                IpEndpoint::new(IpAddress::Ipv4(local.ip()), local.port),
            )
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

fn conn<R>(
    id: SocketId,
    f: impl FnOnce(&mut tcp::Socket<'static>) -> Result<R, NetError>,
) -> Result<R, NetError> {
    with_stack(|s| {
        let Kind::Conn { sock, connected } = s.entry(id)?.kind else {
            return Err(NetError::NotConnected);
        };
        // What the owner does to the socket is not seen by the net thread:
        // the next poll must report the socket whatever it finds.
        s.entry(id)?.last = STALE;
        let socket = s.tcp(sock).ok_or(NetError::NoSocket)?;
        if !connected && matches!(socket.state(), State::SynSent | State::SynReceived) {
            return Err(NetError::NotConnected);
        }
        f(socket)
    })
}

/// Queues bytes for sending; returns how many fit (WouldBlock if none).
pub fn send(id: SocketId, data: &[u8]) -> Result<usize, NetError> {
    conn(id, |t| {
        if !t.may_send() {
            return Err(if t.state() == State::Closed {
                NetError::Reset
            } else {
                NetError::InvalidState
            });
        }
        match t.send_slice(data) {
            Ok(0) if !data.is_empty() => Err(NetError::WouldBlock),
            Ok(n) => Ok(n),
            Err(_) => Err(NetError::InvalidState),
        }
    })
}

/// Takes received bytes; Ok(0) at end of stream, WouldBlock if none yet.
pub fn recv(id: SocketId, out: &mut [u8]) -> Result<usize, NetError> {
    conn(id, |t| {
        if t.can_recv() {
            return t.recv_slice(out).map_err(|_| NetError::Reset);
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
    conn(id, |t| {
        if t.can_recv() {
            return t.peek_slice(out).map_err(|_| NetError::Reset);
        }
        if t.may_recv() { Err(NetError::WouldBlock) } else { Ok(0) }
    })
}

/// Stops sending (FIN after the queued bytes).
pub fn shutdown_send(id: SocketId) -> Result<(), NetError> {
    conn(id, |t| {
        t.close();
        Ok(())
    })
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
            let remote = s.tcp(sock).and_then(|t| t.remote_endpoint()).map(Addr::from_endpoint);
            net_event(if abort { "tcp reset" } else { "tcp close" }, id, entry.local, remote);
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
            Kind::Listener { listening, ready, .. } => {
                for sock in listening {
                    s.remove(sock);
                }
                for (_, sock) in ready {
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
}

pub fn readiness(id: SocketId) -> Result<Readiness, NetError> {
    with_stack_quiet(|s| {
        let entry = s.entry(id)?;
        Ok(match entry.kind {
            Kind::Bound => Readiness::default(),
            Kind::Listener { ref ready, .. } => {
                Readiness { acceptable: !ready.is_empty(), ..Readiness::default() }
            }
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
    IpEndpoint::new(IpAddress::Ipv4(a.ip()), a.port)
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
        ip: match c.listen_endpoint.addr {
            Some(IpAddress::Ipv4(ip)) => ip.octets(),
            None => [0; 4],
        },
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
    let listen = IpListenEndpoint {
        addr: (s.listen.ip != [0; 4]).then(|| IpAddress::Ipv4(s.listen.ip())),
        port: s.listen.port,
    };
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
            Kind::Listener { backlog, listening, ready, next_sequence } => {
                let (backlog, next_sequence) = (*backlog as u32, *next_sequence);
                let mut socks: Vec<(Option<u32>, Sock)> = Vec::new();
                socks.try_reserve(listening.len() + ready.len()).map_err(|_| NetError::NoMemory)?;
                for &(q, sock) in ready {
                    socks.try_push((Some(q), sock)).map_err(|_| NetError::NoMemory)?;
                }
                for &sock in listening {
                    socks.try_push((None, sock)).map_err(|_| NetError::NoMemory)?;
                }
                let (mut ready, mut opening) = (Vec::new(), Vec::new());
                for (q, sock) in socks {
                    match q {
                        Some(q) => ready.try_push((q, s.carry_conn(sock)?)),
                        None if s.tcp(sock).is_some_and(|t| t.state() == State::Listen) => {
                            continue;
                        }
                        None => opening.try_push(s.carry_conn(sock)?),
                    }
                    .map_err(|_| NetError::NoMemory)?;
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
                let backlog = (*backlog as usize).clamp(1, MAX_BACKLOG);
                let mut listening = Vec::new();
                listening
                    .try_reserve(backlog * 2 + opening.len())
                    .map_err(|_| NetError::NoMemory)?;
                let mut queue = Vec::new();
                queue.try_reserve(backlog * 2 + ready.len()).map_err(|_| NetError::NoMemory)?;
                let built = (|| {
                    for (q, conn) in ready {
                        queue
                            .try_push((*q, s.adopt_conn(conn)?))
                            .map_err(|_| NetError::NoMemory)?;
                    }
                    for conn in opening {
                        listening.try_push(s.adopt_conn(conn)?).map_err(|_| NetError::NoMemory)?;
                    }
                    for &side in s.sides_for(c.local.ip) {
                        let busy = listening.iter().filter(|l| l.0 == side).count();
                        for _ in busy..backlog {
                            let sock = s.listening_socket(side, c.local)?;
                            listening.try_push(sock).map_err(|_| NetError::NoMemory)?;
                        }
                    }
                    Ok(())
                })();
                if let Err(e) = built {
                    for &sock in listening.iter().chain(queue.iter().map(|(_, sock)| sock)) {
                        s.remove(sock);
                    }
                    return Err(e);
                }
                Kind::Listener { backlog, listening, ready: queue, next_sequence: *next_sequence }
            }
        };
        // Room was reserved: this cannot fail.
        s.insert(token, c.local, kind)
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
        let addr = Addr { ip: [127, 0, 0, 1], port: 4000 };
        let listener = bind(addr, 1).unwrap();
        listen(listener, 4).unwrap();
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
        let addr = Addr { ip: [127, 0, 0, 1], port: 4100 };
        let listener = bind(addr, 1).unwrap();
        listen(listener, 2).unwrap();
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

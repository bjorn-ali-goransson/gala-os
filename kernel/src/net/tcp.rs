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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SocketId {
    index: u32,
    generation: u32,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Side {
    Lo,
    Eth,
}

type Sock = (Side, SocketHandle);

enum Kind {
    Bound,
    Listener { backlog: usize, listening: Vec<Sock>, ready: Vec<(u32, Sock)>, next_sequence: u32 },
    Conn { sock: Sock, connected: bool },
}

struct Entry {
    generation: u32,
    token: u64,
    local: Addr,
    kind: Kind,
    /// Readiness at the last report (see `signature`); STALE after the
    /// owner sent, received or accepted (the next poll reports it: a queue
    /// emptied and refilled to the same length looks unchanged).
    last: u64,
}

/// No signature: the next poll reports the entry.
const STALE: u64 = u64::MAX;

struct Iface<D: Device> {
    dev: D,
    iface: Interface,
    sockets: SocketSet<'static>,
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

struct Stack {
    lo: Iface<Loopback>,
    eth: Option<Iface<VirtioNet>>,
    eth_ip: Option<[u8; 4]>,
    eth_mac: [u8; 6],
    entries: Vec<Option<Entry>>,
    /// Closed by their owner, still finishing (FIN, TIME-WAIT); removed
    /// from their set when done.
    lingering: Vec<Sock>,
    next_port: u16,
    next_generation: u32,
}

static STACK: IrqMutex<Option<Stack>> = IrqMutex::new(None);
static NET_THREAD: AtomicU64 = AtomicU64::new(u64::MAX);
static KICKED: AtomicBool = AtomicBool::new(false);
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
    fn sockets(&mut self, side: Side) -> Option<&mut SocketSet<'static>> {
        match side {
            Side::Lo => Some(&mut self.lo.sockets),
            Side::Eth => self.eth.as_mut().map(|e| &mut e.sockets),
        }
    }

    fn tcp(&mut self, (side, handle): Sock) -> Option<&mut tcp::Socket<'static>> {
        Some(self.sockets(side)?.get_mut::<tcp::Socket>(handle))
    }

    fn add(&mut self, side: Side, socket: tcp::Socket<'static>) -> Result<Sock, NetError> {
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

    fn remove(&mut self, (side, handle): Sock) {
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

    fn entry(&mut self, id: SocketId) -> Result<&mut Entry, NetError> {
        match self.entries.get_mut(id.index as usize) {
            Some(Some(e)) if e.generation == id.generation => Ok(e),
            _ => Err(NetError::NoSocket),
        }
    }

    fn port_in_use(&self, port: u16) -> bool {
        self.entries.iter().flatten().any(|e| e.local.port == port)
    }

    fn ephemeral_port(&mut self) -> Result<u16, NetError> {
        for _ in EPHEMERAL {
            let port = self.next_port;
            self.next_port = if port == *EPHEMERAL.end() { *EPHEMERAL.start() } else { port + 1 };
            if !self.port_in_use(port) {
                return Ok(port);
            }
        }
        Err(NetError::AddressInUse)
    }

    fn insert(&mut self, token: u64, local: Addr, kind: Kind) -> Result<SocketId, NetError> {
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
    fn sides_for(&self, ip: [u8; 4]) -> &'static [Side] {
        match (ip, self.eth.is_some()) {
            ([0, 0, 0, 0], true) => &[Side::Lo, Side::Eth],
            ([0, 0, 0, 0], false) | ([127, ..], _) => &[Side::Lo],
            _ => &[Side::Eth],
        }
    }

    fn side_to(&self, ip: [u8; 4]) -> Result<Side, NetError> {
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

fn with_stack<R>(f: impl FnOnce(&mut Stack) -> Result<R, NetError>) -> Result<R, NetError> {
    let result = with_stack_quiet(f);
    kick();
    result
}

/// `with_stack` for calls that queue nothing (no reason to poll).
fn with_stack_quiet<R>(f: impl FnOnce(&mut Stack) -> Result<R, NetError>) -> Result<R, NetError> {
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
            0 => s.ephemeral_port()?,
            p if s.port_in_use(p) => return Err(NetError::AddressInUse),
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
            Kind::Conn { .. } => return Err(NetError::InvalidState),
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
        match s.insert(token, local.unwrap_or_default(), Kind::Conn { sock, connected: true }) {
            Ok(new) => Ok(new),
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
        let Kind::Conn { sock, .. } = s.entry(id)?.kind else {
            return Err(NetError::NotConnected);
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A stack with only the loopback interface, as `start` makes it.
    fn loopback_stack() {
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
    fn poll() -> Vec<u64> {
        let mut changed = Vec::new();
        with_stack_quiet(|s| {
            s.poll(&mut changed);
            Ok(())
        })
        .unwrap();
        changed
    }

    #[test]
    fn a_reader_is_told_of_the_next_message_of_the_same_size() {
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
}

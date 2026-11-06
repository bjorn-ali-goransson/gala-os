//! UDP sockets of the kernel's socket API (`net::tcp` keeps the table of
//! sockets; `nt::afd` builds Windows datagram sockets on these). Design:
//! docs/ARCHITECTURE.md, "Networking".
//!
//! A UDP socket bound to an address has a smoltcp UDP socket on each
//! interface the address covers (both for 0.0.0.0, as a listener). After
//! every poll the net thread moves what they received into the socket's
//! own queue of datagrams (`drain`): that is where a connected socket's
//! filter applies (datagrams from other addresses are discarded, as
//! Windows documents for connect on a datagram socket), where the receive
//! limit applies (a full queue drops what comes, as UDP may), and what a
//! kernel handover carries. Sending goes straight into the smoltcp socket
//! of the interface that reaches the destination.

use super::tcp::{
    Addr, Entry, Kind, NetError, Owner, STALE, Side, Sock, SocketId, Stack, with_stack,
    with_stack_quiet,
};
use crate::mm::heap::TryVec;
use alloc::vec::Vec;
use smoltcp::phy::Device;
use smoltcp::socket::udp;
use smoltcp::wire::{IpEndpoint, IpListenEndpoint};

use super::Scope;

/// Bytes of received datagrams a socket keeps until they are read unless
/// its owner asks for more (SO_RCVBUF: Windows' default is 64 KiB, up to
/// `tcp::MAX_BUFFER`); more are dropped.
const RECV_LIMIT: usize = 64 * 1024;
/// Datagrams a socket keeps until they are read.
const RECV_DATAGRAMS: usize = 256;
/// smoltcp's own buffers per socket: datagrams and payload bytes each way
/// (received ones only until the next poll moves them to the queue).
const SMOLTCP_DATAGRAMS: usize = 64;
const SMOLTCP_PAYLOAD: usize = 64 * 1024;
/// IPv4 and UDP headers: the largest datagram is the MTU less these (no
/// fragmentation yet).
const HEADERS: usize = 20 + 8;

/// A datagram received and not read yet.
pub struct Datagram {
    pub from: Addr,
    pub data: Vec<u8>,
}

/// A UDP socket (`tcp::Kind::Udp`).
pub struct Udp {
    /// Its smoltcp sockets: loopback, card (None where the address does
    /// not reach).
    pub(super) socks: [Option<Sock>; 2],
    /// connect's default peer: sends go there, only its datagrams come.
    pub(super) peer: Option<Addr>,
    /// Received, oldest first.
    pub(super) queue: Vec<Datagram>,
    queued_bytes: usize,
    /// The most `queued_bytes` may be (`RECV_LIMIT` or the owner's
    /// SO_RCVBUF).
    recv_limit: usize,
    /// Datagrams that arrived since the socket was made (the readiness
    /// signature changes with each one).
    arrived: u64,
}

/// What the readiness report compares for UDP entry `index`
/// (`Stack::signature`): its queue, the datagrams that arrived and what
/// its smoltcp sockets still have to send (a send that waited for room
/// is retried when that shrinks).
pub(super) fn signature(s: &mut Stack, index: usize) -> u64 {
    let Some(Some(Entry { kind: Kind::Udp(u), .. })) = s.entries.get(index) else { return 0 };
    let (queued, arrived, socks) = (u.queue.len() as u64, u.arrived, u.socks);
    let mut sending = 0u64;
    for sock in socks.into_iter().flatten() {
        if let Some(set) = s.sockets(sock.0) {
            sending += set.get::<udp::Socket>(sock.1).send_queue() as u64;
        }
    }
    4 | queued << 4 | (arrived & 0xff_ffff) << 20 | sending << 44
}

impl Udp {
    fn accepts(&self, from: Addr) -> bool {
        self.peer.is_none_or(|p| p == from)
    }
}

fn new_socket() -> Option<udp::Socket<'static>> {
    let buffer = || -> Option<udp::PacketBuffer<'static>> {
        let mut meta = Vec::new();
        meta.try_reserve_exact(SMOLTCP_DATAGRAMS).ok()?;
        for _ in 0..SMOLTCP_DATAGRAMS {
            meta.try_push(udp::PacketMetadata::EMPTY).ok()?;
        }
        let mut payload = Vec::new();
        payload.try_reserve_exact(SMOLTCP_PAYLOAD).ok()?;
        payload.resize(SMOLTCP_PAYLOAD, 0);
        Some(udp::PacketBuffer::new(meta, payload))
    };
    Some(udp::Socket::new(buffer()?, buffer()?))
}

/// What one smoltcp UDP socket's buffers take (both directions).
const SMOLTCP_COST: u64 =
    2 * (SMOLTCP_PAYLOAD + SMOLTCP_DATAGRAMS * core::mem::size_of::<udp::PacketMetadata>()) as u64;

/// The queue's limit for `owner`.
fn recv_limit(owner: &Owner) -> usize {
    owner.rx.clamp(RECV_LIMIT, super::tcp::MAX_BUFFER)
}

/// The most a queue of `limit` bytes takes (the datagrams' records too).
fn queue_cost(limit: usize) -> u64 {
    (limit + RECV_DATAGRAMS * core::mem::size_of::<Datagram>()) as u64
}

fn side_index(side: Side) -> usize {
    match side {
        Side::Lo => 0,
        Side::Eth => 1,
    }
}

fn endpoint(a: Addr) -> IpEndpoint {
    a.endpoint()
}

fn addr_of(e: IpEndpoint) -> Addr {
    Addr { ip: super::Ip::from_smoltcp(e.addr), port: e.port }
}

/// The smoltcp sockets for `local` (already checked and with its port),
/// charged to `owner`: each its buffers, the first one the queue's most
/// as well (the entry's queue holds what every interface received).
fn open(
    s: &mut Stack,
    local: Addr,
    scope: Scope,
    owner: &Owner,
) -> Result<[Option<Sock>; 2], NetError> {
    let mut socks = [None; 2];
    let mut queue = queue_cost(recv_limit(owner));
    for &side in s.sides_for(local.ip) {
        // The side's address of the socket's family (none: not on it).
        let Some(addr) = s.side_address(side, local.ip, scope) else { continue };
        let listen = IpListenEndpoint { addr, port: local.port };
        let made = s.admit(side, owner, SMOLTCP_COST + queue).and_then(|bill| {
            let mut socket = new_socket().ok_or(NetError::NoMemory)?;
            socket.bind(listen).map_err(|_| NetError::InvalidState)?;
            s.add(side, socket, bill)
        });
        queue = 0;
        match made {
            Ok(sock) => socks[side_index(side)] = Some(sock),
            Err(e) => {
                for sock in socks.into_iter().flatten() {
                    s.remove(sock);
                }
                return Err(e);
            }
        }
    }
    Ok(socks)
}

/// Binds a UDP socket to `local` (port 0: an ephemeral port) for `owner`
/// (its token is passed to the notify hook).
pub fn bind(local: Addr, owner: impl Into<Owner>) -> Result<SocketId, NetError> {
    bind_scoped(local, Scope::of(local.ip), owner)
}

/// The same for a socket of `scope` (`nt::afd`).
pub fn bind_scoped(
    local: Addr,
    scope: Scope,
    owner: impl Into<Owner>,
) -> Result<SocketId, NetError> {
    let owner = owner.into();
    with_stack(|s| {
        if !s.may_bind(local.ip) {
            return Err(NetError::AddressNotAvailable);
        }
        let port = match local.port {
            0 => s.ephemeral_port(true, scope)?,
            p if s.port_in_use(p, true, scope) => return Err(NetError::AddressInUse),
            p => p,
        };
        let local = Addr { ip: local.ip, port };
        let socks = open(s, local, scope, &owner)?;
        let recv_limit = recv_limit(&owner);
        let udp =
            Udp { socks, peer: None, queue: Vec::new(), queued_bytes: 0, recv_limit, arrived: 0 };
        s.insert(owner, local, scope, Kind::Udp(udp)).inspect_err(|_| {
            for sock in socks.into_iter().flatten() {
                s.remove(sock);
            }
        })
    })
}

fn udp_of(e: &mut Entry) -> Result<&mut Udp, NetError> {
    match &mut e.kind {
        Kind::Udp(u) => Ok(u),
        _ => Err(NetError::InvalidState),
    }
}

/// Sets (or with None clears) the socket's default peer; queued datagrams
/// from other addresses are discarded.
pub fn connect(id: SocketId, peer: Option<Addr>) -> Result<(), NetError> {
    with_stack_quiet(|s| {
        let entry = s.entry(id)?;
        entry.last = STALE;
        let u = udp_of(entry)?;
        u.peer = peer;
        let mut i = 0;
        while i < u.queue.len() {
            if u.accepts(u.queue[i].from) {
                i += 1;
            } else {
                let d = u.queue.remove(i);
                u.queued_bytes -= d.data.len();
            }
        }
        Ok(())
    })
}

/// The largest datagram an interface sends (its MTU less the headers).
fn max_datagram(s: &Stack, side: Side) -> usize {
    let mtu = match side {
        Side::Lo => s.lo.dev.capabilities().ip_mtu(),
        Side::Eth => s.eth.as_ref().map_or(0, |e| e.dev.capabilities().ip_mtu()),
    };
    mtu.saturating_sub(HEADERS)
}

/// Queues one datagram to `to` (None: the default peer); returns its
/// length. WouldBlock while the socket's send queue is full, TooLarge for
/// a datagram the interface cannot carry.
pub fn send_to(id: SocketId, to: Option<Addr>, data: &[u8]) -> Result<usize, NetError> {
    with_stack(|s| {
        let entry = s.entry(id)?;
        entry.last = STALE;
        let local = entry.local;
        let u = udp_of(entry)?;
        let to = to.or(u.peer).ok_or(NetError::NotConnected)?;
        let socks = u.socks;
        if to.ip.is_unspecified() || to.port == 0 {
            return Err(NetError::AddressNotAvailable);
        }
        let side = s.side_to(to.ip)?;
        let sock = socks[side_index(side)].ok_or(if local.ip.is_loopback() {
            NetError::Unreachable
        } else {
            NetError::AddressNotAvailable
        })?;
        if data.len() > max_datagram(s, side) {
            return Err(NetError::TooLarge);
        }
        let set = s.sockets(sock.0).ok_or(NetError::Unreachable)?;
        let socket = set.get_mut::<udp::Socket>(sock.1);
        if data.len() > socket.payload_send_capacity() {
            return Err(NetError::TooLarge);
        }
        match socket.send_slice(data, endpoint(to)) {
            Ok(()) => Ok(data.len()),
            Err(udp::SendError::BufferFull) => Err(NetError::WouldBlock),
            Err(udp::SendError::Unaddressable) => Err(NetError::AddressNotAvailable),
        }
    })
}

/// What `recv_from` took: the bytes copied, the datagram's whole length
/// (more than copied: it did not fit, and the rest is gone unless
/// peeking), its sender.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Received {
    pub copied: usize,
    pub length: usize,
    pub from: Addr,
}

/// Takes the oldest datagram into `out` (with `peek` it stays queued);
/// WouldBlock if none.
pub fn recv_from(id: SocketId, out: &mut [u8], peek: bool) -> Result<Received, NetError> {
    with_stack_quiet(|s| {
        // What smoltcp holds since the last poll first.
        drain_one(s, id.index as usize);
        let entry = s.entry(id)?;
        entry.last = STALE;
        let u = udp_of(entry)?;
        let d = u.queue.first().ok_or(NetError::WouldBlock)?;
        let copied = d.data.len().min(out.len());
        out[..copied].copy_from_slice(&d.data[..copied]);
        let r = Received { copied, length: d.data.len(), from: d.from };
        if !peek {
            let d = u.queue.remove(0);
            u.queued_bytes -= d.data.len();
        }
        Ok(r)
    })
}

/// Moves what the smoltcp sockets of every UDP entry received into the
/// entries' queues (after each poll).
pub(super) fn drain(s: &mut Stack) {
    for i in 0..s.entries.len() {
        drain_one(s, i);
    }
}

fn drain_one(s: &mut Stack, index: usize) {
    let Stack { lo, eth, entries, .. } = s;
    let Some(Some(Entry { kind: Kind::Udp(u), .. })) = entries.get_mut(index) else { return };
    for sock in u.socks.into_iter().flatten() {
        let set = match sock.0 {
            Side::Lo => &mut lo.sockets,
            Side::Eth => match eth.as_mut() {
                Some(e) => &mut e.sockets,
                None => continue,
            },
        };
        let socket = set.get_mut::<udp::Socket>(sock.1);
        while let Ok((data, meta)) = socket.recv() {
            let from = addr_of(meta.endpoint);
            u.arrived = u.arrived.wrapping_add(1);
            if !u.accepts(from)
                || u.queue.len() >= RECV_DATAGRAMS
                || u.queued_bytes + data.len() > u.recv_limit
            {
                continue;
            }
            let mut copy = Vec::new();
            if copy.try_extend_from_slice(data).is_err() || u.queue.try_reserve(1).is_err() {
                // Out of memory: dropped, as a full queue drops.
                continue;
            }
            u.queued_bytes += copy.len();
            // Room was reserved.
            let _ = u.queue.try_push(Datagram { from, data: copy });
        }
    }
}

/// Socket `id` as carried (the stack stopped): the peer and the queued
/// datagrams (what smoltcp still held moved to the queue first, so a
/// refused handover loses nothing). Datagrams in smoltcp's send queue are
/// not carried (the poll before the stop sent what it could; UDP may lose
/// the rest).
pub(super) fn carry(s: &mut Stack, id: SocketId) -> Result<super::tcp::CarriedKind, NetError> {
    drain_one(s, id.index as usize);
    let u = udp_of(s.entry(id)?)?;
    let mut datagrams = Vec::new();
    datagrams.try_reserve(u.queue.len()).map_err(|_| NetError::NoMemory)?;
    for d in &u.queue {
        let mut copy = Vec::new();
        copy.try_extend_from_slice(&d.data).map_err(|_| NetError::NoMemory)?;
        datagrams.try_push((d.from, copy)).map_err(|_| NetError::NoMemory)?;
    }
    Ok(super::tcp::CarriedKind::Udp { peer: u.peer, datagrams })
}

/// In the next kernel: a carried UDP socket's kernel half again (its
/// smoltcp sockets bound anew, its queue as it was), for `owner`.
pub(super) fn adopt(
    s: &mut Stack,
    local: Addr,
    peer: Option<Addr>,
    datagrams: &[(Addr, Vec<u8>)],
    owner: &Owner,
) -> Result<Udp, NetError> {
    let mut queue = Vec::new();
    queue.try_reserve(datagrams.len()).map_err(|_| NetError::NoMemory)?;
    let mut queued_bytes = 0;
    for (from, data) in datagrams {
        let mut copy = Vec::new();
        copy.try_extend_from_slice(data).map_err(|_| NetError::NoMemory)?;
        queued_bytes += copy.len();
        queue.try_push(Datagram { from: *from, data: copy }).map_err(|_| NetError::NoMemory)?;
    }
    // Carried sockets are IPv4 (a handover refuses IPv6 ones).
    let scope = Scope::of(local.ip);
    if local.port == 0 || s.port_in_use(local.port, true, scope) {
        return Err(NetError::AddressInUse);
    }
    let socks = open(s, local, scope, owner)?;
    // A queue carried fuller than its limit drains below it first.
    Ok(Udp { socks, peer, queue, queued_bytes, recv_limit: recv_limit(owner), arrived: 0 })
}

#[cfg(test)]
mod tests {
    use super::super::tcp::{self, CarriedKind, testing};
    use super::*;

    const LO: [u8; 4] = [127, 0, 0, 1];

    fn recv_wait(id: SocketId, out: &mut [u8]) -> Received {
        for _ in 0..16 {
            match recv_from(id, out, false) {
                Ok(r) => return r,
                Err(NetError::WouldBlock) => {
                    testing::poll();
                }
                Err(e) => panic!("recv_from: {e:?}"),
            }
        }
        panic!("no datagram")
    }

    #[test]
    fn datagrams_keep_boundaries_senders_and_the_peer_filter() {
        let _turn = testing::turn();
        testing::loopback_stack();
        let a = bind(Addr { ip: LO.into(), port: 0 }, 1).unwrap();
        let b = bind(Addr { ip: LO.into(), port: 0 }, 2).unwrap();
        let (la, lb) = (tcp::local(a).unwrap(), tcp::local(b).unwrap());
        assert!(la.port >= 49152 && la.port != lb.port);
        // UDP and TCP ports are apart; a UDP port in use is refused.
        assert_eq!(bind(la, 3), Err(NetError::AddressInUse));
        let t = tcp::bind(la, 4).unwrap();
        tcp::close(t, false);

        assert_eq!(send_to(a, Some(lb), b"one"), Ok(3));
        assert_eq!(send_to(a, Some(lb), b"two!"), Ok(4));
        let changed = testing::poll();
        assert!(changed.contains(&2), "{changed:?}");
        assert!(tcp::readiness(b).unwrap().readable);
        let mut buf = [0u8; 16];
        let r = recv_from(b, &mut buf, true).unwrap();
        assert_eq!((r.copied, r.length, r.from), (3, 3, la));
        let r = recv_wait(b, &mut buf);
        assert_eq!((&buf[..3], r.from), (&b"one"[..], la));
        // Truncated: the start copied, the rest gone.
        let mut small = [0u8; 2];
        let r = recv_from(b, &mut small, false).unwrap();
        assert_eq!((r.copied, r.length, &small), (2, 4, b"tw"));
        assert_eq!(recv_from(b, &mut buf, false), Err(NetError::WouldBlock));

        // Connected: only the peer's datagrams come.
        let c = bind(Addr { ip: LO.into(), port: 0 }, 5).unwrap();
        connect(b, Some(la)).unwrap();
        assert_eq!(tcp::remote(b), Ok(la));
        assert_eq!(send_to(c, Some(lb), b"stranger"), Ok(8));
        assert_eq!(send_to(a, Some(lb), b"friend"), Ok(6));
        assert_eq!(send_to(b, None, b"back"), Ok(4));
        testing::poll();
        let r = recv_wait(b, &mut buf);
        assert_eq!(&buf[..r.copied], b"friend");
        assert_eq!(recv_from(b, &mut buf, false), Err(NetError::WouldBlock));
        let r = recv_wait(a, &mut buf);
        assert_eq!((&buf[..r.copied], r.from), (&b"back"[..], lb));
        connect(b, None).unwrap();
        assert_eq!(tcp::remote(b), Err(NetError::NotConnected));
        assert_eq!(send_to(b, None, b"x"), Err(NetError::NotConnected));
        // Too large for loopback's MTU.
        let big = alloc::vec![0u8; 20_000];
        assert_eq!(send_to(a, Some(lb), &big), Err(NetError::TooLarge));
        for id in [a, b, c] {
            tcp::close(id, false);
        }
        testing::end();
    }

    /// R9 (e): a bound UDP socket written down and rebuilt in a new stack
    /// keeps its port, its peer and the datagrams not read yet.
    #[test]
    fn udp_sockets_go_on_in_a_new_stack() {
        let _turn = testing::turn();
        testing::loopback_stack();
        let a = bind(Addr::v4([0; 4], 5300), 1).unwrap();
        let b = bind(Addr { ip: LO.into(), port: 0 }, 2).unwrap();
        let lb = tcp::local(b).unwrap();
        assert_eq!(send_to(b, Some(Addr { ip: LO.into(), port: 5300 }), b"kept"), Ok(4));
        assert_eq!(send_to(b, Some(Addr { ip: LO.into(), port: 5300 }), b"too"), Ok(3));
        testing::poll();
        connect(a, Some(lb)).unwrap();
        assert!(tcp::stop(100));
        let carried = [tcp::carry_entry(a).unwrap(), tcp::carry_entry(b).unwrap()];
        match &carried[0].kind {
            CarriedKind::Udp { peer, datagrams } => {
                assert_eq!(*peer, Some(lb));
                assert_eq!(datagrams.len(), 2);
                assert_eq!(datagrams[0], (lb, b"kept".to_vec()));
            }
            other => panic!("carried as {other:?}"),
        }
        tcp::restart();

        testing::loopback_stack();
        let a = tcp::adopt_entry(&carried[0], 10).unwrap();
        let b = tcp::adopt_entry(&carried[1], 11).unwrap();
        assert_eq!(tcp::local(a).unwrap().port, 5300);
        assert_eq!(tcp::local(b).unwrap(), lb);
        assert_eq!(tcp::remote(a), Ok(lb));
        let mut buf = [0u8; 8];
        let r = recv_from(a, &mut buf, false).unwrap();
        assert_eq!(&buf[..r.copied], b"kept");
        let r = recv_from(a, &mut buf, false).unwrap();
        assert_eq!(&buf[..r.copied], b"too");
        // And new datagrams both ways.
        assert_eq!(send_to(a, None, b"new"), Ok(3));
        let r = recv_wait(b, &mut buf);
        assert_eq!(&buf[..r.copied], b"new");
        assert_eq!(send_to(b, Some(Addr { ip: LO.into(), port: 5300 }), b"again"), Ok(5));
        let r = recv_wait(a, &mut buf);
        assert_eq!(&buf[..r.copied], b"again");
        testing::end();
    }
}

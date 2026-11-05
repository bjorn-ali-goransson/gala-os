//! Sockets across a kernel handover (ROADMAP R9 milestone (e),
//! ARCHITECTURE "Sockets across a handover"): the record's formats for
//! the TCP stack and the programs' AFD endpoints. Plain data in gala's own
//! terms (`net::tcp::ConnState`: sequence numbers, windows, options,
//! absolute timer deadlines, queued bytes), never smoltcp's memory layout,
//! so a newer smoltcp is a change of the code that fills and reads these,
//! not of the format. Host-tested; `carry` fills them and rebuilds the
//! endpoints from them.
//!
//! Both sections are REQUIRED (a kernel that does not know them would
//! lose the connections) and written only when a program holds a socket
//! (`version::Kind::when_present`: negotiation checks them on the record
//! as written, so a handover without sockets still goes to a kernel that
//! predates them).
//!
//! ```text
//! NET      a header entry (when the stack stopped, the card's address
//!          configuration, the ephemeral port counter), then an entry per
//!          closing connection nobody owns any more (CONNECTION)
//! SOCKETS  an entry per AFD endpoint object (SocketRec): its own state,
//!          the kernel socket behind it (bound, listener with its accept
//!          queue, connection, UDP socket), its pending requests
//!          (RequestRec), then (version 2) a UDP socket's peer and its
//!          datagrams not read yet (DATAGRAM entries)
//! ```
//!
//! A CONNECTION entry: role and sequence (u32 each), then the `ConnState`
//! fields in the order `write_conn` writes them, then the received and the
//! unsent bytes (blobs). Absent optional values are written as all ones.
//! A DATAGRAM entry: the sender's address, then the payload (a blob).
//!
//! Versions of SOCKETS: 1 (TCP); 2 adds UDP sockets (transport 2, socket
//! kind UDP, the datagram part at the end of the entry) and the requests
//! RECV_FROM and SEND_TO (their fields after the endpoints table). A
//! version-2 writer marks the section must_read 2 only when it holds a
//! UDP socket (`sockets_must_read`): TCP-only records still go to a
//! version-1 reader, which skips the appended fields; this reader takes
//! version 1 with the new fields absent.

use alloc::vec::Vec;

use super::record::{REQUIRED, Reader, Writer, tag};
use super::version;
use crate::mm::heap::{AllocError, TryVec};
use crate::net::Ip;
use crate::net::tcp::{Addr, CardConfig, CarriedConn, CarriedEntry, CarriedKind, CarriedStack};

/// What a CONNECTION entry is to its socket.
pub mod role {
    /// The socket's own connection.
    pub const CONNECTION: u32 = 0;
    /// Established, waiting in a listener's accept queue (with its
    /// sequence number).
    pub const READY: u32 = 1;
    /// A listener's socket that is not plain LISTEN (half-open, or
    /// established while the backlog was full).
    pub const OPENING: u32 = 2;
    /// Closed by its owner, still finishing (NET).
    pub const LINGERING: u32 = 3;
}

/// The kernel socket behind an endpoint (SocketRec::kind).
pub mod socket_kind {
    pub const NONE: u32 = 0;
    pub const BOUND: u32 = 1;
    pub const LISTENER: u32 = 2;
    pub const CONNECTION: u32 = 3;
    /// A UDP socket (version 2).
    pub const UDP: u32 = 4;
}

/// SocketRec::transport.
pub mod transport {
    pub const NONE: u32 = 0;
    pub const TCP: u32 = 1;
    /// Version 2.
    pub const UDP: u32 = 2;
}

/// A pending AFD request's kind (RequestRec::kind).
pub mod request_kind {
    pub const RECV: u32 = 0;
    pub const SEND: u32 = 1;
    pub const WAIT_FOR_LISTEN: u32 = 2;
    pub const SUPER_ACCEPT: u32 = 3;
    pub const CONNECT: u32 = 4;
    pub const POLL: u32 = 5;
    /// A datagram receive, with where the sender's address goes (version 2).
    pub const RECV_FROM: u32 = 6;
    /// A datagram send, with its destination (version 2).
    pub const SEND_TO: u32 = 7;
}

const NONE32: u32 = u32::MAX;
const NONE64: u64 = u64::MAX;

/// Starts one of this module's sections (REQUIRED, at this kernel's
/// version and `must_read`).
fn section(w: &mut Writer, tag: u32) -> Result<(), AllocError> {
    let k = version::kind_of(tag);
    let (writes, must_read) = k.map_or((1, 0), |k| (k.writes, k.must_read));
    w.section_needing(tag, writes, REQUIRED, must_read)
}

fn ip_u32(ip: [u8; 4]) -> u32 {
    u32::from_le_bytes(ip)
}

/// An address as the record holds it: IPv4 only (a handover refuses
/// IPv6 sockets: `carry::collect_sockets`).
fn write_addr(w: &mut Writer, a: Addr) -> Result<(), AllocError> {
    let ip = match a.ip {
        Ip::V4(ip) => ip,
        Ip::V6(_) => [0; 4],
    };
    w.u32(ip_u32(ip))?;
    w.u32(u32::from(a.port))
}

fn read_addr(r: &mut Reader<'_>) -> Addr {
    let ip = r.u32().to_le_bytes();
    Addr::v4(ip, r.u32() as u16)
}

fn opt32(v: Option<u32>) -> u32 {
    v.unwrap_or(NONE32)
}

fn read_opt32(r: &mut Reader<'_>) -> Option<u32> {
    Some(r.u32()).filter(|&v| v != NONE32)
}

fn copy(bytes: &[u8]) -> Result<Vec<u8>, AllocError> {
    let mut v = Vec::new();
    v.try_extend_from_slice(bytes)?;
    Ok(v)
}

/// A CONNECTION entry.
pub fn write_conn(
    w: &mut Writer,
    role: u32,
    sequence: u32,
    c: &CarriedConn,
) -> Result<(), AllocError> {
    let s = &c.state;
    let e = w.begin_entry()?;
    w.u32(role)?;
    w.u32(sequence)?;
    w.u32(u32::from(s.on_card))?;
    w.u32(s.state)?;
    let (local, remote) = s.tuple.unwrap_or_default();
    w.u32(u32::from(s.tuple.is_some()))?;
    write_addr(w, local)?;
    write_addr(w, remote)?;
    write_addr(w, s.listen)?;
    w.u32(s.send_unacked)?;
    w.u32(s.send_last)?;
    w.u32(s.recv_first)?;
    w.u32(u32::from(s.ack_sent.is_some()))?;
    w.u32(s.ack_sent.unwrap_or(0))?;
    w.u32(u32::from(s.window_sent))?;
    w.u32(u32::from(s.window_shift))?;
    w.u32(s.peer_window)?;
    w.u32(opt32(s.peer_window_scale.map(u32::from)))?;
    w.u32(u32::from(s.peer_sack))?;
    w.u32(s.peer_mss)?;
    w.u64(s.last_received_ns.unwrap_or(NONE64))?;
    w.u32(u32::from(s.fin_received))?;
    w.u32(s.timer)?;
    w.u64(s.timer_at_ns)?;
    w.u64(s.timer_delay_ns)?;
    w.u32(s.rtt_ms)?;
    w.u32(s.rtt_dev_ms)?;
    w.u32(s.peer_tsval)?;
    w.u32(u32::from(s.nagle))?;
    w.u32(opt32(s.ack_delay_ms))?;
    w.u32(opt32(s.keep_alive_ms))?;
    w.u32(opt32(s.timeout_ms))?;
    w.u32(opt32(s.hop_limit.map(u32::from)))?;
    w.u32(s.recv_capacity)?;
    w.u32(s.send_capacity)?;
    w.blob(&c.received)?;
    w.blob(&c.unsent)?;
    w.end_entry(e)
}

/// Reads a CONNECTION entry: (role, sequence, the connection).
pub fn read_conn(r: &mut Reader<'_>) -> Result<(u32, u32, CarriedConn), AllocError> {
    let role = r.u32();
    let sequence = r.u32();
    let mut c = CarriedConn::default();
    let s = &mut c.state;
    s.on_card = r.u32() != 0;
    s.state = r.u32();
    let has_tuple = r.u32() != 0;
    let (local, remote) = (read_addr(r), read_addr(r));
    s.tuple = has_tuple.then_some((local, remote));
    s.listen = read_addr(r);
    s.send_unacked = r.u32();
    s.send_last = r.u32();
    s.recv_first = r.u32();
    let has_ack = r.u32() != 0;
    let ack = r.u32();
    s.ack_sent = has_ack.then_some(ack);
    s.window_sent = r.u32() as u16;
    s.window_shift = r.u32() as u8;
    s.peer_window = r.u32();
    s.peer_window_scale = read_opt32(r).map(|v| v as u8);
    s.peer_sack = r.u32() != 0;
    s.peer_mss = r.u32();
    s.last_received_ns = Some(r.u64()).filter(|&v| v != NONE64);
    s.fin_received = r.u32() != 0;
    s.timer = r.u32();
    s.timer_at_ns = r.u64();
    s.timer_delay_ns = r.u64();
    s.rtt_ms = r.u32();
    s.rtt_dev_ms = r.u32();
    s.peer_tsval = r.u32();
    s.nagle = r.u32() != 0;
    s.ack_delay_ms = read_opt32(r);
    s.keep_alive_ms = read_opt32(r);
    s.timeout_ms = read_opt32(r);
    s.hop_limit = read_opt32(r).map(|v| v as u8);
    s.recv_capacity = r.u32();
    s.send_capacity = r.u32();
    c.received = copy(r.blob())?;
    c.unsent = copy(r.blob())?;
    Ok((role, sequence, c))
}

/// The NET section: the stack's own state and the closing connections.
pub fn write_net(w: &mut Writer, s: &CarriedStack) -> Result<(), AllocError> {
    section(w, tag::NET)?;
    let e = w.begin_entry()?;
    w.u64(s.stopped_ns)?;
    let card = s.card.unwrap_or_default();
    w.u32(u32::from(s.card.is_some()))?;
    w.u32(ip_u32(card.ip))?;
    w.u32(u32::from(card.prefix))?;
    w.u32(ip_u32(card.gateway))?;
    w.bytes(&card.mac)?;
    w.bytes(&[0, 0])?;
    w.u32(u32::from(s.next_port))?;
    w.end_entry(e)?;
    for c in &s.lingering {
        write_conn(w, role::LINGERING, 0, c)?;
    }
    Ok(())
}

/// Reads the NET section.
pub fn read_net(r: &mut Reader<'_>) -> Result<CarriedStack, AllocError> {
    let mut s = CarriedStack::default();
    if let Some(mut e) = r.entry() {
        s.stopped_ns = e.u64();
        let has_card = e.u32() != 0;
        let ip = e.u32().to_le_bytes();
        let prefix = e.u32() as u8;
        let gateway = e.u32().to_le_bytes();
        let mac: [u8; 8] = e.array();
        let mut card = CardConfig { ip, prefix, gateway, mac: [0; 6] };
        card.mac.copy_from_slice(&mac[..6]);
        s.card = has_card.then_some(card);
        s.next_port = e.u32() as u16;
    }
    while let Some(mut e) = r.entry() {
        let (_, _, c) = read_conn(&mut e)?;
        s.lingering.try_push(c)?;
    }
    Ok(s)
}

/// A pending AFD request, objects by id.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct RequestRec {
    pub kind: u32,
    /// The process that issued it (its buffers are in its memory).
    pub pid: u64,
    /// IO_STATUS_BLOCK address (0: none).
    pub iosb: u64,
    /// The event set on completion (0: none).
    pub event: u32,
    /// ApcContext: the completion packet's context.
    pub context: u64,
    /// The thread a completion routine is queued to (0: none), the
    /// routine and its three arguments.
    pub apc_thread: u32,
    pub apc: [u64; 4],
    /// RECV: peek (MSG_PEEK); POLL: it waits for an event.
    pub flag: bool,
    /// SEND: bytes already queued in the socket.
    pub done: u64,
    /// The output buffer (WAIT_FOR_LISTEN, SUPER_ACCEPT, POLL) and its
    /// length; SUPER_ACCEPT's receive and local address lengths.
    pub output: u64,
    pub output_len: u32,
    pub receive: u32,
    pub local: u32,
    /// RECV and SEND: the user buffers (address, length).
    pub buffers: Vec<(u64, u64)>,
    /// SUPER_ACCEPT: the accepting endpoint (handle 0); POLL: (the
    /// socket handle, its endpoint object, the events asked for).
    pub endpoints: Vec<(u64, u32, u32)>,
    /// RECV_FROM (version 2): where the sender's SOCKADDR_IN and its
    /// length go (0: not asked for).
    pub address: u64,
    pub address_len: u64,
    /// SEND_TO (version 2): the destination (None: the default peer).
    pub to: Option<Addr>,
}

impl RequestRec {
    pub fn write(&self, w: &mut Writer) -> Result<(), AllocError> {
        let e = w.begin_entry()?;
        w.u32(self.kind)?;
        w.u64(self.pid)?;
        w.u64(self.iosb)?;
        w.u32(self.event)?;
        w.u64(self.context)?;
        w.u32(self.apc_thread)?;
        for a in self.apc {
            w.u64(a)?;
        }
        w.u32(u32::from(self.flag))?;
        w.u64(self.done)?;
        w.u64(self.output)?;
        w.u32(self.output_len)?;
        w.u32(self.receive)?;
        w.u32(self.local)?;
        w.table(self.buffers.len() as u32, 16)?;
        for &(addr, len) in &self.buffers {
            w.u64(addr)?;
            w.u64(len)?;
        }
        w.table(self.endpoints.len() as u32, 16)?;
        for &(handle, object, events) in &self.endpoints {
            w.u64(handle)?;
            w.u32(object)?;
            w.u32(events)?;
        }
        // Version 2.
        w.u64(self.address)?;
        w.u64(self.address_len)?;
        w.u32(u32::from(self.to.is_some()))?;
        write_addr(w, self.to.unwrap_or_default())?;
        w.end_entry(e)
    }

    pub fn read(r: &mut Reader<'_>) -> Result<Self, AllocError> {
        let mut q = RequestRec {
            kind: r.u32(),
            pid: r.u64(),
            iosb: r.u64(),
            event: r.u32(),
            context: r.u64(),
            apc_thread: r.u32(),
            ..RequestRec::default()
        };
        for a in &mut q.apc {
            *a = r.u64();
        }
        q.flag = r.u32() != 0;
        q.done = r.u64();
        q.output = r.u64();
        q.output_len = r.u32();
        q.receive = r.u32();
        q.local = r.u32();
        for mut b in r.table().iter() {
            q.buffers.try_push((b.u64(), b.u64()))?;
        }
        for mut b in r.table().iter() {
            q.endpoints.try_push((b.u64(), b.u32(), b.u32()))?;
        }
        // Version 2 (absent from version 1: zeros, no destination).
        q.address = r.u64();
        q.address_len = r.u64();
        let has_to = r.u32() != 0;
        let to = read_addr(r);
        q.to = has_to.then_some(to);
        Ok(q)
    }
}

/// A SOCKETS entry: an AFD endpoint object, objects by id.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SocketRec {
    pub id: u32,
    /// `transport`: a helper handle (no transport), TCP, UDP.
    pub transport: u32,
    pub non_blocking: bool,
    /// FILE_SKIP_* notification modes.
    pub modes: u32,
    /// The completion port's id (0: none) and key.
    pub port: u32,
    pub key: u64,
    /// WSAEventSelect: the event's id (0: none), the events selected,
    /// the ones recorded since they were last read.
    pub select_event: u32,
    pub select_mask: u32,
    pub recorded: u32,
    /// The endpoint's own signal state (a wait on the socket handle).
    pub signal: i32,
    /// The kernel socket behind it, if any.
    pub entry: Option<CarriedEntry>,
    pub requests: Vec<RequestRec>,
}

impl SocketRec {
    pub fn write(&self, w: &mut Writer) -> Result<(), AllocError> {
        let e = w.begin_entry()?;
        w.u32(self.id)?;
        w.u32(self.transport)?;
        w.u32(u32::from(self.non_blocking))?;
        w.u32(self.modes)?;
        w.u32(self.port)?;
        w.u64(self.key)?;
        w.u32(self.select_event)?;
        w.u32(self.select_mask)?;
        w.u32(self.recorded)?;
        w.u32(self.signal as u32)?;
        let (kind, local, a, b) = match &self.entry {
            None => (socket_kind::NONE, Addr::default(), 0, 0),
            Some(CarriedEntry { local, kind: CarriedKind::Bound }) => {
                (socket_kind::BOUND, *local, 0, 0)
            }
            Some(CarriedEntry {
                local,
                kind: CarriedKind::Listener { backlog, next_sequence, .. },
            }) => (socket_kind::LISTENER, *local, *backlog, *next_sequence),
            Some(CarriedEntry { local, kind: CarriedKind::Conn { connected, .. } }) => {
                (socket_kind::CONNECTION, *local, u32::from(*connected), 0)
            }
            Some(CarriedEntry { local, kind: CarriedKind::Udp { .. } }) => {
                (socket_kind::UDP, *local, 0, 0)
            }
        };
        w.u32(kind)?;
        write_addr(w, local)?;
        w.u32(a)?;
        w.u32(b)?;
        match self.entry.as_ref().map(|e| &e.kind) {
            Some(CarriedKind::Conn { conn, .. }) => {
                w.u32(1)?;
                write_conn(w, role::CONNECTION, 0, conn)?;
            }
            Some(CarriedKind::Listener { ready, opening, .. }) => {
                w.u32((ready.len() + opening.len()) as u32)?;
                for (q, c) in ready {
                    write_conn(w, role::READY, *q, c)?;
                }
                for c in opening {
                    write_conn(w, role::OPENING, 0, c)?;
                }
            }
            _ => w.u32(0)?,
        }
        w.u32(self.requests.len() as u32)?;
        for q in &self.requests {
            q.write(w)?;
        }
        // Version 2: a UDP socket's peer and queued datagrams.
        let (peer, datagrams) = match self.entry.as_ref().map(|e| &e.kind) {
            Some(CarriedKind::Udp { peer, datagrams }) => (*peer, datagrams.as_slice()),
            _ => (None, &[][..]),
        };
        w.u32(u32::from(peer.is_some()))?;
        write_addr(w, peer.unwrap_or_default())?;
        w.u32(datagrams.len() as u32)?;
        for (from, data) in datagrams {
            let d = w.begin_entry()?;
            write_addr(w, *from)?;
            w.blob(data)?;
            w.end_entry(d)?;
        }
        w.end_entry(e)
    }

    /// Reads a SOCKETS entry. Err(None): damaged (a connection where the
    /// socket kind has none); Err(Some): out of memory.
    pub fn read(r: &mut Reader<'_>) -> Result<Self, Option<AllocError>> {
        let mut s = SocketRec {
            id: r.u32(),
            transport: r.u32(),
            non_blocking: r.u32() != 0,
            modes: r.u32(),
            port: r.u32(),
            key: r.u64(),
            select_event: r.u32(),
            select_mask: r.u32(),
            recorded: r.u32(),
            signal: r.u32() as i32,
            ..SocketRec::default()
        };
        let kind = r.u32();
        let local = read_addr(r);
        let (a, b) = (r.u32(), r.u32());
        let conns = r.u32();
        let mut own = None;
        let (mut ready, mut opening) = (Vec::new(), Vec::new());
        for _ in 0..conns {
            let mut e = r.entry().ok_or(None)?;
            let (role, sequence, c) = read_conn(&mut e).map_err(Some)?;
            match role {
                role::CONNECTION if own.is_none() => own = Some(c),
                role::READY => ready.try_push((sequence, c)).map_err(Some)?,
                role::OPENING => opening.try_push(c).map_err(Some)?,
                _ => return Err(None),
            }
        }
        s.entry = match kind {
            socket_kind::NONE => None,
            socket_kind::BOUND => Some(CarriedEntry { local, kind: CarriedKind::Bound }),
            socket_kind::LISTENER => Some(CarriedEntry {
                local,
                kind: CarriedKind::Listener { backlog: a, next_sequence: b, ready, opening },
            }),
            socket_kind::CONNECTION => Some(CarriedEntry {
                local,
                kind: CarriedKind::Conn { conn: own.ok_or(None)?, connected: a != 0 },
            }),
            socket_kind::UDP => Some(CarriedEntry {
                local,
                kind: CarriedKind::Udp { peer: None, datagrams: Vec::new() },
            }),
            _ => return Err(None),
        };
        let requests = r.u32();
        for _ in 0..requests {
            let mut e = r.entry().ok_or(None)?;
            s.requests.try_push(RequestRec::read(&mut e).map_err(Some)?).map_err(Some)?;
        }
        // Version 2 (a version-1 entry ends here: no peer, no datagrams).
        let has_peer = r.u32() != 0;
        let peer_addr = read_addr(r);
        let count = r.u32();
        let mut queued = Vec::new();
        for _ in 0..count {
            let mut d = r.entry().ok_or(None)?;
            let from = read_addr(&mut d);
            queued.try_push((from, copy(d.blob()).map_err(Some)?)).map_err(Some)?;
        }
        match s.entry.as_mut().map(|e| &mut e.kind) {
            Some(CarriedKind::Udp { peer, datagrams }) => {
                *peer = has_peer.then_some(peer_addr);
                *datagrams = queued;
            }
            // Only a UDP socket has these.
            _ if has_peer || count != 0 => return Err(None),
            _ => {}
        }
        Ok(s)
    }
}

/// SOCKETS' `must_read`: a record holding a UDP socket (or a request on
/// one) is misread by a version-1 reader, which knows neither.
pub fn sockets_must_read(any_udp: bool) -> u16 {
    let k = version::kind_of(tag::SOCKETS).map_or(0, |k| k.must_read);
    if any_udp { k.max(2) } else { k }
}

/// Starts the SOCKETS section (its entries: `SocketRec::write`); `any_udp`:
/// a UDP socket is among them.
pub fn sockets_section(w: &mut Writer, any_udp: bool) -> Result<(), AllocError> {
    let writes = version::writes(tag::SOCKETS);
    w.section_needing(tag::SOCKETS, writes, REQUIRED, sockets_must_read(any_udp))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handover::record::{self, Record};
    use crate::net::tcp::{ConnState, tcp_state, timer_kind};

    fn conn(n: u8) -> CarriedConn {
        CarriedConn {
            state: ConnState {
                on_card: true,
                state: tcp_state::ESTABLISHED,
                tuple: Some((
                    Addr::v4([10, 0, 2, 15], 7),
                    Addr::v4([10, 0, 2, 2], 40000 + u16::from(n)),
                )),
                listen: Addr::v4([0; 4], 7),
                send_unacked: 0xffff_fff0,
                send_last: 0x10,
                recv_first: 0x1234_5678,
                ack_sent: Some(0x1234_5679),
                window_sent: 1000,
                window_shift: 1,
                peer_window: 65535,
                peer_window_scale: Some(7),
                peer_sack: true,
                peer_mss: 1460,
                last_received_ns: Some(5_000_000_000),
                fin_received: false,
                timer: timer_kind::RETRANSMIT,
                timer_at_ns: 6_000_000_000,
                timer_delay_ns: 300_000_000,
                rtt_ms: 12,
                rtt_dev_ms: 3,
                peer_tsval: 0,
                nagle: false,
                ack_delay_ms: None,
                keep_alive_ms: Some(7_200_000),
                timeout_ms: None,
                hop_limit: Some(64),
                recv_capacity: 65536,
                send_capacity: 65536,
            },
            received: [n; 5].to_vec(),
            unsent: b"unacked and unsent".to_vec(),
        }
    }

    /// A UDP socket with a peer, two datagrams queued, a receive and a
    /// send pending.
    fn udp_socket() -> SocketRec {
        let recv_from = RequestRec {
            kind: request_kind::RECV_FROM,
            pid: 4,
            iosb: 0x1020,
            flag: true,
            buffers: [(0x7000, 512)].to_vec(),
            address: 0x7200,
            address_len: 0x7210,
            ..RequestRec::default()
        };
        let send_to = RequestRec {
            kind: request_kind::SEND_TO,
            pid: 4,
            iosb: 0x1030,
            buffers: [(0x7400, 3)].to_vec(),
            to: Some(Addr::v4([10, 0, 2, 2], 53)),
            ..RequestRec::default()
        };
        let peer = Addr::v4([127, 0, 0, 1], 50000);
        SocketRec {
            id: 6,
            transport: transport::UDP,
            entry: Some(CarriedEntry {
                local: Addr::v4([0; 4], 7),
                kind: CarriedKind::Udp {
                    peer: Some(peer),
                    datagrams: [(peer, b"first".to_vec()), (peer, Vec::new())].to_vec(),
                },
            }),
            requests: [recv_from, send_to].into_iter().collect(),
            ..SocketRec::default()
        }
    }

    fn sockets() -> [SocketRec; 3] {
        let request = RequestRec {
            kind: request_kind::RECV,
            pid: 4,
            iosb: 0x1000,
            event: 9,
            context: 0,
            apc_thread: 0,
            apc: [0; 4],
            flag: false,
            done: 0,
            output: 0,
            output_len: 0,
            receive: 0,
            local: 0,
            buffers: [(0x2000, 4096), (0x4000, 16)].to_vec(),
            endpoints: Vec::new(),
            ..RequestRec::default()
        };
        let poll = RequestRec {
            kind: request_kind::POLL,
            pid: 4,
            iosb: 0x1010,
            event: 0,
            context: 0x5000,
            apc_thread: 3,
            apc: [0x40_1000, 1, 2, 3],
            flag: true,
            output: 0x6000,
            output_len: 64,
            endpoints: [(0x84, 1, 0x81), (0x88, 2, 1)].to_vec(),
            ..RequestRec::default()
        };
        [
            SocketRec {
                id: 1,
                transport: 1,
                non_blocking: false,
                modes: 1,
                port: 0,
                key: 0,
                select_event: 0,
                select_mask: 0,
                recorded: 0,
                signal: 0,
                entry: Some(CarriedEntry {
                    local: Addr::v4([0; 4], 7),
                    kind: CarriedKind::Listener {
                        backlog: 5,
                        next_sequence: 3,
                        ready: [(2, conn(2))].into_iter().collect(),
                        opening: [conn(9)].into_iter().collect(),
                    },
                }),
                requests: Vec::new(),
            },
            SocketRec {
                id: 2,
                transport: 1,
                non_blocking: true,
                modes: 3,
                port: 7,
                key: 0xabc,
                select_event: 8,
                select_mask: 0x81,
                recorded: 1,
                signal: 1,
                entry: Some(CarriedEntry {
                    local: Addr::v4([10, 0, 2, 15], 7),
                    kind: CarriedKind::Conn { conn: conn(1), connected: true },
                }),
                requests: [request].into_iter().collect(),
            },
            SocketRec {
                id: 5,
                transport: 0,
                entry: None,
                requests: [poll].into_iter().collect(),
                ..SocketRec::default()
            },
        ]
    }

    fn net() -> CarriedStack {
        CarriedStack {
            card: Some(CardConfig {
                ip: [10, 0, 2, 15],
                prefix: 24,
                gateway: [10, 0, 2, 2],
                mac: [0x52, 0x54, 0, 0x12, 0x34, 0x56],
            }),
            next_port: 49160,
            stopped_ns: 7_000_000_000,
            lingering: [conn(4)].into_iter().collect(),
        }
    }

    fn record() -> Vec<u8> {
        let mut w = Writer::new(1).unwrap();
        write_net(&mut w, &net()).unwrap();
        sockets_section(&mut w, false).unwrap();
        for s in sockets() {
            s.write(&mut w).unwrap();
        }
        w.finish().unwrap()
    }

    fn read_sockets(rec: &Record<'_>) -> Vec<SocketRec> {
        let mut r = rec.section(tag::SOCKETS).unwrap();
        let mut out = Vec::new();
        while let Some(mut e) = r.entry() {
            out.push(SocketRec::read(&mut e).unwrap());
        }
        out
    }

    #[test]
    fn round_trip() {
        let bytes = record();
        let rec = record::parse(&bytes).unwrap();
        assert_eq!(read_net(&mut rec.section(tag::NET).unwrap()).unwrap(), net());
        assert_eq!(read_sockets(&rec), sockets());
        // Both are REQUIRED, at this kernel's version.
        for s in rec.sections().flatten() {
            assert_eq!(s.flags & REQUIRED, REQUIRED);
            assert_eq!(s.version, version::writes(s.tag));
        }
    }

    /// SOCKETS 2: UDP sockets round trip, and only a record holding one
    /// needs a version-2 reader.
    #[test]
    fn udp_sockets_round_trip_and_need_version_2() {
        let mut w = Writer::new(1).unwrap();
        write_net(&mut w, &net()).unwrap();
        sockets_section(&mut w, true).unwrap();
        for s in sockets().into_iter().chain([udp_socket()]) {
            s.write(&mut w).unwrap();
        }
        let bytes = w.finish().unwrap();
        let rec = record::parse(&bytes).unwrap();
        let got = read_sockets(&rec);
        assert_eq!(got[3], udp_socket());
        assert_eq!(&got[..3], &sockets());
        let s = rec.sections().flatten().find(|s| s.tag == tag::SOCKETS).unwrap();
        assert_eq!((s.version, s.must_read), (2, 2));
        // Without a UDP socket a version-1 reader may take it.
        let rec_bytes = record();
        let rec = record::parse(&rec_bytes).unwrap();
        let s = rec.sections().flatten().find(|s| s.tag == tag::SOCKETS).unwrap();
        assert_eq!((s.version, s.must_read), (2, 0));
        assert_eq!(sockets_must_read(true), 2);
    }

    /// A version-1 writer's SOCKETS (no datagram fields) reads with them
    /// absent.
    #[test]
    fn version_1_sockets_are_read() {
        let mut w = Writer::new(1).unwrap();
        w.section_needing(tag::SOCKETS, 1, REQUIRED, 0).unwrap();
        let e = w.begin_entry().unwrap();
        for v in [2u32, 1, 0, 0, 0] {
            w.u32(v).unwrap(); // id, TCP, blocking, modes, port
        }
        w.u64(0).unwrap(); // key
        for _ in 0..4 {
            w.u32(0).unwrap(); // select event, mask, recorded, signal
        }
        w.u32(socket_kind::BOUND).unwrap();
        write_addr(&mut w, Addr::v4([0; 4], 80)).unwrap();
        for _ in 0..3 {
            w.u32(0).unwrap(); // a, b, connections
        }
        w.u32(1).unwrap(); // one request, in version 1's layout
        let q = w.begin_entry().unwrap();
        w.u32(request_kind::RECV).unwrap();
        w.u64(4).unwrap();
        w.u64(0x1000).unwrap();
        w.u32(0).unwrap();
        w.u64(0).unwrap();
        w.u32(0).unwrap();
        for _ in 0..4 {
            w.u64(0).unwrap();
        }
        w.u32(0).unwrap();
        for _ in 0..2 {
            w.u64(0).unwrap();
        }
        for _ in 0..3 {
            w.u32(0).unwrap();
        }
        w.table(1, 16).unwrap();
        w.u64(0x2000).unwrap();
        w.u64(64).unwrap();
        w.table(0, 16).unwrap();
        w.end_entry(q).unwrap();
        w.end_entry(e).unwrap();
        let bytes = w.finish().unwrap();
        let rec = record::parse(&bytes).unwrap();
        let got = read_sockets(&rec);
        assert_eq!(got.len(), 1);
        assert_eq!(
            got[0].entry,
            Some(CarriedEntry { local: Addr::v4([0; 4], 80), kind: CarriedKind::Bound })
        );
        let q = &got[0].requests[0];
        assert_eq!(
            (q.kind, q.iosb, q.buffers.as_slice()),
            (request_kind::RECV, 0x1000, &[(0x2000, 64)][..])
        );
        assert_eq!((q.address, q.address_len, q.to), (0, 0, None));
    }

    #[test]
    fn damage_is_refused_not_misread() {
        let bytes = record();
        let mut bad = bytes.clone();
        let n = bad.len();
        bad[n - 20] ^= 0x40;
        assert!(record::parse(&bad).is_err(), "the CRC catches it");
        // An entry cut short (as a damaged length would leave it): the
        // reader gives defaults or refuses, it never reads past the end.
        let mut w = Writer::new(1).unwrap();
        sockets_section(&mut w, false).unwrap();
        let e = w.begin_entry().unwrap();
        w.u32(3).unwrap(); // id
        w.u32(1).unwrap(); // TCP
        for _ in 0..9 {
            w.u32(0).unwrap();
        }
        w.u32(socket_kind::CONNECTION).unwrap();
        w.end_entry(e).unwrap();
        let bytes = w.finish().unwrap();
        let rec = record::parse(&bytes).unwrap();
        let mut r = rec.section(tag::SOCKETS).unwrap();
        let mut e = r.entry().unwrap();
        // A connection socket without its connection: damaged.
        assert_eq!(SocketRec::read(&mut e), Err(None));
        // An unknown role.
        let mut w = Writer::new(1).unwrap();
        sockets_section(&mut w, false).unwrap();
        let e = w.begin_entry().unwrap();
        for _ in 0..12 {
            w.u32(0).unwrap();
        }
        write_addr(&mut w, Addr::default()).unwrap();
        w.u32(0).unwrap();
        w.u32(0).unwrap();
        w.u32(1).unwrap();
        write_conn(&mut w, 77, 0, &conn(1)).unwrap();
        w.end_entry(e).unwrap();
        let bytes = w.finish().unwrap();
        let rec = record::parse(&bytes).unwrap();
        let mut e = rec.section(tag::SOCKETS).unwrap().entry().unwrap();
        assert_eq!(SocketRec::read(&mut e), Err(None));
    }

    /// A newer writer that appended fields to every entry: this reader
    /// takes what it knows and skips the rest.
    #[test]
    fn newer_writers_fields_are_skipped() {
        let mut w = Writer::new(1).unwrap();
        w.section_needing(tag::NET, 2, REQUIRED, 1).unwrap();
        let e = w.begin_entry().unwrap();
        w.u64(9).unwrap();
        w.u32(0).unwrap();
        for _ in 0..3 {
            w.u32(0).unwrap();
        }
        w.bytes(&[0; 8]).unwrap();
        w.u32(50000).unwrap();
        w.u64(0xdead_beef).unwrap(); // a field this reader does not know
        w.end_entry(e).unwrap();
        let bytes = w.finish().unwrap();
        let rec = record::parse(&bytes).unwrap();
        let s = read_net(&mut rec.section(tag::NET).unwrap()).unwrap();
        assert_eq!((s.stopped_ns, s.card, s.next_port), (9, None, 50000));
        // An older (shorter) header: the missing fields read as 0, their
        // defaults (no card, the counter left as the new kernel has it).
        let mut w = Writer::new(1).unwrap();
        w.section_needing(tag::NET, 1, REQUIRED, 1).unwrap();
        let e = w.begin_entry().unwrap();
        w.u64(9).unwrap();
        w.end_entry(e).unwrap();
        let bytes = w.finish().unwrap();
        let rec = record::parse(&bytes).unwrap();
        let s = read_net(&mut rec.section(tag::NET).unwrap()).unwrap();
        assert_eq!((s.stopped_ns, s.card, s.next_port, s.lingering.len()), (9, None, 0, 0));
    }
}

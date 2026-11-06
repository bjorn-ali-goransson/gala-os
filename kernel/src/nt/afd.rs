//! AFD: Windows sockets as file objects of `\Device\Afd`, driven by
//! NtDeviceIoControlFile, on the kernel's TCP API (`net::tcp`). Design,
//! IOCTL table and layout sources: docs/ARCHITECTURE.md, "AFD". Layouts
//! come from Wine's `include/wine/afd.h` (checked by Wine's tests against
//! Windows) and ReactOS's `drivers/afd/shared.h` (marked "to be verified"
//! there); see THIRD_PARTY.md.

use afd_abi::Layout;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};

use super::object::{CompletionPacket, Handle, KObject, ObjectBody, ObjectRef};
use super::process::Process;
use super::status::NtStatus;
use super::wait::{self, Dispatcher};
use crate::mm::heap::{KArc, TryVec};
use crate::net::tcp::{self, Addr, NetError, SocketId};
use crate::net::udp;
use crate::net::{Ip, Scope};
use crate::quota::Block;
use crate::sync::IrqMutex;

pub const IOCTL_AFD_BIND: u32 = 0x12003;
pub const IOCTL_AFD_CONNECT: u32 = 0x12007;
pub const IOCTL_AFD_LISTEN: u32 = 0x1200b;
pub const IOCTL_AFD_WAIT_FOR_LISTEN: u32 = 0x1200c;
pub const IOCTL_AFD_ACCEPT: u32 = 0x12010;
pub const IOCTL_AFD_RECV: u32 = 0x12017;
pub const IOCTL_AFD_RECV_DATAGRAM: u32 = 0x1201b;
pub const IOCTL_AFD_SEND: u32 = 0x1201f;
pub const IOCTL_AFD_SEND_DATAGRAM: u32 = 0x12023;
pub const IOCTL_AFD_DISCONNECT: u32 = 0x1202b;
pub const IOCTL_AFD_GETSOCKNAME: u32 = 0x1202f;
pub const IOCTL_AFD_GET_PEER_NAME: u32 = 0x12033;
pub const IOCTL_AFD_SUPER_ACCEPT: u32 = 0x12083;
pub const IOCTL_AFD_POLL: u32 = 0x12024;
pub const IOCTL_AFD_SET_INFO: u32 = 0x1203b;
const AFD_INFO_BLOCKING_MODE: u32 = 2;
/// SO_RCVBUF and SO_SNDBUF (ReactOS's `drivers/afd/shared.h`: a ULONG at
/// 8): the sizes of the socket's buffers (`tcp::Owner`).
const AFD_INFO_RECEIVE_WINDOW_SIZE: u32 = 6;
const AFD_INFO_SEND_WINDOW_SIZE: u32 = 7;
/// gala's own AFD_INFO class (Windows passes IPV6_V6ONLY to the transport,
/// whose interface is undocumented): a BOOLEAN at 8, IPV6_V6ONLY.
const GALA_AFD_INFO_V6ONLY: u32 = 0x6761_0001;
pub const IOCTL_AFD_SUPER_CONNECT: u32 = 0x120c7;
/// WSAEventSelect and WSAEnumNetworkEvents (the numbers of Wine's afd.h).
pub const IOCTL_AFD_EVENT_SELECT: u32 = 0x12087;
pub const IOCTL_AFD_GET_EVENTS: u32 = 0x1208b;

const AFD_DISCONNECT_SEND: u32 = 1;
const AFD_MSG_OOB: u32 = 0x40;
const AFD_MSG_PEEK: u32 = 0x80;
const AFD_MSG_WAITALL: u32 = 0x4000;
const AFD_DISCONNECT_ABORT: u32 = 4;

/// The extended attribute NtCreateFile passes the open packet in.
const OPEN_PACKET_NAME: &[u8] = b"AfdOpenPacketXX";

const AF_INET: u16 = 2;
const AF_INET6: u16 = 23;
/// SOCKADDR_IN.
const SOCKADDR_LEN: usize = 16;
/// SOCKADDR_IN6.
const SOCKADDR6_LEN: usize = 28;
/// TRANSPORT_ADDRESS with one TA_ADDRESS holding a TDI_ADDRESS_IP.
const TRANSPORT_ADDRESS_LEN: usize = 22;
/// With TDI_ADDRESS_IP6.
const TRANSPORT_ADDRESS6_LEN: usize = 34;
/// Bytes copied through the kernel per step of a send or receive.
const CHUNK: usize = 16 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transport {
    /// No open packet: a helper handle (e.g. for IOCTL_AFD_POLL).
    None,
    Tcp,
    /// A datagram endpoint (`\Device\Udp`, `net::udp`).
    Udp,
    /// The same for IPv6 (`\Device\Tcp6`, `\Device\Udp6`: Windows'
    /// transport names; N3).
    Tcp6,
    Udp6,
}

impl Transport {
    /// The number the handover record gives it (SocketRec::transport).
    pub fn number(self) -> u32 {
        match self {
            Transport::None => 0,
            Transport::Tcp => 1,
            Transport::Udp => 2,
            Transport::Tcp6 => 3,
            Transport::Udp6 => 4,
        }
    }

    pub fn from_number(n: u32) -> Option<Self> {
        Some(match n {
            0 => Transport::None,
            1 => Transport::Tcp,
            2 => Transport::Udp,
            3 => Transport::Tcp6,
            4 => Transport::Udp6,
            _ => return None,
        })
    }

    pub fn is_udp(self) -> bool {
        matches!(self, Transport::Udp | Transport::Udp6)
    }

    /// Whether it is an IPv6 endpoint (its addresses go out as
    /// SOCKADDR_IN6, IPv4 ones IPv4-mapped).
    pub fn is_v6(self) -> bool {
        matches!(self, Transport::Tcp6 | Transport::Udp6)
    }

    /// The unspecified address of its family (an implicit bind's).
    pub fn any_address(self) -> Addr {
        match self {
            Transport::Tcp6 | Transport::Udp6 => Addr { ip: Ip::V6([0; 16]), port: 0 },
            _ => Addr::default(),
        }
    }
}

impl EndpointState {
    /// Its owner for the stack: its token, quota and buffer sizes.
    fn owner(&self) -> tcp::Owner {
        tcp::Owner {
            token: self.token,
            purse: self.purse.clone(),
            rx: self.rcvbuf.load(Ordering::Acquire) as usize,
            tx: self.sndbuf.load(Ordering::Acquire) as usize,
            unchecked: false,
        }
    }

    /// The families its socket takes: IPv4; IPv6 only; or both (an IPv6
    /// endpoint with IPV6_V6ONLY off).
    fn scope(&self) -> Scope {
        match (self.transport.is_v6(), self.v6only.load(Ordering::Acquire)) {
            (false, _) => Scope::V4,
            (true, true) => Scope::V6,
            (true, false) => Scope::Dual,
        }
    }
}

/// FILE_SKIP_COMPLETION_PORT_ON_SUCCESS / FILE_SKIP_SET_EVENT_ON_HANDLE
/// (FileIoCompletionNotificationInformation).
pub const FILE_SKIP_COMPLETION_PORT_ON_SUCCESS: u32 = 1;
pub const FILE_SKIP_SET_EVENT_ON_HANDLE: u32 = 2;

/// Where a request's completion is reported.
pub struct Io {
    pub process: KArc<Process>,
    /// IO_STATUS_BLOCK (an OVERLAPPED's Internal/InternalHigh), or 0.
    pub iosb: u64,
    pub event: Option<ObjectRef>,
    /// ApcContext: the completion packet's context (the OVERLAPPED); a
    /// request with none posts no packet.
    pub context: u64,
    /// ApcRoutine: queued to the calling thread when the request
    /// completes (a completion routine; such a request posts no packet).
    pub apc: Option<(ObjectRef, super::thread::UserApc)>,
    /// Its completion's packet and APC, reserved when it is submitted.
    pub reserved: super::notify::Reserved,
}

/// An operation waiting for the network (docs/ARCHITECTURE.md, "AFD").
enum Op {
    /// `peek`: leave the bytes in the socket (AFD_MSG_PEEK).
    Recv {
        buffers: Vec<(u64, usize)>,
        peek: bool,
    },
    /// `done` bytes are already queued.
    Send {
        buffers: Vec<(u64, usize)>,
        done: usize,
    },
    /// A datagram received (RECV_DATAGRAM, or RECV on a datagram
    /// endpoint): the sender's SOCKADDR_IN goes to `address` and its
    /// length to the INT at `address_len` (0: not asked for).
    RecvFrom {
        buffers: Vec<(u64, usize)>,
        peek: bool,
        address: u64,
        address_len: u64,
    },
    /// A datagram sent (SEND_DATAGRAM to `to`; SEND, None: the peer).
    SendTo {
        buffers: Vec<(u64, usize)>,
        to: Option<Addr>,
    },
    WaitForListen {
        output: u64,
        output_len: u32,
    },
    /// AcceptEx: the accepting endpoint and the output buffer's layout.
    SuperAccept {
        accept: KArc<EndpointState>,
        output: u64,
        receive: u32,
        local: u32,
    },
    Connect,
    /// IOCTL_AFD_POLL: the sockets and the events asked for; the output
    /// buffer receives the ones that happened.
    Poll {
        sockets: Vec<(u64, KArc<EndpointState>, u32)>,
        output: u64,
        output_len: u32,
        wait: bool,
    },
}

impl Op {
    fn kind(&self) -> u8 {
        match self {
            Op::Recv { .. } | Op::RecvFrom { .. } => 0,
            Op::Send { .. } | Op::SendTo { .. } => 1,
            Op::WaitForListen { .. } | Op::SuperAccept { .. } => 2,
            Op::Connect => 3,
            Op::Poll { .. } => 4,
        }
    }
}

struct Pending {
    op: Op,
    io: Io,
}

enum Attempt {
    Done(NtStatus, u64),
    Wait,
}

/// An endpoint's state, shared by its object and the token registry.
pub struct EndpointState {
    pub dispatcher: Dispatcher,
    pub transport: Transport,
    token: u64,
    socket: IrqMutex<Option<SocketId>>,
    /// The completion port and key (FileCompletionInformation).
    pub completion: IrqMutex<Option<(ObjectRef, u64)>>,
    /// FILE_SKIP_* notification modes.
    pub modes: AtomicU32,
    /// Non-blocking mode (FIONBIO): requests without a completion context
    /// fail with STATUS_DEVICE_NOT_READY instead of pending.
    non_blocking: AtomicBool,
    /// IPV6_V6ONLY of an IPv6 endpoint (on, Windows' default; off: dual
    /// stack), taken at bind (`scope`).
    v6only: AtomicBool,
    /// The quota its sockets' buffers are charged to: its creator's (none
    /// for an endpoint a kernel handover carried: its process's quota is
    /// restored after it).
    purse: Option<KArc<Block>>,
    /// SO_RCVBUF and SO_SNDBUF (AFD_INFO_*_WINDOW_SIZE; 0: the default),
    /// for the sockets it makes from then on.
    rcvbuf: AtomicU32,
    sndbuf: AtomicU32,
    pending: IrqMutex<Vec<Pending>>,
    /// How many polls (IOCTL_AFD_POLL) are pending: `notify` keeps the
    /// endpoint in POLLERS while there are any, without taking `pending`
    /// under POLLERS' lock (the lock order is pending, then POLLERS).
    polls: AtomicUsize,
    /// WSAEventSelect: the event and the AFD_POLL_* events it is set for.
    select: IrqMutex<Option<(ObjectRef, u32)>>,
    /// The selected events that happened since WSAEnumNetworkEvents last
    /// read them.
    recorded: AtomicU32,
    /// The sending side (`send_side`): open, shut down with the FIN
    /// waiting for the sends issued before it, or shut down. Changed under
    /// `pending`'s lock.
    send_side: AtomicU32,
}

/// `EndpointState::send_side` (also carried across a handover).
pub mod send_side {
    pub const OPEN: u32 = 0;
    /// shutdown(SD_SEND) came while sends were pending: the FIN follows
    /// once none is (`fin_after_sends`).
    pub const AFTER_SENDS: u32 = 1;
    pub const SHUT: u32 = 2;
}

/// A socket endpoint (`ObjectBody::Socket`).
pub struct Endpoint {
    pub state: KArc<EndpointState>,
}

impl Drop for Endpoint {
    fn drop(&mut self) {
        REGISTRY.lock().retain(|e| e.token != self.state.token);
        // Closing the last handle cancels what is still pending.
        cancel(&self.state, None, None);
        if let Some(id) = self.state.socket.lock().take() {
            tcp::close(id, false);
        }
    }
}

/// Endpoints by token, for the notify hook.
static REGISTRY: IrqMutex<Vec<KArc<EndpointState>>> = IrqMutex::new(Vec::new());
static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);

/// `net::tcp`'s notify hook (in the `net` thread): the endpoint's socket
/// changed; complete what can be completed now.
fn notify(token: u64) {
    let state = REGISTRY.lock().iter().find(|e| e.token == token).cloned();
    if let Some(state) = state {
        try_complete(&state);
        check_select(&state);
    }
    // A poll waits on other endpoints' sockets: retry every pending poll.
    let pollers: Vec<KArc<EndpointState>> = {
        let mut list = POLLERS.lock();
        let mut copy = Vec::new();
        if copy.try_reserve(list.len()).is_ok() {
            list.retain(|e| e.polls.load(Ordering::Acquire) > 0);
            for e in list.iter() {
                let _ = copy.try_push(e.clone());
            }
        }
        copy
    };
    for state in pollers {
        try_complete(&state);
    }
}

/// Endpoints with a pending poll.
static POLLERS: IrqMutex<Vec<KArc<EndpointState>>> = IrqMutex::new(Vec::new());

// IOCTL_AFD_POLL event bits (Wine's afd.h).
const AFD_POLL_READ: u32 = 0x1;
const AFD_POLL_WRITE: u32 = 0x4;
const AFD_POLL_HUP: u32 = 0x8;
const AFD_POLL_CONNECT: u32 = 0x40;
const AFD_POLL_ACCEPT: u32 = 0x80;
const AFD_POLL_CONNECT_ERR: u32 = 0x100;

/// The AFD_POLL_* events of one endpoint now.
fn poll_events(state: &EndpointState) -> u32 {
    let Some(id) = *state.socket.lock() else {
        // An unbound datagram socket can send (it is bound then).
        return if state.transport.is_udp() { AFD_POLL_WRITE } else { 0 };
    };
    let Ok(r) = tcp::readiness(id) else { return AFD_POLL_HUP };
    let mut events = 0;
    for (on, bit) in [
        (r.readable, AFD_POLL_READ),
        (r.writable, AFD_POLL_WRITE),
        (r.hangup, AFD_POLL_HUP),
        (r.acceptable, AFD_POLL_ACCEPT),
        (r.connected, AFD_POLL_CONNECT),
        (r.refused, AFD_POLL_CONNECT_ERR),
    ] {
        if on {
            events |= bit;
        }
    }
    events
}

/// WSAEventSelect's half of a readiness change: the selected events that
/// hold now are recorded and the event is set.
fn check_select(state: &EndpointState) {
    let Some((event, mask)) = state.select.lock().clone() else { return };
    let ready = poll_events(state) & mask;
    if ready != 0 {
        state.recorded.fetch_or(ready, Ordering::AcqRel);
        wait::set_signal(&event, 1);
    }
}

/// IOCTL_AFD_EVENT_SELECT (WSAEventSelect): `event` is set when one of
/// the AFD_POLL_* events in `mask` happens (or holds now); no event or no
/// mask ends the selection. The socket becomes non-blocking, as
/// documented.
pub fn event_select(socket: &ObjectRef, event: Option<ObjectRef>, mask: u32) -> NtStatus {
    let ObjectBody::Socket(endpoint) = &socket.body else { return NtStatus::INVALID_HANDLE };
    let state = &endpoint.state;
    *state.select.lock() = match event {
        Some(e) if mask != 0 => Some((e, mask)),
        _ => None,
    };
    state.recorded.store(0, Ordering::Release);
    state.non_blocking.store(true, Ordering::Release);
    check_select(state);
    NtStatus::SUCCESS
}

/// IOCTL_AFD_GET_EVENTS (WSAEnumNetworkEvents): the recorded events,
/// cleared.
pub fn get_events(socket: &ObjectRef) -> Result<u32, NtStatus> {
    let ObjectBody::Socket(endpoint) = &socket.body else { return Err(NtStatus::INVALID_HANDLE) };
    Ok(endpoint.state.recorded.swap(0, Ordering::AcqRel))
}

/// Reports a finished request: IO_STATUS_BLOCK, event, the endpoint's own
/// signal, the completion port. `immediate`: it finished inside its system
/// call (then a failure posts nothing, and FILE_SKIP_COMPLETION_PORT_ON_
/// SUCCESS suppresses the packet of a success).
fn finish(state: &EndpointState, io: &Io, status: NtStatus, information: u64, immediate: bool) {
    if io.iosb != 0 {
        let _ = io.process.write_iosb(io.iosb, status, information);
    }
    let failed = status.0 >= 0xc000_0000;
    if immediate && failed {
        return;
    }
    if let Some(e) = &io.event {
        wait::set_signal(e, 1);
    }
    if let Some((thread, apc)) = &io.apc {
        io.reserved.queue_apc(thread, *apc);
    }
    let modes = state.modes.load(Ordering::Acquire);
    if modes & FILE_SKIP_SET_EVENT_ON_HANDLE == 0 {
        wait::signal_dispatcher(&state.dispatcher, 1);
    }
    if immediate && modes & FILE_SKIP_COMPLETION_PORT_ON_SUCCESS != 0 {
        return;
    }
    let port = state.completion.lock().clone();
    if let (Some((port, key)), true) = (port, io.context != 0 && io.apc.is_none()) {
        let packet = CompletionPacket::new(key, io.context, status.0, information);
        io.reserved.post(&port, packet);
    }
}

fn user_read(process: &Process, addr: u64, buf: &mut [u8]) -> Result<(), NtStatus> {
    process.read(addr, buf).map_err(|_| NtStatus::ACCESS_VIOLATION)
}

fn user_write(process: &Process, addr: u64, data: &[u8]) -> Result<(), NtStatus> {
    process.write(addr, data).map_err(|_| NtStatus::ACCESS_VIOLATION)
}

/// Tries an operation once, without waiting.
fn attempt(state: &EndpointState, op: &mut Op, process: &Process) -> Attempt {
    let socket = *state.socket.lock();
    let non_blocking = state.non_blocking.load(Ordering::Acquire);
    match attempt_inner(socket, op, process, non_blocking, state.transport.is_v6()) {
        Ok(Some(information)) => Attempt::Done(NtStatus::SUCCESS, information),
        Ok(None) | Err(Failure::Net(NetError::WouldBlock)) => Attempt::Wait,
        Err(Failure::Net(e)) => Attempt::Done(status_of(e), 0),
        Err(Failure::Nt(status)) => Attempt::Done(status, 0),
        Err(Failure::Partly(status, information)) => Attempt::Done(status, information),
    }
}

enum Failure {
    Net(NetError),
    Nt(NtStatus),
    /// Done with a warning and a byte count (a truncated datagram:
    /// STATUS_BUFFER_OVERFLOW with the bytes copied).
    Partly(NtStatus, u64),
}

impl From<NetError> for Failure {
    fn from(e: NetError) -> Self {
        Failure::Net(e)
    }
}

impl From<NtStatus> for Failure {
    fn from(s: NtStatus) -> Self {
        Failure::Nt(s)
    }
}

/// `Ok(Some(information))` when done, `Ok(None)` to keep waiting.
/// `v6`: the endpoint is IPv6 (addresses go out as SOCKADDR_IN6).
fn attempt_inner(
    socket: Option<SocketId>,
    op: &mut Op,
    process: &Process,
    non_blocking: bool,
    v6: bool,
) -> Result<Option<u64>, Failure> {
    if let Op::Poll { sockets, output, output_len, wait } = op {
        return poll_once(sockets, *output, *output_len, *wait, process);
    }
    let id = socket.ok_or(Failure::Nt(NtStatus::INVALID_CONNECTION))?;
    match op {
        Op::Recv { buffers, peek } => {
            let capacity: usize = buffers.iter().map(|&(_, l)| l).sum();
            let mut buf = bounce()?;
            let want = capacity.min(CHUNK);
            let got = if *peek {
                tcp::peek(id, &mut buf[..want])?
            } else {
                tcp::recv(id, &mut buf[..want])?
            };
            let mut copied = 0;
            for &(addr, len) in buffers.iter() {
                if copied == got {
                    break;
                }
                let n = len.min(got - copied);
                user_write(process, addr, &buf[copied..copied + n])?;
                copied += n;
            }
            Ok(Some(got as u64))
        }
        Op::Send { buffers, done } => {
            let total: usize = buffers.iter().map(|&(_, l)| l).sum();
            let mut buf = bounce()?;
            while *done < total {
                // The buffer and offset holding byte `done`.
                let (mut at, mut skip) = (0, *done);
                while skip >= buffers[at].1 {
                    skip -= buffers[at].1;
                    at += 1;
                }
                let (addr, len) = buffers[at];
                let n = (len - skip).min(CHUNK);
                user_read(process, addr + skip as u64, &mut buf[..n])?;
                match tcp::send(id, &buf[..n]) {
                    Ok(queued) => *done += queued,
                    Err(NetError::WouldBlock) => return Ok(None),
                    Err(e) => return Err(e.into()),
                }
            }
            Ok(Some(total as u64))
        }
        Op::RecvFrom { buffers, peek, address, address_len } => {
            let capacity: usize = buffers.iter().map(|&(_, l)| l).sum();
            let mut buf = bounce()?;
            // A datagram is at most an MTU, less than CHUNK.
            let got = udp::recv_from(id, &mut buf[..capacity.min(CHUNK)], *peek)?;
            let mut copied = 0;
            for &(addr, len) in buffers.iter() {
                if copied == got.copied {
                    break;
                }
                let n = len.min(got.copied - copied);
                user_write(process, addr, &buf[copied..copied + n])?;
                copied += n;
            }
            if *address != 0 {
                let (b, n) = to_sockaddr(got.from, v6);
                user_write(process, *address, &b[..n])?;
            }
            if *address_len != 0 {
                let n = to_sockaddr(got.from, v6).1;
                user_write(process, *address_len, &(n as i32).to_le_bytes())?;
            }
            if got.length > got.copied {
                // The rest of the datagram is gone (unless peeking).
                return Err(Failure::Partly(NtStatus::BUFFER_OVERFLOW, got.copied as u64));
            }
            Ok(Some(got.copied as u64))
        }
        Op::SendTo { buffers, to } => {
            let total: usize = buffers.iter().map(|&(_, l)| l).sum();
            if total > CHUNK {
                return Err(NetError::TooLarge.into());
            }
            let mut buf = bounce()?;
            let mut at = 0;
            for &(addr, len) in buffers.iter() {
                user_read(process, addr, &mut buf[at..at + len])?;
                at += len;
            }
            Ok(Some(udp::send_to(id, *to, &buf[..total])? as u64))
        }
        Op::WaitForListen { output, output_len } => {
            let (sequence, remote) = tcp::pending(id)?;
            let (ta, n) = to_transport_address(remote, v6);
            let mut out = [0u8; 4 + TRANSPORT_ADDRESS6_LEN];
            out[..4].copy_from_slice(&sequence.to_le_bytes());
            out[4..4 + n].copy_from_slice(&ta[..n]);
            let out = &out[..4 + n];
            if (*output_len as usize) < out.len() {
                return Err(NtStatus::BUFFER_TOO_SMALL.into());
            }
            user_write(process, *output, out)?;
            Ok(Some(out.len() as u64))
        }
        Op::SuperAccept { accept, output, receive, local } => {
            let (sequence, remote) = tcp::pending(id)?;
            let mut slot = accept.socket.lock();
            if slot.is_some() {
                return Err(NtStatus::INVALID_PARAMETER.into());
            }
            let new = tcp::accept(id, sequence, accept.owner())?;
            // Like Windows, an accepted socket inherits the listener's
            // non-blocking mode (Nginx relies on it).
            accept.non_blocking.store(non_blocking, Ordering::Release);
            *slot = Some(new);
            drop(slot);
            let local_addr = tcp::local(new)?;
            let at = *output + u64::from(*receive);
            let v6 = accept.transport.is_v6();
            let ((lb, ln), (rb, rn)) = (to_sockaddr(local_addr, v6), to_sockaddr(remote, v6));
            if ln > *local as usize {
                return Err(NtStatus::BUFFER_TOO_SMALL.into());
            }
            user_write(process, at, &lb[..ln])?;
            user_write(process, at + u64::from(*local), &rb[..rn])?;
            Ok(Some(0))
        }
        Op::Connect => {
            tcp::connected(id)?;
            Ok(Some(0))
        }
        Op::Poll { .. } => unreachable!("handled above"),
    }
}

/// One look at a poll's sockets: writes the ones with events (Wine's
/// layout: the header, then {SOCKET; flags; status} per socket, count =
/// how many) once any has one, or at once if the poll does not wait.
fn poll_once(
    sockets: &[(u64, KArc<EndpointState>, u32)],
    output: u64,
    output_len: u32,
    wait: bool,
    process: &Process,
) -> Result<Option<u64>, Failure> {
    let l = layout_of(process);
    let mut out = Vec::new();
    out.try_reserve(Layout::POLL_HEADER + l.poll_entry() * sockets.len())
        .map_err(|_| NtStatus::NO_MEMORY)?;
    out.resize(Layout::POLL_HEADER, 0);
    let mut count = 0u32;
    for (handle, state, wanted) in sockets {
        let events = poll_events(state) & wanted;
        if events != 0 {
            // Capacity is reserved above: {HANDLE; events; status}.
            let at = out.len();
            out.resize(at + l.poll_entry(), 0);
            l.put_ptr(&mut out, at, *handle);
            out[at + l.poll_events()..][..4].copy_from_slice(&events.to_le_bytes());
            count += 1;
        }
    }
    if count == 0 && wait {
        return Ok(None);
    }
    out[8..12].copy_from_slice(&count.to_le_bytes());
    if out.len() > output_len as usize {
        return Err(NtStatus::BUFFER_TOO_SMALL.into());
    }
    user_write(process, output, &out)?;
    Ok(Some(out.len() as u64))
}

/// The layouts of `process`'s AFD requests: its pointers' width (a WOW64
/// process's are 4 bytes; afd_abi).
fn layout_of(process: &Process) -> Layout {
    if process.wow64.is_some() { Layout::X86 } else { Layout::X64 }
}

/// Completes the pending requests that can finish now, oldest first
/// (a receive, send, accept or connect waits while an older one of its
/// kind does: they consume the socket's stream in order). Polls are
/// independent: wepoll keeps one per socket pending on the same helper
/// handle, and each completes when its own socket is ready.
fn try_complete(state: &EndpointState) {
    let mut pending = state.pending.lock();
    let mut blocked = [false; 5];
    let mut i = 0;
    while i < pending.len() {
        let kind = usize::from(pending[i].op.kind());
        if blocked[kind] {
            i += 1;
            continue;
        }
        let p = &mut pending[i];
        match attempt(state, &mut p.op, &p.io.process) {
            Attempt::Done(status, information) => {
                let p = pending.remove(i);
                if usize::from(p.op.kind()) == POLL_KIND {
                    state.polls.fetch_sub(1, Ordering::AcqRel);
                }
                if let Op::Send { buffers, done } = &p.op
                    && status.0 >= 0xc000_0000
                {
                    // Its first `done` bytes are in the stream, the rest
                    // never will be: a peer that comes up short says how
                    // much from here.
                    let total: usize = buffers.iter().map(|&(_, l)| l).sum();
                    afd_event(state, "afd send failed", |f| {
                        write!(f, r#","status":{},"queued":{done},"of":{total}"#, status.0)
                    });
                }
                finish(state, &p.io, status, information, false);
            }
            Attempt::Wait => {
                blocked[kind] = kind != POLL_KIND;
                i += 1;
            }
        }
    }
    fin_after_sends(state, &pending);
}

/// `Op::kind` of a send.
const SEND_KIND: u8 = 1;

/// The FIN of a shutdown(SD_SEND) that came while sends were pending,
/// once none is (they completed, failed or were cancelled). Called with
/// `pending` locked.
fn fin_after_sends(state: &EndpointState, pending: &[Pending]) {
    if state.send_side.load(Ordering::Acquire) != send_side::AFTER_SENDS
        || pending.iter().any(|p| p.op.kind() == SEND_KIND)
    {
        return;
    }
    state.send_side.store(send_side::SHUT, Ordering::Release);
    let socket = *state.socket.lock();
    let result = socket.ok_or(NetError::NotConnected).and_then(tcp::shutdown_send);
    if let Err(e) = result {
        afd_event(state, "afd shutdown failed", |f| write!(f, r#","error":"{e:?}""#));
    }
}

/// An endpoint event (`events=net`): its kernel socket's number (as the
/// "tcp ..." events name it, -1 without one), then `fields` (each
/// starting with a comma).
fn afd_event(
    state: &EndpointState,
    what: &str,
    fields: impl Fn(&mut core::fmt::Formatter<'_>) -> core::fmt::Result,
) {
    struct Fields<F>(F);
    impl<F: Fn(&mut core::fmt::Formatter<'_>) -> core::fmt::Result> core::fmt::Display for Fields<F> {
        fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            (self.0)(f)
        }
    }
    let socket = state.socket.lock().map_or(-1, |id| i64::from(id.number()));
    crate::events::emit(
        crate::events::Category::Net,
        what,
        format_args!(r#""socket":{socket}{}"#, Fields(fields)),
    );
}

/// `Op::kind` of a poll.
const POLL_KIND: usize = 4;

/// What a request's completion will post, reserved when it starts
/// (`notify::Reserved`): the packet to the endpoint's port, if the
/// request has an ApcContext and no routine, and its routine's APC.
fn reserve(state: &EndpointState, io: &Io) -> Result<super::notify::Reserved, NtStatus> {
    let port = state.completion.lock().clone();
    let port = port.as_ref().filter(|_| io.context != 0 && io.apc.is_none()).map(|(p, _)| p);
    super::notify::Reserved::new(port, io.apc.as_ref().map(|(t, _)| t))
}

/// Starts a request: done at once if possible, else pending.
fn submit(state: &EndpointState, mut op: Op, mut io: Io) -> NtStatus {
    let mut pending = state.pending.lock();
    io.reserved = match reserve(state, &io) {
        Ok(r) => r,
        Err(status) => return status,
    };
    let kind = op.kind();
    // A non-blocking socket's synchronous connect never completes in the
    // call, as on Windows (WSAEWOULDBLOCK, then writable): over loopback
    // the handshake can finish on another CPU before this attempt, and a
    // program may take a 0 for impossible (Chromium's tcp_socket_win.cc
    // NOTREACHED; winsock_test).
    let nonblocking_connect = matches!(op, Op::Connect)
        && io.context == 0
        && io.apc.is_none()
        && state.non_blocking.load(Ordering::Acquire);
    if !nonblocking_connect
        && (usize::from(kind) == POLL_KIND || !pending.iter().any(|p| p.op.kind() == kind))
        && let Attempt::Done(status, information) = attempt(state, &mut op, &io.process)
    {
        finish(state, &io, status, information, true);
        return status;
    }
    if io.context == 0
        && io.apc.is_none()
        && kind != 4
        && state.non_blocking.load(Ordering::Acquire)
    {
        // A non-blocking socket's synchronous call: a send that queued part
        // of its bytes reports that part (as Windows does: the bytes are
        // in the socket, the caller must not send them again); otherwise
        // "would block".
        if let Op::Send { done, .. } = op
            && done > 0
        {
            finish(state, &io, NtStatus::SUCCESS, done as u64, true);
            return NtStatus::SUCCESS;
        }
        return NtStatus::DEVICE_NOT_READY;
    }
    if kind == 4 {
        // Counted before it is registered, so `notify` never drops it
        // from POLLERS in between.
        state.polls.fetch_add(1, Ordering::AcqRel);
        let token = state.token;
        let mut pollers = POLLERS.lock();
        if !pollers.iter().any(|e| e.token == token) {
            let me = REGISTRY.lock().iter().find(|e| e.token == token).cloned();
            if let Some(me) = me
                && pollers.try_push(me).is_err()
            {
                state.polls.fetch_sub(1, Ordering::AcqRel);
                return NtStatus::INSUFFICIENT_RESOURCES;
            }
        }
    }
    if pending.try_push(Pending { op, io }).is_err() {
        if kind == 4 {
            state.polls.fetch_sub(1, Ordering::AcqRel);
        }
        return NtStatus::INSUFFICIENT_RESOURCES;
    }
    NtStatus::PENDING
}

/// Cancels pending requests of `process` (all processes if None) whose
/// IO_STATUS_BLOCK is `iosb` (any if None); returns how many.
pub fn cancel(state: &EndpointState, process: Option<&KArc<Process>>, iosb: Option<u64>) -> usize {
    let mut pending = state.pending.lock();
    let mut n = 0;
    let mut i = 0;
    while i < pending.len() {
        let p = &pending[i];
        let mine = process.is_none_or(|q| KArc::ptr_eq(q, &p.io.process));
        if mine && iosb.is_none_or(|b| b == p.io.iosb) {
            let p = pending.remove(i);
            if usize::from(p.op.kind()) == POLL_KIND {
                state.polls.fetch_sub(1, Ordering::AcqRel);
            }
            finish(state, &p.io, NtStatus::CANCELLED, 0, false);
            n += 1;
        } else {
            i += 1;
        }
    }
    fin_after_sends(state, &pending);
    n
}

/// Creates `\Device\Afd` and connects the notify hook.
pub fn init() -> Result<(), NtStatus> {
    let device =
        KObject::with_permanence(ObjectBody::AfdDevice, true).map_err(|_| NtStatus::NO_MEMORY)?;
    let name: Vec<u16> = "\\Device\\Afd".encode_utf16().collect_try()?;
    super::namespace::create(None, &name, device, true, false)?;
    tcp::set_notify(notify);
    Ok(())
}

trait CollectTry {
    fn collect_try(self) -> Result<Vec<u16>, NtStatus>;
}

impl<I: Iterator<Item = u16>> CollectTry for I {
    fn collect_try(self) -> Result<Vec<u16>, NtStatus> {
        let mut v = Vec::new();
        for c in self {
            v.try_push(c).map_err(|_| NtStatus::NO_MEMORY)?;
        }
        Ok(v)
    }
}

/// Finds the open packet in an EA buffer (FILE_FULL_EA_INFORMATION list)
/// and returns its transport. Packet (ReactOS AFD_CREATE_PACKET):
/// `{u32 endpoint_flags; u32 group; u32 transport_name_bytes; WCHAR name[]}`.
pub fn parse_open_packet(ea: &[u8]) -> Result<Transport, NtStatus> {
    let mut offset = 0usize;
    loop {
        let entry = ea.get(offset..).ok_or(NtStatus::INVALID_PARAMETER)?;
        if entry.len() < 8 {
            return Err(NtStatus::INVALID_PARAMETER);
        }
        let next = u32::from_le_bytes([entry[0], entry[1], entry[2], entry[3]]) as usize;
        let name_len = usize::from(entry[5]);
        let value_len = usize::from(u16::from_le_bytes([entry[6], entry[7]]));
        let name = entry.get(8..8 + name_len).ok_or(NtStatus::INVALID_PARAMETER)?;
        let value_at = 8 + name_len + 1;
        let value = entry.get(value_at..value_at + value_len).ok_or(NtStatus::INVALID_PARAMETER)?;
        if name == OPEN_PACKET_NAME {
            return transport_of(value);
        }
        if next == 0 {
            return Ok(Transport::None);
        }
        offset += next;
    }
}

fn transport_of(packet: &[u8]) -> Result<Transport, NtStatus> {
    let field = |at: usize| -> Result<u32, NtStatus> {
        let b = packet.get(at..at + 4).ok_or(NtStatus::INVALID_PARAMETER)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };
    let name_bytes = field(8)? as usize;
    let name = packet.get(12..12 + name_bytes).ok_or(NtStatus::INVALID_PARAMETER)?;
    let tcp: &[u8] = &[
        b'\\', 0, b'D', 0, b'e', 0, b'v', 0, b'i', 0, b'c', 0, b'e', 0, b'\\', 0, b'T', 0, b'c', 0,
        b'p', 0,
    ];
    let udp: &[u8] = &[
        b'\\', 0, b'D', 0, b'e', 0, b'v', 0, b'i', 0, b'c', 0, b'e', 0, b'\\', 0, b'U', 0, b'd', 0,
        b'p', 0,
    ];
    let six: &[u8] = &[b'6', 0];
    if name.eq_ignore_ascii_case(tcp) {
        Ok(Transport::Tcp)
    } else if name.eq_ignore_ascii_case(udp) {
        Ok(Transport::Udp)
    } else if name.len() == tcp.len() + 2
        && name.ends_with(six)
        && name[..tcp.len()].eq_ignore_ascii_case(tcp)
    {
        Ok(Transport::Tcp6)
    } else if name.len() == udp.len() + 2
        && name.ends_with(six)
        && name[..udp.len()].eq_ignore_ascii_case(udp)
    {
        Ok(Transport::Udp6)
    } else {
        super::ledger::record("NtCreateFile", "afd transport", name_bytes as u64);
        Err(NtStatus::NOT_SUPPORTED)
    }
}

/// Opens an endpoint (NtCreateFile below `\Device\Afd`) for the process
/// with quota block `purse`.
pub fn open(ea: &[u8], purse: &KArc<Block>) -> Result<ObjectRef, NtStatus> {
    let transport = if ea.is_empty() { Transport::None } else { parse_open_packet(ea)? };
    let token = NEXT_TOKEN.fetch_add(1, Ordering::Relaxed);
    let state = KArc::try_new(EndpointState {
        dispatcher: Dispatcher::new(0),
        transport,
        token,
        socket: IrqMutex::new(None),
        completion: IrqMutex::new(None),
        modes: AtomicU32::new(0),
        non_blocking: AtomicBool::new(false),
        v6only: AtomicBool::new(true),
        purse: Some(purse.clone()),
        rcvbuf: AtomicU32::new(0),
        sndbuf: AtomicU32::new(0),
        pending: IrqMutex::new(Vec::new()),
        select: IrqMutex::new(None),
        recorded: AtomicU32::new(0),
        polls: AtomicUsize::new(0),
        send_side: AtomicU32::new(send_side::OPEN),
    })
    .map_err(|_| NtStatus::NO_MEMORY)?;
    REGISTRY.lock().try_push(state.clone()).map_err(|_| NtStatus::NO_MEMORY)?;
    // On failure the endpoint drops and unregisters itself.
    KObject::new(ObjectBody::Socket(Endpoint { state })).map_err(|_| NtStatus::NO_MEMORY)
}

pub fn status_of(e: NetError) -> NtStatus {
    match e {
        NetError::WouldBlock => NtStatus::PENDING,
        NetError::InvalidState => NtStatus::INVALID_PARAMETER,
        NetError::AddressInUse => NtStatus::ADDRESS_ALREADY_EXISTS,
        NetError::AddressNotAvailable => NtStatus::INVALID_ADDRESS_COMPONENT,
        NetError::Refused => NtStatus::CONNECTION_REFUSED,
        NetError::Reset => NtStatus::CONNECTION_RESET,
        NetError::NotConnected => NtStatus::INVALID_CONNECTION,
        NetError::Unreachable => NtStatus::NETWORK_UNREACHABLE,
        NetError::NoMemory => NtStatus::INSUFFICIENT_RESOURCES,
        // WSAENOBUFS in ws2_32.
        NetError::OverQuota => NtStatus::QUOTA_EXCEEDED,
        NetError::NoSocket => NtStatus::INVALID_HANDLE,
        NetError::TooLarge => NtStatus::INVALID_BUFFER_SIZE,
    }
}

/// One device I/O control request.
pub struct Request<'a> {
    pub process: &'a Process,
    pub object: &'a ObjectRef,
    pub code: u32,
    pub input: u64,
    pub input_len: u32,
    pub output: u64,
    pub output_len: u32,
}

impl Request<'_> {
    fn read(&self, addr: u64, buf: &mut [u8]) -> Result<(), NtStatus> {
        self.process.read(addr, buf).map_err(|_| NtStatus::ACCESS_VIOLATION)
    }

    fn write(&self, addr: u64, data: &[u8]) -> Result<(), NtStatus> {
        self.process.write(addr, data).map_err(|_| NtStatus::ACCESS_VIOLATION)
    }

    /// The caller's request layouts (afd_abi).
    fn layout(&self) -> Layout {
        layout_of(self.process)
    }

    fn input<const N: usize>(&self) -> Result<[u8; N], NtStatus> {
        if (self.input_len as usize) < N {
            return Err(NtStatus::INVALID_PARAMETER);
        }
        let mut buf = [0u8; N];
        self.read(self.input, &mut buf)?;
        Ok(buf)
    }

    /// The first `N` bytes of the input buffer, or all of it when shorter
    /// but at least `min` (an address of either family): the bytes and
    /// how many were read.
    fn input_upto<const N: usize>(&self, min: usize) -> Result<([u8; N], usize), NtStatus> {
        let n = (self.input_len as usize).min(N);
        if n < min {
            return Err(NtStatus::INVALID_PARAMETER);
        }
        let mut buf = [0u8; N];
        self.read(self.input, &mut buf[..n])?;
        Ok((buf, n))
    }

    /// Writes `data` to the output buffer; returns IoStatus.Information.
    fn output(&self, data: &[u8]) -> Result<u64, NtStatus> {
        if (self.output_len as usize) < data.len() {
            return Err(NtStatus::BUFFER_TOO_SMALL);
        }
        self.write(self.output, data)?;
        Ok(data.len() as u64)
    }
}

/// SOMAXCONN (winsock2.h): "a maximum reasonable value".
const SOMAXCONN: i32 = 0x7fff_ffff;
/// The backlog SOMAXCONN gives, and the most an ordinary value gets
/// (gala's reasonable maximum; the bottom of SOMAXCONN_HINT's range).
const REASONABLE_BACKLOG: usize = 200;

/// The connections a listen backlog lets wait, as Microsoft Learn's listen
/// describes it: SOMAXCONN is a reasonable maximum (200 here),
/// SOMAXCONN_HINT(N) (that is -N) is N kept within 200 to 65535, other
/// values are limited to a reasonable one (1 to 200). Nothing is set aside
/// for them: each connection is a socket of its own when it comes
/// (ARCHITECTURE "TCP: net::tcp").
fn backlog_of(asked: i32) -> usize {
    match asked {
        SOMAXCONN => REASONABLE_BACKLOG,
        n if n < 0 => (n.unsigned_abs() as usize).clamp(REASONABLE_BACKLOG, tcp::MAX_BACKLOG),
        n => (n as usize).clamp(1, REASONABLE_BACKLOG),
    }
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn u64_at(b: &[u8], at: usize) -> u64 {
    let mut v = [0u8; 8];
    v.copy_from_slice(&b[at..at + 8]);
    u64::from_le_bytes(v)
}

/// SOCKADDR_IN (16 bytes) or SOCKADDR_IN6 (28: port, flow info, the
/// address, scope id) -> address.
fn sockaddr(b: &[u8]) -> Result<Addr, NtStatus> {
    let bad = NtStatus::INVALID_ADDRESS_COMPONENT;
    match u16::from_le_bytes([*b.first().ok_or(bad)?, *b.get(1).ok_or(bad)?]) {
        AF_INET if b.len() >= SOCKADDR_LEN => Ok(Addr {
            port: u16::from_be_bytes([b[2], b[3]]),
            ip: Ip::V4([b[4], b[5], b[6], b[7]]),
        }),
        AF_INET6 if b.len() >= SOCKADDR6_LEN => {
            let mut ip = [0u8; 16];
            ip.copy_from_slice(&b[8..24]);
            // IPv4-mapped: the IPv4 address (a dual-stack socket's).
            Ok(Addr { port: u16::from_be_bytes([b[2], b[3]]), ip: Ip::V6(ip).unmapped() })
        }
        _ => Err(bad),
    }
}

/// Address -> SOCKADDR_IN or SOCKADDR_IN6: the bytes and their length.
fn to_sockaddr(a: Addr, v6: bool) -> ([u8; SOCKADDR6_LEN], usize) {
    let a = if v6 { Addr { ip: a.ip.mapped(), ..a } } else { a };
    let mut b = [0u8; SOCKADDR6_LEN];
    b[2..4].copy_from_slice(&a.port.to_be_bytes());
    match a.ip {
        Ip::V4(ip) => {
            b[..2].copy_from_slice(&AF_INET.to_le_bytes());
            b[4..8].copy_from_slice(&ip);
            (b, SOCKADDR_LEN)
        }
        Ip::V6(ip) => {
            b[..2].copy_from_slice(&AF_INET6.to_le_bytes());
            b[8..24].copy_from_slice(&ip);
            (b, SOCKADDR6_LEN)
        }
    }
}

/// TRANSPORT_ADDRESS (count, length, type, then TDI_ADDRESS_IP or
/// TDI_ADDRESS_IP6: port, flow info, the address, scope id) -> address.
fn transport_address(b: &[u8]) -> Result<Addr, NtStatus> {
    let bad = NtStatus::INVALID_ADDRESS_COMPONENT;
    if b.len() < TRANSPORT_ADDRESS_LEN || u32_at(b, 0) < 1 {
        return Err(bad);
    }
    let port = u16::from_be_bytes([b[8], b[9]]);
    match u16::from_le_bytes([b[6], b[7]]) {
        AF_INET => Ok(Addr { port, ip: Ip::V4([b[10], b[11], b[12], b[13]]) }),
        AF_INET6 if b.len() >= TRANSPORT_ADDRESS6_LEN => {
            let mut ip = [0u8; 16];
            ip.copy_from_slice(&b[14..30]);
            Ok(Addr { port, ip: Ip::V6(ip).unmapped() })
        }
        _ => Err(bad),
    }
}

/// Address -> TRANSPORT_ADDRESS: the bytes and their length.
fn to_transport_address(a: Addr, v6: bool) -> ([u8; TRANSPORT_ADDRESS6_LEN], usize) {
    let a = if v6 { Addr { ip: a.ip.mapped(), ..a } } else { a };
    let mut b = [0u8; TRANSPORT_ADDRESS6_LEN];
    b[..4].copy_from_slice(&1u32.to_le_bytes());
    b[8..10].copy_from_slice(&a.port.to_be_bytes());
    match a.ip {
        Ip::V4(ip) => {
            b[4..6].copy_from_slice(&14u16.to_le_bytes());
            b[6..8].copy_from_slice(&AF_INET.to_le_bytes());
            b[10..14].copy_from_slice(&ip);
            (b, TRANSPORT_ADDRESS_LEN)
        }
        Ip::V6(ip) => {
            b[4..6].copy_from_slice(&26u16.to_le_bytes());
            b[6..8].copy_from_slice(&AF_INET6.to_le_bytes());
            b[14..30].copy_from_slice(&ip);
            (b, TRANSPORT_ADDRESS6_LEN)
        }
    }
}

/// Reads the WSABUF array of a RECV/SEND request: (address, length) pairs.
fn buffers(r: &Request<'_>) -> Result<Vec<(u64, usize)>, NtStatus> {
    let l = r.layout();
    let (info, _): ([u8; 24], usize) = r.input_upto(l.info_size())?;
    let (array, count) = (l.get_ptr(&info, 0), u32_at(&info, l.info_count()) as usize);
    if count > 1024 {
        return Err(NtStatus::INVALID_PARAMETER);
    }
    let mut out = Vec::new();
    out.try_reserve(count).map_err(|_| NtStatus::NO_MEMORY)?;
    for i in 0..count {
        // WSABUF: {u32 len; char *buf}.
        let mut wsabuf = [0u8; 16];
        let size = l.wsabuf_size();
        r.read(array + (size * i) as u64, &mut wsabuf[..size])?;
        out.try_push((l.get_ptr(&wsabuf, l.wsabuf_buf()), u32_at(&wsabuf, 0) as usize))
            .map_err(|_| NtStatus::NO_MEMORY)?;
    }
    Ok(out)
}

fn bounce() -> Result<Vec<u8>, NtStatus> {
    let mut buf = Vec::new();
    buf.try_reserve_exact(CHUNK).map_err(|_| NtStatus::NO_MEMORY)?;
    buf.resize(CHUNK, 0);
    Ok(buf)
}

/// The endpoint behind a handle (AcceptEx's accepting socket).
fn endpoint_of(process: &Process, handle: u64) -> Result<KArc<EndpointState>, NtStatus> {
    let entry = process.handles.lock().get(Handle(handle)).ok_or(NtStatus::INVALID_HANDLE)?;
    match &entry.object.body {
        ObjectBody::Socket(e) => Ok(e.state.clone()),
        _ => Err(NtStatus::OBJECT_TYPE_MISMATCH),
    }
}

/// Handles an IOCTL on an endpoint and reports its completion (now or
/// later) through `io`; returns the status NtDeviceIoControlFile returns.
pub fn ioctl(r: &Request<'_>, io: Io) -> NtStatus {
    let ObjectBody::Socket(endpoint) = &r.object.body else {
        return NtStatus::INVALID_DEVICE_REQUEST;
    };
    let ep = &*endpoint.state;
    if ep.transport == Transport::None && r.code != IOCTL_AFD_POLL {
        super::ledger::record("NtDeviceIoControlFile", "afd helper ioctl", u64::from(r.code));
        return NtStatus::INVALID_DEVICE_REQUEST;
    }
    let op = match immediate(r, ep) {
        Ok(Immediate::Done(information)) => {
            finish(ep, &io, NtStatus::SUCCESS, information, true);
            return NtStatus::SUCCESS;
        }
        Ok(Immediate::Async(op)) => op,
        Err(status) => {
            finish(ep, &io, status, 0, true);
            return status;
        }
    };
    submit(ep, op, io)
}

enum Immediate {
    Done(u64),
    Async(Op),
}

/// Validates a request; does what needs no waiting, or returns the
/// operation to submit.
fn immediate(r: &Request<'_>, ep: &EndpointState) -> Result<Immediate, NtStatus> {
    if ep.transport.is_udp()
        && let Some(i) = immediate_udp(r, ep)?
    {
        return Ok(i);
    }
    let current = || *ep.socket.lock();
    let socket = || current().ok_or(NtStatus::INVALID_CONNECTION);
    Ok(match r.code {
        IOCTL_AFD_BIND => {
            let (b, n): ([u8; 4 + SOCKADDR6_LEN], usize) = r.input_upto(4 + SOCKADDR_LEN)?;
            let local = sockaddr(&b[4..n])?;
            let mut slot = ep.socket.lock();
            if slot.is_some() {
                return Err(NtStatus::INVALID_PARAMETER);
            }
            let id = tcp::bind_scoped(local, ep.scope(), ep.owner()).map_err(status_of)?;
            *slot = Some(id);
            drop(slot);
            let bound = tcp::local(id).map_err(status_of)?;
            Immediate::Done(if r.output_len as usize >= SOCKADDR_LEN {
                r.output(
                    &to_sockaddr(bound, ep.transport.is_v6()).0
                        [..to_sockaddr(bound, ep.transport.is_v6()).1],
                )?
            } else {
                0
            })
        }
        IOCTL_AFD_LISTEN => {
            let b: [u8; 12] = r.input()?;
            tcp::listen(socket()?, backlog_of(u32_at(&b, 4) as i32), tcp::WhenFull::Refuse)
                .map_err(status_of)?;
            Immediate::Done(0)
        }
        IOCTL_AFD_CONNECT | IOCTL_AFD_SUPER_CONNECT => {
            let remote = if r.code == IOCTL_AFD_CONNECT {
                let (b, n): ([u8; 12 + TRANSPORT_ADDRESS6_LEN], usize) =
                    r.input_upto(12 + TRANSPORT_ADDRESS_LEN)?;
                transport_address(&b[12..n])?
            } else {
                let (b, n): ([u8; 4 + TRANSPORT_ADDRESS6_LEN], usize) =
                    r.input_upto(4 + TRANSPORT_ADDRESS_LEN)?;
                transport_address(&b[4..n])?
            };
            let id = tcp::connect(current(), remote, ep.owner()).map_err(status_of)?;
            *ep.socket.lock() = Some(id);
            Immediate::Async(Op::Connect)
        }
        IOCTL_AFD_WAIT_FOR_LISTEN => {
            socket()?;
            Immediate::Async(Op::WaitForListen { output: r.output, output_len: r.output_len })
        }
        IOCTL_AFD_ACCEPT => {
            let l = r.layout();
            let (b, _): ([u8; 16], usize) = r.input_upto(l.accept_size())?;
            let sequence = u32_at(&b, Layout::ACCEPT_SEQUENCE);
            let handle = l.get_handle(&b, Layout::ACCEPT_HANDLE);
            let accepting = endpoint_of(r.process, handle)?;
            let mut slot = accepting.socket.lock();
            if slot.is_some() {
                return Err(NtStatus::INVALID_PARAMETER);
            }
            *slot = Some(tcp::accept(socket()?, sequence, accepting.owner()).map_err(status_of)?);
            // The listener's non-blocking mode carries over (as on Windows).
            accepting
                .non_blocking
                .store(ep.non_blocking.load(Ordering::Acquire), Ordering::Release);
            Immediate::Done(0)
        }
        IOCTL_AFD_SUPER_ACCEPT => {
            // {u8 san; u8 fix_alignment; HANDLE accept; u32 receive_length;
            //  u32 local_length; u32 remote_length}
            let l = r.layout();
            let (b, _): ([u8; 28], usize) = r.input_upto(l.super_accept_size())?;
            let handle = l.get_handle(&b, l.super_accept_handle());
            let receive = u32_at(&b, l.super_accept_receive());
            let (local, remote) =
                (u32_at(&b, l.super_accept_local()), u32_at(&b, l.super_accept_remote()));
            if receive != 0 {
                // AcceptEx that also waits for the first data.
                super::ledger::record(
                    "NtDeviceIoControlFile",
                    "AcceptEx receive length",
                    u64::from(receive),
                );
                return Err(NtStatus::NOT_SUPPORTED);
            }
            let room = u64::from(receive) + u64::from(local) + u64::from(remote);
            if (local as usize) < SOCKADDR_LEN
                || (remote as usize) < SOCKADDR_LEN
                || u64::from(r.output_len) < room
            {
                return Err(NtStatus::BUFFER_TOO_SMALL);
            }
            socket()?;
            let accept = endpoint_of(r.process, handle)?;
            Immediate::Async(Op::SuperAccept { accept, output: r.output, receive, local })
        }
        IOCTL_AFD_POLL => {
            // AFD_POLL_INFO: the header, then {HANDLE; events; status}
            // entries of the caller's width (afd_abi).
            let l = r.layout();
            let header: [u8; Layout::POLL_HEADER] = r.input()?;
            let timeout = u64_at(&header, 0) as i64;
            let count = u32_at(&header, 8) as usize;
            let entry_size = l.poll_entry();
            if count > 1024 || (r.input_len as usize) < Layout::POLL_HEADER + entry_size * count {
                return Err(NtStatus::INVALID_PARAMETER);
            }
            let mut sockets = Vec::new();
            sockets.try_reserve(count).map_err(|_| NtStatus::NO_MEMORY)?;
            for i in 0..count {
                let mut entry = [0u8; 16];
                r.read(
                    r.input + (Layout::POLL_HEADER + entry_size * i) as u64,
                    &mut entry[..entry_size],
                )?;
                let handle = l.get_handle(&entry, 0);
                let state = endpoint_of(r.process, handle)?;
                let _ = sockets.try_push((handle, state, u32_at(&entry, l.poll_events())));
            }
            // The timeout is left to the caller (a wait with a timeout,
            // then NtCancelIoFileEx); only 0 (just look) is honoured here.
            Immediate::Async(Op::Poll {
                sockets,
                output: r.output,
                output_len: r.output_len,
                wait: timeout != 0,
            })
        }
        IOCTL_AFD_SET_INFO => {
            // AFD_INFO {u32 class; (pad); union at 8; u32 padding}.
            let b: [u8; 16] = r.input()?;
            match u32_at(&b, 0) {
                AFD_INFO_BLOCKING_MODE => ep.non_blocking.store(b[8] != 0, Ordering::Release),
                GALA_AFD_INFO_V6ONLY => ep.v6only.store(b[8] != 0, Ordering::Release),
                class @ (AFD_INFO_RECEIVE_WINDOW_SIZE | AFD_INFO_SEND_WINDOW_SIZE) => {
                    let size = u32_at(&b, 8);
                    let slot =
                        if class == AFD_INFO_RECEIVE_WINDOW_SIZE { &ep.rcvbuf } else { &ep.sndbuf };
                    slot.store(size, Ordering::Release);
                    // Bound or listening: its next sockets get them. A
                    // connection's (or a bound datagram socket's) buffers
                    // stay as they are: they are made whole at the start.
                    if let Some(id) = current()
                        && tcp::set_owner(id, ep.owner()).is_err()
                    {
                        super::ledger::record(
                            "NtDeviceIoControlFile",
                            "afd buffer size of a connected socket",
                            u64::from(size),
                        );
                    }
                }
                class => {
                    super::ledger::record(
                        "NtDeviceIoControlFile",
                        "afd set info",
                        u64::from(class),
                    );
                    return Err(NtStatus::INVALID_PARAMETER);
                }
            }
            Immediate::Done(0)
        }
        IOCTL_AFD_SEND => {
            socket()?;
            if ep.send_side.load(Ordering::Acquire) != send_side::OPEN {
                // After shutdown(SD_SEND): WSAESHUTDOWN (Wine's mapping).
                return Err(NtStatus::PIPE_DISCONNECTED);
            }
            Immediate::Async(Op::Send { buffers: buffers(r)?, done: 0 })
        }
        IOCTL_AFD_RECV => {
            socket()?;
            // afd_recv_params.msg_flags (Wine): AFD_MSG_PEEK 0x80; OOB
            // (0x40) and WAITALL (0x4000) are not supported yet.
            let l = r.layout();
            let (info, _): ([u8; 24], usize) = r.input_upto(l.info_size())?;
            let msg_flags = u32_at(&info, l.info_tdi_flags());
            if msg_flags & (AFD_MSG_OOB | AFD_MSG_WAITALL) != 0 {
                super::ledger::record(
                    "NtDeviceIoControlFile",
                    "afd recv flags",
                    u64::from(msg_flags),
                );
                return Err(NtStatus::NOT_SUPPORTED);
            }
            Immediate::Async(Op::Recv { buffers: buffers(r)?, peek: msg_flags & AFD_MSG_PEEK != 0 })
        }
        IOCTL_AFD_DISCONNECT => {
            let b: [u8; 16] = r.input()?;
            let kind = u32_at(&b, 0);
            if kind & AFD_DISCONNECT_ABORT != 0 {
                // Reset: no FIN is owed any more.
                let _pending = ep.pending.lock();
                ep.send_side.store(send_side::SHUT, Ordering::Release);
                if let Some(id) = ep.socket.lock().take() {
                    tcp::close(id, true);
                }
            } else if kind & AFD_DISCONNECT_SEND != 0 {
                let id = socket()?;
                // The FIN follows the bytes of every send issued before the
                // shutdown, also of overlapped ones still pending
                // (Win32-OpenSSH shuts a channel's socket down while its
                // last WSASend waits for room); shutdown itself does not
                // wait. Sending the FIN now would lose the rest of those
                // sends (`sshd_forward`, winsock_async_test).
                let pending = ep.pending.lock();
                if ep.send_side.load(Ordering::Acquire) == send_side::OPEN {
                    let sends = pending.iter().filter(|p| p.op.kind() == SEND_KIND);
                    let (count, left) = sends.fold((0usize, 0usize), |(n, left), p| match &p.op {
                        Op::Send { buffers, done } => {
                            (n + 1, left + buffers.iter().map(|&(_, l)| l).sum::<usize>() - done)
                        }
                        _ => (n, left),
                    });
                    if count == 0 {
                        tcp::shutdown_send(id).map_err(status_of)?;
                        ep.send_side.store(send_side::SHUT, Ordering::Release);
                    } else {
                        ep.send_side.store(send_side::AFTER_SENDS, Ordering::Release);
                        afd_event(ep, "afd shutdown after sends", |f| {
                            write!(f, r#","sends":{count},"bytes":{left}"#)
                        });
                    }
                }
            }
            Immediate::Done(0)
        }
        IOCTL_AFD_GETSOCKNAME => {
            let local = tcp::local(socket().map_err(|_| NtStatus::INVALID_PARAMETER)?)
                .map_err(status_of)?;
            let (b, n) = to_sockaddr(local, ep.transport.is_v6());
            Immediate::Done(r.output(&b[..n])?)
        }
        IOCTL_AFD_GET_PEER_NAME => {
            let remote = tcp::remote(socket()?).map_err(status_of)?;
            let (b, n) = to_sockaddr(remote, ep.transport.is_v6());
            Immediate::Done(r.output(&b[..n])?)
        }
        code => {
            super::ledger::record("NtDeviceIoControlFile", "afd ioctl", u64::from(code));
            return Err(NtStatus::INVALID_DEVICE_REQUEST);
        }
    })
}

// AFD_RECV_INFO_UDP and AFD_SEND_INFO_UDP (ReactOS `drivers/afd/
// shared.h`, to be verified): their layouts for the caller's width are
// afd_abi's (the destination is TDI_CONNECTION_INFORMATION's
// RemoteAddressLength and RemoteAddress).

/// The requests a datagram endpoint answers differently from a stream
/// one (None: the shared handling applies).
fn immediate_udp(r: &Request<'_>, ep: &EndpointState) -> Result<Option<Immediate>, NtStatus> {
    // Sending from an unbound socket binds it (0.0.0.0, an ephemeral
    // port), as Windows does.
    let bound_or_bind = || -> Result<SocketId, NtStatus> {
        let mut slot = ep.socket.lock();
        if let Some(id) = *slot {
            return Ok(id);
        }
        let id = udp::bind_scoped(ep.transport.any_address(), ep.scope(), ep.owner())
            .map_err(status_of)?;
        *slot = Some(id);
        Ok(id)
    };
    Ok(Some(match r.code {
        IOCTL_AFD_BIND => {
            let (b, n): ([u8; 4 + SOCKADDR6_LEN], usize) = r.input_upto(4 + SOCKADDR_LEN)?;
            let local = sockaddr(&b[4..n])?;
            let mut slot = ep.socket.lock();
            if slot.is_some() {
                return Err(NtStatus::INVALID_PARAMETER);
            }
            let id = udp::bind_scoped(local, ep.scope(), ep.owner()).map_err(status_of)?;
            *slot = Some(id);
            drop(slot);
            let bound = tcp::local(id).map_err(status_of)?;
            Immediate::Done(if r.output_len as usize >= SOCKADDR_LEN {
                r.output(
                    &to_sockaddr(bound, ep.transport.is_v6()).0
                        [..to_sockaddr(bound, ep.transport.is_v6()).1],
                )?
            } else {
                0
            })
        }
        // connect on a datagram socket only sets (or, with the zero
        // address, clears) the default peer.
        IOCTL_AFD_CONNECT => {
            let (b, n): ([u8; 12 + TRANSPORT_ADDRESS6_LEN], usize) =
                r.input_upto(12 + TRANSPORT_ADDRESS_LEN)?;
            let remote = transport_address(&b[12..n])?;
            if remote.ip.is_unspecified() {
                let current = *ep.socket.lock();
                if let Some(id) = current {
                    udp::connect(id, None).map_err(status_of)?;
                }
            } else {
                udp::connect(bound_or_bind()?, Some(remote)).map_err(status_of)?;
            }
            Immediate::Done(0)
        }
        IOCTL_AFD_SEND_DATAGRAM => {
            let l = r.layout();
            let (info, _): ([u8; 104], usize) = r.input_upto(l.send_dgram_size())?;
            let len = u32_at(&info, l.send_dgram_address_len()) as usize;
            if len < TRANSPORT_ADDRESS_LEN {
                return Err(NtStatus::INVALID_PARAMETER);
            }
            let mut ta = [0u8; TRANSPORT_ADDRESS6_LEN];
            let n = len.min(TRANSPORT_ADDRESS6_LEN);
            r.read(l.get_ptr(&info, l.send_dgram_address()), &mut ta[..n])?;
            let to = transport_address(&ta[..n])?;
            let buffers = buffers(r)?;
            bound_or_bind()?;
            Immediate::Async(Op::SendTo { buffers, to: Some(to) })
        }
        IOCTL_AFD_SEND => {
            let id = (*ep.socket.lock()).ok_or(NtStatus::INVALID_CONNECTION)?;
            tcp::remote(id).map_err(status_of)?;
            Immediate::Async(Op::SendTo { buffers: buffers(r)?, to: None })
        }
        IOCTL_AFD_RECV | IOCTL_AFD_RECV_DATAGRAM => {
            // Receiving needs a bound socket (WSAEINVAL otherwise).
            (*ep.socket.lock()).ok_or(NtStatus::INVALID_PARAMETER)?;
            let l = r.layout();
            let (info, _): ([u8; 24], usize) = r.input_upto(l.info_size())?;
            let msg_flags = u32_at(&info, l.info_tdi_flags());
            if msg_flags & (AFD_MSG_OOB | AFD_MSG_WAITALL) != 0 {
                super::ledger::record(
                    "NtDeviceIoControlFile",
                    "afd datagram recv flags",
                    u64::from(msg_flags),
                );
                return Err(NtStatus::NOT_SUPPORTED);
            }
            let (address, address_len) = if r.code == IOCTL_AFD_RECV_DATAGRAM {
                let (b, _): ([u8; 40], usize) = r.input_upto(l.recv_dgram_size())?;
                (l.get_ptr(&b, l.recv_dgram_address()), l.get_ptr(&b, l.recv_dgram_address_len()))
            } else {
                (0, 0)
            };
            Immediate::Async(Op::RecvFrom {
                buffers: buffers(r)?,
                peek: msg_flags & AFD_MSG_PEEK != 0,
                address,
                address_len,
            })
        }
        // Connection requests have no meaning for datagrams.
        IOCTL_AFD_LISTEN
        | IOCTL_AFD_WAIT_FOR_LISTEN
        | IOCTL_AFD_ACCEPT
        | IOCTL_AFD_SUPER_ACCEPT
        | IOCTL_AFD_SUPER_CONNECT => return Err(NtStatus::NOT_SUPPORTED),
        IOCTL_AFD_DISCONNECT => Immediate::Done(0),
        _ => return Ok(None),
    }))
}

// Kernel handover (ROADMAP R9 (e), ARCHITECTURE "Sockets across a
// handover"): an endpoint and its pending requests as data, with the
// objects they name as references (`handover::carry` names them by id),
// and the endpoint rebuilt from that in the next kernel. The kernel
// socket behind it is `net::tcp`'s (`carry_entry` / `adopt_entry`).

/// A pending request as a handover carries it. `kind` is
/// `handover::sockets::request_kind`'s number.
pub struct CarriedRequest {
    pub kind: u32,
    pub process: KArc<Process>,
    pub iosb: u64,
    pub event: Option<ObjectRef>,
    pub context: u64,
    pub apc: Option<(ObjectRef, super::thread::UserApc)>,
    /// RECV, RECV_FROM: peek; POLL: it waits.
    pub flag: bool,
    /// SEND: bytes already queued.
    pub done: u64,
    pub output: u64,
    pub output_len: u32,
    pub receive: u32,
    pub local: u32,
    pub buffers: Vec<(u64, u64)>,
    /// RECV_FROM: where the sender's address and its length go.
    pub address: u64,
    pub address_len: u64,
    /// SEND_TO: the destination (None: the default peer).
    pub to: Option<Addr>,
    /// SUPER_ACCEPT: the accepting endpoint (handle 0); POLL: the sockets.
    pub endpoints: Vec<(u64, KArc<EndpointState>, u32)>,
}

/// An endpoint as a handover carries it (the kernel socket apart).
pub struct CarriedEndpoint {
    pub transport: Transport,
    pub socket: Option<SocketId>,
    pub completion: Option<(ObjectRef, u64)>,
    pub modes: u32,
    pub non_blocking: bool,
    pub select: Option<(ObjectRef, u32)>,
    pub recorded: u32,
    pub signal: i32,
    /// `send_side` (`send_side::*`).
    pub send_side: u32,
    pub requests: Vec<CarriedRequest>,
}

use crate::handover::sockets::request_kind as rk;

/// The objects an endpoint names: its completion port, its selected
/// event, its requests' events and completion routines' threads.
pub fn referenced(state: &EndpointState) -> Result<Vec<ObjectRef>, NtStatus> {
    let mut out = Vec::new();
    let no_memory = |_| NtStatus::NO_MEMORY;
    if let Some((port, _)) = state.completion.lock().clone() {
        out.try_push(port).map_err(no_memory)?;
    }
    if let Some((event, _)) = state.select.lock().clone() {
        out.try_push(event).map_err(no_memory)?;
    }
    for p in state.pending.lock().iter() {
        if let Some(e) = &p.io.event {
            out.try_push(e.clone()).map_err(no_memory)?;
        }
        if let Some((thread, _)) = &p.io.apc {
            out.try_push(thread.clone()).map_err(no_memory)?;
        }
    }
    Ok(out)
}

/// Endpoint `state` as data (the network stopped, the programs stopped).
pub fn carry(state: &EndpointState) -> Result<CarriedEndpoint, NtStatus> {
    let no_memory = |_| NtStatus::NO_MEMORY;
    let mut requests = Vec::new();
    for p in state.pending.lock().iter() {
        let (kind, mut buffers, mut endpoints) = (0, Vec::new(), Vec::new());
        let mut q = CarriedRequest {
            kind,
            process: p.io.process.clone(),
            iosb: p.io.iosb,
            event: p.io.event.clone(),
            context: p.io.context,
            apc: p.io.apc.clone(),
            flag: false,
            done: 0,
            output: 0,
            output_len: 0,
            receive: 0,
            local: 0,
            buffers: Vec::new(),
            endpoints: Vec::new(),
            address: 0,
            address_len: 0,
            to: None,
        };
        match &p.op {
            Op::Recv { buffers: b, peek } => {
                q.kind = rk::RECV;
                q.flag = *peek;
                for &(addr, len) in b {
                    buffers.try_push((addr, len as u64)).map_err(no_memory)?;
                }
            }
            Op::Send { buffers: b, done } => {
                q.kind = rk::SEND;
                q.done = *done as u64;
                for &(addr, len) in b {
                    buffers.try_push((addr, len as u64)).map_err(no_memory)?;
                }
            }
            Op::RecvFrom { buffers: b, peek, address, address_len } => {
                q.kind = rk::RECV_FROM;
                q.flag = *peek;
                (q.address, q.address_len) = (*address, *address_len);
                for &(addr, len) in b {
                    buffers.try_push((addr, len as u64)).map_err(no_memory)?;
                }
            }
            Op::SendTo { buffers: b, to } => {
                q.kind = rk::SEND_TO;
                q.to = *to;
                for &(addr, len) in b {
                    buffers.try_push((addr, len as u64)).map_err(no_memory)?;
                }
            }
            Op::WaitForListen { output, output_len } => {
                q.kind = rk::WAIT_FOR_LISTEN;
                (q.output, q.output_len) = (*output, *output_len);
            }
            Op::SuperAccept { accept, output, receive, local } => {
                q.kind = rk::SUPER_ACCEPT;
                (q.output, q.receive, q.local) = (*output, *receive, *local);
                endpoints.try_push((0, accept.clone(), 0)).map_err(no_memory)?;
            }
            Op::Connect => q.kind = rk::CONNECT,
            Op::Poll { sockets, output, output_len, wait } => {
                q.kind = rk::POLL;
                (q.output, q.output_len, q.flag) = (*output, *output_len, *wait);
                for (handle, s, events) in sockets {
                    endpoints.try_push((*handle, s.clone(), *events)).map_err(no_memory)?;
                }
            }
        }
        (q.buffers, q.endpoints) = (buffers, endpoints);
        requests.try_push(q).map_err(no_memory)?;
    }
    Ok(CarriedEndpoint {
        transport: state.transport,
        socket: *state.socket.lock(),
        completion: state.completion.lock().clone(),
        modes: state.modes.load(Ordering::Acquire),
        non_blocking: state.non_blocking.load(Ordering::Acquire),
        select: state.select.lock().clone(),
        recorded: state.recorded.load(Ordering::Acquire),
        signal: state.dispatcher.signal_state(),
        send_side: state.send_side.load(Ordering::Acquire),
        requests,
    })
}

/// In the next kernel: an endpoint again, with its kernel socket rebuilt
/// from `entry` (requests come after every endpoint is there:
/// `adopt_requests`). `c.socket` and `c.requests` are not used.
pub fn adopt(
    c: &CarriedEndpoint,
    entry: Option<&tcp::CarriedEntry>,
) -> Result<ObjectRef, NtStatus> {
    let token = NEXT_TOKEN.fetch_add(1, Ordering::Relaxed);
    let state = KArc::try_new(EndpointState {
        dispatcher: Dispatcher::new(c.signal),
        transport: c.transport,
        token,
        socket: IrqMutex::new(None),
        completion: IrqMutex::new(c.completion.clone()),
        modes: AtomicU32::new(c.modes),
        non_blocking: AtomicBool::new(c.non_blocking),
        // Carried endpoints are IPv4 (a handover refuses IPv6 ones).
        v6only: AtomicBool::new(true),
        purse: None,
        rcvbuf: AtomicU32::new(0),
        sndbuf: AtomicU32::new(0),
        pending: IrqMutex::new(Vec::new()),
        select: IrqMutex::new(c.select.clone()),
        recorded: AtomicU32::new(c.recorded),
        polls: AtomicUsize::new(0),
        send_side: AtomicU32::new(c.send_side),
    })
    .map_err(|_| NtStatus::NO_MEMORY)?;
    REGISTRY.lock().try_push(state.clone()).map_err(|_| NtStatus::NO_MEMORY)?;
    // On failure the endpoint drops and unregisters itself.
    let object = KObject::new(ObjectBody::Socket(Endpoint { state: state.clone() }))
        .map_err(|_| NtStatus::NO_MEMORY)?;
    if let Some(entry) = entry {
        *state.socket.lock() = Some(tcp::adopt_entry(entry, token).map_err(status_of)?);
    }
    Ok(object)
}

/// In the next kernel, once every endpoint is there: the pending requests
/// of endpoint `object` again; the ones that can finish now do, the rest
/// when their socket changes (as before the handover).
pub fn adopt_requests(object: &ObjectRef, requests: Vec<CarriedRequest>) -> Result<(), NtStatus> {
    let ObjectBody::Socket(endpoint) = &object.body else {
        return Err(NtStatus::OBJECT_TYPE_MISMATCH);
    };
    let state = &endpoint.state;
    let no_memory = |_| NtStatus::NO_MEMORY;
    let mut pending = state.pending.lock();
    for q in requests {
        let mut buffers = Vec::new();
        for &(addr, len) in &q.buffers {
            buffers.try_push((addr, len as usize)).map_err(no_memory)?;
        }
        let mut endpoints = q.endpoints.into_iter();
        let op = match q.kind {
            rk::RECV => Op::Recv { buffers, peek: q.flag },
            rk::SEND => Op::Send { buffers, done: q.done as usize },
            rk::RECV_FROM => Op::RecvFrom {
                buffers,
                peek: q.flag,
                address: q.address,
                address_len: q.address_len,
            },
            rk::SEND_TO => Op::SendTo { buffers, to: q.to },
            rk::WAIT_FOR_LISTEN => Op::WaitForListen { output: q.output, output_len: q.output_len },
            rk::SUPER_ACCEPT => Op::SuperAccept {
                accept: endpoints.next().ok_or(NtStatus::INVALID_PARAMETER)?.1,
                output: q.output,
                receive: q.receive,
                local: q.local,
            },
            rk::CONNECT => Op::Connect,
            rk::POLL => {
                let mut sockets = Vec::new();
                for s in endpoints {
                    sockets.try_push(s).map_err(no_memory)?;
                }
                Op::Poll { sockets, output: q.output, output_len: q.output_len, wait: q.flag }
            }
            _ => return Err(NtStatus::INVALID_PARAMETER),
        };
        let mut io = Io {
            process: q.process,
            iosb: q.iosb,
            event: q.event,
            context: q.context,
            apc: q.apc,
            reserved: super::notify::Reserved::none(),
        };
        io.reserved = reserve(state, &io)?;
        if usize::from(op.kind()) == POLL_KIND {
            state.polls.fetch_add(1, Ordering::AcqRel);
            let mut pollers = POLLERS.lock();
            if !pollers.iter().any(|e| e.token == state.token) {
                pollers.try_push(state.clone()).map_err(no_memory)?;
            }
        }
        pending.try_push(Pending { op, io }).map_err(no_memory)?;
    }
    drop(pending);
    // What can finish already does (the net thread may have reported this
    // socket before its requests were back).
    try_complete(state);
    check_select(state);
    tcp::kick();
    Ok(())
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    /// listen's backlog as Microsoft Learn describes it.
    #[test]
    fn backlogs() {
        assert_eq!(backlog_of(SOMAXCONN), 200);
        assert_eq!(backlog_of(0), 1);
        assert_eq!(backlog_of(5), 5);
        assert_eq!(backlog_of(511), 200);
        // SOMAXCONN_HINT(N) is -N, kept within 200 to 65535.
        assert_eq!(backlog_of(-10), 200);
        assert_eq!(backlog_of(-1000), 1000);
        assert_eq!(backlog_of(-100_000), 65535);
        assert_eq!(backlog_of(i32::MIN), 65535);
    }

    #[test]
    fn open_packet_transport() {
        let mut ea = alloc::vec::Vec::new();
        let name = b"AfdOpenPacketXX";
        let transport: alloc::vec::Vec<u8> =
            "\\Device\\Tcp".encode_utf16().flat_map(u16::to_le_bytes).collect();
        let mut value = alloc::vec::Vec::new();
        value.extend_from_slice(&0u32.to_le_bytes());
        value.extend_from_slice(&0u32.to_le_bytes());
        value.extend_from_slice(&(transport.len() as u32).to_le_bytes());
        value.extend_from_slice(&transport);
        ea.extend_from_slice(&0u32.to_le_bytes());
        ea.push(0);
        ea.push(name.len() as u8);
        ea.extend_from_slice(&(value.len() as u16).to_le_bytes());
        ea.extend_from_slice(name);
        ea.push(0);
        ea.extend_from_slice(&value);
        assert_eq!(parse_open_packet(&ea), Ok(Transport::Tcp));
        for t in [Transport::None, Transport::Tcp, Transport::Udp, Transport::Tcp6, Transport::Udp6]
        {
            assert_eq!(Transport::from_number(t.number()), Some(t));
        }
        assert_eq!(parse_open_packet(&ea[..10]), Err(NtStatus::INVALID_PARAMETER));
    }

    #[test]
    fn address_layouts() {
        let a = Addr::v4([127, 0, 0, 1], 0x1234);
        let (b, n) = to_sockaddr(a, false);
        assert_eq!(n, SOCKADDR_LEN);
        assert_eq!(sockaddr(&b[..n]), Ok(a));
        let (t, m) = to_transport_address(a, false);
        assert_eq!(m, TRANSPORT_ADDRESS_LEN);
        assert_eq!(transport_address(&t[..m]), Ok(a));
        assert_eq!(&b[..4], &[2, 0, 0x12, 0x34]);
        // IPv6: SOCKADDR_IN6 and TDI_ADDRESS_IP6.
        let a6 = Addr { ip: Ip::V6(Ip::LOOPBACK6), port: 0x1234 };
        let (b, n) = to_sockaddr(a6, true);
        assert_eq!(n, SOCKADDR6_LEN);
        assert_eq!(&b[..4], &[23, 0, 0x12, 0x34]);
        assert_eq!(b[23], 1);
        assert_eq!(sockaddr(&b[..n]), Ok(a6));
        let (t, m) = to_transport_address(a6, true);
        assert_eq!(m, TRANSPORT_ADDRESS6_LEN);
        assert_eq!(transport_address(&t[..m]), Ok(a6));
        // A SOCKADDR_IN6 cut short is refused.
        assert!(sockaddr(&b[..SOCKADDR_LEN]).is_err());
        // Dual stack: an IPv4 address goes out of an IPv6 endpoint
        // IPv4-mapped and comes back in as IPv4.
        let (b, n) = to_sockaddr(a, true);
        assert_eq!(n, SOCKADDR6_LEN);
        assert_eq!(&b[18..24], &[0xff, 0xff, 127, 0, 0, 1]);
        assert_eq!(sockaddr(&b[..n]), Ok(a));
        let (t, m) = to_transport_address(a, true);
        assert_eq!(transport_address(&t[..m]), Ok(a));
    }
}

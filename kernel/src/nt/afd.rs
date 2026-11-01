//! AFD: Windows sockets as file objects of `\Device\Afd`, driven by
//! NtDeviceIoControlFile, on the kernel's TCP API (`net::tcp`). Design,
//! IOCTL table and layout sources: docs/ARCHITECTURE.md, "AFD". Layouts
//! come from Wine's `include/wine/afd.h` (checked by Wine's tests against
//! Windows) and ReactOS's `drivers/afd/shared.h` (marked "to be verified"
//! there); see THIRD_PARTY.md.

use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use super::object::{CompletionPacket, Handle, KObject, ObjectBody, ObjectRef};
use super::process::Process;
use super::status::NtStatus;
use super::wait::{self, Dispatcher};
use crate::mm::heap::{KArc, TryVec};
use crate::net::tcp::{self, Addr, NetError, SocketId};
use crate::sync::IrqMutex;

pub const IOCTL_AFD_BIND: u32 = 0x12003;
pub const IOCTL_AFD_CONNECT: u32 = 0x12007;
pub const IOCTL_AFD_LISTEN: u32 = 0x1200b;
pub const IOCTL_AFD_WAIT_FOR_LISTEN: u32 = 0x1200c;
pub const IOCTL_AFD_ACCEPT: u32 = 0x12010;
pub const IOCTL_AFD_RECV: u32 = 0x12017;
pub const IOCTL_AFD_SEND: u32 = 0x1201f;
pub const IOCTL_AFD_DISCONNECT: u32 = 0x1202b;
pub const IOCTL_AFD_GETSOCKNAME: u32 = 0x1202f;
pub const IOCTL_AFD_GET_PEER_NAME: u32 = 0x12033;
pub const IOCTL_AFD_SUPER_ACCEPT: u32 = 0x12083;
pub const IOCTL_AFD_POLL: u32 = 0x12024;
pub const IOCTL_AFD_SET_INFO: u32 = 0x1203b;
const AFD_INFO_BLOCKING_MODE: u32 = 2;
pub const IOCTL_AFD_SUPER_CONNECT: u32 = 0x120c7;

const AFD_DISCONNECT_SEND: u32 = 1;
const AFD_MSG_OOB: u32 = 0x40;
const AFD_MSG_PEEK: u32 = 0x80;
const AFD_MSG_WAITALL: u32 = 0x4000;
const AFD_DISCONNECT_ABORT: u32 = 4;

/// The extended attribute NtCreateFile passes the open packet in.
const OPEN_PACKET_NAME: &[u8] = b"AfdOpenPacketXX";

const AF_INET: u16 = 2;
/// SOCKADDR_IN.
const SOCKADDR_LEN: usize = 16;
/// TRANSPORT_ADDRESS with one TA_ADDRESS holding a TDI_ADDRESS_IP.
const TRANSPORT_ADDRESS_LEN: usize = 22;
/// Bytes copied through the kernel per step of a send or receive.
const CHUNK: usize = 16 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transport {
    /// No open packet: a helper handle (e.g. for IOCTL_AFD_POLL).
    None,
    Tcp,
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
            Op::Recv { .. } => 0,
            Op::Send { .. } => 1,
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
    pending: IrqMutex<Vec<Pending>>,
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
    }
    // A poll waits on other endpoints' sockets: retry every pending poll.
    let pollers: Vec<KArc<EndpointState>> = {
        let mut list = POLLERS.lock();
        let mut copy = Vec::new();
        if copy.try_reserve(list.len()).is_ok() {
            list.retain(|e| e.pending.lock().iter().any(|p| p.op.kind() == 4));
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
    let Some(id) = *state.socket.lock() else { return 0 };
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

/// Reports a finished request: IO_STATUS_BLOCK, event, the endpoint's own
/// signal, the completion port. `immediate`: it finished inside its system
/// call (then a failure posts nothing, and FILE_SKIP_COMPLETION_PORT_ON_
/// SUCCESS suppresses the packet of a success).
fn finish(state: &EndpointState, io: &Io, status: NtStatus, information: u64, immediate: bool) {
    if io.iosb != 0 {
        let mut b = [0u8; 16];
        b[0..4].copy_from_slice(&status.0.to_le_bytes());
        b[8..16].copy_from_slice(&information.to_le_bytes());
        let _ = io.process.write(io.iosb, &b);
    }
    let failed = status.0 >= 0xc000_0000;
    if immediate && failed {
        return;
    }
    if let Some(e) = &io.event {
        wait::set_signal(e, 1);
    }
    if let Some((thread, apc)) = &io.apc {
        let _ = super::thread::queue_user_apc(thread, *apc);
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
        let packet = CompletionPacket { key, context: io.context, status: status.0, information };
        if let ObjectBody::IoCompletion(c) = &port.body
            && c.queue.lock().push(packet).is_ok()
        {
            wait::set_signal(&port, 1);
        }
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
    match attempt_inner(socket, op, process, non_blocking) {
        Ok(Some(information)) => Attempt::Done(NtStatus::SUCCESS, information),
        Ok(None) | Err(Failure::Net(NetError::WouldBlock)) => Attempt::Wait,
        Err(Failure::Net(e)) => Attempt::Done(status_of(e), 0),
        Err(Failure::Nt(status)) => Attempt::Done(status, 0),
    }
}

enum Failure {
    Net(NetError),
    Nt(NtStatus),
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
fn attempt_inner(
    socket: Option<SocketId>,
    op: &mut Op,
    process: &Process,
    non_blocking: bool,
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
        Op::WaitForListen { output, output_len } => {
            let (sequence, remote) = tcp::pending(id)?;
            let mut out = [0u8; 4 + TRANSPORT_ADDRESS_LEN];
            out[..4].copy_from_slice(&sequence.to_le_bytes());
            out[4..].copy_from_slice(&to_transport_address(remote));
            if (*output_len as usize) < out.len() {
                return Err(NtStatus::BUFFER_TOO_SMALL.into());
            }
            user_write(process, *output, &out)?;
            Ok(Some(out.len() as u64))
        }
        Op::SuperAccept { accept, output, receive, local } => {
            let (sequence, remote) = tcp::pending(id)?;
            let mut slot = accept.socket.lock();
            if slot.is_some() {
                return Err(NtStatus::INVALID_PARAMETER.into());
            }
            let new = tcp::accept(id, sequence, accept.token)?;
            // Like Windows, an accepted socket inherits the listener's
            // non-blocking mode (Nginx relies on it).
            accept.non_blocking.store(non_blocking, Ordering::Release);
            *slot = Some(new);
            drop(slot);
            let local_addr = tcp::local(new)?;
            let at = *output + u64::from(*receive);
            user_write(process, at, &to_sockaddr_in(local_addr))?;
            user_write(process, at + u64::from(*local), &to_sockaddr_in(remote))?;
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
    let mut out = Vec::new();
    out.try_reserve(POLL_HEADER + POLL_ENTRY * sockets.len()).map_err(|_| NtStatus::NO_MEMORY)?;
    out.resize(POLL_HEADER, 0);
    let mut count = 0u32;
    for (handle, state, wanted) in sockets {
        let events = poll_events(state) & wanted;
        if events != 0 {
            // Capacity is reserved above.
            let _ = out.try_extend_from_slice(&handle.to_le_bytes());
            let _ = out.try_extend_from_slice(&events.to_le_bytes());
            let _ = out.try_extend_from_slice(&0u32.to_le_bytes());
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

/// afd_poll_params: timeout (i64), count (u32), exclusive (u8), padding;
/// then 16-byte entries (packed to 4).
const POLL_HEADER: usize = 16;
const POLL_ENTRY: usize = 16;

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
                finish(state, &p.io, status, information, false);
            }
            Attempt::Wait => {
                blocked[kind] = kind != POLL_KIND;
                i += 1;
            }
        }
    }
}

/// `Op::kind` of a poll.
const POLL_KIND: usize = 4;

/// Starts a request: done at once if possible, else pending.
fn submit(state: &EndpointState, mut op: Op, io: Io) -> NtStatus {
    let mut pending = state.pending.lock();
    let kind = op.kind();
    if (usize::from(kind) == POLL_KIND || !pending.iter().any(|p| p.op.kind() == kind))
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
        let token = state.token;
        let mut pollers = POLLERS.lock();
        if !pollers.iter().any(|e| e.token == token) {
            let me = REGISTRY.lock().iter().find(|e| e.token == token).cloned();
            if let Some(me) = me
                && pollers.try_push(me).is_err()
            {
                return NtStatus::INSUFFICIENT_RESOURCES;
            }
        }
    }
    if pending.try_push(Pending { op, io }).is_err() {
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
            finish(state, &p.io, NtStatus::CANCELLED, 0, false);
            n += 1;
        } else {
            i += 1;
        }
    }
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
    if name.eq_ignore_ascii_case(tcp) {
        Ok(Transport::Tcp)
    } else {
        super::ledger::record("NtCreateFile", "afd transport", name_bytes as u64);
        Err(NtStatus::NOT_SUPPORTED)
    }
}

/// Opens an endpoint (NtCreateFile below `\Device\Afd`).
pub fn open(ea: &[u8]) -> Result<ObjectRef, NtStatus> {
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
        pending: IrqMutex::new(Vec::new()),
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
        NetError::NoSocket => NtStatus::INVALID_HANDLE,
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

    fn input<const N: usize>(&self) -> Result<[u8; N], NtStatus> {
        if (self.input_len as usize) < N {
            return Err(NtStatus::INVALID_PARAMETER);
        }
        let mut buf = [0u8; N];
        self.read(self.input, &mut buf)?;
        Ok(buf)
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

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn u64_at(b: &[u8], at: usize) -> u64 {
    let mut v = [0u8; 8];
    v.copy_from_slice(&b[at..at + 8]);
    u64::from_le_bytes(v)
}

/// SOCKADDR_IN -> address (IPv4 only).
fn sockaddr_in(b: &[u8]) -> Result<Addr, NtStatus> {
    if u16::from_le_bytes([b[0], b[1]]) != AF_INET {
        return Err(NtStatus::INVALID_ADDRESS_COMPONENT);
    }
    Ok(Addr { port: u16::from_be_bytes([b[2], b[3]]), ip: [b[4], b[5], b[6], b[7]] })
}

fn to_sockaddr_in(a: Addr) -> [u8; SOCKADDR_LEN] {
    let mut b = [0u8; SOCKADDR_LEN];
    b[..2].copy_from_slice(&AF_INET.to_le_bytes());
    b[2..4].copy_from_slice(&a.port.to_be_bytes());
    b[4..8].copy_from_slice(&a.ip);
    b
}

/// TRANSPORT_ADDRESS (count, length, type, TDI_ADDRESS_IP) -> address.
fn transport_address(b: &[u8]) -> Result<Addr, NtStatus> {
    let count = u32_at(b, 0);
    let kind = u16::from_le_bytes([b[6], b[7]]);
    if count < 1 || kind != AF_INET {
        return Err(NtStatus::INVALID_ADDRESS_COMPONENT);
    }
    Ok(Addr { port: u16::from_be_bytes([b[8], b[9]]), ip: [b[10], b[11], b[12], b[13]] })
}

fn to_transport_address(a: Addr) -> [u8; TRANSPORT_ADDRESS_LEN] {
    let mut b = [0u8; TRANSPORT_ADDRESS_LEN];
    b[..4].copy_from_slice(&1u32.to_le_bytes());
    b[4..6].copy_from_slice(&14u16.to_le_bytes());
    b[6..8].copy_from_slice(&AF_INET.to_le_bytes());
    b[8..10].copy_from_slice(&a.port.to_be_bytes());
    b[10..14].copy_from_slice(&a.ip);
    b
}

/// Reads the WSABUF array of a RECV/SEND request: (address, length) pairs.
fn buffers(r: &Request<'_>) -> Result<Vec<(u64, usize)>, NtStatus> {
    let info: [u8; 24] = r.input()?;
    let (array, count) = (u64_at(&info, 0), u32_at(&info, 8) as usize);
    if count > 1024 {
        return Err(NtStatus::INVALID_PARAMETER);
    }
    let mut out = Vec::new();
    out.try_reserve(count).map_err(|_| NtStatus::NO_MEMORY)?;
    for i in 0..count {
        // WSABUF: {u32 len; (pad); char *buf}.
        let mut wsabuf = [0u8; 16];
        r.read(array + 16 * i as u64, &mut wsabuf)?;
        out.try_push((u64_at(&wsabuf, 8), u32_at(&wsabuf, 0) as usize))
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
    let current = || *ep.socket.lock();
    let socket = || current().ok_or(NtStatus::INVALID_CONNECTION);
    Ok(match r.code {
        IOCTL_AFD_BIND => {
            let b: [u8; 4 + SOCKADDR_LEN] = r.input()?;
            let local = sockaddr_in(&b[4..])?;
            let mut slot = ep.socket.lock();
            if slot.is_some() {
                return Err(NtStatus::INVALID_PARAMETER);
            }
            let id = tcp::bind(local, ep.token).map_err(status_of)?;
            *slot = Some(id);
            drop(slot);
            let bound = tcp::local(id).map_err(status_of)?;
            Immediate::Done(if r.output_len as usize >= SOCKADDR_LEN {
                r.output(&to_sockaddr_in(bound))?
            } else {
                0
            })
        }
        IOCTL_AFD_LISTEN => {
            let b: [u8; 12] = r.input()?;
            let backlog = u32_at(&b, 4) as i32;
            tcp::listen(socket()?, backlog.max(1) as usize).map_err(status_of)?;
            Immediate::Done(0)
        }
        IOCTL_AFD_CONNECT | IOCTL_AFD_SUPER_CONNECT => {
            let remote = if r.code == IOCTL_AFD_CONNECT {
                let b: [u8; 12 + TRANSPORT_ADDRESS_LEN] = r.input()?;
                transport_address(&b[12..])?
            } else {
                let b: [u8; 4 + TRANSPORT_ADDRESS_LEN] = r.input()?;
                transport_address(&b[4..])?
            };
            let id = tcp::connect(current(), remote, ep.token).map_err(status_of)?;
            *ep.socket.lock() = Some(id);
            Immediate::Async(Op::Connect)
        }
        IOCTL_AFD_WAIT_FOR_LISTEN => {
            socket()?;
            Immediate::Async(Op::WaitForListen { output: r.output, output_len: r.output_len })
        }
        IOCTL_AFD_ACCEPT => {
            let b: [u8; 16] = r.input()?;
            let (sequence, handle) = (u32_at(&b, 4), u64_at(&b, 8));
            let accepting = endpoint_of(r.process, handle)?;
            let mut slot = accepting.socket.lock();
            if slot.is_some() {
                return Err(NtStatus::INVALID_PARAMETER);
            }
            *slot = Some(tcp::accept(socket()?, sequence, accepting.token).map_err(status_of)?);
            // The listener's non-blocking mode carries over (as on Windows).
            accepting
                .non_blocking
                .store(ep.non_blocking.load(Ordering::Acquire), Ordering::Release);
            Immediate::Done(0)
        }
        IOCTL_AFD_SUPER_ACCEPT => {
            // {u8 san; u8 fix_alignment; HANDLE accept; u32 receive_length;
            //  u32 local_length; u32 remote_length}
            let b: [u8; 28] = r.input()?;
            let (handle, receive) = (u64_at(&b, 8), u32_at(&b, 16));
            let (local, remote) = (u32_at(&b, 20), u32_at(&b, 24));
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
            let header: [u8; POLL_HEADER] = r.input()?;
            let timeout = u64_at(&header, 0) as i64;
            let count = u32_at(&header, 8) as usize;
            if count > 1024 || (r.input_len as usize) < POLL_HEADER + POLL_ENTRY * count {
                return Err(NtStatus::INVALID_PARAMETER);
            }
            let mut sockets = Vec::new();
            sockets.try_reserve(count).map_err(|_| NtStatus::NO_MEMORY)?;
            for i in 0..count {
                let mut entry = [0u8; POLL_ENTRY];
                r.read(r.input + (POLL_HEADER + POLL_ENTRY * i) as u64, &mut entry)?;
                let handle = u64_at(&entry, 0);
                let state = endpoint_of(r.process, handle)?;
                let _ = sockets.try_push((handle, state, u32_at(&entry, 8)));
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
            Immediate::Async(Op::Send { buffers: buffers(r)?, done: 0 })
        }
        IOCTL_AFD_RECV => {
            socket()?;
            // afd_recv_params.msg_flags (Wine): AFD_MSG_PEEK 0x80; OOB
            // (0x40) and WAITALL (0x4000) are not supported yet.
            let info: [u8; 24] = r.input()?;
            let msg_flags = u32_at(&info, 16);
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
                if let Some(id) = ep.socket.lock().take() {
                    tcp::close(id, true);
                }
            } else if kind & AFD_DISCONNECT_SEND != 0 {
                tcp::shutdown_send(socket()?).map_err(status_of)?;
            }
            Immediate::Done(0)
        }
        IOCTL_AFD_GETSOCKNAME => {
            let local = tcp::local(socket().map_err(|_| NtStatus::INVALID_PARAMETER)?)
                .map_err(status_of)?;
            Immediate::Done(r.output(&to_sockaddr_in(local))?)
        }
        IOCTL_AFD_GET_PEER_NAME => {
            let remote = tcp::remote(socket()?).map_err(status_of)?;
            Immediate::Done(r.output(&to_sockaddr_in(remote))?)
        }
        code => {
            super::ledger::record("NtDeviceIoControlFile", "afd ioctl", u64::from(code));
            return Err(NtStatus::INVALID_DEVICE_REQUEST);
        }
    })
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

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
        assert_eq!(parse_open_packet(&ea[..10]), Err(NtStatus::INVALID_PARAMETER));
    }

    #[test]
    fn address_layouts() {
        let a = Addr { ip: [127, 0, 0, 1], port: 0x1234 };
        assert_eq!(sockaddr_in(&to_sockaddr_in(a)), Ok(a));
        assert_eq!(transport_address(&to_transport_address(a)), Ok(a));
        assert_eq!(&to_sockaddr_in(a)[..4], &[2, 0, 0x12, 0x34]);
    }
}

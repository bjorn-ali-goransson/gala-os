//! AFD: Windows sockets as file objects of `\Device\Afd`, driven by
//! NtDeviceIoControlFile, on the kernel's TCP API (`net::tcp`). Design,
//! IOCTL table and layout sources: docs/ARCHITECTURE.md, "AFD". Layouts
//! come from Wine's `include/wine/afd.h` (checked by Wine's tests against
//! Windows) and ReactOS's `drivers/afd/shared.h` (marked "to be verified"
//! there); see THIRD_PARTY.md.

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

use super::object::{Handle, KObject, ObjectBody, ObjectRef};
use super::process::Process;
use super::status::NtStatus;
use super::wait::{self, Dispatcher, Timeout};
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

const AFD_DISCONNECT_SEND: u32 = 1;
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

/// A socket endpoint (`ObjectBody::Socket`).
pub struct Endpoint {
    pub dispatcher: KArc<Dispatcher>,
    pub transport: Transport,
    token: u64,
    socket: IrqMutex<Option<SocketId>>,
}

impl Drop for Endpoint {
    fn drop(&mut self) {
        REGISTRY.lock().retain(|(t, _)| *t != self.token);
        if let Some(id) = self.socket.lock().take() {
            tcp::close(id, false);
        }
    }
}

/// Endpoints by token, for the notify hook.
static REGISTRY: IrqMutex<Vec<(u64, KArc<Dispatcher>)>> = IrqMutex::new(Vec::new());
static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);

/// `net::tcp`'s notify hook: the endpoint's state changed; its blocked
/// callers re-check.
fn notify(token: u64) {
    let dispatcher = REGISTRY.lock().iter().find(|(t, _)| *t == token).map(|(_, d)| d.clone());
    if let Some(d) = dispatcher {
        wait::signal_dispatcher(&d, 1);
    }
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
    let dispatcher = KArc::try_new(Dispatcher::new(0)).map_err(|_| NtStatus::NO_MEMORY)?;
    let token = NEXT_TOKEN.fetch_add(1, Ordering::Relaxed);
    REGISTRY.lock().try_push((token, dispatcher.clone())).map_err(|_| NtStatus::NO_MEMORY)?;
    let endpoint = Endpoint { dispatcher, transport, token, socket: IrqMutex::new(None) };
    // On failure the endpoint drops and unregisters itself.
    KObject::new(ObjectBody::Socket(endpoint)).map_err(|_| NtStatus::NO_MEMORY)
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

/// Runs `f` until it does not block, waiting on the endpoint in between.
/// The wait is bounded (50 ms) so that two threads clearing the same
/// endpoint's signal cannot strand each other.
fn blocking<R>(
    object: &ObjectRef,
    endpoint: &Endpoint,
    mut f: impl FnMut() -> Result<R, NetError>,
) -> Result<R, NtStatus> {
    loop {
        wait::signal_dispatcher(&endpoint.dispatcher, 0);
        match f() {
            Err(NetError::WouldBlock) => {}
            Ok(r) => return Ok(r),
            Err(e) => return Err(status_of(e)),
        }
        let timeout = Timeout::Tick(crate::time::ticks() + 50);
        let status = wait::wait_for(core::slice::from_ref(object), false, timeout);
        if status != NtStatus::SUCCESS && status != NtStatus::TIMEOUT {
            return Err(status);
        }
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

/// Handles an IOCTL on an endpoint; returns IoStatus.Information.
pub fn ioctl(r: &Request<'_>) -> Result<u64, NtStatus> {
    let ObjectBody::Socket(ep) = &r.object.body else {
        return Err(NtStatus::INVALID_DEVICE_REQUEST);
    };
    if ep.transport == Transport::None {
        super::ledger::record("NtDeviceIoControlFile", "afd helper ioctl", u64::from(r.code));
        return Err(NtStatus::INVALID_DEVICE_REQUEST);
    }
    let current = || *ep.socket.lock();
    let socket = || current().ok_or(NtStatus::INVALID_CONNECTION);
    match r.code {
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
            if r.output_len as usize >= SOCKADDR_LEN {
                r.output(&to_sockaddr_in(bound))
            } else {
                Ok(0)
            }
        }
        IOCTL_AFD_LISTEN => {
            let b: [u8; 12] = r.input()?;
            let backlog = u32_at(&b, 4) as i32;
            tcp::listen(socket()?, backlog.max(1) as usize).map_err(status_of)?;
            Ok(0)
        }
        IOCTL_AFD_CONNECT => {
            let b: [u8; 12 + TRANSPORT_ADDRESS_LEN] = r.input()?;
            let remote = transport_address(&b[12..])?;
            let id = tcp::connect(current(), remote, ep.token).map_err(status_of)?;
            *ep.socket.lock() = Some(id);
            blocking(r.object, ep, || tcp::connected(id))?;
            Ok(0)
        }
        IOCTL_AFD_WAIT_FOR_LISTEN => {
            let id = socket()?;
            let (sequence, remote) = blocking(r.object, ep, || tcp::pending(id))?;
            let mut out = [0u8; 4 + TRANSPORT_ADDRESS_LEN];
            out[..4].copy_from_slice(&sequence.to_le_bytes());
            out[4..].copy_from_slice(&to_transport_address(remote));
            r.output(&out)
        }
        IOCTL_AFD_ACCEPT => {
            let b: [u8; 16] = r.input()?;
            let (sequence, handle) = (u32_at(&b, 4), u64_at(&b, 8));
            let target =
                r.process.handles.lock().get(Handle(handle)).ok_or(NtStatus::INVALID_HANDLE)?;
            let ObjectBody::Socket(accepting) = &target.object.body else {
                return Err(NtStatus::OBJECT_TYPE_MISMATCH);
            };
            let mut slot = accepting.socket.lock();
            if slot.is_some() {
                return Err(NtStatus::INVALID_PARAMETER);
            }
            *slot = Some(tcp::accept(socket()?, sequence, accepting.token).map_err(status_of)?);
            Ok(0)
        }
        IOCTL_AFD_SEND => {
            let id = socket()?;
            let mut buf = bounce()?;
            let mut total = 0u64;
            for (addr, len) in buffers(r)? {
                let mut done = 0;
                while done < len {
                    let n = (len - done).min(CHUNK);
                    r.read(addr + done as u64, &mut buf[..n])?;
                    let mut queued = 0;
                    while queued < n {
                        queued += blocking(r.object, ep, || tcp::send(id, &buf[queued..n]))?;
                    }
                    done += n;
                }
                total += len as u64;
            }
            Ok(total)
        }
        IOCTL_AFD_RECV => {
            let id = socket()?;
            let list = buffers(r)?;
            let capacity: usize = list.iter().map(|&(_, l)| l).sum();
            let mut buf = bounce()?;
            // What has arrived (at least one byte, or end of stream), up
            // to the buffers' size: recv's semantics.
            let want = capacity.min(CHUNK);
            let got = blocking(r.object, ep, || tcp::recv(id, &mut buf[..want]))?;
            let mut copied = 0;
            for (addr, len) in list {
                if copied == got {
                    break;
                }
                let n = len.min(got - copied);
                r.write(addr, &buf[copied..copied + n])?;
                copied += n;
            }
            Ok(got as u64)
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
            Ok(0)
        }
        IOCTL_AFD_GETSOCKNAME => {
            let local = tcp::local(socket().map_err(|_| NtStatus::INVALID_PARAMETER)?)
                .map_err(status_of)?;
            r.output(&to_sockaddr_in(local))
        }
        IOCTL_AFD_GET_PEER_NAME => {
            let remote = tcp::remote(socket()?).map_err(status_of)?;
            r.output(&to_sockaddr_in(remote))
        }
        code => {
            super::ledger::record("NtDeviceIoControlFile", "afd ioctl", u64::from(code));
            Err(NtStatus::INVALID_DEVICE_REQUEST)
        }
    }
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

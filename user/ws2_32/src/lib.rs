//! gala's ws2_32.dll (ROADMAP N1b): Winsock on top of the kernel's AFD
//! driver, the way Windows layers it: a SOCKET is a handle to an endpoint
//! opened below `\Device\Afd`, and every socket call is an
//! NtDeviceIoControlFile on it (IOCTL codes and layouts: ARCHITECTURE.md,
//! "AFD"). Written from Microsoft's documentation of each function.
//!
//! Conventions, as documented: failures return SOCKET_ERROR (or
//! INVALID_SOCKET) and set the thread's last error (TEB+0x68, which
//! WSAGetLastError reads) to a WSA error code mapped from the NTSTATUS.
//! IPv4 TCP only so far; anything else fails with a WSA error and is
//! reported to the kernel's missing-features ledger.

#![no_std]
#![allow(non_snake_case)]
// Every export is a C-ABI function called by foreign code with the raw
// pointers its documentation describes; Rust callers do not exist.
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use core::sync::atomic::{AtomicU32, Ordering};

type Socket = usize;

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn NtCreateFile(
        handle: *mut usize,
        access: u32,
        attributes: *const ObjectAttributes,
        iosb: *mut [usize; 2],
        allocation: *const i64,
        file_attributes: u32,
        share: u32,
        disposition: u32,
        options: u32,
        ea: *const u8,
        ea_length: u32,
    ) -> u32;
    fn NtDeviceIoControlFile(
        h: usize,
        event: usize,
        apc: usize,
        apc_context: usize,
        iosb: *mut [usize; 2],
        code: u32,
        input: *const u8,
        input_length: u32,
        output: *mut u8,
        output_length: u32,
    ) -> u32;
    fn NtClose(h: usize) -> u32;
    fn NtCreateEvent(
        handle: *mut usize,
        access: u32,
        attributes: *const ObjectAttributes,
        kind: u32,
        initial: u8,
    ) -> u32;
    fn NtWaitForSingleObject(h: usize, alertable: u8, timeout: *const i64) -> u32;
    fn NtCancelIoFileEx(h: usize, request: *mut [usize; 2], iosb: *mut [usize; 2]) -> u32;
    fn GalaRecordMissing(name: *const u8, length: usize, what: u32, detail: u64) -> u32;
}

#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
    fn strlen(s: *const u8) -> usize;
    // Named by core's unwind tables (MSVC target); ntdll has it.
    fn __CxxFrameHandler3();
    // The stack probe for select's buffers (ntdll exports it).
    fn __chkstk();
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn VirtualAlloc(address: *mut u8, size: usize, kind: u32, protect: u32) -> *mut u8;
    fn VirtualFree(address: *mut u8, size: usize, kind: u32) -> i32;
}

#[repr(C)]
struct UnicodeString {
    length: u16,
    maximum_length: u16,
    buffer: *const u16,
}

#[repr(C)]
struct ObjectAttributes {
    length: u32,
    root_directory: usize,
    object_name: *const UnicodeString,
    attributes: u32,
    security_descriptor: usize,
    security_quality_of_service: usize,
}

const INVALID_SOCKET: Socket = !0;
const SOCKET_ERROR: i32 = -1;

const AF_INET: i32 = 2;
const SOCK_STREAM: i32 = 1;
const IPPROTO_TCP: i32 = 6;
const SOCKADDR_LEN: i32 = 16;

const WSAEFAULT: u32 = 10014;
const WSAEINVAL: u32 = 10022;
const WSAENOTSOCK: u32 = 10038;
const WSAEPROTONOSUPPORT: u32 = 10043;
const WSAESOCKTNOSUPPORT: u32 = 10044;
const WSAEOPNOTSUPP: u32 = 10045;
const WSAEAFNOSUPPORT: u32 = 10047;
const WSAEADDRINUSE: u32 = 10048;
const WSAEADDRNOTAVAIL: u32 = 10049;
const WSAENETUNREACH: u32 = 10051;
const WSAECONNRESET: u32 = 10054;
const WSAENOBUFS: u32 = 10055;
const WSAENOTCONN: u32 = 10057;
const WSAECONNREFUSED: u32 = 10061;
const WSAVERNOTSUPPORTED: u32 = 10092;
const WSAENOPROTOOPT: u32 = 10042;
const WSAEWOULDBLOCK: u32 = 10035;
const WSAHOST_NOT_FOUND: i32 = 11001;
const IOCTL_AFD_POLL: u32 = 0x12024;
const MSG_PEEK: i32 = 2;
/// afd_recv_params.msg_flags: peek (Wine's afd.h).
const AFD_MSG_PEEK: u32 = 0x80;
const IOCTL_AFD_SET_INFO: u32 = 0x1203b;
const WSA_IO_PENDING: u32 = 997;
const WSA_IO_INCOMPLETE: u32 = 996;
const WSA_OPERATION_ABORTED: u32 = 995;
const STATUS_PENDING: u32 = 0x103;
const WSANOTINITIALISED: u32 = 10093;

// AFD device controls (docs/ARCHITECTURE.md, "AFD").
const IOCTL_AFD_BIND: u32 = 0x12003;
const IOCTL_AFD_CONNECT: u32 = 0x12007;
const IOCTL_AFD_LISTEN: u32 = 0x1200b;
const IOCTL_AFD_WAIT_FOR_LISTEN: u32 = 0x1200c;
const IOCTL_AFD_ACCEPT: u32 = 0x12010;
const IOCTL_AFD_RECV: u32 = 0x12017;
const IOCTL_AFD_SEND: u32 = 0x1201f;
const IOCTL_AFD_DISCONNECT: u32 = 0x1202b;
const IOCTL_AFD_GETSOCKNAME: u32 = 0x1202f;
const IOCTL_AFD_GET_PEER_NAME: u32 = 0x12033;
const IOCTL_AFD_SUPER_ACCEPT: u32 = 0x12083;
const IOCTL_AFD_SUPER_CONNECT: u32 = 0x120c7;

/// Referenced by code that uses floating point (MSVC target convention).
#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

/// WSAStartup calls not yet matched by WSACleanup.
static STARTED: AtomicU32 = AtomicU32::new(0);

fn set_last_error(code: u32) {
    // SAFETY: TEB.LastErrorValue (x64 offset 0x68) of the current thread.
    unsafe { core::arch::asm!("mov gs:[0x68], {:e}", in(reg) code, options(nostack)) };
}

fn last_error() -> u32 {
    let value: u32;
    // SAFETY: reads TEB.LastErrorValue of the current thread.
    unsafe { core::arch::asm!("mov {:e}, gs:[0x68]", out(reg) value, options(nostack, readonly)) };
    value
}

/// The WSA error for an NTSTATUS from AFD.
fn wsa_error(status: u32) -> u32 {
    match status {
        0xc000_0120 => WSA_OPERATION_ABORTED,
        0xc000_00a3 => WSAEWOULDBLOCK,
        0xc000_0236 => WSAECONNREFUSED,
        0xc000_020d => WSAECONNRESET,
        0xc000_020a => WSAEADDRINUSE,
        0xc000_0207 => WSAEADDRNOTAVAIL,
        0xc000_0140 => WSAENOTCONN,
        0xc000_023c => WSAENETUNREACH,
        // Not a handle, or a handle to something that is not a socket.
        0xc000_0008 | 0xc000_0024 | 0xc000_0010 => WSAENOTSOCK,
        0xc000_009a | 0xc000_0017 => WSAENOBUFS,
        0xc000_0005 => WSAEFAULT,
        0xc000_00bb => WSAEOPNOTSUPP,
        _ => WSAEINVAL,
    }
}

/// Reports a gap to the kernel's missing-features ledger.
fn missing(function: &[u8], detail: u64) {
    // SAFETY: a valid buffer.
    unsafe { GalaRecordMissing(function.as_ptr(), function.len(), 2, detail) };
}

/// Sets the last error and returns SOCKET_ERROR.
fn fail(code: u32) -> i32 {
    set_last_error(code);
    SOCKET_ERROR
}

fn started() -> bool {
    STARTED.load(Ordering::Acquire) > 0
}

/// A blocking NtDeviceIoControlFile on a socket: AFD answers
/// STATUS_PENDING when it has to wait, so wait for the request's event.
/// Returns IoStatus.Information or the status.
fn afd(s: Socket, code: u32, input: &[u8], output: &mut [u8]) -> Result<usize, u32> {
    let mut event = 0usize;
    // SAFETY: valid output pointer; EVENT_ALL_ACCESS, notification event.
    let status = unsafe { NtCreateEvent(&mut event, 0x1f_0003, core::ptr::null(), 0, 0) };
    if status != 0 {
        return Err(status);
    }
    let mut iosb = [0usize; 2];
    // SAFETY: valid buffers of the lengths given; they live until the
    // request completes (we wait for it below).
    let mut status = unsafe {
        NtDeviceIoControlFile(
            s,
            event,
            0,
            0,
            &mut iosb,
            code,
            input.as_ptr(),
            input.len() as u32,
            output.as_mut_ptr(),
            output.len() as u32,
        )
    };
    if status == STATUS_PENDING {
        // SAFETY: waits on our event; the kernel has written the status
        // block when it is set.
        unsafe {
            NtWaitForSingleObject(event, 0, core::ptr::null());
            status = core::ptr::read_volatile(&raw const iosb[0]) as u32;
        }
    }
    // SAFETY: our event.
    unsafe { NtClose(event) };
    if status == 0 { Ok(iosb[1]) } else { Err(status) }
}

/// OVERLAPPED (x64): the IO_STATUS_BLOCK (Internal, InternalHigh), the
/// offset, hEvent.
#[repr(C)]
pub struct Overlapped {
    internal: usize,
    internal_high: usize,
    offset: u64,
    event: usize,
}

/// An overlapped NtDeviceIoControlFile: the OVERLAPPED is the status
/// block, its event is set and (unless the event's low bit is set, as
/// documented) a completion packet carries it. Returns the NTSTATUS.
fn afd_overlapped(
    s: Socket,
    code: u32,
    input: &[u8],
    output: *mut u8,
    output_len: u32,
    ov: *mut Overlapped,
) -> u32 {
    // SAFETY: the caller's OVERLAPPED, valid until the I/O completes.
    let event = unsafe {
        (*ov).internal = STATUS_PENDING as usize;
        (*ov).internal_high = 0;
        (*ov).event
    };
    let context = if event & 1 != 0 { 0 } else { ov as usize };
    // SAFETY: as above; the input is copied by the kernel during the call.
    unsafe {
        NtDeviceIoControlFile(
            s,
            event & !1,
            0,
            context,
            ov.cast(),
            code,
            input.as_ptr(),
            input.len() as u32,
            output,
            output_len,
        )
    }
}

/// The result of an overlapped call as Winsock reports it: 0 (done),
/// or SOCKET_ERROR with WSA_IO_PENDING or the mapped error.
fn overlapped_result(status: u32) -> i32 {
    match status {
        0 => 0,
        STATUS_PENDING => fail(WSA_IO_PENDING),
        s => fail(wsa_error(s)),
    }
}

/// Runs an AFD request, mapping a failure to the last error.
fn afd_call(s: Socket, code: u32, input: &[u8], output: &mut [u8]) -> Result<usize, i32> {
    if !started() {
        return Err(fail(WSANOTINITIALISED));
    }
    afd(s, code, input, output).map_err(|status| fail(wsa_error(status)))
}

/// Reads a SOCKADDR_IN the caller passed (`len` bytes at `name`).
fn read_sockaddr(name: *const u8, len: i32) -> Result<[u8; 16], i32> {
    if name.is_null() || len < SOCKADDR_LEN {
        return Err(fail(WSAEFAULT));
    }
    let mut b = [0u8; 16];
    // SAFETY: the caller's buffer holds at least 16 bytes.
    unsafe { core::ptr::copy_nonoverlapping(name, b.as_mut_ptr(), 16) };
    if i32::from(u16::from_le_bytes([b[0], b[1]])) != AF_INET {
        return Err(fail(WSAEAFNOSUPPORT));
    }
    Ok(b)
}

/// Writes a SOCKADDR_IN into the caller's (`name`, `*len`) buffer.
fn write_sockaddr(addr: &[u8; 16], name: *mut u8, len: *mut i32) -> Result<(), i32> {
    if name.is_null() {
        return Ok(());
    }
    // SAFETY: the caller passes a valid length pointer with the buffer.
    if len.is_null() || unsafe { *len } < SOCKADDR_LEN {
        return Err(fail(WSAEFAULT));
    }
    // SAFETY: the buffer holds `*len` >= 16 bytes.
    unsafe {
        core::ptr::copy_nonoverlapping(addr.as_ptr(), name, 16);
        *len = SOCKADDR_LEN;
    }
    Ok(())
}

/// A TRANSPORT_ADDRESS for an IPv4 SOCKADDR_IN.
fn transport_address(sa: &[u8; 16]) -> [u8; 22] {
    let mut t = [0u8; 22];
    t[..4].copy_from_slice(&1u32.to_le_bytes());
    t[4..6].copy_from_slice(&14u16.to_le_bytes());
    t[6..8].copy_from_slice(&2u16.to_le_bytes());
    t[8..14].copy_from_slice(&sa[2..8]); // port and address, network order
    t
}

/// The SOCKADDR_IN of a TRANSPORT_ADDRESS.
fn sockaddr_of(t: &[u8]) -> [u8; 16] {
    let mut sa = [0u8; 16];
    sa[..2].copy_from_slice(&2u16.to_le_bytes());
    sa[2..8].copy_from_slice(&t[8..14]);
    sa
}

/// Opens a TCP endpoint: NtCreateFile of `\Device\Afd\Endpoint` with the
/// open packet (ReactOS's AFD_CREATE_PACKET, transport `\Device\Tcp`) as
/// the extended attribute `AfdOpenPacketXX`.
fn open_endpoint() -> Result<Socket, u32> {
    const NAME: &[u8] = b"AfdOpenPacketXX";
    const TRANSPORT: &str = "\\Device\\Tcp";
    // FILE_FULL_EA_INFORMATION header, name + NUL, then the packet.
    let mut ea = [0u8; 8 + 16 + 12 + 22];
    ea[5] = NAME.len() as u8;
    ea[6..8].copy_from_slice(&(12u16 + 22).to_le_bytes());
    ea[8..8 + NAME.len()].copy_from_slice(NAME);
    let packet = 8 + NAME.len() + 1;
    ea[packet + 8..packet + 12].copy_from_slice(&22u32.to_le_bytes());
    for (i, c) in TRANSPORT.encode_utf16().enumerate() {
        ea[packet + 12 + 2 * i..packet + 14 + 2 * i].copy_from_slice(&c.to_le_bytes());
    }
    let path: [u16; 20] = {
        let mut p = [0u16; 20];
        for (i, c) in "\\Device\\Afd\\Endpoint".encode_utf16().enumerate() {
            p[i] = c;
        }
        p
    };
    let name = UnicodeString { length: 40, maximum_length: 40, buffer: path.as_ptr() };
    let attributes = ObjectAttributes {
        length: size_of::<ObjectAttributes>() as u32,
        root_directory: 0,
        object_name: &name,
        attributes: 0x40, // OBJ_CASE_INSENSITIVE
        security_descriptor: 0,
        security_quality_of_service: 0,
    };
    let (mut handle, mut iosb) = (0usize, [0usize; 2]);
    // SAFETY: valid pointers to the structures above.
    let status = unsafe {
        NtCreateFile(
            &mut handle,
            0xc010_0000, // GENERIC_READ | GENERIC_WRITE | SYNCHRONIZE
            &attributes,
            &mut iosb,
            core::ptr::null(),
            0,
            3, // FILE_SHARE_READ | FILE_SHARE_WRITE
            3, // FILE_OPEN_IF
            0,
            ea.as_ptr(),
            ea.len() as u32,
        )
    };
    if status == 0 { Ok(handle) } else { Err(status) }
}

/// WSAStartup(wVersionRequested, lpWSAData): version 2.2 at most.
#[unsafe(no_mangle)]
pub extern "system" fn WSAStartup(version: u16, data: *mut u8) -> i32 {
    let (major, minor) = (version & 0xff, version >> 8);
    if major < 1 {
        return WSAVERNOTSUPPORTED as i32;
    }
    if data.is_null() {
        return WSAEFAULT as i32;
    }
    let granted: u16 = if major > 2 || (major == 2 && minor >= 2) { 0x0202 } else { version };
    // WSADATA (x64): wVersion, wHighVersion, iMaxSockets, iMaxUdpDg,
    // lpVendorInfo, szDescription[257], szSystemStatus[129].
    // SAFETY: the caller passes a WSADATA (408 bytes on x64).
    unsafe {
        memset(data, 0, 408);
        core::ptr::write_unaligned(data.cast::<u16>(), granted);
        core::ptr::write_unaligned(data.add(2).cast::<u16>(), 0x0202);
        let description = b"WinSock 2.0";
        core::ptr::copy_nonoverlapping(description.as_ptr(), data.add(16), description.len());
        let status = b"Running";
        core::ptr::copy_nonoverlapping(status.as_ptr(), data.add(16 + 257), status.len());
    }
    STARTED.fetch_add(1, Ordering::AcqRel);
    0
}

#[unsafe(no_mangle)]
pub extern "system" fn WSACleanup() -> i32 {
    let ok =
        STARTED.fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| n.checked_sub(1)).is_ok();
    if ok { 0 } else { fail(WSANOTINITIALISED) }
}

#[unsafe(no_mangle)]
pub extern "system" fn WSAGetLastError() -> i32 {
    last_error() as i32
}

#[unsafe(no_mangle)]
pub extern "system" fn WSASetLastError(error: i32) {
    set_last_error(error as u32);
}

/// socket(af, type, protocol): IPv4 TCP.
#[unsafe(no_mangle)]
pub extern "system" fn socket(af: i32, kind: i32, protocol: i32) -> Socket {
    if !started() {
        fail(WSANOTINITIALISED);
        return INVALID_SOCKET;
    }
    if af != AF_INET {
        missing(b"ws2_32.dll!socket(address family)", af as u64);
        fail(WSAEAFNOSUPPORT);
        return INVALID_SOCKET;
    }
    if kind != SOCK_STREAM {
        missing(b"ws2_32.dll!socket(type)", kind as u64);
        fail(WSAESOCKTNOSUPPORT);
        return INVALID_SOCKET;
    }
    if protocol != 0 && protocol != IPPROTO_TCP {
        fail(WSAEPROTONOSUPPORT);
        return INVALID_SOCKET;
    }
    match open_endpoint() {
        Ok(s) => s,
        Err(status) => {
            fail(wsa_error(status));
            INVALID_SOCKET
        }
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn bind(s: Socket, name: *const u8, len: i32) -> i32 {
    let sa = match read_sockaddr(name, len) {
        Ok(sa) => sa,
        Err(e) => return e,
    };
    // {u32 share access; SOCKADDR}; the bound address comes back.
    let mut input = [0u8; 20];
    input[4..].copy_from_slice(&sa);
    let mut bound = [0u8; 16];
    match afd_call(s, IOCTL_AFD_BIND, &input, &mut bound) {
        Ok(_) => 0,
        Err(e) => e,
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn listen(s: Socket, backlog: i32) -> i32 {
    let mut input = [0u8; 12];
    input[4..8].copy_from_slice(&backlog.clamp(1, 0x7fff_ffff).to_le_bytes());
    match afd_call(s, IOCTL_AFD_LISTEN, &input, &mut []) {
        Ok(_) => 0,
        Err(e) => e,
    }
}

/// accept: wait for a connection on the listener, open a new endpoint and
/// attach the connection to it (WAIT_FOR_LISTEN, then ACCEPT).
#[unsafe(no_mangle)]
pub extern "system" fn accept(s: Socket, addr: *mut u8, len: *mut i32) -> Socket {
    let mut pending = [0u8; 4 + 22];
    if afd_call(s, IOCTL_AFD_WAIT_FOR_LISTEN, &[], &mut pending).is_err() {
        return INVALID_SOCKET;
    }
    let new = match open_endpoint() {
        Ok(h) => h,
        Err(status) => {
            fail(wsa_error(status));
            return INVALID_SOCKET;
        }
    };
    // {u32 use_san; u32 sequence; HANDLE accept endpoint}.
    let mut input = [0u8; 16];
    input[4..8].copy_from_slice(&pending[..4]);
    input[8..16].copy_from_slice(&(new as u64).to_le_bytes());
    if afd_call(s, IOCTL_AFD_ACCEPT, &input, &mut []).is_err() {
        // SAFETY: closes the endpoint opened above.
        unsafe { NtClose(new) };
        return INVALID_SOCKET;
    }
    if write_sockaddr(&sockaddr_of(&pending[4..]), addr, len).is_err() {
        // SAFETY: as above.
        unsafe { NtClose(new) };
        return INVALID_SOCKET;
    }
    new
}

#[unsafe(no_mangle)]
pub extern "system" fn connect(s: Socket, name: *const u8, len: i32) -> i32 {
    let sa = match read_sockaddr(name, len) {
        Ok(sa) => sa,
        Err(e) => return e,
    };
    // {u8 use_san; u32 root; u32 unknown; TRANSPORT_ADDRESS}.
    let mut input = [0u8; 12 + 22];
    input[12..].copy_from_slice(&transport_address(&sa));
    match afd_call(s, IOCTL_AFD_CONNECT, &input, &mut []) {
        Ok(_) => 0,
        Err(e) => e,
    }
}

/// An AFD_SEND_INFO / AFD_RECV_INFO with one WSABUF.
fn transfer(s: Socket, code: u32, buf: *const u8, len: i32, flags: i32) -> i32 {
    if len < 0 {
        return fail(WSAEINVAL);
    }
    // MSG_PEEK on receives; MSG_OOB, MSG_WAITALL and send flags not yet.
    let peek = code == IOCTL_AFD_RECV && flags == MSG_PEEK;
    if flags != 0 && !peek {
        missing(b"ws2_32.dll!send/recv(flags)", flags as u64);
        return fail(WSAEOPNOTSUPP);
    }
    // WSABUF {u32 len; char *buf}, then {WSABUF *; u32 count; u32 afd
    // flags; u32 message flags (AFD_MSG_PEEK)}.
    let mut wsabuf = [0u8; 16];
    wsabuf[..4].copy_from_slice(&(len as u32).to_le_bytes());
    wsabuf[8..].copy_from_slice(&(buf as u64).to_le_bytes());
    let mut info = [0u8; 24];
    info[..8].copy_from_slice(&(wsabuf.as_ptr() as u64).to_le_bytes());
    info[8..12].copy_from_slice(&1u32.to_le_bytes());
    if peek {
        info[16..20].copy_from_slice(&AFD_MSG_PEEK.to_le_bytes());
    }
    match afd_call(s, code, &info, &mut []) {
        Ok(n) => n as i32,
        Err(e) => e,
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn send(s: Socket, buf: *const u8, len: i32, flags: i32) -> i32 {
    transfer(s, IOCTL_AFD_SEND, buf, len, flags)
}

#[unsafe(no_mangle)]
pub extern "system" fn recv(s: Socket, buf: *mut u8, len: i32, flags: i32) -> i32 {
    transfer(s, IOCTL_AFD_RECV, buf, len, flags)
}

fn query_name(s: Socket, code: u32, name: *mut u8, len: *mut i32) -> i32 {
    let mut sa = [0u8; 16];
    if let Err(e) = afd_call(s, code, &[], &mut sa) {
        return e;
    }
    if name.is_null() {
        return fail(WSAEFAULT);
    }
    match write_sockaddr(&sa, name, len) {
        Ok(()) => 0,
        Err(e) => e,
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn getsockname(s: Socket, name: *mut u8, len: *mut i32) -> i32 {
    query_name(s, IOCTL_AFD_GETSOCKNAME, name, len)
}

#[unsafe(no_mangle)]
pub extern "system" fn getpeername(s: Socket, name: *mut u8, len: *mut i32) -> i32 {
    query_name(s, IOCTL_AFD_GET_PEER_NAME, name, len)
}

/// shutdown(s, how): SD_RECEIVE 0, SD_SEND 1, SD_BOTH 2.
#[unsafe(no_mangle)]
pub extern "system" fn shutdown(s: Socket, how: i32) -> i32 {
    let kind: u32 = match how {
        0 => 2,
        1 => 1,
        2 => 3,
        _ => return fail(WSAEINVAL),
    };
    // {u32 type; LARGE_INTEGER timeout (-1: none)}.
    let mut input = [0u8; 16];
    input[..4].copy_from_slice(&kind.to_le_bytes());
    input[8..].copy_from_slice(&(-1i64).to_le_bytes());
    match afd_call(s, IOCTL_AFD_DISCONNECT, &input, &mut []) {
        Ok(_) => 0,
        Err(e) => e,
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn closesocket(s: Socket) -> i32 {
    if !started() {
        return fail(WSANOTINITIALISED);
    }
    // SAFETY: closing a handle has no memory effects here.
    match unsafe { NtClose(s) } {
        0 => 0,
        _ => fail(WSAENOTSOCK),
    }
}

/// WSASocketW(af, type, protocol, lpProtocolInfo, g, dwFlags): AFD
/// endpoints always allow overlapped I/O, so the flags change nothing.
#[unsafe(no_mangle)]
pub extern "system" fn WSASocketW(
    af: i32,
    kind: i32,
    protocol: i32,
    info: *const u8,
    group: u32,
    _flags: u32,
) -> Socket {
    if !info.is_null() || group != 0 {
        missing(b"ws2_32.dll!WSASocketW(protocol info or group)", group.into());
    }
    socket(af, kind, protocol)
}

/// WSARecv(s, lpBuffers, dwBufferCount, lpNumberOfBytesRecvd, lpFlags,
/// lpOverlapped, lpCompletionRoutine).
#[unsafe(no_mangle)]
pub extern "system" fn WSARecv(
    s: Socket,
    buffers: *const u8,
    count: u32,
    received: *mut u32,
    flags: *mut u32,
    ov: *mut Overlapped,
    routine: usize,
) -> i32 {
    // SAFETY: the caller passes valid flags (in/out).
    // SAFETY: as above.
    let peek = !flags.is_null() && unsafe { *flags } == MSG_PEEK as u32;
    if !flags.is_null() && unsafe { *flags } != 0 && !peek {
        missing(b"ws2_32.dll!WSARecv(flags)", unsafe { *flags }.into());
        return fail(WSAEOPNOTSUPP);
    }
    if peek {
        missing(b"ws2_32.dll!WSARecv(MSG_PEEK)", 0);
        return fail(WSAEOPNOTSUPP);
    }
    let result = transfer_buffers(s, IOCTL_AFD_RECV, buffers, count, received, ov, routine);
    if result == 0 && !flags.is_null() {
        // SAFETY: as above.
        unsafe { *flags = 0 };
    }
    result
}

/// WSASend(s, lpBuffers, dwBufferCount, lpNumberOfBytesSent, dwFlags,
/// lpOverlapped, lpCompletionRoutine).
#[unsafe(no_mangle)]
pub extern "system" fn WSASend(
    s: Socket,
    buffers: *const u8,
    count: u32,
    sent: *mut u32,
    flags: u32,
    ov: *mut Overlapped,
    routine: usize,
) -> i32 {
    if flags != 0 {
        missing(b"ws2_32.dll!WSASend(flags)", flags.into());
        return fail(WSAEOPNOTSUPP);
    }
    transfer_buffers(s, IOCTL_AFD_SEND, buffers, count, sent, ov, routine)
}

/// A RECV/SEND with the caller's WSABUF array (its layout is AFD's).
fn transfer_buffers(
    s: Socket,
    code: u32,
    buffers: *const u8,
    count: u32,
    bytes: *mut u32,
    ov: *mut Overlapped,
    routine: usize,
) -> i32 {
    if !started() {
        return fail(WSANOTINITIALISED);
    }
    if routine != 0 {
        missing(b"ws2_32.dll!WSARecv/WSASend(completion routine)", 0);
        return fail(WSAEOPNOTSUPP);
    }
    let mut info = [0u8; 24];
    info[..8].copy_from_slice(&(buffers as u64).to_le_bytes());
    info[8..12].copy_from_slice(&count.to_le_bytes());
    if ov.is_null() {
        return match afd(s, code, &info, &mut []) {
            Ok(n) => {
                if !bytes.is_null() {
                    // SAFETY: the caller's count pointer.
                    unsafe { *bytes = n as u32 };
                }
                0
            }
            Err(status) => fail(wsa_error(status)),
        };
    }
    let status = afd_overlapped(s, code, &info, core::ptr::null_mut(), 0, ov);
    if status == 0 && !bytes.is_null() {
        // SAFETY: done: the kernel wrote InternalHigh; the caller's count.
        unsafe { *bytes = (*ov).internal_high as u32 };
    }
    overlapped_result(status)
}

/// WSAGetOverlappedResult(s, lpOverlapped, lpcbTransfer, fWait, lpdwFlags).
#[unsafe(no_mangle)]
pub extern "system" fn WSAGetOverlappedResult(
    s: Socket,
    ov: *mut Overlapped,
    bytes: *mut u32,
    wait: i32,
    flags: *mut u32,
) -> i32 {
    if ov.is_null() || bytes.is_null() {
        set_last_error(WSAEFAULT);
        return 0;
    }
    // SAFETY: the caller's OVERLAPPED; the kernel writes it on completion.
    let status = || unsafe { core::ptr::read_volatile(&raw const (*ov).internal) } as u32;
    if status() == STATUS_PENDING {
        if wait == 0 {
            set_last_error(WSA_IO_INCOMPLETE);
            return 0;
        }
        // SAFETY: the OVERLAPPED's event, else the socket (signaled when
        // its I/O completes).
        let event = unsafe { (*ov).event } & !1;
        unsafe { NtWaitForSingleObject(if event != 0 { event } else { s }, 0, core::ptr::null()) };
    }
    // SAFETY: as above.
    unsafe {
        *bytes = core::ptr::read_volatile(&raw const (*ov).internal_high) as u32;
        if !flags.is_null() {
            *flags = 0;
        }
    }
    match status() {
        0 => 1,
        s => {
            set_last_error(wsa_error(s));
            0
        }
    }
}

/// AcceptEx(sListenSocket, sAcceptSocket, lpOutputBuffer,
/// dwReceiveDataLength, dwLocalAddressLength, dwRemoteAddressLength,
/// lpdwBytesReceived, lpOverlapped): IOCTL_AFD_SUPER_ACCEPT on the
/// listener. Also mswsock.dll's export (forwarded here).
#[unsafe(no_mangle)]
pub extern "system" fn AcceptEx(
    listener: Socket,
    accept: Socket,
    output: *mut u8,
    receive: u32,
    local: u32,
    remote: u32,
    bytes: *mut u32,
    ov: *mut Overlapped,
) -> i32 {
    if !started() {
        fail(WSANOTINITIALISED);
        return 0;
    }
    if ov.is_null() {
        fail(WSAEINVAL);
        return 0;
    }
    let mut info = [0u8; 28];
    info[8..16].copy_from_slice(&(accept as u64).to_le_bytes());
    info[16..20].copy_from_slice(&receive.to_le_bytes());
    info[20..24].copy_from_slice(&local.to_le_bytes());
    info[24..28].copy_from_slice(&remote.to_le_bytes());
    let total = receive.saturating_add(local).saturating_add(remote);
    let status = afd_overlapped(listener, IOCTL_AFD_SUPER_ACCEPT, &info, output, total, ov);
    if status == 0 && !bytes.is_null() {
        // SAFETY: the caller's count.
        unsafe { *bytes = 0 };
    }
    i32::from(overlapped_result(status) == 0)
}

/// GetAcceptExSockaddrs: the local and remote SOCKADDR_INs AcceptEx stored
/// at the start of its two address blocks.
#[unsafe(no_mangle)]
pub extern "system" fn GetAcceptExSockaddrs(
    output: *mut u8,
    receive: u32,
    local: u32,
    _remote: u32,
    local_addr: *mut *mut u8,
    local_len: *mut i32,
    remote_addr: *mut *mut u8,
    remote_len: *mut i32,
) {
    // SAFETY: the caller's AcceptEx buffer and output pointers.
    unsafe {
        let at = output.add(receive as usize);
        *local_addr = at;
        *local_len = SOCKADDR_LEN;
        *remote_addr = at.add(local as usize);
        *remote_len = SOCKADDR_LEN;
    }
}

/// ConnectEx (handed out by WSAIoctl): IOCTL_AFD_SUPER_CONNECT on a bound
/// socket.
extern "system" fn connect_ex(
    s: Socket,
    name: *const u8,
    len: i32,
    send: *const u8,
    send_len: u32,
    sent: *mut u32,
    ov: *mut Overlapped,
) -> i32 {
    if ov.is_null() {
        fail(WSAEINVAL);
        return 0;
    }
    if !send.is_null() && send_len > 0 {
        missing(b"ws2_32.dll!ConnectEx(send data)", send_len.into());
        fail(WSAEOPNOTSUPP);
        return 0;
    }
    let sa = match read_sockaddr(name, len) {
        Ok(sa) => sa,
        Err(_) => return 0,
    };
    // {u8 use_san; TRANSPORT_ADDRESS at 4}.
    let mut info = [0u8; 4 + 22];
    info[4..].copy_from_slice(&transport_address(&sa));
    let status = afd_overlapped(s, IOCTL_AFD_SUPER_CONNECT, &info, core::ptr::null_mut(), 0, ov);
    if status == 0 && !sent.is_null() {
        // SAFETY: the caller's count.
        unsafe { *sent = 0 };
    }
    i32::from(overlapped_result(status) == 0)
}

const SIO_GET_EXTENSION_FUNCTION_POINTER: u32 = 0xc800_0006;
const SIO_KEEPALIVE_VALS: u32 = 0x9800_0004;
const SIO_TCP_INITIAL_RTO: u32 = 0x9800_0011;
/// WSAID_ACCEPTEX, WSAID_CONNECTEX, WSAID_GETACCEPTEXSOCKADDRS (mswsock.h)
/// as GUID bytes.
const WSAID_ACCEPTEX: [u8; 16] =
    guid(0xb536_7df1, 0xcbac, 0x11cf, [0x95, 0xca, 0x00, 0x80, 0x5f, 0x48, 0xa1, 0x92]);
const WSAID_CONNECTEX: [u8; 16] =
    guid(0x25a2_07b9, 0xddf3, 0x4660, [0x8e, 0xe9, 0x76, 0xe5, 0x8c, 0x74, 0x06, 0x3e]);
const WSAID_GETACCEPTEXSOCKADDRS: [u8; 16] =
    guid(0xb536_7df2, 0xcbac, 0x11cf, [0x95, 0xca, 0x00, 0x80, 0x5f, 0x48, 0xa1, 0x92]);

const fn guid(a: u32, b: u16, c: u16, d: [u8; 8]) -> [u8; 16] {
    let (a, b, c) = (a.to_le_bytes(), b.to_le_bytes(), c.to_le_bytes());
    [a[0], a[1], a[2], a[3], b[0], b[1], c[0], c[1], d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7]]
}

/// WSAIoctl(s, dwIoControlCode, lpvInBuffer, cbInBuffer, lpvOutBuffer,
/// cbOutBuffer, lpcbBytesReturned, lpOverlapped, lpCompletionRoutine):
/// the extension function pointers; keepalive and initial-RTO tuning are
/// accepted without effect.
#[unsafe(no_mangle)]
pub extern "system" fn WSAIoctl(
    _s: Socket,
    code: u32,
    input: *const u8,
    input_len: u32,
    output: *mut u8,
    output_len: u32,
    returned: *mut u32,
    _ov: *mut Overlapped,
    _routine: usize,
) -> i32 {
    match code {
        SIO_GET_EXTENSION_FUNCTION_POINTER => {
            if input.is_null() || input_len < 16 || output.is_null() || output_len < 8 {
                return fail(WSAEFAULT);
            }
            let mut id = [0u8; 16];
            // SAFETY: the caller's 16-byte GUID.
            unsafe { core::ptr::copy_nonoverlapping(input, id.as_mut_ptr(), 16) };
            let function = match id {
                WSAID_ACCEPTEX => AcceptEx as *const () as usize,
                WSAID_CONNECTEX => connect_ex as *const () as usize,
                WSAID_GETACCEPTEXSOCKADDRS => GetAcceptExSockaddrs as *const () as usize,
                _ => {
                    missing(
                        b"ws2_32.dll!WSAIoctl(extension function)",
                        u64::from(u32::from_le_bytes([id[0], id[1], id[2], id[3]])),
                    );
                    return fail(WSAEINVAL);
                }
            };
            // SAFETY: the caller's 8-byte output buffer and count.
            unsafe {
                core::ptr::write_unaligned(output.cast::<usize>(), function);
                if !returned.is_null() {
                    *returned = 8;
                }
            }
            0
        }
        SIO_KEEPALIVE_VALS | SIO_TCP_INITIAL_RTO => {
            if !returned.is_null() {
                // SAFETY: the caller's count.
                unsafe { *returned = 0 };
            }
            0
        }
        _ => {
            missing(b"ws2_32.dll!WSAIoctl", code.into());
            fail(WSAEOPNOTSUPP)
        }
    }
}

const SOL_SOCKET: i32 = 0xffff;

/// setsockopt(s, level, optname, optval, optlen): options that only tune
/// the stack are accepted without effect (docs/ARCHITECTURE.md).
#[unsafe(no_mangle)]
pub extern "system" fn setsockopt(
    s: Socket,
    level: i32,
    name: i32,
    value: *const u8,
    len: i32,
) -> i32 {
    if !started() {
        return fail(WSANOTINITIALISED);
    }
    let _ = s;
    let accepted = match (level, name) {
        // SO_REUSEADDR, SO_KEEPALIVE, SO_LINGER, SO_SNDBUF, SO_RCVBUF,
        // SO_EXCLUSIVEADDRUSE, SO_UPDATE_ACCEPT_CONTEXT,
        // SO_UPDATE_CONNECT_CONTEXT.
        (SOL_SOCKET, 0x4 | 0x8 | 0x80 | 0x1001 | 0x1002 | -5 | 0x700b | 0x7010) => true,
        // TCP_NODELAY (already the behaviour), TCP_KEEPIDLE, TCP_KEEPCNT,
        // TCP_KEEPINTVL.
        (IPPROTO_TCP, 1 | 3 | 16 | 17) => true,
        _ => false,
    };
    if !accepted {
        missing(b"ws2_32.dll!setsockopt", ((level as u64) << 32) | name as u32 as u64);
        return fail(WSAENOPROTOOPT);
    }
    if len > 0 && value.is_null() {
        return fail(WSAEFAULT);
    }
    0
}

/// getsockopt: SO_ERROR (0) and SO_TYPE (stream) so far.
#[unsafe(no_mangle)]
pub extern "system" fn getsockopt(
    _s: Socket,
    level: i32,
    name: i32,
    value: *mut u8,
    len: *mut i32,
) -> i32 {
    let v: i32 = match (level, name) {
        (SOL_SOCKET, 0x1007) => 0,           // SO_ERROR
        (SOL_SOCKET, 0x1008) => SOCK_STREAM, // SO_TYPE
        _ => {
            missing(b"ws2_32.dll!getsockopt", ((level as u64) << 32) | name as u32 as u64);
            return fail(WSAENOPROTOOPT);
        }
    };
    // SAFETY: the caller's value buffer and length.
    unsafe {
        if value.is_null() || len.is_null() || *len < 4 {
            return fail(WSAEFAULT);
        }
        core::ptr::write_unaligned(value.cast::<i32>(), v);
        *len = 4;
    }
    0
}

/// WSAEnumProtocolsW(lpiProtocols, lpProtocolBuffer, lpdwBufferLength):
/// one provider, TCP over IPv4, with IFS handles (sockets are file
/// handles), which is what lets Go use FILE_SKIP_COMPLETION_PORT_ON_SUCCESS.
#[unsafe(no_mangle)]
pub extern "system" fn WSAEnumProtocolsW(
    protocols: *const i32,
    buffer: *mut u8,
    length: *mut u32,
) -> i32 {
    const SIZE: usize = 628; // WSAPROTOCOL_INFOW
    if length.is_null() {
        return fail(WSAEFAULT);
    }
    // TCP is listed unless the filter (zero-terminated) leaves it out.
    let mut wanted = protocols.is_null();
    let mut p = protocols;
    // SAFETY: the caller's zero-terminated list.
    while !p.is_null() && unsafe { *p } != 0 {
        wanted |= unsafe { *p } == IPPROTO_TCP;
        p = unsafe { p.add(1) };
    }
    if !wanted {
        return 0;
    }
    // SAFETY: the caller's length and buffer of that length.
    unsafe {
        if (*length as usize) < SIZE || buffer.is_null() {
            *length = SIZE as u32;
            return fail(WSAENOBUFS);
        }
        memset(buffer, 0, SIZE);
        // XP1_GUARANTEED_DELIVERY | XP1_GUARANTEED_ORDER |
        // XP1_GRACEFUL_CLOSE | XP1_EXPEDITED_DATA | XP1_IFS_HANDLES.
        core::ptr::write_unaligned(buffer.cast::<u32>(), 0x2_0066);
        let int_at =
            |off: usize, v: i32| core::ptr::write_unaligned(buffer.add(off).cast::<i32>(), v);
        int_at(76, 2); // iVersion
        int_at(80, AF_INET); // iAddressFamily
        int_at(84, SOCKADDR_LEN); // iMaxSockAddr
        int_at(88, SOCKADDR_LEN); // iMinSockAddr
        int_at(92, SOCK_STREAM); // iSocketType
        int_at(96, IPPROTO_TCP); // iProtocol
        int_at(104, 0); // iNetworkByteOrder: BIGENDIAN
        for (i, c) in "gala TCP/IP".encode_utf16().enumerate() {
            core::ptr::write_unaligned(buffer.add(116 + 2 * i).cast::<u16>(), c);
        }
    }
    1
}

/// fd_set (x64): u_int fd_count, then SOCKET fd_array[] at offset 8.
#[repr(C)]
pub struct FdSet {
    count: u32,
    array: [usize; 0],
}

/// The sockets of an fd_set (none for NULL).
fn fd_slice<'a>(set: *mut FdSet) -> &'a [usize] {
    if set.is_null() {
        return &[];
    }
    // SAFETY: the caller's fd_set: `count` handles follow the header.
    unsafe { core::slice::from_raw_parts((&raw const (*set).array).cast(), (*set).count as usize) }
}

#[unsafe(no_mangle)]
pub extern "system" fn __WSAFDIsSet(s: Socket, set: *mut FdSet) -> i32 {
    i32::from(fd_slice(set).contains(&s))
}

// AFD_POLL_* events (see the kernel's nt::afd).
const POLL_READ: u32 = 0x1 | 0x8 | 0x80; // READ, HUP, ACCEPT
const POLL_WRITE: u32 = 0x4 | 0x40; // WRITE, CONNECT
const POLL_EXCEPT: u32 = 0x2 | 0x100; // OOB, CONNECT_ERR
const MAX_SELECT: usize = 512;

/// select(nfds, readfds, writefds, exceptfds, timeout): one IOCTL_AFD_POLL
/// over all the sockets; the timeout is a wait on the request, then a
/// cancel. Returns how many set memberships are ready.
#[unsafe(no_mangle)]
pub extern "system" fn select(
    _nfds: i32,
    read: *mut FdSet,
    write: *mut FdSet,
    except: *mut FdSet,
    timeout: *const [i32; 2],
) -> i32 {
    if !started() {
        return fail(WSANOTINITIALISED);
    }
    // afd_poll_params: timeout, count, exclusive; then {SOCKET; flags;
    // status} entries of 16 bytes.
    let mut input = [0u8; 16 + 16 * MAX_SELECT];
    let mut n = 0usize;
    for (set, flags) in [(read, POLL_READ), (write, POLL_WRITE), (except, POLL_EXCEPT)] {
        for &sock in fd_slice(set) {
            let existing =
                (0..n).find(|&i| input[16 + 16 * i..24 + 16 * i] == (sock as u64).to_le_bytes());
            let i = match existing {
                Some(i) => i,
                None if n < MAX_SELECT => {
                    input[16 + 16 * n..24 + 16 * n].copy_from_slice(&(sock as u64).to_le_bytes());
                    n += 1;
                    n - 1
                }
                None => return fail(WSAEINVAL),
            };
            let at = 24 + 16 * i;
            let old = u32::from_le_bytes([input[at], input[at + 1], input[at + 2], input[at + 3]]);
            input[at..at + 4].copy_from_slice(&(old | flags).to_le_bytes());
        }
    }
    if n == 0 {
        return fail(WSAEINVAL);
    }
    // SAFETY: the caller's optional timeval {tv_sec, tv_usec}.
    let ms: Option<i64> = (!timeout.is_null())
        .then(|| unsafe { i64::from((*timeout)[0]) * 1000 + i64::from((*timeout)[1]) / 1000 });
    let poll_timeout: i64 = match ms {
        Some(0) => 0,
        _ => i64::MAX,
    };
    input[..8].copy_from_slice(&poll_timeout.to_le_bytes());
    input[8..12].copy_from_slice(&(n as u32).to_le_bytes());
    let first = usize::from_le_bytes(input[16..24].try_into().unwrap_or([0; 8]));
    let mut output = [0u8; 16 + 16 * MAX_SELECT];
    let status = poll(first, &input[..16 + 16 * n], &mut output, ms);
    let ready = match status {
        0 => u32::from_le_bytes([output[8], output[9], output[10], output[11]]) as usize,
        0xc000_0120 => 0, // timed out and cancelled
        s => return fail(wsa_error(s)),
    };
    // Rewrite the sets with the ready sockets.
    let mut total = 0;
    for (set, flags) in [(read, POLL_READ), (write, POLL_WRITE), (except, POLL_EXCEPT)] {
        if set.is_null() {
            continue;
        }
        let mut kept = 0u32;
        let wanted = fd_slice(set);
        let mut keep = [0usize; MAX_SELECT];
        for e in 0..ready.min(MAX_SELECT) {
            let at = 16 + 16 * e;
            let sock = usize::from_le_bytes(output[at..at + 8].try_into().unwrap_or([0; 8]));
            let events = u32::from_le_bytes([
                output[at + 8],
                output[at + 9],
                output[at + 10],
                output[at + 11],
            ]);
            if events & flags != 0 && wanted.contains(&sock) {
                keep[kept as usize] = sock;
                kept += 1;
            }
        }
        // SAFETY: the caller's fd_set has room for at least its old count,
        // which `kept` does not exceed.
        unsafe {
            (*set).count = kept;
            let array = (&raw mut (*set).array).cast::<usize>();
            for (i, &sock) in keep[..kept as usize].iter().enumerate() {
                *array.add(i) = sock;
            }
        }
        total += kept as i32;
    }
    total
}

/// Runs IOCTL_AFD_POLL on `s` and waits for it at most `ms` (None:
/// forever); on timeout the request is cancelled. Returns its status.
fn poll(s: Socket, input: &[u8], output: &mut [u8], ms: Option<i64>) -> u32 {
    let mut event = 0usize;
    // SAFETY: valid output pointer; EVENT_ALL_ACCESS, notification event.
    let status = unsafe { NtCreateEvent(&mut event, 0x1f_0003, core::ptr::null(), 0, 0) };
    if status != 0 {
        return status;
    }
    let mut iosb = [0usize; 2];
    // SAFETY: valid buffers; they live until the request completes (we
    // wait for it, cancelling it on timeout).
    let mut status = unsafe {
        NtDeviceIoControlFile(
            s,
            event,
            0,
            0,
            &mut iosb,
            IOCTL_AFD_POLL,
            input.as_ptr(),
            input.len() as u32,
            output.as_mut_ptr(),
            output.len() as u32,
        )
    };
    if status == STATUS_PENDING {
        let relative = ms.map(|ms| -ms.max(0) * 10_000);
        let timeout = relative.as_ref().map_or(core::ptr::null(), |t| t as *const i64);
        // SAFETY: our event and status block.
        unsafe {
            if NtWaitForSingleObject(event, 0, timeout) == 0x102 {
                NtCancelIoFileEx(s, &mut iosb, core::ptr::null_mut());
                NtWaitForSingleObject(event, 0, core::ptr::null());
            }
            status = core::ptr::read_volatile(&raw const iosb[0]) as u32;
        }
    }
    // SAFETY: our event.
    unsafe { NtClose(event) };
    status
}

const FIONBIO: u32 = 0x8004_667e;

/// ioctlsocket(s, cmd, argp): FIONBIO (non-blocking mode, kept by AFD).
#[unsafe(no_mangle)]
pub extern "system" fn ioctlsocket(s: Socket, cmd: i32, arg: *mut u32) -> i32 {
    if cmd as u32 != FIONBIO {
        missing(b"ws2_32.dll!ioctlsocket", u64::from(cmd as u32));
        return fail(WSAEOPNOTSUPP);
    }
    if arg.is_null() {
        return fail(WSAEFAULT);
    }
    // AFD_INFO {u32 class = AFD_INFO_BLOCKING_MODE; union at 8: BOOLEAN
    // non-blocking; u32 padding}.
    let mut info = [0u8; 24];
    info[..4].copy_from_slice(&2u32.to_le_bytes());
    // SAFETY: the caller's argument.
    info[8] = u8::from(unsafe { *arg } != 0);
    match afd_call(s, IOCTL_AFD_SET_INFO, &info, &mut []) {
        Ok(_) => 0,
        Err(e) => e,
    }
}

/// gethostname: gala's machine name.
#[unsafe(no_mangle)]
pub extern "system" fn gethostname(name: *mut u8, len: i32) -> i32 {
    const NAME: &[u8] = b"gala\0";
    if name.is_null() || len < NAME.len() as i32 {
        return fail(WSAEFAULT);
    }
    // SAFETY: the caller's buffer of `len` bytes.
    unsafe { core::ptr::copy_nonoverlapping(NAME.as_ptr(), name, NAME.len()) };
    0
}

/// getaddrinfo(node, service, hints, result): numeric IPv4 addresses,
/// "localhost" and a missing node (any address, or loopback) with a
/// numeric service; no name resolution yet (reported). One result, in a
/// page of its own (freeaddrinfo releases it).
#[unsafe(no_mangle)]
pub extern "system" fn getaddrinfo(
    node: *const u8,
    service: *const u8,
    hints: *const i32,
    result: *mut *mut u8,
) -> i32 {
    // SAFETY: the caller's NUL-terminated strings and hints (ai_flags,
    // ai_family, ai_socktype, ai_protocol first).
    unsafe {
        let (flags, family, socktype, protocol) = if hints.is_null() {
            (0, 0, 0, 0)
        } else {
            (*hints, *hints.add(1), *hints.add(2), *hints.add(3))
        };
        if family != 0 && family != AF_INET {
            return WSAEAFNOSUPPORT as i32;
        }
        let ip = if node.is_null() {
            if flags & 1 != 0 { [0, 0, 0, 0] } else { [127, 0, 0, 1] } // AI_PASSIVE
        } else {
            let text = core::slice::from_raw_parts(node, strlen(node));
            if text.eq_ignore_ascii_case(b"localhost") {
                [127, 0, 0, 1]
            } else if let Some(ip) = parse_ipv4(text) {
                ip
            } else {
                missing(b"ws2_32.dll!getaddrinfo(name resolution)", 0);
                return WSAHOST_NOT_FOUND;
            }
        };
        let port = if service.is_null() {
            0
        } else {
            match parse_u16(core::slice::from_raw_parts(service, strlen(service))) {
                Some(p) => p,
                None => {
                    missing(b"ws2_32.dll!getaddrinfo(service name)", 0);
                    return 10109; // WSATYPE_NOT_FOUND
                }
            }
        };
        let page = VirtualAlloc(core::ptr::null_mut(), 4096, 0x3000, 4);
        if page.is_null() {
            return WSAENOBUFS as i32;
        }
        // ADDRINFOA (x64): flags, family, socktype, protocol, addrlen,
        // canonname, addr, next; the SOCKADDR_IN follows it.
        let ai = page.cast::<i32>();
        *ai = flags;
        *ai.add(1) = AF_INET;
        *ai.add(2) = socktype;
        *ai.add(3) = protocol;
        *page.add(16).cast::<usize>() = SOCKADDR_LEN as usize;
        let sa = page.add(48);
        *page.add(32).cast::<*mut u8>() = sa;
        *sa = AF_INET as u8;
        core::ptr::copy_nonoverlapping(port.to_be_bytes().as_ptr(), sa.add(2), 2);
        core::ptr::copy_nonoverlapping(ip.as_ptr(), sa.add(4), 4);
        *result = page;
    }
    0
}

#[unsafe(no_mangle)]
pub extern "system" fn freeaddrinfo(ai: *mut u8) {
    if !ai.is_null() {
        // SAFETY: a page from getaddrinfo.
        unsafe { VirtualFree(ai, 0, 0x8000) };
    }
}

fn parse_u16(s: &[u8]) -> Option<u16> {
    if s.is_empty() || s.len() > 5 || !s.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let v = s.iter().fold(0u32, |v, &d| v * 10 + u32::from(d - b'0'));
    u16::try_from(v).ok()
}

fn parse_ipv4(s: &[u8]) -> Option<[u8; 4]> {
    let mut out = [0u8; 4];
    let mut parts = s.split(|&c| c == b'.');
    for byte in out.iter_mut() {
        let p = parts.next()?;
        let v = parse_u16(p)?;
        *byte = u8::try_from(v).ok()?;
    }
    parts.next().is_none().then_some(out)
}

#[unsafe(no_mangle)]
pub extern "system" fn WSASocketA(
    af: i32,
    kind: i32,
    protocol: i32,
    info: *const u8,
    group: u32,
    flags: u32,
) -> Socket {
    WSASocketW(af, kind, protocol, info, group, flags)
}

/// inet_addr: a dotted IPv4 address in network order (INADDR_NONE if not
/// one).
#[unsafe(no_mangle)]
pub extern "system" fn inet_addr(text: *const u8) -> u32 {
    if text.is_null() {
        return u32::MAX;
    }
    // SAFETY: the caller's NUL-terminated string.
    let s = unsafe { core::slice::from_raw_parts(text, strlen(text)) };
    parse_ipv4(s).map_or(u32::MAX, u32::from_ne_bytes)
}

/// inet_ntoa: dotted text of an address (network order), in a buffer the
/// next call reuses (per process here; Windows keeps one per thread).
#[unsafe(no_mangle)]
pub extern "system" fn inet_ntoa(address: u32) -> *const u8 {
    static mut TEXT: [u8; 16] = [0; 16];
    let mut n = 0;
    // SAFETY: our buffer: 4 * 3 digits + 3 dots + NUL fits in 16.
    unsafe {
        let text = &mut *(&raw mut TEXT);
        for (i, b) in address.to_ne_bytes().iter().enumerate() {
            if i > 0 {
                text[n] = b'.';
                n += 1;
            }
            for d in [b / 100, b / 10 % 10, b % 10].iter().skip(if *b >= 100 {
                0
            } else if *b >= 10 {
                1
            } else {
                2
            }) {
                text[n] = b'0' + d;
                n += 1;
            }
        }
        text[n] = 0;
        text.as_ptr()
    }
}

/// The legacy resolver (gethostbyname, ...): no name service yet; they
/// fail as they do for unknown names, reported.
#[unsafe(no_mangle)]
pub extern "system" fn gethostbyname(_name: *const u8) -> *const u8 {
    missing(b"ws2_32.dll!gethostbyname", 0);
    set_last_error(WSAHOST_NOT_FOUND as u32);
    core::ptr::null()
}

#[unsafe(no_mangle)]
pub extern "system" fn gethostbyaddr(_address: *const u8, _len: i32, _kind: i32) -> *const u8 {
    missing(b"ws2_32.dll!gethostbyaddr", 0);
    set_last_error(WSAHOST_NOT_FOUND as u32);
    core::ptr::null()
}

const WSANO_DATA: u32 = 11004;

#[unsafe(no_mangle)]
pub extern "system" fn getservbyname(_name: *const u8, _proto: *const u8) -> *const u8 {
    set_last_error(WSANO_DATA);
    core::ptr::null()
}

#[unsafe(no_mangle)]
pub extern "system" fn getservbyport(_port: i32, _proto: *const u8) -> *const u8 {
    set_last_error(WSANO_DATA);
    core::ptr::null()
}

/// sendto/recvfrom: datagram sockets do not exist yet (socket refuses
/// SOCK_DGRAM), so these have nothing to work on.
#[unsafe(no_mangle)]
pub extern "system" fn sendto(
    _s: Socket,
    _buf: *const u8,
    _len: i32,
    _flags: i32,
    _to: *const u8,
    _to_len: i32,
) -> i32 {
    missing(b"ws2_32.dll!sendto", 0);
    fail(WSAEOPNOTSUPP)
}

#[unsafe(no_mangle)]
pub extern "system" fn recvfrom(
    _s: Socket,
    _buf: *mut u8,
    _len: i32,
    _flags: i32,
    _from: *mut u8,
    _from_len: *mut i32,
) -> i32 {
    missing(b"ws2_32.dll!recvfrom", 0);
    fail(WSAEOPNOTSUPP)
}

#[unsafe(no_mangle)]
pub extern "system" fn htons(v: u16) -> u16 {
    v.to_be()
}

#[unsafe(no_mangle)]
pub extern "system" fn ntohs(v: u16) -> u16 {
    u16::from_be(v)
}

#[unsafe(no_mangle)]
pub extern "system" fn htonl(v: u32) -> u32 {
    v.to_be()
}

#[unsafe(no_mangle)]
pub extern "system" fn ntohl(v: u32) -> u32 {
    u32::from_be(v)
}

/// DllMain (the entry point our link flags name).
#[unsafe(no_mangle)]
pub extern "system" fn mainCRTStartup(_instance: usize, _reason: u32, _reserved: usize) -> i32 {
    1
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}

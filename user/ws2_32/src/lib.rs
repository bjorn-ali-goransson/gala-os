//! gala's ws2_32.dll (ROADMAP N1b): Winsock on top of the kernel's AFD
//! driver, the way Windows layers it: a SOCKET is a handle to an endpoint
//! opened below `\Device\Afd`, and every socket call is an
//! NtDeviceIoControlFile on it (IOCTL codes and layouts: ARCHITECTURE.md,
//! "AFD"). Written from Microsoft's documentation of each function.
//!
//! Conventions, as documented: failures return SOCKET_ERROR (or
//! INVALID_SOCKET) and set the thread's last error (TEB.LastErrorValue, which
//! WSAGetLastError reads) to a WSA error code mapped from the NTSTATUS.
//! IPv4 TCP and UDP so far; anything else fails with a WSA error and is
//! reported to the kernel's missing-features ledger.

#![no_std]
#![allow(non_snake_case)]
// Every export is a C-ABI function called by foreign code with the raw
// pointers its documentation describes; Rust callers do not exist.
#![allow(clippy::not_unsafe_ptr_arg_deref)]

mod afd;
mod layout;
mod nsp;
mod unimplemented;

use core::sync::atomic::{AtomicU32, Ordering};

type Socket = usize;

#[cfg_attr(
    target_arch = "x86",
    link(name = "ntdll", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "ntdll", kind = "raw-dylib"))]
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
    fn NtOpenProcess(
        handle: *mut usize,
        access: u32,
        attributes: *const ObjectAttributes,
        client_id: *const [usize; 2],
    ) -> u32;
    fn NtDuplicateObject(
        source_process: isize,
        source: usize,
        target_process: usize,
        target: *mut usize,
        access: u32,
        attributes: u32,
        options: u32,
    ) -> u32;
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

// The runtime functions ws2_32 calls itself: ntrt's on x86 (no link: they
// are in the same image).
#[cfg(target_arch = "x86")]
unsafe extern "C" {
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn strlen(s: *const u8) -> usize;
}

// The 32-bit build (SysWOW64) takes the memory routines, 64-bit
// division helpers and unwind personality from ntrt's x86 runtime
// (ARCHITECTURE "Plan: WOW64").
#[cfg(target_arch = "x86")]
use ntrt as _;

#[cfg(target_arch = "x86_64")]
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

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn VirtualAlloc(address: *mut u8, size: usize, kind: u32, protect: u32) -> *mut u8;
    fn VirtualFree(address: *mut u8, size: usize, kind: u32) -> i32;
    fn CreateEventW(attributes: usize, manual: i32, initial: i32, name: *const u16) -> usize;
    fn SetEvent(h: usize) -> i32;
    fn ResetEvent(h: usize) -> i32;
    fn CloseHandle(h: usize) -> i32;
    fn GetSystemDirectoryW(buffer: *mut u16, size: u32) -> u32;
    fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        security: usize,
        disposition: u32,
        flags: u32,
        template: usize,
    ) -> usize;
    fn ReadFile(h: usize, buffer: *mut u8, len: u32, read: *mut u32, overlapped: usize) -> i32;
}

/// WSACreateEvent: a manual-reset event, initially not signaled (NULL,
/// WSA_INVALID_EVENT, on failure).
#[unsafe(no_mangle)]
pub extern "system" fn WSACreateEvent() -> usize {
    // SAFETY: plain calls.
    unsafe { CreateEventW(0, 1, 0, core::ptr::null()) }
}

#[unsafe(no_mangle)]
pub extern "system" fn WSACloseEvent(event: usize) -> i32 {
    // SAFETY: as above.
    unsafe { CloseHandle(event) }
}

#[unsafe(no_mangle)]
pub extern "system" fn WSASetEvent(event: usize) -> i32 {
    // SAFETY: as above.
    unsafe { SetEvent(event) }
}

#[unsafe(no_mangle)]
pub extern "system" fn WSAResetEvent(event: usize) -> i32 {
    // SAFETY: as above.
    unsafe { ResetEvent(event) }
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
const AF_INET6: i32 = 23;
/// SOCKADDR_IN6.
const SOCKADDR6_LEN: i32 = 28;
const IPPROTO_IPV6: i32 = 41;
const SOCK_STREAM: i32 = 1;
const SOCK_DGRAM: i32 = 2;
const IPPROTO_TCP: i32 = 6;
const IPPROTO_UDP: i32 = 17;
const WSAEMSGSIZE: u32 = 10040;
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
const WSAESHUTDOWN: u32 = 10058;
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
/// gala's AFD_INFO class for IPV6_V6ONLY (kernel/src/nt/afd.rs).
const GALA_AFD_INFO_V6ONLY: u32 = 0x6761_0001;
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
const IOCTL_AFD_RECV_DATAGRAM: u32 = 0x1201b;
const IOCTL_AFD_SEND: u32 = 0x1201f;
const IOCTL_AFD_SEND_DATAGRAM: u32 = 0x12023;
const IOCTL_AFD_DISCONNECT: u32 = 0x1202b;
const IOCTL_AFD_GETSOCKNAME: u32 = 0x1202f;
const IOCTL_AFD_GET_PEER_NAME: u32 = 0x12033;
const IOCTL_AFD_SUPER_ACCEPT: u32 = 0x12083;
const IOCTL_AFD_SUPER_CONNECT: u32 = 0x120c7;

/// Referenced by code that uses floating point (MSVC target convention).
#[cfg(target_arch = "x86_64")]
#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

/// WSAStartup calls not yet matched by WSACleanup.
static STARTED: AtomicU32 = AtomicU32::new(0);

/// TEB.LastErrorValue: gs:[0x68] on x64, fs:[0x34] in the 32-bit TEB.
fn set_last_error(code: u32) {
    // SAFETY: TEB.LastErrorValue of the current thread.
    #[cfg(target_arch = "x86_64")]
    unsafe {
        core::arch::asm!("mov gs:[0x68], {:e}", in(reg) code, options(nostack))
    };
    // SAFETY: as above, in the 32-bit TEB.
    #[cfg(target_arch = "x86")]
    unsafe {
        core::arch::asm!("mov fs:[0x34], {:e}", in(reg) code, options(nostack))
    };
}

fn last_error() -> u32 {
    let value: u32;
    // SAFETY: reads TEB.LastErrorValue of the current thread.
    #[cfg(target_arch = "x86_64")]
    unsafe {
        core::arch::asm!("mov {:e}, gs:[0x68]", out(reg) value, options(nostack, readonly))
    };
    // SAFETY: as above, in the 32-bit TEB.
    #[cfg(target_arch = "x86")]
    unsafe {
        core::arch::asm!("mov {:e}, fs:[0x34]", out(reg) value, options(nostack, readonly))
    };
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
        // A send after shutdown(SD_SEND) (STATUS_PIPE_DISCONNECTED, as
        // Wine maps it).
        0xc000_00b0 => WSAESHUTDOWN,
        0xc000_023c => WSAENETUNREACH,
        // Not a handle, or a handle to something that is not a socket.
        0xc000_0008 | 0xc000_0024 | 0xc000_0010 => WSAENOTSOCK,
        // STATUS_INSUFFICIENT_RESOURCES, STATUS_NO_MEMORY, and
        // STATUS_QUOTA_EXCEEDED: the socket's buffers are over the
        // process's quota.
        0xc000_009a | 0xc000_0017 | 0xc000_0044 => WSAENOBUFS,
        0xc000_0005 => WSAEFAULT,
        0xc000_00bb => WSAEOPNOTSUPP,
        // A datagram larger than the buffer (the start copied), or than
        // the network carries.
        0x8000_0005 | 0xc000_0206 => WSAEMSGSIZE,
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
    afd_overlapped_apc(s, code, input, output, output_len, ov, 0)
}

/// `afd_overlapped` with a completion routine (`routine`, 0 for none): it
/// runs on this thread as an APC when the request completes, and Winsock
/// leaves the OVERLAPPED's event alone (the WSARecv docs).
fn afd_overlapped_apc(
    s: Socket,
    code: u32,
    input: &[u8],
    output: *mut u8,
    output_len: u32,
    ov: *mut Overlapped,
    routine: usize,
) -> u32 {
    // SAFETY: the caller's OVERLAPPED, valid until the I/O completes.
    let event = unsafe {
        (*ov).internal = STATUS_PENDING as usize;
        (*ov).internal_high = 0;
        (*ov).event
    };
    let (event, apc, context) = if routine != 0 {
        (0, completion_trampoline as *const () as usize, routine)
    } else if event & 1 != 0 {
        (event & !1, 0, 0)
    } else {
        (event, 0, ov as usize)
    };
    // SAFETY: as above; the input is copied by the kernel during the call.
    unsafe {
        NtDeviceIoControlFile(
            s,
            event,
            apc,
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

/// The APC of a request with a completion routine: (ApcContext = the
/// routine, IoStatusBlock = the OVERLAPPED) to the routine's (dwError,
/// cbTransferred, lpOverlapped, dwFlags).
extern "system" fn completion_trampoline(routine: usize, ov: *mut Overlapped, _reserved: usize) {
    // SAFETY: the routine the caller gave and its OVERLAPPED.
    unsafe {
        let routine: extern "system" fn(u32, u32, *mut Overlapped, u32) =
            core::mem::transmute(routine);
        let status = (*ov).internal as u32;
        let error = if status == 0 { 0 } else { wsa_error(status) };
        routine(error, (*ov).internal_high as u32, ov, 0);
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

/// A socket address: SOCKADDR_IN (16 bytes) or SOCKADDR_IN6 (28).
#[derive(Clone, Copy)]
struct Sa {
    b: [u8; 28],
    len: usize,
}

impl Sa {
    fn bytes(&self) -> &[u8] {
        &self.b[..self.len]
    }

    fn family(&self) -> i32 {
        i32::from(u16::from_le_bytes([self.b[0], self.b[1]]))
    }

    /// The unspecified address of `family` (port 0).
    fn any(family: i32) -> Sa {
        let mut b = [0u8; 28];
        b[..2].copy_from_slice(&(family as u16).to_le_bytes());
        Sa { b, len: if family == AF_INET6 { 28 } else { 16 } }
    }
}

/// Reads the socket address the caller passed (`len` bytes at `name`):
/// SOCKADDR_IN or SOCKADDR_IN6.
fn read_sockaddr(name: *const u8, len: i32) -> Result<Sa, i32> {
    if name.is_null() || len < SOCKADDR_LEN {
        return Err(fail(WSAEFAULT));
    }
    let mut b = [0u8; 28];
    // SAFETY: the caller's buffer holds at least 16 bytes.
    unsafe { core::ptr::copy_nonoverlapping(name, b.as_mut_ptr(), 16) };
    match i32::from(u16::from_le_bytes([b[0], b[1]])) {
        AF_INET => Ok(Sa { b, len: 16 }),
        AF_INET6 if len >= SOCKADDR6_LEN => {
            // SAFETY: the caller's buffer holds at least 28 bytes.
            unsafe { core::ptr::copy_nonoverlapping(name, b.as_mut_ptr(), 28) };
            Ok(Sa { b, len: 28 })
        }
        AF_INET6 => Err(fail(WSAEFAULT)),
        _ => Err(fail(WSAEAFNOSUPPORT)),
    }
}

/// Writes a socket address into the caller's (`name`, `*len`) buffer.
fn write_sockaddr(addr: &Sa, name: *mut u8, len: *mut i32) -> Result<(), i32> {
    if name.is_null() {
        return Ok(());
    }
    // SAFETY: the caller passes a valid length pointer with the buffer.
    if len.is_null() || (unsafe { *len } as usize) < addr.len {
        return Err(fail(WSAEFAULT));
    }
    // SAFETY: the buffer holds `*len` >= addr.len bytes.
    unsafe {
        core::ptr::copy_nonoverlapping(addr.b.as_ptr(), name, addr.len);
        *len = addr.len as i32;
    }
    Ok(())
}

/// A TRANSPORT_ADDRESS (TDI_ADDRESS_IP or TDI_ADDRESS_IP6) for a socket
/// address: the bytes and their length.
fn transport_address(sa: &Sa) -> ([u8; 34], usize) {
    let mut t = [0u8; 34];
    t[..4].copy_from_slice(&1u32.to_le_bytes());
    if sa.family() == AF_INET6 {
        t[4..6].copy_from_slice(&26u16.to_le_bytes());
        t[6..8].copy_from_slice(&(AF_INET6 as u16).to_le_bytes());
        t[8..10].copy_from_slice(&sa.b[2..4]); // port, network order
        t[10..34].copy_from_slice(&sa.b[4..28]); // flow info, address, scope id
        (t, 34)
    } else {
        t[4..6].copy_from_slice(&14u16.to_le_bytes());
        t[6..8].copy_from_slice(&2u16.to_le_bytes());
        t[8..14].copy_from_slice(&sa.b[2..8]); // port and address, network order
        (t, 22)
    }
}

/// The socket address of a TRANSPORT_ADDRESS.
fn sockaddr_of(t: &[u8]) -> Sa {
    let mut b = [0u8; 28];
    if t.len() >= 34 && u16::from_le_bytes([t[6], t[7]]) == AF_INET6 as u16 {
        b[..2].copy_from_slice(&(AF_INET6 as u16).to_le_bytes());
        b[2..4].copy_from_slice(&t[8..10]);
        b[4..28].copy_from_slice(&t[10..34]);
        Sa { b, len: 28 }
    } else {
        b[..2].copy_from_slice(&2u16.to_le_bytes());
        b[2..8].copy_from_slice(&t[8..14]);
        Sa { b, len: 16 }
    }
}

/// The address family a socket was made with (AF_INET unless AF_INET6).
fn family_of(s: Socket) -> i32 {
    options::get(s, options::FAMILY)
}

/// The smallest address buffer a socket's calls take.
fn min_len_of(s: Socket) -> i32 {
    if family_of(s) == AF_INET6 { SOCKADDR6_LEN } else { SOCKADDR_LEN }
}

/// Opens a TCP (or, with `udp`, a UDP) endpoint: NtCreateFile of
/// `\Device\Afd\Endpoint` with the open packet (ReactOS's
/// AFD_CREATE_PACKET, transport `\Device\Tcp` or `\Device\Udp`) as the
/// extended attribute `AfdOpenPacketXX`.
fn open_endpoint_of(udp: bool) -> Result<Socket, u32> {
    open_endpoint_family(udp, AF_INET)
}

/// The same for `family` (AF_INET6: Windows' `\Device\Tcp6` and
/// `\Device\Udp6`).
fn open_endpoint_family(udp: bool, family: i32) -> Result<Socket, u32> {
    const NAME: &[u8] = b"AfdOpenPacketXX";
    let transport = match (udp, family == AF_INET6) {
        (false, false) => "\\Device\\Tcp",
        (true, false) => "\\Device\\Udp",
        (false, true) => "\\Device\\Tcp6",
        (true, true) => "\\Device\\Udp6",
    };
    let name_bytes = 2 * transport.len();
    // FILE_FULL_EA_INFORMATION header, name + NUL, then the packet.
    let mut ea = [0u8; 8 + 16 + 12 + 24];
    let ea = &mut ea[..8 + 16 + 12 + name_bytes];
    ea[5] = NAME.len() as u8;
    ea[6..8].copy_from_slice(&(12 + name_bytes as u16).to_le_bytes());
    ea[8..8 + NAME.len()].copy_from_slice(NAME);
    let packet = 8 + NAME.len() + 1;
    ea[packet + 8..packet + 12].copy_from_slice(&(name_bytes as u32).to_le_bytes());
    for (i, c) in transport.encode_utf16().enumerate() {
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

/// Whether `s` is a datagram socket (its SO_TYPE, kept per socket).
fn is_datagram(s: Socket) -> bool {
    options::get(s, options::TYPE) == SOCK_DGRAM
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
    let mut w = layout::WsaData {
        version: granted,
        high_version: 0x0202,
        max_sockets: 0,
        max_udp_dg: 0,
        vendor_info: core::ptr::null_mut(),
        description: [0; 257],
        system_status: [0; 129],
    };
    w.description[..11].copy_from_slice(b"WinSock 2.0");
    w.system_status[..7].copy_from_slice(b"Running");
    // SAFETY: the caller passes a WSADATA (layout::WsaData's size).
    unsafe { data.cast::<layout::WsaData>().write_unaligned(w) };
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

/// socket(af, type, protocol): IPv4 TCP and UDP.
#[unsafe(no_mangle)]
pub extern "system" fn socket(af: i32, kind: i32, protocol: i32) -> Socket {
    if !started() {
        fail(WSANOTINITIALISED);
        return INVALID_SOCKET;
    }
    if af != AF_INET && af != AF_INET6 {
        missing(b"ws2_32.dll!socket(address family)", af as u64);
        fail(WSAEAFNOSUPPORT);
        return INVALID_SOCKET;
    }
    let udp = match (kind, protocol) {
        (SOCK_STREAM, 0 | IPPROTO_TCP) => false,
        (SOCK_DGRAM, 0 | IPPROTO_UDP) => true,
        (SOCK_STREAM | SOCK_DGRAM, _) => {
            fail(WSAEPROTONOSUPPORT);
            return INVALID_SOCKET;
        }
        _ => {
            missing(b"ws2_32.dll!socket(type)", kind as u64);
            fail(WSAESOCKTNOSUPPORT);
            return INVALID_SOCKET;
        }
    };
    let s = match open_endpoint_family(udp, af) {
        Ok(s) => s,
        Err(status) => {
            fail(wsa_error(status));
            return INVALID_SOCKET;
        }
    };
    // A handle value is reused once closed: forget what an earlier socket
    // of this value kept, then record the family and the type.
    options::forget(s);
    if af == AF_INET6 && !options::set(s, options::FAMILY, AF_INET6) {
        // SAFETY: closes the endpoint opened above.
        unsafe { NtClose(s) };
        fail(WSAENOBUFS);
        return INVALID_SOCKET;
    }
    if udp && !options::set(s, options::TYPE, SOCK_DGRAM) {
        // SAFETY: closes the endpoint opened above.
        unsafe { NtClose(s) };
        fail(WSAENOBUFS);
        return INVALID_SOCKET;
    }
    s
}

#[unsafe(no_mangle)]
pub extern "system" fn bind(s: Socket, name: *const u8, len: i32) -> i32 {
    let sa = match read_sockaddr(name, len) {
        Ok(sa) => sa,
        Err(e) => return e,
    };
    // {u32 share access; SOCKADDR}; the bound address comes back.
    let mut input = [0u8; 4 + 28];
    input[4..4 + sa.len].copy_from_slice(sa.bytes());
    let mut bound = [0u8; 28];
    match afd_call(s, IOCTL_AFD_BIND, &input[..4 + sa.len], &mut bound) {
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
    let mut pending = [0u8; 4 + 34];
    if afd_call(s, IOCTL_AFD_WAIT_FOR_LISTEN, &[], &mut pending).is_err() {
        return INVALID_SOCKET;
    }
    // A connection takes the listener's family.
    let family = family_of(s);
    let new = match open_endpoint_family(false, family) {
        Ok(h) => h,
        Err(status) => {
            fail(wsa_error(status));
            return INVALID_SOCKET;
        }
    };
    options::forget(new);
    if family == AF_INET6 && !options::set(new, options::FAMILY, AF_INET6) {
        // SAFETY: closes the endpoint opened above.
        unsafe { NtClose(new) };
        fail(WSAENOBUFS);
        return INVALID_SOCKET;
    }
    // AFD_ACCEPT_DATA {u32 use_san; u32 sequence; HANDLE accept endpoint}.
    let mut input = [0u8; 16];
    input[afd::Layout::ACCEPT_SEQUENCE..][..4].copy_from_slice(&pending[..4]);
    afd::L.put_ptr(&mut input, afd::Layout::ACCEPT_HANDLE, new as u64);
    if afd_call(s, IOCTL_AFD_ACCEPT, &input[..afd::L.accept_size()], &mut []).is_err() {
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
    // A datagram socket given a zeroed address (AF_UNSPEC included, as
    // libuv disconnects) loses its default peer, as documented.
    let unspec = is_datagram(s)
        && !name.is_null()
        && len >= SOCKADDR_LEN
        // SAFETY: the caller's buffer holds at least 16 bytes.
        && unsafe { core::slice::from_raw_parts(name, 16) }.iter().all(|&b| b == 0);
    let sa = if unspec { Ok(Sa::any(family_of(s))) } else { read_sockaddr(name, len) };
    let sa = match sa {
        Ok(sa) => sa,
        Err(e) => return e,
    };
    // {u8 use_san; u32 root; u32 unknown; TRANSPORT_ADDRESS}.
    let (t, n) = transport_address(&sa);
    let mut input = [0u8; 12 + 34];
    input[12..12 + n].copy_from_slice(&t[..n]);
    match afd_call(s, IOCTL_AFD_CONNECT, &input[..12 + n], &mut []) {
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
    // A WSABUF, then {WSABUF *; u32 count; u32 afd flags; u32 message
    // flags (AFD_MSG_PEEK)}.
    let wsabuf = afd::wsabuf(len as u32, buf as usize);
    let info = afd::info(wsabuf.as_ptr() as usize, 1, if peek { AFD_MSG_PEEK } else { 0 });
    match afd_call(s, code, &info[..afd::L.info_size()], &mut []) {
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
    let mut b = [0u8; 28];
    let n = match afd_call(s, code, &[], &mut b) {
        Ok(n) => n,
        Err(e) => return e,
    };
    let sa = Sa { b, len: if n >= 28 { 28 } else { 16 } };
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
    options::forget(s);
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
    if group != 0 {
        missing(b"ws2_32.dll!WSASocketW(group)", group.into());
    }
    if !info.is_null() {
        // SAFETY: the caller's WSAPROTOCOL_INFOW.
        let field = |at: usize| unsafe { info.add(at).cast::<i32>().read_unaligned() };
        let reserved = field(INFO_PROVIDER_RESERVED) as u32 as usize;
        if af == FROM_PROTOCOL_INFO && kind == FROM_PROTOCOL_INFO && reserved != 0 {
            // From WSADuplicateSocketW: the handle it made for this process.
            if field(INFO_TYPE) == SOCK_DGRAM {
                options::set(reserved, options::TYPE, SOCK_DGRAM);
            }
            return reserved;
        }
        let pick =
            |given: i32, at: usize| if given == FROM_PROTOCOL_INFO { field(at) } else { given };
        return socket(pick(af, INFO_FAMILY), pick(kind, INFO_TYPE), pick(protocol, INFO_PROTOCOL));
    }
    socket(af, kind, protocol)
}

/// WSAPROTOCOL_INFOW's iAddressFamily, iSocketType, iProtocol,
/// dwProviderReserved and its size; FROM_PROTOCOL_INFO.
const INFO_FAMILY: usize = 76;
const INFO_TYPE: usize = 88;
const INFO_PROTOCOL: usize = 92;
const INFO_PROVIDER_RESERVED: usize = 112;
const INFO_SIZE: usize = 628;
const FROM_PROTOCOL_INFO: i32 = -1;

/// WSADuplicateSocketW(s, dwProcessId, lpProtocolInfo): the socket's
/// handle duplicated into the process (dwProviderReserved, as the base
/// provider keeps it) for that process's WSASocketW(FROM_PROTOCOL_INFO).
#[unsafe(no_mangle)]
pub extern "system" fn WSADuplicateSocketW(s: Socket, process: u32, info: *mut u8) -> i32 {
    duplicate_socket(s, process, info, INFO_SIZE)
}

/// WSADuplicateSocketA: WSADuplicateSocketW with WSAPROTOCOL_INFOA (the same
/// fields, an ANSI szProtocol: 372 bytes).
#[unsafe(no_mangle)]
pub extern "system" fn WSADuplicateSocketA(s: Socket, process: u32, info: *mut u8) -> i32 {
    duplicate_socket(s, process, info, INFO_SIZE_A)
}

/// WSAPROTOCOL_INFOA's size.
const INFO_SIZE_A: usize = 372;

fn duplicate_socket(s: Socket, process: u32, info: *mut u8, info_size: usize) -> i32 {
    const PROCESS_DUP_HANDLE: u32 = 0x40;
    const DUPLICATE_SAME_ACCESS: u32 = 2;
    if !started() {
        return fail(WSANOTINITIALISED);
    }
    if info.is_null() {
        return fail(WSAEFAULT);
    }
    let attributes = ObjectAttributes {
        length: size_of::<ObjectAttributes>() as u32,
        root_directory: 0,
        object_name: core::ptr::null(),
        attributes: 0,
        security_descriptor: 0,
        security_quality_of_service: 0,
    };
    let client = [process as usize, 0];
    let (mut target, mut h) = (0usize, 0usize);
    // SAFETY: our structures and outputs.
    if unsafe { NtOpenProcess(&mut target, PROCESS_DUP_HANDLE, &attributes, &client) } != 0 {
        return fail(WSAEINVAL);
    }
    // SAFETY: plain handle calls; our output.
    let status = unsafe {
        let status = NtDuplicateObject(-1, s, target, &mut h, 0, 0, DUPLICATE_SAME_ACCESS);
        NtClose(target);
        status
    };
    if status != 0 {
        return fail(WSAENOTSOCK);
    }
    // SAFETY: the caller's WSAPROTOCOL_INFOW (INFO_SIZE bytes).
    unsafe {
        memset(info, 0, info_size);
        let put = |at: usize, v: i32| info.add(at).cast::<i32>().write_unaligned(v);
        let datagram = is_datagram(s);
        put(INFO_FAMILY, family_of(s));
        put(INFO_TYPE, if datagram { SOCK_DGRAM } else { SOCK_STREAM });
        put(INFO_PROTOCOL, if datagram { IPPROTO_UDP } else { IPPROTO_TCP });
        put(INFO_PROVIDER_RESERVED, h as i32);
    }
    0
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
    let info = afd::info(buffers as usize, count, 0);
    let info = &info[..afd::L.info_size()];
    if ov.is_null() {
        return match afd(s, code, info, &mut []) {
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
    // A completion routine needs an OVERLAPPED; without one it is ignored.
    let status = afd_overlapped_apc(s, code, info, core::ptr::null_mut(), 0, ov, routine);
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
    let l = afd::L;
    let mut info = [0u8; 28];
    l.put_ptr(&mut info, l.super_accept_handle(), accept as u64);
    info[l.super_accept_receive()..][..4].copy_from_slice(&receive.to_le_bytes());
    info[l.super_accept_local()..][..4].copy_from_slice(&local.to_le_bytes());
    info[l.super_accept_remote()..][..4].copy_from_slice(&remote.to_le_bytes());
    let info = &info[..l.super_accept_size()];
    let total = receive.saturating_add(local).saturating_add(remote);
    let status = afd_overlapped(listener, IOCTL_AFD_SUPER_ACCEPT, info, output, total, ov);
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
        // Each block starts with the address AcceptEx stored: its family
        // says its length.
        let len_of = |p: *const u8| {
            if u16::from_le_bytes([*p, *p.add(1)]) == AF_INET6 as u16 {
                SOCKADDR6_LEN
            } else {
                SOCKADDR_LEN
            }
        };
        let at = output.add(receive as usize);
        *local_addr = at;
        *local_len = len_of(at);
        *remote_addr = at.add(local as usize);
        *remote_len = len_of(at.add(local as usize));
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
    let (t, n) = transport_address(&sa);
    let mut info = [0u8; 4 + 34];
    info[4..4 + n].copy_from_slice(&t[..n]);
    let status =
        afd_overlapped(s, IOCTL_AFD_SUPER_CONNECT, &info[..4 + n], core::ptr::null_mut(), 0, ov);
    if status == 0 && !sent.is_null() {
        // SAFETY: the caller's count.
        unsafe { *sent = 0 };
    }
    i32::from(overlapped_result(status) == 0)
}

/// DisconnectEx (handed out by WSAIoctl): a graceful close of both
/// directions (IOCTL_AFD_DISCONNECT, as shutdown(SD_BOTH)), overlapped when
/// given an OVERLAPPED (completing through its event or the socket's
/// completion port), else synchronous. dwReserved must be 0 (WSAEINVAL).
/// TF_REUSE_SOCKET (the socket used again by AcceptEx or ConnectEx) is not
/// supported: WSAEOPNOTSUPP, recorded.
extern "system" fn disconnect_ex(s: Socket, ov: *mut Overlapped, flags: u32, reserved: u32) -> i32 {
    if reserved != 0 {
        fail(WSAEINVAL);
        return 0;
    }
    if flags & TF_REUSE_SOCKET != 0 {
        missing(b"ws2_32.dll!DisconnectEx(TF_REUSE_SOCKET)", flags.into());
        fail(WSAEOPNOTSUPP);
        return 0;
    }
    // {u32 type (3: send and receive); LARGE_INTEGER timeout (-1: none)}.
    let mut input = [0u8; 16];
    input[..4].copy_from_slice(&3u32.to_le_bytes());
    input[8..].copy_from_slice(&(-1i64).to_le_bytes());
    if ov.is_null() {
        return i32::from(afd_call(s, IOCTL_AFD_DISCONNECT, &input, &mut []).is_ok());
    }
    let status = afd_overlapped(s, IOCTL_AFD_DISCONNECT, &input, core::ptr::null_mut(), 0, ov);
    i32::from(overlapped_result(status) == 0)
}

/// TransmitFile (exported here, forwarded from mswsock, handed out by
/// WSAIoctl): not supported (WSAEOPNOTSUPP, recorded); callers such as the
/// JDK and asyncio's sendfile fall back to reading the file and sending it.
#[unsafe(no_mangle)]
pub extern "system" fn TransmitFile(
    _socket: Socket,
    _file: usize,
    _bytes: u32,
    _per_send: u32,
    _overlapped: *mut Overlapped,
    _buffers: usize,
    _flags: u32,
) -> i32 {
    missing(b"ws2_32.dll!TransmitFile", 0);
    fail(WSAEOPNOTSUPP);
    0
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
const WSAID_DISCONNECTEX: [u8; 16] =
    guid(0x7fda_2e11, 0x8630, 0x436f, [0xa0, 0x31, 0xf5, 0x36, 0xa6, 0xee, 0xc1, 0x57]);
const WSAID_TRANSMITFILE: [u8; 16] =
    guid(0xb536_7df0, 0xcbac, 0x11cf, [0x95, 0xca, 0x00, 0x80, 0x5f, 0x48, 0xa1, 0x92]);
/// DisconnectEx's flag to make the socket reusable by AcceptEx or
/// ConnectEx (mswsock.h).
const TF_REUSE_SOCKET: u32 = 0x02;

// SIO_BASE_HANDLE and SIO_BSP_HANDLE(_SELECT/_POLL) (mswsock.h, ws2def.h).
const SIO_BASE_HANDLE: u32 = 0x4800_0022;
const SIO_BSP_HANDLE: u32 = 0x4800_001b;
const SIO_BSP_HANDLE_SELECT: u32 = 0x4800_001c;
const SIO_BSP_HANDLE_POLL: u32 = 0x4800_001d;

const fn guid(a: u32, b: u16, c: u16, d: [u8; 8]) -> [u8; 16] {
    let (a, b, c) = (a.to_le_bytes(), b.to_le_bytes(), c.to_le_bytes());
    [a[0], a[1], a[2], a[3], b[0], b[1], c[0], c[1], d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7]]
}

/// WSAIoctl(s, dwIoControlCode, lpvInBuffer, cbInBuffer, lpvOutBuffer,
/// cbOutBuffer, lpcbBytesReturned, lpOverlapped, lpCompletionRoutine):
/// the extension function pointers; the base provider's handle
/// (SIO_BASE_HANDLE, SIO_BSP_HANDLE*: the socket itself, as there are no
/// layered providers; wepoll asks for it); keepalive and initial-RTO
/// tuning are accepted without effect.
#[unsafe(no_mangle)]
pub extern "system" fn WSAIoctl(
    s: Socket,
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
                WSAID_DISCONNECTEX => disconnect_ex as *const () as usize,
                WSAID_TRANSMITFILE => TransmitFile as *const () as usize,
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
        SIO_BASE_HANDLE | SIO_BSP_HANDLE | SIO_BSP_HANDLE_SELECT | SIO_BSP_HANDLE_POLL => {
            if output.is_null() || output_len < 8 {
                return fail(WSAEFAULT);
            }
            // SAFETY: the caller's 8-byte output buffer and count.
            unsafe {
                core::ptr::write_unaligned(output.cast::<Socket>(), s);
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
const IPPROTO_IP: i32 = 0;

/// The integer socket options setsockopt keeps per socket, for getsockopt.
mod options {
    use core::sync::atomic::{AtomicBool, Ordering};

    use super::{IPPROTO_TCP, SOL_SOCKET, Socket};

    /// (level, name, Windows' default): SO_REUSEADDR, SO_KEEPALIVE,
    /// SO_SNDBUF and SO_RCVBUF (gala's 64 KiB), SO_EXCLUSIVEADDRUSE,
    /// TCP_NODELAY, TCP_KEEPALIVE (idle, 2 h), TCP_KEEPCNT (10),
    /// TCP_KEEPINTVL (1 s), SO_BROADCAST; SO_TYPE (stream unless socket
    /// made a datagram socket; read-only to setsockopt); the address family
    /// (gala's own, not an option: `family_of`); IPV6_V6ONLY (1, Windows'
    /// default: a v6 socket takes only v6; dual stack is N3 step 2);
    /// SO_RANDOMIZE_PORT,
    /// IP_DONTFRAGMENT and IP_RECVECN (kept only: ephemeral ports are
    /// chosen and datagrams sent as without them, and receives carry no
    /// ECN control data).
    const OPTIONS: [(i32, i32, i32); 16] = [
        (SOL_SOCKET, 0x4, 0),
        (SOL_SOCKET, 0x8, 0),
        (SOL_SOCKET, 0x1001, 65536),
        (SOL_SOCKET, 0x1002, 65536),
        (SOL_SOCKET, -5, 0),
        (IPPROTO_TCP, 1, 0),
        (IPPROTO_TCP, 3, 7200),
        (IPPROTO_TCP, 16, 10),
        (IPPROTO_TCP, 17, 1),
        (SOL_SOCKET, 0x20, 0),
        (SOL_SOCKET, 0x1008, super::SOCK_STREAM),
        (SOL_SOCKET, 0x3005, 0),
        (super::IPPROTO_IP, 14, 0),
        (super::IPPROTO_IP, 50, 0),
        (FAMILY_LEVEL, 0, super::AF_INET),
        (super::IPPROTO_IPV6, 27, 1),
    ];
    /// The family's slot: a level no option uses.
    const FAMILY_LEVEL: i32 = 0x7fff_0000;
    pub const FAMILY: usize = 14;
    /// SO_TYPE's slot.
    pub const TYPE: usize = 10;
    const SOCKETS: usize = 256;

    struct Table {
        sockets: [Socket; SOCKETS],
        values: [[i32; OPTIONS.len()]; SOCKETS],
    }

    static LOCK: AtomicBool = AtomicBool::new(false);
    static mut TABLE: Table =
        Table { sockets: [0; SOCKETS], values: [[0; OPTIONS.len()]; SOCKETS] };

    fn with<R>(f: impl FnOnce(&mut Table) -> R) -> R {
        while LOCK.compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed).is_err()
        {
            core::hint::spin_loop();
        }
        // SAFETY: LOCK gives this thread the table to itself.
        let r = f(unsafe { &mut *core::ptr::addr_of_mut!(TABLE) });
        LOCK.store(false, Ordering::Release);
        r
    }

    /// The option's index, if it is one that is kept.
    pub fn slot(level: i32, name: i32) -> Option<usize> {
        OPTIONS.iter().position(|&(l, n, _)| l == level && n == name)
    }

    pub fn get(s: Socket, slot: usize) -> i32 {
        with(|t| match t.sockets.iter().position(|&x| x == s) {
            Some(i) => t.values[i][slot],
            None => OPTIONS[slot].2,
        })
    }

    /// Records a value; false if the table is full.
    pub fn set(s: Socket, slot: usize, value: i32) -> bool {
        with(|t| {
            let i = match t.sockets.iter().position(|&x| x == s) {
                Some(i) => i,
                None => {
                    let Some(i) = t.sockets.iter().position(|&x| x == 0) else { return false };
                    t.sockets[i] = s;
                    for (v, o) in t.values[i].iter_mut().zip(OPTIONS) {
                        *v = o.2;
                    }
                    i
                }
            };
            t.values[i][slot] = value;
            true
        })
    }

    /// The socket is closed.
    pub fn forget(s: Socket) {
        with(|t| {
            if let Some(i) = t.sockets.iter().position(|&x| x == s) {
                t.sockets[i] = 0;
            }
        });
    }
}

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
    // SO_RANDOMIZE_PORT picks the port bind will take: refused once the
    // socket is bound (WSAEINVAL; Windows 11 25H2, a UDP socket).
    if (level, name) == (SOL_SOCKET, 0x3005) {
        let mut local = [0u8; 28];
        if afd(s, IOCTL_AFD_GETSOCKNAME, &[], &mut local).is_ok() {
            return fail(WSAEINVAL);
        }
    }
    let accepted = match (level, name) {
        // SO_REUSEADDR, SO_KEEPALIVE, SO_BROADCAST, SO_LINGER, SO_SNDBUF,
        // SO_RCVBUF, SO_EXCLUSIVEADDRUSE, SO_UPDATE_ACCEPT_CONTEXT,
        // SO_UPDATE_CONNECT_CONTEXT, SO_RANDOMIZE_PORT (kept only).
        (SOL_SOCKET, 0x4 | 0x8 | 0x20 | 0x80 | 0x1001 | 0x1002 | -5 | 0x700b | 0x7010 | 0x3005) => {
            true
        }
        // IP_DONTFRAGMENT, IP_RECVECN (kept only).
        (IPPROTO_IP, 14 | 50) => true,
        // IPV6_V6ONLY: kept; 0 (dual stack) is N3 step 2.
        (IPPROTO_IPV6, 27) => true,
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
    // Integer options are kept for getsockopt (a BOOL may come as 1 byte).
    if let Some(slot) = options::slot(level, name)
        && len >= 1
    {
        let mut b = [0u8; 4];
        let n = (len as usize).min(4);
        // SAFETY: the caller's value of `len` bytes.
        unsafe { core::ptr::copy_nonoverlapping(value, b.as_mut_ptr(), n) };
        if !options::set(s, slot, i32::from_le_bytes(b)) {
            return fail(WSAENOBUFS);
        }
        // SO_RCVBUF and SO_SNDBUF size the buffers of the connections the
        // socket makes from then on (AFD_INFO_RECEIVE_WINDOW_SIZE and
        // AFD_INFO_SEND_WINDOW_SIZE, ReactOS's numbers).
        if level == SOL_SOCKET && (name == 0x1001 || name == 0x1002) {
            let mut info = [0u8; 16];
            let class: u32 = if name == 0x1002 { 6 } else { 7 };
            info[..4].copy_from_slice(&class.to_le_bytes());
            info[8..12].copy_from_slice(&b);
            if let Err(e) = afd_call(s, IOCTL_AFD_SET_INFO, &info, &mut []) {
                return e;
            }
        }
        // IPV6_V6ONLY decides which families the socket takes: AFD binds
        // by it (gala's own AFD_INFO class).
        if (level, name) == (IPPROTO_IPV6, 27) && family_of(s) == AF_INET6 {
            let mut info = [0u8; 16];
            info[..4].copy_from_slice(&GALA_AFD_INFO_V6ONLY.to_le_bytes());
            info[8] = u8::from(i32::from_le_bytes(b) != 0);
            if let Err(e) = afd_call(s, IOCTL_AFD_SET_INFO, &info, &mut []) {
                return e;
            }
        }
    }
    0
}

/// SO_ERROR: a connect that failed (a non-blocking one, reported by
/// select's except set) is WSAECONNREFUSED; otherwise 0. Asked of AFD
/// with a poll that does not wait (AFD_POLL_CONNECT_FAIL). The error is
/// kept while the socket stays failed (Windows clears it on read; a
/// failed socket is only closed after, as Python and libuv do).
fn socket_error(s: Socket) -> i32 {
    const AFD_POLL_CONNECT_FAIL: u32 = 0x100;
    let mut input = [0u8; 16 + 16];
    afd::put_poll(&mut input, 0, s, AFD_POLL_CONNECT_FAIL);
    input[..8].copy_from_slice(&0i64.to_le_bytes());
    input[8..12].copy_from_slice(&1u32.to_le_bytes());
    let mut output = [0u8; 16 + 16];
    if poll(s, &input[..afd::poll_len(1)], &mut output, Some(0)) != 0 {
        return 0;
    }
    let ready = u32::from_le_bytes([output[8], output[9], output[10], output[11]]);
    if ready != 0 && afd::get_poll(&output, 0).1 & AFD_POLL_CONNECT_FAIL != 0 {
        WSAECONNREFUSED as i32
    } else {
        0
    }
}

/// getsockopt: SO_ERROR (`socket_error`), SO_TYPE (stream or datagram),
/// and the integer options setsockopt keeps (their Windows defaults until
/// set).
#[unsafe(no_mangle)]
pub extern "system" fn getsockopt(
    s: Socket,
    level: i32,
    name: i32,
    value: *mut u8,
    len: *mut i32,
) -> i32 {
    let v: i32 = match (level, name) {
        (SOL_SOCKET, 0x1007) => socket_error(s), // SO_ERROR
        _ if options::slot(level, name).is_some() => {
            options::get(s, options::slot(level, name).unwrap_or(0))
        }
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
        // SO_RANDOMIZE_PORT is a 1-byte BOOLEAN: only its byte is written
        // (Windows 11 25H2: an int set to -1 reads 0xffffff01). The length
        // it reports is not measured; 1, the option's size, is given.
        if (level, name) == (SOL_SOCKET, 0x3005) {
            *value = u8::from(v != 0);
            *len = 1;
            return 0;
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

/// WSAPOLLFD.
#[repr(C)]
pub struct PollFd {
    fd: Socket,
    events: i16,
    revents: i16,
}

const POLLRDNORM: i16 = 0x100;
const POLLRDBAND: i16 = 0x200;
const POLLWRNORM: i16 = 0x10;
const POLLERR: i16 = 0x1;
const POLLHUP: i16 = 0x2;
const POLLNVAL: i16 = 0x4;

/// WSAPoll(fdarray, nfds, timeout): one IOCTL_AFD_POLL over the sockets
/// (a negative timeout waits for ever); revents as poll's: POLLRDNORM for
/// data or a connection to accept, POLLRDBAND for out-of-band data,
/// POLLWRNORM when writable, POLLHUP on a disconnect, POLLERR on a failed
/// connect; returns how many entries have events. Entries with a negative
/// socket are skipped.
#[unsafe(no_mangle)]
pub extern "system" fn WSAPoll(fds: *mut PollFd, count: u32, timeout: i32) -> i32 {
    if !started() {
        return fail(WSANOTINITIALISED);
    }
    if fds.is_null() || count as usize > MAX_SELECT {
        return fail(WSAEINVAL);
    }
    // SAFETY: the caller's array of `count` entries.
    let fds = unsafe { core::slice::from_raw_parts_mut(fds, count as usize) };
    let mut input = [0u8; 16 + 16 * MAX_SELECT];
    let mut n = 0usize;
    for f in fds.iter_mut() {
        f.revents = 0;
        if (f.fd as isize) < 0 {
            continue;
        }
        let mut flags = 0x8 | 0x10 | 0x100; // DISCONNECT, ABORT, CONNECT_FAIL: always
        if f.events & POLLRDNORM != 0 {
            flags |= 0x1 | 0x80; // RECEIVE, ACCEPT
        }
        if f.events & POLLRDBAND != 0 {
            flags |= 0x2; // RECEIVE_EXPEDITED
        }
        if f.events & POLLWRNORM != 0 {
            flags |= 0x4 | 0x40; // SEND, CONNECT
        }
        afd::put_poll(&mut input, n, f.fd, flags as u32);
        n += 1;
    }
    if n == 0 {
        return fail(WSAEINVAL);
    }
    let ms = (timeout >= 0).then_some(i64::from(timeout));
    let poll_timeout: i64 = if timeout == 0 { 0 } else { i64::MAX };
    input[..8].copy_from_slice(&poll_timeout.to_le_bytes());
    input[8..12].copy_from_slice(&(n as u32).to_le_bytes());
    let first = afd::get_poll(&input, 0).0;
    let mut output = [0u8; 16 + 16 * MAX_SELECT];
    let ready = match poll(first, &input[..afd::poll_len(n)], &mut output, ms) {
        0 => u32::from_le_bytes([output[8], output[9], output[10], output[11]]) as usize,
        0xc000_0120 => 0, // timed out and cancelled
        0xc000_0008 => {
            // A handle that is not a socket.
            for f in fds.iter_mut().filter(|f| (f.fd as isize) >= 0) {
                f.revents = POLLNVAL;
            }
            return fds.len() as i32;
        }
        s => return fail(wsa_error(s)),
    };
    for e in 0..ready.min(MAX_SELECT) {
        let (sock, events) = afd::get_poll(&output, e);
        for f in fds.iter_mut().filter(|f| f.fd == sock) {
            let mut r = 0;
            if events & (0x1 | 0x80) != 0 {
                r |= POLLRDNORM;
            }
            if events & 0x2 != 0 {
                r |= POLLRDBAND;
            }
            if events & (0x4 | 0x40) != 0 {
                r |= POLLWRNORM;
            }
            if events & (0x8 | 0x10) != 0 {
                r |= POLLHUP;
            }
            if events & 0x100 != 0 {
                r |= POLLERR;
            }
            // Only what was asked for, besides the error conditions.
            f.revents = r & (f.events | POLLHUP | POLLERR | POLLNVAL);
        }
    }
    fds.iter().filter(|f| f.revents != 0).count() as i32
}

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
    // status} entries (afd_abi).
    let mut input = [0u8; 16 + 16 * MAX_SELECT];
    let mut n = 0usize;
    for (set, flags) in [(read, POLL_READ), (write, POLL_WRITE), (except, POLL_EXCEPT)] {
        for &sock in fd_slice(set) {
            let existing = (0..n).find(|&i| afd::get_poll(&input, i).0 == sock);
            let i = match existing {
                Some(i) => i,
                None if n < MAX_SELECT => {
                    afd::put_poll(&mut input, n, sock, 0);
                    n += 1;
                    n - 1
                }
                None => return fail(WSAEINVAL),
            };
            let old = afd::get_poll(&input, i).1;
            afd::put_poll(&mut input, i, sock, old | flags);
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
    let first = afd::get_poll(&input, 0).0;
    let mut output = [0u8; 16 + 16 * MAX_SELECT];
    let status = poll(first, &input[..afd::poll_len(n)], &mut output, ms);
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
            let (sock, events) = afd::get_poll(&output, e);
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

/// The machine's host name (gethostname).
const HOST_NAME: &[u8] = b"gala";

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
/// "localhost", the machine's own host name (gethostname's; loopback, as
/// Windows answers without network adapters: gala does not list its
/// adapters yet) and a missing node (any address, or loopback) with a
/// numeric service; no other name resolution yet (reported). One result, in a
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
        if family != 0 && family != AF_INET && family != AF_INET6 {
            return WSAEAFNOSUPPORT as i32;
        }
        // IPv6 answers: AF_INET6 hints (numeric addresses, loopback names),
        // or an IPv6 numeric address with any hints. AF_UNSPEC names stay
        // IPv4 (N3 step 2: both, ::1 first as Windows lists them).
        let text = if node.is_null() {
            None
        } else {
            Some(core::slice::from_raw_parts(node, strlen(node)))
        };
        // AF_UNSPEC "localhost" (and the host name): ::1, then 127.0.0.1,
        // as Windows lists them.
        let both = family == 0
            && text.is_some_and(|t| {
                t.eq_ignore_ascii_case(b"localhost") || t.eq_ignore_ascii_case(HOST_NAME)
            });
        let v6 = match text {
            _ if both => Some(LOOPBACK6),
            None if family == AF_INET6 => Some(if flags & 1 != 0 { [0u8; 16] } else { LOOPBACK6 }),
            Some(t) if family == AF_INET6 || family == 0 => match parse_ipv6(t) {
                Some(a) => Some(a),
                None if family == AF_INET6 => {
                    if t.eq_ignore_ascii_case(b"localhost") || t.eq_ignore_ascii_case(HOST_NAME) {
                        Some(LOOPBACK6)
                    } else if flags & AI_NUMERICHOST != 0 {
                        return WSAHOST_NOT_FOUND;
                    } else {
                        // No AAAA queries yet (N3 step 3).
                        missing(b"ws2_32.dll!getaddrinfo(AF_INET6 name)", 0);
                        return WSAHOST_NOT_FOUND;
                    }
                }
                None => None,
            },
            _ => None,
        };
        let ip = if v6.is_some() {
            [0; 4]
        } else if node.is_null() {
            if flags & 1 != 0 { [0, 0, 0, 0] } else { [127, 0, 0, 1] } // AI_PASSIVE
        } else {
            let text = core::slice::from_raw_parts(node, strlen(node));
            if text.eq_ignore_ascii_case(b"localhost") || text.eq_ignore_ascii_case(HOST_NAME) {
                [127, 0, 0, 1]
            } else if let Some(ip) = parse_ipv4(text) {
                ip
            } else if flags & AI_NUMERICHOST != 0 {
                return WSAHOST_NOT_FOUND;
            } else if let Some(ip) = hosts::lookup(text) {
                ip
            } else {
                match dns::resolve(text) {
                    Ok(ip) => ip,
                    Err(e) => return e,
                }
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
        // An ADDRINFOA, its SOCKADDR_IN(6) after it (48 bytes on); a
        // second result (AF_UNSPEC localhost's 127.0.0.1) at 128.
        let fill = |at: *mut u8, v6: Option<[u8; 16]>, ip: [u8; 4]| {
            let sa = at.add(48);
            at.cast::<layout::AddrInfo>().write(layout::AddrInfo {
                flags,
                family: if v6.is_some() { AF_INET6 } else { AF_INET },
                socktype,
                protocol,
                addrlen: if v6.is_some() { SOCKADDR6_LEN } else { SOCKADDR_LEN } as usize,
                canonname: core::ptr::null_mut(),
                addr: sa,
                next: core::ptr::null_mut(),
            });
            core::ptr::copy_nonoverlapping(port.to_be_bytes().as_ptr(), sa.add(2), 2);
            if let Some(a) = v6 {
                *sa = AF_INET6 as u8;
                core::ptr::copy_nonoverlapping(a.as_ptr(), sa.add(8), 16);
            } else {
                *sa = AF_INET as u8;
                core::ptr::copy_nonoverlapping(ip.as_ptr(), sa.add(4), 4);
            }
        };
        fill(page, v6, ip);
        if both {
            let second = page.add(128);
            fill(second, None, [127, 0, 0, 1]);
            (*page.cast::<layout::AddrInfo>()).next = second.cast();
        }
        *result = page;
    }
    0
}

/// GetAddrInfoW: getaddrinfo with UTF-16 strings (names are ASCII here;
/// ADDRINFOW has ADDRINFOA's layout, and gala returns no canonical name).
#[unsafe(no_mangle)]
pub extern "system" fn GetAddrInfoW(
    node: *const u16,
    service: *const u16,
    hints: *const i32,
    result: *mut *mut u8,
) -> i32 {
    fn narrow(w: *const u16, out: &mut [u8; 256]) -> Option<*const u8> {
        if w.is_null() {
            return Some(core::ptr::null());
        }
        let mut i = 0;
        // SAFETY: the caller's NUL-terminated string.
        unsafe {
            while *w.add(i) != 0 {
                let c = *w.add(i);
                if c > 0x7f || i == out.len() - 1 {
                    return None;
                }
                out[i] = c as u8;
                i += 1;
            }
        }
        out[i] = 0;
        Some(out.as_ptr())
    }
    let (mut n, mut s) = ([0u8; 256], [0u8; 256]);
    let (Some(node), Some(service)) = (narrow(node, &mut n), narrow(service, &mut s)) else {
        missing(b"ws2_32.dll!GetAddrInfoW(non-ASCII name)", 0);
        return WSAHOST_NOT_FOUND;
    };
    getaddrinfo(node, service, hints, result)
}

#[unsafe(no_mangle)]
pub extern "system" fn FreeAddrInfoW(ai: *mut u8) {
    freeaddrinfo(ai);
}

#[unsafe(no_mangle)]
pub extern "system" fn freeaddrinfo(ai: *mut u8) {
    if !ai.is_null() {
        // SAFETY: a page from getaddrinfo.
        unsafe { VirtualFree(ai, 0, 0x8000) };
    }
}

const AI_NUMERICHOST: i32 = 4;

/// The hosts file, `%SystemRoot%\System32\drivers\etc\hosts`, which
/// Windows' resolver reads before asking DNS: lines of an address and its
/// names, `#` starting a comment. IPv4 entries only; read at each lookup
/// (no cache: an edit shows at once), the first 16 KiB.
mod hosts {
    use super::*;

    const MAX: usize = 16 * 1024;

    /// The address `name` has in the hosts file, if any.
    pub fn lookup(name: &[u8]) -> Option<[u8; 4]> {
        let mut path = [0u16; 300];
        // SAFETY: a buffer of the length given.
        let n = unsafe { GetSystemDirectoryW(path.as_mut_ptr(), 260) } as usize;
        let tail: &[u8] = b"\\drivers\\etc\\hosts";
        if n == 0 || n + tail.len() + 1 > path.len() {
            return None;
        }
        for (i, &c) in tail.iter().enumerate() {
            path[n + i] = u16::from(c);
        }
        path[n + tail.len()] = 0;
        // SAFETY: a NUL-terminated path; GENERIC_READ, shared reading,
        // OPEN_EXISTING.
        let h = unsafe { CreateFileW(path.as_ptr(), 0x8000_0000, 7, 0, 3, 0, 0) };
        if h == usize::MAX {
            return None;
        }
        let buffer = buffer()?;
        let mut got = 0u32;
        // SAFETY: our handle and a buffer of MAX bytes.
        let ok = unsafe { ReadFile(h, buffer, MAX as u32, &mut got, 0) };
        // SAFETY: our handle.
        unsafe { CloseHandle(h) };
        // SAFETY: the buffer holds `got` bytes read.
        let text = unsafe { core::slice::from_raw_parts(buffer, got as usize) };
        let found = if ok != 0 { find(text, name) } else { None };
        // SAFETY: the allocation made above.
        unsafe { VirtualFree(buffer, 0, 0x8000) };
        found
    }

    fn buffer() -> Option<*mut u8> {
        // SAFETY: a fresh reservation and commit, read-write.
        let p = unsafe { VirtualAlloc(core::ptr::null_mut(), MAX, 0x3000, 4) };
        (!p.is_null()).then_some(p)
    }

    /// The address of `name` in hosts-file `text` (names compared without
    /// case, as Windows does).
    pub fn find(text: &[u8], name: &[u8]) -> Option<[u8; 4]> {
        for line in text.split(|&c| c == b'\n') {
            let line = line.split(|&c| c == b'#').next().unwrap_or(&[]);
            let mut fields = line.split(|c| c.is_ascii_whitespace()).filter(|f| !f.is_empty());
            let Some(address) = fields.next().and_then(parse_ipv4) else { continue };
            if fields.any(|f| f.eq_ignore_ascii_case(name)) {
                return Some(address);
            }
        }
        None
    }
}

/// Name resolution over UDP (RFC 1035): one A query to the DNS server of
/// gala's static network configuration (QEMU's user network answers at
/// 10.0.2.3, forwarding to the host's resolver). No cache, no search
/// list, IPv4 only; without a network card the query cannot be sent and
/// the name is not found.
mod dns {
    use super::*;

    const SERVER: [u8; 4] = [10, 0, 2, 3];
    const PORT: u16 = 53;
    const TRIES: usize = 2;
    const WAIT_MS: i64 = 2000;
    const WSATRY_AGAIN: i32 = 11002;
    const WSANO_DATA: i32 = 11004;

    static NEXT_ID: AtomicU32 = AtomicU32::new(0x6a1a);

    /// The first IPv4 address of `name`, or the getaddrinfo error.
    pub fn resolve(name: &[u8]) -> Result<[u8; 4], i32> {
        let name = name.strip_suffix(b".").unwrap_or(name);
        let mut query = [0u8; 512];
        let id = (NEXT_ID.fetch_add(0x9e37, Ordering::Relaxed) ^ (&raw const query as u32)) as u16;
        let len = build_query(name, id, &mut query).ok_or(WSAHOST_NOT_FOUND)?;
        let s = open_endpoint_of(true).map_err(|_| WSATRY_AGAIN)?;
        let result = exchange(s, &query[..len], id);
        // SAFETY: our endpoint.
        unsafe { NtClose(s) };
        result
    }

    /// Header (id, recursion desired, one question), the name as labels,
    /// type A, class IN; its length (None: not a valid name).
    pub fn build_query(name: &[u8], id: u16, out: &mut [u8; 512]) -> Option<usize> {
        out[..12].copy_from_slice(&[0, 0, 1, 0, 0, 1, 0, 0, 0, 0, 0, 0]);
        out[..2].copy_from_slice(&id.to_be_bytes());
        let mut at = 12;
        if name.is_empty() || name.len() > 253 {
            return None;
        }
        for label in name.split(|&c| c == b'.') {
            if label.is_empty() || label.len() > 63 {
                return None;
            }
            out[at] = label.len() as u8;
            out[at + 1..at + 1 + label.len()].copy_from_slice(label);
            at += 1 + label.len();
        }
        out[at..at + 5].copy_from_slice(&[0, 0, 1, 0, 1]);
        Some(at + 5)
    }

    fn exchange(s: Socket, query: &[u8], id: u16) -> Result<[u8; 4], i32> {
        let mut server = Sa::any(AF_INET);
        server.b[2..4].copy_from_slice(&PORT.to_be_bytes());
        server.b[4..8].copy_from_slice(&SERVER);
        let (address, address_len) = transport_address(&server);
        let address = &address[..address_len];
        let wsabuf = crate::afd::wsabuf(query.len() as u32, query.as_ptr() as usize);
        let send = crate::afd::send_dgram(
            wsabuf.as_ptr() as usize,
            1,
            address.as_ptr() as usize,
            address.len() as u32,
        );
        let send = &send[..crate::afd::L.send_dgram_size()];
        let mut answer = [0u8; 512];
        for _ in 0..TRIES {
            // No route (no network card): not found, at once.
            afd(s, IOCTL_AFD_SEND_DATAGRAM, send, &mut []).map_err(|_| WSAHOST_NOT_FOUND)?;
            // Wait for an answer (AFD_POLL_READ), at most WAIT_MS.
            let mut poll_in = [0u8; 32];
            poll_in[..8].copy_from_slice(&i64::MAX.to_le_bytes());
            poll_in[8..12].copy_from_slice(&1u32.to_le_bytes());
            crate::afd::put_poll(&mut poll_in, 0, s, 1);
            let mut poll_out = [0u8; 32];
            let poll_in = &poll_in[..crate::afd::poll_len(1)];
            if poll(s, poll_in, &mut poll_out, Some(WAIT_MS)) != 0 || poll_out[8] == 0 {
                continue;
            }
            let rbuf = crate::afd::wsabuf(answer.len() as u32, answer.as_mut_ptr() as usize);
            let recv = crate::afd::recv_dgram(rbuf.as_ptr() as usize, 1, 0, 0, 0);
            let recv = &recv[..crate::afd::L.recv_dgram_size()];
            let Ok(n) = afd(s, IOCTL_AFD_RECV_DATAGRAM, recv, &mut []) else { continue };
            match parse_answer(&answer[..n.min(answer.len())], id) {
                Some(result) => return result,
                // Not the answer to this query: ask again.
                None => continue,
            }
        }
        Err(WSATRY_AGAIN)
    }

    /// The answer to query `id`: the first A record's address, or the
    /// error (None: not an answer to it).
    pub fn parse_answer(m: &[u8], id: u16) -> Option<Result<[u8; 4], i32>> {
        if m.len() < 12 || m[..2] != id.to_be_bytes() || m[2] & 0x80 == 0 {
            return None;
        }
        match m[3] & 0xf {
            0 => {}
            3 => return Some(Err(WSAHOST_NOT_FOUND)),
            _ => return Some(Err(WSATRY_AGAIN)),
        }
        let count = |at: usize| usize::from(u16::from_be_bytes([m[at], m[at + 1]]));
        let (questions, answers) = (count(4), count(6));
        let mut at = 12;
        for _ in 0..questions {
            at = skip_name(m, at)? + 4;
        }
        for _ in 0..answers {
            at = skip_name(m, at)?;
            let fixed = m.get(at..at + 10)?;
            let kind = u16::from_be_bytes([fixed[0], fixed[1]]);
            let class = u16::from_be_bytes([fixed[2], fixed[3]]);
            let len = usize::from(u16::from_be_bytes([fixed[8], fixed[9]]));
            let data = m.get(at + 10..at + 10 + len)?;
            if kind == 1 && class == 1 && len == 4 {
                return Some(Ok([data[0], data[1], data[2], data[3]]));
            }
            at += 10 + len;
        }
        Some(Err(WSANO_DATA))
    }

    /// The offset after the name at `at` (labels, maybe ending in a
    /// compression pointer).
    fn skip_name(m: &[u8], mut at: usize) -> Option<usize> {
        loop {
            let len = *m.get(at)?;
            match len {
                0 => return Some(at + 1),
                l if l & 0xc0 == 0xc0 => return Some(at + 2),
                l => at += 1 + usize::from(l),
            }
        }
    }
}

fn parse_u16(s: &[u8]) -> Option<u16> {
    if s.is_empty() || s.len() > 5 || !s.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let v = s.iter().fold(0u32, |v, &d| v * 10 + u32::from(d - b'0'));
    u16::try_from(v).ok()
}

/// ::1.
const LOOPBACK6: [u8; 16] = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1];

/// An IPv6 address in text (RFC 4291 section 2.2: eight groups of up to
/// four hex digits, one `::` for a run of zero groups, the last 32 bits
/// as a dotted IPv4 address).
fn parse_ipv6(s: &[u8]) -> Option<[u8; 16]> {
    if s.is_empty() || s.len() > 45 {
        return None;
    }
    let mut head = [0u16; 8];
    let mut tail = [0u16; 8];
    let (mut nh, mut nt) = (0usize, 0usize);
    let (before, after) = match s.windows(2).position(|w| w == b"::") {
        Some(i) => (&s[..i], Some(&s[i + 2..])),
        None => (s, None),
    };
    // Groups of one side; an IPv4 tail counts as two.
    fn groups(part: &[u8], out: &mut [u16; 8], n: &mut usize, last: bool) -> Option<()> {
        if part.is_empty() {
            return Some(());
        }
        let fields: &[u8] = part;
        let mut iter = fields.split(|&c| c == b':').peekable();
        while let Some(g) = iter.next() {
            let is_last = iter.peek().is_none();
            if is_last && last && g.contains(&b'.') {
                let v4 = parse_ipv4(g)?;
                if *n + 2 > 8 {
                    return None;
                }
                out[*n] = u16::from_be_bytes([v4[0], v4[1]]);
                out[*n + 1] = u16::from_be_bytes([v4[2], v4[3]]);
                *n += 2;
                continue;
            }
            if g.is_empty() || g.len() > 4 || *n == 8 {
                return None;
            }
            let mut v = 0u16;
            for &c in g {
                v = v * 16 + (c as char).to_digit(16)? as u16;
            }
            out[*n] = v;
            *n += 1;
        }
        Some(())
    }
    groups(before, &mut head, &mut nh, after.is_none())?;
    if let Some(after) = after {
        if after.windows(2).any(|w| w == b"::") {
            return None;
        }
        groups(after, &mut tail, &mut nt, true)?;
        if nh + nt > 7 {
            return None;
        }
    } else if nh != 8 {
        return None;
    }
    let mut words = [0u16; 8];
    words[..nh].copy_from_slice(&head[..nh]);
    words[8 - nt..].copy_from_slice(&tail[..nt]);
    let mut a = [0u8; 16];
    for (i, w) in words.iter().enumerate() {
        a[2 * i..2 * i + 2].copy_from_slice(&w.to_be_bytes());
    }
    Some(a)
}

/// An IPv6 address as text (RFC 5952: lower-case hex, no leading zeros,
/// the longest run of two or more zero groups as `::`, the first if
/// tied; IPv4-mapped addresses as `::ffff:a.b.c.d`), NUL-terminated, in
/// `out`; its length without the NUL.
fn format_ipv6(a: &[u8; 16], out: &mut [u8; 48]) -> usize {
    let words: [u16; 8] = core::array::from_fn(|i| u16::from_be_bytes([a[2 * i], a[2 * i + 1]]));
    let mut n = 0;
    let mut put = |b: u8, n: &mut usize| {
        out[*n] = b;
        *n += 1;
    };
    let hex = |v: u16, out: &mut dyn FnMut(u8)| {
        let digits = b"0123456789abcdef";
        let mut started = false;
        for shift in [12, 8, 4, 0] {
            let d = ((v >> shift) & 0xf) as usize;
            if d != 0 || started || shift == 0 {
                started = true;
                out(digits[d]);
            }
        }
    };
    if words[..5] == [0; 5] && words[5] == 0xffff {
        for &b in b"::ffff:" {
            put(b, &mut n);
        }
        let mut text = [0u8; 16];
        let len = format_ipv4(&[a[12], a[13], a[14], a[15]], &mut text);
        for &b in &text[..len] {
            put(b, &mut n);
        }
        out[n] = 0;
        return n;
    }
    // The longest run of zero groups (two or more).
    let (mut best, mut best_len, mut i) = (8usize, 1usize, 0usize);
    while i < 8 {
        if words[i] == 0 {
            let start = i;
            while i < 8 && words[i] == 0 {
                i += 1;
            }
            if i - start > best_len {
                (best, best_len) = (start, i - start);
            }
        } else {
            i += 1;
        }
    }
    let mut i = 0;
    while i < 8 {
        if i == best {
            put(b':', &mut n);
            if i == 0 {
                put(b':', &mut n);
            }
            i += best_len;
            continue;
        }
        hex(words[i], &mut |b| put(b, &mut n));
        if i < 7 {
            put(b':', &mut n);
        }
        i += 1;
    }
    out[n] = 0;
    n
}

/// a.b.c.d into `out`; its length.
fn format_ipv4(a: &[u8; 4], out: &mut [u8; 16]) -> usize {
    let mut n = 0;
    for (i, &v) in a.iter().enumerate() {
        if i > 0 {
            out[n] = b'.';
            n += 1;
        }
        if v >= 100 {
            out[n] = b'0' + v / 100;
            n += 1;
        }
        if v >= 10 {
            out[n] = b'0' + v / 10 % 10;
            n += 1;
        }
        out[n] = b'0' + v % 10;
        n += 1;
    }
    n
}

/// inet_pton(family, text, out): 1 for an address of `family` (AF_INET:
/// 4 bytes, AF_INET6: 16), 0 for text that is none, -1 (WSAEAFNOSUPPORT)
/// for another family.
#[unsafe(no_mangle)]
pub extern "system" fn inet_pton(family: i32, text: *const u8, out: *mut u8) -> i32 {
    if text.is_null() || out.is_null() {
        fail(WSAEFAULT);
        return -1;
    }
    // SAFETY: the caller's NUL-terminated text.
    let t = unsafe { core::slice::from_raw_parts(text, strlen(text)) };
    // SAFETY: the caller's buffer holds the address of `family`.
    unsafe {
        match family {
            AF_INET => match parse_ipv4(t) {
                Some(a) => core::ptr::copy_nonoverlapping(a.as_ptr(), out, 4),
                None => return 0,
            },
            AF_INET6 => match parse_ipv6(t) {
                Some(a) => core::ptr::copy_nonoverlapping(a.as_ptr(), out, 16),
                None => return 0,
            },
            _ => {
                fail(WSAEAFNOSUPPORT);
                return -1;
            }
        }
    }
    1
}

/// InetPtonW: inet_pton with UTF-16 text.
#[unsafe(no_mangle)]
pub extern "system" fn InetPtonW(family: i32, text: *const u16, out: *mut u8) -> i32 {
    let mut narrow = [0u8; 64];
    if text.is_null() {
        fail(WSAEFAULT);
        return -1;
    }
    let mut i = 0;
    // SAFETY: the caller's NUL-terminated text, read to its end or 63.
    unsafe {
        while i < 63 && *text.add(i) != 0 {
            let c = *text.add(i);
            narrow[i] = if c < 0x80 { c as u8 } else { b'?' };
            i += 1;
        }
    }
    inet_pton(family, narrow.as_ptr(), out)
}

/// inet_ntop(family, address, out, size): the address as text in `out`,
/// or NULL (WSAEINVAL: `size` too small; WSAEAFNOSUPPORT).
#[unsafe(no_mangle)]
pub extern "system" fn inet_ntop(
    family: i32,
    addr: *const u8,
    out: *mut u8,
    size: usize,
) -> *const u8 {
    if addr.is_null() || out.is_null() {
        fail(WSAEFAULT);
        return core::ptr::null();
    }
    let mut text = [0u8; 48];
    // SAFETY: the caller's address of `family`.
    let len = unsafe {
        match family {
            AF_INET => {
                let mut a = [0u8; 4];
                core::ptr::copy_nonoverlapping(addr, a.as_mut_ptr(), 4);
                let mut t = [0u8; 16];
                let n = format_ipv4(&a, &mut t);
                text[..n].copy_from_slice(&t[..n]);
                n
            }
            AF_INET6 => {
                let mut a = [0u8; 16];
                core::ptr::copy_nonoverlapping(addr, a.as_mut_ptr(), 16);
                format_ipv6(&a, &mut text)
            }
            _ => {
                fail(WSAEAFNOSUPPORT);
                return core::ptr::null();
            }
        }
    };
    if size <= len {
        fail(WSAEINVAL);
        return core::ptr::null();
    }
    // SAFETY: the caller's buffer of `size` > len bytes.
    unsafe {
        core::ptr::copy_nonoverlapping(text.as_ptr(), out, len);
        *out.add(len) = 0;
    }
    out
}

/// InetNtopW: inet_ntop with UTF-16 text (`size` in characters).
#[unsafe(no_mangle)]
pub extern "system" fn InetNtopW(
    family: i32,
    addr: *const u8,
    out: *mut u16,
    size: usize,
) -> *const u16 {
    let mut narrow = [0u8; 48];
    if out.is_null() || inet_ntop(family, addr, narrow.as_mut_ptr(), 48).is_null() {
        return core::ptr::null();
    }
    let len = narrow.iter().position(|&b| b == 0).unwrap_or(0);
    if size <= len {
        fail(WSAEINVAL);
        return core::ptr::null();
    }
    // SAFETY: the caller's buffer of `size` > len characters.
    unsafe {
        for (i, &b) in narrow[..len].iter().enumerate() {
            *out.add(i) = u16::from(b);
        }
        *out.add(len) = 0;
    }
    out
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
        let text = &mut *core::ptr::addr_of_mut!(TEXT);
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

/// PROTOENT (winsock2.h).
#[repr(C)]
pub struct ProtoEnt {
    name: *const u8,
    aliases: *const *const u8,
    proto: i16,
}

/// The protocol entries and their NULL-ended alias lists (one alias
/// each), built at compile time.
struct Aliases([[*const u8; 2]; 19]);
struct Entries([ProtoEnt; 19]);

// SAFETY: both hold only pointers to static data and are never written;
// sharing them between threads is sound.
unsafe impl Sync for Aliases {}
// SAFETY: as above.
unsafe impl Sync for Entries {}

/// The protocol database: the entries of Windows' default
/// `%SystemRoot%\System32\drivers\etc\protocol` (name, number, alias).
/// gala reads no protocol file; this table is that file's content.
const PROTOCOLS: [(&[u8], i16, &[u8]); 19] = [
    (b"ip\0", 0, b"IP\0"),
    (b"icmp\0", 1, b"ICMP\0"),
    (b"ggp\0", 3, b"GGP\0"),
    (b"tcp\0", 6, b"TCP\0"),
    (b"egp\0", 8, b"EGP\0"),
    (b"pup\0", 12, b"PUP\0"),
    (b"udp\0", 17, b"UDP\0"),
    (b"hmp\0", 20, b"HMP\0"),
    (b"xns-idp\0", 22, b"XNS-IDP\0"),
    (b"rdp\0", 27, b"RDP\0"),
    (b"ipv6\0", 41, b"IPv6\0"),
    (b"ipv6-route\0", 43, b"IPv6-Route\0"),
    (b"ipv6-frag\0", 44, b"IPv6-Frag\0"),
    (b"esp\0", 50, b"ESP\0"),
    (b"ah\0", 51, b"AH\0"),
    (b"ipv6-icmp\0", 58, b"IPv6-ICMP\0"),
    (b"ipv6-nonxt\0", 59, b"IPv6-NoNxt\0"),
    (b"ipv6-opts\0", 60, b"IPv6-Opts\0"),
    (b"rvd\0", 66, b"RVD\0"),
];

static ALIASES: Aliases = {
    let mut out = [[core::ptr::null(); 2]; 19];
    let mut i = 0;
    while i < PROTOCOLS.len() {
        out[i][0] = PROTOCOLS[i].2.as_ptr();
        i += 1;
    }
    Aliases(out)
};

static ENTRIES: Entries = {
    let mut out =
        [const { ProtoEnt { name: core::ptr::null(), aliases: core::ptr::null(), proto: 0 } }; 19];
    let mut i = 0;
    while i < PROTOCOLS.len() {
        out[i].name = PROTOCOLS[i].0.as_ptr();
        out[i].aliases = ALIASES.0[i].as_ptr();
        out[i].proto = PROTOCOLS[i].1;
        i += 1;
    }
    Entries(out)
};

fn protoent(i: usize) -> *const ProtoEnt {
    &ENTRIES.0[i]
}

/// getprotobyname (ordinal 53): the protocol whose name or alias is
/// `name` (ASCII case-insensitive), or NULL with WSANO_DATA.
#[unsafe(no_mangle)]
pub extern "system" fn getprotobyname(name: *const u8) -> *const ProtoEnt {
    if !name.is_null() {
        // SAFETY: the caller's NUL-terminated string.
        let want = unsafe { core::slice::from_raw_parts(name, strlen(name)) };
        for (i, (n, _, alias)) in PROTOCOLS.iter().enumerate() {
            let (n, alias) = (&n[..n.len() - 1], &alias[..alias.len() - 1]);
            if n.eq_ignore_ascii_case(want) || alias.eq_ignore_ascii_case(want) {
                return protoent(i);
            }
        }
    }
    set_last_error(WSANO_DATA);
    core::ptr::null()
}

/// getprotobynumber (ordinal 54): the protocol numbered `number`, or NULL
/// with WSANO_DATA.
#[unsafe(no_mangle)]
pub extern "system" fn getprotobynumber(number: i32) -> *const ProtoEnt {
    match PROTOCOLS.iter().position(|p| i32::from(p.1) == number) {
        Some(i) => protoent(i),
        None => {
            set_last_error(WSANO_DATA);
            core::ptr::null()
        }
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn getservbyport(_port: i32, _proto: *const u8) -> *const u8 {
    set_last_error(WSANO_DATA);
    core::ptr::null()
}

/// sendto(s, buf, len, flags, to, tolen): on a datagram socket one
/// datagram to `to` (IOCTL_AFD_SEND_DATAGRAM; an unbound socket is bound
/// to an ephemeral port), or to the connected peer when `to` is NULL; on
/// a stream socket the address is ignored, as documented (send).
#[unsafe(no_mangle)]
pub extern "system" fn sendto(
    s: Socket,
    buf: *const u8,
    len: i32,
    flags: i32,
    to: *const u8,
    to_len: i32,
) -> i32 {
    if len < 0 {
        return fail(WSAEINVAL);
    }
    let wsabuf = afd::wsabuf(len as u32, buf as usize);
    let mut sent = 0u32;
    let r = WSASendTo(
        s,
        wsabuf.as_ptr(),
        1,
        &mut sent,
        flags as u32,
        to,
        to_len,
        core::ptr::null_mut(),
        0,
    );
    if r == 0 { sent as i32 } else { r }
}

/// recvfrom(s, buf, len, flags, from, fromlen): on a datagram socket one
/// datagram and its sender (IOCTL_AFD_RECV_DATAGRAM; a datagram larger
/// than `len` fills the buffer and fails with WSAEMSGSIZE, the rest
/// discarded); on a stream socket `from` is ignored, as documented (recv).
#[unsafe(no_mangle)]
pub extern "system" fn recvfrom(
    s: Socket,
    buf: *mut u8,
    len: i32,
    flags: i32,
    from: *mut u8,
    from_len: *mut i32,
) -> i32 {
    if len < 0 {
        return fail(WSAEINVAL);
    }
    let wsabuf = afd::wsabuf(len as u32, buf as usize);
    let (mut got, mut f) = (0u32, flags as u32);
    let r = WSARecvFrom(
        s,
        wsabuf.as_ptr(),
        1,
        &mut got,
        &mut f,
        from,
        from_len,
        core::ptr::null_mut(),
        0,
    );
    if r == 0 { got as i32 } else { r }
}

/// The address AFD sends a datagram to: a TRANSPORT_ADDRESS from the
/// caller's SOCKADDR_IN.
fn destination(to: *const u8, to_len: i32) -> Result<([u8; 34], usize), i32> {
    Ok(transport_address(&read_sockaddr(to, to_len)?))
}

/// WSASendTo(s, lpBuffers, dwBufferCount, lpNumberOfBytesSent, dwFlags,
/// lpTo, iTolen, lpOverlapped, lpCompletionRoutine): one datagram of the
/// buffers' bytes to `lpTo` (NULL: the connected peer); on a stream socket
/// WSASend, the address ignored.
#[unsafe(no_mangle)]
pub extern "system" fn WSASendTo(
    s: Socket,
    buffers: *const u8,
    count: u32,
    sent: *mut u32,
    flags: u32,
    to: *const u8,
    to_len: i32,
    ov: *mut Overlapped,
    routine: usize,
) -> i32 {
    if !started() {
        return fail(WSANOTINITIALISED);
    }
    if !is_datagram(s) || to.is_null() {
        return WSASend(s, buffers, count, sent, flags, ov, routine);
    }
    if flags != 0 {
        missing(b"ws2_32.dll!WSASendTo(flags)", flags.into());
        return fail(WSAEOPNOTSUPP);
    }
    let (address, address_len) = match destination(to, to_len) {
        Ok(a) => a,
        Err(e) => return e,
    };
    // AFD_SEND_INFO_UDP (docs/ARCHITECTURE.md, "AFD"): the buffers, then
    // TDI_CONNECTION_INFORMATION's RemoteAddressLength and RemoteAddress
    // (AFD copies it during the call).
    let info =
        afd::send_dgram(buffers as usize, count, address.as_ptr() as usize, address_len as u32);
    request(s, IOCTL_AFD_SEND_DATAGRAM, &info[..afd::L.send_dgram_size()], sent, ov, routine)
}

/// WSARecvFrom(s, lpBuffers, dwBufferCount, lpNumberOfBytesRecvd,
/// lpFlags, lpFrom, lpFromlen, lpOverlapped, lpCompletionRoutine): one
/// datagram and its sender (written when it completes: lpFrom and
/// lpFromlen must stay valid until then, as documented); on a stream
/// socket WSARecv, the address ignored.
#[unsafe(no_mangle)]
pub extern "system" fn WSARecvFrom(
    s: Socket,
    buffers: *const u8,
    count: u32,
    received: *mut u32,
    flags: *mut u32,
    from: *mut u8,
    from_len: *mut i32,
    ov: *mut Overlapped,
    routine: usize,
) -> i32 {
    if !started() {
        return fail(WSANOTINITIALISED);
    }
    if !is_datagram(s) {
        return WSARecv(s, buffers, count, received, flags, ov, routine);
    }
    // SAFETY: the caller passes valid flags (in/out).
    let given = if flags.is_null() { 0 } else { unsafe { *flags } };
    if given & !(MSG_PEEK as u32) != 0 {
        missing(b"ws2_32.dll!WSARecvFrom(flags)", given.into());
        return fail(WSAEOPNOTSUPP);
    }
    // SAFETY: the caller's length, with its buffer.
    if !from.is_null() && (from_len.is_null() || unsafe { *from_len } < min_len_of(s)) {
        return fail(WSAEFAULT);
    }
    // AFD_RECV_INFO_UDP: the buffers, the TDI flags (peek), where the
    // sender's SOCKADDR_IN and its length go.
    let (from, from_len) = if from.is_null() { (0, 0) } else { (from as usize, from_len as usize) };
    let peek = if given != 0 { AFD_MSG_PEEK } else { 0 };
    let info = afd::recv_dgram(buffers as usize, count, peek, from, from_len);
    let info = &info[..afd::L.recv_dgram_size()];
    let result = request(s, IOCTL_AFD_RECV_DATAGRAM, info, received, ov, routine);
    if result == 0 && !flags.is_null() {
        // SAFETY: as above.
        unsafe { *flags = 0 };
    }
    result
}

/// An AFD request with the caller's byte count and OVERLAPPED (blocking
/// without one), as WSARecv/WSASend make it.
fn request(
    s: Socket,
    code: u32,
    info: &[u8],
    bytes: *mut u32,
    ov: *mut Overlapped,
    routine: usize,
) -> i32 {
    if ov.is_null() {
        return match afd(s, code, info, &mut []) {
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
    let status = afd_overlapped_apc(s, code, info, core::ptr::null_mut(), 0, ov, routine);
    if status == 0 && !bytes.is_null() {
        // SAFETY: done: the kernel wrote InternalHigh; the caller's count.
        unsafe { *bytes = (*ov).internal_high as u32 };
    }
    overlapped_result(status)
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

/// Writes the dotted-decimal text of the IPv4 `address` (network order)
/// into `out` (`size` bytes, NUL added); its length, or None.
fn ipv4_text(address: u32, out: *mut u8, size: usize) -> Option<usize> {
    let text = inet_ntoa(address);
    let mut n = 0;
    // SAFETY: inet_ntoa's NUL-terminated text (at most 16 bytes).
    while unsafe { *text.add(n) } != 0 {
        n += 1;
    }
    if out.is_null() || n + 1 > size {
        return None;
    }
    // SAFETY: n + 1 <= size bytes of the caller's buffer.
    unsafe { core::ptr::copy_nonoverlapping(text, out, n + 1) };
    Some(n)
}

/// getnameinfo(sa, len, host, hostlen, serv, servlen, flags): numeric
/// only (there is no resolver): the address and the port as text.
#[unsafe(no_mangle)]
pub extern "system" fn getnameinfo(
    sa: *const u8,
    _len: i32,
    host: *mut u8,
    host_len: u32,
    serv: *mut u8,
    serv_len: u32,
    _flags: i32,
) -> i32 {
    // SAFETY: the caller's sockaddr: family at 0; sockaddr_in port at 2,
    // address at 4.
    let (family, port, address) = unsafe {
        (
            sa.cast::<u16>().read_unaligned(),
            sa.add(2).cast::<u16>().read_unaligned(),
            sa.add(4).cast::<u32>().read_unaligned(),
        )
    };
    if family != 2 {
        return 10047; // WSAEAFNOSUPPORT
    }
    if !host.is_null() && ipv4_text(address, host, host_len as usize).is_none() {
        return 10055; // WSAENOBUFS
    }
    if !serv.is_null() {
        let mut digits = [0u8; 6];
        let mut p = u16::from_be(port);
        let mut n = 0;
        loop {
            digits[n] = b'0' + (p % 10) as u8;
            n += 1;
            p /= 10;
            if p == 0 {
                break;
            }
        }
        if n + 1 > serv_len as usize {
            return 10055;
        }
        for i in 0..n {
            // SAFETY: n + 1 <= serv_len bytes.
            unsafe { *serv.add(i) = digits[n - 1 - i] };
        }
        // SAFETY: as above.
        unsafe { *serv.add(n) = 0 };
    }
    0
}

/// WSAAddressToStringA(sa, len, info, out, *outlen): "a.b.c.d[:port]".
#[unsafe(no_mangle)]
pub extern "system" fn WSAAddressToStringA(
    sa: *const u8,
    _len: u32,
    _info: *mut u8,
    out: *mut u8,
    out_len: *mut u32,
) -> i32 {
    let (mut host, mut serv) = ([0u8; 16], [0u8; 6]);
    if getnameinfo(sa, 16, host.as_mut_ptr(), 16, serv.as_mut_ptr(), 6, 0) != 0 {
        return fail(10022); // WSAEINVAL
    }
    let h = host.iter().position(|&c| c == 0).unwrap_or(0);
    let s = serv.iter().position(|&c| c == 0).unwrap_or(0);
    let with_port = serv[..s] != *b"0";
    let need = h + if with_port { 1 + s } else { 0 } + 1;
    // SAFETY: the caller's length and buffer.
    unsafe {
        if out.is_null() || out_len.is_null() || (*out_len as usize) < need {
            if !out_len.is_null() {
                *out_len = need as u32;
            }
            return fail(10014); // WSAEFAULT
        }
        core::ptr::copy_nonoverlapping(host.as_ptr(), out, h);
        let mut at = h;
        if with_port {
            *out.add(at) = b':';
            core::ptr::copy_nonoverlapping(serv.as_ptr(), out.add(at + 1), s);
            at += 1 + s;
        }
        *out.add(at) = 0;
        *out_len = need as u32;
    }
    0
}

/// A socket address in text, as WSAStringToAddress reads it: for AF_INET
/// `a.b.c.d` or `a.b.c.d:port`; for AF_INET6 `addr`, `addr%scope`,
/// `[addr]`, `[addr%scope]` and either bracketed form with `:port`. The
/// SOCKADDR_IN or SOCKADDR_IN6 (port in network order, the rest zero).
fn parse_socket_address(text: &[u8], af: i32) -> Option<Sa> {
    let mut sa = Sa { b: [0; 28], len: 0 };
    let port_of = |p: Option<&[u8]>| match p {
        None => Some(0),
        Some(p) => parse_u16(p),
    };
    match af {
        AF_INET => {
            let (host, port) = match text.iter().position(|&c| c == b':') {
                Some(i) => (&text[..i], Some(&text[i + 1..])),
                None => (text, None),
            };
            let a = parse_ipv4(host)?;
            sa.b[..2].copy_from_slice(&(AF_INET as u16).to_le_bytes());
            sa.b[2..4].copy_from_slice(&port_of(port)?.to_be_bytes());
            sa.b[4..8].copy_from_slice(&a);
            sa.len = 16;
        }
        AF_INET6 => {
            let (inner, port) = match text.strip_prefix(b"[") {
                Some(rest) => {
                    let close = rest.iter().position(|&c| c == b']')?;
                    let after = &rest[close + 1..];
                    let port = match after {
                        [] => None,
                        [b':', p @ ..] => Some(p),
                        _ => return None,
                    };
                    (&rest[..close], port)
                }
                None => (text, None),
            };
            let (host, scope) = match inner.iter().position(|&c| c == b'%') {
                Some(i) => {
                    let digits = &inner[i + 1..];
                    if digits.is_empty()
                        || digits.len() > 10
                        || !digits.iter().all(u8::is_ascii_digit)
                    {
                        return None;
                    }
                    let v = digits.iter().fold(0u64, |v, &d| v * 10 + u64::from(d - b'0'));
                    (&inner[..i], u32::try_from(v).ok()?)
                }
                None => (inner, 0),
            };
            let a = parse_ipv6(host)?;
            sa.b[..2].copy_from_slice(&(AF_INET6 as u16).to_le_bytes());
            sa.b[2..4].copy_from_slice(&port_of(port)?.to_be_bytes());
            sa.b[8..24].copy_from_slice(&a);
            sa.b[24..28].copy_from_slice(&scope.to_le_bytes());
            sa.len = 28;
        }
        _ => return None,
    }
    Some(sa)
}

/// WSAStringToAddressA(text, af, lpProtocolInfo, lpAddress,
/// lpAddressLength): the address `parse_socket_address` reads, its length
/// in *lpAddressLength; WSAEINVAL for text that is no address of the
/// family, WSAEFAULT for a buffer shorter than the family's SOCKADDR.
#[unsafe(no_mangle)]
pub extern "system" fn WSAStringToAddressA(
    text: *const u8,
    af: i32,
    _info: *const u8,
    addr: *mut u8,
    len: *mut i32,
) -> i32 {
    if text.is_null() {
        return fail(WSAEINVAL);
    }
    // SAFETY: the caller's NUL-terminated string.
    let s = unsafe { core::slice::from_raw_parts(text, strlen(text)) };
    string_to_address(s, af, addr, len)
}

/// WSAStringToAddressW: WSAStringToAddressA with UTF-16 text (an address
/// is ASCII; anything else is WSAEINVAL).
#[unsafe(no_mangle)]
pub extern "system" fn WSAStringToAddressW(
    text: *const u16,
    af: i32,
    _info: *const u8,
    addr: *mut u8,
    len: *mut i32,
) -> i32 {
    if text.is_null() {
        return fail(WSAEINVAL);
    }
    let mut narrow = [0u8; 64];
    let mut n = 0;
    // SAFETY: the caller's NUL-terminated string.
    unsafe {
        while *text.add(n) != 0 {
            let c = *text.add(n);
            if c > 0x7f || n == narrow.len() {
                return fail(WSAEINVAL);
            }
            narrow[n] = c as u8;
            n += 1;
        }
    }
    string_to_address(&narrow[..n], af, addr, len)
}

fn string_to_address(text: &[u8], af: i32, addr: *mut u8, len: *mut i32) -> i32 {
    if !started() {
        return fail(WSANOTINITIALISED);
    }
    let Some(sa) = parse_socket_address(text, af) else {
        return fail(WSAEINVAL);
    };
    // SAFETY: the caller's length and buffer of that length.
    unsafe {
        if addr.is_null() || len.is_null() || (*len as usize) < sa.len {
            if !len.is_null() {
                *len = sa.len as i32;
            }
            return fail(WSAEFAULT);
        }
        core::ptr::copy_nonoverlapping(sa.b.as_ptr(), addr, sa.len);
        *len = sa.len as i32;
    }
    0
}

/// WSAEventSelect/WSAEnumNetworkEvents: not yet (select and overlapped
/// I/O are); recorded.
/// WSAConnect(s, name, namelen, lpCallerData, lpCalleeData, lpSQOS,
/// lpGQOS): connect; connect data and quality of service are not
/// supported (recorded; WSAEOPNOTSUPP).
#[unsafe(no_mangle)]
pub extern "system" fn WSAConnect(
    s: Socket,
    name: *const u8,
    len: i32,
    caller: *const u8,
    callee: *mut u8,
    sqos: *const u8,
    gqos: *const u8,
) -> i32 {
    if !caller.is_null() || !callee.is_null() || !sqos.is_null() || !gqos.is_null() {
        missing(b"ws2_32.dll!WSAConnect(connect data or QoS)", 0);
        return fail(WSAEOPNOTSUPP);
    }
    connect(s, name, len)
}

/// WSAAccept(s, addr, addrlen, lpfnCondition, dwCallbackData): accept;
/// a condition function is not supported yet (recorded; WSAEOPNOTSUPP).
#[unsafe(no_mangle)]
pub extern "system" fn WSAAccept(
    s: Socket,
    addr: *mut u8,
    len: *mut i32,
    condition: usize,
    _data: usize,
) -> Socket {
    if condition != 0 {
        missing(b"ws2_32.dll!WSAAccept(condition function)", 0);
        fail(WSAEOPNOTSUPP);
        return INVALID_SOCKET;
    }
    accept(s, addr, len)
}

const WSA_INVALID_PARAMETER: u32 = 87;
const WSA_INVALID_HANDLE: u32 = 6;

/// IOCTL_AFD_EVENT_SELECT and IOCTL_AFD_GET_EVENTS.
const IOCTL_AFD_EVENT_SELECT: u32 = 0x12087;
const IOCTL_AFD_GET_EVENTS: u32 = 0x1208b;

/// The FD_* network events and the AFD_POLL_* events behind each:
/// (FD bit index, FD bit, AFD_POLL_* bits).
const FD_EVENTS: [(usize, i32, u32); 6] = [
    (0, 1, 0x1),                // FD_READ: READ
    (1, 2, 0x4),                // FD_WRITE: WRITE
    (2, 4, 0x2),                // FD_OOB: OOB
    (3, 8, 0x80),               // FD_ACCEPT: ACCEPT
    (4, 16, 0x40 | 0x100),      // FD_CONNECT: CONNECT, CONNECT_ERR
    (5, 32, 0x8 | 0x10 | 0x20), // FD_CLOSE: HUP, RESET, CLOSE
];

/// WSAEventSelect(s, hEventObject, lNetworkEvents): AFD sets the event
/// when one of the network events happens (or holds already); the socket
/// becomes non-blocking. No event or no events: the selection ends.
#[unsafe(no_mangle)]
pub extern "system" fn WSAEventSelect(s: usize, event: usize, events: i32) -> i32 {
    let mut mask = 0u32;
    for &(_, fd, afd) in &FD_EVENTS {
        if events & fd != 0 {
            mask |= afd;
        }
    }
    if events & !0x3f != 0 {
        missing(b"ws2_32.dll!WSAEventSelect(events)", events as u64);
    }
    // afd_event_select_params {HANDLE event; int mask;}
    let mut input = [0u8; 16];
    afd::L.put_ptr(&mut input, 0, event as u64);
    input[afd::L.event_select_mask()..][..4].copy_from_slice(&mask.to_le_bytes());
    match afd_call(s, IOCTL_AFD_EVENT_SELECT, &input[..afd::L.event_select_size()], &mut []) {
        Ok(_) => 0,
        Err(e) => e,
    }
}

/// WSAWaitForMultipleEvents(count, events, all, ms, alertable): a wait on
/// the event objects (WSA_WAIT_EVENT_0 + i, WSA_WAIT_TIMEOUT,
/// WSA_WAIT_IO_COMPLETION; WSA_WAIT_FAILED with the error set).
#[unsafe(no_mangle)]
pub extern "system" fn WSAWaitForMultipleEvents(
    count: u32,
    events: *const usize,
    all: i32,
    ms: u32,
    alertable: i32,
) -> u32 {
    #[cfg_attr(
        target_arch = "x86",
        link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
    )]
    #[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
    unsafe extern "system" {
        fn WaitForMultipleObjectsEx(
            count: u32,
            handles: *const usize,
            all: i32,
            ms: u32,
            alertable: i32,
        ) -> u32;
    }
    if count == 0 || count > 64 || events.is_null() {
        fail(WSA_INVALID_PARAMETER);
        return 0xffff_ffff;
    }
    // SAFETY: the caller's array of `count` event handles.
    let r = unsafe { WaitForMultipleObjectsEx(count, events, all, ms, alertable) };
    if r == 0xffff_ffff {
        fail(WSA_INVALID_HANDLE);
    }
    r
}

/// WSAEnumNetworkEvents(s, hEventObject, lpNetworkEvents): the network
/// events recorded since the last call (cleared), with their error codes;
/// the event is reset.
#[unsafe(no_mangle)]
pub extern "system" fn WSAEnumNetworkEvents(s: usize, event: usize, out: *mut u8) -> i32 {
    if out.is_null() {
        return fail(WSAEFAULT);
    }
    // afd_get_events_params {int flags; int status[13];}; the input is the
    // event AFD resets with its copy, in one step (as documented: a record
    // made between a copy and a later reset here would lose its signal).
    let mut got = [0u8; 56];
    let input = event.to_le_bytes();
    if let Err(e) = afd_call(s, IOCTL_AFD_GET_EVENTS, &input, &mut got) {
        return e;
    }
    let flags = u32::from_le_bytes([got[0], got[1], got[2], got[3]]);
    // WSANETWORKEVENTS {long lNetworkEvents; int iErrorCode[10];}
    let mut result = [0i32; 11];
    for &(bit, fd, afd) in &FD_EVENTS {
        if flags & afd != 0 {
            result[0] |= fd;
        }
        if bit == 4 && flags & 0x100 != 0 {
            result[1 + bit] = WSAECONNREFUSED as i32;
        }
        if bit == 5 && flags & 0x10 != 0 {
            result[1 + bit] = WSAECONNRESET as i32;
        }
    }
    // SAFETY: the caller's WSANETWORKEVENTS (44 bytes).
    unsafe { core::ptr::copy_nonoverlapping(result.as_ptr().cast::<u8>(), out, 44) };
    0
}

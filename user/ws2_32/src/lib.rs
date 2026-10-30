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
    fn GalaRecordMissing(name: *const u8, length: usize, what: u32, detail: u64) -> u32;
}

#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
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
        0xc000_0236 => WSAECONNREFUSED,
        0xc000_020d => WSAECONNRESET,
        0xc000_020a => WSAEADDRINUSE,
        0xc000_0207 => WSAEADDRNOTAVAIL,
        0xc000_0140 => WSAENOTCONN,
        0xc000_023c => WSAENETUNREACH,
        0xc000_0008 | 0xc000_0024 => WSAENOTSOCK,
        0xc000_009a | 0xc000_0017 => WSAENOBUFS,
        0xc000_0005 => WSAEFAULT,
        0xc000_00bb | 0xc000_0010 => WSAEOPNOTSUPP,
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

/// NtDeviceIoControlFile on a socket; IoStatus.Information or the status.
fn afd(s: Socket, code: u32, input: &[u8], output: &mut [u8]) -> Result<usize, u32> {
    let mut iosb = [0usize; 2];
    // SAFETY: valid buffers of the lengths given.
    let status = unsafe {
        NtDeviceIoControlFile(
            s,
            0,
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
    if status == 0 { Ok(iosb[1]) } else { Err(status) }
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
    if flags != 0 {
        missing(b"ws2_32.dll!send/recv(flags)", flags as u64);
        return fail(WSAEOPNOTSUPP);
    }
    // WSABUF {u32 len; char *buf}, then {WSABUF *; u32 count; u32 afd
    // flags; u32 tdi flags}.
    let mut wsabuf = [0u8; 16];
    wsabuf[..4].copy_from_slice(&(len as u32).to_le_bytes());
    wsabuf[8..].copy_from_slice(&(buf as u64).to_le_bytes());
    let mut info = [0u8; 24];
    info[..8].copy_from_slice(&(wsabuf.as_ptr() as u64).to_le_bytes());
    info[8..12].copy_from_slice(&1u32.to_le_bytes());
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

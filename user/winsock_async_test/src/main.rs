//! Overlapped Winsock with an I/O completion port, the way Go's `net`
//! package (and most servers) use it, with the results real Windows gives:
//! WSASocketW, AcceptEx and ConnectEx (loaded with WSAIoctl
//! SIO_GET_EXTENSION_FUNCTION_POINTER) pending until the other side shows
//! up, completion packets carrying the key and the OVERLAPPED, the
//! SO_UPDATE_*_CONTEXT options, GetAcceptExSockaddrs (from mswsock.dll),
//! overlapped WSARecv/WSASend, CancelIoEx (ERROR_OPERATION_ABORTED, and
//! ERROR_NOT_FOUND when nothing is pending), WSAGetOverlappedResult and
//! FILE_SKIP_COMPLETION_PORT_ON_SUCCESS; completion routines (WSARecv and
//! WSASend with lpCompletionRoutine: queued to the thread, run in SleepEx's
//! alertable wait, also for a receive that completed at once);
//! WSAEventSelect/WSAEnumNetworkEvents (FD_ACCEPT, FD_READ, FD_CLOSE; the
//! event reset). Prints `winsock_async_test: ok`
//! or exits with the number of the failing check.

#![no_std]
#![no_main]

use core::ffi::c_void;

type Bool = i32;
type Handle = *mut c_void;
type Socket = usize;

#[repr(C)]
struct Overlapped {
    internal: usize,
    internal_high: usize,
    offset: u64,
    event: Handle,
}

impl Overlapped {
    const fn new() -> Self {
        Self { internal: 0, internal_high: 0, offset: 0, event: core::ptr::null_mut() }
    }
}

#[repr(C)]
struct WsaBuf {
    len: u32,
    buf: *mut u8,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct SockaddrIn {
    family: u16,
    port: u16,
    addr: [u8; 4],
    zero: [u8; 8],
}

impl SockaddrIn {
    fn new(addr: [u8; 4], port: u16) -> Self {
        Self { family: AF_INET as u16, port: port.to_be(), addr, zero: [0; 8] }
    }
}

#[repr(C)]
struct Guid(u32, u16, u16, [u8; 8]);

type AcceptExFn = unsafe extern "system" fn(
    Socket,
    Socket,
    *mut u8,
    u32,
    u32,
    u32,
    *mut u32,
    *mut Overlapped,
) -> Bool;
type ConnectExFn = unsafe extern "system" fn(
    Socket,
    *const SockaddrIn,
    i32,
    *const u8,
    u32,
    *mut u32,
    *mut Overlapped,
) -> Bool;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut c_void) -> Bool;
    fn GetLastError() -> u32;
    fn CreateIoCompletionPort(file: Handle, port: Handle, key: usize, threads: u32) -> Handle;
    fn GetQueuedCompletionStatus(
        port: Handle,
        bytes: *mut u32,
        key: *mut usize,
        ov: *mut *mut Overlapped,
        ms: u32,
    ) -> Bool;
    fn CancelIoEx(h: Handle, ov: *mut Overlapped) -> Bool;
    fn SetFileCompletionNotificationModes(h: Handle, flags: u8) -> Bool;
    fn CloseHandle(h: Handle) -> Bool;
    fn Sleep(ms: u32);
    fn SleepEx(ms: u32, alertable: Bool) -> u32;
    fn CreateEventW(
        attributes: *mut c_void,
        manual: Bool,
        initial: Bool,
        name: *const u16,
    ) -> Handle;
    fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
    fn ExitProcess(code: u32) -> !;
}

#[link(name = "ws2_32", kind = "raw-dylib")]
unsafe extern "system" {
    fn WSAStartup(version: u16, data: *mut u8) -> i32;
    fn WSACleanup() -> i32;
    fn WSAGetLastError() -> i32;
    fn WSASocketW(
        af: i32,
        kind: i32,
        protocol: i32,
        info: *mut c_void,
        g: u32,
        flags: u32,
    ) -> Socket;
    fn WSAIoctl(
        s: Socket,
        code: u32,
        input: *const c_void,
        input_len: u32,
        output: *mut c_void,
        output_len: u32,
        returned: *mut u32,
        ov: *mut Overlapped,
        routine: usize,
    ) -> i32;
    fn WSARecv(
        s: Socket,
        bufs: *mut WsaBuf,
        count: u32,
        received: *mut u32,
        flags: *mut u32,
        ov: *mut Overlapped,
        routine: usize,
    ) -> i32;
    fn WSASend(
        s: Socket,
        bufs: *mut WsaBuf,
        count: u32,
        sent: *mut u32,
        flags: u32,
        ov: *mut Overlapped,
        routine: usize,
    ) -> i32;
    fn WSAGetOverlappedResult(
        s: Socket,
        ov: *mut Overlapped,
        bytes: *mut u32,
        wait: Bool,
        flags: *mut u32,
    ) -> Bool;
    fn bind(s: Socket, addr: *const SockaddrIn, len: i32) -> i32;
    fn listen(s: Socket, backlog: i32) -> i32;
    fn connect(s: Socket, addr: *const SockaddrIn, len: i32) -> i32;
    fn WSAEventSelect(s: Socket, event: Handle, events: i32) -> i32;
    fn WSAEnumNetworkEvents(s: Socket, event: Handle, out: *mut i32) -> i32;
    fn accept(s: Socket, addr: *mut SockaddrIn, len: *mut i32) -> Socket;
    fn getsockname(s: Socket, addr: *mut SockaddrIn, len: *mut i32) -> i32;
    fn setsockopt(s: Socket, level: i32, name: i32, value: *const u8, len: i32) -> i32;
    fn closesocket(s: Socket) -> i32;
}

#[link(name = "mswsock", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetAcceptExSockaddrs(
        buf: *mut u8,
        receive_len: u32,
        local_len: u32,
        remote_len: u32,
        local: *mut *mut SockaddrIn,
        local_size: *mut i32,
        remote: *mut *mut SockaddrIn,
        remote_size: *mut i32,
    );
}

#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
}

#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

#[unsafe(no_mangle)]
pub extern "C" fn __CxxFrameHandler3() -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdeac) }
}

const AF_INET: i32 = 2;
const SOCK_STREAM: i32 = 1;
const IPPROTO_TCP: i32 = 6;
const SOL_SOCKET: i32 = 0xffff;
const SO_UPDATE_ACCEPT_CONTEXT: i32 = 0x700b;
const SO_UPDATE_CONNECT_CONTEXT: i32 = 0x7010;
const TCP_NODELAY: i32 = 1;
const WSA_FLAG_OVERLAPPED: u32 = 1;
const INVALID_SOCKET: Socket = !0;
const SIO_GET_EXTENSION_FUNCTION_POINTER: u32 = 0xc800_0006;
const WSAID_ACCEPTEX: Guid =
    Guid(0xb536_7df1, 0xcbac, 0x11cf, [0x95, 0xca, 0x00, 0x80, 0x5f, 0x48, 0xa1, 0x92]);
const WSAID_CONNECTEX: Guid =
    Guid(0x25a2_07b9, 0xddf3, 0x4660, [0x8e, 0xe9, 0x76, 0xe5, 0x8c, 0x74, 0x06, 0x3e]);
const ERROR_IO_PENDING: u32 = 997;
const ERROR_OPERATION_ABORTED: u32 = 995;
const ERROR_NOT_FOUND: u32 = 1168;
const WAIT_TIMEOUT: u32 = 258;
const FILE_SKIP_COMPLETION_PORT_ON_SUCCESS: u8 = 1;
const LOOPBACK: [u8; 4] = [127, 0, 0, 1];
const ADDR_LEN: i32 = size_of::<SockaddrIn>() as i32;
/// AcceptEx's address blocks: a SOCKADDR_IN plus 16, as documented.
const ADDR_BLOCK: u32 = ADDR_LEN as u32 + 16;

fn out(text: &[u8]) {
    let mut written = 0;
    // SAFETY: valid buffer and output pointer.
    unsafe {
        WriteFile(
            GetStdHandle(-11i32 as u32),
            text.as_ptr(),
            text.len() as u32,
            &mut written,
            core::ptr::null_mut(),
        )
    };
}

fn hex(v: u32) -> [u8; 8] {
    let mut s = [0u8; 8];
    for (i, c) in s.iter_mut().enumerate() {
        let d = (v >> (28 - 4 * i)) & 0xf;
        *c = if d < 10 { b'0' + d as u8 } else { b'a' + d as u8 - 10 };
    }
    s
}

struct Checks(u32);

impl Checks {
    fn ok(&mut self, cond: bool, got: u64) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        out(b"winsock_async_test: check ");
        out(&hex(self.0));
        out(b" got ");
        out(&hex(got as u32));
        out(b"\r\n");
        Err(self.0)
    }

    fn eq(&mut self, got: u64, want: u64) -> Result<(), u32> {
        self.ok(got == want, got)
    }
}

/// One completion packet: (succeeded, bytes, key, OVERLAPPED, error).
struct Packet {
    ok: bool,
    bytes: u32,
    key: usize,
    ov: *mut Overlapped,
    error: u32,
}

fn dequeue(port: Handle, ms: u32) -> Packet {
    let (mut bytes, mut key, mut ov) = (0u32, 0usize, core::ptr::null_mut());
    // SAFETY: valid output pointers.
    let ok = unsafe { GetQueuedCompletionStatus(port, &mut bytes, &mut key, &mut ov, ms) } != 0;
    // SAFETY: plain call.
    let error = if ok { 0 } else { unsafe { GetLastError() } };
    Packet { ok, bytes, key, ov, error }
}

fn extension<T>(s: Socket, guid: &Guid) -> Option<T> {
    let mut f: usize = 0;
    let mut returned = 0u32;
    // SAFETY: valid buffers of the sizes given.
    let r = unsafe {
        WSAIoctl(
            s,
            SIO_GET_EXTENSION_FUNCTION_POINTER,
            (guid as *const Guid).cast(),
            size_of::<Guid>() as u32,
            (&raw mut f).cast(),
            8,
            &mut returned,
            core::ptr::null_mut(),
            0,
        )
    };
    // SAFETY: a non-null function pointer of the documented type.
    (r == 0 && f != 0).then(|| unsafe { core::mem::transmute_copy::<usize, T>(&f) })
}

static mut DONE_COUNT: u32 = 0;
static mut DONE_ERROR: u32 = 0;
static mut DONE_BYTES: u32 = 0;
static mut DONE_OV: *mut Overlapped = core::ptr::null_mut();
const WAIT_IO_COMPLETION: u32 = 0xc0;

/// The completion routine: (dwError, cbTransferred, lpOverlapped, dwFlags).
extern "system" fn done(error: u32, bytes: u32, ov: *mut Overlapped, _flags: u32) {
    // SAFETY: runs on the test's thread only (an APC).
    unsafe {
        DONE_COUNT += 1;
        DONE_ERROR = error;
        DONE_BYTES = bytes;
        DONE_OV = ov;
    }
}

fn overlapped_socket() -> Socket {
    // SAFETY: plain call.
    unsafe {
        WSASocketW(AF_INET, SOCK_STREAM, IPPROTO_TCP, core::ptr::null_mut(), 0, WSA_FLAG_OVERLAPPED)
    }
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    // SAFETY: documented Winsock and kernel32 calls with valid arguments;
    // every OVERLAPPED and buffer outlives its I/O (all are waited for).
    unsafe {
        let mut data = [0u8; 408];
        c.eq(WSAStartup(0x0202, data.as_mut_ptr()) as u64, 0)?;
        let port = CreateIoCompletionPort((-1isize) as Handle, core::ptr::null_mut(), 0, 0);
        c.ok(!port.is_null(), GetLastError().into())?;

        // Listener on an ephemeral loopback port, associated with key 1.
        let listener = overlapped_socket();
        c.ok(listener != INVALID_SOCKET, WSAGetLastError() as u64)?;
        c.eq(bind(listener, &SockaddrIn::new(LOOPBACK, 0), ADDR_LEN) as u64, 0)?;
        c.eq(listen(listener, 5) as u64, 0)?;
        let mut name = SockaddrIn::new([0; 4], 0);
        let mut len = ADDR_LEN;
        c.eq(getsockname(listener, &mut name, &mut len) as u64, 0)?;
        let listen_port = u16::from_be(name.port);
        c.ok(CreateIoCompletionPort(listener as Handle, port, 1, 0) == port, 0)?;

        let accept_ex: Option<AcceptExFn> = extension(listener, &WSAID_ACCEPTEX);
        c.ok(accept_ex.is_some(), WSAGetLastError() as u64)?;
        let connect_ex: Option<ConnectExFn> = extension(listener, &WSAID_CONNECTEX);
        c.ok(connect_ex.is_some(), WSAGetLastError() as u64)?;
        let (accept_ex, connect_ex) = (accept_ex.unwrap_unchecked(), connect_ex.unwrap_unchecked());

        // AcceptEx with nobody connecting: pending.
        let accepted = overlapped_socket();
        c.ok(accepted != INVALID_SOCKET, WSAGetLastError() as u64)?;
        let mut accept_buf = [0u8; 2 * ADDR_BLOCK as usize];
        let mut accept_ov = Overlapped::new();
        let mut bytes = 0u32;
        let r = accept_ex(
            listener,
            accepted,
            accept_buf.as_mut_ptr(),
            0,
            ADDR_BLOCK,
            ADDR_BLOCK,
            &mut bytes,
            &mut accept_ov,
        );
        c.ok(r == 0, r as u64)?;
        c.eq(WSAGetLastError() as u64, ERROR_IO_PENDING.into())?;

        // ConnectEx from a bound socket (key 2).
        let client = overlapped_socket();
        c.ok(client != INVALID_SOCKET, WSAGetLastError() as u64)?;
        c.eq(bind(client, &SockaddrIn::new([0; 4], 0), ADDR_LEN) as u64, 0)?;
        c.ok(CreateIoCompletionPort(client as Handle, port, 2, 0) == port, 0)?;
        let mut connect_ov = Overlapped::new();
        let target = SockaddrIn::new(LOOPBACK, listen_port);
        let r = connect_ex(
            client,
            &target,
            ADDR_LEN,
            core::ptr::null(),
            0,
            &mut bytes,
            &mut connect_ov,
        );
        c.ok(r != 0 || WSAGetLastError() as u32 == ERROR_IO_PENDING, WSAGetLastError() as u64)?;

        // Both complete through the port, with their keys and OVERLAPPEDs.
        let mut seen = 0;
        for _ in 0..2 {
            let p = dequeue(port, 5000);
            c.ok(p.ok, p.error.into())?;
            if p.key == 1 {
                c.ok(core::ptr::eq(p.ov, &raw mut accept_ov), p.ov as u64)?;
                seen |= 1;
            } else {
                c.eq(p.key as u64, 2)?;
                c.ok(core::ptr::eq(p.ov, &raw mut connect_ov), p.ov as u64)?;
                seen |= 2;
            }
        }
        c.eq(seen, 3)?;
        let one: [u8; 4] = 1u32.to_le_bytes();
        let listener_bytes = (listener as u64).to_le_bytes();
        c.eq(
            setsockopt(accepted, SOL_SOCKET, SO_UPDATE_ACCEPT_CONTEXT, listener_bytes.as_ptr(), 8)
                as u64,
            0,
        )?;
        c.eq(
            setsockopt(client, SOL_SOCKET, SO_UPDATE_CONNECT_CONTEXT, core::ptr::null(), 0) as u64,
            0,
        )?;
        c.eq(setsockopt(client, IPPROTO_TCP, TCP_NODELAY, one.as_ptr(), 4) as u64, 0)?;

        // The addresses AcceptEx stored: ours and the client's.
        let (mut local, mut remote) = (core::ptr::null_mut(), core::ptr::null_mut());
        let (mut local_len, mut remote_len) = (0i32, 0i32);
        GetAcceptExSockaddrs(
            accept_buf.as_mut_ptr(),
            0,
            ADDR_BLOCK,
            ADDR_BLOCK,
            &mut local,
            &mut local_len,
            &mut remote,
            &mut remote_len,
        );
        c.ok(!local.is_null() && !remote.is_null(), 0)?;
        let mut client_name = SockaddrIn::new([0; 4], 0);
        len = ADDR_LEN;
        c.eq(getsockname(client, &mut client_name, &mut len) as u64, 0)?;
        c.eq(u16::from_be((*local).port).into(), listen_port.into())?;
        c.ok((*remote).port == client_name.port && (*remote).addr == LOOPBACK, 0)?;

        // Overlapped receive (key 3) pends until the client sends.
        c.ok(CreateIoCompletionPort(accepted as Handle, port, 3, 0) == port, 0)?;
        let mut rbuf = [0u8; 64];
        let mut rwsabuf = WsaBuf { len: 64, buf: rbuf.as_mut_ptr() };
        let (mut received, mut flags) = (0u32, 0u32);
        let mut recv_ov = Overlapped::new();
        let r = WSARecv(accepted, &mut rwsabuf, 1, &mut received, &mut flags, &mut recv_ov, 0);
        c.ok(r != 0 && WSAGetLastError() as u32 == ERROR_IO_PENDING, WSAGetLastError() as u64)?;
        let mut sbuf = *b"hello";
        let mut swsabuf = WsaBuf { len: 5, buf: sbuf.as_mut_ptr() };
        let mut send_ov = Overlapped::new();
        let r = WSASend(client, &mut swsabuf, 1, &mut bytes, 0, &mut send_ov, 0);
        c.ok(r == 0 || WSAGetLastError() as u32 == ERROR_IO_PENDING, WSAGetLastError() as u64)?;
        seen = 0;
        for _ in 0..2 {
            let p = dequeue(port, 5000);
            c.ok(p.ok, p.error.into())?;
            c.eq(p.bytes.into(), 5)?;
            seen |= 1 << p.key;
        }
        c.eq(seen, 1 << 2 | 1 << 3)?;
        c.ok(&rbuf[..5] == b"hello", rbuf[0].into())?;
        c.ok(WSAGetOverlappedResult(accepted, &mut recv_ov, &mut received, 0, &mut flags) != 0, 0)?;
        c.eq(received.into(), 5)?;

        // CancelIoEx: a pending receive ends with ERROR_OPERATION_ABORTED.
        let mut cancel_ov = Overlapped::new();
        let r = WSARecv(client, &mut rwsabuf, 1, &mut received, &mut flags, &mut cancel_ov, 0);
        c.ok(r != 0 && WSAGetLastError() as u32 == ERROR_IO_PENDING, WSAGetLastError() as u64)?;
        c.ok(CancelIoEx(client as Handle, &mut cancel_ov) != 0, GetLastError().into())?;
        let p = dequeue(port, 5000);
        c.ok(!p.ok && p.error == ERROR_OPERATION_ABORTED, p.error.into())?;
        c.ok(core::ptr::eq(p.ov, &raw mut cancel_ov) && p.key == 2, p.key as u64)?;
        let r = WSAGetOverlappedResult(client, &mut cancel_ov, &mut received, 0, &mut flags);
        c.ok(
            r == 0 && WSAGetLastError() as u32 == ERROR_OPERATION_ABORTED,
            WSAGetLastError() as u64,
        )?;
        // Nothing left to cancel.
        c.ok(CancelIoEx(client as Handle, &mut cancel_ov) == 0, 0)?;
        c.eq(GetLastError().into(), ERROR_NOT_FOUND.into())?;

        // Skip mode: a receive that completes at once posts no packet.
        c.ok(
            SetFileCompletionNotificationModes(
                accepted as Handle,
                FILE_SKIP_COMPLETION_PORT_ON_SUCCESS,
            ) != 0,
            GetLastError().into(),
        )?;
        let mut send_ov = Overlapped::new();
        swsabuf.len = 3;
        let r = WSASend(client, &mut swsabuf, 1, &mut bytes, 0, &mut send_ov, 0);
        c.ok(r == 0 || WSAGetLastError() as u32 == ERROR_IO_PENDING, WSAGetLastError() as u64)?;
        let p = dequeue(port, 5000); // the send's packet (key 2: no skip mode)
        c.ok(p.ok && p.key == 2, p.key as u64)?;
        Sleep(200); // the bytes reach the other end
        let mut quick_ov = Overlapped::new();
        received = 0;
        let r = WSARecv(accepted, &mut rwsabuf, 1, &mut received, &mut flags, &mut quick_ov, 0);
        c.eq(r as u64, 0)?;
        c.eq(received.into(), 3)?;
        let p = dequeue(port, 200);
        c.ok(!p.ok && p.ov.is_null() && p.error == WAIT_TIMEOUT, p.error.into())?;

        for s in [accepted, client, listener] {
            c.eq(closesocket(s) as u64, 0)?;
        }
        c.ok(CloseHandle(port) != 0, 0)?;

        // Completion routines (no port): queued to the calling thread, run
        // in an alertable wait (SleepEx returns WAIT_IO_COMPLETION).
        let l2 = overlapped_socket();
        c.eq(bind(l2, &SockaddrIn::new(LOOPBACK, 0), ADDR_LEN) as u64, 0)?;
        c.eq(listen(l2, 1) as u64, 0)?;
        len = ADDR_LEN;
        c.eq(getsockname(l2, &mut name, &mut len) as u64, 0)?;
        let c2 = overlapped_socket();
        c.eq(connect(c2, &name, ADDR_LEN) as u64, 0)?;
        let a2 = accept(l2, core::ptr::null_mut(), core::ptr::null_mut());
        c.ok(a2 != INVALID_SOCKET, WSAGetLastError() as u64)?;
        let mut rbuf2 = [0u8; 16];
        let mut r2 = WsaBuf { len: 16, buf: rbuf2.as_mut_ptr() };
        // A receive that has to wait: the routine runs once data came.
        let mut ov_a = Overlapped::new();
        flags = 0;
        let r = WSARecv(
            a2,
            &mut r2,
            1,
            &mut received,
            &mut flags,
            &mut ov_a,
            done as *const () as usize,
        );
        c.ok(r != 0 && WSAGetLastError() as u32 == ERROR_IO_PENDING, WSAGetLastError() as u64)?;
        c.eq(DONE_COUNT.into(), 0)?;
        let mut sbuf2 = *b"abc";
        let mut s2 = WsaBuf { len: 3, buf: sbuf2.as_mut_ptr() };
        c.eq(WSASend(c2, &mut s2, 1, &mut bytes, 0, core::ptr::null_mut(), 0) as u64, 0)?;
        c.eq(SleepEx(5000, 1).into(), WAIT_IO_COMPLETION.into())?;
        c.eq(DONE_COUNT.into(), 1)?;
        c.ok(DONE_ERROR == 0 && DONE_BYTES == 3, DONE_ERROR.into())?;
        c.ok(core::ptr::eq(DONE_OV, &raw mut ov_a), 0)?;
        c.ok(&rbuf2[..3] == b"abc", rbuf2[0].into())?;
        // One that completes at once returns 0, and its routine is still
        // queued (it runs at the next alertable wait).
        s2.len = 2;
        c.eq(WSASend(c2, &mut s2, 1, &mut bytes, 0, core::ptr::null_mut(), 0) as u64, 0)?;
        Sleep(200);
        let mut ov_b = Overlapped::new();
        let r = WSARecv(
            a2,
            &mut r2,
            1,
            &mut received,
            &mut flags,
            &mut ov_b,
            done as *const () as usize,
        );
        c.ok(r == 0 && received == 2, WSAGetLastError() as u64)?;
        c.eq(DONE_COUNT.into(), 1)?;
        c.eq(SleepEx(0, 1).into(), WAIT_IO_COMPLETION.into())?;
        c.ok(DONE_COUNT == 2 && DONE_BYTES == 2, DONE_COUNT.into())?;
        c.ok(core::ptr::eq(DONE_OV, &raw mut ov_b), 0)?;
        // WSASend with a routine.
        let mut ov_c = Overlapped::new();
        let r = WSASend(c2, &mut s2, 1, &mut bytes, 0, &mut ov_c, done as *const () as usize);
        c.ok(r == 0 || WSAGetLastError() as u32 == ERROR_IO_PENDING, WSAGetLastError() as u64)?;
        c.eq(SleepEx(5000, 1).into(), WAIT_IO_COMPLETION.into())?;
        c.ok(DONE_COUNT == 3 && DONE_BYTES == 2 && DONE_ERROR == 0, DONE_COUNT.into())?;
        c.ok(core::ptr::eq(DONE_OV, &raw mut ov_c), 0)?;
        for s in [a2, c2, l2] {
            c.eq(closesocket(s) as u64, 0)?;
        }

        // WSAEventSelect / WSAEnumNetworkEvents (PostgreSQL's latches):
        // the event is set when a selected network event happens and
        // WSAEnumNetworkEvents reports (and clears) it and resets the event.
        const FD_READ: i32 = 1;
        const FD_ACCEPT: i32 = 8;
        const FD_CLOSE: i32 = 32;
        let ev = CreateEventW(core::ptr::null_mut(), 1, 0, core::ptr::null());
        let l3 = overlapped_socket();
        c.eq(bind(l3, &SockaddrIn::new(LOOPBACK, 0), ADDR_LEN) as u64, 0)?;
        c.eq(listen(l3, 1) as u64, 0)?;
        len = ADDR_LEN;
        c.eq(getsockname(l3, &mut name, &mut len) as u64, 0)?;
        c.eq(WSAEventSelect(l3, ev, FD_ACCEPT | FD_CLOSE) as u64, 0)?;
        c.eq(WaitForSingleObject(ev, 0).into(), WAIT_TIMEOUT.into())?;
        let c3 = overlapped_socket();
        c.eq(connect(c3, &name, ADDR_LEN) as u64, 0)?;
        c.eq(WaitForSingleObject(ev, 5000).into(), 0)?;
        let mut ne = [0i32; 11];
        c.eq(WSAEnumNetworkEvents(l3, ev, ne.as_mut_ptr()) as u64, 0)?;
        c.ok(ne[0] & FD_ACCEPT != 0 && ne[4] == 0, ne[0] as u64)?; // iErrorCode[FD_ACCEPT_BIT]
        c.eq(WaitForSingleObject(ev, 0).into(), WAIT_TIMEOUT.into())?;
        let a3 = accept(l3, core::ptr::null_mut(), core::ptr::null_mut());
        c.ok(a3 != INVALID_SOCKET, WSAGetLastError() as u64)?;
        c.eq(WSAEventSelect(l3, core::ptr::null_mut(), 0) as u64, 0)?;
        c.eq(WSAEventSelect(a3, ev, FD_READ | FD_CLOSE) as u64, 0)?;
        let mut one = *b"x";
        let mut sb = WsaBuf { len: 1, buf: one.as_mut_ptr() };
        c.eq(WSASend(c3, &mut sb, 1, &mut bytes, 0, core::ptr::null_mut(), 0) as u64, 0)?;
        c.eq(WaitForSingleObject(ev, 5000).into(), 0)?;
        c.eq(WSAEnumNetworkEvents(a3, ev, ne.as_mut_ptr()) as u64, 0)?;
        c.ok(ne[0] & FD_READ != 0, ne[0] as u64)?;
        let mut rb = WsaBuf { len: 16, buf: rbuf2.as_mut_ptr() };
        flags = 0;
        c.eq(
            WSARecv(a3, &mut rb, 1, &mut received, &mut flags, core::ptr::null_mut(), 0) as u64,
            0,
        )?;
        c.eq(received.into(), 1)?;
        c.eq(closesocket(c3) as u64, 0)?;
        c.eq(WaitForSingleObject(ev, 5000).into(), 0)?;
        c.eq(WSAEnumNetworkEvents(a3, ev, ne.as_mut_ptr()) as u64, 0)?;
        c.ok(ne[0] & FD_CLOSE != 0, ne[0] as u64)?;
        for s in [a3, l3] {
            c.eq(closesocket(s) as u64, 0)?;
        }
        CloseHandle(ev);
        c.eq(WSACleanup() as u64, 0)?;
    }
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = match run() {
        Ok(()) => {
            out(b"winsock_async_test: ok\r\n");
            0
        }
        Err(n) => n,
    };
    // SAFETY: ends the process.
    unsafe { ExitProcess(code) }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdead) }
}

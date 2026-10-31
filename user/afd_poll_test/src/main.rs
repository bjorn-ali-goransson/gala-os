//! AFD polls as wepoll (the JDK's NIO selector on Windows, libuv, ...)
//! makes them, with the results real Windows gives: an
//! IOCTL_AFD_POLL on a helper handle (`\Device\Afd\Wepoll`) per socket,
//! several pending at once on the same helper; each completes on its own
//! when its socket gets one of the events asked for (a listener's
//! AFD_POLL_ACCEPT does not wait for another socket's AFD_POLL_RECEIVE),
//! reporting the socket and the events. Layouts from Wine's afd.h
//! (AFD_POLL_INFO: Timeout, NumberOfHandles, Exclusive, then
//! {Handle, Events, Status}).
//!
//! Prints `afd_poll_test: ok` or exits with the number of the failing
//! check.

#![no_std]
#![no_main]

use core::ffi::c_void;

type Handle = *mut c_void;
type Socket = usize;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut c_void) -> i32;
    fn ExitProcess(code: u32) -> !;
    fn CreateEventW(security: *mut c_void, manual: i32, initial: i32, name: *const u16) -> Handle;
    fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
}

#[repr(C)]
struct WsaData {
    version: u16,
    high_version: u16,
    max_sockets: u16,
    max_udp_dg: u16,
    vendor_info: *mut u8,
    description: [u8; 257],
    system_status: [u8; 129],
}

#[repr(C)]
struct SockaddrIn {
    family: u16,
    port: u16,
    addr: [u8; 4],
    zero: [u8; 8],
}

#[link(name = "ws2_32", kind = "raw-dylib")]
unsafe extern "system" {
    fn WSAStartup(version: u16, data: *mut WsaData) -> i32;
    fn socket(af: i32, kind: i32, protocol: i32) -> Socket;
    fn bind(s: Socket, addr: *const SockaddrIn, len: i32) -> i32;
    fn listen(s: Socket, backlog: i32) -> i32;
    fn connect(s: Socket, addr: *const SockaddrIn, len: i32) -> i32;
    fn accept(s: Socket, addr: *mut SockaddrIn, len: *mut i32) -> Socket;
    fn getsockname(s: Socket, addr: *mut SockaddrIn, len: *mut i32) -> i32;
    fn send(s: Socket, buf: *const u8, len: i32, flags: i32) -> i32;
}

#[repr(C)]
struct UnicodeString {
    length: u16,
    maximum: u16,
    buffer: *const u16,
}

#[repr(C)]
struct ObjectAttributes {
    length: u32,
    root: usize,
    name: *const UnicodeString,
    attributes: u32,
    security: usize,
    qos: usize,
}

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn NtCreateFile(
        out: *mut Handle,
        access: u32,
        attributes: *const ObjectAttributes,
        iosb: *mut [usize; 2],
        size: *const i64,
        file_attributes: u32,
        share: u32,
        disposition: u32,
        options: u32,
        ea: *const c_void,
        ea_len: u32,
    ) -> u32;
    fn NtDeviceIoControlFile(
        h: Handle,
        event: Handle,
        apc: usize,
        apc_context: usize,
        iosb: *mut [usize; 2],
        code: u32,
        input: *const u8,
        input_len: u32,
        output: *mut u8,
        output_len: u32,
    ) -> u32;
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

const IOCTL_AFD_POLL: u32 = 0x12024;
const AFD_POLL_RECEIVE: u32 = 0x1;
const AFD_POLL_ACCEPT: u32 = 0x80;
const STATUS_PENDING: u64 = 0x103;
const WAIT_TIMEOUT: u64 = 0x102;

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
        out(b"afd_poll_test: check ");
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

/// AFD_POLL_INFO for one socket (32 bytes), and its IO_STATUS_BLOCK.
#[repr(C, align(8))]
struct Poll {
    info: [u8; 32],
    iosb: [usize; 2],
}

impl Poll {
    fn new(s: Socket, events: u32) -> Poll {
        let mut info = [0u8; 32];
        info[0..8].copy_from_slice(&i64::MAX.to_le_bytes()); // Timeout: none
        info[8..12].copy_from_slice(&1u32.to_le_bytes()); // NumberOfHandles
        info[16..24].copy_from_slice(&(s as u64).to_le_bytes());
        info[24..28].copy_from_slice(&events.to_le_bytes());
        Poll { info, iosb: [0; 2] }
    }

    fn handles(&self) -> u32 {
        u32::from_le_bytes([self.info[8], self.info[9], self.info[10], self.info[11]])
    }

    fn handle(&self) -> u64 {
        u64::from_le_bytes(self.info[16..24].try_into().unwrap_or([0; 8]))
    }

    fn events(&self) -> u32 {
        u32::from_le_bytes([self.info[24], self.info[25], self.info[26], self.info[27]])
    }
}

/// Submits `p` on `helper`, signaling `event` when it completes.
fn submit(helper: Handle, event: Handle, p: &mut Poll) -> u64 {
    let info = p.info.as_mut_ptr();
    // SAFETY: the poll's input and output are its own buffer.
    unsafe {
        NtDeviceIoControlFile(helper, event, 0, 0, &mut p.iosb, IOCTL_AFD_POLL, info, 32, info, 32)
            .into()
    }
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    // SAFETY: Winsock and ntdll calls with buffers of ours.
    unsafe {
        let mut data: WsaData = core::mem::zeroed();
        c.eq(WSAStartup(0x0202, &mut data) as u64, 0)?;
        let listener = socket(2, 1, 6);
        let any = SockaddrIn { family: 2, port: 0, addr: [127, 0, 0, 1], zero: [0; 8] };
        c.eq(bind(listener, &any, 16) as u64, 0)?;
        c.eq(listen(listener, 4) as u64, 0)?;
        let mut addr = SockaddrIn { family: 0, port: 0, addr: [0; 4], zero: [0; 8] };
        let mut len = 16;
        c.eq(getsockname(listener, &mut addr, &mut len) as u64, 0)?;
        // A connected pair: `client` and `server`.
        let client = socket(2, 1, 6);
        c.eq(connect(client, &addr, 16) as u64, 0)?;
        let server = accept(listener, core::ptr::null_mut(), core::ptr::null_mut());
        c.ok(server != !0, 0)?;

        // The helper handle wepoll opens.
        let name: [u16; 18] = b"\\Device\\Afd\\Wepoll".map(u16::from);
        let us = UnicodeString { length: 36, maximum: 36, buffer: name.as_ptr() };
        let oa = ObjectAttributes {
            length: size_of::<ObjectAttributes>() as u32,
            root: 0,
            name: &us,
            attributes: 0,
            security: 0,
            qos: 0,
        };
        let mut helper = core::ptr::null_mut();
        let mut iosb = [0usize; 2];
        let r = NtCreateFile(
            &mut helper,
            0x0010_0000, // SYNCHRONIZE
            &oa,
            &mut iosb,
            core::ptr::null(),
            0,
            3, // FILE_SHARE_READ | FILE_SHARE_WRITE
            1, // FILE_OPEN
            0,
            core::ptr::null(),
            0,
        );
        c.eq(r.into(), 0)?;

        // Two polls pending on the helper: the pair's server side (nothing
        // to read yet) first, then the listener.
        let read_event = CreateEventW(core::ptr::null_mut(), 1, 0, core::ptr::null());
        let accept_event = CreateEventW(core::ptr::null_mut(), 1, 0, core::ptr::null());
        let mut read_poll = Poll::new(server, AFD_POLL_RECEIVE);
        let mut accept_poll = Poll::new(listener, AFD_POLL_ACCEPT);
        c.eq(submit(helper, read_event, &mut read_poll), STATUS_PENDING)?;
        c.eq(submit(helper, accept_event, &mut accept_poll), STATUS_PENDING)?;

        // A connection: the listener's poll completes, the other does not.
        let second = socket(2, 1, 6);
        c.eq(connect(second, &addr, 16) as u64, 0)?;
        c.eq(WaitForSingleObject(accept_event, 5000).into(), 0)?;
        c.eq(accept_poll.iosb[0] as u64, 0)?;
        c.eq(accept_poll.handles().into(), 1)?;
        c.eq(accept_poll.handle(), listener as u64)?;
        c.ok(accept_poll.events() & AFD_POLL_ACCEPT != 0, accept_poll.events().into())?;
        c.eq(WaitForSingleObject(read_event, 100).into(), WAIT_TIMEOUT)?;

        // Data on the pair: now the first completes too.
        c.eq(send(client, b"x".as_ptr(), 1, 0) as u64, 1)?;
        c.eq(WaitForSingleObject(read_event, 5000).into(), 0)?;
        c.eq(read_poll.handle(), server as u64)?;
        c.ok(read_poll.events() & AFD_POLL_RECEIVE != 0, read_poll.events().into())?;
    }
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = match run() {
        Ok(()) => {
            out(b"afd_poll_test: ok\r\n");
            0
        }
        Err(id) => id,
    };
    // SAFETY: ends the process.
    unsafe { ExitProcess(code) }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdead) }
}

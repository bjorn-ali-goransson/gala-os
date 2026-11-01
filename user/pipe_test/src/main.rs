//! Anonymous pipes through kernel32, with the results real Windows gives
//! (from the CreatePipe, ReadFile, PeekNamedPipe and GetFileType docs):
//! - CreatePipe makes a read end and a write end; WriteFile writes all its
//!   bytes, ReadFile returns what is there (not the whole buffer);
//! - PeekNamedPipe copies without removing and reports the bytes waiting;
//! - after the write end is closed, the rest is read, then ReadFile fails
//!   with ERROR_BROKEN_PIPE; writing with the read end closed fails;
//! - GetFileType is FILE_TYPE_PIPE; ends created inheritable report
//!   HANDLE_FLAG_INHERIT, which SetHandleInformation clears;
//! - 1 MiB written by one thread arrives intact at another (writes wait
//!   while the pipe is full).
//!
//! Named pipe servers (the CreateNamedPipe, ConnectNamedPipe,
//! DisconnectNamedPipe, SetNamedPipeHandleState and CreateFile docs):
//! - opening a pipe no server created fails with ERROR_FILE_NOT_FOUND; a
//!   client may connect before ConnectNamedPipe, which then fails with
//!   ERROR_PIPE_CONNECTED; with every instance taken, CreateFile fails
//!   with ERROR_PIPE_BUSY, and so does creating more than nMaxInstances
//!   instances; FILE_FLAG_FIRST_PIPE_INSTANCE fails with
//!   ERROR_ACCESS_DENIED once an instance exists;
//! - message mode keeps each write a message: a read returns one, a read
//!   into a smaller buffer fails with ERROR_MORE_DATA and the rest
//!   follows; a client starts in byte read mode and SetNamedPipeHandleState
//!   switches it; PeekNamedPipe reports the bytes left of the message;
//! - ImpersonateNamedPipeClient on a connected server end once it has
//!   read: the thread gets a token (OpenThreadToken succeeds) until
//!   RevertToSelf; on a handle that is not a pipe it fails;
//! - after the client closes, the server's read fails with
//!   ERROR_BROKEN_PIPE and ConnectNamedPipe with ERROR_NO_DATA until
//!   DisconnectNamedPipe;
//! - an overlapped server: ConnectNamedPipe and ReadFile pend
//!   (ERROR_IO_PENDING) and complete when a client connects and writes,
//!   through the event and the completion port;
//!   SetFileCompletionNotificationModes(FILE_SKIP_COMPLETION_PORT_ON_SUCCESS)
//!   leaves the port alone for a read that completes at once.
//! - the ANSI CreateNamedPipeA and CreateFileA, as Win32-OpenSSH's pipe()
//!   uses them: an overlapped duplex pipe whose ends are pipes (GetFileType)
//!   and inheritable; a read pending on its server end fails with
//!   ERROR_BROKEN_PIPE once every copy of the client end (DuplicateHandle)
//!   is closed without writing (and ReadFileEx's completion routine runs
//!   with that error); CallNamedPipeA writes a message to a message pipe and
//!   reads its answer (ERROR_FILE_NOT_FOUND without a server); the
//!   server's FlushFileBuffers waits until the client has read it, so the
//!   DisconnectNamedPipe after it does not take the answer away.
//!
//! Prints `pipe_test: ok` or exits with the number of the failing check.

#![no_std]
#![no_main]

use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, Ordering};

type Bool = i32;
type Handle = *mut c_void;

#[repr(C)]
struct SecurityAttributes {
    length: u32,
    descriptor: *mut c_void,
    inherit: Bool,
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut c_void) -> Bool;
    fn ReadFile(h: Handle, buf: *mut u8, len: u32, read: *mut u32, ov: *mut c_void) -> Bool;
    fn CreatePipe(
        read: *mut Handle,
        write: *mut Handle,
        attributes: *const SecurityAttributes,
        size: u32,
    ) -> Bool;
    fn PeekNamedPipe(
        h: Handle,
        buf: *mut u8,
        size: u32,
        read: *mut u32,
        available: *mut u32,
        left_this_message: *mut u32,
    ) -> Bool;
    fn GetFileType(h: Handle) -> u32;
    fn GetHandleInformation(h: Handle, flags: *mut u32) -> Bool;
    fn SetHandleInformation(h: Handle, mask: u32, flags: u32) -> Bool;
    fn CreateThread(
        attributes: *mut c_void,
        stack: usize,
        start: extern "system" fn(*mut c_void) -> u32,
        parameter: *mut c_void,
        flags: u32,
        id: *mut u32,
    ) -> Handle;
    fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
    fn GetExitCodeThread(h: Handle, code: *mut u32) -> Bool;
    fn GetLastError() -> u32;
    fn CloseHandle(h: Handle) -> Bool;
    fn CreateNamedPipeW(
        name: *const u16,
        open_mode: u32,
        pipe_mode: u32,
        max_instances: u32,
        out_size: u32,
        in_size: u32,
        timeout: u32,
        attributes: *const SecurityAttributes,
    ) -> Handle;
    fn ConnectNamedPipe(h: Handle, ov: *mut Overlapped) -> Bool;
    fn DisconnectNamedPipe(h: Handle) -> Bool;
    fn SetNamedPipeHandleState(
        h: Handle,
        mode: *const u32,
        max_collection: *const u32,
        timeout: *const u32,
    ) -> Bool;
    fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        attributes: *const SecurityAttributes,
        disposition: u32,
        flags: u32,
        template: Handle,
    ) -> Handle;
    fn CreateNamedPipeA(
        name: *const u8,
        open_mode: u32,
        pipe_mode: u32,
        max_instances: u32,
        out_size: u32,
        in_size: u32,
        timeout: u32,
        attributes: *const SecurityAttributes,
    ) -> Handle;
    fn CreateFileA(
        name: *const u8,
        access: u32,
        share: u32,
        attributes: *const SecurityAttributes,
        disposition: u32,
        flags: u32,
        template: Handle,
    ) -> Handle;
    fn CreateEventW(
        attributes: *const c_void,
        manual: Bool,
        initial: Bool,
        name: *const u16,
    ) -> Handle;
    fn GetOverlappedResult(h: Handle, ov: *mut Overlapped, n: *mut u32, wait: Bool) -> Bool;
    fn GetCurrentProcess() -> Handle;
    fn FlushFileBuffers(h: Handle) -> Bool;
    fn CallNamedPipeA(
        name: *const u8,
        input: *const u8,
        input_size: u32,
        output: *mut u8,
        output_size: u32,
        read: *mut u32,
        timeout: u32,
    ) -> Bool;
    fn SleepEx(ms: u32, alertable: Bool) -> u32;
    fn ReadFileEx(
        h: Handle,
        buf: *mut u8,
        len: u32,
        ov: *mut Overlapped,
        routine: extern "system" fn(u32, u32, *mut Overlapped),
    ) -> Bool;
    fn DuplicateHandle(
        source_process: Handle,
        source: Handle,
        target_process: Handle,
        target: *mut Handle,
        access: u32,
        inherit: Bool,
        options: u32,
    ) -> Bool;
    fn SetFileCompletionNotificationModes(h: Handle, flags: u8) -> i32;
    fn CreateIoCompletionPort(file: Handle, port: Handle, key: usize, threads: u32) -> Handle;
    fn GetQueuedCompletionStatus(
        port: Handle,
        n: *mut u32,
        key: *mut usize,
        ov: *mut *mut Overlapped,
        ms: u32,
    ) -> Bool;
    fn ExitProcess(code: u32) -> !;
}

#[repr(C)]
struct Overlapped {
    internal: usize,
    internal_high: usize,
    offset: u32,
    offset_high: u32,
    event: Handle,
}

#[link(name = "advapi32", kind = "raw-dylib")]
unsafe extern "system" {
    fn ImpersonateNamedPipeClient(pipe: Handle) -> i32;
    fn RevertToSelf() -> i32;
    fn OpenThreadToken(thread: Handle, access: u32, as_self: i32, token: *mut Handle) -> i32;
    fn OpenProcessToken(process: Handle, access: u32, token: *mut Handle) -> i32;
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

const ERROR_BROKEN_PIPE: u32 = 109;
const ERROR_FILE_NOT_FOUND: u32 = 2;
const ERROR_ACCESS_DENIED: u32 = 5;
const ERROR_PIPE_BUSY: u32 = 231;
const ERROR_NO_DATA: u32 = 232;
const ERROR_MORE_DATA: u32 = 234;
const ERROR_PIPE_CONNECTED: u32 = 535;
const ERROR_IO_PENDING: u32 = 997;
const PIPE_ACCESS_DUPLEX: u32 = 3;
const FILE_FLAG_FIRST_PIPE_INSTANCE: u32 = 0x0008_0000;
const FILE_FLAG_OVERLAPPED: u32 = 0x4000_0000;
const PIPE_TYPE_MESSAGE: u32 = 4;
const PIPE_READMODE_MESSAGE: u32 = 2;
const PIPE_UNLIMITED_INSTANCES: u32 = 255;
const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const OPEN_EXISTING: u32 = 3;
const INVALID_HANDLE: Handle = usize::MAX as Handle;
const FILE_TYPE_PIPE: u32 = 3;
const HANDLE_FLAG_INHERIT: u32 = 1;
/// Bytes the writer thread sends through the pipe.
const BULK: u32 = 1 << 20;

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
        out(b"pipe_test: check ");
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

/// The byte at position `i` of the bulk stream.
fn pattern(i: u32) -> u8 {
    (i.wrapping_mul(2_654_435_761) >> 24) as u8
}

/// Writes BULK bytes of `pattern` in uneven pieces, then closes the end.
extern "system" fn writer(h: *mut c_void) -> u32 {
    let mut buf = [0u8; 3000];
    let mut sent = 0u32;
    while sent < BULK {
        let n = (BULK - sent).min(buf.len() as u32);
        for (k, b) in buf[..n as usize].iter_mut().enumerate() {
            *b = pattern(sent + k as u32);
        }
        let mut written = 0;
        // SAFETY: valid buffer; the write end handle.
        if unsafe { WriteFile(h, buf.as_ptr(), n, &mut written, core::ptr::null_mut()) } == 0
            || written != n
        {
            return 1;
        }
        sent += n;
    }
    // SAFETY: the write end, no longer used.
    unsafe { CloseHandle(h) };
    0
}

/// `text` as a NUL-terminated UTF-16 string.
fn wide(text: &str) -> [u16; 64] {
    let mut w = [0u16; 64];
    for (d, b) in w.iter_mut().zip(text.bytes()) {
        *d = u16::from(b);
    }
    w
}

/// Opens the client end of pipe `name` (read and write, blocking).
fn client(name: &[u16; 64], flags: u32) -> Handle {
    // SAFETY: a NUL-terminated name; no template.
    unsafe {
        CreateFileW(
            name.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            0,
            core::ptr::null(),
            OPEN_EXISTING,
            flags,
            core::ptr::null_mut(),
        )
    }
}

fn write(h: Handle, data: &[u8]) -> bool {
    let mut n = 0;
    // SAFETY: valid buffer and count.
    let ok =
        unsafe { WriteFile(h, data.as_ptr(), data.len() as u32, &mut n, core::ptr::null_mut()) };
    ok != 0 && n == data.len() as u32
}

/// Named pipe servers and clients (see the module comment).
fn named(c: &mut Checks) -> Result<(), u32> {
    let name = wide("\\\\.\\pipe\\gala_test\\one");
    let none = wide("\\\\.\\pipe\\gala_test\\none");
    // SAFETY: documented kernel32 calls with valid arguments.
    unsafe {
        let h = client(&none, 0);
        c.ok(h == INVALID_HANDLE, h as u64)?;
        c.eq(GetLastError().into(), ERROR_FILE_NOT_FOUND.into())?;

        let mode = PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE;
        let server = CreateNamedPipeW(
            name.as_ptr(),
            PIPE_ACCESS_DUPLEX,
            mode,
            2,
            4096,
            4096,
            0,
            core::ptr::null(),
        );
        c.ok(server != INVALID_HANDLE, GetLastError().into())?;
        c.eq(GetFileType(server).into(), FILE_TYPE_PIPE.into())?;
        // A client takes the only instance; creating more is limited.
        let cl = client(&name, 0);
        c.ok(cl != INVALID_HANDLE, GetLastError().into())?;
        let first = FILE_FLAG_FIRST_PIPE_INSTANCE | PIPE_ACCESS_DUPLEX;
        let h = CreateNamedPipeW(name.as_ptr(), first, mode, 2, 4096, 4096, 0, core::ptr::null());
        c.ok(h == INVALID_HANDLE, h as u64)?;
        c.eq(GetLastError().into(), ERROR_ACCESS_DENIED.into())?;
        let second = CreateNamedPipeW(
            name.as_ptr(),
            PIPE_ACCESS_DUPLEX,
            mode,
            2,
            4096,
            4096,
            0,
            core::ptr::null(),
        );
        c.ok(second != INVALID_HANDLE, GetLastError().into())?;
        let h = CreateNamedPipeW(
            name.as_ptr(),
            PIPE_ACCESS_DUPLEX,
            mode,
            2,
            4096,
            4096,
            0,
            core::ptr::null(),
        );
        c.ok(h == INVALID_HANDLE, h as u64)?;
        c.eq(GetLastError().into(), ERROR_PIPE_BUSY.into())?;
        // A second client takes the second; a third finds none free.
        let cl2 = client(&name, 0);
        c.ok(cl2 != INVALID_HANDLE, GetLastError().into())?;
        let h = client(&name, 0);
        c.ok(h == INVALID_HANDLE, h as u64)?;
        c.eq(GetLastError().into(), ERROR_PIPE_BUSY.into())?;
        CloseHandle(cl2);
        CloseHandle(second);
        c.eq(ConnectNamedPipe(server, core::ptr::null_mut()) as u64, 0)?;
        c.eq(GetLastError().into(), ERROR_PIPE_CONNECTED.into())?;

        // Messages towards the server.
        c.ok(write(cl, b"one") && write(cl, b"three"), GetLastError().into())?;
        let (mut buf, mut got) = ([0u8; 64], 0u32);
        c.ok(ReadFile(server, buf.as_mut_ptr(), 64, &mut got, core::ptr::null_mut()) != 0, 0)?;
        c.eq(got.into(), 3)?;
        c.ok(&buf[..3] == b"one", buf[0].into())?;
        let ok = ReadFile(server, buf.as_mut_ptr(), 2, &mut got, core::ptr::null_mut());
        c.eq(ok as u64, 0)?;
        c.eq(GetLastError().into(), ERROR_MORE_DATA.into())?;
        c.eq(got.into(), 2)?;
        c.ok(&buf[..2] == b"th", buf[0].into())?;
        c.ok(ReadFile(server, buf.as_mut_ptr(), 64, &mut got, core::ptr::null_mut()) != 0, 0)?;
        c.eq(got.into(), 3)?;
        c.ok(&buf[..3] == b"ree", buf[0].into())?;

        // The server impersonates its client (after reading from it).
        let mut token = core::ptr::null_mut();
        c.eq(OpenThreadToken(-2isize as Handle, 8, 1, &mut token) as u64, 0)?;
        c.ok(ImpersonateNamedPipeClient(server) != 0, GetLastError().into())?;
        c.ok(OpenThreadToken(-2isize as Handle, 8, 1, &mut token) != 0, GetLastError().into())?;
        CloseHandle(token);
        c.ok(RevertToSelf() != 0, 0)?;
        c.eq(OpenThreadToken(-2isize as Handle, 8, 1, &mut token) as u64, 0)?;
        let mut process_token = core::ptr::null_mut();
        c.ok(OpenProcessToken(-1isize as Handle, 8, &mut process_token) != 0, 0)?;
        c.eq(ImpersonateNamedPipeClient(process_token) as u64, 0)?;
        CloseHandle(process_token);

        // Messages towards the client, once it reads in message mode.
        let read_mode = PIPE_READMODE_MESSAGE;
        let set = SetNamedPipeHandleState(cl, &read_mode, core::ptr::null(), core::ptr::null());
        c.ok(set != 0, GetLastError().into())?;
        c.ok(write(server, b"xyz") && write(server, b"ab"), GetLastError().into())?;
        let (mut avail, mut left) = (0, 0);
        let peeked = PeekNamedPipe(cl, buf.as_mut_ptr(), 1, &mut got, &mut avail, &mut left);
        c.ok(peeked != 0, GetLastError().into())?;
        c.eq(got.into(), 1)?;
        c.eq(avail.into(), 5)?;
        c.eq(left.into(), 2)?;
        c.ok(ReadFile(cl, buf.as_mut_ptr(), 64, &mut got, core::ptr::null_mut()) != 0, 0)?;
        c.eq(got.into(), 3)?;
        c.ok(ReadFile(cl, buf.as_mut_ptr(), 64, &mut got, core::ptr::null_mut()) != 0, 0)?;
        c.eq(got.into(), 2)?;
        c.ok(&buf[..2] == b"ab", buf[0].into())?;

        // The client leaves: broken pipe, then no data until disconnected.
        CloseHandle(cl);
        let ok = ReadFile(server, buf.as_mut_ptr(), 64, &mut got, core::ptr::null_mut());
        c.eq(ok as u64, 0)?;
        c.eq(GetLastError().into(), ERROR_BROKEN_PIPE.into())?;
        c.eq(ConnectNamedPipe(server, core::ptr::null_mut()) as u64, 0)?;
        c.eq(GetLastError().into(), ERROR_NO_DATA.into())?;
        c.ok(DisconnectNamedPipe(server) != 0, GetLastError().into())?;
        CloseHandle(server);

        // An overlapped server: the connection and a read pend.
        let name = wide("\\\\.\\pipe\\gala_test\\two");
        let open = PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED;
        let server = CreateNamedPipeW(
            name.as_ptr(),
            open,
            0,
            PIPE_UNLIMITED_INSTANCES,
            4096,
            4096,
            0,
            core::ptr::null(),
        );
        c.ok(server != INVALID_HANDLE, GetLastError().into())?;
        let port = CreateIoCompletionPort(server, core::ptr::null_mut(), 77, 0);
        c.ok(!port.is_null(), GetLastError().into())?;
        let event = CreateEventW(core::ptr::null(), 1, 0, core::ptr::null());
        let mut ov = Overlapped { internal: 0, internal_high: 0, offset: 0, offset_high: 0, event };
        c.eq(ConnectNamedPipe(server, &mut ov) as u64, 0)?;
        c.eq(GetLastError().into(), ERROR_IO_PENDING.into())?;
        c.eq(WaitForSingleObject(event, 0).into(), 0x102)?; // WAIT_TIMEOUT
        let cl = client(&name, 0);
        c.ok(cl != INVALID_HANDLE, GetLastError().into())?;
        c.eq(WaitForSingleObject(event, 5000).into(), 0)?;
        c.ok(GetOverlappedResult(server, &mut ov, &mut got, 0) != 0, GetLastError().into())?;
        let (mut key, mut done) = (0usize, core::ptr::null_mut());
        let dequeued = GetQueuedCompletionStatus(port, &mut got, &mut key, &mut done, 5000);
        c.ok(dequeued != 0, GetLastError().into())?;
        c.eq(key as u64, 77)?;
        c.ok(core::ptr::eq(done, &ov), done as u64)?;

        let mut ov = Overlapped { internal: 0, internal_high: 0, offset: 0, offset_high: 0, event };
        let ok =
            ReadFile(server, buf.as_mut_ptr(), 64, &mut got, (&mut ov as *mut Overlapped).cast());
        c.eq(ok as u64, 0)?;
        c.eq(GetLastError().into(), ERROR_IO_PENDING.into())?;
        c.ok(write(cl, b"late"), GetLastError().into())?;
        c.ok(GetOverlappedResult(server, &mut ov, &mut got, 1) != 0, GetLastError().into())?;
        c.eq(got.into(), 4)?;
        c.ok(&buf[..4] == b"late", buf[0].into())?;
        let dequeued = GetQueuedCompletionStatus(port, &mut got, &mut key, &mut done, 5000);
        c.ok(dequeued != 0 && got == 4, got.into())?;

        // FILE_SKIP_COMPLETION_PORT_ON_SUCCESS: a read that completes at
        // once posts no packet.
        c.ok(SetFileCompletionNotificationModes(server, 1) != 0, GetLastError().into())?;
        c.ok(write(cl, b"now"), GetLastError().into())?;
        let mut ov = Overlapped { internal: 0, internal_high: 0, offset: 0, offset_high: 0, event };
        let ok =
            ReadFile(server, buf.as_mut_ptr(), 64, &mut got, (&mut ov as *mut Overlapped).cast());
        c.ok(ok != 0 && got == 3, GetLastError().into())?;
        let dequeued = GetQueuedCompletionStatus(port, &mut got, &mut key, &mut done, 0);
        c.eq(dequeued as u64, 0)?;
        c.eq(GetLastError().into(), 258)?; // WAIT_TIMEOUT
        CloseHandle(cl);
        CloseHandle(server);
        CloseHandle(port);
        CloseHandle(event);
    }
    Ok(())
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    named(&mut c)?;
    // SAFETY: documented kernel32 calls with valid arguments.
    unsafe {
        let (mut r, mut w) = (core::ptr::null_mut(), core::ptr::null_mut());
        c.ok(CreatePipe(&mut r, &mut w, core::ptr::null(), 0) != 0, GetLastError().into())?;
        c.eq(GetFileType(r).into(), FILE_TYPE_PIPE.into())?;
        c.eq(GetFileType(w).into(), FILE_TYPE_PIPE.into())?;

        // Write, peek (copies, keeps), read (what is there).
        let mut n = 0;
        c.ok(WriteFile(w, b"hello".as_ptr(), 5, &mut n, core::ptr::null_mut()) != 0, 0)?;
        c.eq(n.into(), 5)?;
        let (mut buf, mut got, mut avail, mut left) = ([0u8; 64], 0, 0, 0xff);
        let peeked = PeekNamedPipe(r, buf.as_mut_ptr(), 3, &mut got, &mut avail, &mut left);
        c.ok(peeked != 0, GetLastError().into())?;
        c.eq(got.into(), 3)?;
        c.eq(avail.into(), 5)?;
        c.eq(left.into(), 0)?; // byte mode: no message boundaries
        c.ok(&buf[..3] == b"hel", buf[0].into())?;
        let peeked = PeekNamedPipe(
            r,
            core::ptr::null_mut(),
            0,
            core::ptr::null_mut(),
            &mut avail,
            core::ptr::null_mut(),
        );
        c.ok(peeked != 0 && avail == 5, avail.into())?;
        c.ok(ReadFile(r, buf.as_mut_ptr(), 64, &mut got, core::ptr::null_mut()) != 0, 0)?;
        c.eq(got.into(), 5)?;
        c.ok(&buf[..5] == b"hello", buf[0].into())?;

        // The rest after the write end closes, then ERROR_BROKEN_PIPE.
        c.ok(WriteFile(w, b"abc".as_ptr(), 3, &mut n, core::ptr::null_mut()) != 0, 0)?;
        c.ok(CloseHandle(w) != 0, 0)?;
        c.ok(ReadFile(r, buf.as_mut_ptr(), 64, &mut got, core::ptr::null_mut()) != 0, 0)?;
        c.eq(got.into(), 3)?;
        c.ok(&buf[..3] == b"abc", buf[0].into())?;
        let ok = ReadFile(r, buf.as_mut_ptr(), 64, &mut got, core::ptr::null_mut());
        c.eq(ok as u64, 0)?;
        c.eq(GetLastError().into(), ERROR_BROKEN_PIPE.into())?;
        c.eq(got.into(), 0)?;
        CloseHandle(r);

        // Writing with the read end closed fails.
        c.ok(CreatePipe(&mut r, &mut w, core::ptr::null(), 0) != 0, GetLastError().into())?;
        CloseHandle(r);
        let ok = WriteFile(w, b"x".as_ptr(), 1, &mut n, core::ptr::null_mut());
        c.eq(ok as u64, 0)?;
        CloseHandle(w);

        // Inheritable ends; SetHandleInformation clears the flag.
        let inherit =
            SecurityAttributes { length: 24, descriptor: core::ptr::null_mut(), inherit: 1 };
        c.ok(CreatePipe(&mut r, &mut w, &inherit, 0) != 0, GetLastError().into())?;
        let mut flags = 0;
        c.ok(GetHandleInformation(r, &mut flags) != 0, GetLastError().into())?;
        c.eq((flags & HANDLE_FLAG_INHERIT).into(), HANDLE_FLAG_INHERIT.into())?;
        c.ok(SetHandleInformation(r, HANDLE_FLAG_INHERIT, 0) != 0, GetLastError().into())?;
        c.ok(GetHandleInformation(r, &mut flags) != 0, GetLastError().into())?;
        c.eq((flags & HANDLE_FLAG_INHERIT).into(), 0)?;
        c.ok(GetHandleInformation(w, &mut flags) != 0, GetLastError().into())?;
        c.eq((flags & HANDLE_FLAG_INHERIT).into(), HANDLE_FLAG_INHERIT.into())?;

        // 1 MiB from a writer thread; reads take what is there until EOF.
        let mut tid = 0;
        let t = CreateThread(core::ptr::null_mut(), 0, writer, w, 0, &mut tid);
        c.ok(!t.is_null(), GetLastError().into())?;
        let mut total = 0u32;
        let mut wrong = 0u32;
        let mut chunk = [0u8; 2000];
        loop {
            if ReadFile(r, chunk.as_mut_ptr(), 2000, &mut got, core::ptr::null_mut()) == 0 {
                break;
            }
            for (k, &b) in chunk[..got as usize].iter().enumerate() {
                if b != pattern(total + k as u32) {
                    wrong += 1;
                }
            }
            total += got;
        }
        c.eq(GetLastError().into(), ERROR_BROKEN_PIPE.into())?;
        c.eq(total.into(), BULK.into())?;
        c.eq(wrong.into(), 0)?;
        c.eq(WaitForSingleObject(t, 10_000).into(), 0)?;
        let mut code = 0xff;
        c.ok(GetExitCodeThread(t, &mut code) != 0 && code == 0, code.into())?;
        CloseHandle(t);
        CloseHandle(r);

        // pipe() as Win32-OpenSSH makes it: an overlapped duplex byte pipe
        // through the ANSI functions, both ends inheritable.
        let name = b"\\\\.\\Pipe\\W32PosixPipe.00000001.00000002\0";
        let server = CreateNamedPipeA(
            name.as_ptr(),
            PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED,
            0,
            1,
            4096,
            4096,
            0,
            &inherit,
        );
        c.ok(server as isize != -1, GetLastError().into())?;
        let client = CreateFileA(
            name.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            0,
            &inherit,
            OPEN_EXISTING,
            0x80 | FILE_FLAG_OVERLAPPED,
            core::ptr::null_mut(),
        );
        c.ok(client as isize != -1, GetLastError().into())?;
        c.eq(GetFileType(server).into(), 3)?;
        c.eq(GetFileType(client).into(), 3)?;
        c.ok(GetHandleInformation(client, &mut flags) != 0, GetLastError().into())?;
        c.eq((flags & HANDLE_FLAG_INHERIT).into(), HANDLE_FLAG_INHERIT.into())?;
        // A read pending on the server end ends with ERROR_BROKEN_PIPE once
        // every copy of the client end is closed, nothing written (a
        // child's unused stderr, as sshd-session hands it out).
        let mut copy = core::ptr::null_mut();
        c.ok(
            DuplicateHandle(GetCurrentProcess(), client, GetCurrentProcess(), &mut copy, 0, 1, 2)
                != 0,
            GetLastError().into(),
        )?;
        c.ok(CloseHandle(client) != 0, 0)?;
        let event = CreateEventW(core::ptr::null(), 1, 0, core::ptr::null());
        let mut ov = Overlapped { internal: 0, internal_high: 0, offset: 0, offset_high: 0, event };
        let mut buf = [0u8; 16];
        let mut got = 0;
        let r = ReadFile(server, buf.as_mut_ptr(), 16, &mut got, (&raw mut ov).cast());
        c.ok(r == 0 && GetLastError() == ERROR_IO_PENDING, GetLastError().into())?;
        c.ok(CloseHandle(copy) != 0, 0)?;
        c.eq(GetOverlappedResult(server, &mut ov, &mut got, 1) as u64, 0)?;
        c.eq(GetLastError().into(), ERROR_BROKEN_PIPE.into())?;
        CloseHandle(ov.event);
        CloseHandle(server);

        // The same with ReadFileEx: its completion routine runs (in an
        // alertable wait) with ERROR_BROKEN_PIPE.
        let name = b"\\\\.\\Pipe\\W32PosixPipe.00000001.00000003\0";
        let server = CreateNamedPipeA(
            name.as_ptr(),
            PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED,
            0,
            1,
            4096,
            4096,
            0,
            &inherit,
        );
        let client = CreateFileA(
            name.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            0,
            &inherit,
            OPEN_EXISTING,
            0x80 | FILE_FLAG_OVERLAPPED,
            core::ptr::null_mut(),
        );
        c.ok(server as isize != -1 && client as isize != -1, GetLastError().into())?;
        let mut ov = Overlapped {
            internal: 0,
            internal_high: 0,
            offset: 0,
            offset_high: 0,
            event: core::ptr::null_mut(),
        };
        c.ok(
            ReadFileEx(server, buf.as_mut_ptr(), 16, &mut ov, read_done) != 0,
            GetLastError().into(),
        )?;
        c.eq(SleepEx(0, 1).into(), 0)?; // nothing yet
        c.ok(CloseHandle(client) != 0, 0)?;
        c.eq(SleepEx(5000, 1).into(), WAIT_IO_COMPLETION.into())?;
        c.eq(READ_DONE_ERROR.load(Ordering::SeqCst).into(), ERROR_BROKEN_PIPE.into())?;
        CloseHandle(server);

        // CallNamedPipeA (PostgreSQL's signals): connect to a message pipe,
        // write a message, read the answer, close; a pipe nobody serves is
        // ERROR_FILE_NOT_FOUND.
        let name = b"\\\\.\\pipe\\gala_call_test\0";
        let server = CreateNamedPipeA(
            name.as_ptr(),
            PIPE_ACCESS_DUPLEX,
            PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE,
            1,
            64,
            64,
            0,
            core::ptr::null(),
        );
        c.ok(server as isize != -1, GetLastError().into())?;
        let t = CreateThread(core::ptr::null_mut(), 0, answer, server, 0, core::ptr::null_mut());
        c.ok(!t.is_null(), 0)?;
        let mut reply = [0u8; 16];
        let mut got = 0;
        c.ok(
            CallNamedPipeA(
                name.as_ptr(),
                b"ping".as_ptr(),
                4,
                reply.as_mut_ptr(),
                16,
                &mut got,
                5000,
            ) != 0,
            GetLastError().into(),
        )?;
        c.ok(got == 4 && &reply[..4] == b"pong", got.into())?;
        c.eq(WaitForSingleObject(t, 10_000).into(), 0)?;
        let mut code = 9;
        c.ok(GetExitCodeThread(t, &mut code) != 0 && code == 0, code.into())?;
        CloseHandle(t);
        CloseHandle(server);
        let none = b"\\\\.\\pipe\\gala_nobody\0";
        c.eq(
            CallNamedPipeA(none.as_ptr(), b"x".as_ptr(), 1, reply.as_mut_ptr(), 16, &mut got, 100)
                as u64,
            0,
        )?;
        c.eq(GetLastError().into(), 2)?;
    }
    Ok(())
}

static READ_DONE_ERROR: AtomicU32 = AtomicU32::new(0);

/// CallNamedPipeA's server: connects, reads "ping", answers "pong".
extern "system" fn answer(server: *mut c_void) -> u32 {
    let mut b = [0u8; 16];
    let mut n = 0;
    // SAFETY: plain calls with our buffer and the pipe.
    unsafe {
        ConnectNamedPipe(server, core::ptr::null_mut());
        if ReadFile(server, b.as_mut_ptr(), 16, &mut n, core::ptr::null_mut()) == 0
            || &b[..n as usize] != b"ping"
        {
            return 1;
        }
        WriteFile(server, b"pong".as_ptr(), 4, &mut n, core::ptr::null_mut());
        // On a pipe this waits until the client has read the answer.
        if FlushFileBuffers(server) == 0 {
            return 2;
        }
        DisconnectNamedPipe(server);
    }
    0
}
const WAIT_IO_COMPLETION: u32 = 0xc0;

/// ReadFileEx's completion routine: (dwErrorCode, dwNumberOfBytesTransfered,
/// lpOverlapped).
extern "system" fn read_done(error: u32, _bytes: u32, _ov: *mut Overlapped) {
    READ_DONE_ERROR.store(error, Ordering::SeqCst);
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = match run() {
        Ok(()) => {
            out(b"pipe_test: ok\r\n");
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

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
//! Prints `pipe_test: ok` or exits with the number of the failing check.

#![no_std]
#![no_main]

use core::ffi::c_void;

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
    fn ExitProcess(code: u32) -> !;
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

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
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
    }
    Ok(())
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

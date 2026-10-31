//! Keyed events (ntdll's NtCreateKeyedEvent, NtWaitForKeyedEvent,
//! NtReleaseKeyedEvent), as the JDK's wepoll and other lock code use them,
//! with the results real Windows gives (Windows Internals, "Keyed
//! events"): a release on a key wakes exactly one thread waiting on that
//! key, and when no thread waits it blocks until one does (and the other
//! way round); a timeout ends either with STATUS_TIMEOUT; keys must be
//! even (the low bit is reserved: STATUS_INVALID_PARAMETER_1); a null
//! handle means the system's keyed event.
//!
//! Prints `keyed_event_test: ok` or exits with the number of the failing
//! check.

#![no_std]
#![no_main]

use core::ffi::c_void;

type Handle = *mut c_void;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut c_void) -> i32;
    fn ExitProcess(code: u32) -> !;
    fn CreateThread(
        security: *mut c_void,
        stack: usize,
        start: extern "system" fn(*mut c_void) -> u32,
        arg: *mut c_void,
        flags: u32,
        id: *mut u32,
    ) -> Handle;
    fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
    fn GetExitCodeThread(h: Handle, code: *mut u32) -> i32;
    fn CloseHandle(h: Handle) -> i32;
    fn Sleep(ms: u32);
}

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn NtCreateKeyedEvent(
        out: *mut Handle,
        access: u32,
        attributes: *const c_void,
        flags: u32,
    ) -> u32;
    fn NtWaitForKeyedEvent(h: Handle, key: usize, alertable: u8, timeout: *const i64) -> u32;
    fn NtReleaseKeyedEvent(h: Handle, key: usize, alertable: u8, timeout: *const i64) -> u32;
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

const KEYEDEVENT_ALL_ACCESS: u32 = 0xf_0003;
const STATUS_TIMEOUT: u64 = 0x102;
const STATUS_INVALID_PARAMETER_1: u64 = 0xc000_00ef;
/// 100 ms, relative.
const SHORT: i64 = -1_000_000;

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
    fn eq(&mut self, got: u64, want: u64) -> Result<(), u32> {
        self.0 += 1;
        if got == want {
            return Ok(());
        }
        out(b"keyed_event_test: check ");
        out(&hex(self.0));
        out(b" got ");
        out(&hex(got as u32));
        out(b"\r\n");
        Err(self.0)
    }
}

/// The keyed event the threads use.
static mut EVENT: Handle = core::ptr::null_mut();

/// Waits on key 16 without a timeout; exits with the status.
extern "system" fn waiter(_: *mut c_void) -> u32 {
    // SAFETY: EVENT is set before the thread starts.
    unsafe { NtWaitForKeyedEvent(EVENT, 16, 0, core::ptr::null()) }
}

/// Releases key 24 without a timeout (blocks until a waiter comes).
extern "system" fn releaser(_: *mut c_void) -> u32 {
    // SAFETY: as above.
    unsafe { NtReleaseKeyedEvent(EVENT, 24, 0, core::ptr::null()) }
}

fn start(f: extern "system" fn(*mut c_void) -> u32) -> Handle {
    // SAFETY: a plain thread with no argument.
    unsafe {
        CreateThread(core::ptr::null_mut(), 0, f, core::ptr::null_mut(), 0, core::ptr::null_mut())
    }
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    let short = SHORT;
    // SAFETY: ntdll and kernel32 calls with values of ours.
    unsafe {
        let mut h = core::ptr::null_mut();
        c.eq(NtCreateKeyedEvent(&mut h, KEYEDEVENT_ALL_ACCESS, core::ptr::null(), 0).into(), 0)?;
        EVENT = h;
        // Keys are even.
        c.eq(NtWaitForKeyedEvent(h, 9, 0, &short).into(), STATUS_INVALID_PARAMETER_1)?;
        c.eq(NtReleaseKeyedEvent(h, 9, 0, &short).into(), STATUS_INVALID_PARAMETER_1)?;
        // Nobody on the other side: both time out.
        c.eq(NtWaitForKeyedEvent(h, 8, 0, &short).into(), STATUS_TIMEOUT)?;
        c.eq(NtReleaseKeyedEvent(h, 8, 0, &short).into(), STATUS_TIMEOUT)?;

        // A waiter first: the release wakes it at once.
        let t = start(waiter);
        Sleep(100);
        // A release on another key does not reach it.
        c.eq(NtReleaseKeyedEvent(h, 32, 0, &short).into(), STATUS_TIMEOUT)?;
        c.eq(NtReleaseKeyedEvent(h, 16, 0, core::ptr::null()).into(), 0)?;
        c.eq(WaitForSingleObject(t, 5000).into(), 0)?;
        let mut code = 7u32;
        GetExitCodeThread(t, &mut code);
        c.eq(code.into(), 0)?;
        CloseHandle(t);

        // A release first: it waits until the waiter comes.
        let t = start(releaser);
        Sleep(100);
        c.eq(WaitForSingleObject(t, 0).into(), STATUS_TIMEOUT)?;
        c.eq(NtWaitForKeyedEvent(h, 24, 0, core::ptr::null()).into(), 0)?;
        c.eq(WaitForSingleObject(t, 5000).into(), 0)?;
        GetExitCodeThread(t, &mut code);
        c.eq(code.into(), 0)?;
        CloseHandle(t);

        // The system's keyed event (a null handle).
        c.eq(NtReleaseKeyedEvent(core::ptr::null_mut(), 8, 0, &short).into(), STATUS_TIMEOUT)?;
        c.eq(CloseHandle(h) as u64, 1)?;
    }
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = match run() {
        Ok(()) => {
            out(b"keyed_event_test: ok\r\n");
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

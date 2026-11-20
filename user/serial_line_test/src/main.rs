//! Two programs writing to gala's serial line at once (gala's own serial
//! console, not a Windows contract: on Windows each has its own console).
//! A program's line stays whole when another program writes a line while
//! it is open, and a line a program leaves open when it exits ends there:
//!
//! - the parent writes the first half of its line, starts itself as
//!   `child` and waits for it;
//! - the child writes a whole line, then half a line, and exits;
//! - the parent writes the rest of its line.
//!
//! The log holds `serial_line_test: parent's line`, `serial_line_test:
//! child's line` and `serial_line_test: child's half` each whole, then
//! `serial_line_test: ok`; a failing check exits with its source line.

#![no_std]
#![no_main]

use ntrt as _;

type Handle = usize;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, overlapped: usize) -> i32;
    fn ExitProcess(code: u32) -> !;
    fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
    fn GetExitCodeProcess(h: Handle, code: *mut u32) -> i32;
    fn CloseHandle(h: Handle) -> i32;
    fn GetCommandLineW() -> *const u16;
    fn GetModuleFileNameW(module: Handle, buf: *mut u16, n: u32) -> u32;
    fn CreateProcessW(
        app: *const u16,
        cmdline: *mut u16,
        process_sa: usize,
        thread_sa: usize,
        inherit: i32,
        flags: u32,
        env: usize,
        dir: *const u16,
        startup: *const StartupInfo,
        info: *mut ProcessInformation,
    ) -> i32;
}

#[repr(C)]
struct StartupInfo {
    cb: u32,
    rest: [u8; 100],
}

#[repr(C)]
#[derive(Default)]
struct ProcessInformation {
    process: Handle,
    thread: Handle,
    pid: u32,
    tid: u32,
}

fn out(bytes: &[u8]) {
    let mut n = 0;
    // SAFETY: the standard output handle and a live buffer.
    unsafe {
        WriteFile(GetStdHandle(-11i32 as u32), bytes.as_ptr(), bytes.len() as u32, &mut n, 0)
    };
}

/// Ends the process with the caller's source line when `ok` is false.
#[track_caller]
fn check(ok: bool) {
    if !ok {
        let line = core::panic::Location::caller().line();
        out(b"\r\nserial_line_test: check failed\r\n");
        // SAFETY: ends the process.
        unsafe { ExitProcess(line) }
    }
}

/// Whether this process was started as the child (`... child`).
fn is_child() -> bool {
    // SAFETY: the process's command line, NUL-terminated.
    let mut p = unsafe { GetCommandLineW() };
    let mut len = 0;
    // SAFETY: within the NUL-terminated command line.
    unsafe {
        while *p.add(len) != 0 {
            len += 1;
        }
        p = p.add(len.saturating_sub(5));
    }
    // SAFETY: the last five units of the command line (or all of it).
    let tail = unsafe { core::slice::from_raw_parts(p, len.min(5)) };
    tail.iter().copied().eq("child".encode_utf16())
}

fn child() -> ! {
    out(b"serial_line_test: child's line\r\n");
    out(b"serial_line_test: child's half");
    // SAFETY: ends the process with its line open.
    unsafe { ExitProcess(0) }
}

fn parent() {
    let mut path = [0u16; 260];
    // SAFETY: a buffer of the length given.
    let n = unsafe { GetModuleFileNameW(0, path.as_mut_ptr(), path.len() as u32) } as usize;
    check(n > 0 && n < path.len());
    let mut cmdline = [0u16; 300];
    let mut len = 0;
    for u in path[..n].iter().copied().chain(" child".encode_utf16()) {
        cmdline[len] = u;
        len += 1;
    }
    let startup = StartupInfo { cb: 104, rest: [0; 100] };
    let mut info = ProcessInformation::default();

    out(b"serial_line_test: parent's ");
    // SAFETY: NUL-terminated strings, a STARTUPINFOW and the result.
    let started = unsafe {
        CreateProcessW(
            path.as_ptr(),
            cmdline.as_mut_ptr(),
            0,
            0,
            0,
            0,
            0,
            core::ptr::null(),
            &startup,
            &mut info,
        )
    };
    check(started != 0);
    // SAFETY: the child's process handle.
    check(unsafe { WaitForSingleObject(info.process, 30_000) } == 0);
    let mut code = 1;
    // SAFETY: as above.
    check(unsafe { GetExitCodeProcess(info.process, &mut code) } != 0 && code == 0);
    out(b"line\r\n");
    // SAFETY: handles CreateProcessW returned.
    unsafe {
        CloseHandle(info.thread);
        CloseHandle(info.process);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    if is_child() {
        child();
    }
    parent();
    out(b"serial_line_test: ok\r\n");
    // SAFETY: ends the process.
    unsafe { ExitProcess(0) }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdead) }
}

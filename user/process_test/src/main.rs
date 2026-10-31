//! CreateProcessW, with the results real Windows gives: the parent starts
//! itself as a child (`process_test child <handle>`) with an inheritable
//! event; the child prints through the inherited console, signals the
//! event through the inherited handle value and exits with 0x1234. The
//! parent waits on the process handle and reads the exit code
//! (STILL_ACTIVE while it runs), sees the event set, starts a child
//! suspended (it does not run until ResumeThread) and checks that a
//! missing program fails with ERROR_FILE_NOT_FOUND. Prints
//! `process_test: ok` or exits with the number of the failing check.

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

#[repr(C)]
struct StartupInfo {
    cb: u32,
    rest: [u8; 100],
}

#[repr(C)]
struct ProcessInformation {
    process: Handle,
    thread: Handle,
    process_id: u32,
    thread_id: u32,
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut c_void) -> Bool;
    fn GetCommandLineW() -> *const u16;
    fn GetModuleFileNameW(module: Handle, buf: *mut u16, size: u32) -> u32;
    fn GetLastError() -> u32;
    fn CreateEventW(
        attributes: *const SecurityAttributes,
        manual: Bool,
        initial: Bool,
        name: *const u16,
    ) -> Handle;
    fn SetEvent(h: Handle) -> Bool;
    fn CreateProcessW(
        application: *const u16,
        command_line: *mut u16,
        process_attributes: *const c_void,
        thread_attributes: *const c_void,
        inherit: Bool,
        flags: u32,
        environment: *const c_void,
        directory: *const u16,
        startup: *const StartupInfo,
        info: *mut ProcessInformation,
    ) -> Bool;
    fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
    fn GetExitCodeProcess(h: Handle, code: *mut u32) -> Bool;
    fn ResumeThread(h: Handle) -> u32;
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

const INFINITE: u32 = 0xffff_ffff;
const WAIT_TIMEOUT: u32 = 258;
const STILL_ACTIVE: u32 = 259;
const CREATE_SUSPENDED: u32 = 4;
const ERROR_FILE_NOT_FOUND: u32 = 2;
const CHILD_EXIT: u32 = 0x1234;

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
        out(b"process_test: check ");
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

/// The command line's words after the program name, as UTF-16 slices.
fn arguments<'a>(line: &'a [u16], out: &mut [&'a [u16]; 4]) -> usize {
    // Skip the program name: quoted (it may contain spaces) or one word.
    let rest = if line.first() == Some(&u16::from(b'"')) {
        let close =
            line[1..].iter().position(|&c| c == u16::from(b'"')).map_or(line.len(), |i| i + 2);
        &line[close.min(line.len())..]
    } else {
        let end = line.iter().position(|&c| c == u16::from(b' ')).unwrap_or(line.len());
        &line[end..]
    };
    let mut n = 0;
    for w in rest.split(|&c| c == u16::from(b' ')).filter(|w| !w.is_empty()) {
        if n < out.len() {
            out[n] = w;
            n += 1;
        }
    }
    n
}

fn parse_hex(w: &[u16]) -> Option<usize> {
    let mut v = 0usize;
    for &c in w {
        let d = (c as u8 as char).to_digit(16)?;
        v = v * 16 + d as usize;
    }
    Some(v)
}

/// UTF-16 of `text` into `buf` at `at`; returns the new end.
fn put(buf: &mut [u16], at: usize, text: &[u8]) -> usize {
    for (i, &b) in text.iter().enumerate() {
        buf[at + i] = u16::from(b);
    }
    at + text.len()
}

fn child(handle: usize) -> ! {
    out(b"process_test: child says hello through the inherited console\r\n");
    // SAFETY: the inherited event handle (by value, as Windows passes it).
    let ok = unsafe { SetEvent(handle as Handle) } != 0;
    // SAFETY: ends the process.
    unsafe { ExitProcess(if ok { CHILD_EXIT } else { 0xbad }) }
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    // SAFETY: documented kernel32 calls with valid arguments.
    unsafe {
        let mut path = [0u16; 260];
        let n = GetModuleFileNameW(core::ptr::null_mut(), path.as_mut_ptr(), 260) as usize;
        c.ok(n > 0 && n < 260, n as u64)?;

        // An inheritable event; its value goes on the child's command line.
        let inherit =
            SecurityAttributes { length: 24, descriptor: core::ptr::null_mut(), inherit: 1 };
        let event = CreateEventW(&inherit, 1, 0, core::ptr::null());
        c.ok(!event.is_null(), GetLastError().into())?;
        let mut line = [0u16; 400];
        let mut at = put(&mut line, 0, b"\"");
        line[at..at + n].copy_from_slice(&path[..n]);
        at += n;
        at = put(&mut line, at, b"\" child ");
        at = put(&mut line, at, &hex(event as usize as u32));
        line[at] = 0;

        let startup = StartupInfo { cb: 104, rest: [0; 100] };
        let mut info = ProcessInformation {
            process: core::ptr::null_mut(),
            thread: core::ptr::null_mut(),
            process_id: 0,
            thread_id: 0,
        };
        let ok = CreateProcessW(
            path.as_ptr(),
            line.as_mut_ptr(),
            core::ptr::null(),
            core::ptr::null(),
            1,
            0,
            core::ptr::null(),
            core::ptr::null(),
            &startup,
            &mut info,
        );
        c.ok(ok != 0, GetLastError().into())?;
        c.ok(!info.process.is_null() && !info.thread.is_null(), 0)?;
        c.ok(info.process_id != 0 && info.thread_id != 0, info.process_id.into())?;
        c.eq(WaitForSingleObject(info.process, 10_000).into(), 0)?;
        let mut code = 0;
        c.ok(GetExitCodeProcess(info.process, &mut code) != 0, GetLastError().into())?;
        c.eq(code.into(), CHILD_EXIT.into())?;
        c.eq(WaitForSingleObject(event, 0).into(), 0)?; // the child set it
        CloseHandle(info.thread);
        CloseHandle(info.process);

        // Suspended: nothing runs until ResumeThread.
        let ok = CreateProcessW(
            path.as_ptr(),
            line.as_mut_ptr(),
            core::ptr::null(),
            core::ptr::null(),
            1,
            CREATE_SUSPENDED,
            core::ptr::null(),
            core::ptr::null(),
            &startup,
            &mut info,
        );
        c.ok(ok != 0, GetLastError().into())?;
        c.eq(WaitForSingleObject(info.process, 200).into(), WAIT_TIMEOUT.into())?;
        c.ok(GetExitCodeProcess(info.process, &mut code) != 0, GetLastError().into())?;
        c.eq(code.into(), STILL_ACTIVE.into())?;
        c.eq(ResumeThread(info.thread).into(), 1)?;
        c.eq(WaitForSingleObject(info.process, INFINITE).into(), 0)?;
        c.ok(GetExitCodeProcess(info.process, &mut code) != 0 && code == CHILD_EXIT, code.into())?;
        CloseHandle(info.thread);
        CloseHandle(info.process);

        // A program that does not exist (in the same directory).
        let mut missing = path;
        let slash = missing[..n].iter().rposition(|&ch| ch == u16::from(b'\\')).unwrap_or(0);
        let end = put(&mut missing, slash + 1, b"no_such_program.exe");
        missing[end] = 0;
        let ok = CreateProcessW(
            missing.as_ptr(),
            core::ptr::null_mut(),
            core::ptr::null(),
            core::ptr::null(),
            0,
            0,
            core::ptr::null(),
            core::ptr::null(),
            &startup,
            &mut info,
        );
        c.eq(ok as u64, 0)?;
        c.eq(GetLastError().into(), ERROR_FILE_NOT_FOUND.into())?;
        CloseHandle(event);
    }
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    // SAFETY: the command line, NUL-terminated, for the process's lifetime.
    let line = unsafe {
        let p = GetCommandLineW();
        let mut n = 0;
        while *p.add(n) != 0 {
            n += 1;
        }
        core::slice::from_raw_parts(p, n)
    };
    let mut args: [&[u16]; 4] = [&[]; 4];
    let argc = arguments(line, &mut args);
    let child_word: [u16; 5] = [b'c', b'h', b'i', b'l', b'd'].map(u16::from);
    if argc == 2 && args[0] == child_word {
        child(parse_hex(args[1]).unwrap_or(0));
    }
    let code = match run() {
        Ok(()) => {
            out(b"process_test: ok\r\n");
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

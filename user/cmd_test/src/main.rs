//! cmd.exe's `/c` as programs use it (the UCRT's `_popen` and `system` run
//! `cmd.exe /c <command>`; PostgreSQL's initdb does), with the results
//! documented in `cmd /?` and Windows' observable behaviour:
//!
//! - `cmd /c exit 3`: exit code 3.
//! - The quote rules of `/c`: `"prog" child 7` keeps its quotes (exactly
//!   two, around an executable's name); `""prog" child 5"` has its first
//!   and last quote removed. The command's exit code is cmd's.
//! - Redirections: `> file` (stdout), `>> file` (appended), `< file`
//!   (stdin), `2>&1` (stderr where stdout goes).
//! - `echo text> file`: the text and CRLF.
//! - Through a pipe (`_popen`'s way): the command's output arrives at the
//!   pipe's read end, then end of file.
//! - An unknown command: exit code 9009, and on stderr "'name' is not
//!   recognized as an internal or external command," then "operable
//!   program or batch file.".
//!
//! The child modes of this program: `child N` (exit N), `print` ("hello"
//! on stdout), `cat` (stdin to stdout), `both` ("out" on stdout, "err" on
//! stderr).
//!
//! Prints `cmd_test: ok` or exits with the number of the failing check.

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
    fn GetLastError() -> u32;
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut c_void) -> Bool;
    fn ReadFile(h: Handle, buf: *mut u8, len: u32, read: *mut u32, ov: *mut c_void) -> Bool;
    fn ExitProcess(code: u32) -> !;
    fn CloseHandle(h: Handle) -> Bool;
    fn GetCommandLineW() -> *const u16;
    fn GetModuleFileNameW(module: Handle, name: *mut u16, size: u32) -> u32;
    fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
    fn GetExitCodeProcess(h: Handle, code: *mut u32) -> Bool;
    fn CreatePipe(
        read: *mut Handle,
        write: *mut Handle,
        sa: *const SecurityAttributes,
        size: u32,
    ) -> Bool;
    fn SetHandleInformation(h: Handle, mask: u32, flags: u32) -> Bool;
    fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        sa: *const SecurityAttributes,
        disposition: u32,
        flags: u32,
        template: Handle,
    ) -> Handle;
    fn DeleteFileW(name: *const u16) -> Bool;
    #[allow(clippy::too_many_arguments)]
    fn CreateProcessW(
        application: *const u16,
        command_line: *mut u16,
        process_attributes: *mut c_void,
        thread_attributes: *mut c_void,
        inherit: Bool,
        flags: u32,
        environment: *mut c_void,
        directory: *const u16,
        startup: *const u8,
        info: *mut [usize; 3],
    ) -> Bool;
}

#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn __chkstk();
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

const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const CREATE_ALWAYS: u32 = 2;
const OPEN_EXISTING: u32 = 3;
const STARTF_USESTDHANDLES: u32 = 0x100;

fn out(text: &[u8]) {
    write_to(-11i32 as u32, text);
}

fn write_to(which: u32, text: &[u8]) {
    let mut written = 0;
    // SAFETY: valid buffer and output pointer.
    unsafe {
        WriteFile(
            GetStdHandle(which),
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

struct Checks(core::cell::Cell<u32>);

impl Checks {
    fn ok(&self, cond: bool, got: u64) -> Result<(), u32> {
        self.0.set(self.0.get() + 1);
        if cond {
            return Ok(());
        }
        out(b"cmd_test: check ");
        out(&hex(self.0.get()));
        out(b" got ");
        out(&hex(got as u32));
        out(b"\r\n");
        Err(self.0.get())
    }

    fn eq(&self, got: u64, want: u64) -> Result<(), u32> {
        self.ok(got == want, got)
    }
}

/// A UTF-16 string being built, NUL-terminated.
struct W {
    b: [u16; 1024],
    n: usize,
}

impl W {
    fn new() -> W {
        W { b: [0; 1024], n: 0 }
    }

    fn s(mut self, text: &str) -> W {
        for c in text.encode_utf16() {
            self.b[self.n] = c;
            self.n += 1;
        }
        self
    }

    fn w(mut self, text: &[u16]) -> W {
        self.b[self.n..self.n + text.len()].copy_from_slice(text);
        self.n += text.len();
        self
    }

    fn ptr(&mut self) -> *mut u16 {
        self.b[self.n] = 0;
        self.b.as_mut_ptr()
    }
}

/// This program's path.
fn exe() -> ([u16; 260], usize) {
    let mut b = [0u16; 260];
    // SAFETY: our buffer.
    let n = unsafe { GetModuleFileNameW(core::ptr::null_mut(), b.as_mut_ptr(), 260) } as usize;
    (b, n)
}

/// Runs `line` (cmd's command line) with the given standard handles (null:
/// the inherited ones); its exit code.
fn run(line: &mut W, stdout: Handle, stderr: Handle) -> Option<u32> {
    let mut cmd = W::new().s("C:\\Windows\\System32\\cmd.exe");
    let mut startup = [0u8; 104];
    startup[0] = 104;
    // SAFETY: plain calls; STARTUPINFOW's dwFlags at 60, handles at 80.
    unsafe {
        let std_out = if stdout.is_null() { GetStdHandle(-11i32 as u32) } else { stdout };
        let std_err = if stderr.is_null() { GetStdHandle(-12i32 as u32) } else { stderr };
        startup[60..64].copy_from_slice(&STARTF_USESTDHANDLES.to_le_bytes());
        startup[80..88].copy_from_slice(&(GetStdHandle(-10i32 as u32) as u64).to_le_bytes());
        startup[88..96].copy_from_slice(&(std_out as u64).to_le_bytes());
        startup[96..104].copy_from_slice(&(std_err as u64).to_le_bytes());
        let mut info = [0usize; 3];
        let null = core::ptr::null_mut();
        if CreateProcessW(
            cmd.ptr(),
            line.ptr(),
            null,
            null,
            1,
            0,
            null,
            core::ptr::null(),
            startup.as_ptr(),
            &mut info,
        ) == 0
        {
            return None;
        }
        WaitForSingleObject(info[0] as Handle, 30_000);
        let mut code = 0;
        GetExitCodeProcess(info[0] as Handle, &mut code);
        CloseHandle(info[0] as Handle);
        CloseHandle(info[1] as Handle);
        Some(code)
    }
}

/// The contents of file `path` (up to 256 bytes).
fn read_file(path: &mut W, buf: &mut [u8; 256]) -> usize {
    // SAFETY: plain calls with our buffers.
    unsafe {
        let h = CreateFileW(
            path.ptr(),
            GENERIC_READ,
            7,
            core::ptr::null(),
            OPEN_EXISTING,
            0,
            core::ptr::null_mut(),
        );
        if h as isize == -1 {
            return 0;
        }
        let mut n = 0;
        ReadFile(h, buf.as_mut_ptr(), 256, &mut n, core::ptr::null_mut());
        CloseHandle(h);
        n as usize
    }
}

fn write_file(path: &mut W, data: &[u8]) {
    // SAFETY: plain calls with our buffers.
    unsafe {
        let h = CreateFileW(
            path.ptr(),
            GENERIC_WRITE,
            0,
            core::ptr::null(),
            CREATE_ALWAYS,
            0,
            core::ptr::null_mut(),
        );
        let mut n = 0;
        WriteFile(h, data.as_ptr(), data.len() as u32, &mut n, core::ptr::null_mut());
        CloseHandle(h);
    }
}

fn run_tests() -> Result<(), u32> {
    let c = Checks(core::cell::Cell::new(0));
    let (path, n) = exe();
    let me = &path[..n];
    let null = core::ptr::null_mut();
    let mut buf = [0u8; 256];

    // exit N.
    c.eq(run(&mut W::new().s("cmd.exe /c exit 3"), null, null).unwrap_or(99).into(), 3)?;
    // Quotes kept: exactly two, around the program's name.
    let mut line = W::new().s("cmd.exe /c \"").w(me).s("\" child 7");
    c.eq(run(&mut line, null, null).unwrap_or(99).into(), 7)?;
    // Quotes stripped: the first and the last.
    let mut line = W::new().s("cmd.exe /c \"\"").w(me).s("\" child 5\"");
    c.eq(run(&mut line, null, null).unwrap_or(99).into(), 5)?;

    // Redirections.
    let out_file = || W::new().s("C:\\gala\\cmd_test_out.txt");
    let in_file = || W::new().s("C:\\gala\\cmd_test_in.txt");
    let mut line = W::new().s("cmd.exe /c \"").w(me).s("\" print > C:\\gala\\cmd_test_out.txt");
    c.eq(run(&mut line, null, null).unwrap_or(99).into(), 0)?;
    let got = read_file(&mut out_file(), &mut buf);
    c.ok(&buf[..got] == b"hello", got as u64)?;
    let mut line = W::new().s("cmd.exe /c \"").w(me).s("\" print >> C:\\gala\\cmd_test_out.txt");
    c.eq(run(&mut line, null, null).unwrap_or(99).into(), 0)?;
    let got = read_file(&mut out_file(), &mut buf);
    c.ok(&buf[..got] == b"hellohello", got as u64)?;
    write_file(&mut in_file(), b"from a file");
    let mut line = W::new()
        .s("cmd.exe /c \"")
        .w(me)
        .s("\" cat < C:\\gala\\cmd_test_in.txt > C:\\gala\\cmd_test_out.txt");
    c.eq(run(&mut line, null, null).unwrap_or(99).into(), 0)?;
    let got = read_file(&mut out_file(), &mut buf);
    c.ok(&buf[..got] == b"from a file", got as u64)?;
    let mut line = W::new().s("cmd.exe /c \"").w(me).s("\" both > C:\\gala\\cmd_test_out.txt 2>&1");
    c.eq(run(&mut line, null, null).unwrap_or(99).into(), 0)?;
    let got = read_file(&mut out_file(), &mut buf);
    c.ok(&buf[..got] == b"outerr", got as u64)?;

    // echo.
    let mut line = W::new().s("cmd.exe /c echo hello world> C:\\gala\\cmd_test_out.txt");
    c.eq(run(&mut line, null, null).unwrap_or(99).into(), 0)?;
    let got = read_file(&mut out_file(), &mut buf);
    c.ok(&buf[..got] == b"hello world\r\n", got as u64)?;

    // Through a pipe, as _popen does.
    let sa = SecurityAttributes { length: 24, descriptor: null, inherit: 1 };
    let (mut r, mut w) = (null, null);
    // SAFETY: plain calls with our handles.
    unsafe {
        c.ok(CreatePipe(&mut r, &mut w, &sa, 0) != 0, GetLastError().into())?;
        SetHandleInformation(r, 1, 0);
        let mut line = W::new().s("cmd.exe /c \"").w(me).s("\" print");
        c.eq(run(&mut line, w, null).unwrap_or(99).into(), 0)?;
        CloseHandle(w);
        let mut got = 0;
        let mut total = 0;
        while ReadFile(r, buf.as_mut_ptr().add(total), (256 - total) as u32, &mut got, null) != 0
            && got > 0
        {
            total += got as usize;
        }
        c.ok(&buf[..total] == b"hello", total as u64)?;
        CloseHandle(r);
    }

    // An unknown command: 9009 and the message on stderr.
    let mut line = W::new().s("cmd.exe /c nosuchprogram 2> C:\\gala\\cmd_test_out.txt");
    c.eq(run(&mut line, null, null).unwrap_or(99).into(), 9009)?;
    let got = read_file(&mut out_file(), &mut buf);
    let want: &[u8] = b"'nosuchprogram' is not recognized as an internal or external command,\r\noperable program or batch file.\r\n";
    c.ok(&buf[..got] == want, got as u64)?;

    // SAFETY: our paths.
    unsafe {
        DeleteFileW(out_file().ptr());
        DeleteFileW(in_file().ptr());
    }
    Ok(())
}

/// The child modes; None if this is not a child.
fn child() -> Option<u32> {
    // SAFETY: the process's command line.
    let line = unsafe { GetCommandLineW() };
    let mut n = 0;
    // SAFETY: a NUL-terminated string.
    while unsafe { *line.add(n) } != 0 {
        n += 1;
    }
    // SAFETY: `n` units.
    let s = unsafe { core::slice::from_raw_parts(line, n) };
    let ends = |t: &str| {
        let t: [u16; 16] = core::array::from_fn(|i| t.encode_utf16().nth(i).unwrap_or(0));
        let len = t.iter().position(|&c| c == 0).unwrap_or(16);
        s.ends_with(&t[..len])
    };
    if ends(" print") {
        out(b"hello");
        return Some(0);
    }
    if ends(" both") {
        out(b"out");
        write_to(-12i32 as u32, b"err");
        return Some(0);
    }
    if ends(" cat") {
        let mut b = [0u8; 256];
        let mut got = 0;
        // SAFETY: plain calls with our buffer.
        unsafe {
            while ReadFile(
                GetStdHandle(-10i32 as u32),
                b.as_mut_ptr(),
                256,
                &mut got,
                core::ptr::null_mut(),
            ) != 0
                && got > 0
            {
                out(&b[..got as usize]);
            }
        }
        return Some(0);
    }
    // "child N": the last character is the digit.
    let len = s.len();
    if len >= 8
        && s[..len - 1].ends_with(&[
            u16::from(b'c'),
            u16::from(b'h'),
            u16::from(b'i'),
            u16::from(b'l'),
            u16::from(b'd'),
            u16::from(b' '),
        ])
    {
        return Some(u32::from(s[len - 1]).wrapping_sub(u32::from(b'0')));
    }
    None
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    if let Some(code) = child() {
        // SAFETY: ends the process.
        unsafe { ExitProcess(code) }
    }
    let code = match run_tests() {
        Ok(()) => {
            out(b"cmd_test: ok\r\n");
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

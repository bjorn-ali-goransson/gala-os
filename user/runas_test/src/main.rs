//! ShellExecute's "runas" verb (elevation, ROADMAP OF: ODT's setup.exe
//! relaunches itself with it), with the results of Windows. One source
//! for two programs: `runas_test` (x64) and `wow64_runas_test` (x86, built
//! from this file by user/wow64_runas_test).
//!
//! Expected values come from Microsoft's documentation (ShellExecuteExW,
//! SHELLEXECUTEINFOW, TOKEN_ELEVATION, TOKEN_ELEVATION_TYPE, "How User
//! Account Control works"):
//!
//! - The caller's token: TokenElevationType is 1 (default), 2 (full) or
//!   3 (limited); TokenElevation 0 or 1.
//! - An elevated caller (an administrator's full token: CI's run-all.ps1
//!   runs the programs elevated, gala's scenario as SYSTEM): "runas" needs
//!   no prompt. ShellExecuteExW with SEE_MASK_NOCLOSEPROCESS starts the
//!   program: TRUE, hInstApp over 32, a process handle of another process;
//!   the child's token is elevated with the caller's elevation type (it
//!   exits with 100 + 10 * TokenElevation + TokenElevationType, read back
//!   with GetExitCodeProcess).
//! - A caller that is not elevated would get UAC's prompt; gala has none
//!   and answers as a cancelled prompt does (ShellExecuteExW FALSE with
//!   ERROR_CANCELLED, hInstApp SE_ERR_ACCESSDENIED, no process;
//!   ShellExecuteW, the verb in another case, "RunAs":
//!   SE_ERR_ACCESSDENIED). Checked on gala only: on Windows
//!   that run would wait for someone to answer the prompt, so it says
//!   "skipped".
//!
//! Prints what it saw, then `<name>: ok` and exits with 0, or prints
//! `<name>: check N got X (line L)` and exits with N.

#![no_std]
#![no_main]

use core::ffi::c_void;
use core::ptr::null;

use ntrt as _;

#[cfg(target_arch = "x86_64")]
const NAME: &str = "runas_test";
#[cfg(target_arch = "x86")]
const NAME: &str = "wow64_runas_test";

type Handle = usize;

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, overlapped: usize) -> i32;
    fn ExitProcess(code: u32) -> !;
    fn CloseHandle(h: Handle) -> i32;
    fn GetLastError() -> u32;
    fn SetLastError(e: u32);
    fn GetCurrentProcess() -> Handle;
    fn GetCurrentProcessId() -> u32;
    fn GetProcessId(process: Handle) -> u32;
    fn GetModuleFileNameW(module: usize, buf: *mut u16, len: u32) -> u32;
    fn GetCommandLineW() -> *const u16;
    fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
    fn GetExitCodeProcess(h: Handle, code: *mut u32) -> i32;
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "advapi32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "advapi32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn OpenProcessToken(process: Handle, access: u32, token: *mut Handle) -> i32;
    fn GetTokenInformation(
        token: Handle,
        class: u32,
        info: *mut c_void,
        length: u32,
        returned: *mut u32,
    ) -> i32;
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "shell32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "shell32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn ShellExecuteExW(info: *mut ShellExecuteInfo) -> i32;
    fn ShellExecuteW(
        window: usize,
        verb: *const u16,
        file: *const u16,
        params: *const u16,
        dir: *const u16,
        show: i32,
    ) -> usize;
}

/// SHELLEXECUTEINFOW (shellapi.h).
#[repr(C)]
struct ShellExecuteInfo {
    size: u32,
    mask: u32,
    window: usize,
    verb: *const u16,
    file: *const u16,
    params: *const u16,
    dir: *const u16,
    show: i32,
    instance: usize,
    id_list: *const c_void,
    class: *const u16,
    class_key: usize,
    hot_key: u32,
    icon: usize,
    process: Handle,
}

const _: () =
    assert!(size_of::<ShellExecuteInfo>() == if cfg!(target_arch = "x86") { 60 } else { 112 });

const TOKEN_QUERY: u32 = 8;
const TOKEN_ELEVATION_TYPE: u32 = 18;
const TOKEN_ELEVATION: u32 = 20;
const SEE_MASK_NOCLOSEPROCESS: u32 = 0x40;
const SEE_MASK_FLAG_NO_UI: u32 = 0x400;
const SW_HIDE: i32 = 0;
const WAIT_OBJECT_0: u32 = 0;
const ERROR_CANCELLED: u32 = 1223;
const SE_ERR_ACCESSDENIED: usize = 5;

// ---- Output and checks ---------------------------------------------------

fn print(text: &[u8]) {
    let mut written = 0;
    // SAFETY: valid arguments.
    unsafe {
        WriteFile(GetStdHandle(-11i32 as u32), text.as_ptr(), text.len() as u32, &mut written, 0)
    };
}

struct Line {
    buf: [u8; 160],
    n: usize,
}

impl Line {
    fn new() -> Self {
        Self { buf: [0; 160], n: 0 }
    }

    fn s(mut self, s: &[u8]) -> Self {
        for &b in s {
            if self.n < self.buf.len() {
                self.buf[self.n] = b;
                self.n += 1;
            }
        }
        self
    }

    fn dec(self, v: u64) -> Self {
        let mut d = [0u8; 20];
        let mut i = d.len();
        let mut v = v;
        loop {
            i -= 1;
            d[i] = b'0' + (v % 10) as u8;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        self.s(&d[i..])
    }

    fn hex(self, v: u64) -> Self {
        let mut d = [0u8; 18];
        d[0] = b'0';
        d[1] = b'x';
        for i in 0..16 {
            d[2 + i] = b"0123456789abcdef"[((v >> (60 - 4 * i)) & 0xf) as usize];
        }
        self.s(&d)
    }

    fn print(self) {
        print(&self.buf[..self.n]);
    }
}

struct Checks(u32);

impl Checks {
    #[track_caller]
    fn ok(&mut self, cond: bool, got: u64) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        Line::new()
            .s(NAME.as_bytes())
            .s(b": check ")
            .dec(u64::from(self.0))
            .s(b" got ")
            .hex(got)
            .s(b" (line ")
            .dec(u64::from(core::panic::Location::caller().line()))
            .s(b")\r\n")
            .print();
        Err(self.0)
    }

    #[track_caller]
    fn eq(&mut self, got: u64, want: u64) -> Result<(), u32> {
        self.ok(got == want, got)
    }
}

/// `s` as a NUL-terminated UTF-16 string.
fn wide<const N: usize>(s: &str) -> [u16; N] {
    let mut w = [0u16; N];
    for (d, c) in w.iter_mut().zip(s.encode_utf16()) {
        *d = c;
    }
    w
}

/// The process's TokenElevation and TokenElevationType (None if they
/// cannot be read).
fn elevation() -> Option<(u32, u32)> {
    // SAFETY: a token handle of ours, closed; 4-byte results.
    unsafe {
        let mut token = 0;
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return None;
        }
        let (mut elevated, mut kind, mut n) = (0u32, 0u32, 0u32);
        let ok = GetTokenInformation(token, TOKEN_ELEVATION, (&raw mut elevated).cast(), 4, &mut n)
            != 0
            && GetTokenInformation(token, TOKEN_ELEVATION_TYPE, (&raw mut kind).cast(), 4, &mut n)
                != 0;
        CloseHandle(token);
        ok.then_some((elevated, kind))
    }
}

/// The command line ends with ` child`: this is the started copy.
fn is_child() -> bool {
    // SAFETY: the process's NUL-terminated command line.
    let line = unsafe {
        let p = GetCommandLineW();
        let mut n = 0;
        while *p.add(n) != 0 {
            n += 1;
        }
        core::slice::from_raw_parts(p, n)
    };
    line.ends_with(&wide::<5>("child"))
}

/// The child's exit code: 100 + 10 * TokenElevation + TokenElevationType
/// (1 if they cannot be read).
fn child() -> u32 {
    elevation().map_or(1, |(elevated, kind)| 100 + 10 * elevated + kind)
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    let (elevated, kind) = elevation().ok_or(1u32)?;
    c.ok(elevated <= 1, elevated.into())?;
    c.ok((1..=3).contains(&kind), kind.into())?;
    let mut me = [0u16; 300];
    // SAFETY: our buffer and its size.
    let n = unsafe { GetModuleFileNameW(0, me.as_mut_ptr(), 300) } as usize;
    c.ok(n > 0 && n < 300, n as u64)?;
    let runas = wide::<6>("runas");
    let params = wide::<6>("child");
    let mut info = ShellExecuteInfo {
        size: size_of::<ShellExecuteInfo>() as u32,
        mask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_FLAG_NO_UI,
        window: 0,
        verb: runas.as_ptr(),
        file: me.as_ptr(),
        params: params.as_ptr(),
        dir: null(),
        show: SW_HIDE,
        instance: 0,
        id_list: null(),
        class: null(),
        class_key: 0,
        hot_key: 0,
        icon: 0,
        process: 0,
    };
    if elevated == 0 {
        if !ntrt::on_gala() {
            // Windows would show UAC's prompt and wait for an answer.
            Line::new()
                .s(NAME.as_bytes())
                .s(b": skipped (needs an elevated administrator: runas would prompt)\r\n")
                .print();
            return Ok(());
        }
        // gala has no prompt: declined, as a cancelled one answers.
        // SAFETY: our structure and strings.
        let ok = unsafe {
            SetLastError(0);
            ShellExecuteExW(&mut info)
        };
        // SAFETY: plain call.
        let error = unsafe { GetLastError() };
        c.eq(ok as u64, 0)?;
        c.eq(error.into(), ERROR_CANCELLED.into())?;
        c.eq(info.instance as u64, SE_ERR_ACCESSDENIED as u64)?;
        c.eq(info.process as u64, 0)?;
        let verb = wide::<6>("RunAs");
        // SAFETY: our strings.
        let r = unsafe {
            ShellExecuteW(0, verb.as_ptr(), me.as_ptr(), params.as_ptr(), null(), SW_HIDE)
        };
        c.eq(r as u64, SE_ERR_ACCESSDENIED as u64)?;
        Line::new()
            .s(NAME.as_bytes())
            .s(b": not elevated (type ")
            .dec(kind.into())
            .s(b"): runas declined\r\n")
            .print();
        return Ok(());
    }
    // Elevated: no prompt; the program starts elevated.
    // SAFETY: our structure and strings.
    let ok = unsafe { ShellExecuteExW(&mut info) };
    // SAFETY: plain call.
    c.ok(ok != 0, unsafe { GetLastError() }.into())?;
    c.ok(info.instance > 32, info.instance as u64)?;
    c.ok(info.process != 0, 0)?;
    // SAFETY: the child's handle, ours; closed below.
    unsafe {
        let pid = GetProcessId(info.process);
        c.ok(pid != 0 && pid != GetCurrentProcessId(), pid.into())?;
        c.eq(WaitForSingleObject(info.process, 30_000).into(), WAIT_OBJECT_0.into())?;
        let mut code = 0u32;
        c.ok(GetExitCodeProcess(info.process, &mut code) != 0, GetLastError().into())?;
        CloseHandle(info.process);
        // The child: elevated, with the caller's elevation type.
        c.eq(code.into(), (110 + kind).into())?;
    }
    Line::new()
        .s(NAME.as_bytes())
        .s(b": elevated (type ")
        .dec(kind.into())
        .s(b"): runas started an elevated child\r\n")
        .print();
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    if is_child() {
        // SAFETY: ends the process.
        unsafe { ExitProcess(child()) }
    }
    let code = match run() {
        Ok(()) => {
            Line::new().s(NAME.as_bytes()).s(b": ok\r\n").print();
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
    unsafe { ExitProcess(0xc000_0409) }
}

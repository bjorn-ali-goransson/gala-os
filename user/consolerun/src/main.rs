//! `consolerun <program> [arguments]`: starts a program on the console
//! session's interactive desktop with its caller's account, and waits
//! for it: what `psexec -s -i` and MDT's ServiceUI.exe do for an
//! installer that runs as SYSTEM (in session 0, whose desktop nothing
//! shows) and should be seen. Scenario `office_install_gui` (ROADMAP OF)
//! runs ODT's setup.exe through it, elevated (SYSTEM) on the screen.
//!
//! Windows' documented path, the one session_test checks: the caller's
//! primary token duplicated (DuplicateTokenEx), moved to the console's
//! session (WTSGetActiveConsoleSessionId; SetTokenInformation
//! TokenSessionId, which needs SeTcbPrivilege: SYSTEM holds it), and
//! CreateProcessAsUserW with STARTUPINFO.lpDesktop `winsta0\default`.
//!
//! Prints `consolerun: started pid P in session S: <command line>`, then
//! `consolerun: pid P exited with X` and exits with the program's exit
//! code; a step that fails prints `consolerun: <step> failed: error E`
//! and exits with 1 (2: no program named).

#![no_std]
#![no_main]

use core::ffi::c_void;

use ntrt as _;

type Bool = i32;
type Handle = *mut c_void;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut c_void) -> Bool;
    fn ExitProcess(code: u32) -> !;
    fn GetCommandLineW() -> *const u16;
    fn GetCurrentProcess() -> Handle;
    fn GetLastError() -> u32;
    fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
    fn GetExitCodeProcess(h: Handle, code: *mut u32) -> Bool;
    fn CloseHandle(h: Handle) -> Bool;
    fn WTSGetActiveConsoleSessionId() -> u32;
}

#[link(name = "advapi32", kind = "raw-dylib")]
unsafe extern "system" {
    fn OpenProcessToken(process: Handle, access: u32, token: *mut Handle) -> Bool;
    fn DuplicateTokenEx(
        token: Handle,
        access: u32,
        sa: *const c_void,
        level: u32,
        kind: u32,
        new: *mut Handle,
    ) -> Bool;
    fn SetTokenInformation(token: Handle, class: u32, info: *const u8, len: u32) -> Bool;
    #[allow(clippy::too_many_arguments)]
    fn CreateProcessAsUserW(
        token: Handle,
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

const TOKEN_DUPLICATE: u32 = 0x2;
const TOKEN_QUERY: u32 = 0x8;
const TOKEN_ALL_ACCESS: u32 = 0xf01ff;
const SECURITY_IMPERSONATION: u32 = 2;
const TOKEN_PRIMARY: u32 = 1;
const TOKEN_SESSION_ID: u32 = 12;
const INFINITE: u32 = u32::MAX;
/// The longest command line CreateProcess takes (32767 characters).
const MAX_LINE: usize = 32768;

struct Line {
    b: [u8; 600],
    n: usize,
}

impl Line {
    fn new() -> Self {
        Self { b: [0; 600], n: 0 }
    }

    fn s(&mut self, s: &str) -> &mut Self {
        for &c in s.as_bytes() {
            if self.n < self.b.len() {
                self.b[self.n] = c;
                self.n += 1;
            }
        }
        self
    }

    /// UTF-16 text, its non-ASCII characters as `?` (a log line).
    fn w(&mut self, s: &[u16]) -> &mut Self {
        for &c in s.iter().take_while(|&&c| c != 0) {
            if self.n < self.b.len() {
                self.b[self.n] = if c < 0x80 { c as u8 } else { b'?' };
                self.n += 1;
            }
        }
        self
    }

    fn dec(&mut self, v: u64) -> &mut Self {
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
        for &c in &d[i..] {
            if self.n < self.b.len() {
                self.b[self.n] = c;
                self.n += 1;
            }
        }
        self
    }

    fn hex(&mut self, v: u32) -> &mut Self {
        self.s("0x");
        for i in 0..8 {
            let d = (v >> (28 - 4 * i)) & 0xf;
            let c = if d < 10 { b'0' + d as u8 } else { b'a' + d as u8 - 10 };
            if self.n < self.b.len() {
                self.b[self.n] = c;
                self.n += 1;
            }
        }
        self
    }

    fn print(&mut self) {
        self.s("\r\n");
        let mut written = 0;
        // SAFETY: our buffer; the standard output handle.
        unsafe {
            WriteFile(
                GetStdHandle(-11i32 as u32),
                self.b.as_ptr(),
                self.n as u32,
                &mut written,
                core::ptr::null_mut(),
            )
        };
    }
}

/// `consolerun: <step> failed: error E`; exit code 1.
fn failed(step: &str) -> u32 {
    // SAFETY: no arguments.
    let error = unsafe { GetLastError() };
    Line::new().s("consolerun: ").s(step).s(" failed: error ").dec(error.into()).print();
    1
}

/// The program's command line: ours without its first argument (our
/// name, quoted or not) and the blanks after it, NUL-terminated in `out`
/// (CreateProcessAsUserW may write to it). Its length, 0 for none.
fn child_line(out: &mut [u16]) -> usize {
    // SAFETY: the process's command line, NUL-terminated, lives as long
    // as the process.
    let line = unsafe {
        let p = GetCommandLineW();
        let mut n = 0;
        while *p.add(n) != 0 {
            n += 1;
        }
        core::slice::from_raw_parts(p, n)
    };
    let mut i = 0;
    let mut quoted = false;
    while i < line.len() {
        let c = line[i];
        if c == u16::from(b'"') {
            quoted = !quoted;
        } else if !quoted && (c == u16::from(b' ') || c == u16::from(b'\t')) {
            break;
        }
        i += 1;
    }
    while i < line.len() && (line[i] == u16::from(b' ') || line[i] == u16::from(b'\t')) {
        i += 1;
    }
    let rest = &line[i..];
    let n = rest.len().min(out.len() - 1);
    out[..n].copy_from_slice(&rest[..n]);
    out[n] = 0;
    n
}

fn run() -> u32 {
    let mut line = [0u16; MAX_LINE];
    if child_line(&mut line) == 0 {
        Line::new().s("usage: consolerun <program> [arguments]").print();
        return 2;
    }
    // SAFETY: the calls' own out-parameters and NUL-terminated buffers
    // that outlive them; handles from these calls.
    unsafe {
        let mut own = core::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_DUPLICATE | TOKEN_QUERY, &mut own) == 0 {
            return failed("OpenProcessToken");
        }
        let mut token = core::ptr::null_mut();
        let r = DuplicateTokenEx(
            own,
            TOKEN_ALL_ACCESS,
            core::ptr::null(),
            SECURITY_IMPERSONATION,
            TOKEN_PRIMARY,
            &mut token,
        );
        CloseHandle(own);
        if r == 0 {
            return failed("DuplicateTokenEx");
        }
        let session = WTSGetActiveConsoleSessionId();
        if SetTokenInformation(token, TOKEN_SESSION_ID, (&raw const session).cast(), 4) == 0 {
            return failed("SetTokenInformation(TokenSessionId)");
        }
        // STARTUPINFOW (104 bytes on x64): cb, then lpDesktop at 16.
        let desktop: [u16; 16] = {
            let mut d = [0u16; 16];
            for (i, b) in b"winsta0\\default".iter().enumerate() {
                d[i] = (*b).into();
            }
            d
        };
        let mut startup = [0usize; 13];
        startup[0] = 104;
        startup[2] = desktop.as_ptr() as usize;
        let mut info = [0usize; 3];
        let r = CreateProcessAsUserW(
            token,
            core::ptr::null(),
            line.as_mut_ptr(),
            core::ptr::null_mut(),
            core::ptr::null_mut(),
            0,
            0,
            core::ptr::null_mut(),
            core::ptr::null(),
            startup.as_ptr().cast(),
            &mut info,
        );
        CloseHandle(token);
        if r == 0 {
            return failed("CreateProcessAsUserW");
        }
        let (process, pid) = (info[0] as Handle, u64::from(info[2] as u32));
        CloseHandle(info[1] as Handle);
        Line::new()
            .s("consolerun: started pid ")
            .dec(pid)
            .s(" in session ")
            .dec(session.into())
            .s(": ")
            .w(&line)
            .print();
        WaitForSingleObject(process, INFINITE);
        let mut code = 0;
        if GetExitCodeProcess(process, &mut code) == 0 {
            return failed("GetExitCodeProcess");
        }
        CloseHandle(process);
        Line::new().s("consolerun: pid ").dec(pid).s(" exited with ").hex(code).print();
        code
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = run();
    // SAFETY: ends the process.
    unsafe { ExitProcess(code) }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xc000_0409) }
}

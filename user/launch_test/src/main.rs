//! The start menu's launches as a user-mode shell asks for them (ROADMAP
//! SH step 1, part 4): GalaDesktopAction's Start (with its console
//! window), Install and Linux verbs start their program as the caller,
//! not as the boot user, and give the caller a handle to the process
//! when asked, as ShellExecuteEx does with SEE_MASK_NOCLOSEPROCESS.
//!
//! The test runs itself again at Low integrity (CreateProcessAsUserW
//! with a copy of its token labelled S-1-16-4096) and the Low copy asks
//! for each launch, so a program started as the boot user (High) instead
//! of the caller shows. Expected results are Windows' for a process
//! created with the caller's token:
//!
//! - Start in a console window: the program's token is Low (the caller's
//!   user, level and all), it has a console of its own (GetConsoleMode on
//!   its standard output succeeds) whose window (conhost.exe's) is there
//!   while it runs, conhost.exe Low as well; the handle names the program
//!   (GetProcessId) and its exit code comes back through it.
//! - Start without asking for the handle writes the process id alone
//!   (the request's old form).
//! - Install: `gala install <id>` runs Low; offline, it ends with gala's
//!   exit code 4 (a step failed). An id with a space, or none, is refused
//!   (STATUS_INVALID_PARAMETER): it would be more than one argument.
//! - Linux terminal: `wsl -d <name>` runs Low in a console window; for a
//!   distribution that is not registered wsl.exe ends with -1. A name
//!   that is not a distribution's, an install suggestion out of range or
//!   an unknown operation is refused; a desktop or an application of a
//!   distribution that is not registered is not found
//!   (STATUS_OBJECT_NAME_NOT_FOUND).
//!
//! Prints `launch_test: ok`, or `launch_test: check N got X (line L)`
//! and exits with N.

#![no_std]
#![no_main]

use ntrt as _;

const NAME: &str = "launch_test";

type Handle = usize;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, overlapped: usize) -> i32;
    fn ExitProcess(code: u32) -> !;
    fn GetProcessId(process: Handle) -> u32;
    fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
    fn GetExitCodeProcess(h: Handle, code: *mut u32) -> i32;
    fn CloseHandle(h: Handle) -> i32;
    fn GetCurrentProcess() -> Handle;
    fn GetCommandLineW() -> *const u16;
    fn GetModuleFileNameW(module: Handle, buf: *mut u16, n: u32) -> u32;
    fn GetConsoleMode(h: Handle, mode: *mut u32) -> i32;
    fn CreateEventW(sa: usize, manual: i32, initial: i32, name: *const u16) -> Handle;
    fn OpenEventW(access: u32, inherit: i32, name: *const u16) -> Handle;
    fn SetEvent(h: Handle) -> i32;
    fn OpenProcess(access: u32, inherit: i32, pid: u32) -> Handle;
    fn Sleep(ms: u32);
}

#[link(name = "user32", kind = "raw-dylib")]
unsafe extern "system" {
    fn FindWindowW(class: *const u16, title: *const u16) -> Handle;
    fn GetWindowThreadProcessId(hwnd: Handle, pid: *mut u32) -> u32;
}

#[link(name = "advapi32", kind = "raw-dylib")]
unsafe extern "system" {
    fn OpenProcessToken(process: Handle, access: u32, token: *mut Handle) -> i32;
    fn GetTokenInformation(token: Handle, class: u32, buf: *mut u8, len: u32, got: *mut u32)
    -> i32;
    fn SetTokenInformation(token: Handle, class: u32, buf: *const u8, len: u32) -> i32;
    fn DuplicateTokenEx(
        token: Handle,
        access: u32,
        sa: usize,
        level: u32,
        kind: u32,
        new: *mut Handle,
    ) -> i32;
    #[allow(clippy::too_many_arguments)]
    fn CreateProcessAsUserW(
        token: Handle,
        app: *const u16,
        line: *mut u16,
        psa: usize,
        tsa: usize,
        inherit: i32,
        flags: u32,
        env: usize,
        dir: *const u16,
        startup: *const u8,
        info: *mut [usize; 3],
    ) -> i32;
}

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn GalaDesktopAction(
        request: *const u8,
        len: u32,
        out: *mut u8,
        out_len: u32,
        returned: *mut u32,
    ) -> u32;
}

const REQUEST_HEADER: usize = 40;
const ACTION_START: u32 = 1;
const ACTION_INSTALL: u32 = 16;
const ACTION_LINUX: u32 = 17;
/// The launch verbs' flags (args[0]).
const LAUNCH_CONSOLE_WINDOW: u32 = 1;
const LAUNCH_HANDLE: u32 = 2;
/// ACTION_LINUX's operations (args[1]).
const LINUX_INSTALL: u32 = 0;
const LINUX_DESKTOP: u32 = 1;
const LINUX_TERMINAL: u32 = 2;
const LINUX_APP: u32 = 3;

const STATUS_INVALID_PARAMETER: u32 = 0xc000_000d;
const STATUS_OBJECT_NAME_NOT_FOUND: u32 = 0xc000_0034;
const LOW_RID: u32 = 0x1000;
const TOKEN_QUERY: u32 = 0x0008;
const TOKEN_DUPLICATE: u32 = 0x0002;
const TOKEN_ASSIGN_PRIMARY: u32 = 0x0001;
const TOKEN_ADJUST_DEFAULT: u32 = 0x0080;
const TOKEN_INTEGRITY_LEVEL: u32 = 25;
const SE_GROUP_INTEGRITY: u32 = 0x20;
const SECURITY_IMPERSONATION: u32 = 2;
const TOKEN_PRIMARY: u32 = 1;
const WAIT: u32 = 120_000;
const SYNCHRONIZE: u32 = 0x0010_0000;
const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
/// What the started console program waits for: the caller has seen its
/// console window.
const GO: &str = "Local\\launch_test_go";

/// `s` as a NUL-terminated UTF-16 string.
struct W([u16; 64]);

impl W {
    fn new(s: &str) -> W {
        let mut w = [0u16; 64];
        for (d, b) in w.iter_mut().zip(s.bytes()) {
            *d = b.into();
        }
        W(w)
    }

    fn ptr(&self) -> *const u16 {
        self.0.as_ptr()
    }
}

/// The console window conhost.exe draws for program `title` (its class
/// GalaConsole, titled with the program's file name), waited for up to
/// 30 s: its process id, or 0.
fn console_window(title: &str) -> u32 {
    let (class, title) = (W::new("GalaConsole"), W::new(title));
    for _ in 0..1500 {
        let mut pid = 0;
        // SAFETY: our strings; the window may be gone again (pid 0).
        unsafe {
            let hwnd = FindWindowW(class.ptr(), title.ptr());
            if hwnd != 0 && GetWindowThreadProcessId(hwnd, &mut pid) != 0 && pid != 0 {
                return pid;
            }
            Sleep(20);
        }
    }
    0
}

// ---- Output ---------------------------------------------------------------

struct Line {
    buf: [u8; 300],
    n: usize,
}

impl Line {
    fn new() -> Self {
        Self { buf: [0; 300], n: 0 }
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

    fn dec(self, v: i64) -> Self {
        let mut d = [0u8; 21];
        let mut i = d.len();
        let neg = v < 0;
        let mut v = v.unsigned_abs();
        loop {
            i -= 1;
            d[i] = b'0' + (v % 10) as u8;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        if neg {
            i -= 1;
            d[i] = b'-';
        }
        self.s(&d[i..])
    }

    fn hex(self, v: u32) -> Self {
        let mut d = [0u8; 10];
        d[0] = b'0';
        d[1] = b'x';
        for (i, slot) in d[2..].iter_mut().enumerate() {
            let nibble = (v >> (28 - 4 * i)) & 0xf;
            *slot = b"0123456789abcdef"[nibble as usize];
        }
        self.s(&d)
    }

    fn print(self) {
        let mut written = 0;
        // SAFETY: our buffer; the standard output handle.
        unsafe {
            WriteFile(
                GetStdHandle(-11i32 as u32),
                self.buf.as_ptr(),
                self.n as u32,
                &mut written,
                0,
            )
        };
    }
}

fn say(what: &[u8]) {
    Line::new().s(NAME.as_bytes()).s(b": ").s(what).s(b"\r\n").print();
}

struct Checks(u32);

impl Checks {
    #[track_caller]
    fn ok(&mut self, cond: bool, got: i64) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        Line::new()
            .s(NAME.as_bytes())
            .s(b": check ")
            .dec(i64::from(self.0))
            .s(b" got ")
            .dec(got)
            .s(b" (line ")
            .dec(i64::from(core::panic::Location::caller().line()))
            .s(b")\r\n")
            .print();
        Err(self.0)
    }

    #[track_caller]
    fn eq(&mut self, got: i64, want: i64) -> Result<(), u32> {
        self.ok(got == want, got)
    }

    /// A status, printed in hex when it is not the one wanted.
    #[track_caller]
    fn status(&mut self, got: u32, want: u32) -> Result<(), u32> {
        if got != want {
            Line::new().s(NAME.as_bytes()).s(b": status ").hex(got).s(b"\r\n").print();
        }
        self.ok(got == want, got.into())
    }
}

// ---- Our command line -----------------------------------------------------

/// Our command line's last word (the role we were started in).
fn role() -> [u8; 16] {
    let mut word = [0u8; 16];
    // SAFETY: the process's command line, NUL-terminated.
    unsafe {
        let line = GetCommandLineW();
        let mut n = 0;
        while *line.add(n) != 0 {
            n += 1;
        }
        let units = core::slice::from_raw_parts(line, n);
        let units = match units.iter().rposition(|&u| u != u16::from(b' ')) {
            Some(end) => &units[..=end],
            None => units,
        };
        let start = units.iter().rposition(|&u| u == u16::from(b' ')).map_or(0, |i| i + 1);
        for (d, &u) in word.iter_mut().zip(&units[start..]) {
            *d = u as u8;
        }
    }
    word
}

fn is(role: &[u8; 16], name: &[u8]) -> bool {
    role.starts_with(name) && role.get(name.len()) == Some(&0)
}

// ---- Tokens ---------------------------------------------------------------

/// Process `h`'s (0: ours) integrity level (its label's RID), or 0.
fn level(h: Handle) -> u32 {
    let mut token = 0;
    let mut buf = [0u8; 64];
    let mut got = 0;
    // SAFETY: plain calls with our handles and buffers.
    unsafe {
        let process = if h == 0 { GetCurrentProcess() } else { h };
        if OpenProcessToken(process, TOKEN_QUERY, &mut token) == 0 {
            return 0;
        }
        let ok =
            GetTokenInformation(token, TOKEN_INTEGRITY_LEVEL, buf.as_mut_ptr(), 64, &mut got) != 0;
        CloseHandle(token);
        if !ok {
            return 0;
        }
        // TOKEN_MANDATORY_LABEL: the label SID's last sub-authority.
        let sid = u64::from_le_bytes(buf[0..8].try_into().unwrap_or([0; 8])) as *const u8;
        let count = usize::from(*sid.add(1));
        sid.add(8 + 4 * (count - 1)).cast::<u32>().read_unaligned()
    }
}

/// A copy of our primary token at Low integrity, or 0.
fn low_token() -> Handle {
    let (mut token, mut low) = (0, 0);
    // SAFETY: plain calls with our handles and buffers.
    unsafe {
        let access = TOKEN_DUPLICATE | TOKEN_ADJUST_DEFAULT | TOKEN_QUERY | TOKEN_ASSIGN_PRIMARY;
        if OpenProcessToken(GetCurrentProcess(), access, &mut token) == 0 {
            return 0;
        }
        let ok = DuplicateTokenEx(token, 0, 0, SECURITY_IMPERSONATION, TOKEN_PRIMARY, &mut low);
        CloseHandle(token);
        if ok == 0 {
            return 0;
        }
        // TOKEN_MANDATORY_LABEL: the SID's pointer and attributes, then
        // the SID S-1-16-4096 itself.
        let mut label = [0u8; 16 + 12];
        let sid = label.as_ptr() as u64 + 16;
        label[0..8].copy_from_slice(&sid.to_le_bytes());
        label[8..12].copy_from_slice(&SE_GROUP_INTEGRITY.to_le_bytes());
        label[16..28].copy_from_slice(&[1, 1, 0, 0, 0, 0, 0, 16, 0, 0x10, 0, 0]);
        if SetTokenInformation(low, TOKEN_INTEGRITY_LEVEL, label.as_ptr(), 28) == 0 {
            CloseHandle(low);
            return 0;
        }
    }
    low
}

// ---- The verbs ------------------------------------------------------------

/// Launch verb `what` with `args` and `text`; its status and what it
/// wrote (up to 16 bytes) with its length.
fn launch(what: u32, args: [u32; 4], text: &str) -> (u32, [u8; 16], u32) {
    let mut b = [0u8; REQUEST_HEADER + 2 * 128];
    b[0..4].copy_from_slice(&1u32.to_le_bytes());
    b[4..8].copy_from_slice(&what.to_le_bytes());
    for (i, a) in args.iter().enumerate() {
        b[16 + 4 * i..20 + 4 * i].copy_from_slice(&a.to_le_bytes());
    }
    let n = text.len().min(128);
    b[32..36].copy_from_slice(&(n as u32).to_le_bytes());
    for (i, u) in text.bytes().take(n).enumerate() {
        b[REQUEST_HEADER + 2 * i] = u;
    }
    let len = REQUEST_HEADER + 2 * n;
    let mut out = [0u8; 16];
    let mut returned = 0;
    let room = if args[0] & LAUNCH_HANDLE != 0 { 16 } else { 8 };
    // SAFETY: our buffers, with their lengths.
    let status =
        unsafe { GalaDesktopAction(b.as_ptr(), len as u32, out.as_mut_ptr(), room, &mut returned) };
    (status, out, returned)
}

/// A launch that gives back the process: (pid, handle).
fn launched(c: &mut Checks, what: u32, args: [u32; 4], text: &str) -> Result<(u32, Handle), u32> {
    let (status, out, returned) = launch(what, args, text);
    c.status(status, 0)?;
    c.eq(returned.into(), 16)?;
    let pid = u64::from_le_bytes(out[0..8].try_into().unwrap_or([0; 8]));
    let handle = u64::from_le_bytes(out[8..16].try_into().unwrap_or([0; 8])) as Handle;
    c.ok(pid != 0 && handle != 0, pid as i64)?;
    // SAFETY: the handle the verb gave us.
    c.eq(unsafe { GetProcessId(handle) }.into(), pid as i64)?;
    Ok((pid as u32, handle))
}

/// Waits for `process` to end and closes it: its exit code.
fn ended(c: &mut Checks, process: Handle) -> Result<u32, u32> {
    let mut code = 0;
    // SAFETY: our handle.
    unsafe {
        c.eq(WaitForSingleObject(process, WAIT).into(), 0)?;
        c.ok(GetExitCodeProcess(process, &mut code) != 0, 0)?;
        CloseHandle(process);
    }
    Ok(code)
}

/// The Low copy's part: each launch, as the caller.
fn as_caller() -> Result<(), u32> {
    let mut c = Checks(0);
    c.eq(level(0).into(), LOW_RID.into())?;

    // Start in a console window: Low, with a console of its own, whose
    // window conhost.exe draws as the caller too (Low), while it runs.
    let go = W::new(GO);
    // SAFETY: our string.
    let event = unsafe { CreateEventW(0, 1, 0, go.ptr()) };
    c.ok(event != 0, 0)?;
    let both = LAUNCH_CONSOLE_WINDOW | LAUNCH_HANDLE;
    let (_, process) = launched(&mut c, ACTION_START, [both, 0, 0, 0], "launch_test.exe console")?;
    c.eq(level(process).into(), LOW_RID.into())?;
    let host = console_window("launch_test.exe");
    c.ok(host != 0, 0)?;
    // SAFETY: plain calls with our handles.
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, host);
        c.ok(h != 0, host.into())?;
        let host_level = level(h);
        CloseHandle(h);
        c.eq(host_level.into(), LOW_RID.into())?;
        SetEvent(event);
    }
    let code = ended(&mut c, process)?;
    c.eq(code.into(), 0)?;
    // SAFETY: our handle.
    unsafe { CloseHandle(event) };
    say(b"console program: its own console window, as its caller");

    // Without the handle: the process id alone.
    let (status, out, returned) = launch(ACTION_START, [0; 4], "launch_test.exe done");
    c.status(status, 0)?;
    c.eq(returned.into(), 8)?;
    c.ok(out[0..8] != [0; 8], 0)?;
    say(b"started without a handle");

    // Install: gala.exe as its caller; offline, gala's exit code 4.
    let (_, process) = launched(&mut c, ACTION_INSTALL, [LAUNCH_HANDLE, 0, 0, 0], "7zip.7zip")?;
    c.eq(level(process).into(), LOW_RID.into())?;
    let code = ended(&mut c, process)?;
    c.eq(code.into(), 4)?;
    for bad in ["7zip 7zip", "", "\"7zip.7zip\""] {
        c.status(
            launch(ACTION_INSTALL, [LAUNCH_HANDLE, 0, 0, 0], bad).0,
            STATUS_INVALID_PARAMETER,
        )?;
    }
    say(b"install: gala install as its caller");

    // Linux: a terminal is wsl.exe in a console window, as the caller.
    let terminal = [LAUNCH_HANDLE, LINUX_TERMINAL, 0, 0];
    let (_, process) = launched(&mut c, ACTION_LINUX, terminal, "NoSuchLinux")?;
    c.eq(level(process).into(), LOW_RID.into())?;
    let code = ended(&mut c, process)?;
    c.eq(code.into(), 0xffff_ffff)?;
    let refused: [([u32; 4], &str); 4] = [
        ([LAUNCH_HANDLE, LINUX_TERMINAL, 0, 0], "../etc"),
        ([LAUNCH_HANDLE, LINUX_TERMINAL, 0, 0], ""),
        ([LAUNCH_HANDLE, LINUX_INSTALL, 1000, 0], "Debian"),
        ([LAUNCH_HANDLE, 9, 0, 0], "Debian"),
    ];
    for (args, name) in refused {
        c.status(launch(ACTION_LINUX, args, name).0, STATUS_INVALID_PARAMETER)?;
    }
    for op in [LINUX_DESKTOP, LINUX_APP] {
        let status = launch(ACTION_LINUX, [LAUNCH_HANDLE, op, 0, 0], "NoSuchLinux").0;
        c.status(status, STATUS_OBJECT_NAME_NOT_FOUND)?;
    }
    say(b"linux: wsl as its caller");
    Ok(())
}

/// The started console program's part: Low, and a console of its own.
fn console() -> u32 {
    let go = W::new(GO);
    // SAFETY: our string and handle.
    unsafe {
        let event = OpenEventW(SYNCHRONIZE, 0, go.ptr());
        if event == 0 || WaitForSingleObject(event, WAIT) != 0 {
            return 12;
        }
        CloseHandle(event);
    }
    let mut mode = 0;
    // SAFETY: our standard output handle.
    let has_console = unsafe { GetConsoleMode(GetStdHandle(-11i32 as u32), &mut mode) } != 0;
    match (level(0) == LOW_RID, has_console) {
        (true, true) => 0,
        (false, _) => 10,
        (true, false) => 11,
    }
}

/// Ours: the Low copy runs the launches.
fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    c.ok(level(0) > LOW_RID, level(0).into())?;
    let low = low_token();
    c.ok(low != 0, 0)?;
    let mut exe = [0u16; 260];
    // SAFETY: our buffer, with its length.
    let n = unsafe { GetModuleFileNameW(0, exe.as_mut_ptr(), 259) } as usize;
    c.ok(n > 0 && n < 259, n as i64)?;
    let mut line = [0u16; 300];
    let mut at = 0;
    line[at] = u16::from(b'"');
    at += 1;
    line[at..at + n].copy_from_slice(&exe[..n]);
    at += n;
    for b in b"\" caller" {
        line[at] = u16::from(*b);
        at += 1;
    }
    let mut startup = [0u8; 104];
    startup[0] = 104;
    let mut info = [0usize; 3];
    // SAFETY: plain calls with our buffers and handles.
    let code = unsafe {
        let ok = CreateProcessAsUserW(
            low,
            exe.as_ptr(),
            line.as_mut_ptr(),
            0,
            0,
            1,
            0,
            0,
            core::ptr::null(),
            startup.as_ptr(),
            &mut info,
        );
        CloseHandle(low);
        c.ok(ok != 0, 0)?;
        CloseHandle(info[1]);
        ended(&mut c, info[0])?
    };
    c.eq(code.into(), 0)?;
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let role = role();
    let code = if is(&role, b"console") {
        console()
    } else if is(&role, b"done") {
        0
    } else {
        let caller = is(&role, b"caller");
        match if caller { as_caller() } else { run() } {
            Ok(()) => {
                say(if caller { b"caller ok" } else { b"ok" });
                0
            }
            Err(n) => n,
        }
    };
    // SAFETY: ends the process.
    unsafe { ExitProcess(code) }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xc000_0409) }
}

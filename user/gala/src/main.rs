//! `gala`: shasha (the graphical shell) and the machine from a terminal
//! (ROADMAP SD "CLI parity", ARCHITECTURE "Server and desktop
//! workloads"). Everything shasha does with the mouse and keys is a named
//! action of the kernel's GalaDesktopAction (kernel/src/nt/action.rs);
//! this program asks for them:
//!
//! ```text
//! gala start <program> [arguments]   start a program (a path, a start menu name, a file name)
//! gala windows                       list the windows (the taskbar's)
//! gala programs                      list the start menu
//! gala activate|minimize|maximize|restore|close <window>
//! gala snap left|right|up|down <window>
//! gala move <window> <x> <y> <width> <height>
//! gala wallpaper <name>              dunes, ribbons, najdi, night or drawn
//! gala ps                            list the processes
//! gala kill <pid|name>               end a process (all of that name)
//! gala install <id> [--version v] [--source dir] [--location dir]
//! gala search <text> [--source dir]
//! gala list [<text>]
//! gala upgrade [<id>|--all] [--version v] [--source dir]
//! gala uninstall <id>|--product-code <key>
//! gala help
//! ```
//!
//! The package commands (ROADMAP PK, ARCHITECTURE "Installing software")
//! install winget packages the way winget does: module `pkg` over the
//! `galapkg` library. Exit codes there: 0 done, 1 usage, 2 not found, 3
//! an installer type or source gala cannot handle yet, 4 failed (rolled
//! back), 5 the download's SHA-256 is not the manifest's. gala.exe
//! copied into WinGet\Links under a command's name is that command's
//! shim (`pkg::shim_main`).
//!
//! A window is its HWND (`0x20010`, as `gala windows` prints it) or its
//! title (whole, or a part only one window's title has; any case).
//! Exit codes: 0 done, 1 usage, 2 no such window, program or process,
//! 3 not supported here (no graphical shell: the server kernel or
//! `profile=server`), 4 failed (the status is printed; "access denied"
//! for a window of a process of higher integrity level than gala's, as
//! UIPI rules for messages). `gala start` runs the program as gala's
//! caller (a copy of its token, as CreateProcess).

#![no_std]
#![no_main]

extern crate alloc;

// The process heap as Rust's allocator (galapkg uses `alloc`).
use gala_heap as _;

mod pkg;

use core::ffi::c_void;

type Bool = i32;
type Handle = *mut c_void;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut c_void) -> Bool;
    fn ExitProcess(code: u32) -> !;
    fn CloseHandle(h: Handle) -> Bool;
    fn GetCommandLineW() -> *const u16;
    fn OpenProcess(access: u32, inherit: Bool, id: u32) -> Handle;
    fn TerminateProcess(h: Handle, code: u32) -> Bool;
    fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
    fn GetLastError() -> u32;
    fn GetCurrentProcessId() -> u32;
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
    fn NtQuerySystemInformation(class: u32, buf: *mut u8, len: u32, ret: *mut u32) -> u32;
}

#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memmove(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn strlen(s: *const u8) -> usize;
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

const USAGE: &str = "usage: gala <command> [arguments]

  start <program> [arguments]   start a program (a path, a start menu name, a file name)
  windows                       list the windows (the taskbar's)
  programs                      list the start menu
  activate <window>             bring a window to the front (restored)
  minimize <window>
  maximize <window>
  restore <window>              back from minimized, maximized or snapped
  close <window>                ask a window's program to close it
  snap left|right|up|down <window>
  move <window> <x> <y> <width> <height>
  wallpaper <name>              dunes, ribbons, najdi, night or drawn
  ps                            list the processes
  kill <pid|name>               end a process (every one of that name)
  install <id> [--version v] [--source dir] [--location dir]
                                install a winget package (zip, portable;
                                Inno Setup, NSIS, exe: run silently)
  search <text> [--source dir]  find packages by id, name, moniker or tag
  list [<text>]                 installed programs (the uninstall records)
  upgrade [<id>|--all] [--version v] [--source dir]
                                list or install newer versions
  uninstall <id>                remove a package (its files, commands, record;
                                or run its uninstaller silently)
  help                          this text

Packages come from a winget-pkgs tree: --source <dir> (a local snapshot,
installers in its installers\\ directory) or %GALA_WINGET_SOURCE%; the
repository over HTTPS is not supported yet. The install log is
%LOCALAPPDATA%\\gala\\logs\\install.log (--verbose shows it as it runs).

A window is its HWND (as `gala windows` prints it) or its title (whole,
or a part only one title has; any case).
Exit codes: 0 done, 1 usage, 2 not found, 3 not supported here (no
graphical shell; an installer type or source not supported yet), 4 failed
(an install rolled back), 5 a download whose SHA-256 is not the manifest's.
";

const EXIT_USAGE: u32 = 1;
const EXIT_NOT_FOUND: u32 = 2;
const EXIT_NOT_SUPPORTED: u32 = 3;
const EXIT_FAILED: u32 = 4;

// GalaDesktopAction (kernel/src/nt/action.rs).
const REQUEST_VERSION: u32 = 1;
const REQUEST_HEADER: usize = 40;
const MAX_TEXT: usize = 520;
const ACTION_START: u32 = 1;
const ACTION_WINDOWS: u32 = 2;
const ACTION_ACTIVATE: u32 = 3;
const ACTION_MINIMIZE: u32 = 4;
const ACTION_MAXIMIZE: u32 = 5;
const ACTION_RESTORE: u32 = 6;
const ACTION_CLOSE: u32 = 7;
const ACTION_SNAP: u32 = 8;
const ACTION_MOVE: u32 = 9;
const ACTION_WALLPAPER: u32 = 10;
const ACTION_PROGRAMS: u32 = 11;
const LIST_HEADER: usize = 8;
const TITLE: usize = 128;
const WINDOW_ENTRY: usize = 40 + 2 * TITLE;
const PROGRAM_ENTRY: usize = 8 + 2 * 32 + 2 * 128;
const MAX_WINDOWS: usize = 64;

const STATUS_SUCCESS: u32 = 0;
const STATUS_INVALID_HANDLE: u32 = 0xc000_0008;
const STATUS_OBJECT_NAME_NOT_FOUND: u32 = 0xc000_0034;
const STATUS_NOT_SUPPORTED: u32 = 0xc000_00bb;
const STATUS_ACCESS_DENIED: u32 = 0xc000_0022;
const STATUS_INFO_LENGTH_MISMATCH: u32 = 0xc000_0004;
const STATUS_INVALID_PARAMETER: u32 = 0xc000_000d;

// ---- Output ----------------------------------------------------------

/// A line being built (ASCII), written to stdout or stderr.
struct Line {
    b: [u8; 512],
    n: usize,
}

impl Line {
    fn new() -> Line {
        Line { b: [0; 512], n: 0 }
    }

    fn s(&mut self, text: &str) -> &mut Line {
        self.bytes(text.as_bytes())
    }

    fn bytes(&mut self, text: &[u8]) -> &mut Line {
        for &c in text {
            if self.n < self.b.len() {
                self.b[self.n] = c;
                self.n += 1;
            }
        }
        self
    }

    /// UTF-16 text, other than ASCII as '?'.
    fn w(&mut self, text: &[u16]) -> &mut Line {
        for &c in text {
            let c = if (0x20..0x7f).contains(&c) { c as u8 } else { b'?' };
            self.bytes(&[c]);
        }
        self
    }

    fn dec(&mut self, v: i64) -> &mut Line {
        let mut digits = [0u8; 20];
        let mut n = 0;
        let mut u = v.unsigned_abs();
        loop {
            digits[n] = b'0' + (u % 10) as u8;
            n += 1;
            u /= 10;
            if u == 0 {
                break;
            }
        }
        if v < 0 {
            self.bytes(b"-");
        }
        for i in (0..n).rev() {
            self.bytes(&[digits[i]]);
        }
        self
    }

    fn hex(&mut self, v: u64, width: usize) -> &mut Line {
        self.bytes(b"0x");
        let mut started = false;
        for i in (0..16).rev() {
            let d = ((v >> (4 * i)) & 0xf) as u8;
            if d != 0 || started || i < width {
                started = true;
                self.bytes(&[if d < 10 { b'0' + d } else { b'a' + d - 10 }]);
            }
        }
        self
    }

    /// Spaces up to column `col`.
    fn to(&mut self, col: usize) -> &mut Line {
        while self.n < col {
            self.bytes(b" ");
        }
        self.bytes(b" ")
    }

    fn print(&mut self) {
        self.bytes(b"\r\n");
        write_to(-11i32 as u32, &self.b[..self.n]);
        self.n = 0;
    }
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

fn out(text: &str) {
    write_to(-11i32 as u32, text.as_bytes());
}

/// An error on stderr ("gala: ..."); the exit code.
fn fail(code: u32, parts: &[&str]) -> u32 {
    let mut l = Line::new();
    l.s("gala: ");
    for p in parts {
        l.s(p);
    }
    l.bytes(b"\r\n");
    write_to(-12i32 as u32, &l.b[..l.n]);
    code
}

/// A failed action's status: the exit code (and the message).
fn status_error(status: u32, what: &str) -> u32 {
    match status {
        STATUS_NOT_SUPPORTED => fail(
            EXIT_NOT_SUPPORTED,
            &["no graphical shell here (a server: windows cannot be ", what, ")"],
        ),
        STATUS_INVALID_HANDLE => fail(EXIT_NOT_FOUND, &["no such window on the screen"]),
        STATUS_OBJECT_NAME_NOT_FOUND => fail(EXIT_NOT_FOUND, &["not found: ", what]),
        STATUS_INVALID_PARAMETER => fail(EXIT_USAGE, &["not a valid request: ", what]),
        // UIPI: the window's process has a higher integrity level.
        STATUS_ACCESS_DENIED => fail(
            EXIT_FAILED,
            &["access denied (the window's program runs at a higher integrity level): not ", what],
        ),
        _ => {
            let mut l = Line::new();
            l.s("gala: ").s(what).s(" failed with status ").hex(u64::from(status), 8);
            l.bytes(b"\r\n");
            write_to(-12i32 as u32, &l.b[..l.n]);
            EXIT_FAILED
        }
    }
}

// ---- The command line ------------------------------------------------

const MAX_ARGS: usize = 16;
const MAX_CHARS: usize = 1024;

/// The arguments (CommandLineToArgvW's rules), UTF-16, back to back.
struct Args {
    chars: [u16; MAX_CHARS],
    len: usize,
    ends: [usize; MAX_ARGS],
    count: usize,
}

impl Args {
    fn push(&mut self, c: u16) {
        if self.len < MAX_CHARS {
            self.chars[self.len] = c;
            self.len += 1;
        }
    }

    fn get(&self, i: usize) -> &[u16] {
        if i >= self.count {
            return &[];
        }
        let start = if i == 0 { 0 } else { self.ends[i - 1] };
        &self.chars[start..self.ends[i]]
    }

    fn is(&self, i: usize, word: &str) -> bool {
        eq_ignore_case(self.get(i), word)
    }
}

fn eq_ignore_case(a: &[u16], b: &str) -> bool {
    a.len() == b.len() && a.iter().zip(b.bytes()).all(|(&x, y)| lower(x) == u16::from(lower8(y)))
}

fn lower(c: u16) -> u16 {
    if (u16::from(b'A')..=u16::from(b'Z')).contains(&c) { c + 32 } else { c }
}

fn lower8(c: u8) -> u8 {
    c.to_ascii_lowercase()
}

fn args() -> Args {
    let mut a = Args { chars: [0; MAX_CHARS], len: 0, ends: [0; MAX_ARGS], count: 0 };
    // SAFETY: the process's NUL-terminated command line.
    let cmd = unsafe {
        let p = GetCommandLineW();
        let mut n = 0;
        while *p.add(n) != 0 {
            n += 1;
        }
        core::slice::from_raw_parts(p, n)
    };
    let (space, tab, quote, backslash) = (32u16, 9u16, 34u16, 92u16);
    let mut i = 0;
    // argv[0]: to the next space, or quoted.
    if cmd.first() == Some(&quote) {
        i = 1;
        while i < cmd.len() && cmd[i] != quote {
            a.push(cmd[i]);
            i += 1;
        }
        i += 1;
    } else {
        while i < cmd.len() && cmd[i] != space && cmd[i] != tab {
            a.push(cmd[i]);
            i += 1;
        }
    }
    a.ends[0] = a.len;
    a.count = 1;
    loop {
        while i < cmd.len() && (cmd[i] == space || cmd[i] == tab) {
            i += 1;
        }
        if i >= cmd.len() || a.count == MAX_ARGS {
            return a;
        }
        let mut quoted = false;
        while i < cmd.len() && (quoted || (cmd[i] != space && cmd[i] != tab)) {
            if cmd[i] == backslash {
                let mut n = 0;
                while i < cmd.len() && cmd[i] == backslash {
                    n += 1;
                    i += 1;
                }
                if i < cmd.len() && cmd[i] == quote {
                    for _ in 0..n / 2 {
                        a.push(backslash);
                    }
                    if n % 2 == 1 {
                        a.push(quote);
                        i += 1;
                    }
                } else {
                    for _ in 0..n {
                        a.push(backslash);
                    }
                }
            } else if cmd[i] == quote {
                if quoted && i + 1 < cmd.len() && cmd[i + 1] == quote {
                    a.push(quote);
                    i += 2;
                } else {
                    quoted = !quoted;
                    i += 1;
                }
            } else {
                a.push(cmd[i]);
                i += 1;
            }
        }
        a.ends[a.count] = a.len;
        a.count += 1;
    }
}

/// A number: decimal (with a sign) or 0x hexadecimal.
fn number(s: &[u16]) -> Option<i64> {
    let (neg, s) = match s.first() {
        Some(&c) if c == u16::from(b'-') => (true, &s[1..]),
        _ => (false, s),
    };
    let (radix, digits) =
        if s.len() > 2 && s[0] == u16::from(b'0') && lower(s[1]) == u16::from(b'x') {
            (16, &s[2..])
        } else {
            (10, s)
        };
    if digits.is_empty() || digits.len() > 16 {
        return None;
    }
    let mut v: i64 = 0;
    for &c in digits {
        let d = char::from_u32(u32::from(lower(c)))?.to_digit(radix)?;
        v = v.checked_mul(i64::from(radix))?.checked_add(i64::from(d))?;
    }
    Some(if neg { -v } else { v })
}

// ---- GalaDesktopAction ------------------------------------------------

/// A request: the action, a window, four numbers, a text.
fn request(
    action: u32,
    hwnd: u64,
    numbers: [i32; 4],
    text: &[u16],
    out: &mut [u8],
) -> (u32, usize) {
    let mut b = [0u8; REQUEST_HEADER + 2 * MAX_TEXT];
    let len = text.len().min(MAX_TEXT);
    b[0..4].copy_from_slice(&REQUEST_VERSION.to_le_bytes());
    b[4..8].copy_from_slice(&action.to_le_bytes());
    b[8..16].copy_from_slice(&hwnd.to_le_bytes());
    for (i, v) in numbers.iter().enumerate() {
        b[16 + 4 * i..20 + 4 * i].copy_from_slice(&v.to_le_bytes());
    }
    b[32..36].copy_from_slice(&(len as u32).to_le_bytes());
    for (i, c) in text[..len].iter().enumerate() {
        b[REQUEST_HEADER + 2 * i..REQUEST_HEADER + 2 + 2 * i].copy_from_slice(&c.to_le_bytes());
    }
    let mut returned = 0u32;
    // SAFETY: our buffers, with their lengths.
    let status = unsafe {
        GalaDesktopAction(
            b.as_ptr(),
            (REQUEST_HEADER + 2 * len) as u32,
            out.as_mut_ptr(),
            out.len() as u32,
            &mut returned,
        )
    };
    (status, returned as usize)
}

/// A window as listed.
#[derive(Clone, Copy)]
struct Window {
    hwnd: u64,
    pid: u32,
    flags: u32,
    rect: [i32; 4],
    title: [u16; TITLE],
    title_len: usize,
}

impl Window {
    fn title(&self) -> &[u16] {
        &self.title[..self.title_len]
    }
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

/// The taskbar's windows (at most MAX_WINDOWS); how many, or the status.
fn windows(list: &mut [Option<Window>; MAX_WINDOWS]) -> Result<usize, u32> {
    let mut b = [0u8; LIST_HEADER + MAX_WINDOWS * WINDOW_ENTRY];
    let (status, n) = request(ACTION_WINDOWS, 0, [0; 4], &[], &mut b);
    if status != STATUS_SUCCESS {
        return Err(status);
    }
    let count = ((n.saturating_sub(LIST_HEADER)) / WINDOW_ENTRY).min(MAX_WINDOWS);
    for (i, slot) in list.iter_mut().enumerate().take(count) {
        let e = &b[LIST_HEADER + i * WINDOW_ENTRY..];
        let mut w = Window {
            hwnd: u64::from(u32_at(e, 0)) | u64::from(u32_at(e, 4)) << 32,
            pid: u32_at(e, 8),
            flags: u32_at(e, 16),
            rect: [
                u32_at(e, 20) as i32,
                u32_at(e, 24) as i32,
                u32_at(e, 28) as i32,
                u32_at(e, 32) as i32,
            ],
            title: [0; TITLE],
            title_len: (u32_at(e, 36) as usize).min(TITLE),
        };
        for (k, c) in w.title.iter_mut().enumerate() {
            *c = u16::from_le_bytes([e[40 + 2 * k], e[41 + 2 * k]]);
        }
        *slot = Some(w);
    }
    Ok(count)
}

/// The window `name` names: an HWND, or a title (whole, or a part only
/// one title has; any case).
fn find_window(name: &[u16]) -> Result<u64, u32> {
    if let Some(hwnd) = number(name).filter(|&v| v > 0) {
        return Ok(hwnd as u64);
    }
    let mut list = [None; MAX_WINDOWS];
    let n = windows(&mut list).map_err(|s| status_error(s, "listed"))?;
    let same = |w: &Window| {
        w.title_len == name.len() && w.title().iter().zip(name).all(|(&a, &b)| lower(a) == lower(b))
    };
    let contains = |w: &Window| {
        name.len() <= w.title_len
            && w.title()
                .windows(name.len())
                .any(|part| part.iter().zip(name).all(|(&a, &b)| lower(a) == lower(b)))
    };
    let all = || list[..n].iter().flatten();
    if let Some(w) = all().find(|w| same(w)) {
        return Ok(w.hwnd);
    }
    let mut found = all().filter(|w| contains(w));
    match (found.next(), found.next()) {
        (Some(w), None) => Ok(w.hwnd),
        (Some(_), Some(_)) => {
            let mut l = Line::new();
            l.s("several windows have \"").w(name).s("\" in their title; name one by its HWND");
            Err(fail(EXIT_NOT_FOUND, &[core::str::from_utf8(&l.b[..l.n]).unwrap_or("")]))
        }
        _ => {
            let mut l = Line::new();
            l.s("no window titled \"").w(name).s("\"");
            Err(fail(EXIT_NOT_FOUND, &[core::str::from_utf8(&l.b[..l.n]).unwrap_or("")]))
        }
    }
}

/// Arguments `from..to` joined by spaces (a title given unquoted).
fn joined(a: &Args, from: usize, to: usize, buf: &mut [u16; 256]) -> usize {
    let mut n = 0;
    for i in from..to {
        if i > from && n < buf.len() {
            buf[n] = 32;
            n += 1;
        }
        for &c in a.get(i) {
            if n < buf.len() {
                buf[n] = c;
                n += 1;
            }
        }
    }
    n
}

/// A window action on the window named by arguments `from..to`.
fn window_action(
    a: &Args,
    action: u32,
    numbers: [i32; 4],
    from: usize,
    to: usize,
    what: &str,
) -> u32 {
    if from >= to {
        return fail(EXIT_USAGE, &["which window? (gala help)"]);
    }
    let mut name = [0u16; 256];
    let n = joined(a, from, to, &mut name);
    let hwnd = match find_window(&name[..n]) {
        Ok(h) => h,
        Err(code) => return code,
    };
    let (status, _) = request(action, hwnd, numbers, &[], &mut []);
    if status != STATUS_SUCCESS {
        return status_error(status, what);
    }
    0
}

// ---- Commands ----------------------------------------------------------

fn start(a: &Args) -> u32 {
    if a.count < 3 {
        return fail(EXIT_USAGE, &["start what? (gala start <program> [arguments])"]);
    }
    // The program and its arguments, quoted again where they have spaces.
    let mut text = [0u16; MAX_TEXT];
    let mut n = 0;
    for i in 2..a.count {
        let arg = a.get(i);
        let quote = arg.contains(&32) || arg.is_empty();
        let parts: [&[u16]; 4] = [
            if i > 2 { &[32] } else { &[] },
            if quote { &[34] } else { &[] },
            arg,
            if quote { &[34] } else { &[] },
        ];
        for p in parts {
            for &c in p {
                if n < text.len() {
                    text[n] = c;
                    n += 1;
                }
            }
        }
    }
    let mut b = [0u8; 8];
    let (status, got) = request(ACTION_START, 0, [0; 4], &text[..n], &mut b);
    if status == STATUS_OBJECT_NAME_NOT_FOUND {
        let mut l = Line::new();
        l.s("no program \"").w(a.get(2)).s("\"");
        return fail(EXIT_NOT_FOUND, &[core::str::from_utf8(&l.b[..l.n]).unwrap_or("")]);
    }
    if status != STATUS_SUCCESS || got < 8 {
        return status_error(status, "started");
    }
    let pid = u64::from_le_bytes(b);
    Line::new().s("started ").w(a.get(2)).s(" (pid ").dec(pid as i64).s(")").print();
    0
}

fn list_windows() -> u32 {
    let mut list = [None; MAX_WINDOWS];
    let n = match windows(&mut list) {
        Ok(n) => n,
        Err(s) => return status_error(s, "listed"),
    };
    Line::new()
        .s("HWND")
        .to(10)
        .s("PID")
        .to(17)
        .s("STATE")
        .to(37)
        .s("X")
        .to(43)
        .s("Y")
        .to(49)
        .s("WIDTH")
        .to(55)
        .s("HEIGHT")
        .to(62)
        .s("TITLE")
        .print();
    for w in list[..n].iter().flatten() {
        let mut l = Line::new();
        l.hex(w.hwnd, 5).to(10).dec(i64::from(w.pid)).to(17);
        let mut first = true;
        for (bit, name) in [(4, "active"), (1, "minimized"), (2, "maximized"), (8, "snapped")] {
            if w.flags & bit != 0 {
                if !first {
                    l.s(",");
                }
                l.s(name);
                first = false;
            }
        }
        if first {
            l.s("normal");
        }
        l.to(37).dec(i64::from(w.rect[0])).to(43).dec(i64::from(w.rect[1])).to(49);
        l.dec(i64::from(w.rect[2])).to(55).dec(i64::from(w.rect[3])).to(62).w(w.title()).print();
    }
    0
}

fn list_programs() -> u32 {
    let mut b = [0u8; LIST_HEADER + 16 * PROGRAM_ENTRY];
    let (status, n) = request(ACTION_PROGRAMS, 0, [0; 4], &[], &mut b);
    if status != STATUS_SUCCESS {
        return status_error(status, "listed");
    }
    let count = n.saturating_sub(LIST_HEADER) / PROGRAM_ENTRY;
    Line::new().s("NAME").to(20).s("PATH").print();
    for i in 0..count {
        let e = &b[LIST_HEADER + i * PROGRAM_ENTRY..];
        let (name_len, path_len) = (u32_at(e, 0) as usize, u32_at(e, 4) as usize);
        let mut l = Line::new();
        for k in 0..name_len.min(32) {
            l.bytes(&[e[8 + 2 * k]]);
        }
        l.to(20);
        for k in 0..path_len.min(128) {
            l.bytes(&[e[8 + 64 + 2 * k]]);
        }
        l.print();
    }
    0
}

/// A process from SystemProcessInformation: (pid, parent, threads, name).
fn for_each_process(mut f: impl FnMut(u32, u32, u32, &[u16])) -> Result<(), u32> {
    // A 256 KiB buffer from the stack would be large; the process heap is
    // not linked: a static one (this program is single-threaded).
    static mut BUF: [u8; 256 * 1024] = [0; 256 * 1024];
    // SAFETY: the only reference to BUF while this runs (one thread).
    let buf = unsafe { &mut *core::ptr::addr_of_mut!(BUF) };
    let mut ret = 0u32;
    // SAFETY: our buffer with its length.
    let status =
        unsafe { NtQuerySystemInformation(5, buf.as_mut_ptr(), buf.len() as u32, &mut ret) };
    if status == STATUS_INFO_LENGTH_MISMATCH {
        return Err(fail(EXIT_FAILED, &["too many processes to list"]));
    }
    if status != STATUS_SUCCESS {
        return Err(status_error(status, "listed"));
    }
    let mut at = 0usize;
    loop {
        let e = &buf[at..];
        let threads = u32_at(e, 4);
        let name_len = usize::from(u16::from_le_bytes([e[0x38], e[0x39]])) / 2;
        let name_ptr = u64::from(u32_at(e, 0x40)) | u64::from(u32_at(e, 0x44)) << 32;
        let pid = u32_at(e, 0x50);
        let parent = u32_at(e, 0x58);
        let name: &[u16] = if name_ptr == 0 || name_len == 0 {
            &[]
        } else {
            // SAFETY: ImageName.Buffer points into our buffer (the kernel
            // wrote it so), name_len units long.
            unsafe { core::slice::from_raw_parts(name_ptr as *const u16, name_len) }
        };
        f(pid, parent, threads, name);
        let next = u32_at(e, 0) as usize;
        if next == 0 || at + next >= buf.len() {
            return Ok(());
        }
        at += next;
    }
}

fn ps() -> u32 {
    Line::new().s("PID").to(7).s("PARENT").to(15).s("THREADS").to(24).s("NAME").print();
    let result = for_each_process(|pid, parent, threads, name| {
        let mut l = Line::new();
        l.dec(i64::from(pid)).to(7).dec(i64::from(parent)).to(15).dec(i64::from(threads)).to(24);
        if name.is_empty() {
            l.s(if pid == 0 { "System Idle Process" } else { "?" });
        } else {
            l.w(name);
        }
        l.print();
    });
    match result {
        Ok(()) => 0,
        Err(code) => code,
    }
}

/// Whether process image `name` is `want` (any case; ".exe" optional).
fn same_program(name: &[u16], want: &[u16]) -> bool {
    let eq = |a: &[u16], b: &[u16]| {
        a.len() == b.len() && a.iter().zip(b).all(|(&x, &y)| lower(x) == lower(y))
    };
    if eq(name, want) {
        return true;
    }
    name.len() == want.len() + 4
        && eq(&name[..want.len()], want)
        && eq(&name[want.len()..], &[46, 101, 120, 101])
}

fn kill(a: &Args) -> u32 {
    if a.count != 3 {
        return fail(EXIT_USAGE, &["kill what? (gala kill <pid|name>)"]);
    }
    let target = a.get(2);
    // SAFETY: a plain call.
    let me = unsafe { GetCurrentProcessId() };
    let mut pids = [0u32; 64];
    let mut n = 0;
    match number(target) {
        Some(pid) if pid > 0 && pid <= i64::from(u32::MAX) => {
            pids[0] = pid as u32;
            n = 1;
        }
        _ => {
            let result = for_each_process(|pid, _, _, name| {
                if pid != me && pid != 0 && n < pids.len() && same_program(name, target) {
                    pids[n] = pid;
                    n += 1;
                }
            });
            if let Err(code) = result {
                return code;
            }
            if n == 0 {
                let mut l = Line::new();
                l.s("no process named \"").w(target).s("\"");
                return fail(EXIT_NOT_FOUND, &[core::str::from_utf8(&l.b[..l.n]).unwrap_or("")]);
            }
        }
    }
    let mut code = 0;
    for &pid in &pids[..n] {
        // SAFETY: plain calls; the handle is closed.
        unsafe {
            // PROCESS_TERMINATE | SYNCHRONIZE: it is ended and waited for,
            // so `gala kill` returns once the process is gone
            // (TerminateProcess itself does not wait).
            let h = OpenProcess(0x0010_0001, 0, pid);
            if h.is_null() {
                let error = GetLastError();
                let mut l = Line::new();
                l.s("cannot open process ").dec(i64::from(pid));
                if error == 87 {
                    l.s(": there is none");
                    code = EXIT_NOT_FOUND;
                } else {
                    l.s(": error ").dec(i64::from(error));
                    code = EXIT_FAILED;
                }
                fail(code, &[core::str::from_utf8(&l.b[..l.n]).unwrap_or("")]);
                continue;
            }
            if TerminateProcess(h, 1) == 0 {
                let mut l = Line::new();
                l.s("cannot end process ")
                    .dec(i64::from(pid))
                    .s(": error ")
                    .dec(i64::from(GetLastError()));
                code = fail(EXIT_FAILED, &[core::str::from_utf8(&l.b[..l.n]).unwrap_or("")]);
            } else {
                WaitForSingleObject(h, 10_000);
                Line::new().s("ended process ").dec(i64::from(pid)).print();
            }
            CloseHandle(h);
        }
    }
    code
}

/// The package commands and their winget aliases (module `pkg`).
const PACKAGE_COMMANDS: [&str; 11] = [
    "install",
    "add",
    "search",
    "find",
    "list",
    "ls",
    "upgrade",
    "update",
    "uninstall",
    "remove",
    "rm",
];

fn run() -> u32 {
    let a = args();
    if a.count < 2 {
        write_to(-12i32 as u32, USAGE.as_bytes());
        return EXIT_USAGE;
    }
    let simple = [
        ("activate", ACTION_ACTIVATE, "activated"),
        ("minimize", ACTION_MINIMIZE, "minimized"),
        ("maximize", ACTION_MAXIMIZE, "maximized"),
        ("restore", ACTION_RESTORE, "restored"),
        ("close", ACTION_CLOSE, "closed"),
    ];
    for (word, action, what) in simple {
        if a.is(1, word) {
            return window_action(&a, action, [0; 4], 2, a.count, what);
        }
    }
    if a.is(1, "help") || a.is(1, "-h") || a.is(1, "--help") || a.is(1, "/?") {
        out(USAGE);
        0
    } else if a.is(1, "start") {
        start(&a)
    } else if a.is(1, "windows") {
        list_windows()
    } else if a.is(1, "programs") {
        list_programs()
    } else if a.is(1, "snap") {
        let side = ["left", "right", "up", "down"].iter().position(|s| a.is(2, s));
        let Some(side) = side else {
            return fail(EXIT_USAGE, &["snap where? (gala snap left|right|up|down <window>)"]);
        };
        window_action(&a, ACTION_SNAP, [side as i32, 0, 0, 0], 3, a.count, "snapped")
    } else if a.is(1, "move") {
        if a.count < 7 {
            return fail(EXIT_USAGE, &["gala move <window> <x> <y> <width> <height>"]);
        }
        let mut v = [0i32; 4];
        for (k, slot) in v.iter_mut().enumerate() {
            match number(a.get(a.count - 4 + k)).and_then(|n| i32::try_from(n).ok()) {
                Some(n) => *slot = n,
                None => return fail(EXIT_USAGE, &["x, y, width and height are numbers"]),
            }
        }
        if v[2] <= 0 || v[3] <= 0 {
            return fail(EXIT_USAGE, &["the width and height must be more than 0"]);
        }
        window_action(&a, ACTION_MOVE, v, 2, a.count - 4, "moved")
    } else if a.is(1, "wallpaper") {
        if a.count != 3 {
            return fail(EXIT_USAGE, &["which wallpaper? (dunes, ribbons, najdi, night, drawn)"]);
        }
        let (status, _) = request(ACTION_WALLPAPER, 0, [0; 4], a.get(2), &mut []);
        if status == STATUS_OBJECT_NAME_NOT_FOUND {
            return fail(
                EXIT_NOT_FOUND,
                &["no such wallpaper (dunes, ribbons, najdi, night, drawn)"],
            );
        }
        if status != STATUS_SUCCESS {
            return status_error(status, "changed");
        }
        0
    } else if PACKAGE_COMMANDS.iter().any(|w| a.is(1, w)) {
        pkg::run(&a)
    } else if a.is(1, "ps") {
        ps()
    } else if a.is(1, "kill") {
        kill(&a)
    } else {
        let mut l = Line::new();
        l.s("unknown command \"").w(a.get(1)).s("\" (gala help)");
        fail(EXIT_USAGE, &[core::str::from_utf8(&l.b[..l.n]).unwrap_or("")])
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    // A command shim (gala.exe copied into WinGet\Links), else gala.
    let code = pkg::shim_main().unwrap_or_else(run);
    // SAFETY: ends the process.
    unsafe { ExitProcess(code) }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdead) }
}

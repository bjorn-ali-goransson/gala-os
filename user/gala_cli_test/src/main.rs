//! The `gala` command drives shasha (ROADMAP SD "CLI parity"): a GUI
//! program is started, listed, snapped, maximized and restored, moved,
//! minimized and brought back, and closed through `gala` alone, and the
//! program sees what the mouse and keys would have given it. gala's own
//! command, so there is no Windows run to compare with; what the program
//! sees is what Windows documents for the same user actions:
//!
//! - Snap and Move: the window's rectangle changes (WM_SIZE, or only
//!   WM_MOVE when the size stays); the window
//!   registry (FindWindowW, GetWindowRect across processes) agrees with
//!   `gala windows`, and the HWND `gala windows` prints is FindWindowW's.
//! - Maximize: WM_SYSCOMMAND SC_MAXIMIZE, after which IsZoomed; Restore:
//!   SC_RESTORE, back to the rectangle it had.
//! - Minimize: WM_SIZE SIZE_MINIMIZED (IsIconic from another process);
//!   Activate brings it back.
//! - Close: WM_SYSCOMMAND SC_CLOSE, then WM_CLOSE (DefWindowProc); the
//!   program ends, the window is gone from the list and the registry.
//! - `gala ps` lists the program, `gala kill <pid>` and `gala kill
//!   <name>` end processes; the exit codes say what went wrong (1 usage,
//!   2 not found, 3 no graphical shell).
//!
//! Modes: no argument: the above, on shasha (scenario gala_cli, UEFI:
//! xtask checks the screen once it prints `gala_cli_test: snapped left`
//! and then types `g`, which the window shows in its title). `server`:
//! on the server kernel or with `profile=server` the window commands fail
//! with exit code 3 and start, ps and kill work (scenarios server_kernel,
//! server_profile). `target`: the GUI program (the window "CLI target").
//!
//! Prints `gala_cli_test: ok` (the target `gala_cli_test target: ok`) or
//! exits with the number of the failing check.

#![no_std]
#![no_main]

use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, Ordering};

type Bool = i32;
type Handle = *mut c_void;
type Hwnd = *mut c_void;
type WndProc = extern "system" fn(Hwnd, u32, usize, isize) -> isize;

#[repr(C)]
struct SecurityAttributes {
    length: u32,
    descriptor: *mut c_void,
    inherit: Bool,
}

#[repr(C)]
struct WndClassExW {
    size: u32,
    style: u32,
    proc_: WndProc,
    class_extra: i32,
    window_extra: i32,
    instance: Handle,
    icon: Handle,
    cursor: Handle,
    background: Handle,
    menu_name: *const u16,
    class_name: *const u16,
    icon_small: Handle,
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

#[repr(C)]
struct PaintStruct {
    hdc: Handle,
    erase: i32,
    paint: Rect,
    restore: i32,
    inc_update: i32,
    reserved: [u8; 32],
}

#[repr(C)]
struct Msg {
    hwnd: Hwnd,
    message: u32,
    wparam: usize,
    lparam: isize,
    time: u32,
    pt: [i32; 2],
    private: u32,
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
    fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
    fn GetExitCodeProcess(h: Handle, code: *mut u32) -> Bool;
    fn CreatePipe(
        read: *mut Handle,
        write: *mut Handle,
        sa: *const SecurityAttributes,
        size: u32,
    ) -> Bool;
    fn SetHandleInformation(h: Handle, mask: u32, flags: u32) -> Bool;
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
    fn OpenProcess(access: u32, inherit: Bool, id: u32) -> Handle;
    fn Sleep(ms: u32);
    fn GetTickCount() -> u32;
    fn GetModuleHandleW(name: *const u16) -> Handle;
}

#[link(name = "user32", kind = "raw-dylib")]
unsafe extern "system" {
    fn RegisterClassExW(class: *const WndClassExW) -> u16;
    fn CreateWindowExW(
        ex_style: u32,
        class: *const u16,
        name: *const u16,
        style: u32,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        parent: Hwnd,
        menu: Handle,
        instance: Handle,
        param: *mut c_void,
    ) -> Hwnd;
    fn DefWindowProcW(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize;
    fn ShowWindow(hwnd: Hwnd, cmd: i32) -> i32;
    fn UpdateWindow(hwnd: Hwnd) -> i32;
    fn BeginPaint(hwnd: Hwnd, ps: *mut PaintStruct) -> Handle;
    fn EndPaint(hwnd: Hwnd, ps: *const PaintStruct) -> i32;
    fn FillRect(hdc: Handle, rect: *const Rect, brush: Handle) -> i32;
    fn GetClientRect(hwnd: Hwnd, rect: *mut Rect) -> i32;
    fn GetWindowRect(hwnd: Hwnd, rect: *mut Rect) -> i32;
    fn GetMessageW(msg: *mut Msg, hwnd: Hwnd, min: u32, max: u32) -> i32;
    fn TranslateMessage(msg: *const Msg) -> i32;
    fn DispatchMessageW(msg: *const Msg) -> isize;
    fn PostQuitMessage(code: i32);
    fn SetWindowTextW(hwnd: Hwnd, text: *const u16) -> i32;
    fn IsZoomed(hwnd: Hwnd) -> i32;
    fn IsIconic(hwnd: Hwnd) -> i32;
    fn IsWindow(hwnd: Hwnd) -> i32;
    fn FindWindowW(class: *const u16, name: *const u16) -> Hwnd;
    fn GetSystemMetrics(index: i32) -> i32;
}

#[link(name = "gdi32", kind = "raw-dylib")]
unsafe extern "system" {
    fn CreateSolidBrush(color: u32) -> Handle;
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

const STARTF_USESTDHANDLES: u32 = 0x100;
const SYNCHRONIZE: u32 = 0x0010_0000;
const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;

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

/// UTF-16 of an ASCII string, NUL-terminated, in a fixed buffer.
struct W {
    b: [u16; 512],
    n: usize,
}

impl W {
    fn new(text: &str) -> W {
        let mut w = W { b: [0; 512], n: 0 };
        w.add(text.as_bytes());
        w
    }

    fn add(&mut self, text: &[u8]) -> &mut W {
        for &c in text {
            if self.n < self.b.len() - 1 {
                self.b[self.n] = u16::from(c);
                self.n += 1;
            }
        }
        self
    }

    fn ptr(&mut self) -> *mut u16 {
        self.b[self.n] = 0;
        self.b.as_mut_ptr()
    }
}

// ---- The target: a window that records what it is told ----------------

const TITLE: &str = "CLI target";
const CLASS: &str = "GalaCliTarget";
/// The client area's colour (COLORREF 0x00BBGGRR: RGB 2e 8b 57).
const GREEN: u32 = 0x0057_8b2e;

const WM_DESTROY: u32 = 0x0002;
const WM_MOVE: u32 = 0x0003;
const WM_SIZE: u32 = 0x0005;
const WM_PAINT: u32 = 0x000f;
const WM_CLOSE: u32 = 0x0010;
const WM_KEYDOWN: u32 = 0x0100;
const WM_SYSCOMMAND: u32 = 0x0112;
const SC_MAXIMIZE: usize = 0xf030;
const SC_CLOSE: usize = 0xf060;
const SC_RESTORE: usize = 0xf120;
const SIZE_MINIMIZED: usize = 1;
const WS_OVERLAPPEDWINDOW: u32 = 0x00cf_0000;

/// What the target saw (bits, in the order the driver asks).
const SAW_SNAP_LEFT: u32 = 1;
const SAW_KEY: u32 = 2;
const SAW_MAXIMIZE: u32 = 4;
const SAW_RESTORE: u32 = 8;
const SAW_SNAP_RIGHT: u32 = 16;
const SAW_MOVE: u32 = 32;
const SAW_MINIMIZE: u32 = 64;
const SAW_SC_CLOSE: u32 = 128;
const SAW_CLOSE: u32 = 256;
const SAW_ALL: u32 = 511;

static SAW: AtomicU32 = AtomicU32::new(0);

fn saw(bit: u32, what: &[u8]) {
    if SAW.fetch_or(bit, Ordering::Relaxed) & bit == 0 {
        // One write per line, so the kernel's log cannot split it.
        let mut line = [0u8; 96];
        let prefix = b"gala_cli_test target: ";
        let n = what.len().min(line.len() - prefix.len() - 2);
        line[..prefix.len()].copy_from_slice(prefix);
        line[prefix.len()..prefix.len() + n].copy_from_slice(&what[..n]);
        let end = prefix.len() + n;
        line[end..end + 2].copy_from_slice(b"\r\n");
        out(&line[..end + 2]);
    }
}

extern "system" fn target_proc(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize {
    // SAFETY: plain calls on our window.
    unsafe {
        match msg {
            WM_PAINT => {
                let mut ps = PaintStruct {
                    hdc: core::ptr::null_mut(),
                    erase: 0,
                    paint: Rect::default(),
                    restore: 0,
                    inc_update: 0,
                    reserved: [0; 32],
                };
                let dc = BeginPaint(hwnd, &mut ps);
                let mut r = Rect::default();
                GetClientRect(hwnd, &mut r);
                FillRect(dc, &r, CreateSolidBrush(GREEN));
                EndPaint(hwnd, &ps);
                0
            }
            WM_SYSCOMMAND => {
                let r = DefWindowProcW(hwnd, msg, wparam, lparam);
                match wparam & 0xfff0 {
                    SC_MAXIMIZE if IsZoomed(hwnd) != 0 => saw(SAW_MAXIMIZE, b"maximized"),
                    SC_RESTORE
                        if IsZoomed(hwnd) == 0
                            && SAW.load(Ordering::Relaxed) & SAW_MAXIMIZE != 0 =>
                    {
                        saw(SAW_RESTORE, b"restored")
                    }
                    SC_CLOSE => saw(SAW_SC_CLOSE, b"SC_CLOSE"),
                    _ => {}
                }
                r
            }
            WM_SIZE if wparam == SIZE_MINIMIZED => {
                saw(SAW_MINIMIZE, b"minimized");
                0
            }
            // A move without a new size (snapped from one half to the
            // other) is WM_MOVE only.
            WM_SIZE | WM_MOVE => {
                let mut r = Rect::default();
                GetWindowRect(hwnd, &mut r);
                let half = GetSystemMetrics(0) / 2;
                if (r.left, r.top, r.right) == (0, 0, half) {
                    saw(SAW_SNAP_LEFT, b"snapped left");
                } else if (r.left, r.top, r.right) == (half, 0, 2 * half) {
                    saw(SAW_SNAP_RIGHT, b"snapped right");
                } else if r == (Rect { left: 100, top: 80, right: 600, bottom: 480 }) {
                    saw(SAW_MOVE, b"moved");
                }
                0
            }
            WM_KEYDOWN if wparam == usize::from(b'G') => {
                saw(SAW_KEY, b"key g");
                SetWindowTextW(hwnd, W::new("CLI target go").ptr());
                0
            }
            WM_CLOSE => {
                saw(SAW_CLOSE, b"WM_CLOSE");
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                0
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

/// The target's window and message loop; 0 if it saw everything.
fn target() -> u32 {
    // SAFETY: plain calls with our structures and strings.
    unsafe {
        let instance = GetModuleHandleW(core::ptr::null());
        let mut class = W::new(CLASS);
        let wc = WndClassExW {
            size: core::mem::size_of::<WndClassExW>() as u32,
            style: 3,
            proc_: target_proc,
            class_extra: 0,
            window_extra: 0,
            instance,
            icon: core::ptr::null_mut(),
            cursor: core::ptr::null_mut(),
            background: CreateSolidBrush(GREEN),
            menu_name: core::ptr::null(),
            class_name: class.ptr(),
            icon_small: core::ptr::null_mut(),
        };
        if RegisterClassExW(&wc) == 0 {
            return 101;
        }
        let hwnd = CreateWindowExW(
            0,
            class.ptr(),
            W::new(TITLE).ptr(),
            WS_OVERLAPPEDWINDOW,
            200,
            150,
            480,
            320,
            core::ptr::null_mut(),
            core::ptr::null_mut(),
            instance,
            core::ptr::null_mut(),
        );
        if hwnd.is_null() {
            return 102;
        }
        ShowWindow(hwnd, 1);
        UpdateWindow(hwnd);
        out(b"gala_cli_test target: up\r\n");
        let mut msg = Msg {
            hwnd: core::ptr::null_mut(),
            message: 0,
            wparam: 0,
            lparam: 0,
            time: 0,
            pt: [0; 2],
            private: 0,
        };
        while GetMessageW(&mut msg, core::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    let seen = SAW.load(Ordering::Relaxed);
    if seen != SAW_ALL {
        out(b"gala_cli_test target: saw ");
        out(&hex(seen));
        out(b"\r\n");
        return 103;
    }
    0
}

// ---- The driver: runs `gala` ----------------------------------------------

/// `gala`'s output (stdout and stderr together).
struct Output {
    b: [u8; 8192],
    n: usize,
}

impl Output {
    fn text(&self) -> &[u8] {
        &self.b[..self.n]
    }

    fn has(&self, what: &[u8]) -> bool {
        contains(self.text(), what)
    }

    fn lines(&self) -> impl Iterator<Item = &[u8]> {
        self.text().split(|&c| c == b'\n').map(|l| l.strip_suffix(b"\r").unwrap_or(l))
    }
}

fn contains(hay: &[u8], what: &[u8]) -> bool {
    what.is_empty() || hay.windows(what.len()).any(|w| w == what)
}

/// Runs `gala <args>`: its exit code and output.
fn gala(args: &str) -> (u32, Output) {
    let mut o = Output { b: [0; 8192], n: 0 };
    let null = core::ptr::null_mut();
    let sa = SecurityAttributes { length: 24, descriptor: null, inherit: 1 };
    let (mut r, mut w) = (null, null);
    let mut line = W::new("gala.exe ");
    line.add(args.as_bytes());
    let mut exe = W::new("C:\\gala\\gala.exe");
    // SAFETY: plain calls with our buffers and handles.
    unsafe {
        if CreatePipe(&mut r, &mut w, &sa, 0) == 0 {
            return (0xffff_0000 | GetLastError(), o);
        }
        SetHandleInformation(r, 1, 0);
        let mut startup = [0u8; 104];
        startup[0] = 104;
        startup[60..64].copy_from_slice(&STARTF_USESTDHANDLES.to_le_bytes());
        startup[80..88].copy_from_slice(&(GetStdHandle(-10i32 as u32) as u64).to_le_bytes());
        startup[88..96].copy_from_slice(&(w as u64).to_le_bytes());
        startup[96..104].copy_from_slice(&(w as u64).to_le_bytes());
        let mut info = [0usize; 3];
        let ok = CreateProcessW(
            exe.ptr(),
            line.ptr(),
            null,
            null,
            1,
            0,
            null,
            core::ptr::null(),
            startup.as_ptr(),
            &mut info,
        );
        CloseHandle(w);
        if ok == 0 {
            CloseHandle(r);
            return (0xfffe_0000 | GetLastError(), o);
        }
        let mut got = 0;
        while o.n < o.b.len()
            && ReadFile(r, o.b.as_mut_ptr().add(o.n), (o.b.len() - o.n) as u32, &mut got, null) != 0
            && got > 0
        {
            o.n += got as usize;
        }
        CloseHandle(r);
        WaitForSingleObject(info[0] as Handle, 30_000);
        let mut code = 0xdead_0000;
        GetExitCodeProcess(info[0] as Handle, &mut code);
        CloseHandle(info[0] as Handle);
        CloseHandle(info[1] as Handle);
        (code, o)
    }
}

struct Checks(core::cell::Cell<u32>);

impl Checks {
    fn ok(&self, cond: bool, got: u64) -> Result<(), u32> {
        self.0.set(self.0.get() + 1);
        if cond {
            return Ok(());
        }
        out(b"gala_cli_test: check ");
        out(&hex(self.0.get()));
        out(b" got ");
        out(&hex(got as u32));
        out(b"\r\n");
        Err(self.0.get())
    }

    /// `gala <args>` exits with `code` and its output has `has`.
    fn run(&self, args: &str, code: u32, has: &[u8]) -> Result<Output, u32> {
        let (got, o) = gala(args);
        if got != code || !o.has(has) {
            out(b"gala_cli_test: gala ");
            out(args.as_bytes());
            out(b" -> ");
            out(&hex(got));
            out(b":\r\n");
            out(o.text());
        }
        self.ok(got == code && o.has(has), got.into())?;
        Ok(o)
    }
}

/// The fields of a `gala windows` line.
#[derive(Clone, Copy)]
struct Listed {
    hwnd: [u8; 20],
    hwnd_len: usize,
    state: [u8; 48],
    state_len: usize,
    rect: [i64; 4],
    title: [u8; 64],
    title_len: usize,
}

impl Listed {
    fn hwnd(&self) -> &str {
        core::str::from_utf8(&self.hwnd[..self.hwnd_len]).unwrap_or("")
    }

    fn state(&self) -> &[u8] {
        &self.state[..self.state_len]
    }

    fn title(&self) -> &[u8] {
        &self.title[..self.title_len]
    }

    fn hwnd_value(&self) -> usize {
        let digits = self.hwnd().strip_prefix("0x").unwrap_or("");
        usize::from_str_radix(digits, 16).unwrap_or(0)
    }
}

fn parse_number(s: &[u8]) -> Option<i64> {
    let (neg, digits) = match s.strip_prefix(b"-") {
        Some(rest) => (true, rest),
        None => (false, s),
    };
    let mut v: i64 = 0;
    if digits.is_empty() {
        return None;
    }
    for &c in digits {
        if !c.is_ascii_digit() {
            return None;
        }
        v = v * 10 + i64::from(c - b'0');
    }
    Some(if neg { -v } else { v })
}

/// A `gala windows` line: HWND PID STATE X Y WIDTH HEIGHT TITLE.
fn parse_line(line: &[u8]) -> Option<Listed> {
    let mut fields = [&line[..0]; 7];
    let mut rest = line;
    for f in fields.iter_mut() {
        let start = rest.iter().position(|&c| c != b' ')?;
        rest = &rest[start..];
        let end = rest.iter().position(|&c| c == b' ').unwrap_or(rest.len());
        *f = &rest[..end];
        rest = &rest[end..];
    }
    let title = rest.iter().position(|&c| c != b' ').map_or(&rest[..0], |s| &rest[s..]);
    if !fields[0].starts_with(b"0x") {
        return None;
    }
    let mut l = Listed {
        hwnd: [0; 20],
        hwnd_len: fields[0].len().min(20),
        state: [0; 48],
        state_len: fields[2].len().min(48),
        rect: [0; 4],
        title: [0; 64],
        title_len: title.len().min(64),
    };
    l.hwnd[..l.hwnd_len].copy_from_slice(&fields[0][..l.hwnd_len]);
    l.state[..l.state_len].copy_from_slice(&fields[2][..l.state_len]);
    l.title[..l.title_len].copy_from_slice(&title[..l.title_len]);
    for (r, f) in l.rect.iter_mut().zip(&fields[3..7]) {
        *r = parse_number(f)?;
    }
    Some(l)
}

/// The target's line in `gala windows` (its title starts with TITLE).
fn find_target() -> Option<Listed> {
    let (code, o) = gala("windows");
    if code != 0 {
        return None;
    }
    o.lines().filter_map(parse_line).find(|l| l.title().starts_with(TITLE.as_bytes()))
}

/// Waits (up to `ms`) for the target's line to satisfy `cond`.
fn wait_for(ms: u32, cond: impl Fn(Option<&Listed>) -> bool) -> Option<Listed> {
    // SAFETY: plain calls.
    let start = unsafe { GetTickCount() };
    loop {
        let found = find_target();
        if cond(found.as_ref()) {
            return found.or(Some(Listed {
                hwnd: [0; 20],
                hwnd_len: 0,
                state: [0; 48],
                state_len: 0,
                rect: [0; 4],
                title: [0; 64],
                title_len: 0,
            }));
        }
        // SAFETY: plain calls.
        if unsafe { GetTickCount() }.wrapping_sub(start) > ms {
            out(b"gala_cli_test: timed out; last line: ");
            if let Some(l) = found {
                out(l.hwnd().as_bytes());
                out(b" ");
                out(l.state());
                out(b" ");
                out(l.title());
            }
            out(b"\r\n");
            return None;
        }
        // SAFETY: a plain call.
        unsafe { Sleep(200) };
    }
}

/// Whether `cond` holds within 5 seconds.
fn eventually(cond: impl Fn() -> bool) -> bool {
    for _ in 0..50 {
        if cond() {
            return true;
        }
        // SAFETY: a plain call.
        unsafe { Sleep(100) };
    }
    cond()
}

fn has_state(l: &Listed, what: &[u8]) -> bool {
    contains(l.state(), what)
}

/// The pid `gala start` printed ("(pid N)").
fn started_pid(o: &Output) -> Option<u32> {
    let t = o.text();
    let at = t.windows(5).position(|w| w == b"(pid ")? + 5;
    let end = t[at..].iter().position(|&c| c == b')')? + at;
    parse_number(&t[at..end]).map(|v| v as u32)
}

fn args_after(a: &str, word: &str, buf: &mut [u8; 128]) -> usize {
    let mut n = 0;
    for part in [word.as_bytes(), b" ", a.as_bytes()] {
        buf[n..n + part.len()].copy_from_slice(part);
        n += part.len();
    }
    n
}

/// `gala <command> <hwnd> [more]` as one string.
fn cmd<'a>(buf: &'a mut [u8; 128], command: &str, hwnd: &str, more: &str) -> &'a str {
    let mut n = args_after(hwnd, command, buf);
    if !more.is_empty() {
        buf[n] = b' ';
        n += 1;
        buf[n..n + more.len()].copy_from_slice(more.as_bytes());
        n += more.len();
    }
    core::str::from_utf8(&buf[..n]).unwrap_or("")
}

/// Waits for process `h` to end (up to 30 s); its exit code.
fn wait_exit(h: Handle) -> u32 {
    let mut code = 0xdead_0001;
    // SAFETY: our handle.
    unsafe {
        WaitForSingleObject(h, 30_000);
        GetExitCodeProcess(h, &mut code);
    }
    code
}

fn open_process(pid: u32) -> Handle {
    // SAFETY: a plain call.
    unsafe { OpenProcess(SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) }
}

/// Whether `gala ps` lists process `pid`.
fn ps_has(pid: u32) -> bool {
    let (_, o) = gala("ps");
    o.lines().any(|l| {
        let l = l.iter().position(|&c| c != b' ').map_or(&l[..0], |s| &l[s..]);
        let end = l.iter().position(|&c| c == b' ').unwrap_or(l.len());
        parse_number(&l[..end]) == Some(i64::from(pid))
    })
}

fn desktop() -> Result<(), u32> {
    let c = Checks(core::cell::Cell::new(0));
    let mut buf = [0u8; 128];
    // Help, usage errors, things that do not exist.
    c.run("help", 0, b"usage: gala")?;
    c.run("frobnicate", 1, b"unknown command")?;
    c.run("snap sideways x", 1, b"snap where")?;
    c.run("close \"No such window\"", 2, b"no window titled")?;
    c.run("start nosuchprogram", 2, b"no program")?;
    c.run("wallpaper plaid", 2, b"no such wallpaper")?;
    // The start menu's programs (the boot image has gui_test).
    c.run("programs", 0, b"Hello window")?;

    // Start the target by its file name, with an argument.
    let o = c.run("start gala_cli_test.exe target", 0, b"started gala_cli_test.exe (pid ")?;
    let pid = started_pid(&o).unwrap_or(0);
    c.ok(pid != 0, 0)?;
    let process = open_process(pid);
    c.ok(!process.is_null(), pid.into())?;
    let first = wait_for(30_000, |l| l.is_some_and(|l| has_state(l, b"active")));
    let first = first.ok_or(c.0.get() + 1)?;
    let hwnd = first.hwnd();
    // The HWND is the registry's: FindWindowW from this process finds it.
    // SAFETY: plain calls; our strings.
    let found = unsafe { FindWindowW(W::new(CLASS).ptr(), core::ptr::null()) };
    c.ok(found as usize == first.hwnd_value() && found as usize != 0, found as u64)?;
    c.ok(first.rect == [200, 150, 480, 320], first.rect[0] as u64)?;

    // Snap left by title: the left half of the work area.
    // SAFETY: plain calls.
    let (sw, sh) = unsafe { (i64::from(GetSystemMetrics(0)), i64::from(GetSystemMetrics(1))) };
    let work_h = sh - 48;
    c.run("snap left CLI target", 0, b"")?;
    let left = [0, 0, sw / 2, work_h];
    let l = wait_for(10_000, |l| l.is_some_and(|l| l.rect == left && has_state(l, b"snapped")));
    c.ok(l.is_some(), 0)?;
    let mut r = Rect::default();
    // SAFETY: the target's window; our rectangle.
    unsafe { GetWindowRect(found, &mut r) };
    c.ok(
        r == Rect { left: 0, top: 0, right: (sw / 2) as i32, bottom: work_h as i32 },
        r.right as u64,
    )?;
    // The screen check, then a key typed into the window: its title.
    out(b"gala_cli_test: snapped left\r\n");
    let l = wait_for(90_000, |l| l.is_some_and(|l| l.title() == b"CLI target go"));
    c.ok(l.is_some(), 0)?;

    // Maximize and restore by HWND.
    c.run(cmd(&mut buf, "maximize", hwnd, ""), 0, b"")?;
    let full = [0, 0, sw, work_h];
    let l = wait_for(10_000, |l| l.is_some_and(|l| l.rect == full && has_state(l, b"maximized")));
    c.ok(l.is_some(), 0)?;
    // The registry follows (the program mirrors its state after moving).
    // SAFETY: the target's window.
    c.ok(eventually(|| unsafe { IsZoomed(found) } != 0), 0)?;
    c.run(cmd(&mut buf, "restore", hwnd, ""), 0, b"")?;
    let l = wait_for(10_000, |l| l.is_some_and(|l| l.rect == left && !has_state(l, b"maximized")));
    c.ok(l.is_some(), 0)?;
    // SAFETY: the target's window.
    c.ok(eventually(|| unsafe { IsZoomed(found) } == 0), 1)?;

    // Snap right, then move.
    c.run(cmd(&mut buf, "snap right", hwnd, ""), 0, b"")?;
    let right = [sw / 2, 0, sw - sw / 2, work_h];
    let l = wait_for(10_000, |l| l.is_some_and(|l| l.rect == right));
    c.ok(l.is_some(), 0)?;
    c.run(cmd(&mut buf, "move", hwnd, "100 80 500 400"), 0, b"")?;
    let l = wait_for(10_000, |l| {
        l.is_some_and(|l| l.rect == [100, 80, 500, 400] && !has_state(l, b"snapped"))
    });
    c.ok(l.is_some(), 0)?;
    c.run(cmd(&mut buf, "move", hwnd, "1 2 0 5"), 1, b"more than 0")?;

    // Minimize, then activate brings it back.
    c.run(cmd(&mut buf, "minimize", hwnd, ""), 0, b"")?;
    let l = wait_for(10_000, |l| l.is_some_and(|l| has_state(l, b"minimized")));
    c.ok(l.is_some(), 0)?;
    // SAFETY: the target's window.
    c.ok(eventually(|| unsafe { IsIconic(found) } != 0), 0)?;
    c.run(cmd(&mut buf, "activate", hwnd, ""), 0, b"")?;
    let l = wait_for(10_000, |l| {
        l.is_some_and(|l| has_state(l, b"active") && !has_state(l, b"minimized"))
    });
    c.ok(l.is_some(), 0)?;

    // The wallpaper at run time; the process list.
    c.run("wallpaper ribbons", 0, b"")?;
    c.ok(ps_has(pid), pid.into())?;

    // Close: the program ends having seen everything (exit code 0), and
    // its window is gone from the list and the registry.
    c.run(cmd(&mut buf, "close", hwnd, ""), 0, b"")?;
    let code = wait_exit(process);
    c.ok(code == 0, code.into())?;
    // SAFETY: our handle.
    unsafe { CloseHandle(process) };
    let l = wait_for(10_000, |l| l.is_none());
    c.ok(l.is_some(), 0)?;
    // SAFETY: a plain call.
    c.ok(unsafe { IsWindow(found) } == 0, 0)?;
    c.run(cmd(&mut buf, "close", hwnd, ""), 2, b"no such window")?;
    c.ok(!ps_has(pid), pid.into())?;

    // Kill by pid: a second target.
    let o = c.run("start gala_cli_test.exe target", 0, b"(pid ")?;
    let pid = started_pid(&o).unwrap_or(0);
    let process = open_process(pid);
    c.ok(!process.is_null(), pid.into())?;
    let l = wait_for(30_000, |l| l.is_some());
    c.ok(l.is_some(), 0)?;
    let mut num = [0u8; 16];
    let n = dec(pid, &mut num);
    let kill = cmd(&mut buf, "kill", core::str::from_utf8(&num[..n]).unwrap_or(""), "");
    c.run(kill, 0, b"ended process")?;
    let code = wait_exit(process);
    c.ok(code == 1, code.into())?;
    // SAFETY: our handle.
    unsafe { CloseHandle(process) };
    let l = wait_for(10_000, |l| l.is_none());
    c.ok(l.is_some(), 0)?;
    c.run("kill 999999", 2, b"cannot open process")?;
    // Kill by name: gui_test, started from the start menu's name.
    c.run("start \"Hello window\"", 0, b"started Hello window (pid ")?;
    // SAFETY: a plain call.
    unsafe { Sleep(500) };
    c.run("kill gui_test", 0, b"ended process")?;
    c.run("kill gui_test", 2, b"no process named")?;
    Ok(())
}

/// Without a graphical shell: window commands say so (exit code 3),
/// programs still start and end.
fn server() -> Result<(), u32> {
    let c = Checks(core::cell::Cell::new(0));
    let mut buf = [0u8; 128];
    c.run("windows", 3, b"no graphical shell")?;
    c.run("snap left 0x20010", 3, b"no graphical shell")?;
    c.run("maximize 0x20010", 3, b"no graphical shell")?;
    c.run("close 0x20010", 3, b"no graphical shell")?;
    c.run("wallpaper night", 3, b"no graphical shell")?;
    c.run("programs", 3, b"no graphical shell")?;
    // A window by title needs the list: not supported either.
    c.run("close CLI target", 3, b"no graphical shell")?;
    let o = c.run("start gala_cli_test.exe target", 0, b"started gala_cli_test.exe (pid ")?;
    let pid = started_pid(&o).unwrap_or(0);
    let process = open_process(pid);
    c.ok(!process.is_null(), pid.into())?;
    c.ok(ps_has(pid), pid.into())?;
    // Its window exists (the registry) though nothing is drawn.
    // SAFETY: plain calls; our string.
    let find = || unsafe { FindWindowW(W::new(CLASS).ptr(), core::ptr::null()) };
    c.ok(eventually(|| !find().is_null()), 0)?;
    let mut num = [0u8; 16];
    let n = dec(pid, &mut num);
    let kill = cmd(&mut buf, "kill", core::str::from_utf8(&num[..n]).unwrap_or(""), "");
    c.run(kill, 0, b"ended process")?;
    let code = wait_exit(process);
    c.ok(code == 1, code.into())?;
    // SAFETY: our handle.
    unsafe { CloseHandle(process) };
    c.ok(!ps_has(pid), pid.into())?;
    Ok(())
}

fn dec(v: u32, buf: &mut [u8; 16]) -> usize {
    let mut digits = [0u8; 10];
    let (mut n, mut u) = (0, v);
    loop {
        digits[n] = b'0' + (u % 10) as u8;
        n += 1;
        u /= 10;
        if u == 0 {
            break;
        }
    }
    for i in 0..n {
        buf[i] = digits[n - 1 - i];
    }
    n
}

/// The mode: the command line's last word.
fn mode() -> [u8; 8] {
    let mut m = [0u8; 8];
    // SAFETY: the process's NUL-terminated command line.
    unsafe {
        let p = GetCommandLineW();
        let mut n = 0;
        while *p.add(n) != 0 {
            n += 1;
        }
        let line = core::slice::from_raw_parts(p, n);
        let end = line.iter().rposition(|&c| c != 32).map_or(0, |e| e + 1);
        let start = line[..end].iter().rposition(|&c| c == 32 || c == 34).map_or(0, |s| s + 1);
        for (d, &s) in m.iter_mut().zip(&line[start..end]) {
            *d = s as u8;
        }
    }
    m
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let mode = mode();
    let code = if mode.starts_with(b"target\0") {
        let code = target();
        if code == 0 {
            out(b"gala_cli_test target: ok\r\n");
        }
        code
    } else {
        let result = if mode.starts_with(b"server\0") { server() } else { desktop() };
        match result {
            Ok(()) => {
                out(b"gala_cli_test: ok\r\n");
                0
            }
            Err(id) => id,
        }
    };
    // SAFETY: ends the process.
    unsafe { ExitProcess(code) }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdead) }
}

//! The topmost band and tool windows, which a user-mode taskbar and start
//! menu are made of (ROADMAP SH step 1, part 1). Expected results are
//! Windows' as Microsoft documents CreateWindowEx's WS_EX_TOPMOST and
//! WS_EX_TOOLWINDOW, SetWindowPos's HWND_TOPMOST and HWND_NOTOPMOST,
//! SetForegroundWindow and WindowFromPoint:
//!
//! - A topmost window stays above every non-topmost one, also when a
//!   non-topmost window is made the foreground window: WindowFromPoint
//!   where they overlap is the topmost one.
//! - SetWindowPos(HWND_TOPMOST) makes a window topmost (WS_EX_TOPMOST in
//!   its extended style) and puts it at the top of the band: above the
//!   other topmost window. SetWindowPos(HWND_NOTOPMOST) takes it out of
//!   the band (the style bit cleared), below the topmost windows.
//! - A tool window (WS_POPUP with WS_EX_TOOLWINDOW | WS_EX_TOPMOST, as a
//!   start menu is) can be the foreground window and gets the keys typed
//!   while it is: WM_KEYDOWN with the key's virtual-key code.
//!
//! Not on Windows' side of the test: whether the taskbar lists a window
//! (Windows has no API that says); gala's host tests pin that a tool
//! window is not listed.
//!
//! Each state waits for a key, so the scenario checks the screen while
//! it holds: `a` into the active normal window, `b` into the tool window
//! (after `topmost_test: type a` and `type b`). Prints `topmost_test: ok`, or
//! `topmost_test: check N got X (line L)` and exits with N.

#![no_std]
#![no_main]

use core::sync::atomic::{AtomicUsize, Ordering};

use ntrt as _;

const NAME: &str = "topmost_test";

type Handle = usize;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, overlapped: usize) -> i32;
    fn ExitProcess(code: u32) -> !;
    fn GetModuleHandleW(name: *const u16) -> Handle;
    fn Sleep(ms: u32);
    fn GetTickCount64() -> u64;
}

#[link(name = "user32", kind = "raw-dylib")]
unsafe extern "system" {
    fn RegisterClassW(class: *const WndClass) -> u16;
    #[allow(clippy::too_many_arguments)]
    fn CreateWindowExW(
        ex_style: u32,
        class: *const u16,
        title: *const u16,
        style: u32,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        parent: Handle,
        menu: Handle,
        instance: Handle,
        param: usize,
    ) -> Handle;
    fn DestroyWindow(hwnd: Handle) -> i32;
    fn DefWindowProcW(hwnd: Handle, msg: u32, wparam: usize, lparam: isize) -> isize;
    fn PeekMessageW(msg: *mut Msg, hwnd: Handle, min: u32, max: u32, remove: u32) -> i32;
    fn TranslateMessage(msg: *const Msg) -> i32;
    fn DispatchMessageW(msg: *const Msg) -> isize;
    fn ShowWindow(hwnd: Handle, cmd: i32) -> i32;
    #[allow(clippy::too_many_arguments)]
    fn SetWindowPos(hwnd: Handle, after: isize, x: i32, y: i32, w: i32, h: i32, flags: u32) -> i32;
    fn SetForegroundWindow(hwnd: Handle) -> i32;
    fn GetForegroundWindow() -> Handle;
    fn WindowFromPoint(point: Point) -> Handle;
    fn GetWindowLongW(hwnd: Handle, index: i32) -> i32;
}

#[link(name = "gdi32", kind = "raw-dylib")]
unsafe extern "system" {
    fn CreateSolidBrush(color: u32) -> Handle;
}

#[repr(C)]
struct WndClass {
    style: u32,
    proc_: extern "system" fn(Handle, u32, usize, isize) -> isize,
    cls_extra: i32,
    wnd_extra: i32,
    instance: Handle,
    icon: Handle,
    cursor: Handle,
    background: Handle,
    menu: *const u16,
    class: *const u16,
}

#[repr(C)]
struct Msg {
    hwnd: Handle,
    message: u32,
    wparam: usize,
    lparam: isize,
    time: u32,
    pt: [i32; 2],
    private: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Point {
    x: i32,
    y: i32,
}

const WS_POPUP: u32 = 0x8000_0000;
const WS_OVERLAPPEDWINDOW: u32 = 0x00cf_0000;
const WS_EX_TOPMOST: u32 = 0x8;
const WS_EX_TOOLWINDOW: u32 = 0x80;
const GWL_EXSTYLE: i32 = -20;
const HWND_TOPMOST: isize = -1;
const HWND_NOTOPMOST: isize = -2;
const SWP_NOSIZE: u32 = 0x1;
const SWP_NOMOVE: u32 = 0x2;
const SWP_NOACTIVATE: u32 = 0x10;
const SW_SHOW: i32 = 5;
const WM_KEYDOWN: u32 = 0x0100;
const VK_A: usize = 0x41;
const VK_B: usize = 0x42;

// The windows: the normal one red, the topmost one green, the tool window
// blue (COLORREF, 0x00BBGGRR), overlapping at OVERLAP.
const NORMAL: (i32, i32, i32, i32) = (100, 100, 400, 300);
const TOPMOST: (i32, i32, i32, i32) = (300, 200, 400, 300);
const TOOL: (i32, i32, i32, i32) = (760, 120, 240, 160);
const OVERLAP: Point = Point { x: 400, y: 300 };
const RED: u32 = 0x0000_00ff;
const GREEN: u32 = 0x0000_ff00;
const BLUE: u32 = 0x00ff_0000;

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
}

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

// ---- The windows ----------------------------------------------------------

/// The last key a window got: its window and virtual-key code.
static KEY_HWND: AtomicUsize = AtomicUsize::new(0);
static KEY: AtomicUsize = AtomicUsize::new(0);

extern "system" fn proc_(hwnd: Handle, msg: u32, wparam: usize, lparam: isize) -> isize {
    if msg == WM_KEYDOWN {
        KEY_HWND.store(hwnd, Ordering::Relaxed);
        KEY.store(wparam, Ordering::Relaxed);
        return 0;
    }
    // SAFETY: the default handling.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// Dispatches the waiting messages for up to `ms`, or until `done`.
fn pump(ms: u64, done: impl Fn() -> bool) -> bool {
    // SAFETY: our message structure.
    unsafe {
        let end = GetTickCount64() + ms;
        loop {
            let mut m: Msg = core::mem::zeroed();
            while PeekMessageW(&mut m, 0, 0, 0, 1) != 0 {
                TranslateMessage(&m);
                DispatchMessageW(&m);
            }
            if done() {
                return true;
            }
            if GetTickCount64() >= end {
                return false;
            }
            Sleep(10);
        }
    }
}

/// A class of windows filled with `color`.
fn class(name: &W, color: u32) -> bool {
    // SAFETY: a class of our procedure and a brush of its colour.
    unsafe {
        let c = WndClass {
            style: 0,
            proc_,
            cls_extra: 0,
            wnd_extra: 0,
            instance: GetModuleHandleW(core::ptr::null()),
            icon: 0,
            cursor: 0,
            background: CreateSolidBrush(color),
            menu: core::ptr::null(),
            class: name.ptr(),
        };
        RegisterClassW(&c) != 0
    }
}

fn window(ex: u32, style: u32, class: &W, title: &W, r: (i32, i32, i32, i32)) -> Handle {
    // SAFETY: a top-level window of our class.
    unsafe {
        let h = CreateWindowExW(
            ex,
            class.ptr(),
            title.ptr(),
            style,
            r.0,
            r.1,
            r.2,
            r.3,
            0,
            0,
            GetModuleHandleW(core::ptr::null()),
            0,
        );
        if h != 0 {
            ShowWindow(h, SW_SHOW);
        }
        h
    }
}

fn ex_style(hwnd: Handle) -> u32 {
    // SAFETY: a plain query.
    unsafe { GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 }
}

fn at_overlap() -> Handle {
    // SAFETY: a plain query.
    unsafe { WindowFromPoint(OVERLAP) }
}

/// A point the screen check looks at.
fn say_point(what: &[u8], x: i32, y: i32) {
    Line::new()
        .s(NAME.as_bytes())
        .s(b": ")
        .s(what)
        .s(b" at ")
        .dec(x.into())
        .s(b",")
        .dec(y.into())
        .s(b"\r\n")
        .print();
}

/// Waits for the scenario to type key `vk` (after `type <name>`): the
/// window that got it, 0 if none came.
fn typed(name: &[u8], vk: usize) -> Handle {
    KEY.store(0, Ordering::Relaxed);
    Line::new().s(NAME.as_bytes()).s(b": type ").s(name).s(b"\r\n").print();
    if pump(60_000, || KEY.load(Ordering::Relaxed) == vk) {
        KEY_HWND.load(Ordering::Relaxed)
    } else {
        0
    }
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    let (red, green, blue) = (W::new("TopmostRed"), W::new("TopmostGreen"), W::new("TopmostBlue"));
    c.ok(class(&red, RED) && class(&green, GREEN) && class(&blue, BLUE), 0)?;
    // SAFETY: plain calls on our windows.
    unsafe {
        let normal = window(0, WS_OVERLAPPEDWINDOW, &red, &W::new("topmost_test normal"), NORMAL);
        let top = window(
            WS_EX_TOPMOST,
            WS_OVERLAPPEDWINDOW,
            &green,
            &W::new("topmost_test topmost"),
            TOPMOST,
        );
        c.ok(normal != 0 && top != 0, 0)?;
        c.eq(i64::from(ex_style(top) & WS_EX_TOPMOST), i64::from(WS_EX_TOPMOST))?;
        c.eq(i64::from(ex_style(normal) & WS_EX_TOPMOST), 0)?;
        pump(300, || false);

        // The normal window made the foreground window stays below.
        c.ok(SetForegroundWindow(normal) != 0, 0)?;
        pump(300, || false);
        c.eq(GetForegroundWindow() as i64, normal as i64)?;
        c.eq(at_overlap() as i64, top as i64)?;
        say_point(b"the topmost window above the active one", OVERLAP.x, OVERLAP.y);
        // The keys go to the active window.
        c.eq(typed(b"a", VK_A) as i64, normal as i64)?;

        // HWND_TOPMOST: into the band, above the other topmost window.
        c.ok(SetWindowPos(normal, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE) != 0, 0)?;
        c.eq(i64::from(ex_style(normal) & WS_EX_TOPMOST), i64::from(WS_EX_TOPMOST))?;
        c.eq(at_overlap() as i64, normal as i64)?;
        // HWND_NOTOPMOST: out of it, below the topmost window again.
        let flags = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE;
        c.ok(SetWindowPos(normal, HWND_NOTOPMOST, 0, 0, 0, 0, flags) != 0, 0)?;
        c.eq(i64::from(ex_style(normal) & WS_EX_TOPMOST), 0)?;
        c.eq(at_overlap() as i64, top as i64)?;
        pump(300, || false);
        say_point(b"out of the band, under the topmost window", OVERLAP.x, OVERLAP.y);

        // A tool window is made the foreground window and gets the keys.
        let tool = window(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            WS_POPUP,
            &blue,
            &W::new("topmost_test tool"),
            TOOL,
        );
        c.ok(tool != 0, 0)?;
        c.ok(SetForegroundWindow(tool) != 0, 0)?;
        pump(300, || false);
        c.eq(GetForegroundWindow() as i64, tool as i64)?;
        say_point(b"the tool window", TOOL.0 + TOOL.2 / 2, TOOL.1 + TOOL.3 / 2);
        c.eq(typed(b"b", VK_B) as i64, tool as i64)?;
        say(b"the tool window got the key");
        DestroyWindow(tool);
        DestroyWindow(top);
        DestroyWindow(normal);
    }
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = match run() {
        Ok(()) => {
            say(b"ok");
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

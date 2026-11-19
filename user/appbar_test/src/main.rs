//! App bars: windows that reserve an edge of the screen
//! (SHAppBarMessage), as the docked agent panel does (ROADMAP "Change this
//! program"). Expected results are Windows' as Microsoft documents
//! SHAppBarMessage, ABM_NEW, ABM_QUERYPOS, ABM_SETPOS, ABM_REMOVE and
//! ABN_POSCHANGED ("Using Application Desktop Toolbars"):
//!
//! - ABM_NEW registers a window and returns TRUE; a second ABM_NEW for
//!   the same window returns FALSE (already registered).
//! - ABM_QUERYPOS on ABE_RIGHT with nothing else on that edge keeps the
//!   proposed right edge (the screen's) and returns TRUE; a second bar's
//!   proposal on the same edge is moved so it does not overlap the first
//!   (its right edge at or left of the first's left edge).
//! - ABM_SETPOS reserves the strip: SPI_GETWORKAREA's right edge becomes
//!   the bar's left edge, the other edges unchanged; a maximized window
//!   is fitted to the new work area (its right edge moves left by the
//!   bar's width).
//! - A bar on the same edge gets its callback message with
//!   ABN_POSCHANGED (wParam 1) when another bar there is set or removed.
//! - ABM_REMOVE returns TRUE and gives the strip back: the work area and
//!   the maximized window are as before.
//!
//! Assumed and not pinned: where Windows puts a right bar's top and
//! bottom against a bottom taskbar (QUERYPOS may cut them), so only the
//! horizontal edges are checked. ABM_GETTASKBARPOS is printed as an
//! `info` line before and after.
//!
//! Prints `appbar_test: ok`, or `appbar_test: check N got X (line L)` and
//! exits with N.

#![no_std]
#![no_main]

use core::sync::atomic::{AtomicUsize, Ordering};

use ntrt as _;

const NAME: &str = "appbar_test";

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
    fn DispatchMessageW(msg: *const Msg) -> isize;
    fn ShowWindow(hwnd: Handle, cmd: i32) -> i32;
    fn GetWindowRect(hwnd: Handle, rect: *mut Rect) -> i32;
    fn MoveWindow(hwnd: Handle, x: i32, y: i32, w: i32, h: i32, repaint: i32) -> i32;
    fn SystemParametersInfoW(action: u32, param: u32, out: *mut Rect, ini: u32) -> i32;
    fn GetSystemMetrics(index: i32) -> i32;
}

#[link(name = "shell32", kind = "raw-dylib")]
unsafe extern "system" {
    fn SHAppBarMessage(message: u32, data: *mut AppBarData) -> usize;
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
#[derive(Clone, Copy, Default, PartialEq)]
struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

/// APPBARDATA.
#[repr(C)]
struct AppBarData {
    size: u32,
    hwnd: Handle,
    callback: u32,
    edge: u32,
    rect: Rect,
    param: isize,
}

const ABM_NEW: u32 = 0;
const ABM_REMOVE: u32 = 1;
const ABM_QUERYPOS: u32 = 2;
const ABM_SETPOS: u32 = 3;
const ABM_GETTASKBARPOS: u32 = 5;
const ABE_RIGHT: u32 = 2;
const ABN_POSCHANGED: usize = 1;
const SPI_GETWORKAREA: u32 = 0x30;
const SM_CXSCREEN: i32 = 0;
const SM_CYSCREEN: i32 = 1;
const WS_POPUP: u32 = 0x8000_0000;
const WS_OVERLAPPEDWINDOW: u32 = 0x00cf_0000;
const WS_EX_TOOLWINDOW: u32 = 0x80;
const SW_MAXIMIZE: i32 = 3;
const CALLBACK_A: u32 = 0x8000 + 0x41; // WM_APP + 0x41
const CALLBACK_B: u32 = 0x8000 + 0x42;
const BAR_W: i32 = 300;
const SECOND_W: i32 = 100;

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

    fn rect(self, r: Rect) -> Self {
        self.s(b"[")
            .dec(r.left.into())
            .s(b",")
            .dec(r.top.into())
            .s(b",")
            .dec(r.right.into())
            .s(b",")
            .dec(r.bottom.into())
            .s(b"]")
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

fn say_rect(what: &[u8], r: Rect) {
    Line::new().s(NAME.as_bytes()).s(b": ").s(what).s(b" ").rect(r).s(b"\r\n").print();
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

/// The ABN_POSCHANGED notifications the first bar's window got.
static CHANGED_A: AtomicUsize = AtomicUsize::new(0);

extern "system" fn proc_(hwnd: Handle, msg: u32, wparam: usize, lparam: isize) -> isize {
    if msg == CALLBACK_A && wparam == ABN_POSCHANGED {
        CHANGED_A.fetch_add(1, Ordering::Relaxed);
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

fn work_area() -> Rect {
    let mut r = Rect::default();
    // SAFETY: our RECT.
    unsafe { SystemParametersInfoW(SPI_GETWORKAREA, 0, &mut r, 0) };
    r
}

fn window_rect(hwnd: Handle) -> Rect {
    let mut r = Rect::default();
    // SAFETY: our RECT.
    unsafe { GetWindowRect(hwnd, &mut r) };
    r
}

fn data(hwnd: Handle, callback: u32) -> AppBarData {
    AppBarData {
        size: core::mem::size_of::<AppBarData>() as u32,
        hwnd,
        callback,
        edge: 0,
        rect: Rect::default(),
        param: 0,
    }
}

fn taskbar_info(when: &[u8]) {
    let mut d = data(0, 0);
    // SAFETY: our APPBARDATA.
    let ok = unsafe { SHAppBarMessage(ABM_GETTASKBARPOS, &mut d) };
    if ok != 0 {
        say_rect(when, d.rect);
    }
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    // SAFETY: plain calls with our structures; the windows made here are
    // destroyed and the bars removed.
    unsafe {
        let (sw, sh) = (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN));
        let work0 = work_area();
        say_rect(b"work area", work0);
        taskbar_info(b"info taskbar");
        let class = W::new("appbar_test window");
        let wc = WndClass {
            style: 0,
            proc_,
            cls_extra: 0,
            wnd_extra: 0,
            instance: GetModuleHandleW(core::ptr::null()),
            icon: 0,
            cursor: 0,
            background: 0,
            menu: core::ptr::null(),
            class: class.ptr(),
        };
        c.ok(RegisterClassW(&wc) != 0, 0)?;
        let title = W::new("appbar_test maximized");
        let big = CreateWindowExW(
            0,
            class.ptr(),
            title.ptr(),
            WS_OVERLAPPEDWINDOW,
            100,
            100,
            400,
            300,
            0,
            0,
            wc.instance,
            0,
        );
        c.ok(big != 0, 0)?;
        ShowWindow(big, SW_MAXIMIZE);
        pump(200, || false);
        let max0 = window_rect(big);
        say_rect(b"maximized", max0);

        let bar_title = W::new("appbar_test bar");
        let bar = |title: &W| {
            CreateWindowExW(
                WS_EX_TOOLWINDOW,
                class.ptr(),
                title.ptr(),
                WS_POPUP,
                0,
                0,
                10,
                10,
                0,
                0,
                wc.instance,
                0,
            )
        };
        let a = bar(&bar_title);
        c.ok(a != 0, 0)?;

        // Registration: once.
        let mut d = data(a, CALLBACK_A);
        c.eq(SHAppBarMessage(ABM_NEW, &mut d) as i64, 1)?;
        c.eq(SHAppBarMessage(ABM_NEW, &mut d) as i64, 0)?;

        // The first bar on the right edge keeps its right edge.
        d.edge = ABE_RIGHT;
        d.rect = Rect { left: sw - BAR_W, top: 0, right: sw, bottom: sh };
        c.eq(SHAppBarMessage(ABM_QUERYPOS, &mut d) as i64, 1)?;
        c.eq(d.rect.right.into(), sw.into())?;
        d.rect.left = d.rect.right - BAR_W;
        c.eq(SHAppBarMessage(ABM_SETPOS, &mut d) as i64, 1)?;
        let a_rect = d.rect;
        say_rect(b"bar", a_rect);
        MoveWindow(
            a,
            a_rect.left,
            a_rect.top,
            a_rect.right - a_rect.left,
            a_rect.bottom - a_rect.top,
            1,
        );

        // The strip is reserved.
        let work1 = work_area();
        say_rect(b"work area with the bar", work1);
        c.eq(work1.right.into(), a_rect.left.into())?;
        c.eq(work1.left.into(), work0.left.into())?;
        c.eq(work1.top.into(), work0.top.into())?;
        c.eq(work1.bottom.into(), work0.bottom.into())?;
        // The maximized window is fitted to it.
        let fitted = pump(2000, || window_rect(big).right == max0.right - BAR_W);
        let max1 = window_rect(big);
        say_rect(b"maximized with the bar", max1);
        c.ok(fitted, max1.right.into())?;
        c.eq(max1.left.into(), max0.left.into())?;
        taskbar_info(b"info taskbar with the bar");

        // A second bar on the same edge goes beside the first, and the
        // first is told.
        let second_title = W::new("appbar_test second bar");
        let b = bar(&second_title);
        c.ok(b != 0, 0)?;
        let mut e = data(b, CALLBACK_B);
        c.eq(SHAppBarMessage(ABM_NEW, &mut e) as i64, 1)?;
        e.edge = ABE_RIGHT;
        e.rect = Rect { left: sw - SECOND_W, top: 0, right: sw, bottom: sh };
        c.eq(SHAppBarMessage(ABM_QUERYPOS, &mut e) as i64, 1)?;
        c.ok(e.rect.right <= a_rect.left, e.rect.right.into())?;
        e.rect.left = e.rect.right - SECOND_W;
        let before = CHANGED_A.load(Ordering::Relaxed);
        c.eq(SHAppBarMessage(ABM_SETPOS, &mut e) as i64, 1)?;
        c.eq(work_area().right.into(), e.rect.left.into())?;
        let told = pump(2000, || CHANGED_A.load(Ordering::Relaxed) > before);
        c.ok(told, CHANGED_A.load(Ordering::Relaxed) as i64)?;
        say(b"the first bar was told");

        // Removing the second gives its strip back.
        let before = CHANGED_A.load(Ordering::Relaxed);
        c.eq(SHAppBarMessage(ABM_REMOVE, &mut e) as i64, 1)?;
        c.eq(work_area().right.into(), a_rect.left.into())?;
        let told = pump(2000, || CHANGED_A.load(Ordering::Relaxed) > before);
        c.ok(told, CHANGED_A.load(Ordering::Relaxed) as i64)?;
        DestroyWindow(b);

        // Removing the first gives the whole strip back.
        c.eq(SHAppBarMessage(ABM_REMOVE, &mut d) as i64, 1)?;
        let work2 = work_area();
        c.ok(work2 == work0, work2.right.into())?;
        let back = pump(2000, || window_rect(big) == max0);
        c.ok(back, window_rect(big).right.into())?;
        say(b"the strip is given back");
        DestroyWindow(a);
        DestroyWindow(big);
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

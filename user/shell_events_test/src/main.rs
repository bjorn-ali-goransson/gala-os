//! What a taskbar listens to when the desktop itself changes a window
//! (ROADMAP SH step 1, part 2): the shell's actions (GalaDesktopAction's
//! Activate, Minimize and Restore, which a taskbar button and gala's own
//! taskbar do) reach the window's program as Windows' do. Expected
//! results are Windows' as Microsoft documents SetWinEventHook's events
//! and WM_ACTIVATE:
//!
//! - Another window made active: EVENT_SYSTEM_FOREGROUND for it
//!   (OBJID_WINDOW, CHILDID_SELF), the old one gets WM_ACTIVATE
//!   WA_INACTIVE and the new one WM_ACTIVATE WA_ACTIVE;
//!   GetForegroundWindow says so.
//! - Minimized: EVENT_SYSTEM_MINIMIZESTART for it, IsIconic, and the
//!   next window becomes the foreground one (EVENT_SYSTEM_FOREGROUND).
//! - Restored: EVENT_SYSTEM_MINIMIZEEND and EVENT_SYSTEM_FOREGROUND.
//!
//! Prints `shell_events_test: ok`, or `shell_events_test: check N got X
//! (line L)` and exits with N.

#![no_std]
#![no_main]

use core::sync::atomic::{AtomicUsize, Ordering};

use ntrt as _;

const NAME: &str = "shell_events_test";

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
    fn GetForegroundWindow() -> Handle;
    fn IsIconic(hwnd: Handle) -> i32;
    #[allow(clippy::too_many_arguments)]
    fn SetWinEventHook(
        min: u32,
        max: u32,
        module: Handle,
        proc_: extern "system" fn(Handle, u32, Handle, i32, i32, u32, u32),
        pid: u32,
        tid: u32,
        flags: u32,
    ) -> Handle;
    fn UnhookWinEvent(hook: Handle) -> i32;
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

const WS_OVERLAPPEDWINDOW: u32 = 0x00cf_0000;
const SW_SHOW: i32 = 5;
const WM_ACTIVATE: u32 = 0x0006;
const WA_INACTIVE: usize = 0;
const WA_ACTIVE: usize = 1;
const EVENT_SYSTEM_FOREGROUND: u32 = 0x0003;
const EVENT_SYSTEM_MINIMIZESTART: u32 = 0x0016;
const EVENT_SYSTEM_MINIMIZEEND: u32 = 0x0017;
const OBJID_WINDOW: i32 = 0;
const CHILDID_SELF: i32 = 0;
const WINEVENT_OUTOFCONTEXT: u32 = 0;

// GalaDesktopAction (kernel/src/nt/action.rs): version 1, the action,
// the window's HWND, four numbers, the text's length, zero.
const ACTION_ACTIVATE: u32 = 3;
const ACTION_MINIMIZE: u32 = 4;
const REQUEST_HEADER: usize = 40;

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

// ---- The windows and the events -----------------------------------------

/// The events the hook got, in order: (event, hwnd); the object and child
/// of the last one that was not (OBJID_WINDOW, CHILDID_SELF).
static EVENTS: [AtomicUsize; 64] = [const { AtomicUsize::new(0) }; 64];
static EVENT_HWNDS: [AtomicUsize; 64] = [const { AtomicUsize::new(0) }; 64];
static N_EVENTS: AtomicUsize = AtomicUsize::new(0);
static ODD_OBJECT: AtomicUsize = AtomicUsize::new(0);
/// The last WM_ACTIVATE's wParam (low word) for the two windows, +1 (0:
/// none since cleared).
static ACT_A: AtomicUsize = AtomicUsize::new(0);
static ACT_B: AtomicUsize = AtomicUsize::new(0);
static HWND_A: AtomicUsize = AtomicUsize::new(0);

extern "system" fn hook(
    _: Handle,
    event: u32,
    hwnd: Handle,
    object: i32,
    child: i32,
    _: u32,
    _: u32,
) {
    if object != OBJID_WINDOW || child != CHILDID_SELF {
        ODD_OBJECT.store(event as usize, Ordering::Relaxed);
        return;
    }
    let n = N_EVENTS.load(Ordering::Relaxed);
    if n < EVENTS.len() {
        EVENTS[n].store(event as usize, Ordering::Relaxed);
        EVENT_HWNDS[n].store(hwnd, Ordering::Relaxed);
        N_EVENTS.store(n + 1, Ordering::Relaxed);
    }
}

extern "system" fn proc_(hwnd: Handle, msg: u32, wparam: usize, lparam: isize) -> isize {
    if msg == WM_ACTIVATE {
        let slot = if hwnd == HWND_A.load(Ordering::Relaxed) { &ACT_A } else { &ACT_B };
        slot.store((wparam & 0xffff) + 1, Ordering::Relaxed);
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

/// Whether the hook got `event` for `hwnd` since the events were cleared.
fn got(event: u32, hwnd: Handle) -> bool {
    (0..N_EVENTS.load(Ordering::Relaxed)).any(|i| {
        EVENTS[i].load(Ordering::Relaxed) == event as usize
            && EVENT_HWNDS[i].load(Ordering::Relaxed) == hwnd
    })
}

fn clear() {
    N_EVENTS.store(0, Ordering::Relaxed);
    ACT_A.store(0, Ordering::Relaxed);
    ACT_B.store(0, Ordering::Relaxed);
}

/// The desktop's action on one of our windows (as a taskbar asks it):
/// its status.
fn action(what: u32, hwnd: Handle) -> u32 {
    let mut b = [0u8; REQUEST_HEADER];
    b[0..4].copy_from_slice(&1u32.to_le_bytes());
    b[4..8].copy_from_slice(&what.to_le_bytes());
    b[8..16].copy_from_slice(&(hwnd as u64).to_le_bytes());
    let mut returned = 0u32;
    // SAFETY: our buffers, with their lengths.
    unsafe {
        GalaDesktopAction(b.as_ptr(), b.len() as u32, core::ptr::null_mut(), 0, &mut returned)
    }
}

fn window(class: &W, title: &W, x: i32) -> Handle {
    // SAFETY: a top-level window of our class.
    unsafe {
        let h = CreateWindowExW(
            0,
            class.ptr(),
            title.ptr(),
            WS_OVERLAPPEDWINDOW,
            x,
            100,
            400,
            300,
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

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    let class = W::new("ShellEvents");
    // SAFETY: plain calls on our class, windows and hook.
    unsafe {
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
        let h = SetWinEventHook(
            EVENT_SYSTEM_FOREGROUND,
            EVENT_SYSTEM_MINIMIZEEND,
            0,
            hook,
            0,
            0,
            WINEVENT_OUTOFCONTEXT,
        );
        c.ok(h != 0, 0)?;
        let a = window(&class, &W::new("shell_events_test A"), 100);
        HWND_A.store(a, Ordering::Relaxed);
        let b = window(&class, &W::new("shell_events_test B"), 600);
        c.ok(a != 0 && b != 0, 0)?;
        pump(300, || false);
        c.eq(GetForegroundWindow() as i64, b as i64)?;

        // Activate A as its taskbar button does.
        clear();
        c.eq(i64::from(action(ACTION_ACTIVATE, a)), 0)?;
        let done = || got(EVENT_SYSTEM_FOREGROUND, a) && ACT_A.load(Ordering::Relaxed) != 0;
        c.ok(pump(5000, done), N_EVENTS.load(Ordering::Relaxed) as i64)?;
        c.eq(GetForegroundWindow() as i64, a as i64)?;
        c.eq(ACT_A.load(Ordering::Relaxed) as i64 - 1, WA_ACTIVE as i64)?;
        c.eq(ACT_B.load(Ordering::Relaxed) as i64 - 1, WA_INACTIVE as i64)?;
        say(b"activated by the desktop");

        // Minimize A: B is the foreground window again.
        clear();
        c.eq(i64::from(action(ACTION_MINIMIZE, a)), 0)?;
        let done = || got(EVENT_SYSTEM_MINIMIZESTART, a) && got(EVENT_SYSTEM_FOREGROUND, b);
        c.ok(pump(5000, done), N_EVENTS.load(Ordering::Relaxed) as i64)?;
        c.eq(i64::from(IsIconic(a)), 1)?;
        c.eq(GetForegroundWindow() as i64, b as i64)?;
        say(b"minimized by the desktop");

        // Restore A (its taskbar button again).
        clear();
        c.eq(i64::from(action(ACTION_ACTIVATE, a)), 0)?;
        let done = || got(EVENT_SYSTEM_MINIMIZEEND, a) && got(EVENT_SYSTEM_FOREGROUND, a);
        c.ok(pump(5000, done), N_EVENTS.load(Ordering::Relaxed) as i64)?;
        c.eq(i64::from(IsIconic(a)), 0)?;
        c.eq(GetForegroundWindow() as i64, a as i64)?;
        say(b"restored by the desktop");
        c.eq(ODD_OBJECT.load(Ordering::Relaxed) as i64, 0)?;

        UnhookWinEvent(h);
        DestroyWindow(b);
        DestroyWindow(a);
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

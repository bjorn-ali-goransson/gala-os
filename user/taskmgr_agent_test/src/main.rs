//! "Ask the agent about this process" (ROADMAP TM step 6; ARCHITECTURE
//! "The task manager", "The agent panel"): Task Manager's right-click
//! menu on a process hands the process to the docked agent panel. This
//! test is the process asked about (it has no window, so it is one of
//! the background processes) and checks the panel through UI
//! Automation while the screen scenario (`taskmgr_agent`) clicks:
//!
//! - the panel (agent.exe, started with the session) is docked; its Hide
//!   button is announced and clicked: the panel is the dock tab;
//! - the scenario opens Task Manager (Ctrl+Shift+Esc) and searches
//!   `agent_`, which leaves this test's row alone under its heading
//!   (row 1); the row is announced and right-clicked, then the menu's
//!   one item ("Ask the agent about this process") is announced and
//!   clicked;
//! - the panel docks again by itself (the hand-off shows it), its
//!   context chip names this process ("ContextProgram" named
//!   `taskmgr_agent_test.exe`, "ContextTitle" `pid N, Windows`) and its
//!   first card says what it is about ("Request0Text" named `About:
//!   taskmgr_agent_test.exe (pid N)`), with nothing else listed;
//! - Task Manager is still the active window (the panel never takes
//!   it), so the scenario's Alt+F4 closes it; the card stays.
//!
//! Task Manager serves no UI Automation yet (ARCHITECTURE "The task
//! manager", Limits), so its row and its menu item are announced at the
//! places its layout gives them, pinned here: a row is `ROW_H` (24)
//! high from client y `ROWS_Y` (128), the menu opens at the click with
//! its item `MENU_ROW` (30) high below a 4-pixel edge, `MENU_W` (260)
//! wide. The panel's elements are found as an accessibility tool finds
//! them.
//!
//! Prints `taskmgr_agent_test: docked`, `hidden`, `task manager open`,
//! `menu`, `listed` and `ok`, or `taskmgr_agent_test: check N got X
//! (line L)` and exits with N.

#![no_std]
#![no_main]

use ntrt as _;
use uia_client::{Element, TEXT, Uia};

const NAME: &str = "taskmgr_agent_test";

type Handle = usize;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, overlapped: usize) -> i32;
    fn ExitProcess(code: u32) -> !;
    fn Sleep(ms: u32);
    fn GetTickCount64() -> u64;
    fn GetCurrentProcessId() -> u32;
}

#[link(name = "user32", kind = "raw-dylib")]
unsafe extern "system" {
    fn PeekMessageW(msg: *mut Msg, hwnd: Handle, min: u32, max: u32, remove: u32) -> i32;
    fn DispatchMessageW(msg: *const Msg) -> isize;
    fn FindWindowW(class: *const u16, title: *const u16) -> Handle;
    fn GetWindowRect(hwnd: Handle, rect: *mut Rect) -> i32;
    fn GetSystemMetrics(index: i32) -> i32;
    fn SystemParametersInfoW(action: u32, param: u32, out: *mut Rect, ini: u32) -> i32;
    fn GetWindowLongW(hwnd: Handle, index: i32) -> i32;
    fn AdjustWindowRectEx(rect: *mut Rect, style: u32, menu: i32, ex_style: u32) -> i32;
    fn GetForegroundWindow() -> Handle;
    fn IsWindow(hwnd: Handle) -> i32;
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

const SM_CXSCREEN: i32 = 0;
const SM_CYSCREEN: i32 = 1;
const SPI_GETWORKAREA: u32 = 0x30;
const GWL_STYLE: i32 = -16;
const GWL_EXSTYLE: i32 = -20;
/// The panel's docked width and its dock tab (agent's WIDTH and TAB).
const WIDTH: i32 = 320;
const TAB: (i32, i32) = (56, 40);
/// Task Manager's row 1 (the first under the heading), in its client
/// area: x anywhere in the row, y the row's middle (ROWS_Y + ROW_H * 3/2).
const ROW_1: (i32, i32) = (400, 128 + 24 + 12);
/// The menu's item from the click: the middle of the 260-wide row, 30
/// high, below the menu's 4-pixel edge.
const ITEM: (i32, i32) = (130, 4 + 15);
/// How long a step may take: the scenario's input comes once this test
/// has printed what the step waits for.
const STEP_MS: u64 = 60_000;

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

    fn bytes(&self) -> &[u8] {
        &self.buf[..self.n]
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

/// `<NAME>: <id> at X,Y (pointer at PX,PY)`, what the scenario's
/// `click_on` and `right_click_on` read, for a place this test computed.
fn announce_at(id: &[u8], (x, y): (i32, i32)) {
    let (px, py) = uia_client::pointer();
    Line::new()
        .s(NAME.as_bytes())
        .s(b": ")
        .s(id)
        .s(b" at ")
        .dec(x.into())
        .s(b",")
        .dec(y.into())
        .s(b" (pointer at ")
        .dec(px.into())
        .s(b",")
        .dec(py.into())
        .s(b")\r\n")
        .print();
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

fn window_rect(hwnd: Handle) -> Rect {
    let mut r = Rect::default();
    // SAFETY: our RECT.
    unsafe { GetWindowRect(hwnd, &mut r) };
    r
}

fn work_area() -> Rect {
    let mut r = Rect::default();
    // SAFETY: our RECT.
    unsafe { SystemParametersInfoW(SPI_GETWORKAREA, 0, &mut r, 0) };
    r
}

/// Whether `e` is named exactly `want`.
fn named(e: &Element, want: &[u8]) -> bool {
    e.name().as_str().as_bytes() == want
}

// ---- The checks -----------------------------------------------------------

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    // SAFETY: plain calls with our structures.
    unsafe {
        let (sw, sh) = (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN));
        let tab =
            Rect { left: sw - TAB.0, top: (sh - TAB.1) / 2, right: sw, bottom: (sh + TAB.1) / 2 };
        let pid = GetCurrentProcessId();

        // The panel, started with the session, docked.
        let (class, title) = (W::new("GalaUiWindow"), W::new("Agent"));
        let docked = || work_area().right == sw - WIDTH;
        let found = pump(STEP_MS, || FindWindowW(class.ptr(), title.ptr()) != 0 && docked());
        c.ok(found, work_area().right.into())?;
        let panel = FindWindowW(class.ptr(), title.ptr());
        let uia = Uia::new();
        c.ok(uia.is_ok(), uia.as_ref().err().copied().unwrap_or(0).into())?;
        let uia = uia.unwrap_or_else(|_| unreachable!());
        let find = |id: &str| -> Option<Element> { uia.find(&uia.window(panel)?, id) };
        let hide = find("HideButton");
        c.ok(hide.as_ref().is_some_and(|e| e.name() == "Hide"), 0)?;
        uia_client::announce(NAME, "HideButton", &hide.unwrap_or_else(|| unreachable!()));
        say(b"docked");

        // Hidden (the scenario's click): the dock tab.
        let hidden = pump(STEP_MS, || work_area().right == sw && window_rect(panel) == tab);
        c.ok(hidden, work_area().right.into())?;
        say(b"hidden");

        // Task Manager (the scenario's Ctrl+Shift+Esc): this test's row,
        // once the scenario's search leaves it alone, is row 1.
        let tm_class = W::new("TaskManagerWindow");
        let open = pump(STEP_MS, || FindWindowW(tm_class.ptr(), core::ptr::null()) != 0);
        c.ok(open, 0)?;
        let tm = FindWindowW(tm_class.ptr(), core::ptr::null());
        // Its client area's origin on the screen: the window's rectangle
        // less the frame AdjustWindowRectEx gives its styles (it has no
        // menu bar). Not ClientToScreen: gala's answers for the calling
        // process's own windows only (the window registry has no client
        // rectangle), where Windows' answers for any window.
        let frame = window_rect(tm);
        let mut inset = Rect::default();
        let (style, ex) = (GetWindowLongW(tm, GWL_STYLE), GetWindowLongW(tm, GWL_EXSTYLE));
        c.ok(AdjustWindowRectEx(&mut inset, style as u32, 0, ex as u32) != 0, 0)?;
        c.ok(frame.right > frame.left, frame.right.into())?;
        let row = (frame.left - inset.left + ROW_1.0, frame.top - inset.top + ROW_1.1);
        announce_at(b"ProcessRow", row);
        say(b"task manager open");

        // The right click on the row (the scenario's): the menu opens
        // where it was, its item announced from there.
        let at_row = pump(STEP_MS, || uia_client::pointer() == row);
        c.ok(at_row, uia_client::pointer().0.into())?;
        announce_at(b"AskTheAgent", (row.0 + ITEM.0, row.1 + ITEM.1));
        say(b"menu");

        // The item clicked: the panel docks by itself and is about this
        // process.
        let about = Line::new().s(b"About: taskmgr_agent_test.exe (pid ").dec(pid.into()).s(b")");
        let pid_line = Line::new().s(b"pid ").dec(pid.into()).s(b", Windows");
        let listed = pump(STEP_MS, || {
            docked() && find("Request0Text").is_some_and(|e| named(&e, about.bytes()))
        });
        c.ok(listed, work_area().right.into())?;
        let program = find("ContextProgram");
        c.ok(program.as_ref().is_some_and(|e| e.name() == "taskmgr_agent_test.exe"), 0)?;
        let title = find("ContextTitle");
        c.ok(title.as_ref().is_some_and(|e| named(e, pid_line.bytes())), 0)?;
        c.ok(title.is_some_and(|e| e.control_type() == TEXT), 0)?;
        c.ok(find("Request1Text").is_none(), 0)?;
        // A process handed over is listed, not asked: no reply card.
        c.ok(find("Reply0").is_none(), 0)?;
        // Task Manager is still the active window: the scenario's Alt+F4
        // closes it, not the panel.
        let active = pump(STEP_MS, || GetForegroundWindow() == tm);
        c.ok(active, GetForegroundWindow() as i64)?;
        say(b"listed");

        let closed = pump(STEP_MS, || IsWindow(tm) == 0);
        c.ok(closed, 0)?;
        c.ok(find("Request0Text").is_some_and(|e| named(&e, about.bytes())), 0)?;
    }
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    match run() {
        Ok(()) => {
            say(b"ok");
            // SAFETY: plain call.
            unsafe { ExitProcess(0) }
        }
        // SAFETY: plain call.
        Err(n) => unsafe { ExitProcess(n) },
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xc000_0409) }
}

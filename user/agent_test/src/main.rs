//! The agent panel (agent.exe; ROADMAP LY "Change this program", step
//! 4; ARCHITECTURE "The agent panel"), as a program sees it. The desktop
//! starts agent.exe with the session; this test finds its window and
//! checks it against Windows' documented app bar behaviour
//! (SHAppBarMessage, SPI_GETWORKAREA: "Using Application Desktop
//! Toolbars"), with the screen scenario clicking Hide and the dock tab:
//!
//! - docked: the panel's window ("Agent") is the strip at the screen's
//!   right edge, `WIDTH` (320) wide, from the top of the screen down to
//!   the taskbar (never over it); the work area ends at its left edge;
//!   ABM_GETTASKBARPOS is the whole width before and after (gala's
//!   taskbar does not give way to a side bar);
//! - this test's own window, made active, is the panel's context: the
//!   panel's UI Automation element "ContextProgram" is named
//!   `agent_test` and "ContextTitle" `Agent test window`;
//! - the Hide button, found through UI Automation (AutomationId
//!   "HideButton", a Button named "Hide", inside the panel's top right),
//!   is announced for the scenario's `click_on` (uia_client: the panel's
//!   elements are found as an accessibility tool finds them, not at a
//!   position written into the scenario);
//! - hidden (the scenario clicks Hide): the work area is the whole width
//!   again, the panel's window is the dock tab (56 x 40 at the middle of
//!   the right edge), its "DockTab" element a Button named "Agent" that
//!   fills it, and HKCU\Software\gala\Agent's `Hidden` is 1;
//! - docked again (the scenario clicks the tab): as at first, `Hidden` 0;
//! - a hand-off (gala's own: WM_COPYDATA of kind "gal1" and the wire form
//!   `agent::handoff` defines, written out here): another kind or a
//!   malformed one is not taken (0), a request on an element is (1) and
//!   is listed: "Request0Text" named with its text, "Request0Element"
//!   `On: <the element's label>`;
//! - then this test's own window is announced and the test waits for the
//!   scenario's click on it, so the run ends after the last screen step.
//!
//! Assumed and not pinned: that agent.exe is gala's own, so its sizes
//! are gala's choice (the same on Windows were it run there).
//!
//! Prints `agent_test: docked`, `agent_test: hidden`, `agent_test:
//! docked again`, `agent_test: handed off` and `agent_test: ok`, or `agent_test: check N got X
//! (line L)` and exits with N.

#![no_std]
#![no_main]

use core::sync::atomic::{AtomicBool, Ordering};
use ntrt as _;
use uia_client::{BUTTON, Element, TEXT, Uia};

const NAME: &str = "agent_test";

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
    fn SetForegroundWindow(hwnd: Handle) -> i32;
    fn FindWindowW(class: *const u16, title: *const u16) -> Handle;
    fn GetWindowRect(hwnd: Handle, rect: *mut Rect) -> i32;
    fn SystemParametersInfoW(action: u32, param: u32, out: *mut Rect, ini: u32) -> i32;
    fn GetSystemMetrics(index: i32) -> i32;
    fn SendMessageW(hwnd: Handle, msg: u32, wparam: usize, lparam: isize) -> isize;
}

/// COPYDATASTRUCT.
#[repr(C)]
struct CopyData {
    kind: usize,
    size: u32,
    data: *const u8,
}

/// The panel's hand-off (agent::handoff, written out here so the wire
/// form is pinned by the test): its dwData and a request on an element.
const HANDOFF: usize = 0x6761_6c31;
const REQUEST: &[u8] = b"gala-handoff 1\0agent_test\0Agent test window\x0020010\0\
make the header bigger\0Header\0the header\x0010,20,300,40\0";

/// WM_COPYDATA of `kind` with `data` to `hwnd`: its result.
fn copy_data(hwnd: Handle, kind: usize, data: &[u8]) -> isize {
    let cds = CopyData { kind, size: data.len() as u32, data: data.as_ptr() };
    // SAFETY: the structure and its bytes live across the call.
    unsafe { SendMessageW(hwnd, 0x004a, 0, &cds as *const _ as isize) }
}

#[link(name = "shell32", kind = "raw-dylib")]
unsafe extern "system" {
    fn SHAppBarMessage(message: u32, data: *mut AppBarData) -> usize;
}

#[link(name = "advapi32", kind = "raw-dylib")]
unsafe extern "system" {
    fn RegGetValueW(
        key: isize,
        subkey: *const u16,
        value: *const u16,
        flags: u32,
        kind: *mut u32,
        data: *mut u32,
        size: *mut u32,
    ) -> u32;
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

const ABM_GETTASKBARPOS: u32 = 5;
const SPI_GETWORKAREA: u32 = 0x30;
const SM_CXSCREEN: i32 = 0;
const SM_CYSCREEN: i32 = 1;
const WS_OVERLAPPEDWINDOW: u32 = 0x00cf_0000;
const SW_SHOW: i32 = 5;
const HKEY_CURRENT_USER: isize = 0x8000_0001u32 as i32 as isize;
const RRF_RT_REG_DWORD: u32 = 0x10;
/// The panel's docked width and its dock tab (agent's WIDTH and TAB).
const WIDTH: i32 = 320;
const TAB: (i32, i32) = (56, 40);
/// How long a step may take: the scenario's clicks come once this test
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

// ---- The checks -----------------------------------------------------------

/// Whether this test's window was clicked (the scenario's last step: the
/// run ends only after its last input, WORKFLOW "Screen checks").
static CLICKED: AtomicBool = AtomicBool::new(false);
const WM_LBUTTONUP: u32 = 0x0202;

extern "system" fn proc_(hwnd: Handle, msg: u32, wparam: usize, lparam: isize) -> isize {
    if msg == WM_LBUTTONUP {
        CLICKED.store(true, Ordering::Relaxed);
    }
    // SAFETY: the default handling.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// The agent's remembered `Hidden`, or -1 when it is not there.
fn hidden_value() -> i64 {
    let (key, name) = (W::new("Software\\gala\\Agent"), W::new("Hidden"));
    let (mut v, mut size) = (0u32, 4u32);
    // SAFETY: our strings and DWORD.
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.ptr(),
            name.ptr(),
            RRF_RT_REG_DWORD,
            core::ptr::null_mut(),
            &mut v,
            &mut size,
        )
    };
    if r == 0 { v.into() } else { -1 }
}

fn taskbar() -> Rect {
    let mut d = AppBarData {
        size: core::mem::size_of::<AppBarData>() as u32,
        hwnd: 0,
        callback: 0,
        edge: 0,
        rect: Rect::default(),
        param: 0,
    };
    // SAFETY: our APPBARDATA.
    unsafe { SHAppBarMessage(ABM_GETTASKBARPOS, &mut d) };
    d.rect
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    // SAFETY: plain calls with our structures.
    unsafe {
        let (sw, sh) = (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN));
        // The taskbar: the whole width at the bottom, the panel or not.
        let bar = taskbar();
        c.eq(bar.left.into(), 0)?;
        c.eq(bar.right.into(), sw.into())?;
        c.eq(bar.bottom.into(), sh.into())?;
        c.ok(bar.top > 0 && bar.top < sh, bar.top.into())?;
        // Docked: the right edge, from the top down to the taskbar.
        let docked = Rect { left: sw - WIDTH, top: 0, right: sw, bottom: bar.top };
        let tab =
            Rect { left: sw - TAB.0, top: (sh - TAB.1) / 2, right: sw, bottom: (sh + TAB.1) / 2 };

        // The panel, started with the session, docked.
        let (class, title) = (W::new("GalaUiWindow"), W::new("Agent"));
        let found = pump(STEP_MS, || {
            let p = FindWindowW(class.ptr(), title.ptr());
            p != 0 && window_rect(p) == docked && work_area().right == sw - WIDTH
        });
        let panel = FindWindowW(class.ptr(), title.ptr());
        c.ok(panel != 0, 0)?;
        say_rect(b"the panel", window_rect(panel));
        c.ok(found, work_area().right.into())?;
        say_rect(b"the taskbar", bar);
        let work = work_area();
        c.eq(work.left.into(), 0)?;
        c.eq(work.top.into(), 0)?;
        c.eq(work.bottom.into(), bar.top.into())?;
        // The taskbar does not give way to the panel.
        c.ok(taskbar() == bar, taskbar().right.into())?;
        c.ok(hidden_value() != 1, hidden_value())?;

        // This test's window, active: the panel's context.
        let cls = W::new("AgentTestWindow");
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
            class: cls.ptr(),
        };
        c.ok(RegisterClassW(&wc) != 0, 0)?;
        let own_title = W::new("Agent test window");
        let own = CreateWindowExW(
            0,
            cls.ptr(),
            own_title.ptr(),
            WS_OVERLAPPEDWINDOW,
            120,
            120,
            480,
            300,
            0,
            0,
            wc.instance,
            0,
        );
        c.ok(own != 0, 0)?;
        ShowWindow(own, SW_SHOW);
        SetForegroundWindow(own);

        // The panel as UI Automation sees it.
        let uia = Uia::new();
        c.ok(uia.is_ok(), uia.as_ref().err().copied().unwrap_or(0).into())?;
        let uia = uia.unwrap_or_else(|_| unreachable!());
        let find = |id: &str| -> Option<Element> { uia.find(&uia.window(panel)?, id) };
        // The context follows the active window (the panel's WinEvent).
        let named =
            pump(STEP_MS, || find("ContextProgram").is_some_and(|e| e.name() == "agent_test"));
        c.ok(named, 0)?;
        let title = find("ContextTitle");
        c.ok(title.as_ref().is_some_and(|e| e.name() == "Agent test window"), 0)?;
        c.eq(title.map_or(0, |e| e.control_type()).into(), TEXT.into())?;
        let hide = find("HideButton");
        c.ok(hide.is_some(), 0)?;
        let hide = hide.unwrap_or_else(|| unreachable!());
        c.ok(hide.name() == "Hide", 0)?;
        c.eq(hide.control_type().into(), BUTTON.into())?;
        let r = hide.rect();
        say_rect(
            b"HideButton",
            Rect { left: r.left, top: r.top, right: r.right, bottom: r.bottom },
        );
        c.ok(
            r.left >= docked.left && r.right <= sw && r.top >= 0 && r.bottom <= 64,
            r.left.into(),
        )?;
        uia_client::announce(NAME, "HideButton", &hide);
        say(b"docked");

        // Hide (the scenario's click): the strip is given back.
        let hidden = pump(STEP_MS, || work_area().right == sw && window_rect(panel) == tab);
        say_rect(b"the panel hidden", window_rect(panel));
        c.ok(hidden, work_area().right.into())?;
        c.eq(taskbar().right.into(), sw.into())?;
        c.eq(hidden_value(), 1)?;
        let dock = find("DockTab");
        c.ok(dock.as_ref().is_some_and(|e| e.name() == "Agent"), 0)?;
        let dock = dock.unwrap_or_else(|| unreachable!());
        c.eq(dock.control_type().into(), BUTTON.into())?;
        let r = dock.rect();
        let r = Rect { left: r.left, top: r.top, right: r.right, bottom: r.bottom };
        say_rect(b"DockTab", r);
        c.ok(r == tab, r.left.into())?;
        uia_client::announce(NAME, "DockTab", &dock);
        say(b"hidden");

        // The dock tab (the scenario's click): docked again.
        let again =
            pump(STEP_MS, || work_area().right == sw - WIDTH && window_rect(panel) == docked);
        c.ok(again, work_area().right.into())?;
        c.eq(hidden_value(), 0)?;

        // A hand-off (Explorer's "Make the change" sends the same): taken,
        // listed with the part of the window it is about.
        c.eq(copy_data(panel, HANDOFF + 1, REQUEST) as i64, 0)?;
        c.eq(copy_data(panel, HANDOFF, b"gala-handoff 1\0short\0") as i64, 0)?;
        c.eq(copy_data(panel, HANDOFF, REQUEST) as i64, 1)?;
        let listed = pump(STEP_MS, || {
            find("Request0Text").is_some_and(|e| e.name() == "make the header bigger")
        });
        c.ok(listed, 0)?;
        c.ok(find("Request0Element").is_some_and(|e| e.name() == "On: the header"), 0)?;
        c.ok(find("Request1Text").is_none(), 0)?;
        say(b"handed off");
        // This test's own window, found as the panel's elements are, is
        // the last click: the screen scenario checks the docked panel
        // (with the hand-off listed) before it, and the run ends after
        // it.
        let window = uia.window(own);
        c.ok(window.is_some(), 0)?;
        let window = window.unwrap_or_else(|| unreachable!());
        uia_client::announce(NAME, "TestWindow", &window);
        say(b"docked again");
        c.ok(pump(STEP_MS, || CLICKED.load(Ordering::Relaxed)), 0)?;
        DestroyWindow(own);
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

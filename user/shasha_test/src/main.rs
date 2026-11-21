//! gala's user-mode taskbar (shasha.exe; ROADMAP SH step 1, part 6;
//! ARCHITECTURE "The user-mode shell"), as a program sees it. With
//! `shell=shasha` the desktop starts shasha.exe with the session; this
//! test checks it against what Windows documents for the shell's taskbar
//! (GetShellWindow, SHAppBarMessage ABM_GETTASKBARPOS, SPI_GETWORKAREA),
//! with the screen scenario clicking the taskbar's buttons, found by UI
//! Automation:
//!
//! - the desktop window ("Program Manager", of class Progman as
//!   Explorer's) is the shell's window (GetShellWindow), shown over the
//!   whole screen; the wallpaper (SPI_GETDESKWALLPAPER) is the picture
//!   the boot chose, `C:\Windows\Web\Wallpaper\night.qoi`;
//! - the taskbar's window ("Taskbar", of class Shell_TrayWnd as
//!   Explorer's) is the strip at the bottom of the screen,
//!   `HEIGHT` (48) high and the whole width; ABM_GETTASKBARPOS answers
//!   with that strip on the bottom edge, and the work area ends above it;
//! - its UI Automation elements: "StartButton" a Button named "Start",
//!   "Clock" a Text named with the time (`HH:MM`), and a Button per
//!   window a program shows, its AutomationId `Task<hwnd in hex>` and its
//!   name the window's title;
//! - with two windows of this test shown, the second active: a click on
//!   the first's button (the scenario's, at the place this test
//!   announces) makes it the foreground window; a second click, while it
//!   is active, minimizes it (as Windows' taskbar does); the taskbar's
//!   window is never activated by them (WS_EX_NOACTIVATE);
//! - the start menu (part 7): a click on the Start button shows the
//!   "Start" window (a GalaUiWindow, topmost, above the taskbar) as the
//!   foreground window, its "Search" input (an Edit, the keyboard focus)
//!   empty and a tile per installed program (a Button, AutomationId
//!   `Program` and the name without spaces, named with the program's
//!   name: here Command Prompt and Task Manager, the programs the
//!   scenario installs); typing filters the tiles by name (`task`: Task
//!   Manager's alone); a click on a tile starts the program and hides
//!   the menu; the Windows key alone (gala's hot key for it, the shell's)
//!   shows it again with the search cleared; a click on another window
//!   (which then is active) hides it;
//! - the desktop (part 9): a click on it (the scenario's, at an empty
//!   point this test announces) makes the desktop window the foreground
//!   window, and it stays under the others: a click on window two then
//!   reaches window two, which becomes active again;
//!   SPI_SETDESKWALLPAPER with `C:\Windows\Web\Wallpaper\dunes.qoi`
//!   succeeds and SPI_GETDESKWALLPAPER reads it back (shasha, told
//!   with WM_SETTINGCHANGE, shows it: the scenario's expected line);
//! - shasha ended (TerminateProcess): its windows are gone, there is no
//!   shell window, and the desktop holds the taskbar's strip again,
//!   reserved and drawn by nothing (ABM_GETTASKBARPOS the same strip, the work area unchanged).
//!
//! Assumed and not pinned: that shasha.exe is gala's own, so its height
//! and the ids of its elements are gala's choice.
//!
//! Prints `shasha_test: listed`, `shasha_test: activated`, `shasha_test:
//! minimized`, `shasha_test: menu ready`, `shasha_test: menu open`,
//! `shasha_test: filtered`, `shasha_test: started`, `shasha_test: menu
//! again`, `shasha_test: closed`, `shasha_test: desktop ready`,
//! `shasha_test: desktop clicked`, `shasha_test: two above`,
//! `shasha_test: wallpaper set` and `shasha_test: ok`, or `shasha_test:
//! check N got X (line L)` and exits with N.
//!
//! Assumed and not pinned either: that a click on the desktop makes
//! Progman itself the foreground window (Windows may give it to a
//! WorkerW window of Explorer's).

#![no_std]
#![no_main]

use ntrt as _;
use uia_client::{BUTTON, EDIT, Element, TEXT, Uia};

const NAME: &str = "shasha_test";

type Handle = usize;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, overlapped: usize) -> i32;
    fn ExitProcess(code: u32) -> !;
    fn GetModuleHandleW(name: *const u16) -> Handle;
    fn Sleep(ms: u32);
    fn GetTickCount64() -> u64;
    fn OpenProcess(access: u32, inherit: i32, pid: u32) -> Handle;
    fn TerminateProcess(h: Handle, code: u32) -> i32;
    fn CloseHandle(h: Handle) -> i32;
    fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
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
    fn GetForegroundWindow() -> Handle;
    fn IsIconic(hwnd: Handle) -> i32;
    fn IsWindowVisible(hwnd: Handle) -> i32;
    fn PostMessageW(hwnd: Handle, msg: u32, wparam: usize, lparam: isize) -> i32;
    fn FindWindowW(class: *const u16, title: *const u16) -> Handle;
    fn GetWindowRect(hwnd: Handle, rect: *mut Rect) -> i32;
    fn GetShellWindow() -> Handle;
    fn GetWindowThreadProcessId(hwnd: Handle, pid: *mut u32) -> u32;
    fn SystemParametersInfoW(action: u32, param: u32, out: *mut u8, ini: u32) -> i32;
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

const ABM_GETTASKBARPOS: u32 = 5;
const ABE_BOTTOM: u32 = 3;
const SPI_GETWORKAREA: u32 = 0x30;
const SPI_SETDESKWALLPAPER: u32 = 0x14;
const SPI_GETDESKWALLPAPER: u32 = 0x73;
/// SPIF_UPDATEINIFILE | SPIF_SENDCHANGE.
const SPIF_SAVE_AND_TELL: u32 = 3;
const NIGHT: &str = "C:\\Windows\\Web\\Wallpaper\\night.qoi";
const DUNES: &str = "C:\\Windows\\Web\\Wallpaper\\dunes.qoi";
const SM_CXSCREEN: i32 = 0;
const SM_CYSCREEN: i32 = 1;
const WS_OVERLAPPEDWINDOW: u32 = 0x00cf_0000;
const SW_SHOW: i32 = 5;
const WM_CLOSE: u32 = 0x0010;
const PROCESS_TERMINATE: u32 = 0x0001;
const SYNCHRONIZE: u32 = 0x0010_0000;
/// The taskbar's height (shasha's HEIGHT, the desktop's TASKBAR strip).
const HEIGHT: i32 = 48;
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
    unsafe { SystemParametersInfoW(SPI_GETWORKAREA, 0, (&raw mut r).cast(), 0) };
    r
}

/// SPI_GETDESKWALLPAPER's path, whether it succeeded.
fn wallpaper(out: &mut [u16; 262]) -> bool {
    // SAFETY: our buffer, its length in units.
    unsafe { SystemParametersInfoW(SPI_GETDESKWALLPAPER, 262, out.as_mut_ptr().cast(), 0) != 0 }
}

/// Whether `w` (NUL-ended) is `s`.
fn same(w: &[u16], s: &str) -> bool {
    let n = w.iter().position(|&c| c == 0).unwrap_or(w.len());
    n == s.len() && w.iter().zip(s.bytes()).all(|(&a, b)| a == u16::from(b))
}

/// Prints `<name> at x,y (pointer at px,py)` for the scenario's click.
fn announce_point(name: &[u8], x: i32, y: i32) {
    let (px, py) = uia_client::pointer();
    Line::new()
        .s(NAME.as_bytes())
        .s(b": ")
        .s(name)
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

fn window_rect(hwnd: Handle) -> Rect {
    let mut r = Rect::default();
    // SAFETY: our RECT.
    unsafe { GetWindowRect(hwnd, &mut r) };
    r
}

// ---- The checks -----------------------------------------------------------

extern "system" fn proc_(hwnd: Handle, msg: u32, wparam: usize, lparam: isize) -> isize {
    // SAFETY: the default handling.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// ABM_GETTASKBARPOS: whether it answered, the edge and the strip.
fn taskbar() -> (usize, u32, Rect) {
    let mut d = AppBarData {
        size: core::mem::size_of::<AppBarData>() as u32,
        hwnd: 0,
        callback: 0,
        edge: 99,
        rect: Rect::default(),
        param: 0,
    };
    // SAFETY: our APPBARDATA.
    let r = unsafe { SHAppBarMessage(ABM_GETTASKBARPOS, &mut d) };
    (r, d.edge, d.rect)
}

/// The AutomationId of window `hwnd`'s taskbar button: `Task` and the
/// handle in hex (lower case, no leading zeros).
fn task_id(hwnd: Handle, buf: &mut [u8; 24]) -> &str {
    buf[..4].copy_from_slice(b"Task");
    let mut digits = [0u8; 16];
    let (mut v, mut n) = (hwnd, 0);
    loop {
        digits[n] = b"0123456789abcdef"[v & 15];
        n += 1;
        v >>= 4;
        if v == 0 {
            break;
        }
    }
    for i in 0..n {
        buf[4 + i] = digits[n - 1 - i];
    }
    core::str::from_utf8(&buf[..4 + n]).unwrap_or("")
}

/// Makes and shows a window of this test titled `title` at `x`.
fn make_window(class: &W, instance: Handle, title: &str, x: i32) -> Handle {
    let t = W::new(title);
    // SAFETY: our registered class and strings.
    let hwnd = unsafe {
        CreateWindowExW(
            0,
            class.ptr(),
            t.ptr(),
            WS_OVERLAPPEDWINDOW,
            x,
            120,
            420,
            280,
            0,
            0,
            instance,
            0,
        )
    };
    if hwnd != 0 {
        // SAFETY: our window.
        unsafe {
            ShowWindow(hwnd, SW_SHOW);
            SetForegroundWindow(hwnd);
        }
    }
    hwnd
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    // SAFETY: plain calls with our structures.
    unsafe {
        let (sw, sh) = (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN));
        let strip = Rect { left: 0, top: sh - HEIGHT, right: sw, bottom: sh };

        // The desktop window, started with the session: the shell's
        // window, over the whole screen; the boot's wallpaper.
        let screen = Rect { left: 0, top: 0, right: sw, bottom: sh };
        let (desk_class, desk_title) = (W::new("Progman"), W::new("Program Manager"));
        let found = pump(STEP_MS, || {
            let d = FindWindowW(desk_class.ptr(), desk_title.ptr());
            d != 0 && GetShellWindow() == d && IsWindowVisible(d) != 0
        });
        let desk = FindWindowW(desk_class.ptr(), desk_title.ptr());
        c.ok(desk != 0, 0)?;
        say_rect(b"the desktop", window_rect(desk));
        c.ok(found, GetShellWindow() as i64)?;
        c.ok(window_rect(desk) == screen, window_rect(desk).right.into())?;
        let mut path = [0u16; 262];
        c.ok(wallpaper(&mut path), 0)?;
        c.ok(same(&path, NIGHT), path[0].into())?;

        // The taskbar: the strip, the bar ABM_GETTASKBARPOS names.
        let (class, title) = (W::new("Shell_TrayWnd"), W::new("Taskbar"));
        let found = pump(STEP_MS, || {
            let t = FindWindowW(class.ptr(), title.ptr());
            t != 0 && window_rect(t) == strip
        });
        let bar = FindWindowW(class.ptr(), title.ptr());
        c.ok(bar != 0, 0)?;
        say_rect(b"the taskbar", window_rect(bar));
        c.ok(found, window_rect(bar).top.into())?;
        let (ok, edge, r) = taskbar();
        c.eq(ok as i64, 1)?;
        c.eq(edge.into(), ABE_BOTTOM.into())?;
        c.ok(r == strip, r.top.into())?;
        c.eq(work_area().bottom.into(), (sh - HEIGHT).into())?;

        // Two windows of this test, the second active.
        let cls = W::new("ShashaTestWindow");
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
        let one = make_window(&cls, wc.instance, "Shasha test one", 80);
        c.ok(one != 0, 0)?;
        let two = make_window(&cls, wc.instance, "Shasha test two", 560);
        c.ok(two != 0, 0)?;
        c.ok(pump(STEP_MS, || GetForegroundWindow() == two), GetForegroundWindow() as i64)?;

        // The taskbar as UI Automation sees it.
        let uia = Uia::new();
        c.ok(uia.is_ok(), uia.as_ref().err().copied().unwrap_or(0).into())?;
        let uia = uia.unwrap_or_else(|_| unreachable!());
        // The boot's wallpaper shown: the element named after its file.
        let named = |want: &str| {
            uia.window(desk)
                .and_then(|d| uia.find(&d, "Wallpaper"))
                .is_some_and(|e| e.name() == want)
        };
        c.ok(pump(STEP_MS, || named(NIGHT)), 0)?;
        let find = |id: &str| -> Option<Element> { uia.find(&uia.window(bar)?, id) };
        let start = find("StartButton");
        c.ok(start.as_ref().is_some_and(|e| e.name() == "Start"), 0)?;
        c.eq(start.map_or(0, |e| e.control_type()).into(), BUTTON.into())?;
        let clock = find("Clock");
        c.ok(clock.is_some(), 0)?;
        let clock = clock.unwrap_or_else(|| unreachable!());
        c.eq(clock.control_type().into(), TEXT.into())?;
        let time = clock.name();
        let t = time.as_str().as_bytes();
        c.ok(t.len() == 5 && t[2] == b':', t.len() as i64)?;
        // A button per window, named with its title (listed once the
        // windows' WinEvents reach the taskbar).
        let (mut id1, mut id2) = ([0u8; 24], [0u8; 24]);
        let (id1, id2) = (task_id(one, &mut id1), task_id(two, &mut id2));
        let listed = pump(STEP_MS, || {
            find(id1).is_some_and(|e| e.name() == "Shasha test one")
                && find(id2).is_some_and(|e| e.name() == "Shasha test two")
        });
        c.ok(listed, 0)?;
        let task = find(id1).unwrap_or_else(|| unreachable!());
        c.eq(task.control_type().into(), BUTTON.into())?;
        let r = task.rect();
        c.ok(r.top >= strip.top && r.bottom <= sh && r.left >= 0 && r.right <= sw, r.top.into())?;
        uia_client::announce(NAME, "TaskOne", &task);
        say(b"listed");

        // The scenario's click: the first window comes to the front.
        let front = pump(STEP_MS, || GetForegroundWindow() == one);
        c.ok(front, GetForegroundWindow() as i64)?;
        c.eq(IsIconic(one).into(), 0)?;
        let task = find(id1);
        c.ok(task.is_some(), 0)?;
        uia_client::announce(NAME, "TaskOne", &task.unwrap_or_else(|| unreachable!()));
        say(b"activated");

        // Clicked again while active: minimized.
        c.ok(pump(STEP_MS, || IsIconic(one) != 0), 0)?;
        c.ok(GetForegroundWindow() != bar, 0)?;
        say(b"minimized");

        // The start menu: the Start button's click shows it, in front.
        let start = find("StartButton");
        c.ok(start.is_some(), 0)?;
        uia_client::announce(NAME, "StartButton", &start.unwrap_or_else(|| unreachable!()));
        say(b"menu ready");
        let (menu_class, menu_title) = (W::new("GalaUiWindow"), W::new("Start"));
        let shown = pump(STEP_MS, || {
            let m = FindWindowW(menu_class.ptr(), menu_title.ptr());
            m != 0 && IsWindowVisible(m) != 0 && GetForegroundWindow() == m
        });
        let menu = FindWindowW(menu_class.ptr(), menu_title.ptr());
        c.ok(menu != 0, 0)?;
        c.ok(shown, GetForegroundWindow() as i64)?;
        let r = window_rect(menu);
        say_rect(b"the start menu", r);
        // Above the taskbar, within the screen.
        c.ok(r.bottom <= strip.top && r.top >= 0 && r.left >= 0 && r.right <= sw, r.bottom.into())?;
        let in_menu = |id: &str| -> Option<Element> { uia.find(&uia.window(menu)?, id) };
        let search = in_menu("Search");
        c.ok(search.as_ref().is_some_and(|e| e.name() == "Search"), 0)?;
        let search = search.unwrap_or_else(|| unreachable!());
        c.eq(search.control_type().into(), EDIT.into())?;
        c.ok(search.value().as_str().is_empty(), search.value().as_str().len() as i64)?;
        let tile = |id: &str, name: &str| in_menu(id).is_some_and(|e| e.name() == name);
        let both = || {
            tile("ProgramCommandPrompt", "Command Prompt")
                && tile("ProgramTaskManager", "Task Manager")
        };
        c.ok(pump(STEP_MS, both), 0)?;
        let t = in_menu("ProgramTaskManager").unwrap_or_else(|| unreachable!());
        c.eq(t.control_type().into(), BUTTON.into())?;
        say(b"menu open");

        // Typed into the search (the scenario's keys): Task Manager's
        // tile alone.
        let filtered = pump(STEP_MS, || {
            in_menu("Search").is_some_and(|e| e.value().as_str() == "task")
                && in_menu("ProgramCommandPrompt").is_none()
                && tile("ProgramTaskManager", "Task Manager")
        });
        c.ok(filtered, 0)?;
        let t = in_menu("ProgramTaskManager").unwrap_or_else(|| unreachable!());
        uia_client::announce(NAME, "TaskManagerTile", &t);
        say(b"filtered");

        // The tile's click: Task Manager started, the menu hidden.
        let tm_title = W::new("Task Manager");
        let started = pump(STEP_MS, || {
            IsWindowVisible(menu) == 0 && FindWindowW(core::ptr::null(), tm_title.ptr()) != 0
        });
        c.ok(started, IsWindowVisible(menu).into())?;
        say(b"started");
        PostMessageW(FindWindowW(core::ptr::null(), tm_title.ptr()), WM_CLOSE, 0, 0);

        // The Windows key alone (the scenario's): the menu again, the
        // search cleared.
        let again = pump(STEP_MS, || {
            IsWindowVisible(menu) != 0
                && GetForegroundWindow() == menu
                && in_menu("Search").is_some_and(|e| e.value().as_str().is_empty())
                && both()
        });
        c.ok(again, GetForegroundWindow() as i64)?;
        // A click on window two (its caption's middle, above the menu).
        let w = window_rect(two);
        let (x, y) = ((w.left + w.right) / 2, w.top + 12);
        c.ok(y < window_rect(menu).top, y.into())?;
        announce_point(b"WindowTwo", x, y);
        say(b"menu again");

        // Clicked away: the menu hides, the window clicked is active.
        let closed = pump(STEP_MS, || IsWindowVisible(menu) == 0 && GetForegroundWindow() == two);
        c.ok(closed, GetForegroundWindow() as i64)?;
        let gone = pump(STEP_MS, || FindWindowW(core::ptr::null(), tm_title.ptr()) == 0);
        c.ok(gone, 0)?;
        say(b"closed");

        // A click on the desktop (an empty point right of window two,
        // above the strip): the desktop window is active...
        let (dx, dy) = (sw - 180, sh - HEIGHT - 150);
        c.ok(dx > window_rect(two).right, dx.into())?;
        announce_point(b"Desktop", dx, dy);
        say(b"desktop ready");
        let clicked = pump(STEP_MS, || GetForegroundWindow() == desk);
        c.ok(clicked, GetForegroundWindow() as i64)?;
        c.ok(IsWindowVisible(two) != 0, 0)?;
        // ... and under window two: a click on it reaches it.
        let w = window_rect(two);
        announce_point(b"WindowTwo", (w.left + w.right) / 2, w.top + 12);
        say(b"desktop clicked");
        let above = pump(STEP_MS, || GetForegroundWindow() == two);
        c.ok(above, GetForegroundWindow() as i64)?;
        say(b"two above");

        // Another wallpaper: saved, read back; shasha is told.
        let dunes = W::new(DUNES);
        c.eq(
            SystemParametersInfoW(
                SPI_SETDESKWALLPAPER,
                0,
                dunes.ptr().cast_mut().cast(),
                SPIF_SAVE_AND_TELL,
            )
            .into(),
            1,
        )?;
        c.ok(wallpaper(&mut path), 0)?;
        c.ok(same(&path, DUNES), path[0].into())?;
        // Shown: shasha's "Wallpaper" element is named after its file.
        c.ok(pump(STEP_MS, || named(DUNES)), 0)?;
        say(b"wallpaper set");

        // shasha ended: no shell window, and the desktop holds the
        // taskbar's strip again (reserved, drawn by nothing).
        let mut pid = 0u32;
        GetWindowThreadProcessId(bar, &mut pid);
        c.ok(pid != 0, 0)?;
        let h = OpenProcess(PROCESS_TERMINATE | SYNCHRONIZE, 0, pid);
        c.ok(h != 0, 0)?;
        c.ok(TerminateProcess(h, 1) != 0, 0)?;
        // Ended (signaled): everything it held is let go, its bar too,
        // so the strip seen next is the desktop's reservation.
        c.eq(WaitForSingleObject(h, 10_000).into(), 0)?;
        CloseHandle(h);
        let gone = pump(STEP_MS, || {
            FindWindowW(class.ptr(), title.ptr()) == 0
                && FindWindowW(desk_class.ptr(), desk_title.ptr()) == 0
                && GetShellWindow() == 0
        });
        c.ok(gone, GetShellWindow() as i64)?;
        let back = pump(STEP_MS, || taskbar().0 == 1 && taskbar().2 == strip);
        let (_, edge, r) = taskbar();
        say_rect(b"the taskbar after shasha", r);
        c.ok(back, r.top.into())?;
        c.eq(edge.into(), ABE_BOTTOM.into())?;
        c.eq(work_area().bottom.into(), (sh - HEIGHT).into())?;
        DestroyWindow(two);
        DestroyWindow(one);
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

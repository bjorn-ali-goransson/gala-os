//! gala's `shasha.exe` (ROADMAP SH step 1, part 6; ARCHITECTURE "The
//! user-mode shell"): the taskbar as a program of the session instead of
//! the kernel's drawing. The desktop starts it with the session when the
//! boot asks for it (`shell=shasha`) and it is installed
//! (`%SystemRoot%\System32\shasha.exe`).
//!
//! - **The strip**: a popup window `HEIGHT` high across the bottom of the
//!   screen, topmost and never activated (WS_EX_NOACTIVATE: a click on a
//!   button acts on the window that is active, as on Windows). It is the
//!   shell's window (SetShellWindow) and takes over the taskbar's app bar
//!   from the desktop (GalaAppBar's ABM_GALA_TASKBAR, which only the
//!   shell window's process may ask): ABM_GETTASKBARPOS answers with it,
//!   the work area stays above it, and the desktop stops drawing its own
//!   taskbar. When shasha ends, the desktop's taskbar comes back.
//! - **The buttons**: the desktop's window list (GalaDesktopAction
//!   ACTION_WINDOWS), read again on each WinEvent that changes it
//!   (EVENT_SYSTEM_FOREGROUND, MINIMIZESTART, MINIMIZEEND; EVENT_OBJECT_
//!   CREATE, DESTROY, SHOW, HIDE, NAMECHANGE of a window); a click asks
//!   for ACTION_ACTIVATE or ACTION_MINIMIZE (`Taskbar::click`).
//! - **The start menu** (part 7): a second window, "Start" (a popup,
//!   topmost, made hidden), centred above the strip. The Start button
//!   and the Windows key alone (RegisterHotKey (MOD_WIN, VK_LWIN), gala's
//!   hot key for it, which only the shell's process may take) show it
//!   if hidden, else hide it. Shown, it lists the desktop's installed
//!   programs again (ACTION_PROGRAMS), clears the search, comes to the
//!   front and puts the keyboard focus on the search. A tile's click
//!   starts its program (ACTION_START, a console program in a console
//!   window of its own) and hides the menu; another window becoming
//!   active hides it too (WM_ACTIVATE), as on Windows.
//! - **The clock**: the local time, read each second (WM_TIMER), the
//!   view changed when the minute does.
//!
//! The view and what a message does to it are the library (`shasha`);
//! this file does what is outside it.
//!
//! Observability: each line on standard output (`shasha: ...`, the
//! serial log for a program the desktop starts): the start, the shell
//! window taken or refused, the strip handed over or why not, every
//! window list that changed (how many windows), every click and the
//! desktop's answer (its NTSTATUS when refused), the menu opening (how
//! many programs) and closing (why), each program started (its path and
//! pid, or the status), the hot key refused, and gala_ui's line for each
//! message handled (`click Task20010: ...`).

#![no_std]
#![no_main]

extern crate alloc;

use gala_heap as _;

use alloc::format;
use alloc::vec::Vec;
use core::ffi::c_void;
use core::sync::atomic::{AtomicUsize, Ordering};

use gala_ui::{Component, Node, win};
use shasha::{
    Click, HEIGHT, MENU_GAP, MENU_HEIGHT, MENU_WIDTH, MenuMsg, Msg, StartMenu, Taskbar, clock_text,
    parse_programs, parse_windows,
};

type Handle = *mut c_void;
type Bool = i32;

#[repr(C)]
#[derive(Clone, Copy, Default, PartialEq)]
struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

/// SYSTEMTIME.
#[repr(C)]
#[derive(Default)]
struct SystemTime {
    year: u16,
    month: u16,
    weekday: u16,
    day: u16,
    hour: u16,
    minute: u16,
    second: u16,
    ms: u16,
}

type WinEventProc = extern "system" fn(usize, u32, Handle, i32, i32, u32, u32);

const SM_CXSCREEN: i32 = 0;
const SM_CYSCREEN: i32 = 1;
const HWND_TOPMOST: isize = -1;
const SWP_NOACTIVATE: u32 = 0x0010;
const WINEVENT_OUTOFCONTEXT: u32 = 0;
const OBJID_WINDOW: i32 = 0;
const EVENT_SYSTEM_FOREGROUND: u32 = 0x0003;
const EVENT_SYSTEM_MINIMIZEEND: u32 = 0x0017;
const EVENT_OBJECT_CREATE: u32 = 0x8000;
const EVENT_OBJECT_NAMECHANGE: u32 = 0x800c;
const WM_TIMER: u32 = 0x0113;
const WM_ACTIVATE: u32 = 0x0006;
const WA_INACTIVE: usize = 0;
const WM_HOTKEY: u32 = 0x0312;
const MOD_WIN: u32 = 0x0008;
const VK_LWIN: u32 = 0x5b;
const SW_HIDE: i32 = 0;
const SW_SHOW: i32 = 5;
/// Read the window list again (the WinEvent hook's, posted).
const WM_APP_WINDOWS: u32 = 0x8001;
/// Show or hide the start menu (the Start button's, posted to the menu).
const WM_APP_TOGGLE: u32 = 0x8002;
const CLOCK_TIMER: usize = 1;
/// The start menu's hot key's id (the Windows key alone).
const MENU_HOT_KEY: i32 = 1;

/// gala's app bar op: the caller's window (the shell's) becomes the
/// taskbar's bar in the desktop's place (kernel desktop::appbar).
const ABM_GALA_TASKBAR: u32 = 0x100;
const ABE_BOTTOM: u32 = 3;
/// GalaAppBar's request (shell32 browser.rs): window, callback, edge,
/// rectangle, result.
const APP_BAR_REQUEST: usize = 36;

// GalaDesktopAction (kernel/src/nt/action.rs).
const REQUEST_VERSION: u32 = 1;
const REQUEST_HEADER: usize = 40;
const ACTION_WINDOWS: u32 = 2;
const ACTION_ACTIVATE: u32 = 3;
const ACTION_MINIMIZE: u32 = 4;
const ACTION_START: u32 = 1;
const ACTION_PROGRAMS: u32 = 11;
/// ACTION_START's flag: a console program in a console window of its own.
const LAUNCH_CONSOLE_WINDOW: i32 = 1;
const LIST_HEADER: usize = 8;
const PROGRAM_ENTRY: usize = 8 + 2 * 32 + 2 * 128;
const MAX_PROGRAMS: usize = 64;
const WINDOW_ENTRY: usize = 40 + 2 * 128;
const MAX_WINDOWS: usize = 64;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut c_void) -> Bool;
    fn ExitProcess(code: u32) -> !;
    fn GetLocalTime(t: *mut SystemTime);
    fn GetLastError() -> u32;
}

#[link(name = "user32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetSystemMetrics(index: i32) -> i32;
    fn SetWindowPos(hwnd: Handle, after: isize, x: i32, y: i32, w: i32, h: i32, flags: u32)
    -> Bool;
    fn PostMessageW(hwnd: Handle, msg: u32, wparam: usize, lparam: isize) -> Bool;
    fn SetShellWindow(hwnd: Handle) -> Bool;
    fn ShowWindow(hwnd: Handle, cmd: i32) -> Bool;
    fn IsWindowVisible(hwnd: Handle) -> Bool;
    fn SetForegroundWindow(hwnd: Handle) -> Bool;
    fn RegisterHotKey(hwnd: Handle, id: i32, modifiers: u32, vk: u32) -> Bool;
    fn SetTimer(hwnd: Handle, id: usize, ms: u32, proc_: *const c_void) -> usize;
    fn SetWinEventHook(
        min: u32,
        max: u32,
        module: usize,
        proc_: WinEventProc,
        pid: u32,
        tid: u32,
        flags: u32,
    ) -> usize;
}

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn GalaAppBar(op: u32, request: *mut u8, length: usize) -> u32;
    fn GalaDesktopAction(
        request: *const u8,
        len: u32,
        out: *mut u8,
        out_len: u32,
        returned: *mut u32,
    ) -> u32;
}

#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memmove(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn __chkstk();
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
    fn strlen(s: *const u8) -> usize;
}

#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

#[unsafe(no_mangle)]
pub extern "C" fn __CxxFrameHandler3() -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdeac) }
}

/// The taskbar's window (0 until it is made).
static WINDOW: AtomicUsize = AtomicUsize::new(0);
/// The start menu's window (0 until it is made).
static MENU: AtomicUsize = AtomicUsize::new(0);

fn window() -> Handle {
    WINDOW.load(Ordering::Acquire) as Handle
}

fn menu_window() -> Handle {
    MENU.load(Ordering::Acquire) as Handle
}

/// A line on standard output (`shasha: ...`).
fn log(text: &str) {
    let s = format!("shasha: {text}\r\n");
    let mut n = 0;
    // SAFETY: a valid buffer; the standard output handle (or none).
    unsafe {
        WriteFile(
            GetStdHandle(-11i32 as u32),
            s.as_ptr(),
            s.len() as u32,
            &mut n,
            core::ptr::null_mut(),
        )
    };
}

/// GalaDesktopAction's `action` on window `hwnd`: its NTSTATUS and the
/// bytes it gave back into `out`.
fn action(action: u32, hwnd: u64, out: &mut [u8]) -> (u32, usize) {
    request(action, hwnd, [0; 4], "", out)
}

/// GalaDesktopAction's `action` on window `hwnd` with `args` and `text`.
fn request(action: u32, hwnd: u64, args: [i32; 4], text: &str, out: &mut [u8]) -> (u32, usize) {
    let units: Vec<u16> = text.encode_utf16().collect();
    let mut b: Vec<u8> = Vec::new();
    b.resize(REQUEST_HEADER + 2 * units.len(), 0);
    b[0..4].copy_from_slice(&REQUEST_VERSION.to_le_bytes());
    b[4..8].copy_from_slice(&action.to_le_bytes());
    b[8..16].copy_from_slice(&hwnd.to_le_bytes());
    for (k, a) in args.iter().enumerate() {
        b[16 + 4 * k..20 + 4 * k].copy_from_slice(&a.to_le_bytes());
    }
    b[32..36].copy_from_slice(&(units.len() as u32).to_le_bytes());
    for (k, u) in units.iter().enumerate() {
        b[REQUEST_HEADER + 2 * k..REQUEST_HEADER + 2 * k + 2].copy_from_slice(&u.to_le_bytes());
    }
    let mut returned = 0u32;
    // SAFETY: our buffers, with their lengths.
    let status = unsafe {
        GalaDesktopAction(
            b.as_ptr(),
            b.len() as u32,
            out.as_mut_ptr(),
            out.len() as u32,
            &mut returned,
        )
    };
    (status, returned as usize)
}

/// The desktop's window list, or None (logged) when it cannot be read.
fn windows() -> Option<Vec<shasha::Task>> {
    let mut b: Vec<u8> = Vec::new();
    b.resize(LIST_HEADER + MAX_WINDOWS * WINDOW_ENTRY, 0);
    let (status, n) = action(ACTION_WINDOWS, 0, &mut b);
    if status != 0 {
        log(&format!("the window list is not read: status {status:#x}"));
        return None;
    }
    Some(parse_windows(&b[..n.min(b.len())]))
}

/// The desktop's installed programs, or None (logged).
fn programs() -> Option<Vec<shasha::Program>> {
    let mut b: Vec<u8> = Vec::new();
    b.resize(LIST_HEADER + MAX_PROGRAMS * PROGRAM_ENTRY, 0);
    let (status, n) = action(ACTION_PROGRAMS, 0, &mut b);
    if status != 0 {
        log(&format!("the program list is not read: status {status:#x}"));
        return None;
    }
    Some(parse_programs(&b[..n.min(b.len())]))
}

fn clock() -> alloc::string::String {
    let mut t = SystemTime::default();
    // SAFETY: a SYSTEMTIME to fill.
    unsafe { GetLocalTime(&mut t) };
    clock_text(t.hour, t.minute)
}

/// Hands the taskbar's app bar to `hwnd` (the shell's window): the strip
/// it is given, or None (logged).
fn take_taskbar(hwnd: Handle) -> Option<Rect> {
    let mut r = [0u8; APP_BAR_REQUEST];
    r[0..8].copy_from_slice(&(hwnd as u64).to_le_bytes());
    r[12..16].copy_from_slice(&ABE_BOTTOM.to_le_bytes());
    // SAFETY: a request of its length.
    let status = unsafe { GalaAppBar(ABM_GALA_TASKBAR, r.as_mut_ptr(), r.len()) };
    let at = |o: usize| i32::from_le_bytes([r[o], r[o + 1], r[o + 2], r[o + 3]]);
    if status != 0 || at(32) == 0 {
        log(&format!("the taskbar's strip is not handed over: status {status:#x}"));
        return None;
    }
    Some(Rect { left: at(16), top: at(20), right: at(24), bottom: at(28) })
}

extern "system" fn changed(
    _: usize,
    event: u32,
    hwnd: Handle,
    object: i32,
    _: i32,
    _: u32,
    _: u32,
) {
    let ours = window();
    if object == OBJID_WINDOW && !hwnd.is_null() && hwnd != ours && !ours.is_null() {
        // SAFETY: our window; the loop reads the list.
        unsafe { PostMessageW(ours, WM_APP_WINDOWS, event as usize, 0) };
    }
}

struct App {
    bar: Taskbar,
}

impl Component for App {
    type Msg = Msg;

    fn view(&self) -> Node<Msg> {
        self.bar.view()
    }

    fn update(&mut self, msg: Msg) {
        match &msg {
            Msg::Start => {
                // The menu's window acts on it, on this thread, once this
                // message is handled.
                // SAFETY: our menu window (or none: refused).
                if unsafe { PostMessageW(menu_window(), WM_APP_TOGGLE, 0, 0) } == 0 {
                    log("the start menu is not there");
                }
            }
            Msg::Task(hwnd) => {
                let (verb, name) = match self.bar.click(*hwnd) {
                    Some(Click::Minimize(_)) => (ACTION_MINIMIZE, "minimize"),
                    Some(Click::Activate(_)) => (ACTION_ACTIVATE, "activate"),
                    None => {
                        log(&format!("window {hwnd:#x} is no longer listed"));
                        return;
                    }
                };
                let (status, _) = action(verb, *hwnd, &mut []);
                if status == 0 {
                    log(&format!("{name} window {hwnd:#x}"));
                } else {
                    log(&format!("{name} window {hwnd:#x} refused: status {status:#x}"));
                }
            }
            Msg::Windows(tasks) => {
                log(&format!("{} window(s) listed", tasks.len()));
            }
            Msg::Clock(_) => {}
        }
        self.bar.update(msg);
    }

    fn message(&mut self, _: usize, msg: u32, wparam: usize, _: isize) -> Option<Msg> {
        match msg {
            WM_APP_WINDOWS => {
                let tasks = windows()?;
                (tasks != self.bar.tasks).then_some(Msg::Windows(tasks))
            }
            WM_TIMER if wparam == CLOCK_TIMER => {
                let c = clock();
                (c != self.bar.clock).then_some(Msg::Clock(c))
            }
            _ => None,
        }
    }
}

/// The start menu's window.
struct Menu {
    menu: StartMenu,
}

impl Menu {
    fn show(&mut self, hwnd: Handle) {
        let programs = programs().unwrap_or_default();
        let names: Vec<&str> = programs.iter().map(|p| p.name.as_str()).collect();
        log(&format!("the start menu opens: {} program(s): {}", programs.len(), names.join(", ")));
        self.menu.open(programs);
        // SAFETY: our window.
        unsafe {
            ShowWindow(hwnd, SW_SHOW);
            SetForegroundWindow(hwnd);
        }
        if !win::focus(hwnd, "Search") {
            log("the search is not focused: typing goes nowhere");
        }
    }

    fn hide(&mut self, hwnd: Handle, why: &str) {
        log(&format!("the start menu closes: {why}"));
        // SAFETY: our window.
        unsafe { ShowWindow(hwnd, SW_HIDE) };
    }

    fn launch(&mut self, i: usize) {
        let Some(p) = self.menu.programs.get(i).cloned() else { return };
        let mut out = [0u8; 8];
        let text = format!("\"{}\"", p.path);
        let (status, n) =
            request(ACTION_START, 0, [LAUNCH_CONSOLE_WINDOW, 0, 0, 0], &text, &mut out);
        if status == 0 && n >= 4 {
            let pid = u32::from_le_bytes([out[0], out[1], out[2], out[3]]);
            log(&format!("start {} ({}): pid {pid}", p.name, p.path));
        } else {
            log(&format!("start {} ({}) refused: status {status:#x}", p.name, p.path));
        }
    }
}

impl Component for Menu {
    type Msg = MenuMsg;

    fn view(&self) -> Node<MenuMsg> {
        self.menu.view()
    }

    fn update(&mut self, msg: MenuMsg) {
        let hwnd = menu_window();
        // SAFETY: our window.
        let shown = unsafe { IsWindowVisible(hwnd) } != 0;
        match &msg {
            MenuMsg::Toggle if shown => self.hide(hwnd, "toggled"),
            MenuMsg::Toggle => self.show(hwnd),
            MenuMsg::Deactivated if shown => self.hide(hwnd, "another window is active"),
            MenuMsg::Launch(i) => {
                self.launch(*i);
                self.hide(hwnd, "a program started");
            }
            MenuMsg::Deactivated | MenuMsg::Query(_) => {}
        }
        self.menu.update(msg);
    }

    fn message(&mut self, window: usize, msg: u32, wparam: usize, _: isize) -> Option<MenuMsg> {
        match msg {
            WM_APP_TOGGLE => Some(MenuMsg::Toggle),
            WM_HOTKEY if wparam == MENU_HOT_KEY as usize => Some(MenuMsg::Toggle),
            // SAFETY: our window.
            WM_ACTIVATE
                if wparam & 0xffff == WA_INACTIVE
                    && unsafe { IsWindowVisible(window as Handle) } != 0 =>
            {
                Some(MenuMsg::Deactivated)
            }
            _ => None,
        }
    }
}

/// Makes the start menu's window (hidden) above the strip at `bar_top`
/// on a screen `sw` wide, and takes the Windows key for it.
fn make_menu(sw: i32, bar_top: i32) {
    let options = win::Options {
        title: "Start",
        x: (sw - MENU_WIDTH) / 2,
        y: bar_top - MENU_GAP - MENU_HEIGHT,
        width: MENU_WIDTH,
        height: MENU_HEIGHT,
        topmost: true,
        popup: true,
        no_activate: false,
        hidden: true,
    };
    let Some(hwnd) = win::open(Menu { menu: StartMenu::default() }, &options, log) else {
        log("the start menu's window could not be made: Start does nothing");
        return;
    };
    MENU.store(hwnd as usize, Ordering::Release);
    // SAFETY: our window.
    if unsafe { RegisterHotKey(hwnd, MENU_HOT_KEY, MOD_WIN, VK_LWIN) } == 0 {
        // SAFETY: plain call.
        let e = unsafe { GetLastError() };
        log(&format!("the Windows key is not the start menu's: RegisterHotKey error {e}"));
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    log("started");
    // SAFETY: plain calls.
    let (sw, sh) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
    let options = win::Options {
        title: "Taskbar",
        x: 0,
        y: sh - HEIGHT,
        width: sw,
        height: HEIGHT,
        topmost: true,
        popup: true,
        no_activate: true,
        hidden: false,
    };
    let app = App { bar: Taskbar { tasks: Vec::new(), clock: clock() } };
    let Some(hwnd) = win::open(app, &options, log) else {
        log("the window could not be made");
        // SAFETY: plain call.
        unsafe { ExitProcess(1) }
    };
    WINDOW.store(hwnd as usize, Ordering::Release);
    // SAFETY: our window.
    if unsafe { SetShellWindow(hwnd) } == 0 {
        // Another shell holds the session: this one does not take over.
        log("not the shell: SetShellWindow refused");
        // SAFETY: plain call.
        unsafe { ExitProcess(2) }
    }
    match take_taskbar(hwnd) {
        Some(r) => {
            log(&format!(
                "the taskbar at {},{} {}x{}",
                r.left,
                r.top,
                r.right - r.left,
                r.bottom - r.top
            ));
            let want = Rect { left: 0, top: sh - HEIGHT, right: sw, bottom: sh };
            if r != want {
                // SAFETY: our window.
                unsafe {
                    SetWindowPos(
                        hwnd,
                        HWND_TOPMOST,
                        r.left,
                        r.top,
                        r.right - r.left,
                        r.bottom - r.top,
                        SWP_NOACTIVATE,
                    )
                };
            }
        }
        // Drawn, but the desktop's taskbar stays under it.
        None => log("the desktop keeps its taskbar"),
    }
    make_menu(sw, sh - HEIGHT);
    // The events that change the list: the active window, minimize and
    // restore; a window made, gone, shown, hidden or renamed.
    for (min, max) in [
        (EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_MINIMIZEEND),
        (EVENT_OBJECT_CREATE, EVENT_OBJECT_NAMECHANGE),
    ] {
        // SAFETY: an out-of-context hook to our procedure, on this thread.
        let hook = unsafe { SetWinEventHook(min, max, 0, changed, 0, 0, WINEVENT_OUTOFCONTEXT) };
        if hook == 0 {
            log(&format!("SetWinEventHook {min:#x}..{max:#x} failed: the list may go stale"));
        }
    }
    // SAFETY: our window; plain calls.
    unsafe {
        if SetTimer(hwnd, CLOCK_TIMER, 1000, core::ptr::null()) == 0 {
            log("SetTimer failed: the clock stops");
        }
        PostMessageW(hwnd, WM_APP_WINDOWS, 0, 0);
    }
    let code = win::pump();
    log("ended");
    // SAFETY: plain call.
    unsafe { ExitProcess(code as u32) }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    log(&format!("panic: {info}"));
    // SAFETY: plain call.
    unsafe { ExitProcess(0xdead) }
}

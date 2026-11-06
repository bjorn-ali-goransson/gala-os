//! Presenters (ROADMAP RS step B3a, ARCHITECTURE "Presenters (B3a)"): a
//! session's desktop keeps its windows and their last pixels whether or
//! not anything shows it, and a Test presenter renders it to memory,
//! where a program reads it back with gala's GalaSession service.
//!
//! GalaSession is gala's own (SeTcbPrivilege), so the expected results are
//! gala's spec (ARCHITECTURE), not Windows': on Windows the call is an
//! unknown system service and the test reports itself skipped (exit 0).
//!
//! gala starts it as SYSTEM in session 0 (`as=system`; `wallpaper=night`,
//! so the colours below are known). It checks:
//!
//! - Session 2 does not exist yet: GalaSession's query is
//!   STATUS_NOT_FOUND.
//! - SYSTEM's own token moved to session 2 (SetTokenInformation) starts
//!   the child (this program, `child`): session 2 is made. The child
//!   registers a class, makes a WS_OVERLAPPEDWINDOW window, 400 x 300 at
//!   (100, 100), shows it and paints its client area red (WM_PAINT,
//!   FillRect); its desktop is 1280 x 800 (GetSystemMetrics: the size of a
//!   desktop nothing shows) and the window is visible. Nothing shows
//!   session 2 (the console shows session 1): the calls succeed all the
//!   same.
//! - The query: session 2's desktop is live, shown by none, 1280 x 800,
//!   with a window; reading a frame is STATUS_INVALID_DEVICE_STATE.
//! - A Test presenter attached: the frame is 1280 x 800 and shows the
//!   window as the console would (the client red from (101, 133), the
//!   active caption's colour, the night wallpaper's corner, the taskbar).
//! - The child paints green: the frame shows it (damage reaches the
//!   presenter), with more flushes than before.
//! - Detached (presenter none): a frame read fails again; the child paints
//!   blue meanwhile. A new Test presenter shows blue: the window kept its
//!   last pixels while nothing showed it.
//! - Input a Test presenter brings (GalaSession's SESSION_INPUT, as
//!   screen checks without the PS/2 devices do): a left click at client
//!   (200, 100) reaches the child (WM_LBUTTONDOWN; it marks the point in
//!   magenta), which the frame shows; a desktop shown by nothing takes no
//!   input (STATUS_INVALID_DEVICE_STATE).
//! - Bad requests: an area outside the frame, an unknown presenter and an
//!   unknown input kind are STATUS_INVALID_PARAMETER; the console on
//!   session 0 is STATUS_NOT_SUPPORTED (session 0 isolation: never on
//!   the console).
//! - The console switch (fast user switching; the scenario boots by UEFI,
//!   so the screen shows session 1): WTSGetActiveConsoleSessionId and
//!   KUSER_SHARED_DATA.ActiveConsoleId are 1. A second child in session 1
//!   shows a yellow window at (600, 100) on the console. xtask's screen
//!   check sees it and clicks it (PS/2); then `gala attach 2` (gala.exe,
//!   SYSTEM's) moves the console to session 2: its desktop is on the
//!   screen (the blue window and the injected click's mark, xtask checks),
//!   session 1's is shown by nothing and keeps its window, the console id
//!   is 2 for both readers. xtask's PS/2 clicks reach session 2's window
//!   (twice: the second lets this program go on). GalaSession switches
//!   back: session 1 is on the screen again with its window and mark,
//!   session 2 is kept, the console id is 1, and a click reaches session
//!   1's window.
//! - The children end (exit code 0): session 2's window goes from a new
//!   Test frame.
//!
//! Prints `presenter_test: ok` or exits with the number of the failing
//! check (the child's failures are 0x100 + its check, seen as its exit
//! code).

#![no_std]
#![no_main]

use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, Ordering};

use ntrt::{
    NtStatus, PRESENTER_CONSOLE, PRESENTER_NONE, PRESENTER_TEST, SESSION_PRESENT, gala_session,
    session_frame, session_input, session_query,
};

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
#[derive(Clone, Copy, Default)]
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

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetLastError() -> u32;
    fn CloseHandle(h: Handle) -> Bool;
    fn GetCurrentProcess() -> Handle;
    fn GetCommandLineW() -> *const u16;
    fn GetModuleFileNameW(module: Handle, name: *mut u16, size: u32) -> u32;
    fn GetModuleHandleW(name: *const u16) -> Handle;
    fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
    fn WaitForMultipleObjects(n: u32, handles: *const Handle, all: Bool, ms: u32) -> u32;
    fn GetExitCodeProcess(h: Handle, code: *mut u32) -> Bool;
    fn CreateEventW(
        sa: *const SecurityAttributes,
        manual: Bool,
        initial: Bool,
        name: *const u16,
    ) -> Handle;
    fn SetEvent(h: Handle) -> Bool;
    fn WTSGetActiveConsoleSessionId() -> u32;
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
}

#[link(name = "advapi32", kind = "raw-dylib")]
unsafe extern "system" {
    fn OpenProcessToken(process: Handle, access: u32, token: *mut Handle) -> Bool;
    fn SetTokenInformation(token: Handle, class: u32, info: *const u8, len: u32) -> Bool;
    fn DuplicateTokenEx(
        token: Handle,
        access: u32,
        sa: *const SecurityAttributes,
        level: u32,
        kind: u32,
        new: *mut Handle,
    ) -> Bool;
    #[allow(clippy::too_many_arguments)]
    fn CreateProcessAsUserW(
        token: Handle,
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
}

#[link(name = "user32", kind = "raw-dylib")]
unsafe extern "system" {
    fn RegisterClassExW(class: *const WndClassExW) -> u16;
    #[allow(clippy::too_many_arguments)]
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
    fn InvalidateRect(hwnd: Hwnd, rect: *const Rect, erase: Bool) -> Bool;
    fn IsWindowVisible(hwnd: Hwnd) -> i32;
    fn GetSystemMetrics(index: i32) -> i32;
    fn BeginPaint(hwnd: Hwnd, ps: *mut PaintStruct) -> Handle;
    fn EndPaint(hwnd: Hwnd, ps: *const PaintStruct) -> i32;
    fn FillRect(hdc: Handle, rect: *const Rect, brush: Handle) -> i32;
    fn PeekMessageW(msg: *mut [u64; 6], hwnd: Hwnd, min: u32, max: u32, remove: u32) -> Bool;
    fn MsgWaitForMultipleObjects(
        n: u32,
        handles: *const Handle,
        all: Bool,
        ms: u32,
        wake: u32,
    ) -> u32;
    fn DispatchMessageW(msg: *const [u64; 6]) -> isize;
}

#[link(name = "gdi32", kind = "raw-dylib")]
unsafe extern "system" {
    fn CreateSolidBrush(color: u32) -> Handle;
    fn DeleteObject(object: Handle) -> Bool;
}

const STATUS_SUCCESS: NtStatus = 0;
const STATUS_INVALID_SYSTEM_SERVICE: NtStatus = 0xc000_001c;
const STATUS_INVALID_PARAMETER: NtStatus = 0xc000_000d;
const STATUS_NOT_SUPPORTED: NtStatus = 0xc000_00bb;
const STATUS_INVALID_DEVICE_STATE: NtStatus = 0xc000_0184;
const STATUS_NOT_FOUND: NtStatus = 0xc000_0225;
const TOKEN_SESSION_ID: u32 = 12;
const TOKEN_ALL_ACCESS: u32 = 0xf01ff;
const SECURITY_IMPERSONATION: u32 = 2;
const TOKEN_PRIMARY: u32 = 1;
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const WS_OVERLAPPEDWINDOW: u32 = 0x00cf_0000;
const WM_PAINT: u32 = 0x000f;
const WM_LBUTTONDOWN: u32 = 0x0201;
const QS_ALLINPUT: u32 = 0x04ff;
const SW_SHOW: i32 = 5;
const PM_REMOVE: u32 = 1;
const SM_CXSCREEN: i32 = 0;
const SM_CYSCREEN: i32 = 1;
/// KUSER_SHARED_DATA.ActiveConsoleId.
const ACTIVE_CONSOLE_ID: usize = 0x7ffe_02d8;

/// The session the child runs in (nothing shows it).
const SESSION: u32 = 2;
/// The console's session at boot, where the second child runs.
const CONSOLE: u32 = 1;
/// The children's windows: 400 x 300 at (100, 100) in session 2 and at
/// (600, 100) in session 1; the desktop's frame is a 1-pixel outline and
/// a 32-pixel caption, so a client area starts at (x + 1, 133).
const CLIENT: (u32, u32) = (101, 133);
const CLIENT_1: (u32, u32) = (601, 133);
/// The client colours the children paint, as COLORREFs (0x00BBGGRR) and
/// as the frame's pixels (0x00RRGGBB): session 2's child steps through
/// the first three, session 1's paints yellow.
const PAINTS: [(u32, u32); 4] = [
    (0x0000_00ff, 0x00ff_0000),
    (0x0000_c000, 0x0000_c000),
    (0x00ff_0000, 0x0000_00ff),
    (0x0000_ffff, 0x00ff_ff00),
];
const YELLOW: u32 = 3;
/// A click's mark: a 12 x 12 magenta square centred on it (COLORREF and
/// pixel alike).
const MARK: u32 = 0x00ff_00ff;
/// Where the injected click goes: client (200, 100) of session 2's window.
const INJECTED: (u32, u32) = (CLIENT.0 + 200, CLIENT.1 + 100);
/// The active caption's colour (kernel/src/nt/desktop).
const CAPTION_ACTIVE: u32 = 0x00ee_f2f7;

fn out(text: &[u8]) {
    ntrt::write_file(ntrt::stdout(), text);
}

/// Numbered checks; the child's numbers start at 0x100 and it does not
/// print (its exit code says which failed).
struct Checks {
    n: core::cell::Cell<u32>,
    quiet: bool,
}

impl Checks {
    fn new(first: u32, quiet: bool) -> Self {
        Checks { n: core::cell::Cell::new(first), quiet }
    }

    fn ok(&self, cond: bool, got: u64) -> Result<(), u32> {
        self.n.set(self.n.get() + 1);
        if cond {
            return Ok(());
        }
        if !self.quiet {
            // One line, one WriteFile (WORKFLOW).
            let mut line = *b"presenter_test: check 00000000 got 00000000\r\n";
            line[22..30].copy_from_slice(&ntrt::hex32(self.n.get()));
            line[35..43].copy_from_slice(&ntrt::hex32(got as u32));
            out(&line);
        }
        Err(self.n.get())
    }

    fn eq(&self, got: u64, want: u64) -> Result<(), u32> {
        self.ok(got == want, got)
    }
}

fn last_error() -> u64 {
    // SAFETY: a plain call.
    u64::from(unsafe { GetLastError() })
}

fn w<const N: usize>(s: &str) -> [u16; N] {
    let mut b = [0u16; N];
    for (slot, c) in b.iter_mut().zip(s.encode_utf16()) {
        *slot = c;
    }
    b
}

/// Appends `v` in decimal to `line` at `*n`.
fn put_number(line: &mut [u16], n: &mut usize, v: usize) {
    let mut digits = [0u16; 20];
    let (mut k, mut v) = (0, v);
    loop {
        digits[k] = u16::from(b'0') + (v % 10) as u16;
        k += 1;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    for d in digits[..k].iter().rev() {
        line[*n] = *d;
        *n += 1;
    }
}

/// The numbers after `child` on the command line (its window's x, its
/// first paint, then the events ready, step, ack, quit and clicked), if
/// this is a child.
fn child_args() -> Option<[usize; 7]> {
    // SAFETY: the process's command line, NUL-terminated.
    let line = unsafe {
        let p = GetCommandLineW();
        core::slice::from_raw_parts(p, (0..).take_while(|&i| *p.add(i) != 0).count())
    };
    // After the quoted program name.
    let rest = line.iter().skip(1).position(|&c| c == u16::from(b'"')).map(|i| &line[i + 2..])?;
    let mut words = rest.split(|&c| c == u16::from(b' ')).filter(|w| !w.is_empty());
    if !words.next()?.iter().copied().eq("child".encode_utf16()) {
        return None;
    }
    let mut numbers = [0usize; 7];
    for slot in numbers.iter_mut() {
        *slot = words.next()?.iter().try_fold(0usize, |v, &d| {
            let d = d.checked_sub(u16::from(b'0')).filter(|&d| d < 10)?;
            Some(v * 10 + usize::from(d))
        })?;
    }
    Some(numbers)
}

/// The child's current paint (an index into PAINTS).
static PAINT: AtomicU32 = AtomicU32::new(0);
/// The clicks it got (lParam: client y << 16 | x) and whether one came
/// since it last said so.
static MARKS: [AtomicU32; 4] = [const { AtomicU32::new(u32::MAX) }; 4];
static CLICKED: AtomicU32 = AtomicU32::new(0);

/// The mark of the click at client (x, y).
fn mark_rect(at: u32) -> Rect {
    let (x, y) = ((at & 0xffff) as i32, (at >> 16) as i32);
    Rect { left: x - 6, top: y - 6, right: x + 6, bottom: y + 6 }
}

extern "system" fn window_proc(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize {
    if msg == WM_LBUTTONDOWN {
        if let Some(slot) = MARKS.iter().find(|m| m.load(Ordering::Relaxed) == u32::MAX) {
            slot.store(lparam as u32, Ordering::Relaxed);
            // SAFETY: our window and rectangle.
            unsafe { InvalidateRect(hwnd, &mark_rect(lparam as u32), 0) };
        }
        CLICKED.store(1, Ordering::Relaxed);
        return 0;
    }
    if msg == WM_PAINT {
        // SAFETY: plain calls with our paint structure.
        unsafe {
            let mut ps = PaintStruct {
                hdc: core::ptr::null_mut(),
                erase: 0,
                paint: Rect::default(),
                restore: 0,
                inc_update: 0,
                reserved: [0; 32],
            };
            let dc = BeginPaint(hwnd, &mut ps);
            let brush = CreateSolidBrush(PAINTS[PAINT.load(Ordering::Relaxed) as usize].0);
            FillRect(dc, &ps.paint, brush);
            DeleteObject(brush);
            let magenta = CreateSolidBrush(MARK);
            for m in &MARKS {
                let at = m.load(Ordering::Relaxed);
                if at != u32::MAX {
                    FillRect(dc, &mark_rect(at), magenta);
                }
            }
            DeleteObject(magenta);
            EndPaint(hwnd, &ps);
        }
        return 0;
    }
    // SAFETY: the arguments user32 gave us.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// Handles the messages waiting: the thread's idle moment, when user32
/// shows what was drawn (as a GUI thread does before it waits).
fn pump() {
    let mut msg = [0u64; 6];
    // SAFETY: our message buffer.
    unsafe {
        while PeekMessageW(&mut msg, core::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
            DispatchMessageW(&msg);
        }
    }
}

/// A child (in session 2, or 1): makes and paints its window at (x, 100),
/// then repaints in the next colour each time `step` is set (setting
/// `ack`) until `quit`, marking each click and setting `clicked` once it
/// is shown.
fn child([x, paint, ready, step, ack, quit, clicked]: [usize; 7]) -> u32 {
    PAINT.store(paint as u32, Ordering::Relaxed);
    let c = Checks::new(0x100, true);
    let run = || -> Result<(), u32> {
        let class = w::<32>("GalaPresenterTest");
        let null = core::ptr::null_mut();
        // SAFETY: plain calls with our structures, names and inherited
        // handles throughout.
        unsafe {
            let instance = GetModuleHandleW(core::ptr::null());
            let wc = WndClassExW {
                size: core::mem::size_of::<WndClassExW>() as u32,
                style: 0,
                proc_: window_proc,
                class_extra: 0,
                window_extra: 0,
                instance,
                icon: null,
                cursor: null,
                background: null,
                menu_name: core::ptr::null(),
                class_name: class.as_ptr(),
                icon_small: null,
            };
            c.ok(RegisterClassExW(&wc) != 0, last_error())?;
            let hwnd = CreateWindowExW(
                0,
                class.as_ptr(),
                class.as_ptr(),
                WS_OVERLAPPEDWINDOW,
                x as i32,
                100,
                400,
                300,
                null,
                null,
                instance,
                null,
            );
            c.ok(!hwnd.is_null(), last_error())?;
            ShowWindow(hwnd, SW_SHOW);
            UpdateWindow(hwnd);
            pump();
            c.ok(IsWindowVisible(hwnd) != 0, 0)?;
            // The desktop nothing shows has gala's default size (the
            // console's, UEFI's mode, is the same).
            c.eq(GetSystemMetrics(SM_CXSCREEN) as u64, 1280)?;
            c.eq(GetSystemMetrics(SM_CYSCREEN) as u64, 800)?;
            c.ok(SetEvent(ready as Handle) != 0, last_error())?;
            let waits = [step as Handle, quit as Handle];
            loop {
                // A give-up only: the parent waits for xtask's clicks.
                let woke = MsgWaitForMultipleObjects(2, waits.as_ptr(), 0, 300_000, QS_ALLINPUT);
                if woke == 1 {
                    break;
                }
                if woke == 2 {
                    // Input: handled, the mark shown, then said.
                    pump();
                    if CLICKED.swap(0, Ordering::Relaxed) != 0 {
                        c.ok(SetEvent(clicked as Handle) != 0, last_error())?;
                    }
                    continue;
                }
                c.eq(woke.into(), 0)?;
                let next = (PAINT.load(Ordering::Relaxed) + 1) % 3;
                PAINT.store(next, Ordering::Relaxed);
                c.ok(InvalidateRect(hwnd, core::ptr::null(), 0) != 0, last_error())?;
                UpdateWindow(hwnd);
                pump();
                c.ok(SetEvent(ack as Handle) != 0, last_error())?;
            }
        }
        // The window goes with the process.
        Ok(())
    };
    run().err().unwrap_or(0)
}

/// The Test frame's pixel at (x, y) and its flushes so far.
fn pixel(c: &Checks, x: u32, y: u32) -> Result<(u32, u64), u32> {
    let mut p = [0u32; 1];
    let (st, fw, fh, flushes) = session_frame(SESSION, [x, y, 1, 1], &mut p);
    c.eq(st.into(), STATUS_SUCCESS.into())?;
    c.eq(u64::from(fw) << 32 | u64::from(fh), 1280 << 32 | 800)?;
    Ok((p[0], flushes))
}

/// Whether some pixel of area (x, y, w, h) of the frame is within 0x40 of
/// `want` in each channel (anti-aliased and blended colours).
fn near_in(c: &Checks, area: [u32; 4], want: u32) -> Result<bool, u32> {
    let mut pixels = [0u32; 64 * 16];
    let n = (area[2] * area[3]) as usize;
    let (st, ..) = session_frame(SESSION, area, &mut pixels[..n]);
    c.eq(st.into(), STATUS_SUCCESS.into())?;
    let close = |a: u32, b: u32| {
        (0..3).all(|k| ((a >> (8 * k)) & 0xff).abs_diff((b >> (8 * k)) & 0xff) <= 0x40)
    };
    Ok(pixels[..n].iter().any(|&p| close(p, want)))
}

/// Asks the child for its next paint and waits until it is done.
fn repaint(c: &Checks, step: Handle, ack: Handle, process: Handle) -> Result<(), u32> {
    // SAFETY: our handles.
    unsafe {
        c.ok(SetEvent(step) != 0, last_error())?;
        let either = [ack, process];
        let woke = WaitForMultipleObjects(2, either.as_ptr(), 0, 30_000);
        let mut code = 0;
        GetExitCodeProcess(process, &mut code);
        c.ok(woke == 0, if woke == 1 { code.into() } else { woke.into() })
    }
}

fn present(kind: u64) -> NtStatus {
    gala_session(SESSION_PRESENT, SESSION, kind, 0, 0)
}

/// A left click at (x, y) of the desktop the Test frame shows.
fn inject_click(c: &Checks, x: u32, y: u32) -> Result<(), u32> {
    for buttons in [0, 1, 0] {
        c.eq(session_input(SESSION, [0, x, y, buttons]).into(), STATUS_SUCCESS.into())?;
    }
    Ok(())
}

/// The session the console shows, as WTSGetActiveConsoleSessionId and
/// KUSER_SHARED_DATA say (both, so they must agree).
fn console_id(c: &Checks) -> Result<u32, u32> {
    // SAFETY: kernel32's call; KUSER_SHARED_DATA is mapped in every
    // process.
    let (wts, kuser) = unsafe {
        (WTSGetActiveConsoleSessionId(), core::ptr::read_volatile(ACTIVE_CONSOLE_ID as *const u32))
    };
    c.eq(kuser.into(), wts.into())?;
    Ok(wts)
}

/// Waits for `event` (a child's), failing if `process` ends first; a
/// give-up only (xtask's screen check clicks within its own limits).
fn wait_for(c: &Checks, event: Handle, process: Handle, ms: u32) -> Result<(), u32> {
    // SAFETY: our handles.
    unsafe {
        let either = [event, process];
        let woke = WaitForMultipleObjects(2, either.as_ptr(), 0, ms);
        let mut code = 0;
        GetExitCodeProcess(process, &mut code);
        // 1: the child ended first (its exit code says why).
        c.ok(woke == 0, if woke == 1 { code.into() } else { woke.into() })
    }
}

/// A child's events, inheritable: ready (manual), step, ack, quit
/// (manual), clicked.
struct Events {
    ready: Handle,
    step: Handle,
    ack: Handle,
    quit: Handle,
    clicked: Handle,
}

impl Events {
    fn new(c: &Checks) -> Result<Events, u32> {
        let inherit =
            SecurityAttributes { length: 24, descriptor: core::ptr::null_mut(), inherit: 1 };
        // SAFETY: plain calls.
        let e = unsafe {
            Events {
                ready: CreateEventW(&inherit, 1, 0, core::ptr::null()),
                step: CreateEventW(&inherit, 0, 0, core::ptr::null()),
                ack: CreateEventW(&inherit, 0, 0, core::ptr::null()),
                quit: CreateEventW(&inherit, 1, 0, core::ptr::null()),
                clicked: CreateEventW(&inherit, 0, 0, core::ptr::null()),
            }
        };
        c.ok(!e.all().iter().any(|h| h.is_null()), last_error())?;
        Ok(e)
    }

    fn all(&self) -> [Handle; 5] {
        [self.ready, self.step, self.ack, self.quit, self.clicked]
    }
}

/// Starts this program as a child in `session` (SYSTEM's own token moved
/// there) with its window at (x, 100) painted PAINTS[paint], and waits
/// until it is shown: its process and thread handles.
fn spawn(c: &Checks, session: u32, x: u32, paint: u32, e: &Events) -> Result<[usize; 3], u32> {
    let null = core::ptr::null_mut();
    // SAFETY: plain calls with our buffers and handles throughout.
    unsafe {
        let (mut own, mut token) = (null, null);
        c.ok(OpenProcessToken(GetCurrentProcess(), TOKEN_ALL_ACCESS, &mut own) != 0, last_error())?;
        c.ok(
            DuplicateTokenEx(
                own,
                TOKEN_ALL_ACCESS,
                core::ptr::null(),
                SECURITY_IMPERSONATION,
                TOKEN_PRIMARY,
                &mut token,
            ) != 0,
            last_error(),
        )?;
        c.ok(
            SetTokenInformation(token, TOKEN_SESSION_ID, (&raw const session).cast(), 4) != 0,
            last_error(),
        )?;
        let mut exe = [0u16; 260];
        let n = GetModuleFileNameW(null, exe.as_mut_ptr(), 260) as usize;
        c.ok(n > 0 && n < 200, 0)?;
        let mut line = [0u16; 400];
        line[0] = u16::from(b'"');
        line[1..=n].copy_from_slice(&exe[..n]);
        let mut at = n + 1;
        for ch in "\" child".encode_utf16() {
            line[at] = ch;
            at += 1;
        }
        let mut numbers = [x as usize, paint as usize, 0, 0, 0, 0, 0];
        for (slot, h) in numbers[2..].iter_mut().zip(e.all()) {
            *slot = h as usize;
        }
        for v in numbers {
            line[at] = u16::from(b' ');
            at += 1;
            put_number(&mut line, &mut at, v);
        }
        // STARTUPINFOW: cb, then lpDesktop at 16.
        let desktop = w::<32>("winsta0\\default");
        let mut startup = [0usize; 13];
        startup[0] = 104;
        startup[2] = desktop.as_ptr() as usize;
        let mut info = [0usize; 3];
        let r = CreateProcessAsUserW(
            token,
            exe.as_ptr(),
            line.as_mut_ptr(),
            null,
            null,
            1,
            CREATE_NO_WINDOW,
            null,
            core::ptr::null(),
            startup.as_ptr().cast(),
            &mut info,
        );
        c.ok(r != 0, last_error())?;
        CloseHandle(token);
        CloseHandle(own);
        wait_for(c, e.ready, info[0] as Handle, 30_000)?;
        Ok(info)
    }
}

/// `gala attach <session>` (C:\gala\gala.exe, as this program's user):
/// its exit code.
fn gala_attach(c: &Checks, session: u32) -> Result<u32, u32> {
    let null = core::ptr::null_mut();
    let exe = w::<32>("C:\\gala\\gala.exe");
    let mut line = [0u16; 32];
    let mut at = 0;
    for ch in "gala.exe attach ".encode_utf16() {
        line[at] = ch;
        at += 1;
    }
    put_number(&mut line, &mut at, session as usize);
    let mut startup = [0usize; 13];
    startup[0] = 104;
    let mut info = [0usize; 3];
    // SAFETY: plain calls with our buffers and handles.
    unsafe {
        let r = CreateProcessW(
            exe.as_ptr(),
            line.as_mut_ptr(),
            null,
            null,
            1,
            0,
            null,
            core::ptr::null(),
            startup.as_ptr().cast(),
            &mut info,
        );
        c.ok(r != 0, last_error())?;
        c.eq(WaitForSingleObject(info[0] as Handle, 30_000).into(), 0)?;
        let mut code = 0;
        c.ok(GetExitCodeProcess(info[0] as Handle, &mut code) != 0, last_error())?;
        CloseHandle(info[0] as Handle);
        CloseHandle(info[1] as Handle);
        Ok(code)
    }
}

/// What shows `session`'s desktop and whether it is live with windows
/// (GalaSession's query): presenter << 1 | (live, windows >= 1).
fn shown_by(c: &Checks, session: u32) -> Result<u64, u32> {
    let (st, q) = session_query(session);
    c.eq(st.into(), STATUS_SUCCESS.into())?;
    Ok(u64::from(q[0]) << 1 | u64::from(q[1] == 1 && q[4] >= 1))
}

/// `shown_by`'s answer for a live desktop with windows shown by `kind`.
const fn kept(kind: u64) -> u64 {
    kind << 1 | 1
}

fn run() -> Result<bool, u32> {
    let c = Checks::new(0, false);
    // Not gala: skipped.
    let (st, _) = session_query(SESSION);
    if st == STATUS_INVALID_SYSTEM_SERVICE {
        return Ok(false);
    }
    c.eq(st.into(), STATUS_NOT_FOUND.into())?;
    let two = Events::new(&c)?;
    let info = spawn(&c, SESSION, 100, 0, &two)?;
    let process = info[0] as Handle;
    out(b"presenter_test: child painted\r\n");

    // Live, shown by nothing, the default size, its window.
    let (st, q) = session_query(SESSION);
    c.eq(st.into(), STATUS_SUCCESS.into())?;
    c.eq(q[0].into(), PRESENTER_NONE)?;
    c.eq(q[1].into(), 1)?;
    c.eq(u64::from(q[2]) << 32 | u64::from(q[3]), 1280 << 32 | 800)?;
    c.ok(q[4] >= 1, q[4].into())?;
    let mut p = [0u32; 1];
    c.eq(
        session_frame(SESSION, [0, 0, 1, 1], &mut p).0.into(),
        STATUS_INVALID_DEVICE_STATE.into(),
    )?;
    // Nothing shows it: no input either.
    c.eq(
        session_input(SESSION, [0, INJECTED.0, INJECTED.1, 0]).into(),
        STATUS_INVALID_DEVICE_STATE.into(),
    )?;

    // A Test presenter: the window as the console would show it.
    c.eq(present(PRESENTER_TEST).into(), STATUS_SUCCESS.into())?;
    let (st, q) = session_query(SESSION);
    c.eq(st.into(), STATUS_SUCCESS.into())?;
    c.eq(q[0].into(), PRESENTER_TEST)?;
    let (red, first_flushes) = pixel(&c, CLIENT.0 + 50, CLIENT.1 + 50)?;
    c.eq(red.into(), PAINTS[0].1.into())?;
    c.ok(first_flushes >= 1, first_flushes)?;
    c.eq(pixel(&c, 300, 110)?.0.into(), CAPTION_ACTIVE.into())?;
    c.ok(near_in(&c, [4, 4, 12, 12], 0x060e2b)?, 0)?; // the night wallpaper
    c.ok(near_in(&c, [880, 780, 40, 12], 0xe0e7f3)?, 0)?; // the taskbar
    out(b"presenter_test: frame shows the window\r\n");

    // A repaint reaches the frame.
    repaint(&c, two.step, two.ack, process)?;
    let (green, flushes) = pixel(&c, CLIENT.0 + 50, CLIENT.1 + 50)?;
    c.eq(green.into(), PAINTS[1].1.into())?;
    c.ok(flushes > first_flushes, flushes)?;

    // Input through the Test presenter: a click reaches the child, whose
    // mark the frame shows (up and left of the point: the pointer is
    // drawn down and right of it).
    inject_click(&c, INJECTED.0, INJECTED.1)?;
    wait_for(&c, two.clicked, process, 30_000)?;
    c.eq(pixel(&c, INJECTED.0 - 4, INJECTED.1 - 4)?.0.into(), MARK.into())?;
    out(b"presenter_test: injected click reached the window\r\n");

    // Detached: no frame; the window keeps what it paints meanwhile.
    c.eq(present(PRESENTER_NONE).into(), STATUS_SUCCESS.into())?;
    c.eq(
        session_frame(SESSION, [0, 0, 1, 1], &mut p).0.into(),
        STATUS_INVALID_DEVICE_STATE.into(),
    )?;
    repaint(&c, two.step, two.ack, process)?;
    let (st, q) = session_query(SESSION);
    c.eq(st.into(), STATUS_SUCCESS.into())?;
    c.eq(u64::from(q[0]) << 32 | u64::from(q[1]), PRESENTER_NONE << 32 | 1)?;
    c.eq(present(PRESENTER_TEST).into(), STATUS_SUCCESS.into())?;
    c.eq(pixel(&c, CLIENT.0 + 50, CLIENT.1 + 50)?.0.into(), PAINTS[2].1.into())?;
    c.eq(pixel(&c, INJECTED.0 - 4, INJECTED.1 - 4)?.0.into(), MARK.into())?;
    out(b"presenter_test: kept while not shown\r\n");

    // Bad requests.
    c.eq(
        session_frame(SESSION, [1280, 0, 1, 1], &mut p).0.into(),
        STATUS_INVALID_PARAMETER.into(),
    )?;
    c.eq(present(7).into(), STATUS_INVALID_PARAMETER.into())?;
    c.eq(session_input(SESSION, [7, 0, 0, 0]).into(), STATUS_INVALID_PARAMETER.into())?;
    c.eq(
        gala_session(SESSION_PRESENT, 0, PRESENTER_CONSOLE, 0, 0).into(),
        STATUS_NOT_SUPPORTED.into(),
    )?;
    c.eq(console_id(&c)?.into(), CONSOLE.into())?;

    // The console switch. Session 1's own window on the console first.
    let one = Events::new(&c)?;
    let info_1 = spawn(&c, CONSOLE, CLIENT_1.0 - 1, YELLOW, &one)?;
    let process_1 = info_1[0] as Handle;
    c.eq(shown_by(&c, CONSOLE)?, kept(PRESENTER_CONSOLE))?;
    out(b"presenter_test: session 1 shows its window\r\n");
    // xtask checks the screen, then clicks the window (PS/2).
    wait_for(&c, one.clicked, process_1, 180_000)?;
    out(b"presenter_test: a click on the console reached session 1\r\n");

    // `gala attach 2`: session 2 on the console, session 1 kept.
    c.eq(gala_attach(&c, SESSION)?.into(), 0)?;
    c.eq(shown_by(&c, SESSION)?, kept(PRESENTER_CONSOLE))?;
    c.eq(shown_by(&c, CONSOLE)?, kept(PRESENTER_NONE))?;
    c.eq(console_id(&c)?.into(), SESSION.into())?;
    // No Test frame shows it now: no frame, no injected input.
    c.eq(
        session_frame(SESSION, [0, 0, 1, 1], &mut p).0.into(),
        STATUS_INVALID_DEVICE_STATE.into(),
    )?;
    out(b"presenter_test: the console shows session 2\r\n");
    // xtask checks the screen and clicks session 2's window: its PS/2
    // input follows the console. A second click says xtask is done.
    wait_for(&c, two.clicked, process, 180_000)?;
    out(b"presenter_test: a click on the console reached session 2\r\n");
    wait_for(&c, two.clicked, process, 180_000)?;

    // Back to session 1 (GalaSession itself): its window was kept.
    c.eq(
        gala_session(SESSION_PRESENT, CONSOLE, PRESENTER_CONSOLE, 0, 0).into(),
        STATUS_SUCCESS.into(),
    )?;
    c.eq(shown_by(&c, CONSOLE)?, kept(PRESENTER_CONSOLE))?;
    c.eq(shown_by(&c, SESSION)?, kept(PRESENTER_NONE))?;
    c.eq(console_id(&c)?.into(), CONSOLE.into())?;
    out(b"presenter_test: the console shows session 1 again\r\n");
    wait_for(&c, one.clicked, process_1, 180_000)?;
    out(b"presenter_test: a click reached session 1 again\r\n");

    // The children end: session 2's window goes.
    // SAFETY: our handles.
    unsafe {
        let mut code = 0;
        for (e, h) in [(&one, process_1), (&two, process)] {
            c.ok(SetEvent(e.quit) != 0, last_error())?;
            c.eq(WaitForSingleObject(h, 30_000).into(), 0)?;
            c.ok(GetExitCodeProcess(h, &mut code) != 0, last_error())?;
            c.eq(code.into(), 0)?;
        }
        c.eq(present(PRESENTER_TEST).into(), STATUS_SUCCESS.into())?;
        c.ok(pixel(&c, CLIENT.0 + 50, CLIENT.1 + 50)?.0 != PAINTS[2].1, 0)?;
        c.eq(present(PRESENTER_NONE).into(), STATUS_SUCCESS.into())?;
        for h in [info[0], info[1], info_1[0], info_1[1]] {
            CloseHandle(h as Handle);
        }
        for h in two.all().iter().chain(one.all().iter()) {
            CloseHandle(*h);
        }
    }
    Ok(true)
}

fn main() -> u32 {
    if let Some(args) = child_args() {
        return child(args);
    }
    match run() {
        Ok(true) => {
            out(b"presenter_test: ok\r\n");
            0
        }
        Ok(false) => {
            out(b"presenter_test: skipped (GalaSession is gala's own)\r\n");
            0
        }
        Err(n) => n,
    }
}

ntrt::entry!(main);

//! Linux windows as a taskbar sees them (ROADMAP SH step 1, part 2b): a
//! Wayland client's window (weston-simple-shm, started with GalaLxSpawn)
//! is a window like a Windows program's for the window calls and the
//! shell's. Expected results are Windows' as Microsoft documents them for
//! a top-level window of another process:
//!
//! - GalaDesktopAction's window list names it by an HWND (not 0) that
//!   FindWindowW finds by its title; IsWindow, GetWindowTextW and
//!   GetWindowThreadProcessId (the Linux process's id) answer for it.
//! - Shown, it is the foreground window: EVENT_SYSTEM_FOREGROUND for it
//!   (OBJID_WINDOW, CHILDID_SELF).
//! - Minimized by the shell: EVENT_SYSTEM_MINIMIZESTART, IsIconic.
//! - Activated again (its taskbar button): EVENT_SYSTEM_MINIMIZEEND and
//!   EVENT_SYSTEM_FOREGROUND, IsIconic no longer.
//! - Closed by the shell: the client ends, and its window is gone
//!   (IsWindow 0, not listed).
//!
//! Prints `linux_hwnd_test: ok`, or `linux_hwnd_test: check N got X
//! (line L)` and exits with N.

#![no_std]
#![no_main]

use core::sync::atomic::{AtomicUsize, Ordering};

use ntrt as _;

const NAME: &str = "linux_hwnd_test";

type Handle = usize;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, overlapped: usize) -> i32;
    fn ExitProcess(code: u32) -> !;
    fn Sleep(ms: u32);
    fn GetTickCount64() -> u64;
    fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        sa: usize,
        disposition: u32,
        flags: u32,
        template: Handle,
    ) -> Handle;
    fn GetProcessId(process: Handle) -> u32;
    fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
    fn CloseHandle(h: Handle) -> i32;
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
    fn GalaLxSpawn(record: *const u8, length: usize, process: *mut Handle) -> u32;
}

#[link(name = "user32", kind = "raw-dylib")]
unsafe extern "system" {
    fn PeekMessageW(msg: *mut Msg, hwnd: Handle, min: u32, max: u32, remove: u32) -> i32;
    fn TranslateMessage(msg: *const Msg) -> i32;
    fn DispatchMessageW(msg: *const Msg) -> isize;
    fn IsIconic(hwnd: Handle) -> i32;
    fn IsWindow(hwnd: Handle) -> i32;
    fn FindWindowW(class: *const u16, title: *const u16) -> Handle;
    fn GetWindowTextW(hwnd: Handle, buf: *mut u16, n: i32) -> i32;
    fn GetWindowThreadProcessId(hwnd: Handle, pid: *mut u32) -> u32;
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
struct Msg {
    hwnd: Handle,
    message: u32,
    wparam: usize,
    lparam: isize,
    time: u32,
    pt: [i32; 2],
    private: u32,
}

const EVENT_SYSTEM_FOREGROUND: u32 = 0x0003;
const EVENT_SYSTEM_MINIMIZESTART: u32 = 0x0016;
const EVENT_SYSTEM_MINIMIZEEND: u32 = 0x0017;
const OBJID_WINDOW: i32 = 0;
const CHILDID_SELF: i32 = 0;
const WINEVENT_OUTOFCONTEXT: u32 = 0;

// GalaDesktopAction (kernel/src/nt/action.rs): version 1, the action,
// the window's HWND, four numbers, the text's length, zero. The window
// list: version and count (u32 each), then per window its HWND (u64),
// owner, id and flags (u32), rectangle (4 x i32), title length (u32) and
// title (128 UTF-16 units).
const ACTION_WINDOWS: u32 = 2;
const ACTION_ACTIVATE: u32 = 3;
const ACTION_MINIMIZE: u32 = 4;
const ACTION_CLOSE: u32 = 7;
const REQUEST_HEADER: usize = 40;
const LIST_HEADER: usize = 8;
const ENTRY: usize = 8 + 4 * 3 + 16 + 4 + 256;

/// GalaLxSpawn's root: FILE_TRAVERSE | SYNCHRONIZE (and listing).
const DIR_ACCESS: u32 = 0x1 | 0x20 | 0x0010_0000;
const FILE_SHARE_ALL: u32 = 7;
const OPEN_EXISTING: u32 = 3;
const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
const TITLE: &str = "simple-shm";

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

// ---- The events -----------------------------------------------------------

/// The events the hook got, in order: (event, hwnd); the last event whose
/// object and child were not (OBJID_WINDOW, CHILDID_SELF).
static EVENTS: [AtomicUsize; 64] = [const { AtomicUsize::new(0) }; 64];
static EVENT_HWNDS: [AtomicUsize; 64] = [const { AtomicUsize::new(0) }; 64];
static N_EVENTS: AtomicUsize = AtomicUsize::new(0);
static ODD_OBJECT: AtomicUsize = AtomicUsize::new(0);

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

/// Dispatches the waiting messages (the hook's events) for up to `ms`, or
/// until `done`.
fn pump(ms: u64, mut done: impl FnMut() -> bool) -> bool {
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
            Sleep(20);
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
}

/// The shell's action `what` on window `hwnd` (as a taskbar asks it);
/// its status.
fn action(what: u32, hwnd: Handle, out: &mut [u8], returned: &mut u32) -> u32 {
    let mut b = [0u8; REQUEST_HEADER];
    b[0..4].copy_from_slice(&1u32.to_le_bytes());
    b[4..8].copy_from_slice(&what.to_le_bytes());
    b[8..16].copy_from_slice(&(hwnd as u64).to_le_bytes());
    // SAFETY: our buffers, with their lengths.
    unsafe {
        GalaDesktopAction(b.as_ptr(), b.len() as u32, out.as_mut_ptr(), out.len() as u32, returned)
    }
}

fn act(what: u32, hwnd: Handle) -> u32 {
    let mut none = [0u8; 0];
    action(what, hwnd, &mut none, &mut 0)
}

/// The listed window titled TITLE: (its HWND, its owner's pid), or None.
fn listed() -> Option<(Handle, u32)> {
    let mut out = [0u8; LIST_HEADER + 32 * ENTRY];
    let mut returned = 0;
    if action(ACTION_WINDOWS, 0, &mut out, &mut returned) != 0 {
        return None;
    }
    let u32_at = |o: usize| u32::from_le_bytes([out[o], out[o + 1], out[o + 2], out[o + 3]]);
    let count = (u32_at(4) as usize).min(32);
    (0..count).find_map(|i| {
        let e = LIST_HEADER + i * ENTRY;
        let len = (u32_at(e + 36) as usize).min(128);
        let title =
            (0..len).map(|k| u16::from_le_bytes([out[e + 40 + 2 * k], out[e + 41 + 2 * k]]));
        if !title.eq(TITLE.encode_utf16()) {
            return None;
        }
        let mut h = [0u8; 8];
        h.copy_from_slice(&out[e..e + 8]);
        Some((u64::from_le_bytes(h) as Handle, u32_at(e + 8)))
    })
}

fn put64(r: &mut [u8], at: usize, v: u64) {
    r[at..at + 8].copy_from_slice(&v.to_le_bytes());
}

/// weston-simple-shm on the Linux personality, rooted at C:\ (where the
/// image's /lx and the Wayland socket are), its standard handles ours:
/// its process handle.
fn spawn(c: &mut Checks) -> Result<Handle, u32> {
    let root = W::new("\\\\?\\C:\\");
    // SAFETY: a valid path; plain flags.
    let dir = unsafe {
        CreateFileW(
            root.ptr(),
            DIR_ACCESS,
            FILE_SHARE_ALL,
            0,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            0,
        )
    };
    c.ok(dir != usize::MAX, 0)?;
    let path = b"/lx/weston-simple-shm";
    let argv = b"./weston-simple-shm\0";
    let envp = b"XDG_RUNTIME_DIR=/run/user/1000\0WAYLAND_DISPLAY=wayland-0\0";
    let cwd = b"/lx";
    let name = b"linux_hwnd_test";
    let mut r = [0u8; 120];
    r[0..4].copy_from_slice(&1u32.to_le_bytes());
    put64(&mut r, 8, dir as u64);
    for i in 0..3 {
        // SAFETY: a plain call.
        let h = unsafe { GetStdHandle((-10 - i as i32) as u32) };
        put64(&mut r, 16 + 8 * i, h as u64);
    }
    for (i, f) in [&path[..], &argv[..], &envp[..], &cwd[..], &name[..]].into_iter().enumerate() {
        put64(&mut r, 40 + 16 * i, f.as_ptr() as u64);
        put64(&mut r, 48 + 16 * i, f.len() as u64);
    }
    let mut process = 0;
    // SAFETY: the record and every buffer it points to live across the call.
    let s = unsafe { GalaLxSpawn(r.as_ptr(), r.len(), &mut process) };
    // SAFETY: our handle.
    unsafe { CloseHandle(dir) };
    c.eq(i64::from(s), 0)?;
    Ok(process)
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    // SAFETY: plain calls on our hook and the Linux client's window.
    unsafe {
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
        let process = spawn(&mut c)?;
        let pid = GetProcessId(process);
        c.ok(pid != 0, 0)?;

        // Its window is listed by an HWND, and it is the foreground one.
        let mut found = None;
        let shown = pump(20000, || {
            found = listed();
            found.is_some_and(|(hwnd, _)| hwnd != 0 && got(EVENT_SYSTEM_FOREGROUND, hwnd))
        });
        let (hwnd, owner) = found.unwrap_or((0, 0));
        c.ok(found.is_some(), 0)?;
        c.ok(hwnd != 0, 0)?;
        c.ok(shown, N_EVENTS.load(Ordering::Relaxed) as i64)?;
        c.eq(i64::from(owner), i64::from(pid))?;
        c.eq(FindWindowW(core::ptr::null(), W::new(TITLE).ptr()) as i64, hwnd as i64)?;
        c.eq(i64::from(IsWindow(hwnd)), 1)?;
        let mut text = [0u16; 64];
        let n = GetWindowTextW(hwnd, text.as_mut_ptr(), text.len() as i32);
        c.ok(text[..n.max(0) as usize].iter().copied().eq(TITLE.encode_utf16()), i64::from(n))?;
        let mut of = 0u32;
        GetWindowThreadProcessId(hwnd, &mut of);
        c.eq(i64::from(of), i64::from(pid))?;
        c.eq(i64::from(IsIconic(hwnd)), 0)?;
        say(b"listed with its hwnd");

        // Minimized by the shell.
        clear();
        c.eq(i64::from(act(ACTION_MINIMIZE, hwnd)), 0)?;
        let done = || got(EVENT_SYSTEM_MINIMIZESTART, hwnd) && IsIconic(hwnd) != 0;
        c.ok(pump(5000, done), N_EVENTS.load(Ordering::Relaxed) as i64)?;
        say(b"minimized by the shell");

        // Its taskbar button: restored and the foreground window again.
        clear();
        c.eq(i64::from(act(ACTION_ACTIVATE, hwnd)), 0)?;
        let done = || {
            got(EVENT_SYSTEM_MINIMIZEEND, hwnd)
                && got(EVENT_SYSTEM_FOREGROUND, hwnd)
                && IsIconic(hwnd) == 0
        };
        c.ok(pump(5000, done), N_EVENTS.load(Ordering::Relaxed) as i64)?;
        say(b"restored by the shell");
        c.eq(ODD_OBJECT.load(Ordering::Relaxed) as i64, 0)?;

        // Closed by the shell: the client ends, its window is gone.
        c.eq(i64::from(act(ACTION_CLOSE, hwnd)), 0)?;
        c.eq(i64::from(WaitForSingleObject(process, 20000)), 0)?;
        c.ok(pump(5000, || IsWindow(hwnd) == 0 && listed().is_none()), 0)?;
        say(b"closed by the shell");

        CloseHandle(process);
        UnhookWinEvent(h);
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

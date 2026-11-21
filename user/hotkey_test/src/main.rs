//! Hot keys (ROADMAP SH step 1, parts 3 and 11): RegisterHotKey and
//! UnregisterHotKey as Microsoft documents them, and the shell's keys,
//! which only the shell holds. It runs as a program of a session whose
//! shell is shasha.exe, as a Windows program runs beside Explorer.
//! Expected results are Windows':
//!
//! - RegisterHotKey(NULL, id, MOD_CONTROL | MOD_ALT | MOD_NOREPEAT, 'K')
//!   succeeds; the same keys again (whatever MOD_NOREPEAT) fail with
//!   ERROR_HOTKEY_ALREADY_REGISTERED; UnregisterHotKey of an id never
//!   registered fails with ERROR_HOTKEY_NOT_REGISTERED.
//! - Pressing the keys posts WM_HOTKEY to the registering thread (hwnd
//!   NULL): wParam the id, lParam MAKELONG(the modifiers without
//!   MOD_NOREPEAT, the virtual key).
//! - The shell's window is another process's (GetShellWindow: the
//!   shell's desktop window, class `Progman`), and SetShellWindow, once
//!   per desktop, fails for this one.
//! - Win+E is the shell's (Explorer holds it), and so is the Windows key
//!   alone ((MOD_WIN, VK_LWIN): gala's way of offering it); Win+Left is
//!   Snap's: this program's RegisterHotKey fails with
//!   ERROR_HOTKEY_ALREADY_REGISTERED for each.
//! - Pressed, Win+E opens a File Explorer window (`CabinetWClass`) and
//!   the Windows key the shell's start menu (its "Start" window shown);
//!   neither reaches this program.
//!
//! The shell is ready, its keys taken, once its desktop window shows
//! (shasha shows it last). The scenario presses the keys after
//! `registered`, `shell keys` and `file explorer`. Prints
//! `hotkey_test: ok`, or `hotkey_test: check N got X (line L)` and
//! exits with N.

#![no_std]
#![no_main]

use core::sync::atomic::{AtomicUsize, Ordering};

use ntrt as _;

const NAME: &str = "hotkey_test";

type Handle = usize;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, overlapped: usize) -> i32;
    fn ExitProcess(code: u32) -> !;
    fn GetModuleHandleW(name: *const u16) -> Handle;
    fn GetLastError() -> u32;
    fn SetLastError(e: u32);
    fn Sleep(ms: u32);
    fn GetTickCount64() -> u64;
    fn GetCurrentProcessId() -> u32;
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
    fn RegisterHotKey(hwnd: Handle, id: i32, modifiers: u32, vk: u32) -> i32;
    fn UnregisterHotKey(hwnd: Handle, id: i32) -> i32;
    fn SetShellWindow(hwnd: Handle) -> i32;
    fn GetShellWindow() -> Handle;
    fn IsWindowVisible(hwnd: Handle) -> i32;
    fn GetWindowThreadProcessId(hwnd: Handle, pid: *mut u32) -> u32;
    fn GetClassNameW(hwnd: Handle, buf: *mut u16, n: i32) -> i32;
    fn FindWindowW(class: *const u16, title: *const u16) -> Handle;
    fn PostMessageW(hwnd: Handle, msg: u32, wparam: usize, lparam: isize) -> i32;
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
const WM_HOTKEY: u32 = 0x0312;
const WM_CLOSE: u32 = 0x0010;
const MOD_ALT: u32 = 0x1;
const MOD_CONTROL: u32 = 0x2;
const MOD_WIN: u32 = 0x8;
const MOD_NOREPEAT: u32 = 0x4000;
const VK_LWIN: u32 = 0x5b;
const VK_LEFT: u32 = 0x25;
const ERROR_HOTKEY_ALREADY_REGISTERED: u32 = 1409;
const ERROR_HOTKEY_NOT_REGISTERED: u32 = 1419;

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

// ---- WM_HOTKEY ------------------------------------------------------------

/// The last WM_HOTKEY: its window (+1, so a thread message's 0 is 1),
/// id and lParam; 0 when none since cleared.
static HOT_HWND: AtomicUsize = AtomicUsize::new(0);
static HOT_ID: AtomicUsize = AtomicUsize::new(0);
static HOT_LPARAM: AtomicUsize = AtomicUsize::new(0);

fn note_hot_key(hwnd: Handle, wparam: usize, lparam: isize) {
    HOT_ID.store(wparam, Ordering::Relaxed);
    HOT_LPARAM.store(lparam as usize, Ordering::Relaxed);
    HOT_HWND.store(hwnd + 1, Ordering::Relaxed);
}

extern "system" fn proc_(hwnd: Handle, msg: u32, wparam: usize, lparam: isize) -> isize {
    if msg == WM_HOTKEY {
        note_hot_key(hwnd, wparam, lparam);
        return 0;
    }
    // SAFETY: the default handling.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// Dispatches messages (a thread's WM_HOTKEY noted here, as it has no
/// window procedure) until a WM_HOTKEY or `ms` pass: whether one came.
fn wait_hot_key(ms: u64) -> bool {
    // SAFETY: our message structure.
    unsafe {
        let end = GetTickCount64() + ms;
        loop {
            let mut m: Msg = core::mem::zeroed();
            while PeekMessageW(&mut m, 0, 0, 0, 1) != 0 {
                if m.hwnd == 0 && m.message == WM_HOTKEY {
                    note_hot_key(0, m.wparam, m.lparam);
                    continue;
                }
                TranslateMessage(&m);
                DispatchMessageW(&m);
            }
            if HOT_HWND.load(Ordering::Relaxed) != 0 {
                return true;
            }
            if GetTickCount64() >= end {
                return false;
            }
            Sleep(10);
        }
    }
}

fn clear() {
    HOT_HWND.store(0, Ordering::Relaxed);
}

fn make_lparam(modifiers: u32, vk: u32) -> i64 {
    i64::from(vk << 16 | modifiers)
}

/// RegisterHotKey that must fail with `error`.
fn refused(
    c: &mut Checks,
    hwnd: Handle,
    id: i32,
    modifiers: u32,
    vk: u32,
    error: u32,
) -> Result<(), u32> {
    // SAFETY: plain calls.
    unsafe {
        SetLastError(0);
        c.eq(i64::from(RegisterHotKey(hwnd, id, modifiers, vk)), 0)?;
        c.eq(i64::from(GetLastError()), i64::from(error))
    }
}

/// Dispatches messages until `cond` holds or `ms` pass: whether it held.
fn wait_for(ms: u64, mut cond: impl FnMut() -> bool) -> bool {
    // SAFETY: plain calls.
    let end = unsafe { GetTickCount64() } + ms;
    loop {
        wait_hot_key(0);
        if cond() {
            return true;
        }
        // SAFETY: plain calls.
        if unsafe { GetTickCount64() } >= end {
            return false;
        }
        // SAFETY: plain call.
        unsafe { Sleep(10) };
    }
}

/// Window `hwnd`'s process.
fn pid_of(hwnd: Handle) -> u32 {
    let mut pid = 0;
    // SAFETY: our DWORD.
    unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
    pid
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    let class = W::new("HotKeyTest");
    // SAFETY: plain calls on our class and window.
    unsafe {
        // The shell: another process's desktop window, shown once its
        // keys are taken.
        let ready = wait_for(600_000, || {
            let s = GetShellWindow();
            s != 0 && IsWindowVisible(s) != 0
        });
        c.ok(ready, 0)?;
        let shell = GetShellWindow();
        let shell_pid = pid_of(shell);
        c.ok(shell_pid != 0 && shell_pid != GetCurrentProcessId(), i64::from(shell_pid))?;
        let mut name = [0u16; 16];
        let n = GetClassNameW(shell, name.as_mut_ptr(), 16);
        c.ok(name[..n.clamp(0, 16) as usize] == W::new("Progman").0[..7], i64::from(n))?;

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
        let w = CreateWindowExW(
            0,
            class.ptr(),
            W::new("hotkey_test").ptr(),
            WS_OVERLAPPEDWINDOW,
            200,
            150,
            500,
            300,
            0,
            0,
            GetModuleHandleW(core::ptr::null()),
            0,
        );
        c.ok(w != 0, 0)?;
        ShowWindow(w, SW_SHOW);

        // A program's own hot key, for its thread.
        c.eq(
            i64::from(RegisterHotKey(0, 1, MOD_CONTROL | MOD_ALT | MOD_NOREPEAT, u32::from(b'K'))),
            1,
        )?;
        refused(
            &mut c,
            0,
            2,
            MOD_CONTROL | MOD_ALT,
            u32::from(b'K'),
            ERROR_HOTKEY_ALREADY_REGISTERED,
        )?;
        SetLastError(0);
        c.eq(i64::from(UnregisterHotKey(0, 99)), 0)?;
        c.eq(i64::from(GetLastError()), i64::from(ERROR_HOTKEY_NOT_REGISTERED))?;
        // The shell's window is once per desktop: not this program's.
        c.eq(i64::from(SetShellWindow(w)), 0)?;
        c.eq(GetShellWindow() as i64, shell as i64)?;
        // The shell's keys and Snap's are not a program's to take.
        refused(&mut c, w, 3, MOD_WIN, u32::from(b'E'), ERROR_HOTKEY_ALREADY_REGISTERED)?;
        refused(&mut c, w, 4, MOD_WIN, VK_LWIN, ERROR_HOTKEY_ALREADY_REGISTERED)?;
        refused(&mut c, w, 5, MOD_WIN, VK_LEFT, ERROR_HOTKEY_ALREADY_REGISTERED)?;
        clear();
        say(b"registered");
        c.ok(wait_hot_key(60_000), 0)?;
        c.eq(HOT_HWND.load(Ordering::Relaxed) as i64, 1)?;
        c.eq(HOT_ID.load(Ordering::Relaxed) as i64, 1)?;
        c.eq(
            HOT_LPARAM.load(Ordering::Relaxed) as i64,
            make_lparam(MOD_CONTROL | MOD_ALT, u32::from(b'K')),
        )?;
        say(b"hot key 1");
        c.eq(i64::from(UnregisterHotKey(0, 1)), 1)?;

        // The shell's keys do what the shell does with them: Win+E a
        // File Explorer window ...
        clear();
        say(b"shell keys");
        let cabinet = W::new("CabinetWClass");
        let explorer = || FindWindowW(cabinet.ptr(), core::ptr::null());
        c.ok(wait_for(600_000, || explorer() != 0), 0)?;
        let e = explorer();
        c.ok(pid_of(e) != GetCurrentProcessId(), i64::from(pid_of(e)))?;
        c.eq(HOT_HWND.load(Ordering::Relaxed) as i64, 0)?;
        c.ok(PostMessageW(e, WM_CLOSE, 0, 0) != 0, 0)?;
        c.ok(wait_for(600_000, || explorer() == 0), 0)?;
        say(b"file explorer");
        // ... and the Windows key the shell's start menu.
        let start = W::new("Start");
        let menu = || {
            let m = FindWindowW(core::ptr::null(), start.ptr());
            m != 0 && pid_of(m) == shell_pid && IsWindowVisible(m) != 0
        };
        c.ok(wait_for(600_000, menu), 0)?;
        c.eq(HOT_HWND.load(Ordering::Relaxed) as i64, 0)?;
        say(b"start menu");
        c.ok(DestroyWindow(w) != 0, 0)?;
        // The shell's window is the shell's still.
        c.eq(GetShellWindow() as i64, shell as i64)?;
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

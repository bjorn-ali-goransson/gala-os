//! The start menu's Linux section (ROADMAP LD step 7, SH step 1 part
//! 11): shasha's start menu suggests distributions, installs one from its
//! tile and starts its desktop session, a terminal and one of its
//! applications. Driven by xtask's screen check (scenario `linux_menu`,
//! UEFI), which clicks what this program finds through UI Automation and
//! announces (`click_on linux_menu_test: <element>`). This program follows
//! what the clicks do through the menu's elements, the desktop's window
//! list (GalaDesktopAction ACTION_WINDOWS), `gala ps`, and what they leave
//! where Windows keeps it: the installed distribution in WSL's registry
//! (Lxss) and its directory. gala's own shell, so there is no Windows run
//! to compare with; on Windows it says so and passes.
//!
//! - Before: no start menu choices saved, no "Ubuntu" or "Alpine"
//!   registered, `gala programs` answers.
//! - The menu (the taskbar's Start button) has the Linux section after
//!   the programs: a "Linux" heading, Ubuntu's tile (`LinuxUbuntu`, a
//!   Button named "Ubuntu") and Alpine's, each with a Text
//!   `Linux<name>State` reading "Not installed".
//! - A click on Ubuntu's installs it and the menu stays open: its state
//!   reads "Downloading N%" with N above 0 and it has its bar
//!   (`LinuxUbuntuBar`) while the download runs (the host holds the rest
//!   of the file until the screen check has seen that: HELD_DOWNLOADS).
//! - Installed (fetched from the host, imported by `wsl --import`):
//!   registered (State 1, Version 2) with BasePath
//!   %LOCALAPPDATA%\Linux\Ubuntu, whose rootfs has the desktop session;
//!   the downloaded tarball is gone. The tile reads "Installed" without a
//!   bar and is followed by its terminal's (`LinuxTerminalUbuntu`,
//!   "Terminal (Ubuntu)") and its applications' (`LinuxAppUbuntu_<i>`),
//!   XTerm's among them ("XTerm (Ubuntu)", from its .desktop file).
//! - A click on Ubuntu's tile starts its desktop session: the window
//!   "Xwayland on :1" shows and the menu hides. Once the screen check has
//!   typed `exit` into the session's xterm, wsl.exe ends and the window
//!   goes.
//! - The terminal's tile: a console window "wsl.exe" and the menu hidden;
//!   once `exit` was typed there, wsl.exe ends (the console's window
//!   stays, as conhost's does).
//! - XTerm's tile: its window "xterm" and the menu hidden.
//!
//! Assumed and not pinned: the AutomationIds, the button names and the
//! state's words are gala's own choice (shasha's menu, `shasha::linux`).
//!
//! Prints `linux_menu_test: listed`, `Linux section shown`, `downloading
//! shown`, `Ubuntu installed`, `desktop session shown`, `desktop session
//! ended`, `menu open again`, `terminal ended`, `app tile shown`,
//! `application shown` and `ok` (each after `linux_menu_test: `), or
//! `linux_menu_test: check N got X (line L)` and exits with N.

#![no_std]
#![no_main]

extern crate alloc;

use gala_heap as _;
use ntrt as _;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::ffi::c_void;

use uia_client::{BUTTON, Element, TEXT, Uia};

const NAME: &str = "linux_menu_test";

type Bool = i32;
type Handle = *mut c_void;
type Hkey = usize;

#[repr(C)]
struct SecurityAttributes {
    length: u32,
    descriptor: *mut c_void,
    inherit: Bool,
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetModuleHandleA(name: *const u8) -> Handle;
    fn GetProcAddress(module: Handle, name: *const u8) -> *mut c_void;
    fn GetLastError() -> u32;
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut c_void) -> Bool;
    fn ReadFile(h: Handle, buf: *mut u8, len: u32, read: *mut u32, ov: *mut c_void) -> Bool;
    fn ExitProcess(code: u32) -> !;
    fn CloseHandle(h: Handle) -> Bool;
    fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
    fn GetExitCodeProcess(h: Handle, code: *mut u32) -> Bool;
    fn CreatePipe(
        read: *mut Handle,
        write: *mut Handle,
        attributes: *const SecurityAttributes,
        size: u32,
    ) -> Bool;
    fn SetHandleInformation(h: Handle, mask: u32, flags: u32) -> Bool;
    fn CreateProcessW(
        app: *const u16,
        command_line: *mut u16,
        process_attributes: *const c_void,
        thread_attributes: *const c_void,
        inherit: Bool,
        flags: u32,
        environment: *const c_void,
        directory: *const u16,
        startup: *const u8,
        info: *mut [usize; 3],
    ) -> Bool;
    fn GetEnvironmentVariableW(name: *const u16, buf: *mut u16, size: u32) -> u32;
    fn GetFileAttributesW(name: *const u16) -> u32;
    fn Sleep(ms: u32);
    fn GetTickCount64() -> u64;
}

#[link(name = "user32", kind = "raw-dylib")]
unsafe extern "system" {
    fn FindWindowW(class: *const u16, title: *const u16) -> usize;
    fn IsWindowVisible(hwnd: usize) -> Bool;
}

#[link(name = "advapi32", kind = "raw-dylib")]
unsafe extern "system" {
    fn RegOpenKeyExW(key: Hkey, sub: *const u16, options: u32, access: u32, out: *mut Hkey) -> u32;
    fn RegQueryValueExW(
        key: Hkey,
        name: *const u16,
        reserved: *mut u32,
        kind: *mut u32,
        data: *mut u8,
        len: *mut u32,
    ) -> u32;
    fn RegEnumKeyExW(
        key: Hkey,
        index: u32,
        name: *mut u16,
        len: *mut u32,
        reserved: *mut u32,
        class: *mut u16,
        class_len: *mut u32,
        time: *mut u64,
    ) -> u32;
    fn RegCloseKey(key: Hkey) -> u32;
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

// The compiler's own calls bind to it (ntrt has the memory intrinsics).
#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn strlen(s: *const u8) -> usize;
}

const STARTF_USESTDHANDLES: u32 = 0x100;
const HKEY_CURRENT_USER: Hkey = 0x8000_0001;
const KEY_READ: u32 = 0x20019;
const LXSS: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Lxss";
const CHOICES: &str = "Software\\gala\\StartMenu";
const INVALID_ATTRIBUTES: u32 = u32::MAX;
/// How long a step may take: the scenario's clicks come once this test
/// has printed what the step waits for.
const STEP_MS: u64 = 60_000;
/// How long a program started from the menu may take to show its window
/// or end (wsl.exe, Xwayland, conhost on a busy host).
const PROGRAM_MS: u64 = 240_000;
/// How long the install may take (a download and `wsl --import` under
/// TCG take minutes).
const INSTALL_MS: u64 = 1_800_000;
/// A point inside the desktop session's xterm from its window's outer
/// corner (session.sh's `xterm -geometry 80x24+40+40` under twm's title
/// bar, in the window's caption and frame): twm's focus follows the
/// pointer, so the keys the screen check types reach the xterm only with
/// the pointer there.
const IN_SESSION_XTERM: (i32, i32) = (292, 292);

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
        out(&self.buf[..self.n]);
    }
}

fn out(text: &[u8]) {
    let mut written = 0;
    // SAFETY: valid buffer and output pointer.
    unsafe {
        WriteFile(
            GetStdHandle(-11i32 as u32),
            text.as_ptr(),
            text.len() as u32,
            &mut written,
            core::ptr::null_mut(),
        )
    };
}

fn say(what: &[u8]) {
    Line::new().s(NAME.as_bytes()).s(b": ").s(what).s(b"\r\n").print();
}

/// `<NAME>: <name> at X,Y (pointer at PX,PY)`: a point for the screen
/// check's `click_on`, where UI Automation has no element (inside the X
/// session's window), as `uia_client::announce` says an element's.
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

/// UTF-16 of an ASCII string, NUL-terminated, in a fixed buffer.
struct W {
    b: [u16; 256],
    n: usize,
}

impl W {
    fn new(text: &str) -> W {
        let mut w = W { b: [0; 256], n: 0 };
        w.push(text);
        w
    }

    fn push(&mut self, text: &str) {
        for &c in text.as_bytes() {
            if self.n < self.b.len() - 1 {
                self.b[self.n] = u16::from(c);
                self.n += 1;
            }
        }
    }

    fn ptr(&self) -> *const u16 {
        self.b.as_ptr()
    }

    fn ptr_mut(&mut self) -> *mut u16 {
        self.b.as_mut_ptr()
    }
}

// ---- gala.exe -------------------------------------------------------------

/// `gala`'s output (stdout and stderr together).
struct Output {
    b: [u8; 8192],
    n: usize,
}

impl Output {
    fn text(&self) -> &[u8] {
        &self.b[..self.n]
    }

    /// Lines naming process image `name` (`gala ps`'s last column).
    fn count_lines_ending(&self, name: &[u8]) -> usize {
        self.text()
            .split(|&c| c == b'\n')
            .map(|l| l.strip_suffix(b"\r").unwrap_or(l))
            .filter(|l| {
                l.len() >= name.len() && l[l.len() - name.len()..].eq_ignore_ascii_case(name)
            })
            .count()
    }

    /// Printed before a failed check: what gala said.
    fn show(&self) {
        out(b"linux_menu_test: gala said:\r\n");
        out(self.text());
        out(b"\r\n");
    }
}

/// Runs `C:\gala\gala.exe <args>`: its exit code and output.
fn gala(args: &str) -> (u32, Output) {
    let mut o = Output { b: [0; 8192], n: 0 };
    let null = core::ptr::null_mut();
    let sa = SecurityAttributes { length: 24, descriptor: null, inherit: 1 };
    let (mut r, mut w) = (null, null);
    let mut line = W::new("gala.exe ");
    line.push(args);
    let exe = W::new("C:\\gala\\gala.exe");
    // SAFETY: plain calls with our buffers and handles.
    unsafe {
        if CreatePipe(&mut r, &mut w, &sa, 0) == 0 {
            return (0xffff_0000 | GetLastError(), o);
        }
        SetHandleInformation(r, 1, 0);
        let mut startup = [0u8; 104];
        startup[0] = 104;
        startup[60..64].copy_from_slice(&STARTF_USESTDHANDLES.to_le_bytes());
        startup[80..88].copy_from_slice(&(GetStdHandle(-10i32 as u32) as u64).to_le_bytes());
        startup[88..96].copy_from_slice(&(w as u64).to_le_bytes());
        startup[96..104].copy_from_slice(&(w as u64).to_le_bytes());
        let mut info = [0usize; 3];
        let ok = CreateProcessW(
            exe.ptr(),
            line.ptr_mut(),
            null,
            null,
            1,
            0,
            null,
            core::ptr::null(),
            startup.as_ptr(),
            &mut info,
        );
        CloseHandle(w);
        if ok == 0 {
            CloseHandle(r);
            return (0xfffe_0000 | GetLastError(), o);
        }
        let mut got = 0;
        while o.n < o.b.len()
            && ReadFile(r, o.b.as_mut_ptr().add(o.n), (o.b.len() - o.n) as u32, &mut got, null) != 0
            && got > 0
        {
            o.n += got as usize;
        }
        CloseHandle(r);
        WaitForSingleObject(info[0] as Handle, 30_000);
        let mut code = 0xdead_0000;
        GetExitCodeProcess(info[0] as Handle, &mut code);
        CloseHandle(info[0] as Handle);
        CloseHandle(info[1] as Handle);
        (code, o)
    }
}

/// The wsl.exe processes running (`gala ps`), and the listing.
fn wsl_running() -> (usize, Output) {
    let o = gala("ps").1;
    (o.count_lines_ending(b"wsl.exe"), o)
}

/// Polls every half second for up to `ms`, or until `done`.
fn poll(ms: u64, mut done: impl FnMut() -> bool) -> bool {
    // SAFETY: plain call.
    let end = unsafe { GetTickCount64() } + ms;
    loop {
        if done() {
            return true;
        }
        // SAFETY: plain call.
        if unsafe { GetTickCount64() } >= end {
            return false;
        }
        // SAFETY: plain call.
        unsafe { Sleep(500) };
    }
}

// ---- The desktop's window list ----------------------------------------------

/// The outer rectangle (x, y, width, height) of the listed window titled
/// `title` (ACTION_WINDOWS: an 8-byte header, then entries of hwnd, pid,
/// id, flags, the rectangle, the title's length and 128 UTF-16 units).
fn window(title: &str) -> Option<[i32; 4]> {
    const ACTION_WINDOWS: u8 = 2;
    const ENTRY: usize = 40 + 2 * 128;
    let mut request = [0u8; 40];
    request[0] = 1; // version
    request[4] = ACTION_WINDOWS;
    let mut b: Vec<u8> = alloc::vec![0; 8 + 64 * ENTRY];
    let mut returned = 0;
    // SAFETY: our buffers with their lengths.
    let status = unsafe {
        GalaDesktopAction(request.as_ptr(), 40, b.as_mut_ptr(), b.len() as u32, &mut returned)
    };
    if status != 0 {
        return None;
    }
    let i32_at = |e: &[u8], at: usize| i32::from_le_bytes([e[at], e[at + 1], e[at + 2], e[at + 3]]);
    b[8..(returned as usize).min(b.len())].chunks_exact(ENTRY).find_map(|e| {
        let n = (i32_at(e, 36) as usize).min(128);
        let units: Vec<u16> =
            (0..n).map(|k| u16::from_le_bytes([e[40 + 2 * k], e[41 + 2 * k]])).collect();
        (String::from_utf16_lossy(&units) == title)
            .then(|| [i32_at(e, 20), i32_at(e, 24), i32_at(e, 28), i32_at(e, 32)])
    })
}

// ---- The registry -----------------------------------------------------------

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn utf16le(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    char::decode_utf16(units).map(|c| c.unwrap_or('\u{fffd}')).collect()
}

struct Key(Hkey);

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: a key this program opened.
        unsafe { RegCloseKey(self.0) };
    }
}

impl Key {
    fn open(parent: Hkey, path: &str) -> Option<Key> {
        let p = wide(path);
        let mut k = 0;
        // SAFETY: valid strings and out pointer.
        (unsafe { RegOpenKeyExW(parent, p.as_ptr(), 0, KEY_READ, &mut k) } == 0).then_some(Key(k))
    }

    fn value(&self, name: &str) -> Option<(u32, Vec<u8>)> {
        let n = wide(name);
        let mut buf = [0u8; 2048];
        let (mut kind, mut len) = (0u32, buf.len() as u32);
        // SAFETY: valid buffers of the stated sizes.
        let r = unsafe {
            RegQueryValueExW(
                self.0,
                n.as_ptr(),
                core::ptr::null_mut(),
                &mut kind,
                buf.as_mut_ptr(),
                &mut len,
            )
        };
        (r == 0).then(|| (kind, buf[..len as usize].to_vec()))
    }

    fn string(&self, name: &str) -> Option<String> {
        let (kind, data) = self.value(name)?;
        (kind == 1).then(|| String::from(utf16le(&data).trim_end_matches('\0')))
    }

    fn dword(&self, name: &str) -> Option<u32> {
        let (kind, data) = self.value(name)?;
        (kind == 4 && data.len() == 4)
            .then(|| u32::from_le_bytes([data[0], data[1], data[2], data[3]]))
    }

    fn subkeys(&self) -> Vec<String> {
        let mut out = Vec::new();
        for i in 0.. {
            let mut name = [0u16; 256];
            let mut len = name.len() as u32;
            let n = || core::ptr::null_mut();
            // SAFETY: valid buffers of the stated sizes.
            if unsafe {
                RegEnumKeyExW(
                    self.0,
                    i,
                    name.as_mut_ptr(),
                    &mut len,
                    n(),
                    core::ptr::null_mut(),
                    n(),
                    core::ptr::null_mut(),
                )
            } != 0
            {
                break;
            }
            out.push(String::from_utf16_lossy(&name[..len as usize]));
        }
        out
    }
}

/// Distribution `name`'s registration.
fn registration(name: &str) -> Option<Key> {
    let lxss = Key::open(HKEY_CURRENT_USER, LXSS)?;
    for guid in lxss.subkeys() {
        if let Some(k) = Key::open(lxss.0, &guid)
            && k.string("DistributionName").as_deref() == Some(name)
        {
            return Some(k);
        }
    }
    None
}

fn env(name: &str) -> String {
    let n = wide(name);
    let mut buf = [0u16; 512];
    // SAFETY: our buffers with their lengths.
    let len = unsafe { GetEnvironmentVariableW(n.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) };
    String::from_utf16_lossy(&buf[..(len as usize).min(buf.len())])
}

fn exists(path: &str) -> bool {
    // SAFETY: a NUL-terminated path.
    unsafe { GetFileAttributesW(wide(path).as_ptr()) != INVALID_ATTRIBUTES }
}

/// Whether this is gala (its ntdll has gala's own exports).
fn on_gala() -> bool {
    // SAFETY: NUL-terminated names.
    unsafe {
        let ntdll = GetModuleHandleA(c"ntdll.dll".as_ptr().cast());
        ntdll.is_null() || !GetProcAddress(ntdll, c"GalaDesktopAction".as_ptr().cast()).is_null()
    }
}

/// The percent of a "Downloading N%" state, or None.
fn downloading(state: &str) -> Option<u32> {
    state.strip_prefix("Downloading ")?.strip_suffix('%')?.parse().ok()
}

// ---- The checks -------------------------------------------------------------

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    // A fresh user: nothing chosen, nothing installed; the desktop
    // answers.
    c.ok(Key::open(HKEY_CURRENT_USER, CHOICES).is_none(), 0)?;
    c.ok(registration("Ubuntu").is_none() && registration("Alpine").is_none(), 0)?;
    let (code, o) = gala("programs");
    if code != 0 {
        o.show();
    }
    c.eq(code.into(), 0)?;

    // The taskbar's Start button, through UI Automation.
    let uia = Uia::new();
    c.ok(uia.is_ok(), uia.as_ref().err().copied().unwrap_or(0).into())?;
    let uia = uia.unwrap_or_else(|_| unreachable!());
    let (bar_class, bar_title) = (W::new("Shell_TrayWnd"), W::new("Taskbar"));
    let (menu_class, menu_title) = (W::new("GalaUiWindow"), W::new("Start"));
    // SAFETY: our NUL-ended names.
    let bar = || unsafe { FindWindowW(bar_class.ptr(), bar_title.ptr()) };
    let start = || uia.window(bar()).and_then(|b| uia.find(&b, "StartButton"));
    c.ok(poll(STEP_MS, || start().is_some()), bar() as i64)?;
    let s = start().unwrap_or_else(|| unreachable!());
    c.eq(s.control_type().into(), BUTTON.into())?;
    uia_client::announce(NAME, "StartButton", &s);
    say(b"listed");

    // SAFETY: our NUL-ended names.
    let menu = || unsafe { FindWindowW(menu_class.ptr(), menu_title.ptr()) };
    // SAFETY: plain call.
    let shown = || menu() != 0 && unsafe { IsWindowVisible(menu()) } != 0;
    let in_menu = |id: &str| -> Option<Element> { uia.find(&uia.window(menu())?, id) };
    let named = |id: &str, name: &str| in_menu(id).is_some_and(|e| e.name() == name);
    let state = |id: &str| -> String {
        in_menu(&format!("{id}State")).map(|e| String::from(e.name().as_str())).unwrap_or_default()
    };
    let announce_start = |c: &mut Checks| -> Result<(), u32> {
        let s = start();
        c.ok(s.is_some(), 0)?;
        uia_client::announce(NAME, "StartButton", &s.unwrap_or_else(|| unreachable!()));
        Ok(())
    };

    // The scenario's click: the menu, its Linux section with the two
    // suggestions not installed.
    let section = poll(STEP_MS, || {
        shown()
            && named("LinuxTitle", "Linux")
            && named("LinuxUbuntu", "Ubuntu")
            && state("LinuxUbuntu") == "Not installed"
            && named("LinuxAlpine", "Alpine")
            && state("LinuxAlpine") == "Not installed"
    });
    c.ok(section, menu() as i64)?;
    let ubuntu = in_menu("LinuxUbuntu").unwrap_or_else(|| unreachable!());
    c.eq(ubuntu.control_type().into(), BUTTON.into())?;
    let st = in_menu("LinuxUbuntuState").unwrap_or_else(|| unreachable!());
    c.eq(st.control_type().into(), TEXT.into())?;
    c.ok(in_menu("LinuxUbuntuBar").is_none(), 0)?;
    // After the programs' tiles.
    let cmd = in_menu("ProgramCommandPrompt").map(|e| e.rect());
    c.ok(cmd.is_some_and(|r| r.top < ubuntu.rect().top), ubuntu.rect().top.into())?;
    uia_client::announce(NAME, "Ubuntu", &ubuntu);
    say(b"Linux section shown");

    // Its click: the install; the menu stays open, the tile downloading
    // with its bar.
    let mut percent = 0;
    let down = poll(INSTALL_MS, || {
        percent = downloading(&state("LinuxUbuntu")).unwrap_or(0);
        shown() && percent > 0 && in_menu("LinuxUbuntuBar").is_some()
    });
    c.ok(down, percent.into())?;
    say(b"downloading shown");

    // Installed: registered, its tree in place, the tarball gone.
    let registered = || registration("Ubuntu").is_some_and(|k: Key| k.dword("State") == Some(1));
    c.ok(poll(INSTALL_MS, registered), 0)?;
    let k = registration("Ubuntu").unwrap_or_else(|| unreachable!());
    let base = k.string("BasePath").unwrap_or_default();
    let want = format!("{}\\Linux\\Ubuntu", env("LOCALAPPDATA"));
    c.ok(base.eq_ignore_ascii_case(&want), base.len() as i64)?;
    c.eq(k.dword("Version").map_or(-1, i64::from), 2)?;
    c.ok(exists(&format!("{base}\\rootfs\\lx\\xdesktop\\session.sh")), 0)?;
    // gala linux-install deletes it once wsl.exe has imported it.
    let tarball = format!("{}\\Linux\\Ubuntu.rootfs.tar.gz", env("LOCALAPPDATA"));
    c.ok(poll(STEP_MS, || !exists(&tarball)), 0)?;

    // The tile says so, without a bar, and its terminal's and
    // applications' tiles follow.
    let xterm_id = |i: usize| format!("LinuxAppUbuntu_{i}");
    let xterm = || (0..8).map(xterm_id).find(|id| named(id, "XTerm (Ubuntu)"));
    let listed = poll(STEP_MS, || {
        shown()
            && state("LinuxUbuntu") == "Installed"
            && in_menu("LinuxUbuntuBar").is_none()
            && named("LinuxTerminalUbuntu", "Terminal (Ubuntu)")
            && xterm().is_some()
    });
    c.ok(listed, shown().into())?;
    let xterm_id = xterm().unwrap_or_else(|| unreachable!());
    for id in ["LinuxTerminalUbuntu", xterm_id.as_str()] {
        let t = in_menu(id).unwrap_or_else(|| unreachable!());
        c.eq(t.control_type().into(), BUTTON.into())?;
    }
    let ubuntu = in_menu("LinuxUbuntu").unwrap_or_else(|| unreachable!());
    uia_client::announce(NAME, "UbuntuDesktop", &ubuntu);
    say(b"Ubuntu installed");

    // Its click: the desktop session's window, the menu hidden.
    let mut session = None;
    let started = poll(PROGRAM_MS, || {
        session = window("Xwayland on :1");
        session.is_some() && !shown()
    });
    c.ok(started, shown().into())?;
    let r = session.unwrap_or_else(|| unreachable!());
    announce_point(b"Session", r[0] + IN_SESSION_XTERM.0, r[1] + IN_SESSION_XTERM.1);
    say(b"desktop session shown");

    // `exit` in its xterm: wsl.exe ends, the window goes.
    let mut last = None;
    let ended = poll(PROGRAM_MS, || {
        let (n, o) = wsl_running();
        last = Some(o);
        n == 0 && window("Xwayland on :1").is_none()
    });
    if !ended && let Some(o) = &last {
        o.show();
    }
    c.ok(ended, window("Xwayland on :1").is_some().into())?;
    announce_start(&mut c)?;
    say(b"desktop session ended");

    // The menu again: the terminal's tile.
    let open = poll(STEP_MS, || shown() && named("LinuxTerminalUbuntu", "Terminal (Ubuntu)"));
    c.ok(open, menu() as i64)?;
    let t = in_menu("LinuxTerminalUbuntu").unwrap_or_else(|| unreachable!());
    uia_client::announce(NAME, "UbuntuTerminal", &t);
    say(b"menu open again");

    // Its click: wsl.exe's console window, the menu hidden; `exit` there
    // ends wsl.exe (its window stays).
    c.ok(poll(PROGRAM_MS, || window("wsl.exe").is_some() && !shown()), shown().into())?;
    let mut last = None;
    let ended = poll(PROGRAM_MS, || {
        let (n, o) = wsl_running();
        last = Some(o);
        n == 0
    });
    if !ended && let Some(o) = &last {
        o.show();
    }
    c.ok(ended, 0)?;
    announce_start(&mut c)?;
    say(b"terminal ended");

    // The menu again: XTerm's tile.
    let open = poll(STEP_MS, || shown() && named(&xterm_id, "XTerm (Ubuntu)"));
    c.ok(open, menu() as i64)?;
    let t = in_menu(&xterm_id).unwrap_or_else(|| unreachable!());
    uia_client::announce(NAME, "XTerm", &t);
    say(b"app tile shown");

    // Its click: its window, the menu hidden.
    c.ok(poll(PROGRAM_MS, || window("xterm").is_some() && !shown()), shown().into())?;
    say(b"application shown");
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    if !on_gala() {
        say(b"skipped (gala's start menu)");
        // SAFETY: ends the process.
        unsafe { ExitProcess(0) }
    }
    match run() {
        Ok(()) => {
            say(b"ok");
            // SAFETY: ends the process.
            unsafe { ExitProcess(0) }
        }
        // SAFETY: ends the process.
        Err(n) => unsafe { ExitProcess(n) },
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdead) }
}

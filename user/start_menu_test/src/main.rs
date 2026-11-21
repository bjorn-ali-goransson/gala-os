//! The start menu's apps (ROADMAP SM, SH step 1 part 11): shasha's start
//! menu lists gala's own Command Prompt beside the catalog's programs,
//! which install on a click. Driven by xtask's screen check (scenario
//! `start_menu`, UEFI), which clicks what this program finds through UI
//! Automation and announces (`click_on start_menu_test: <element>`).
//! gala's own shell and catalog, so there is no Windows run to compare
//! with; on Windows it says so and passes.
//!
//! - `gala programs` lists the Command Prompt at
//!   C:\Windows\System32\cmd.exe (the scenario ships gala's cmd.exe
//!   there too, so Windows' place is preferred to C:\gala) and no
//!   catalog entry: none is installed.
//! - The taskbar's Start button shows the "Start" window, which holds a
//!   tile per installed program (`ProgramCommandPrompt`, a Button named
//!   "Command Prompt", with no state under it). A click on it starts
//!   cmd.exe with its own console window (conhost.exe), which stays when
//!   cmd ends, and hides the menu.
//! - The menu again: a tile per catalog program that is not installed,
//!   after the installed ones: 7-Zip's (`Program7Zip`, a Button named
//!   "7-Zip") with a Text `Program7ZipState` reading "Not installed".
//!   A click on it asks for `gala install 7zip.7zip` without a window
//!   and the menu stays open: the state follows the install, here
//!   (offline: no network card) to "The install failed.".
//! - No second console window comes of that click: a console window
//!   would be made in the click's own handling and would stay when gala
//!   ends (the Command Prompt's does), so none by the time the failure
//!   shows means none was made.
//!
//! Assumed and not pinned: the AutomationIds and the state's words are
//! gala's own choice (shasha's menu).
//!
//! Prints `start_menu_test: listed`, `start_menu_test: menu open`,
//! `start_menu_test: command prompt window`, `start_menu_test: catalog
//! shown`, `start_menu_test: install failed shown`, `start_menu_test: no
//! install window` and `start_menu_test: ok`, or `start_menu_test: check
//! N got X (line L)` and exits with N.

#![no_std]
#![no_main]

use core::ffi::c_void;

use ntrt as _;
use uia_client::{BUTTON, Element, TEXT, Uia};

const NAME: &str = "start_menu_test";

type Bool = i32;
type Handle = *mut c_void;

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
    fn Sleep(ms: u32);
    fn GetTickCount64() -> u64;
}

#[link(name = "user32", kind = "raw-dylib")]
unsafe extern "system" {
    fn FindWindowW(class: *const u16, title: *const u16) -> usize;
    fn IsWindowVisible(hwnd: usize) -> Bool;
}

const STARTF_USESTDHANDLES: u32 = 0x100;
/// How long a step may take: the scenario's clicks come once this test
/// has printed what the step waits for.
const STEP_MS: u64 = 60_000;
/// How long the Command Prompt's console window may take to show (cmd
/// and conhost started from the menu on a busy host).
const CONSOLE_MS: u64 = 240_000;

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

/// `gala`'s output (stdout and stderr together).
struct Output {
    b: [u8; 8192],
    n: usize,
}

impl Output {
    fn text(&self) -> &[u8] {
        &self.b[..self.n]
    }

    fn has(&self, what: &[u8]) -> bool {
        what.is_empty() || self.text().windows(what.len()).any(|w| w == what)
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
        out(b"start_menu_test: gala said:\r\n");
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

/// The console windows running (`gala ps`'s conhost.exe lines), and the
/// listing.
fn consoles() -> (usize, Output) {
    let o = gala("ps").1;
    (o.count_lines_ending(b"conhost.exe"), o)
}

/// Whether this is gala (its ntdll has gala's own exports).
fn on_gala() -> bool {
    // SAFETY: NUL-terminated names.
    unsafe {
        let ntdll = GetModuleHandleA(c"ntdll.dll".as_ptr().cast());
        ntdll.is_null() || !GetProcAddress(ntdll, c"GalaConsoleCreate".as_ptr().cast()).is_null()
    }
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    // The Command Prompt in System32; nothing of the catalog.
    let (code, o) = gala("programs");
    let listed = code == 0 && o.has(b"Command Prompt") && o.has(b"C:\\Windows\\System32\\cmd.exe");
    let catalog = o.has(b"7-Zip") || o.has(b"Python");
    if !listed || catalog {
        o.show();
    }
    c.ok(listed, code.into())?;
    c.ok(!catalog, 0)?;

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

    // The scenario's click: the menu, its Command Prompt tile.
    // SAFETY: our NUL-ended names.
    let menu = || unsafe { FindWindowW(menu_class.ptr(), menu_title.ptr()) };
    // SAFETY: plain call.
    let shown = || menu() != 0 && unsafe { IsWindowVisible(menu()) } != 0;
    let in_menu = |id: &str| -> Option<Element> { uia.find(&uia.window(menu())?, id) };
    let tile = |id: &str, name: &str| in_menu(id).is_some_and(|e| e.name() == name);
    let open = poll(STEP_MS, || shown() && tile("ProgramCommandPrompt", "Command Prompt"));
    c.ok(open, menu() as i64)?;
    let t = in_menu("ProgramCommandPrompt").unwrap_or_else(|| unreachable!());
    c.eq(t.control_type().into(), BUTTON.into())?;
    // Installed: no state under it.
    c.ok(in_menu("ProgramCommandPromptState").is_none(), 0)?;
    uia_client::announce(NAME, "CommandPrompt", &t);
    say(b"menu open");

    // The tile's click: cmd.exe's console window (gala's cmd has no
    // interactive prompt yet: it says so there and ends; the window
    // stays), and the menu hidden.
    let mut last = None;
    let console = poll(CONSOLE_MS, || {
        let (n, o) = consoles();
        last = Some(o);
        n >= 1
    });
    if !console && let Some(o) = &last {
        o.show();
    }
    c.ok(console, 0)?;
    c.ok(poll(STEP_MS, || !shown()), 0)?;
    let s = start();
    c.ok(s.is_some(), 0)?;
    uia_client::announce(NAME, "StartButton", &s.unwrap_or_else(|| unreachable!()));
    say(b"command prompt window");

    // The menu again: 7-Zip's tile after the installed programs, not
    // installed.
    let state = |want: &str| in_menu("Program7ZipState").is_some_and(|e| e.name() == want);
    let listed =
        poll(STEP_MS, || shown() && tile("Program7Zip", "7-Zip") && state("Not installed"));
    c.ok(listed, menu() as i64)?;
    let t = in_menu("Program7Zip").unwrap_or_else(|| unreachable!());
    c.eq(t.control_type().into(), BUTTON.into())?;
    let st = in_menu("Program7ZipState").unwrap_or_else(|| unreachable!());
    c.eq(st.control_type().into(), TEXT.into())?;
    // After the installed programs: further down, or further right in
    // the same row.
    let (r7, cmd) = (t.rect(), in_menu("ProgramCommandPrompt").map(|e| e.rect()));
    let after = cmd.is_some_and(|r| r.top < r7.top || (r.top == r7.top && r.left < r7.left));
    c.ok(after, r7.top.into())?;
    uia_client::announce(NAME, "SevenZip", &t);
    say(b"catalog shown");

    // Its click: the install, without a window; the menu stays open and
    // the state follows it to the failure (offline).
    let failed = poll(STEP_MS, || shown() && state("The install failed."));
    c.ok(failed, shown().into())?;
    say(b"install failed shown");

    // No second console window.
    let (n, o) = consoles();
    if n >= 2 {
        o.show();
    }
    c.ok(n < 2, n as i64)?;
    say(b"no install window");
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

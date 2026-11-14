//! The start menu's apps (ROADMAP SM): gala's own Command Prompt beside
//! catalog entries that install on a click. Driven by xtask's screen
//! check (scenario `start_menu`, UEFI): this program checks what `gala
//! programs` lists, says `start_menu_test: ready`, and then waits for
//! what the clicks start: the Command Prompt (cmd.exe in a console
//! window) and `gala install 7zip.7zip` (a second console window). gala's
//! own shell, so there is no Windows run to compare with; on Windows it
//! says so and passes.
//!
//! - `gala programs` lists the Command Prompt at
//!   C:\Windows\System32\cmd.exe (the scenario ships gala's cmd.exe
//!   there too, so Windows' place is preferred to C:\gala) and no
//!   catalog entry: none is installed.
//! - A click on the Command Prompt's tile starts cmd.exe with its own
//!   console window (conhost.exe), which stays when cmd ends.
//! - A click on 7-Zip's tile (not installed: a download badge) starts
//!   `gala install 7zip.7zip` in another console window, which stays
//!   after gala ends (its output in view) until it is closed (Alt+F4).
//!
//! Prints `start_menu_test: ok` or exits with the number of the failing
//! check.

#![no_std]
#![no_main]

use core::ffi::c_void;

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
}

// The compiler's own calls (copies, stack probes) bind to these.
#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn __chkstk();
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
}

#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

/// core's unwind tables name it; nothing unwinds (panics end the process).
#[unsafe(no_mangle)]
pub extern "C" fn __CxxFrameHandler3() -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdeac) }
}

const STARTF_USESTDHANDLES: u32 = 0x100;

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

fn hex(v: u32) -> [u8; 8] {
    let mut s = [0u8; 8];
    for (i, c) in s.iter_mut().enumerate() {
        let d = (v >> (28 - 4 * i)) & 0xf;
        *c = if d < 10 { b'0' + d as u8 } else { b'a' + d as u8 - 10 };
    }
    s
}

/// UTF-16 of an ASCII string, NUL-terminated, in a fixed buffer.
struct W {
    b: [u16; 256],
    n: usize,
}

impl W {
    fn new(text: &str) -> W {
        let mut w = W { b: [0; 256], n: 0 };
        for &c in text.as_bytes() {
            if w.n < w.b.len() - 1 {
                w.b[w.n] = u16::from(c);
                w.n += 1;
            }
        }
        w
    }

    fn ptr(&mut self) -> *mut u16 {
        self.b[self.n] = 0;
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
}

/// Runs `C:\gala\gala.exe <args>`: its exit code and output.
fn gala(args: &str) -> (u32, Output) {
    let mut o = Output { b: [0; 8192], n: 0 };
    let null = core::ptr::null_mut();
    let sa = SecurityAttributes { length: 24, descriptor: null, inherit: 1 };
    let (mut r, mut w) = (null, null);
    let mut line = W::new("gala.exe ");
    for &c in args.as_bytes() {
        if line.n < line.b.len() - 1 {
            line.b[line.n] = u16::from(c);
            line.n += 1;
        }
    }
    let mut exe = W::new("C:\\gala\\gala.exe");
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
            line.ptr(),
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

fn fail(check: u32, got: u32, o: &Output) -> ! {
    out(b"start_menu_test: check ");
    out(&hex(check));
    out(b" got ");
    out(&hex(got));
    out(b":\r\n");
    out(o.text());
    out(b"\r\n");
    // SAFETY: ends the process.
    unsafe { ExitProcess(check) }
}

/// Polls `gala ps` every half second, for at most `secs` seconds, until
/// `done` says so; the last listing.
fn wait_for(secs: u32, done: impl Fn(&Output) -> bool) -> (bool, Output) {
    let mut last = gala("ps").1;
    for _ in 0..secs * 2 {
        if done(&last) {
            return (true, last);
        }
        // SAFETY: plain call.
        unsafe { Sleep(500) };
        last = gala("ps").1;
    }
    (done(&last), last)
}

/// Whether this is gala (its ntdll has gala's own exports).
fn on_gala() -> bool {
    // SAFETY: NUL-terminated names.
    unsafe {
        let ntdll = GetModuleHandleA(c"ntdll.dll".as_ptr().cast());
        ntdll.is_null() || !GetProcAddress(ntdll, c"GalaConsoleCreate".as_ptr().cast()).is_null()
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    if !on_gala() {
        out(b"start_menu_test: skipped (gala's start menu)\r\n");
        // SAFETY: ends the process.
        unsafe { ExitProcess(0) }
    }
    // 1: the Command Prompt in System32; nothing of the catalog.
    let (code, o) = gala("programs");
    if code != 0 || !o.has(b"Command Prompt") || !o.has(b"C:\\Windows\\System32\\cmd.exe") {
        fail(1, code, &o);
    }
    if o.has(b"7-Zip") || o.has(b"Python") {
        fail(2, code, &o);
    }
    out(b"start_menu_test: listed\r\n");
    // The screen check clicks the tiles now.
    out(b"start_menu_test: ready\r\n");
    // 3: the Command Prompt's tile: cmd.exe's console window (gala's
    // cmd has no interactive prompt yet: it says so there and ends; the
    // window stays).
    let (seen, o) = wait_for(240, |o| o.count_lines_ending(b"conhost.exe") >= 1);
    if !seen {
        fail(3, 0, &o);
    }
    out(b"start_menu_test: command prompt window\r\n");
    // 4: 7-Zip's tile: `gala install 7zip.7zip` in a second console
    // window, which stays when gala ends.
    let (seen, o) = wait_for(240, |o| o.count_lines_ending(b"conhost.exe") >= 2);
    if !seen {
        fail(4, 0, &o);
    }
    out(b"start_menu_test: install window\r\n");
    // 5: the screen check closes it (Alt+F4): its conhost.exe ends.
    let (seen, o) = wait_for(240, |o| o.count_lines_ending(b"conhost.exe") < 2);
    if !seen {
        fail(5, 0, &o);
    }
    out(b"start_menu_test: install window closed\r\n");
    out(b"start_menu_test: ok\r\n");
    // SAFETY: ends the process.
    unsafe { ExitProcess(0) }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdead) }
}

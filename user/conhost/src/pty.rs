//! conhost.exe's pseudo-console mode (ConPTY; ARCHITECTURE "Pseudo
//! consoles"): no window; the console's output goes to the standard
//! output (a pipe to the terminal) and the terminal's input, read from the
//! standard input, becomes key events. Two ways in, both as Windows'
//! console host is started:
//!
//! - kernel32's CreatePseudoConsole: `conhost.exe --headless --width W
//!   --height H --signal 0xS --session N [--inheritcursor]` hosts session
//!   N, which kernel32 made (GalaConsoleCreate); the caller attaches its
//!   programs with PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE.
//! - a program that runs the host itself, as Win32-OpenSSH's sshd-session
//!   does for a pty: `conhost.exe --headless --width W --height H --signal
//!   0xS -- <command line>`: conhost makes the console and starts the
//!   command in it, and ends with the command's exit code once it ends.
//!
//! The signal pipe carries resizes from the program that started the host
//! (records of three 16-bit words: 8, columns, rows); its end closes the
//! console. Input: UTF-8 text; CR is Enter, DEL and BS Backspace, 0x03
//! Ctrl+C (the console's control event when input is processed), other
//! control characters Ctrl+letter; ESC [ / ESC O sequences are the arrow,
//! editing and function keys (with xterm's modifier parameter); a cursor
//! position report (the answer to `--inheritcursor`'s ESC [ 6 n) is
//! dropped. With `conpty=trace` on the kernel command line both
//! directions are logged (GalaConsoleTrace).

use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, Ordering};

type Handle = *mut c_void;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn ReadFile(h: Handle, buf: *mut u8, len: u32, read: *mut u32, ov: *mut c_void) -> i32;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut c_void) -> i32;
    fn CreateEventW(sa: *const c_void, manual: i32, initial: i32, name: *const u16) -> Handle;
    fn CreateThread(
        sa: *const c_void,
        stack: usize,
        start: extern "system" fn(*mut c_void) -> u32,
        arg: *mut c_void,
        flags: u32,
        id: *mut u32,
    ) -> Handle;
    fn WaitForMultipleObjects(n: u32, handles: *const Handle, all: i32, ms: u32) -> u32;
    fn CreateProcessW(
        application: *const u16,
        command_line: *mut u16,
        process_attributes: *const c_void,
        thread_attributes: *const c_void,
        inherit: i32,
        flags: u32,
        environment: *const c_void,
        directory: *const u16,
        startup: *const u8,
        info: *mut [usize; 3],
    ) -> i32;
    fn InitializeProcThreadAttributeList(
        list: *mut u8,
        count: u32,
        flags: u32,
        size: *mut usize,
    ) -> i32;
    fn UpdateProcThreadAttribute(
        list: *mut u8,
        flags: u32,
        attribute: usize,
        value: *const c_void,
        size: usize,
        previous: *mut c_void,
        returned: *mut usize,
    ) -> i32;
    fn GetExitCodeProcess(h: Handle, code: *mut u32) -> i32;
    fn CloseHandle(h: Handle) -> i32;
    fn GetLastError() -> u32;
}

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn GalaConsoleHost(id: u32, event: Handle) -> u32;
    fn GalaConsoleRead(id: u32, buf: *mut u8, len: u32, count: *mut u32) -> u32;
    fn GalaConsoleInput(id: u32, records: *const u8, count: u32) -> u32;
    fn GalaConsoleCreate(flags: u32, size: u32, info: *mut [u32; 2]) -> u32;
    fn GalaConsoleSize(id: u32, size: *mut u32, set: u32) -> u32;
    fn GalaConsoleClose(id: u32) -> u32;
    fn GalaConsoleTrace(id: u32, direction: u32, bytes: *const u8, len: usize) -> u32;
}

const PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE: usize = 0x0002_0016;
const EXTENDED_STARTUPINFO_PRESENT: u32 = 0x0008_0000;
const INFINITE: u32 = 0xffff_ffff;
const STATUS_NOT_SUPPORTED: u32 = 0xc000_00bb;

/// The command line to start (CreateProcessW may change it).
static mut COMMAND: [u16; 2048] = [0; 2048];

/// The session hosted, and whether `conpty=trace` is on (0/1).
static SESSION: AtomicU32 = AtomicU32::new(0);
static TRACING: AtomicU32 = AtomicU32::new(0);

/// Logs `bytes` to the kernel's trace (0: from the terminal, 1: to it,
/// 2: a note) when `conpty=trace` is on.
fn trace(direction: u32, bytes: &[u8]) {
    if TRACING.load(Ordering::Relaxed) == 0 {
        return;
    }
    for chunk in bytes.chunks(1024) {
        // SAFETY: our buffer.
        unsafe {
            GalaConsoleTrace(
                SESSION.load(Ordering::Relaxed),
                direction,
                chunk.as_ptr(),
                chunk.len(),
            )
        };
    }
}

/// The options after the program's path; the command line after `--`.
struct Options {
    width: u32,
    height: u32,
    signal: usize,
    session: u32,
    inherit_cursor: bool,
    /// Where the command line after ` -- ` starts (0: none).
    command: usize,
}

/// Whether `line` (the whole command line) asks for pseudo-console mode.
pub fn wanted(line: &[u16]) -> bool {
    words(line).any(|(w, _)| eq(w, b"--headless"))
}

fn eq(word: &[u16], text: &[u8]) -> bool {
    word.len() == text.len() && word.iter().zip(text).all(|(&a, &b)| a == u16::from(b))
}

/// The words of `line` (separated by spaces; quotes keep spaces) with the
/// index after each.
fn words(line: &[u16]) -> impl Iterator<Item = (&[u16], usize)> {
    let mut i = 0;
    core::iter::from_fn(move || {
        while i < line.len() && line[i] == u16::from(b' ') {
            i += 1;
        }
        if i >= line.len() {
            return None;
        }
        let start = i;
        let mut quoted = false;
        while i < line.len() && (quoted || line[i] != u16::from(b' ')) {
            if line[i] == u16::from(b'"') {
                quoted = !quoted;
            }
            i += 1;
        }
        Some((&line[start..i], i))
    })
}

fn number(word: &[u16]) -> usize {
    let (digits, base) = match word {
        [z, x, rest @ ..] if *z == u16::from(b'0') && (*x | 0x20) == u16::from(b'x') => (rest, 16),
        _ => (word, 10),
    };
    let mut n = 0usize;
    for &c in digits {
        let d = match c as u8 {
            c @ b'0'..=b'9' => usize::from(c - b'0'),
            c @ b'a'..=b'f' => usize::from(c - b'a' + 10),
            c @ b'A'..=b'F' => usize::from(c - b'A' + 10),
            _ => break,
        };
        n = n.wrapping_mul(base).wrapping_add(d);
    }
    n
}

fn options(line: &[u16]) -> Options {
    let mut o =
        Options { width: 80, height: 25, signal: 0, session: 0, inherit_cursor: false, command: 0 };
    let mut it = words(line).skip(1);
    while let Some((w, end)) = it.next() {
        let mut value = || it.next().map_or(0, |(v, _)| number(v));
        if eq(w, b"--") {
            o.command = end;
            break;
        } else if eq(w, b"--width") {
            o.width = value() as u32;
        } else if eq(w, b"--height") {
            o.height = value() as u32;
        } else if eq(w, b"--signal") {
            o.signal = value();
        } else if eq(w, b"--session") {
            o.session = value() as u32;
        } else if eq(w, b"--inheritcursor") {
            o.inherit_cursor = true;
        } else if !eq(w, b"--headless") {
            // Another documented option of Windows' host (--vtmode,
            // --feature ...): noted, not acted on.
            let mut note = [0u8; 64];
            let n = w.len().min(64);
            for (d, &c) in note.iter_mut().zip(&w[..n]) {
                *d = c as u8;
            }
            trace(2, &note[..n]);
        }
    }
    // A terminal that did not say (an ssh client without a tty asks for
    // 0 x 0): the default size.
    if o.width == 0 {
        o.width = 80;
    }
    if o.height == 0 {
        o.height = 25;
    }
    o.width = o.width.clamp(1, 0x7fff);
    o.height = o.height.clamp(1, 0x7fff);
    o
}

/// Writes all of `bytes` to the terminal; false when it is gone.
fn to_terminal(out: Handle, bytes: &[u8]) -> bool {
    let mut done = 0;
    while done < bytes.len() {
        let mut written = 0u32;
        // SAFETY: our buffer; the output pipe.
        let ok = unsafe {
            WriteFile(
                out,
                bytes[done..].as_ptr(),
                (bytes.len() - done) as u32,
                &mut written,
                core::ptr::null_mut(),
            )
        };
        if ok == 0 || written == 0 {
            return false;
        }
        done += written as usize;
    }
    trace(1, bytes);
    true
}

// ---- Input: VT text to key events ----

const VK_BACK: u16 = 0x08;
const VK_TAB: u16 = 0x09;
const VK_RETURN: u16 = 0x0d;
const VK_ESCAPE: u16 = 0x1b;
const VK_SPACE: u16 = 0x20;
const SHIFT_PRESSED: u32 = 0x10;
const LEFT_CTRL_PRESSED: u32 = 0x08;
const LEFT_ALT_PRESSED: u32 = 0x02;
const ENHANCED_KEY: u32 = 0x100;

/// Key records waiting to be queued (KEY_EVENT_RECORD, 16 bytes).
struct Keys {
    buf: [u8; 16 * 64],
    n: usize,
}

impl Keys {
    fn flush(&mut self) {
        if self.n > 0 {
            // SAFETY: our records.
            unsafe {
                GalaConsoleInput(SESSION.load(Ordering::Relaxed), self.buf.as_ptr(), self.n as u32)
            };
            self.n = 0;
        }
    }

    /// A key pressed and released.
    fn key(&mut self, vk: u16, ch: u16, control: u32) {
        for down in [1u32, 0] {
            if self.n == 64 {
                self.flush();
            }
            let r = &mut self.buf[16 * self.n..16 * (self.n + 1)];
            r[0..4].copy_from_slice(&down.to_le_bytes());
            r[4..6].copy_from_slice(&1u16.to_le_bytes());
            r[6..8].copy_from_slice(&vk.to_le_bytes());
            r[8..10].copy_from_slice(&0u16.to_le_bytes());
            r[10..12].copy_from_slice(&ch.to_le_bytes());
            r[12..16].copy_from_slice(&control.to_le_bytes());
            self.n += 1;
        }
    }

    /// A character typed (not a control character).
    fn char(&mut self, c: char, control: u32) {
        let (vk, shift) = match c {
            'a'..='z' => (c as u16 - 0x20, 0),
            'A'..='Z' => (c as u16, SHIFT_PRESSED),
            '0'..='9' | ' ' => (c as u16, 0),
            _ => (0, 0),
        };
        let mut units = [0u16; 2];
        for &u in c.encode_utf16(&mut units).iter() {
            self.key(vk, u, control | shift);
        }
    }
}

/// The VT input parser's state across reads.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Parse {
    Text,
    /// After ESC.
    Escape,
    /// In ESC [ (CSI) or ESC O (SS3): parameters so far.
    Csi {
        ss3: bool,
    },
}

struct Input {
    state: Parse,
    params: [u16; 4],
    count: usize,
    /// A UTF-8 sequence being collected.
    utf8: [u8; 4],
    have: usize,
    need: usize,
}

/// The modifiers in xterm's parameter (1 + shift 1, alt 2, ctrl 4).
fn modifiers(p: u16) -> u32 {
    let m = p.saturating_sub(1);
    let mut c = 0;
    if m & 1 != 0 {
        c |= SHIFT_PRESSED;
    }
    if m & 2 != 0 {
        c |= LEFT_ALT_PRESSED;
    }
    if m & 4 != 0 {
        c |= LEFT_CTRL_PRESSED;
    }
    c
}

impl Input {
    fn csi(&mut self, keys: &mut Keys, ss3: bool, last: u8) {
        let p = &self.params[..self.count.max(1)];
        let control = modifiers(*p.get(1).unwrap_or(&1)) | ENHANCED_KEY;
        let vk = match last {
            b'A' => 0x26,
            b'B' => 0x28,
            b'C' => 0x27,
            b'D' => 0x25,
            b'H' => 0x24,
            b'F' => 0x23,
            b'P' => 0x70,
            b'Q' => 0x71,
            b'R' if ss3 => 0x72,
            b'S' => 0x73,
            b'Z' => {
                keys.key(VK_TAB, 9, SHIFT_PRESSED);
                return;
            }
            // A cursor position report (ESC [ row ; col R).
            b'R' => return,
            b'~' => match p[0] {
                1 | 7 => 0x24,
                2 => 0x2d,
                3 => 0x2e,
                4 | 8 => 0x23,
                5 => 0x21,
                6 => 0x22,
                11..=15 => 0x70 + p[0] - 11,
                17..=21 => 0x75 + p[0] - 17,
                23 | 24 => 0x7a + p[0] - 23,
                _ => 0,
            },
            _ => 0,
        };
        if vk == 0 {
            trace(2, b"unknown input sequence");
            return;
        }
        let control = if (0x70..=0x7b).contains(&vk) { control & !ENHANCED_KEY } else { control };
        keys.key(vk, 0, control);
    }

    fn feed(&mut self, keys: &mut Keys, bytes: &[u8]) {
        for &b in bytes {
            match self.state {
                Parse::Escape => match b {
                    b'[' => {
                        self.state = Parse::Csi { ss3: false };
                        self.params = [0; 4];
                        self.count = 0;
                    }
                    b'O' => {
                        self.state = Parse::Csi { ss3: true };
                        self.params = [0; 4];
                        self.count = 0;
                    }
                    0x1b => keys.key(VK_ESCAPE, 0x1b, 0),
                    _ => {
                        // Alt and a key.
                        self.state = Parse::Text;
                        self.text(keys, b, LEFT_ALT_PRESSED);
                    }
                },
                Parse::Csi { ss3 } => match b {
                    b'0'..=b'9' => {
                        let i = self.count.max(1) - 1;
                        if self.count == 0 {
                            self.count = 1;
                        }
                        self.params[i] = self.params[i].saturating_mul(10) + u16::from(b - b'0');
                    }
                    b';' => {
                        if self.count == 0 {
                            self.count = 1;
                        }
                        if self.count < 4 {
                            self.count += 1;
                        }
                    }
                    0x40..=0x7e => {
                        self.state = Parse::Text;
                        self.csi(keys, ss3, b);
                    }
                    _ => {}
                },
                Parse::Text => self.text(keys, b, 0),
            }
        }
        // ESC at the end of what the terminal sent: the Escape key.
        if self.state == Parse::Escape {
            self.state = Parse::Text;
            keys.key(VK_ESCAPE, 0x1b, 0);
        }
    }

    fn text(&mut self, keys: &mut Keys, b: u8, control: u32) {
        if self.need > 0 {
            if b & 0xc0 == 0x80 {
                self.utf8[self.have] = b;
                self.have += 1;
                if self.have == self.need {
                    self.need = 0;
                    let c = core::str::from_utf8(&self.utf8[..self.have])
                        .ok()
                        .and_then(|s| s.chars().next())
                        .unwrap_or('\u{fffd}');
                    keys.char(c, control);
                }
                return;
            }
            self.need = 0;
        }
        match b {
            0x1b => self.state = Parse::Escape,
            b'\r' => keys.key(VK_RETURN, 13, control),
            0x7f | 0x08 => keys.key(VK_BACK, 8, control),
            b'\t' => keys.key(VK_TAB, 9, control),
            // Ctrl+@ / Ctrl+Space.
            0 => keys.key(VK_SPACE, 0, control | LEFT_CTRL_PRESSED),
            // Ctrl+letter (Ctrl+C included: the console makes it the
            // control event when input is processed).
            1..=0x1a => {
                keys.key(u16::from(b'A' + b - 1), u16::from(b), control | LEFT_CTRL_PRESSED)
            }
            0x1c..=0x1f => keys.key(0, u16::from(b), control | LEFT_CTRL_PRESSED),
            0x20..=0x7e => keys.char(char::from(b), control),
            0xc0..=0xdf => self.start_utf8(b, 2),
            0xe0..=0xef => self.start_utf8(b, 3),
            0xf0..=0xf7 => self.start_utf8(b, 4),
            _ => keys.char('\u{fffd}', control),
        }
    }

    fn start_utf8(&mut self, b: u8, need: usize) {
        self.utf8[0] = b;
        self.have = 1;
        self.need = need;
    }
}

/// The input thread: the terminal's bytes become key events; at the input
/// pipe's end the console closes (the terminal is gone).
extern "system" fn input_thread(input: *mut c_void) -> u32 {
    let mut parser =
        Input { state: Parse::Text, params: [0; 4], count: 0, utf8: [0; 4], have: 0, need: 0 };
    let mut keys = Keys { buf: [0; 16 * 64], n: 0 };
    loop {
        let mut buf = [0u8; 512];
        let mut got = 0u32;
        // SAFETY: our buffer; the input pipe.
        let ok = unsafe { ReadFile(input, buf.as_mut_ptr(), 512, &mut got, core::ptr::null_mut()) };
        if ok == 0 || got == 0 {
            if ok == 0 {
                trace(2, b"input ended");
                // SAFETY: the session we host.
                unsafe { GalaConsoleClose(SESSION.load(Ordering::Relaxed)) };
                return 0;
            }
            continue;
        }
        trace(0, &buf[..got as usize]);
        parser.feed(&mut keys, &buf[..got as usize]);
        keys.flush();
    }
}

/// The signal thread: resizes; the pipe's end closes the console.
extern "system" fn signal_thread(signal: *mut c_void) -> u32 {
    let mut pending = [0u8; 64];
    let mut have = 0;
    loop {
        let mut got = 0u32;
        // SAFETY: our buffer; the signal pipe.
        let ok = unsafe {
            ReadFile(
                signal,
                pending[have..].as_mut_ptr(),
                (pending.len() - have) as u32,
                &mut got,
                core::ptr::null_mut(),
            )
        };
        if ok == 0 {
            trace(2, b"signal pipe closed");
            // SAFETY: the session we host.
            unsafe { GalaConsoleClose(SESSION.load(Ordering::Relaxed)) };
            return 0;
        }
        have += got as usize;
        trace(2, &pending[..have]);
        // Records: a 16-bit code; 8 (resize) has columns and rows.
        let mut at = 0;
        while have - at >= 2 {
            let code = u16::from_le_bytes([pending[at], pending[at + 1]]);
            if code == 8 {
                if have - at < 6 {
                    break;
                }
                let w = u16::from_le_bytes([pending[at + 2], pending[at + 3]]);
                let h = u16::from_le_bytes([pending[at + 4], pending[at + 5]]);
                let mut size = u32::from(w) | (u32::from(h) << 16);
                // SAFETY: our size; the session we host.
                unsafe { GalaConsoleSize(SESSION.load(Ordering::Relaxed), &mut size, 1) };
                at += 6;
            } else {
                trace(2, b"unknown signal");
                at = have;
            }
        }
        pending.copy_within(at..have, 0);
        have -= at;
    }
}

/// Pseudo-console mode; the exit code.
pub fn run(line: &[u16]) -> u32 {
    let o = options(line);
    // SAFETY: plain calls with our buffers and handles.
    unsafe {
        let input = GetStdHandle(-10i32 as u32);
        let output = GetStdHandle(-11i32 as u32);
        let size = o.width | (o.height << 16);
        let session = if o.session != 0 {
            o.session
        } else {
            let mut info = [0u32; 2];
            if GalaConsoleCreate(1, size, &mut info) != 0 {
                return 1;
            }
            info[0]
        };
        SESSION.store(session, Ordering::Relaxed);
        let traced = GalaConsoleTrace(session, 2, b"host".as_ptr(), 4) != STATUS_NOT_SUPPORTED;
        TRACING.store(u32::from(traced), Ordering::Relaxed);
        let event = CreateEventW(core::ptr::null(), 0, 0, core::ptr::null());
        if event.is_null() || GalaConsoleHost(session, event) != 0 {
            return 2;
        }
        if o.inherit_cursor {
            // Where is the terminal's cursor? (The answer is dropped.)
            to_terminal(output, b"\x1b[6n");
        }
        // The command, in the console (sshd-session's way).
        let mut child: Handle = core::ptr::null_mut();
        if o.command != 0 {
            // (A static: a 4 KiB frame would need __chkstk.)
            let cmd = &mut *(&raw mut COMMAND);
            let rest = &line[o.command..];
            let start = rest.iter().position(|&c| c != u16::from(b' ')).unwrap_or(rest.len());
            let rest = &rest[start..];
            let n = rest.len().min(cmd.len() - 1);
            cmd[..n].copy_from_slice(&rest[..n]);
            // The attribute's value: a block whose first field is the
            // session (what kernel32 reads from an HPCON).
            let hpcon = [u64::from(session), 0, 0, 0];
            let mut list = [0u64; 64];
            let list = list.as_mut_ptr().cast::<u8>();
            let mut list_size = 512usize;
            let mut startup = [0u8; 112];
            startup[0..4].copy_from_slice(&112u32.to_le_bytes());
            startup[104..112].copy_from_slice(&(list as u64).to_le_bytes());
            let mut info = [0usize; 3];
            if InitializeProcThreadAttributeList(list, 1, 0, &mut list_size) == 0
                || UpdateProcThreadAttribute(
                    list,
                    0,
                    PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE,
                    hpcon.as_ptr().cast(),
                    8,
                    core::ptr::null_mut(),
                    core::ptr::null_mut(),
                ) == 0
                || CreateProcessW(
                    core::ptr::null(),
                    cmd.as_mut_ptr(),
                    core::ptr::null(),
                    core::ptr::null(),
                    0,
                    EXTENDED_STARTUPINFO_PRESENT,
                    core::ptr::null(),
                    core::ptr::null(),
                    startup.as_ptr(),
                    &mut info,
                ) == 0
            {
                let error = GetLastError();
                let mut note = *b"cannot start the command: error 0000000000";
                let mut e = error;
                for d in note.iter_mut().rev().take(10) {
                    *d = b'0' + (e % 10) as u8;
                    e /= 10;
                }
                trace(2, &note);
                GalaConsoleClose(session);
                return error.max(1);
            }
            child = info[0] as Handle;
            CloseHandle(info[1] as Handle);
        }
        let thread =
            CreateThread(core::ptr::null(), 0, input_thread, input, 0, core::ptr::null_mut());
        if !thread.is_null() {
            CloseHandle(thread);
        }
        if o.signal != 0 {
            let signal = o.signal as Handle;
            let thread =
                CreateThread(core::ptr::null(), 0, signal_thread, signal, 0, core::ptr::null_mut());
            if !thread.is_null() {
                CloseHandle(thread);
            }
        }
        // The output, until the console closes (or the command ends).
        let waits = [event, child];
        let count = if child.is_null() { 1 } else { 2 };
        let mut child_done = false;
        loop {
            let which = WaitForMultipleObjects(count, waits.as_ptr(), 0, INFINITE);
            // Everything written so far goes to the terminal.
            loop {
                let mut buf = [0u8; 2048];
                let mut got = 0u32;
                let status = GalaConsoleRead(session, buf.as_mut_ptr(), 2048, &mut got);
                if status != 0 {
                    // Closed, and its last output carried.
                    return exit_code(child);
                }
                if got == 0 {
                    break;
                }
                if !to_terminal(output, &buf[..got as usize]) {
                    // The terminal is gone.
                    GalaConsoleClose(session);
                    return exit_code(child);
                }
            }
            if which == 1 && !child_done {
                // The command ended: the console closes with it.
                child_done = true;
                GalaConsoleClose(session);
            }
            if which == 0xffff_ffff {
                return 3;
            }
        }
    }
}

/// The command's exit code (0 without one).
fn exit_code(child: Handle) -> u32 {
    if child.is_null() {
        return 0;
    }
    let mut code = 0u32;
    // SAFETY: our process handle.
    unsafe { GetExitCodeProcess(child, &mut code) };
    code
}

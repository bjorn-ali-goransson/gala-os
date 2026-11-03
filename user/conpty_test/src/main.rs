//! Pseudo consoles (ConPTY), as Microsoft's "Creating a Pseudoconsole
//! session" documents them: the parent makes two pipes, a pseudo console
//! on them (CreatePseudoConsole, 80 x 25), and starts itself as a child
//! console program attached to it (PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE in
//! a STARTUPINFOEX, EXTENDED_STARTUPINFO_PRESENT). It then plays the
//! terminal: it writes keystrokes (bytes, VT sequences for special keys)
//! into the input pipe and reads the console's output (UTF-8 text and VT
//! sequences) from the output pipe on a thread of its own, as the
//! documentation says a terminal must (the pipe must be drained).
//!
//! The checks (each failure ends with its number; what the output pipe
//! carried so far is printed escaped):
//! 1-3. the pipes, CreatePseudoConsole (S_OK), the child started;
//! 4. the child is attached: its standard input is a console
//!    (GetConsoleMode) and it says `child: ready`;
//! 5. line editing in the console's cooked input: "hellx", DEL (what a
//!    terminal sends for Backspace), "o", CR: the child's ReadFile gets
//!    "hello" (`got:hello`) and the echo shows on the output;
//! 6. the child sees the console's size (GetConsoleScreenBufferInfo's
//!    window): 80 x 25;
//! 7. after ResizePseudoConsole to 100 x 30 it sees 100 x 30 (asked again
//!    until the resize has reached the console: it is asynchronous);
//! 8. raw input (SetConsoleMode 0, ReadConsoleInputW): ESC [ A and ESC [ D
//!    arrive as key events VK_UP (0x26) and VK_LEFT (0x25), DEL as VK_BACK;
//! 9. Ctrl+C (0x03) with processed input calls the child's control handler
//!    (SetConsoleCtrlHandler) and the child goes on;
//! 10-11. ClosePseudoConsole ends the child (it waits forever), and the
//!    output pipe then ends (ERROR_BROKEN_PIPE).
//!
//! Expected output text is matched as substrings: Windows' console host
//! renders its screen into VT sequences of its own (cursor moves, colour
//! resets), which are not checked.

#![no_std]
#![no_main]

use core::ffi::c_void;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

type Handle = *mut c_void;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut c_void) -> i32;
    fn ReadFile(h: Handle, buf: *mut u8, len: u32, read: *mut u32, ov: *mut c_void) -> i32;
    fn ExitProcess(code: u32) -> !;
    fn Sleep(ms: u32);
    fn GetTickCount() -> u32;
    fn GetLastError() -> u32;
    fn CloseHandle(h: Handle) -> i32;
    fn CreatePipe(read: *mut Handle, write: *mut Handle, sa: *const c_void, size: u32) -> i32;
    fn CreatePseudoConsole(
        size: u32,
        input: Handle,
        output: Handle,
        flags: u32,
        pc: *mut usize,
    ) -> i32;
    fn ResizePseudoConsole(pc: usize, size: u32) -> i32;
    fn ClosePseudoConsole(pc: usize);
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
    fn DeleteProcThreadAttributeList(list: *mut u8);
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
    fn GetModuleFileNameW(module: Handle, buf: *mut u16, size: u32) -> u32;
    fn GetCommandLineW() -> *const u16;
    fn CreateThread(
        sa: *const c_void,
        stack: usize,
        start: extern "system" fn(*mut c_void) -> u32,
        arg: *mut c_void,
        flags: u32,
        id: *mut u32,
    ) -> Handle;
    fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
    fn GetConsoleMode(h: Handle, mode: *mut u32) -> i32;
    fn SetConsoleMode(h: Handle, mode: u32) -> i32;
    fn GetConsoleScreenBufferInfo(h: Handle, info: *mut [i16; 11]) -> i32;
    fn ReadConsoleInputW(h: Handle, records: *mut [u16; 10], n: u32, read: *mut u32) -> i32;
    fn SetConsoleCtrlHandler(handler: Option<extern "system" fn(u32) -> i32>, add: i32) -> i32;
}

#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

#[unsafe(no_mangle)]
pub extern "C" fn __CxxFrameHandler3() -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdeac) }
}

#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
}

const PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE: usize = 0x0002_0016;
const EXTENDED_STARTUPINFO_PRESENT: u32 = 0x0008_0000;
const ERROR_BROKEN_PIPE: u32 = 109;
const INFINITE: u32 = 0xffff_ffff;

fn write_to(h: Handle, text: &[u8]) {
    let mut written = 0;
    // SAFETY: valid buffer and output pointer.
    unsafe { WriteFile(h, text.as_ptr(), text.len() as u32, &mut written, core::ptr::null_mut()) };
}

/// A line to the program's own standard output (one WriteFile).
fn out(text: &[u8]) {
    // SAFETY: a plain call.
    write_to(unsafe { GetStdHandle(-11i32 as u32) }, text);
}

/// `prefix` and a decimal number, then `suffix`, in one write to `h`.
fn write_number(h: Handle, prefix: &[u8], n: u32, suffix: &[u8]) {
    let mut line = [0u8; 96];
    let mut at = 0;
    for &b in prefix.iter().take(60) {
        line[at] = b;
        at += 1;
    }
    let mut digits = [0u8; 10];
    let mut d = 0;
    let mut v = n;
    loop {
        digits[d] = b'0' + (v % 10) as u8;
        d += 1;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    while d > 0 {
        d -= 1;
        line[at] = digits[d];
        at += 1;
    }
    for &b in suffix.iter().take(20) {
        line[at] = b;
        at += 1;
    }
    write_to(h, &line[..at]);
}

// ---- The child: a console program in the pseudo console ----

extern "system" fn on_ctrl(event: u32) -> i32 {
    if event == 0 {
        // SAFETY: a plain call.
        write_to(unsafe { GetStdHandle(-11i32 as u32) }, b"ctrl:handled\n");
        return 1;
    }
    0
}

/// Reads lines (cooked input) and answers them; `raw` reads key events
/// until a `q`, `wait` waits forever (for ClosePseudoConsole).
fn child() -> ! {
    // SAFETY: plain calls on our standard handles and buffers.
    unsafe {
        let stdin = GetStdHandle(-10i32 as u32);
        let stdout = GetStdHandle(-11i32 as u32);
        let mut mode = 0u32;
        let mut info = [0i16; 11];
        if GetConsoleMode(stdin, &mut mode) == 0
            || mode & 7 != 7
            || GetConsoleScreenBufferInfo(stdout, &mut info) == 0
            || SetConsoleCtrlHandler(Some(on_ctrl), 1) == 0
        {
            write_number(stdout, b"child: not a console, error ", GetLastError(), b"\n");
            ExitProcess(2);
        }
        write_to(stdout, b"child: ready\n");
        let mut empty = 0;
        loop {
            let mut line = [0u8; 128];
            let mut got = 0u32;
            let ok = ReadFile(stdin, line.as_mut_ptr(), 128, &mut got, core::ptr::null_mut());
            if ok == 0 || got == 0 {
                // Ctrl+C may end a read early; a console never ends.
                empty += 1;
                if empty > 100 {
                    ExitProcess(3);
                }
                Sleep(10);
                continue;
            }
            empty = 0;
            let mut n = got as usize;
            while n > 0 && (line[n - 1] == b'\r' || line[n - 1] == b'\n') {
                n -= 1;
            }
            let text = &line[..n];
            if text == b"size" {
                GetConsoleScreenBufferInfo(stdout, &mut info);
                // srWindow: Left, Top, Right, Bottom at words 5..9.
                let w = (info[7] - info[5] + 1) as u32;
                let h = (info[8] - info[6] + 1) as u32;
                let mut l = [0u8; 40];
                l[..5].copy_from_slice(b"size:");
                let mut at = 5;
                for (i, v) in [w, h].into_iter().enumerate() {
                    if i == 1 {
                        l[at] = b'x';
                        at += 1;
                    }
                    if v >= 100 {
                        l[at] = b'0' + (v / 100 % 10) as u8;
                        at += 1;
                    }
                    l[at] = b'0' + (v / 10 % 10) as u8;
                    l[at + 1] = b'0' + (v % 10) as u8;
                    at += 2;
                }
                l[at] = b'\n';
                write_to(stdout, &l[..at + 1]);
            } else if text == b"raw" {
                SetConsoleMode(stdin, 0);
                write_to(stdout, b"raw:on\n");
                'keys: loop {
                    let mut records = [[0u16; 10]; 8];
                    let mut n = 0u32;
                    if ReadConsoleInputW(stdin, records.as_mut_ptr(), 8, &mut n) == 0 {
                        write_to(stdout, b"raw:failed\n");
                        ExitProcess(4);
                    }
                    for r in &records[..n as usize] {
                        // KEY_EVENT: bKeyDown (u32 at word 2), wVirtualKeyCode
                        // at word 5, the character at word 7.
                        if r[0] != 1 || r[2] == 0 {
                            continue;
                        }
                        if r[7] == u16::from(b'q') {
                            break 'keys;
                        }
                        let vk = u32::from(r[5]);
                        let hex =
                            |d: u32| if d < 10 { b'0' + d as u8 } else { b'a' + d as u8 - 10 };
                        let line = [
                            b'r',
                            b'a',
                            b'w',
                            b':',
                            b'v',
                            b'k',
                            b'=',
                            hex(vk >> 4 & 15),
                            hex(vk & 15),
                            b'\n',
                        ];
                        write_to(stdout, &line);
                    }
                }
                SetConsoleMode(stdin, mode);
                write_to(stdout, b"raw:off\n");
            } else if text == b"wait" {
                write_to(stdout, b"child: waiting\n");
                Sleep(INFINITE);
            } else {
                let mut l = [0u8; 140];
                l[..4].copy_from_slice(b"got:");
                l[4..4 + n].copy_from_slice(text);
                l[4 + n] = b'\n';
                write_to(stdout, &l[..5 + n]);
            }
        }
    }
}

// ---- The parent: the terminal ----

/// Everything the output pipe carried (the reader thread appends).
static mut SEEN: [u8; 65536] = [0; 65536];
static SEEN_LEN: AtomicUsize = AtomicUsize::new(0);
/// The reader's end: the pipe broke (true) after the pseudo console closed.
static READER_DONE: AtomicBool = AtomicBool::new(false);
static READER_BROKEN: AtomicBool = AtomicBool::new(false);

extern "system" fn reader(pipe: *mut c_void) -> u32 {
    loop {
        let mut buf = [0u8; 1024];
        let mut got = 0u32;
        // SAFETY: our buffer; the output pipe's read end.
        let ok = unsafe { ReadFile(pipe, buf.as_mut_ptr(), 1024, &mut got, core::ptr::null_mut()) };
        if ok == 0 {
            // SAFETY: a plain call.
            READER_BROKEN.store(unsafe { GetLastError() } == ERROR_BROKEN_PIPE, Ordering::Release);
            READER_DONE.store(true, Ordering::Release);
            return 0;
        }
        let at = SEEN_LEN.load(Ordering::Relaxed);
        let n = (got as usize).min(65536 - at);
        // SAFETY: only this thread writes SEEN, past what readers read.
        unsafe {
            let seen = (&raw mut SEEN).cast::<u8>();
            core::ptr::copy_nonoverlapping(buf.as_ptr(), seen.add(at), n);
        }
        SEEN_LEN.store(at + n, Ordering::Release);
    }
}

fn seen() -> &'static [u8] {
    let n = SEEN_LEN.load(Ordering::Acquire);
    // SAFETY: the first `n` bytes are written and no longer change.
    unsafe { core::slice::from_raw_parts((&raw const SEEN).cast::<u8>(), n) }
}

fn find(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    (from..hay.len().saturating_sub(needle.len() - 1)).find(|&i| hay[i..].starts_with(needle))
}

/// Waits up to `ms` for `text` in the output after offset `from`; where
/// it ends.
fn wait_for(text: &[u8], from: usize, ms: u32) -> Option<usize> {
    // SAFETY: a plain call.
    let start = unsafe { GetTickCount() };
    loop {
        if let Some(at) = find(seen(), text, from) {
            return Some(at + text.len());
        }
        // SAFETY: plain calls.
        if unsafe { GetTickCount() }.wrapping_sub(start) > ms {
            return None;
        }
        // SAFETY: a plain call.
        unsafe { Sleep(20) };
    }
}

/// Ends with check `n`'s failure, showing the output's last bytes escaped.
fn fail(n: u32) -> ! {
    let all = seen();
    let tail = &all[all.len().saturating_sub(600)..];
    let mut line = [0u8; 2600];
    let mut at = 0;
    for &b in b"conpty_test: output was: " {
        line[at] = b;
        at += 1;
    }
    for &b in tail {
        let hex = |d: u8| if d < 10 { b'0' + d } else { b'a' + d - 10 };
        if (0x20..0x7f).contains(&b) && b != b'\\' {
            line[at] = b;
            at += 1;
        } else {
            line[at..at + 4].copy_from_slice(&[b'\\', b'x', hex(b >> 4), hex(b & 15)]);
            at += 4;
        }
    }
    line[at] = b'\n';
    out(&line[..at + 1]);
    // SAFETY: plain calls.
    unsafe {
        write_number(GetStdHandle(-11i32 as u32), b"conpty_test: failed check ", n, b"\n");
        ExitProcess(n)
    }
}

fn coord(x: u16, y: u16) -> u32 {
    u32::from(x) | (u32::from(y) << 16)
}

fn parent() -> ! {
    // SAFETY: our handles, buffers and structures, laid out as the
    // documented ones (STARTUPINFOEXW: 104 + 8 bytes).
    unsafe {
        let (mut in_read, mut in_write) = (core::ptr::null_mut(), core::ptr::null_mut());
        let (mut out_read, mut out_write) = (core::ptr::null_mut(), core::ptr::null_mut());
        if CreatePipe(&mut in_read, &mut in_write, core::ptr::null(), 0) == 0
            || CreatePipe(&mut out_read, &mut out_write, core::ptr::null(), 0) == 0
        {
            fail(1);
        }
        let mut pc = 0usize;
        let hr = CreatePseudoConsole(coord(80, 25), in_read, out_write, 0, &mut pc);
        if hr != 0 || pc == 0 {
            write_number(
                GetStdHandle(-11i32 as u32),
                b"conpty_test: CreatePseudoConsole ",
                hr as u32,
                b"\n",
            );
            fail(2);
        }
        // The console host has its own copies now.
        CloseHandle(in_read);
        CloseHandle(out_write);
        let thread = CreateThread(core::ptr::null(), 0, reader, out_read, 0, core::ptr::null_mut());
        if thread.is_null() {
            fail(3);
        }

        // The child, attached to the pseudo console.
        let mut size = 0usize;
        InitializeProcThreadAttributeList(core::ptr::null_mut(), 1, 0, &mut size);
        let mut list = [0u64; 64];
        let list = list.as_mut_ptr().cast::<u8>();
        if size > 512
            || InitializeProcThreadAttributeList(list, 1, 0, &mut size) == 0
            || UpdateProcThreadAttribute(
                list,
                0,
                PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE,
                pc as *const c_void,
                8,
                core::ptr::null_mut(),
                core::ptr::null_mut(),
            ) == 0
        {
            fail(3);
        }
        let mut startup = [0u8; 112];
        startup[0..4].copy_from_slice(&112u32.to_le_bytes());
        startup[104..112].copy_from_slice(&(list as u64).to_le_bytes());
        let mut exe = [0u16; 300];
        let n = GetModuleFileNameW(core::ptr::null_mut(), exe.as_mut_ptr(), 260) as usize;
        let mut cmd = [0u16; 320];
        cmd[0] = u16::from(b'"');
        cmd[1..1 + n].copy_from_slice(&exe[..n]);
        for (i, &b) in b"\" child".iter().enumerate() {
            cmd[1 + n + i] = u16::from(b);
        }
        let mut info = [0usize; 3];
        let ok = CreateProcessW(
            exe.as_ptr(),
            cmd.as_mut_ptr(),
            core::ptr::null(),
            core::ptr::null(),
            0,
            EXTENDED_STARTUPINFO_PRESENT,
            core::ptr::null(),
            core::ptr::null(),
            startup.as_ptr(),
            &mut info,
        );
        DeleteProcThreadAttributeList(list);
        if ok == 0 {
            write_number(
                GetStdHandle(-11i32 as u32),
                b"conpty_test: CreateProcessW error ",
                GetLastError(),
                b"\n",
            );
            fail(3);
        }
        let child = info[0] as Handle;
        CloseHandle(info[1] as Handle);
        out(b"conpty_test: child started\n");

        let key = |bytes: &[u8]| write_to(in_write, bytes);
        let Some(mut at) = wait_for(b"child: ready", 0, 20000) else { fail(4) };
        out(b"conpty_test: attached\n");

        // Line editing: DEL is the terminal's Backspace.
        key(b"hellx\x7fo\r");
        let Some(echo) = wait_for(b"hell", at, 10000) else { fail(5) };
        let Some(next) = wait_for(b"got:hello", echo, 10000) else { fail(5) };
        at = next;
        out(b"conpty_test: line edited\n");

        key(b"size\r");
        let Some(next) = wait_for(b"size:80x25", at, 10000) else { fail(6) };
        at = next;
        if ResizePseudoConsole(pc, coord(100, 30)) != 0 {
            fail(7);
        }
        let mut resized = None;
        for _ in 0..20 {
            key(b"size\r");
            if let Some(next) = wait_for(b"size:100x30", at, 500) {
                resized = Some(next);
                break;
            }
        }
        let Some(next) = resized else { fail(7) };
        at = next;
        out(b"conpty_test: resized\n");

        key(b"raw\r");
        let Some(next) = wait_for(b"raw:on", at, 10000) else { fail(8) };
        key(b"\x1b[A");
        let Some(next) = wait_for(b"raw:vk=26", next, 10000) else { fail(8) };
        key(b"\x1b[D");
        let Some(next) = wait_for(b"raw:vk=25", next, 10000) else { fail(8) };
        key(b"\x7f");
        let Some(next) = wait_for(b"raw:vk=08", next, 10000) else { fail(8) };
        key(b"q");
        let Some(next) = wait_for(b"raw:off", next, 10000) else { fail(8) };
        at = next;
        out(b"conpty_test: keys\n");

        key(b"\x03");
        let Some(next) = wait_for(b"ctrl:handled", at, 10000) else { fail(9) };
        key(b"again\r");
        let Some(next) = wait_for(b"got:again", next, 10000) else { fail(9) };
        at = next;
        out(b"conpty_test: ctrl-c\n");

        key(b"wait\r");
        if wait_for(b"child: waiting", at, 10000).is_none() {
            fail(10);
        }
        ClosePseudoConsole(pc);
        if WaitForSingleObject(child, 15000) != 0 {
            fail(10);
        }
        out(b"conpty_test: closed\n");
        if WaitForSingleObject(thread, 15000) != 0
            || !READER_DONE.load(Ordering::Acquire)
            || !READER_BROKEN.load(Ordering::Acquire)
        {
            fail(11);
        }
        CloseHandle(in_write);
        out(b"conpty_test: ok\n");
        ExitProcess(0)
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    // The child is started with the argument `child`.
    // SAFETY: the process's command line, NUL-terminated.
    let line = unsafe {
        let p = GetCommandLineW();
        let mut n = 0;
        while *p.add(n) != 0 {
            n += 1;
        }
        core::slice::from_raw_parts(p, n)
    };
    let is_child = line.len() >= 5
        && line[line.len() - 5..].iter().zip(b"child").all(|(&a, &b)| a == u16::from(b));
    if is_child { child() } else { parent() }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdead) }
}

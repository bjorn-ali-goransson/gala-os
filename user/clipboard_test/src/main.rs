//! The clipboard across two processes, with the results real Windows
//! gives ("Using the Clipboard", "Synthesized Clipboard Formats",
//! AddClipboardFormatListener): the clipboard is the window station's,
//! not a process's.
//!
//! The parent makes a window, puts it on the format listener list and,
//! with the clipboard open on it, empties it (its window becomes the
//! owner, GetOpenClipboardWindow names it) and sets CF_UNICODETEXT from a
//! moveable HGLOBAL (GetClipboardData gives the same handle back while it
//! is open). After CloseClipboard the sequence number has grown and its
//! window gets WM_CLIPBOARDUPDATE. It starts itself as a child
//! (`clipboard_test child <events> <window> <sequence>`), which sees the
//! parent's window as the owner and a sequence number at least the
//! parent's, reads CF_UNICODETEXT and the synthesized CF_TEXT and
//! CF_LOCALE (four formats, enumerated in Windows' order: the one set,
//! CF_LOCALE, then the synthesized text formats), then empties the
//! clipboard and sets CF_TEXT from its own window. The parent's window
//! gets WM_CLIPBOARDUPDATE, the owner is the child's window, and the
//! parent reads CF_UNICODETEXT synthesized from the child's CF_TEXT. While
//! one process has the clipboard open the other's OpenClipboard fails
//! with ERROR_ACCESS_DENIED and GetOpenClipboardWindow names the holder's
//! window. CloseClipboard, EmptyClipboard and GetClipboardData without
//! the clipboard open fail (ERROR_CLIPBOARD_NOT_OPEN for the first two).
//!
//! Assumed: the listener hears changes its own process made too (Windows
//! documents no exception). The test replaces the user's clipboard
//! contents. Off gala another process (clipboard history, a remote
//! desktop's clipboard agent) may hold the clipboard for a moment after
//! each change: an OpenClipboard that is to succeed is retried for up to
//! 2 s while it fails with ERROR_ACCESS_DENIED, and if the clipboard is
//! held at the start the test says `skipped`, naming the holder's
//! process.
//!
//! Prints `clipboard_test: ok` or exits with the number of the failing
//! check (the child's report starts `clipboard_test: child`).

#![no_std]
#![no_main]

use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, Ordering};

type Bool = i32;
type Handle = *mut c_void;
type Hwnd = usize;

#[repr(C)]
struct SecurityAttributes {
    length: u32,
    descriptor: *mut c_void,
    inherit: Bool,
}

#[repr(C)]
struct StartupInfo {
    cb: u32,
    rest: [u8; 100],
}

#[repr(C)]
struct ProcessInformation {
    process: Handle,
    thread: Handle,
    process_id: u32,
    thread_id: u32,
}

#[repr(C)]
struct WndClassW {
    style: u32,
    proc_: extern "system" fn(Hwnd, u32, usize, isize) -> isize,
    cls_extra: i32,
    wnd_extra: i32,
    instance: Handle,
    icon: Handle,
    cursor: Handle,
    background: Handle,
    menu_name: *const u16,
    class_name: *const u16,
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut c_void) -> Bool;
    fn GetCommandLineW() -> *const u16;
    fn GetModuleFileNameW(module: Handle, buf: *mut u16, size: u32) -> u32;
    fn GetModuleHandleW(name: *const u16) -> Handle;
    fn GetLastError() -> u32;
    fn SetLastError(code: u32);
    fn CreateEventW(
        attributes: *const SecurityAttributes,
        manual: Bool,
        initial: Bool,
        name: *const u16,
    ) -> Handle;
    fn SetEvent(h: Handle) -> Bool;
    fn CreateProcessW(
        application: *const u16,
        command_line: *mut u16,
        process_attributes: *const c_void,
        thread_attributes: *const c_void,
        inherit: Bool,
        flags: u32,
        environment: *const c_void,
        directory: *const u16,
        startup: *const StartupInfo,
        info: *mut ProcessInformation,
    ) -> Bool;
    fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
    fn WaitForMultipleObjects(count: u32, handles: *const Handle, all: Bool, ms: u32) -> u32;
    fn GetExitCodeProcess(h: Handle, code: *mut u32) -> Bool;
    fn GetTickCount() -> u32;
    fn Sleep(ms: u32);
    fn GlobalAlloc(flags: u32, bytes: usize) -> Handle;
    fn GlobalLock(mem: Handle) -> *mut c_void;
    fn GlobalUnlock(mem: Handle) -> Bool;
    fn GlobalSize(mem: Handle) -> usize;
    fn ExitProcess(code: u32) -> !;
}

#[link(name = "user32", kind = "raw-dylib")]
unsafe extern "system" {
    fn RegisterClassW(class: *const WndClassW) -> u16;
    fn CreateWindowExW(
        ex_style: u32,
        class: *const u16,
        title: *const u16,
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
    fn PeekMessageW(msg: *mut [u64; 6], hwnd: Hwnd, min: u32, max: u32, remove: u32) -> Bool;
    fn TranslateMessage(msg: *const [u64; 6]) -> Bool;
    fn DispatchMessageW(msg: *const [u64; 6]) -> isize;
    fn GetWindowThreadProcessId(hwnd: Hwnd, pid: *mut u32) -> u32;
    fn OpenClipboard(owner: Hwnd) -> Bool;
    fn CloseClipboard() -> Bool;
    fn EmptyClipboard() -> Bool;
    fn SetClipboardData(format: u32, mem: Handle) -> Handle;
    fn GetClipboardData(format: u32) -> Handle;
    fn IsClipboardFormatAvailable(format: u32) -> Bool;
    fn CountClipboardFormats() -> i32;
    fn EnumClipboardFormats(format: u32) -> u32;
    fn GetClipboardSequenceNumber() -> u32;
    fn GetClipboardOwner() -> Hwnd;
    fn GetOpenClipboardWindow() -> Hwnd;
    fn AddClipboardFormatListener(hwnd: Hwnd) -> Bool;
    fn RemoveClipboardFormatListener(hwnd: Hwnd) -> Bool;
}

#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memmove(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
    fn __chkstk();
}

#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

#[unsafe(no_mangle)]
pub extern "C" fn __CxxFrameHandler3() -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdeac) }
}

const CF_TEXT: u32 = 1;
const CF_OEMTEXT: u32 = 7;
const CF_UNICODETEXT: u32 = 13;
const CF_LOCALE: u32 = 16;
const WM_CLIPBOARDUPDATE: u32 = 0x031d;
const GMEM_MOVEABLE: u32 = 2;
const ERROR_ACCESS_DENIED: u32 = 5;
const ERROR_CLIPBOARD_NOT_OPEN: u32 = 1418;
const PM_REMOVE: u32 = 1;

const PARENT_TEXT: &str = "gala clipboard: from the parent";
const CHILD_TEXT: &str = "gala clipboard: from the child";

/// WM_CLIPBOARDUPDATE messages the parent's window got.
static UPDATES: AtomicU32 = AtomicU32::new(0);

extern "system" fn window_proc(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize {
    if msg == WM_CLIPBOARDUPDATE {
        UPDATES.fetch_add(1, Ordering::SeqCst);
        return 0;
    }
    // SAFETY: the system's default handling.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
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

fn hex(v: u32) -> [u8; 8] {
    let mut s = [0u8; 8];
    for (i, c) in s.iter_mut().enumerate() {
        let d = (v >> (28 - 4 * i)) & 0xf;
        *c = if d < 10 { b'0' + d as u8 } else { b'a' + d as u8 - 10 };
    }
    s
}

/// ` (line L)`, built in `b` from position `at`; the new end.
fn put_line(b: &mut [u8], mut at: usize, line: u32) -> usize {
    let mut digits = [0u8; 10];
    let (mut n, mut len) = (line, 0);
    loop {
        digits[len] = b'0' + (n % 10) as u8;
        len += 1;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    for &c in b" (line " {
        b[at] = c;
        at += 1;
    }
    for i in 0..len {
        b[at] = digits[len - 1 - i];
        at += 1;
    }
    b[at] = b')';
    at + 1
}

/// The checks of one process (`who`: "" or "child "); a failure is
/// reported in one write (`clipboard_test: <who>check N got X (line L)`).
struct Checks {
    n: u32,
    who: &'static [u8],
}

impl Checks {
    #[track_caller]
    fn ok(&mut self, cond: bool, got: u64) -> Result<(), u32> {
        self.n += 1;
        if cond {
            return Ok(());
        }
        let mut b = [0u8; 128];
        let mut at = 0;
        for part in [&b"clipboard_test: "[..], self.who, b"check ", &hex(self.n), b" got "] {
            b[at..at + part.len()].copy_from_slice(part);
            at += part.len();
        }
        b[at..at + 8].copy_from_slice(&hex(got as u32));
        at = put_line(&mut b, at + 8, core::panic::Location::caller().line());
        b[at..at + 2].copy_from_slice(b"\r\n");
        out(&b[..at + 2]);
        Err(self.n)
    }

    #[track_caller]
    fn eq(&mut self, got: u64, want: u64) -> Result<(), u32> {
        self.ok(got == want, got)
    }
}

/// `text` as UTF-16 with a NUL.
fn wide<const N: usize>(text: &str) -> [u16; N] {
    let mut w = [0u16; N];
    for (d, b) in w.iter_mut().zip(text.bytes()) {
        *d = u16::from(b);
    }
    w
}

/// A moveable HGLOBAL holding `bytes`.
fn global(bytes: &[u8]) -> Handle {
    // SAFETY: documented calls; the block holds `bytes.len()` bytes.
    unsafe {
        let h = GlobalAlloc(GMEM_MOVEABLE, bytes.len());
        if h.is_null() {
            return h;
        }
        let p = GlobalLock(h).cast::<u8>();
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
        GlobalUnlock(h);
        h
    }
}

/// Whether HGLOBAL `h` holds `text` with a NUL, as UTF-16 (`unicode`) or
/// 8-bit characters, within its GlobalSize.
fn holds(h: Handle, text: &str, unicode: bool) -> bool {
    if h.is_null() {
        return false;
    }
    let unit = if unicode { 2 } else { 1 };
    // SAFETY: a clipboard HGLOBAL, read within its size while locked.
    unsafe {
        if GlobalSize(h) < (text.len() + 1) * unit {
            return false;
        }
        let p = GlobalLock(h).cast::<u8>();
        if p.is_null() {
            return false;
        }
        let at = |i: usize| -> u16 {
            if unicode { p.cast::<u16>().add(i).read_unaligned() } else { u16::from(*p.add(i)) }
        };
        let same =
            text.bytes().enumerate().all(|(i, b)| at(i) == u16::from(b)) && at(text.len()) == 0;
        GlobalUnlock(h);
        same
    }
}

/// The formats EnumClipboardFormats lists (the clipboard open), at most 8.
fn formats() -> ([u32; 8], usize) {
    let (mut list, mut n, mut f) = ([0u32; 8], 0, 0);
    loop {
        // SAFETY: plain call.
        f = unsafe { EnumClipboardFormats(f) };
        if f == 0 || n == list.len() {
            return (list, n);
        }
        list[n] = f;
        n += 1;
    }
}

/// OpenClipboard(`hwnd`) that is to succeed: retried for up to 2 s while
/// another process holds the clipboard (off gala, clipboard history and
/// remote-desktop agents open it after each change). The last error of
/// the last try when it never succeeds.
fn open(hwnd: Hwnd) -> Result<(), u32> {
    // SAFETY: plain calls.
    unsafe {
        let start = GetTickCount();
        loop {
            if OpenClipboard(hwnd) != 0 {
                return Ok(());
            }
            let e = GetLastError();
            if e != ERROR_ACCESS_DENIED || GetTickCount().wrapping_sub(start) > 2000 {
                return Err(e);
            }
            Sleep(20);
        }
    }
}

/// Dispatches the thread's queued messages.
fn pump() {
    let mut msg = [0u64; 6];
    // SAFETY: our MSG buffer.
    unsafe {
        while PeekMessageW(&mut msg, 0, 0, 0, PM_REMOVE) != 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// Waits up to 10 s, dispatching messages, until `event` is set (true)
/// or `process` ends first (false).
fn wait_pumping(event: Handle, process: Handle) -> bool {
    let handles = [event, process];
    // SAFETY: our handles.
    unsafe {
        let start = GetTickCount();
        while GetTickCount().wrapping_sub(start) < 10_000 {
            match WaitForMultipleObjects(2, handles.as_ptr(), 0, 10) {
                0 => return true,
                1 => return false,
                _ => pump(),
            }
        }
    }
    false
}

/// Dispatches messages until UPDATES is above `seen` (true), or 5 s pass.
fn wait_update(seen: u32) -> bool {
    // SAFETY: plain calls.
    let start = unsafe { GetTickCount() };
    loop {
        pump();
        if UPDATES.load(Ordering::SeqCst) > seen {
            return true;
        }
        // SAFETY: plain calls.
        if unsafe { GetTickCount().wrapping_sub(start) } > 5000 {
            return false;
        }
        // SAFETY: plain call.
        unsafe { Sleep(10) };
    }
}

/// A window of class `class` (registered here) with window procedure
/// `window_proc`, not shown.
fn make_window(class: &[u16]) -> Hwnd {
    // SAFETY: our class description and names.
    unsafe {
        let instance = GetModuleHandleW(core::ptr::null());
        let wc = WndClassW {
            style: 0,
            proc_: window_proc,
            cls_extra: 0,
            wnd_extra: 0,
            instance,
            icon: core::ptr::null_mut(),
            cursor: core::ptr::null_mut(),
            background: core::ptr::null_mut(),
            menu_name: core::ptr::null(),
            class_name: class.as_ptr(),
        };
        RegisterClassW(&wc);
        CreateWindowExW(
            0,
            class.as_ptr(),
            class.as_ptr(),
            0,
            0,
            0,
            200,
            100,
            0,
            core::ptr::null_mut(),
            instance,
            core::ptr::null_mut(),
        )
    }
}

/// Off gala, at the start: if another process holds the clipboard, says
/// `clipboard_test: skipped (held by pid N)` and ends with 0.
fn skip_if_held() {
    // SAFETY: plain calls.
    unsafe {
        let holder = GetOpenClipboardWindow();
        let mut pid = 0;
        if holder != 0 {
            GetWindowThreadProcessId(holder, &mut pid);
        }
        let mut b = [0u8; 64];
        let head = b"clipboard_test: skipped (held by pid ";
        b[..head.len()].copy_from_slice(head);
        b[head.len()..head.len() + 8].copy_from_slice(&hex(pid));
        let end = head.len() + 8;
        b[end..end + 3].copy_from_slice(b")\r\n");
        out(&b[..end + 3]);
        ExitProcess(0)
    }
}

fn run() -> Result<(), u32> {
    let mut c = Checks { n: 0, who: b"" };
    // SAFETY: documented calls with valid arguments.
    unsafe {
        let window = make_window(&wide::<32>("gala_clipboard_parent"));
        c.ok(window != 0, GetLastError().into())?;
        c.ok(AddClipboardFormatListener(window) != 0, GetLastError().into())?;

        // Without the clipboard open.
        SetLastError(0);
        c.ok(CloseClipboard() == 0, 0)?;
        c.eq(GetLastError().into(), ERROR_CLIPBOARD_NOT_OPEN.into())?;
        SetLastError(0);
        c.ok(EmptyClipboard() == 0, 0)?;
        c.eq(GetLastError().into(), ERROR_CLIPBOARD_NOT_OPEN.into())?;
        c.ok(GetClipboardData(CF_TEXT).is_null(), 0)?;

        // The parent's CF_UNICODETEXT.
        let before = GetClipboardSequenceNumber();
        match open(window) {
            Ok(()) => {}
            Err(ERROR_ACCESS_DENIED) if !on_gala() => skip_if_held(),
            Err(e) => c.ok(false, e.into())?,
        }
        c.eq(GetOpenClipboardWindow() as u64, window as u64)?;
        c.ok(EmptyClipboard() != 0, GetLastError().into())?;
        c.eq(GetClipboardOwner() as u64, window as u64)?;
        let mut bytes = [0u8; 128];
        for (i, b) in PARENT_TEXT.bytes().enumerate() {
            bytes[2 * i] = b;
        }
        let h = global(&bytes[..2 * (PARENT_TEXT.len() + 1)]);
        c.ok(!h.is_null(), GetLastError().into())?;
        c.eq(SetClipboardData(CF_UNICODETEXT, h) as u64, h as u64)?;
        c.ok(IsClipboardFormatAvailable(CF_UNICODETEXT) != 0, 0)?;
        // Its own handle back while it is open.
        c.eq(GetClipboardData(CF_UNICODETEXT) as u64, h as u64)?;
        let seen = UPDATES.load(Ordering::SeqCst);
        c.ok(CloseClipboard() != 0, GetLastError().into())?;
        let sequence = GetClipboardSequenceNumber();
        c.ok(sequence > before, sequence.into())?;
        // The listener hears the change (its own process's too).
        c.ok(wait_update(seen), UPDATES.load(Ordering::SeqCst).into())?;
        c.ok(IsClipboardFormatAvailable(CF_TEXT) != 0, 0)?;
        c.ok(IsClipboardFormatAvailable(CF_OEMTEXT) != 0, 0)?;
        c.eq(CountClipboardFormats() as u64, 4)?;

        // The child: `clipboard_test child <to child> <to parent> <window>
        // <sequence>`, the events inheritable.
        let inherit =
            SecurityAttributes { length: 24, descriptor: core::ptr::null_mut(), inherit: 1 };
        let to_child = CreateEventW(&inherit, 0, 0, core::ptr::null());
        let to_parent = CreateEventW(&inherit, 0, 0, core::ptr::null());
        c.ok(!to_child.is_null() && !to_parent.is_null(), GetLastError().into())?;
        let mut path = [0u16; 260];
        let n = GetModuleFileNameW(core::ptr::null_mut(), path.as_mut_ptr(), 260) as usize;
        c.ok(n > 0 && n < 260, n as u64)?;
        let mut line = [0u16; 400];
        let mut at = put(&mut line, 0, b"\"");
        line[at..at + n].copy_from_slice(&path[..n]);
        at += n;
        at = put(&mut line, at, b"\" child ");
        for (i, v) in [to_child as usize as u32, to_parent as usize as u32, window as u32, sequence]
            .into_iter()
            .enumerate()
        {
            if i > 0 {
                at = put(&mut line, at, b" ");
            }
            at = put(&mut line, at, &hex(v));
        }
        line[at] = 0;
        let startup = StartupInfo { cb: 104, rest: [0; 100] };
        let mut info = ProcessInformation {
            process: core::ptr::null_mut(),
            thread: core::ptr::null_mut(),
            process_id: 0,
            thread_id: 0,
        };
        let seen = UPDATES.load(Ordering::SeqCst);
        let started = CreateProcessW(
            path.as_ptr(),
            line.as_mut_ptr(),
            core::ptr::null(),
            core::ptr::null(),
            1,
            0,
            core::ptr::null(),
            core::ptr::null(),
            &startup,
            &mut info,
        );
        c.ok(started != 0, GetLastError().into())?;
        let child = info.process;

        // The child read ours and set CF_TEXT: the listener hears it.
        c.ok(wait_pumping(to_parent, child), 0)?;
        c.ok(wait_update(seen), UPDATES.load(Ordering::SeqCst).into())?;
        c.ok(GetClipboardSequenceNumber() > sequence, GetClipboardSequenceNumber().into())?;
        let owner = GetClipboardOwner();
        let mut pid = 0;
        c.ok(owner != 0 && owner != window, owner as u64)?;
        GetWindowThreadProcessId(owner, &mut pid);
        c.eq(pid.into(), info.process_id.into())?;
        // Its CF_TEXT, and CF_UNICODETEXT synthesized from it.
        c.ok(open(window).is_ok(), GetLastError().into())?;
        let (list, n) = formats();
        c.eq(n as u64, 4)?;
        c.ok(list[..4] == [CF_TEXT, CF_LOCALE, CF_OEMTEXT, CF_UNICODETEXT], list[1].into())?;
        c.ok(holds(GetClipboardData(CF_TEXT), CHILD_TEXT, false), 0)?;
        c.ok(holds(GetClipboardData(CF_UNICODETEXT), CHILD_TEXT, true), 0)?;
        c.ok(holds(GetClipboardData(CF_OEMTEXT), CHILD_TEXT, false), 0)?;

        // Held open here: the child's OpenClipboard fails.
        c.ok(SetEvent(to_child) != 0, GetLastError().into())?;
        c.ok(wait_pumping(to_parent, child), 0)?;
        c.ok(CloseClipboard() != 0, GetLastError().into())?;

        // Held open by the child: ours fails, and names its window.
        c.ok(SetEvent(to_child) != 0, GetLastError().into())?;
        c.ok(wait_pumping(to_parent, child), 0)?;
        SetLastError(0);
        c.ok(OpenClipboard(window) == 0, 0)?;
        c.eq(GetLastError().into(), ERROR_ACCESS_DENIED.into())?;
        c.eq(GetOpenClipboardWindow() as u64, owner as u64)?;
        c.ok(SetEvent(to_child) != 0, GetLastError().into())?;

        c.eq(WaitForSingleObject(child, 10_000).into(), 0)?;
        let mut code = 0;
        c.ok(GetExitCodeProcess(child, &mut code) != 0, GetLastError().into())?;
        c.eq(code.into(), 0)?;
        c.ok(RemoveClipboardFormatListener(window) != 0, GetLastError().into())?;
    }
    Ok(())
}

/// The child's part (see the module comment); the parent's window and
/// the sequence number it saw.
fn child_run(to_child: Handle, to_parent: Handle, parent: Hwnd, sequence: u32) -> Result<(), u32> {
    let mut c = Checks { n: 0, who: b"child " };
    // SAFETY: documented calls with valid arguments.
    unsafe {
        let window = make_window(&wide::<32>("gala_clipboard_child"));
        c.ok(window != 0, GetLastError().into())?;
        c.ok(GetClipboardSequenceNumber() >= sequence, GetClipboardSequenceNumber().into())?;
        c.eq(GetClipboardOwner() as u64, parent as u64)?;
        c.ok(open(window).is_ok(), GetLastError().into())?;
        // The parent's CF_UNICODETEXT, then CF_LOCALE and the synthesized
        // text formats.
        let (list, n) = formats();
        c.eq(n as u64, 4)?;
        c.ok(list[..4] == [CF_UNICODETEXT, CF_LOCALE, CF_TEXT, CF_OEMTEXT], list[1].into())?;
        c.eq(CountClipboardFormats() as u64, 4)?;
        c.ok(holds(GetClipboardData(CF_UNICODETEXT), PARENT_TEXT, true), 0)?;
        c.ok(holds(GetClipboardData(CF_TEXT), PARENT_TEXT, false), 0)?;
        let locale = GetClipboardData(CF_LOCALE);
        c.ok(!locale.is_null() && GlobalSize(locale) >= 4, 0)?;
        let lcid = GlobalLock(locale).cast::<u32>().read_unaligned();
        GlobalUnlock(locale);
        c.ok(lcid != 0, 0)?;
        // Ours replaces it.
        c.ok(EmptyClipboard() != 0, GetLastError().into())?;
        c.eq(GetClipboardOwner() as u64, window as u64)?;
        c.ok(IsClipboardFormatAvailable(CF_UNICODETEXT) == 0, 0)?;
        let mut bytes = [0u8; 64];
        bytes[..CHILD_TEXT.len()].copy_from_slice(CHILD_TEXT.as_bytes());
        let h = global(&bytes[..CHILD_TEXT.len() + 1]);
        c.eq(SetClipboardData(CF_TEXT, h) as u64, h as u64)?;
        let before = GetClipboardSequenceNumber();
        c.ok(before > sequence, before.into())?;
        c.ok(CloseClipboard() != 0, GetLastError().into())?;
        c.ok(SetEvent(to_parent) != 0, GetLastError().into())?;

        // The parent holds it open.
        c.eq(WaitForSingleObject(to_child, 10_000).into(), 0)?;
        SetLastError(0);
        c.ok(OpenClipboard(window) == 0, 0)?;
        c.eq(GetLastError().into(), ERROR_ACCESS_DENIED.into())?;
        c.eq(GetOpenClipboardWindow() as u64, parent as u64)?;
        c.ok(SetEvent(to_parent) != 0, GetLastError().into())?;

        // We hold it open while the parent tries.
        c.eq(WaitForSingleObject(to_child, 10_000).into(), 0)?;
        c.ok(open(window).is_ok(), GetLastError().into())?;
        c.ok(SetEvent(to_parent) != 0, GetLastError().into())?;
        c.eq(WaitForSingleObject(to_child, 10_000).into(), 0)?;
        c.ok(CloseClipboard() != 0, GetLastError().into())?;
    }
    Ok(())
}

/// Whether this runs on gala: no ntdll is loaded, or the loaded one is
/// gala's (it exports GalaConsoleCreate), as `ntrt::on_gala` decides.
fn on_gala() -> bool {
    #[link(name = "kernel32", kind = "raw-dylib")]
    unsafe extern "system" {
        fn GetProcAddress(module: Handle, name: *const u8) -> *mut c_void;
    }
    // SAFETY: NUL-terminated names.
    unsafe {
        let ntdll = GetModuleHandleW(wide::<6>("ntdll").as_ptr());
        ntdll.is_null() || !GetProcAddress(ntdll, c"GalaConsoleCreate".as_ptr().cast()).is_null()
    }
}

/// The command line's words after the program name, as UTF-16 slices.
fn arguments<'a>(line: &'a [u16], out: &mut [&'a [u16]; 6]) -> usize {
    // Skip the program name: quoted (it may contain spaces) or one word.
    let rest = if line.first() == Some(&u16::from(b'"')) {
        let close =
            line[1..].iter().position(|&c| c == u16::from(b'"')).map_or(line.len(), |i| i + 2);
        &line[close.min(line.len())..]
    } else {
        let end = line.iter().position(|&c| c == u16::from(b' ')).unwrap_or(line.len());
        &line[end..]
    };
    let mut n = 0;
    for w in rest.split(|&c| c == u16::from(b' ')).filter(|w| !w.is_empty()) {
        if n < out.len() {
            out[n] = w;
            n += 1;
        }
    }
    n
}

fn parse_hex(w: &[u16]) -> usize {
    let mut v = 0usize;
    for &c in w {
        v = v * 16 + (c as u8 as char).to_digit(16).unwrap_or(0) as usize;
    }
    v
}

/// UTF-16 of `text` into `buf` at `at`; the new end.
fn put(buf: &mut [u16], at: usize, text: &[u8]) -> usize {
    for (i, &b) in text.iter().enumerate() {
        buf[at + i] = u16::from(b);
    }
    at + text.len()
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    // SAFETY: the process's command line, NUL-terminated.
    let line = unsafe {
        let p = GetCommandLineW();
        let mut n = 0;
        while *p.add(n) != 0 {
            n += 1;
        }
        core::slice::from_raw_parts(p, n)
    };
    let mut args: [&[u16]; 6] = [&[]; 6];
    let argc = arguments(line, &mut args);
    let code = if argc == 5 && args[0] == b"child".map(u16::from) {
        let handle = |w: &[u16]| parse_hex(w) as Handle;
        match child_run(
            handle(args[1]),
            handle(args[2]),
            parse_hex(args[3]),
            parse_hex(args[4]) as u32,
        ) {
            Ok(()) => 0,
            Err(n) => n,
        }
    } else {
        match run() {
            Ok(()) => {
                out(b"clipboard_test: ok\r\n");
                0
            }
            Err(n) => n,
        }
    };
    // SAFETY: ends the process.
    unsafe { ExitProcess(code) }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdead) }
}

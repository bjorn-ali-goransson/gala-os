//! gala's `agent.exe` (ROADMAP LY "Change this program", step 4;
//! ARCHITECTURE "The agent panel"): the panel the user asks for changes
//! to the program they are using in. The desktop starts it with the
//! session when it is installed (`%SystemRoot%\System32\agent.exe`).
//!
//! - **Docked**: a popup window `WIDTH` wide on the screen's right edge,
//!   registered as an app bar (SHAppBarMessage ABM_NEW, ABM_QUERYPOS,
//!   ABM_SETPOS). It asks for the whole height and takes what SETPOS
//!   gives: from the top down to the taskbar, never over it ("App bars").
//!   The work area ends at its left edge; the taskbar keeps the whole
//!   width. ABN_POSCHANGED places it again.
//! - **Hidden** (Hide): the strip is given back (ABM_REMOVE) and the
//!   window is the dock tab, `TAB` in size, topmost at the middle of the
//!   right edge; a click on it docks the panel again. The state is kept
//!   in HKCU\Software\gala\Agent's `Hidden` (REG_DWORD) and taken at the
//!   next start.
//! - **Context**: the active window's program and title, from
//!   EVENT_SYSTEM_FOREGROUND (SetWinEventHook, out of context); the
//!   panel's own window does not change it. The hook is set before the
//!   first placement, so a window activated once the panel is docked is
//!   always its context.
//! - **Hand-off**: WM_COPYDATA of kind `handoff::KIND` from another
//!   program: a request (Explorer's "Make the change") or a process to
//!   ask about (Task Manager's "Ask the agent about this process"); the
//!   panel docks if hidden and lists it; any other kind, or a malformed
//!   one, is logged and not taken.
//! - **Chat**: a request sent is asked of the model on a worker thread
//!   (`ask`): `agent::chat::body` POSTed to the relay at `chat::HOST`
//!   over winhttp, loaded on the first ask (a panel that is never asked
//!   needs no network); the reply comes back to the window as
//!   WM_APP_ANSWER (wParam the request, lParam the boxed `Reply`).
//! - **Folders**: File Explorer's hand-off of the folder its window shows
//!   (`handoff::Folder`), kept by window: the working folder of a request
//!   sent while that window is the context.
//! - **Running**: a step the user clicked Run on runs on a worker thread
//!   (`run`): `"<System32>\cmd.exe" /c <command>` in the working folder,
//!   its standard output and error one pipe read to the end (the first
//!   `chat::MAX_OUTPUT` bytes kept, the rest counted), then its exit
//!   code; the result comes back as WM_APP_RAN. What is due after a
//!   message (`Panel::take_due`: a call of the model, a command) is
//!   started from `update`.
//!
//! The view and what a message does to it are the library (`agent`);
//! this file does what is outside it. The window is placed from the
//! message loop (a posted WM_APP_PLACE), never from inside gala_ui's
//! handling of a message, which the WM_SIZE of a move would re-enter.
//!
//! Observability: each line on standard output (`agent: ...`, the serial
//! log for a program the desktop starts): the start and the remembered
//! state, the tools found (cmd.exe's path, busybox on the PATH or not),
//! every placement with its rectangle, every context, every
//! hand-off (its program, window, text and element, or the process and
//! the length of its text) or why one was not taken, a process let go
//! when another program's window is active, every request asked
//! (`asking the model: request N, B bytes, round R of 8`) and its answer
//! (the text, or each call of the tool with its id and arguments) or why
//! there is none (the winhttp call that failed and its error, the HTTP
//! status, the relay's message), every folder reported, every request's
//! working folder or why it has none, every step shown or refused (and
//! why), every Run and Cancel clicked, every command started (its pid)
//! and ended (its exit code, the bytes read and kept) or why it could
//! not run, a request stopped by the limit of rounds, every message the
//! panel handled (gala_ui's `click HideButton: Hide`), and what failed
//! (an app bar refused, the state not saved) with its error.

#![no_std]
#![no_main]

extern crate alloc;

use gala_heap as _;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::ffi::c_void;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use agent::{
    Answer, Context, Due, Msg, Panel, Ran, Reply, Step, TAB, Tools, WIDTH, Work, chat, handoff,
    program_name,
};
use alloc::boxed::Box;
use gala_ui::{Component, Node, win};

type Handle = *mut c_void;
type Bool = i32;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

/// APPBARDATA.
#[repr(C)]
struct AppBarData {
    size: u32,
    hwnd: Handle,
    callback: u32,
    edge: u32,
    rect: Rect,
    param: isize,
}

#[repr(C)]
struct WinMsg {
    hwnd: Handle,
    message: u32,
    wparam: usize,
    lparam: isize,
    time: u32,
    pt: [i32; 2],
    private: u32,
}

type WinEventProc = extern "system" fn(usize, u32, Handle, i32, i32, u32, u32);

const ABM_NEW: u32 = 0;
const ABM_REMOVE: u32 = 1;
const ABM_QUERYPOS: u32 = 2;
const ABM_SETPOS: u32 = 3;
const ABE_RIGHT: u32 = 2;
const ABN_POSCHANGED: usize = 1;
const SM_CXSCREEN: i32 = 0;
const SM_CYSCREEN: i32 = 1;
const HWND_TOPMOST: isize = -1;
const SWP_NOACTIVATE: u32 = 0x0010;
const EVENT_SYSTEM_FOREGROUND: u32 = 0x0003;
const WINEVENT_OUTOFCONTEXT: u32 = 0;
const OBJID_WINDOW: i32 = 0;
const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
const HKEY_CURRENT_USER: isize = 0x8000_0001u32 as i32 as isize;
const REG_DWORD: u32 = 4;
const RRF_RT_REG_DWORD: u32 = 0x10;

/// The active window changed: wParam is it (from the WinEvent hook).
const WM_APP_CONTEXT: u32 = 0x8001;
const WM_COPYDATA: u32 = 0x004a;

/// COPYDATASTRUCT.
#[repr(C)]
struct CopyData {
    kind: usize,
    size: u32,
    data: *const u8,
}
/// Place the window as the panel's state says (the message loop's).
const WM_APP_PLACE: u32 = 0x8002;
/// The app bar's callback message (ABN_*: wParam).
const WM_APP_BAR: u32 = 0x8003;
/// A request's reply (the worker's): wParam the request, lParam a
/// `Box<Reply>` the window takes back.
const WM_APP_ANSWER: u32 = 0x8004;
/// A command ended (the runner's): lParam a `Box<RunJob>` with its
/// result, which the window takes back.
const WM_APP_RAN: u32 = 0x8005;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut c_void) -> Bool;
    fn ExitProcess(code: u32) -> !;
    fn OpenProcess(access: u32, inherit: Bool, pid: u32) -> Handle;
    fn QueryFullProcessImageNameW(h: Handle, flags: u32, name: *mut u16, size: *mut u32) -> Bool;
    fn CloseHandle(h: Handle) -> Bool;
    fn CreateThread(
        attributes: *mut c_void,
        stack: usize,
        start: extern "system" fn(*mut c_void) -> u32,
        param: *mut c_void,
        flags: u32,
        id: *mut u32,
    ) -> Handle;
    fn GetLastError() -> u32;
    fn LoadLibraryW(name: *const u16) -> Handle;
    fn GetProcAddress(module: Handle, name: *const u8) -> *const c_void;
    fn GetSystemDirectoryW(buf: *mut u16, size: u32) -> u32;
    fn GetFileAttributesW(path: *const u16) -> u32;
    fn SearchPathW(
        path: *const u16,
        file: *const u16,
        ext: *const u16,
        size: u32,
        buf: *mut u16,
        part: *mut *mut u16,
    ) -> u32;
    fn CreatePipe(read: *mut Handle, write: *mut Handle, sa: *const Security, size: u32) -> Bool;
    fn SetHandleInformation(h: Handle, mask: u32, flags: u32) -> Bool;
    fn CreateProcessW(
        app: *const u16,
        cmd: *mut u16,
        pa: *const c_void,
        ta: *const c_void,
        inherit: Bool,
        flags: u32,
        env: *const c_void,
        dir: *const u16,
        si: *const StartupInfo,
        pi: *mut ProcessInfo,
    ) -> Bool;
    fn ReadFile(h: Handle, buf: *mut u8, len: u32, read: *mut u32, ov: *mut c_void) -> Bool;
    fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
    fn GetExitCodeProcess(h: Handle, code: *mut u32) -> Bool;
}

/// SECURITY_ATTRIBUTES.
#[repr(C)]
struct Security {
    size: u32,
    descriptor: *const c_void,
    inherit: Bool,
}

/// STARTUPINFOW.
#[repr(C)]
struct StartupInfo {
    cb: u32,
    reserved: *const u16,
    desktop: *const u16,
    title: *const u16,
    x: u32,
    y: u32,
    x_size: u32,
    y_size: u32,
    x_chars: u32,
    y_chars: u32,
    fill: u32,
    flags: u32,
    show: u16,
    reserved2_len: u16,
    reserved2: *const u8,
    input: Handle,
    output: Handle,
    error: Handle,
}

/// PROCESS_INFORMATION.
#[repr(C)]
struct ProcessInfo {
    process: Handle,
    thread: Handle,
    pid: u32,
    tid: u32,
}

const HANDLE_FLAG_INHERIT: u32 = 1;
const STARTF_USESTDHANDLES: u32 = 0x100;
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const CREATE_UNICODE_ENVIRONMENT: u32 = 0x400;
const INFINITE: u32 = u32::MAX;
const INVALID_FILE_ATTRIBUTES: u32 = u32::MAX;
const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;

#[link(name = "user32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetSystemMetrics(index: i32) -> i32;
    fn SetWindowPos(hwnd: Handle, after: isize, x: i32, y: i32, w: i32, h: i32, flags: u32)
    -> Bool;
    fn PostMessageW(hwnd: Handle, msg: u32, wparam: usize, lparam: isize) -> Bool;
    fn GetMessageW(msg: *mut WinMsg, hwnd: Handle, min: u32, max: u32) -> i32;
    fn TranslateMessage(msg: *const WinMsg) -> Bool;
    fn DispatchMessageW(msg: *const WinMsg) -> isize;
    fn GetWindowTextW(hwnd: Handle, text: *mut u16, max: i32) -> i32;
    fn GetWindowThreadProcessId(hwnd: Handle, pid: *mut u32) -> u32;
    fn GetForegroundWindow() -> Handle;
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

#[link(name = "shell32", kind = "raw-dylib")]
unsafe extern "system" {
    fn SHAppBarMessage(message: u32, data: *mut AppBarData) -> usize;
}

#[link(name = "advapi32", kind = "raw-dylib")]
unsafe extern "system" {
    fn RegGetValueW(
        key: isize,
        subkey: *const u16,
        value: *const u16,
        flags: u32,
        kind: *mut u32,
        data: *mut c_void,
        size: *mut u32,
    ) -> u32;
    fn RegSetKeyValueW(
        key: isize,
        subkey: *const u16,
        value: *const u16,
        kind: u32,
        data: *const c_void,
        size: u32,
    ) -> u32;
}

/// The panel's window (0 until it is made).
static WINDOW: AtomicUsize = AtomicUsize::new(0);
/// Whether the window is an app bar now (ABM_NEW taken, no ABM_REMOVE).
static REGISTERED: AtomicBool = AtomicBool::new(false);
/// The state the window is placed for (`Panel::hidden`).
static HIDDEN: AtomicBool = AtomicBool::new(false);

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// A line on standard output (`agent: ...`).
fn log(text: &str) {
    let s = format!("agent: {text}\r\n");
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

fn window() -> Handle {
    WINDOW.load(Ordering::Acquire) as Handle
}

const KEY: &str = "Software\\gala\\Agent";

/// The remembered state: hidden or not (not when never saved).
fn remembered() -> bool {
    let (key, name) = (wide(KEY), wide("Hidden"));
    let (mut v, mut size) = (0u32, 4u32);
    // SAFETY: live strings and a DWORD to fill.
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_DWORD,
            core::ptr::null_mut(),
            (&mut v as *mut u32).cast(),
            &mut size,
        )
    };
    r == 0 && v != 0
}

fn remember(hidden: bool) {
    let (key, name) = (wide(KEY), wide("Hidden"));
    let v = u32::from(hidden);
    // SAFETY: live strings and a DWORD.
    let r = unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ptr(),
            REG_DWORD,
            (&v as *const u32).cast(),
            4,
        )
    };
    if r != 0 {
        log(&format!("the state is not saved: RegSetKeyValueW {r}"));
    }
}

fn bar(message: u32, rect: Rect) -> (usize, Rect) {
    let mut d = AppBarData {
        size: core::mem::size_of::<AppBarData>() as u32,
        hwnd: window(),
        callback: WM_APP_BAR,
        edge: ABE_RIGHT,
        rect,
        param: 0,
    };
    // SAFETY: a filled APPBARDATA.
    let r = unsafe { SHAppBarMessage(message, &mut d) };
    (r, d.rect)
}

/// Places the window as `HIDDEN` says: docked as an app bar on the
/// right edge, or the dock tab with the strip given back.
fn place() {
    // SAFETY: plain calls.
    let (sw, sh) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
    let r = if HIDDEN.load(Ordering::Acquire) {
        if REGISTERED.swap(false, Ordering::AcqRel) {
            bar(ABM_REMOVE, Rect::default());
        }
        let r =
            Rect { left: sw - TAB.0, top: (sh - TAB.1) / 2, right: sw, bottom: (sh + TAB.1) / 2 };
        log(&format!("hidden: the dock tab at {},{} {}x{}", r.left, r.top, TAB.0, TAB.1));
        r
    } else {
        if !REGISTERED.load(Ordering::Acquire) {
            if bar(ABM_NEW, Rect::default()).0 == 0 {
                log("docked without a strip: ABM_NEW refused");
            } else {
                REGISTERED.store(true, Ordering::Release);
            }
        }
        let want = Rect { left: sw - WIDTH, top: 0, right: sw, bottom: sh };
        let r = if REGISTERED.load(Ordering::Acquire) {
            let (_, q) = bar(ABM_QUERYPOS, want);
            let (_, s) = bar(ABM_SETPOS, q);
            s
        } else {
            want
        };
        log(&format!("docked at {},{} {}x{}", r.left, r.top, r.right - r.left, r.bottom - r.top));
        r
    };
    // SAFETY: our window.
    unsafe {
        SetWindowPos(
            window(),
            HWND_TOPMOST,
            r.left,
            r.top,
            r.right - r.left,
            r.bottom - r.top,
            SWP_NOACTIVATE,
        )
    };
}

/// The program and title of window `hwnd`.
fn context_of(hwnd: Handle) -> Context {
    let mut t = [0u16; 256];
    // SAFETY: a buffer of its size.
    let n = unsafe { GetWindowTextW(hwnd, t.as_mut_ptr(), t.len() as i32) };
    let title = String::from_utf16_lossy(&t[..n.clamp(0, 256) as usize]);
    let mut pid = 0u32;
    // SAFETY: a u32 to fill.
    unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
    // SAFETY: plain call; the handle is closed below.
    let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    let mut program = String::new();
    if !h.is_null() {
        let mut p = [0u16; 260];
        let mut size = p.len() as u32;
        // SAFETY: a buffer of `size` units.
        if unsafe { QueryFullProcessImageNameW(h, 0, p.as_mut_ptr(), &mut size) } != 0 {
            program = program_name(&String::from_utf16_lossy(&p[..size as usize]));
        }
        // SAFETY: the handle opened above.
        unsafe { CloseHandle(h) };
    }
    if program.is_empty() {
        program = format!("process {pid}");
    }
    Context { program, title, window: hwnd as u64 }
}

extern "system" fn foreground(
    _: usize,
    event: u32,
    hwnd: Handle,
    object: i32,
    _: i32,
    _: u32,
    _: u32,
) {
    if event == EVENT_SYSTEM_FOREGROUND
        && object == OBJID_WINDOW
        && !hwnd.is_null()
        && hwnd != window()
    {
        // SAFETY: our window; the loop takes it.
        unsafe { PostMessageW(window(), WM_APP_CONTEXT, hwnd as usize, 0) };
    }
}

// ---- Asking the model ------------------------------------------------------

type Open = extern "system" fn(*const u16, u32, *const u16, *const u16, u32) -> usize;
type Connect = extern "system" fn(usize, *const u16, u16, u32) -> usize;
type OpenRequest = extern "system" fn(
    usize,
    *const u16,
    *const u16,
    *const u16,
    *const u16,
    *const *const u16,
    u32,
) -> usize;
type Send = extern "system" fn(usize, *const u16, u32, *const u8, u32, u32, usize) -> Bool;
type Receive = extern "system" fn(usize, usize) -> Bool;
type QueryHeaders = extern "system" fn(usize, u32, *const u16, *mut u8, *mut u32, *mut u32) -> Bool;
type Available = extern "system" fn(usize, *mut u32) -> Bool;
type Read = extern "system" fn(usize, *mut u8, u32, *mut u32) -> Bool;
type Close = extern "system" fn(usize) -> Bool;

/// WinHTTP's functions, from winhttp.dll loaded on the first ask.
#[derive(Clone, Copy)]
struct WinHttp {
    open: Open,
    connect: Connect,
    open_request: OpenRequest,
    send: Send,
    receive: Receive,
    query_headers: QueryHeaders,
    available: Available,
    read: Read,
    close: Close,
}

const WINHTTP_ACCESS_TYPE_NO_PROXY: u32 = 1;
const WINHTTP_QUERY_STATUS_CODE: u32 = 19;
const WINHTTP_QUERY_FLAG_NUMBER: u32 = 0x2000_0000;

fn winhttp() -> Result<WinHttp, String> {
    let name = wide("winhttp.dll");
    // SAFETY: a NUL-terminated name.
    let module = unsafe { LoadLibraryW(name.as_ptr()) };
    if module.is_null() {
        // SAFETY: plain call.
        return Err(format!("winhttp.dll not loaded (error {})", unsafe { GetLastError() }));
    }
    let f = |n: &[u8]| -> Result<*const c_void, String> {
        // SAFETY: a NUL-terminated name in a loaded module.
        let p = unsafe { GetProcAddress(module, n.as_ptr()) };
        if p.is_null() {
            Err(format!(
                "winhttp.dll has no {}",
                core::str::from_utf8(&n[..n.len() - 1]).unwrap_or("?")
            ))
        } else {
            Ok(p)
        }
    };
    // SAFETY: each pointer is the export of that name, whose signature
    // (WinHTTP's, as documented) is the field's.
    unsafe {
        Ok(WinHttp {
            open: core::mem::transmute::<*const c_void, Open>(f(b"WinHttpOpen\0")?),
            connect: core::mem::transmute::<*const c_void, Connect>(f(b"WinHttpConnect\0")?),
            open_request: core::mem::transmute::<*const c_void, OpenRequest>(f(
                b"WinHttpOpenRequest\0",
            )?),
            send: core::mem::transmute::<*const c_void, Send>(f(b"WinHttpSendRequest\0")?),
            receive: core::mem::transmute::<*const c_void, Receive>(f(
                b"WinHttpReceiveResponse\0",
            )?),
            query_headers: core::mem::transmute::<*const c_void, QueryHeaders>(f(
                b"WinHttpQueryHeaders\0",
            )?),
            available: core::mem::transmute::<*const c_void, Available>(f(
                b"WinHttpQueryDataAvailable\0",
            )?),
            read: core::mem::transmute::<*const c_void, Read>(f(b"WinHttpReadData\0")?),
            close: core::mem::transmute::<*const c_void, Close>(f(b"WinHttpCloseHandle\0")?),
        })
    }
}

/// A handle closed when dropped.
struct H(usize, Close);
impl Drop for H {
    fn drop(&mut self) {
        (self.1)(self.0);
    }
}

/// What `call` failed in: the function and GetLastError.
fn failed(what: &str) -> String {
    // SAFETY: plain call.
    format!("{what} failed (error {})", unsafe { GetLastError() })
}

/// POSTs `body` to the relay: the reply's answer, or why there is none.
fn call(body: &[u8]) -> Reply {
    let w = winhttp()?;
    let (agent, host, verb, path) =
        (wide("gala-agent/1"), wide(chat::HOST), wide("POST"), wide(chat::PATH));
    let headers = wide("Content-Type: application/json");
    let null = core::ptr::null();
    let session = (w.open)(agent.as_ptr(), WINHTTP_ACCESS_TYPE_NO_PROXY, null, null, 0);
    if session == 0 {
        return Err(failed("WinHttpOpen"));
    }
    let session = H(session, w.close);
    let connect = (w.connect)(session.0, host.as_ptr(), chat::PORT, 0);
    if connect == 0 {
        return Err(failed("WinHttpConnect"));
    }
    let connect = H(connect, w.close);
    let request =
        (w.open_request)(connect.0, verb.as_ptr(), path.as_ptr(), null, null, core::ptr::null(), 0);
    if request == 0 {
        return Err(failed("WinHttpOpenRequest"));
    }
    let request = H(request, w.close);
    let len = u32::try_from(body.len()).map_err(|_| String::from("the request is too long"))?;
    if (w.send)(request.0, headers.as_ptr(), u32::MAX, body.as_ptr(), len, len, 0) == 0 {
        return Err(failed("WinHttpSendRequest"));
    }
    if (w.receive)(request.0, 0) == 0 {
        return Err(failed("WinHttpReceiveResponse"));
    }
    let (mut status, mut size) = (0u32, 4u32);
    let level = WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER;
    let got = (w.query_headers)(
        request.0,
        level,
        null,
        (&mut status as *mut u32).cast(),
        &mut size,
        core::ptr::null_mut(),
    );
    if got == 0 {
        return Err(failed("WinHttpQueryHeaders(status)"));
    }
    let mut reply = Vec::new();
    loop {
        let mut n = 0u32;
        if (w.available)(request.0, &mut n) == 0 {
            return Err(failed("WinHttpQueryDataAvailable"));
        }
        if n == 0 {
            break;
        }
        if reply.len() + n as usize > chat::MAX_REPLY {
            return Err(format!("the reply is longer than {} bytes", chat::MAX_REPLY));
        }
        let at = reply.len();
        reply.resize(at + n as usize, 0);
        let mut read = 0u32;
        if (w.read)(request.0, reply[at..].as_mut_ptr(), n, &mut read) == 0 {
            return Err(failed("WinHttpReadData"));
        }
        reply.truncate(at + read as usize);
    }
    match (status, chat::reply(&reply)) {
        (200, a) => a,
        (s, Err(why)) => Err(format!("HTTP {s}: {why}")),
        (s, Ok(_)) => Err(format!("HTTP {s}")),
    }
}

/// The worker's job: the request and the body to POST.
struct Job {
    index: usize,
    body: String,
}

/// What a reply said, for the log.
fn said(r: &Reply) -> String {
    match r {
        Ok(chat::Said::Text(t)) => format!("{t:?}"),
        Ok(chat::Said::Calls { text, calls }) => {
            let mut s = match text {
                Some(t) => format!("{t:?} and "),
                None => String::new(),
            };
            s.push_str(&format!("{} call(s):", calls.len()));
            for c in calls {
                s.push_str(&format!(" {} {}({})", c.id, c.name, c.arguments));
            }
            s
        }
        Err(e) => e.clone(),
    }
}

extern "system" fn worker(param: *mut c_void) -> u32 {
    // SAFETY: `ask` made it with Box::into_raw and gave it to this thread
    // alone.
    let job = unsafe { Box::from_raw(param.cast::<Job>()) };
    let reply = call(job.body.as_bytes());
    match &reply {
        Ok(_) => log(&format!("answer {}: {}", job.index, said(&reply))),
        Err(e) => log(&format!("no answer {}: {e}", job.index)),
    }
    let reply = Box::into_raw(Box::new(reply));
    // SAFETY: the panel's window; the window takes the box back.
    if unsafe { PostMessageW(window(), WM_APP_ANSWER, job.index, reply as isize) } == 0 {
        // SAFETY: not posted: still ours.
        drop(unsafe { Box::from_raw(reply) });
        log(&failed("PostMessageW(WM_APP_ANSWER)"));
    }
    0
}

/// Asks the model for request `index` (its `round`th call) on a worker
/// thread.
fn ask(index: usize, round: usize, body: String) {
    log(&format!(
        "asking the model: request {index}, {} bytes, round {round} of {}",
        body.len(),
        agent::ROUNDS
    ));
    let job = Box::into_raw(Box::new(Job { index, body }));
    // SAFETY: the worker owns the job from here.
    let t = unsafe {
        CreateThread(core::ptr::null_mut(), 0, worker, job.cast(), 0, core::ptr::null_mut())
    };
    if t.is_null() {
        // SAFETY: no thread took it.
        drop(unsafe { Box::from_raw(job) });
        let why = failed("CreateThread");
        log(&format!("no answer {index}: {why}"));
        // The request says why.
        let reply = Box::into_raw(Box::new(Err::<chat::Said, String>(why)));
        // SAFETY: our window; it takes the box back.
        if unsafe { PostMessageW(window(), WM_APP_ANSWER, index, reply as isize) } == 0 {
            // SAFETY: not posted: still ours.
            drop(unsafe { Box::from_raw(reply) });
        }
    } else {
        // SAFETY: the thread's handle, not needed.
        unsafe { CloseHandle(t) };
    }
}

// ---- Running a command ----------------------------------------------------

/// A command to run for step `step` of request `index`, and once it
/// ended (`run_job`), what it gave.
struct RunJob {
    index: usize,
    step: usize,
    cmd: String,
    command: String,
    folder: String,
    ran: Ran,
}

/// A handle closed when dropped (kernel32's).
struct Owned(Handle);
impl Drop for Owned {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: a handle this program opened and owns.
            unsafe { CloseHandle(self.0) };
        }
    }
}

/// Runs `job`'s command and waits for it: `cmd.exe /c <command>` in its
/// folder, its standard output and error one pipe; the exit code, the
/// output kept and the bytes read; or why it could not run.
fn run_job(job: &RunJob) -> Ran {
    let gone = |why: String| Ran { code: None, output: why, bytes: 0 };
    let (mut read, mut write) = (core::ptr::null_mut(), core::ptr::null_mut());
    let sa = Security {
        size: core::mem::size_of::<Security>() as u32,
        descriptor: core::ptr::null(),
        inherit: 1,
    };
    // SAFETY: two handles to fill and a filled SECURITY_ATTRIBUTES.
    if unsafe { CreatePipe(&mut read, &mut write, &sa, 0) } == 0 {
        return gone(failed("CreatePipe"));
    }
    let (read, write) = (Owned(read), Owned(write));
    // Only the write end goes to the child.
    // SAFETY: our handle.
    if unsafe { SetHandleInformation(read.0, HANDLE_FLAG_INHERIT, 0) } == 0 {
        return gone(failed("SetHandleInformation"));
    }
    let mut line = wide(&format!("\"{}\" /c {}", job.cmd, job.command));
    let (app, dir) = (wide(&job.cmd), wide(&job.folder));
    let si = StartupInfo {
        cb: core::mem::size_of::<StartupInfo>() as u32,
        reserved: core::ptr::null(),
        desktop: core::ptr::null(),
        title: core::ptr::null(),
        x: 0,
        y: 0,
        x_size: 0,
        y_size: 0,
        x_chars: 0,
        y_chars: 0,
        fill: 0,
        flags: STARTF_USESTDHANDLES,
        show: 0,
        reserved2_len: 0,
        reserved2: core::ptr::null(),
        input: core::ptr::null_mut(),
        output: write.0,
        error: write.0,
    };
    let mut pi = ProcessInfo {
        process: core::ptr::null_mut(),
        thread: core::ptr::null_mut(),
        pid: 0,
        tid: 0,
    };
    // SAFETY: NUL-terminated strings (the command line writable), filled
    // structures, live through the call.
    let made = unsafe {
        CreateProcessW(
            app.as_ptr(),
            line.as_mut_ptr(),
            core::ptr::null(),
            core::ptr::null(),
            1,
            CREATE_NO_WINDOW | CREATE_UNICODE_ENVIRONMENT,
            core::ptr::null(),
            dir.as_ptr(),
            &si,
            &mut pi,
        )
    };
    if made == 0 {
        return gone(failed("CreateProcessW"));
    }
    let (process, _thread) = (Owned(pi.process), Owned(pi.thread));
    log(&format!(
        "running request {} step {}: pid {}, {:?} in {}",
        job.index, job.step, pi.pid, job.command, job.folder
    ));
    // Our write end closed, the pipe ends when the child's (and its
    // children's) do.
    drop(write);
    let mut kept = Vec::new();
    let mut bytes = 0usize;
    let mut buf = [0u8; 4096];
    loop {
        let mut n = 0u32;
        // SAFETY: a buffer of its size; our read end.
        let ok = unsafe {
            ReadFile(read.0, buf.as_mut_ptr(), buf.len() as u32, &mut n, core::ptr::null_mut())
        };
        if ok == 0 || n == 0 {
            // The end (ERROR_BROKEN_PIPE once every writer is gone).
            break;
        }
        let n = n as usize;
        bytes += n;
        let room = chat::MAX_OUTPUT.saturating_sub(kept.len());
        kept.extend_from_slice(&buf[..n.min(room)]);
    }
    let mut code = 0u32;
    // SAFETY: the child's handle; a u32 to fill.
    let ended = unsafe {
        WaitForSingleObject(process.0, INFINITE) == 0
            && GetExitCodeProcess(process.0, &mut code) != 0
    };
    if !ended {
        return gone(failed(&format!("waiting for pid {}", pi.pid)));
    }
    log(&format!(
        "ran request {} step {}: pid {} exit code {code}, {bytes} bytes of output ({} kept)",
        job.index,
        job.step,
        pi.pid,
        kept.len()
    ));
    Ran { code: Some(code), output: String::from_utf8_lossy(&kept).into_owned(), bytes }
}

extern "system" fn runner(param: *mut c_void) -> u32 {
    // SAFETY: `run` made it with Box::into_raw and gave it to this thread
    // alone.
    let mut job = unsafe { Box::from_raw(param.cast::<RunJob>()) };
    job.ran = run_job(&job);
    if job.ran.code.is_none() {
        log(&format!("request {} step {} not run: {}", job.index, job.step, job.ran.output));
    }
    finish(job);
    0
}

/// Gives a run's result back to the window.
fn finish(job: Box<RunJob>) {
    let job = Box::into_raw(job);
    // SAFETY: the panel's window; the window takes the box back.
    if unsafe { PostMessageW(window(), WM_APP_RAN, 0, job as isize) } == 0 {
        // SAFETY: not posted: still ours.
        drop(unsafe { Box::from_raw(job) });
        log(&failed("PostMessageW(WM_APP_RAN)"));
    }
}

/// Runs a step's command on a worker thread.
fn run(index: usize, step: usize, cmd: String, command: String, folder: String) {
    let ran = Ran { code: None, output: String::new(), bytes: 0 };
    let job = Box::into_raw(Box::new(RunJob { index, step, cmd, command, folder, ran }));
    // SAFETY: the runner owns the job from here.
    let t = unsafe {
        CreateThread(core::ptr::null_mut(), 0, runner, job.cast(), 0, core::ptr::null_mut())
    };
    if t.is_null() {
        // SAFETY: no thread took it.
        let mut job = unsafe { Box::from_raw(job) };
        job.ran.output = failed("CreateThread");
        log(&format!("request {index} step {step} not run: {}", job.ran.output));
        finish(job);
    } else {
        // SAFETY: the thread's handle, not needed.
        unsafe { CloseHandle(t) };
    }
}

/// What this machine has to run with: cmd.exe in System32, busybox.exe
/// on the PATH.
fn tools() -> Tools {
    let mut dir = [0u16; 260];
    // SAFETY: a buffer of its size.
    let n = unsafe { GetSystemDirectoryW(dir.as_mut_ptr(), dir.len() as u32) } as usize;
    let cmd = (n > 0 && n < dir.len()).then(|| {
        format!("{}\\cmd.exe", String::from_utf16_lossy(&dir[..n]).trim_end_matches('\\'))
    });
    let cmd = cmd.filter(|c| {
        let w = wide(c);
        // SAFETY: a NUL-terminated path.
        let a = unsafe { GetFileAttributesW(w.as_ptr()) };
        a != INVALID_FILE_ATTRIBUTES && a & FILE_ATTRIBUTE_DIRECTORY == 0
    });
    let mut found = [0u16; 260];
    let name = wide("busybox.exe");
    // SAFETY: a NUL-terminated name and a buffer of its size.
    let n = unsafe {
        SearchPathW(
            core::ptr::null(),
            name.as_ptr(),
            core::ptr::null(),
            found.len() as u32,
            found.as_mut_ptr(),
            core::ptr::null_mut(),
        )
    } as usize;
    let busybox = (n > 0 && n < found.len()).then(|| String::from_utf16_lossy(&found[..n]));
    log(&format!(
        "tools: cmd.exe {}, busybox {}",
        cmd.as_deref().unwrap_or("not installed"),
        busybox.as_deref().unwrap_or("not on the PATH")
    ));
    Tools { cmd, busybox: busybox.is_some() }
}

/// Where request `r` runs, for the log.
fn work(r: &agent::Request) -> String {
    match &r.work {
        Work::Windows { folder, .. } => format!("works in {folder}"),
        Work::Linux { distro, path, .. } => format!("works in {path} of {distro} (not run yet)"),
        Work::Unknown(why) => format!("has no working folder: {why}"),
    }
}

struct App {
    panel: Panel,
}

impl App {
    /// Logs what `update` made of request `i`: the steps it now has
    /// beyond `before`, and its end by the limit of rounds.
    fn log_steps(&self, i: usize, before: usize, failed_before: bool) {
        let Some(r) = self.panel.requests.get(i) else { return };
        for (n, c) in r.rounds.iter().flat_map(|round| &round.calls).enumerate().skip(before) {
            match &c.step {
                Step::Waiting => log(&format!(
                    "request {i} step {n} ({}) waits for Run or Cancel: {:?} ({:?})",
                    c.id, c.command, c.explanation
                )),
                Step::Refused(why) => {
                    log(&format!("request {i} step {n} ({}) refused: {why}", c.id))
                }
                _ => {}
            }
        }
        if let Some(Answer::Failed(why)) = &r.answer
            && !failed_before
            && r.asked >= agent::ROUNDS
        {
            log(&format!("request {i} ended: {why}"));
        }
    }
}

impl Component for App {
    type Msg = Msg;

    fn view(&self) -> Node<Msg> {
        self.panel.view()
    }

    fn update(&mut self, msg: Msg) {
        let placing = matches!(msg, Msg::Hide | Msg::Show)
            || matches!(msg, Msg::Handoff(_) | Msg::About(_)) && self.panel.hidden;
        if let Msg::Handoff(h) = &msg {
            log(&format!(
                "handoff from {} (window {:#x}): {:?} on {}",
                h.program,
                h.window,
                h.text,
                h.element.as_ref().map_or("the whole window", |e| e.id.as_str())
            ));
        }
        if let Msg::About(a) = &msg {
            log(&format!(
                "handoff from {} (window {:#x}): about {} (pid {}, {}), {} bytes of text",
                a.program,
                a.window,
                a.name,
                a.pid,
                a.personality(),
                a.text.len()
            ));
        }
        if let Msg::Context(c) = &msg {
            log(&format!("context {}: {}", c.program, c.title));
            if let Some(a) = &self.panel.about
                && a.program != c.program
            {
                log(&format!(
                    "no longer about {} (pid {}): {} is active",
                    a.name, a.pid, c.program
                ));
            }
        }
        if let Msg::Send = &msg {
            log(&format!("request {:?}", self.panel.draft));
        }
        if let Msg::Folder(f) = &msg {
            if f.path.is_empty() {
                log(&format!(
                    "folder of {} (window {:#x}): none (it shows no file-system folder)",
                    f.program, f.window
                ));
            } else {
                log(&format!("folder of {} (window {:#x}): {}", f.program, f.window, f.path));
            }
        }
        if let Msg::Run(i, n) | Msg::Cancel(i, n) = &msg {
            let what = if matches!(msg, Msg::Run(..)) { "Run" } else { "Cancel" };
            log(&format!("{what} clicked: request {i} step {n}"));
        }
        if let Msg::Ran(i, n, r) = &msg {
            match r.code {
                Some(code) => log(&format!("request {i} step {n} ran: exit code {code}")),
                None => log(&format!("request {i} step {n} could not run: {}", r.output)),
            }
        }
        // What an answer or a step's end may add to its request.
        let watched = match &msg {
            Msg::Answer(i, _) | Msg::Ran(i, ..) | Msg::Cancel(i, _) => Some(*i),
            _ => None,
        };
        let before = watched.and_then(|i| self.panel.requests.get(i)).map(|r| {
            let calls = r.rounds.iter().map(|round| round.calls.len()).sum::<usize>();
            (calls, matches!(r.answer, Some(Answer::Failed(_))))
        });
        let sent = matches!(msg, Msg::Send);
        let requests = self.panel.requests.len();
        self.panel.update(msg);
        if sent && self.panel.requests.len() > requests {
            let i = self.panel.requests.len() - 1;
            log(&format!("request {i} {}", work(&self.panel.requests[i])));
        }
        if let (Some(i), Some((calls, failed))) = (watched, before) {
            self.log_steps(i, calls, failed);
        }
        while let Some(due) = self.panel.take_due() {
            match due {
                Due::Ask { index, round, body } => ask(index, round, body),
                Due::Run { index, step, cmd, command, folder } => {
                    run(index, step, cmd, command, folder)
                }
            }
        }
        if placing {
            HIDDEN.store(self.panel.hidden, Ordering::Release);
            remember(self.panel.hidden);
            // SAFETY: our window; the loop places it.
            unsafe { PostMessageW(window(), WM_APP_PLACE, 0, 0) };
        }
    }

    fn message(&mut self, _: usize, msg: u32, wparam: usize, lparam: isize) -> Option<Msg> {
        match msg {
            WM_COPYDATA => {
                // SAFETY: WM_COPYDATA's lParam is a COPYDATASTRUCT valid
                // for the message (user32 copied it in from the sender).
                let cds = unsafe { (lparam as *const CopyData).as_ref() }?;
                if cds.kind != handoff::KIND {
                    log(&format!("WM_COPYDATA of kind {:#x} not taken", cds.kind));
                    return None;
                }
                let bytes = if cds.size == 0 || cds.data.is_null() {
                    &[][..]
                } else {
                    // SAFETY: as above: `size` bytes at `data`.
                    unsafe { core::slice::from_raw_parts(cds.data, cds.size as usize) }
                };
                match handoff::receive(bytes) {
                    Ok(handoff::Received::Request(h)) => Some(Msg::Handoff(h)),
                    Ok(handoff::Received::About(a)) => Some(Msg::About(a)),
                    Ok(handoff::Received::Folder(f)) => Some(Msg::Folder(f)),
                    Err(e) => {
                        log(&format!("handoff refused ({} bytes): {e:?}", bytes.len()));
                        None
                    }
                }
            }
            WM_APP_ANSWER => {
                // SAFETY: the worker's box, posted once to this window.
                let reply = unsafe { Box::from_raw(lparam as *mut Reply) };
                Some(Msg::Answer(wparam, *reply))
            }
            WM_APP_RAN => {
                // SAFETY: the runner's box, posted once to this window.
                let job = unsafe { Box::from_raw(lparam as *mut RunJob) };
                let RunJob { index, step, ran, .. } = *job;
                Some(Msg::Ran(index, step, ran))
            }
            WM_APP_CONTEXT => {
                let c = context_of(wparam as Handle);
                (self.panel.context.as_ref() != Some(&c)).then_some(Msg::Context(c))
            }
            _ => None,
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let hidden = remembered();
    HIDDEN.store(hidden, Ordering::Release);
    log(if hidden { "started hidden (remembered)" } else { "started docked" });
    // SAFETY: plain calls.
    let (sw, sh) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
    // Made where it docks; `place` sets the strip and the final place.
    let options = win::Options {
        title: "Agent",
        x: sw - WIDTH,
        y: 0,
        width: WIDTH,
        height: sh,
        topmost: true,
        popup: true,
        no_activate: false,
        hidden: false,
        class: None,
    };
    let app = App { panel: Panel { hidden, tools: tools(), ..Panel::default() } };
    let Some(hwnd) = win::open(app, &options, log) else {
        log("the window could not be made");
        // SAFETY: plain call.
        unsafe { ExitProcess(1) }
    };
    WINDOW.store(hwnd as usize, Ordering::Release);
    // Hooked before it docks: a window activated once the panel is
    // docked is its context (its callback runs from the loop below).
    // SAFETY: an out-of-context hook to our procedure, on this thread.
    let hook = unsafe {
        SetWinEventHook(
            EVENT_SYSTEM_FOREGROUND,
            EVENT_SYSTEM_FOREGROUND,
            0,
            foreground,
            0,
            0,
            WINEVENT_OUTOFCONTEXT,
        )
    };
    if hook == 0 {
        log("no context: SetWinEventHook failed");
    }
    place();
    // The window active now, if not the panel.
    // SAFETY: plain call.
    let active = unsafe { GetForegroundWindow() };
    if !active.is_null() && active != hwnd {
        // SAFETY: our window.
        unsafe { PostMessageW(hwnd, WM_APP_CONTEXT, active as usize, 0) };
    }
    // SAFETY: a message structure GetMessageW fills.
    let mut m: WinMsg = unsafe { core::mem::zeroed() };
    // SAFETY: the thread's messages.
    while unsafe { GetMessageW(&mut m, core::ptr::null_mut(), 0, 0) } > 0 {
        if m.hwnd == hwnd && m.message == WM_APP_PLACE {
            place();
            continue;
        }
        if m.hwnd == hwnd && m.message == WM_APP_BAR {
            if m.wparam == ABN_POSCHANGED && !HIDDEN.load(Ordering::Acquire) {
                log("ABN_POSCHANGED: placed again");
                place();
            }
            continue;
        }
        // SAFETY: the message just taken.
        unsafe {
            TranslateMessage(&m);
            DispatchMessageW(&m);
        }
    }
    if REGISTERED.load(Ordering::Acquire) {
        bar(ABM_REMOVE, Rect::default());
    }
    log("ended");
    // SAFETY: plain call.
    unsafe { ExitProcess(m.wparam as u32) }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    log(&format!("panic: {info}"));
    // SAFETY: plain call.
    unsafe { ExitProcess(0xdead) }
}

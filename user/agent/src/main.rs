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
//!   program (Explorer's "Make the change"): the panel docks if hidden
//!   and lists the request; any other kind, or a malformed one, is
//!   logged and not taken.
//! - **Chat**: a request sent is asked of the model on a worker thread
//!   (`ask`): `agent::chat::body` POSTed to the relay at `chat::HOST`
//!   over winhttp, loaded on the first ask (a panel that is never asked
//!   needs no network); the reply comes back to the window as
//!   WM_APP_ANSWER (wParam the request, lParam the boxed `Reply`).
//!
//! The view and what a message does to it are the library (`agent`);
//! this file does what is outside it. The window is placed from the
//! message loop (a posted WM_APP_PLACE), never from inside gala_ui's
//! handling of a message, which the WM_SIZE of a move would re-enter.
//!
//! Observability: each line on standard output (`agent: ...`, the serial
//! log for a program the desktop starts): the start and the remembered
//! state, every placement with its rectangle, every context, every
//! hand-off (its program, window, text and element) or why one was not
//! taken, every request asked (`asking the model: request N, B bytes`)
//! and its answer or why there is none (the winhttp call that failed and
//! its error, the HTTP status, the relay's message), every message the
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

use agent::{Context, Msg, Panel, Reply, TAB, WIDTH, chat, handoff, program_name};
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
}

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
    Context { program, title }
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
    match (status, chat::answer(&reply)) {
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

extern "system" fn worker(param: *mut c_void) -> u32 {
    // SAFETY: `ask` made it with Box::into_raw and gave it to this thread
    // alone.
    let job = unsafe { Box::from_raw(param.cast::<Job>()) };
    let reply = call(job.body.as_bytes());
    match &reply {
        Ok(t) => log(&format!("answer {}: {t:?}", job.index)),
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

/// Asks the model for request `index` on a worker thread.
fn ask(index: usize, body: String) {
    log(&format!("asking the model: request {index}, {} bytes", body.len()));
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
        let reply = Box::into_raw(Box::new(Err::<String, String>(why)));
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

struct App {
    panel: Panel,
}

impl Component for App {
    type Msg = Msg;

    fn view(&self) -> Node<Msg> {
        self.panel.view()
    }

    fn update(&mut self, msg: Msg) {
        let placing = matches!(msg, Msg::Hide | Msg::Show)
            || matches!(msg, Msg::Handoff(_)) && self.panel.hidden;
        if let Msg::Handoff(h) = &msg {
            log(&format!(
                "handoff from {} (window {:#x}): {:?} on {}",
                h.program,
                h.window,
                h.text,
                h.element.as_ref().map_or("the whole window", |e| e.id.as_str())
            ));
        }
        if let Msg::Context(c) = &msg {
            log(&format!("context {}: {}", c.program, c.title));
        }
        if let Msg::Send = &msg {
            log(&format!("request {:?}", self.panel.draft));
        }
        let sent = matches!(msg, Msg::Send);
        self.panel.update(msg);
        if sent && let Some((i, body)) = self.panel.asking() {
            ask(i, body);
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
                match handoff::decode(bytes) {
                    Ok(h) => Some(Msg::Handoff(h)),
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
    };
    let app = App { panel: Panel { hidden, ..Panel::default() } };
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

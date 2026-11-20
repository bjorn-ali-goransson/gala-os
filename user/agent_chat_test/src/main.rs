//! The agent panel's chat (agent.exe; ROADMAP LY "Change this program",
//! step 4; ARCHITECTURE "The agent panel", "Relay"), as a user sees it:
//! a request typed into the panel's prompt and sent is asked of the model
//! and its answer listed under it.
//!
//! The model is reached through `cargo xtask agent-relay` (QEMU's
//! guestfwd to 10.0.2.101:80), which replays the call recorded in
//! `xtask/agent-recordings/`: the answer is the same on every run.
//!
//! - once the panel is docked (the work area ends at it), this test's
//!   own window, made active, is the panel's context;
//! - the panel's prompt, found through UI Automation ("Prompt", an Edit),
//!   is announced for the scenario's `click_on` (the click makes the
//!   panel active: Windows activates a clicked popup), which then types
//!   `say pong`, read back as the Edit's value (the Value pattern);
//! - its Send button ("SendButton", a Button shown once there is a
//!   draft) is announced and clicked;
//! - the request is listed ("Request0Text" `say pong`) with its reply
//!   card ("Reply0", Text) named with the model's answer, which says pong
//!   (not "Asking the model..." nor "No answer: ..."); the prompt is
//!   empty again and Send gone;
//! - then this test's own window is announced and the test waits for the
//!   scenario's click on it, so the run ends after the last screen step.
//!
//! Prints `agent_chat_test: typed`, `agent_chat_test: answered: <the
//! answer>`, `agent_chat_test: reply listed` and `agent_chat_test: ok`, or
//! `agent_chat_test: check N got X (line L)` and exits with N.

#![no_std]
#![no_main]

use core::sync::atomic::{AtomicBool, Ordering};
use ntrt as _;
use uia_client::{BUTTON, EDIT, Element, TEXT, Uia};

const NAME: &str = "agent_chat_test";
/// What the scenario types (AGENT_CHAT_SCREEN).
const PROMPT: &str = "say pong";

type Handle = usize;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, overlapped: usize) -> i32;
    fn ExitProcess(code: u32) -> !;
    fn GetModuleHandleW(name: *const u16) -> Handle;
    fn Sleep(ms: u32);
    fn GetTickCount64() -> u64;
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
    fn DispatchMessageW(msg: *const Msg) -> isize;
    fn ShowWindow(hwnd: Handle, cmd: i32) -> i32;
    fn SetForegroundWindow(hwnd: Handle) -> i32;
    fn FindWindowW(class: *const u16, title: *const u16) -> Handle;
    fn SystemParametersInfoW(action: u32, param: u32, out: *mut [i32; 4], ini: u32) -> i32;
    fn GetSystemMetrics(index: i32) -> i32;
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

const SPI_GETWORKAREA: u32 = 0x30;
const SM_CXSCREEN: i32 = 0;
/// The panel's docked width (agent's WIDTH).
const WIDTH: i32 = 320;
const WS_OVERLAPPEDWINDOW: u32 = 0x00cf_0000;
const SW_SHOW: i32 = 5;
/// How long a step may take: the scenario's clicks come once this test
/// has printed what the step waits for, and the answer once the relay
/// has replayed it.
const STEP_MS: u64 = 60_000;

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

/// Dispatches the waiting messages for up to `ms`, or until `done`.
fn pump(ms: u64, done: impl Fn() -> bool) -> bool {
    // SAFETY: our message structure.
    unsafe {
        let end = GetTickCount64() + ms;
        loop {
            let mut m: Msg = core::mem::zeroed();
            while PeekMessageW(&mut m, 0, 0, 0, 1) != 0 {
                DispatchMessageW(&m);
            }
            if done() {
                return true;
            }
            if GetTickCount64() >= end {
                return false;
            }
            Sleep(10);
        }
    }
}

// ---- The checks -----------------------------------------------------------

/// Whether this test's window was clicked (the scenario's last step).
static CLICKED: AtomicBool = AtomicBool::new(false);
const WM_LBUTTONUP: u32 = 0x0202;

extern "system" fn proc_(hwnd: Handle, msg: u32, wparam: usize, lparam: isize) -> isize {
    if msg == WM_LBUTTONUP {
        CLICKED.store(true, Ordering::Relaxed);
    }
    // SAFETY: the default handling.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    // SAFETY: plain calls with our structures.
    unsafe {
        // The panel, started with the session, docked: the work area ends
        // at it (it watches the active window from before it docks).
        let (class, title) = (W::new("GalaUiWindow"), W::new("Agent"));
        let sw = GetSystemMetrics(SM_CXSCREEN);
        let docked = pump(STEP_MS, || {
            let mut work = [0i32; 4];
            SystemParametersInfoW(SPI_GETWORKAREA, 0, &mut work, 0);
            FindWindowW(class.ptr(), title.ptr()) != 0 && work[2] == sw - WIDTH
        });
        c.ok(docked, 0)?;
        let panel = FindWindowW(class.ptr(), title.ptr());

        // This test's window, active: the panel's context.
        let cls = W::new("AgentChatTestWindow");
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
            class: cls.ptr(),
        };
        c.ok(RegisterClassW(&wc) != 0, 0)?;
        let own_title = W::new("Agent chat test window");
        let own = CreateWindowExW(
            0,
            cls.ptr(),
            own_title.ptr(),
            WS_OVERLAPPEDWINDOW,
            120,
            120,
            480,
            300,
            0,
            0,
            wc.instance,
            0,
        );
        c.ok(own != 0, 0)?;
        ShowWindow(own, SW_SHOW);
        SetForegroundWindow(own);

        let uia = Uia::new();
        c.ok(uia.is_ok(), uia.as_ref().err().copied().unwrap_or(0).into())?;
        let uia = uia.unwrap_or_else(|_| unreachable!());
        let find = |id: &str| -> Option<Element> { uia.find(&uia.window(panel)?, id) };
        // The context is this window before anything is asked (the
        // request body names it).
        let named =
            pump(STEP_MS, || find("ContextProgram").is_some_and(|e| e.name() == "agent_chat_test"));
        c.ok(named, 0)?;

        // The prompt: the scenario clicks it and types.
        let prompt = find("Prompt");
        c.ok(prompt.is_some(), 0)?;
        let prompt = prompt.unwrap_or_else(|| unreachable!());
        c.eq(prompt.control_type().into(), EDIT.into())?;
        uia_client::announce(NAME, "Prompt", &prompt);
        // The whole draft (the scenario types it a key at a time; Send
        // shows from its first letter).
        let typed = pump(STEP_MS, || find("Prompt").is_some_and(|e| e.value() == PROMPT));
        c.ok(typed, 0)?;
        let send = find("SendButton");
        c.ok(send.as_ref().is_some_and(|e| e.name() == "Send"), 0)?;
        let send = send.unwrap_or_else(|| unreachable!());
        c.eq(send.control_type().into(), BUTTON.into())?;
        uia_client::announce(NAME, "SendButton", &send);
        say(b"typed");

        // Sent: listed, then answered through the relay.
        let listed = pump(STEP_MS, || find("Request0Text").is_some_and(|e| e.name() == PROMPT));
        c.ok(listed, 0)?;
        let answered =
            pump(STEP_MS, || find("Reply0").is_some_and(|e| e.name() != "Asking the model..."));
        c.ok(answered, 0)?;
        let reply = find("Reply0");
        c.ok(reply.is_some(), 0)?;
        let reply = reply.unwrap_or_else(|| unreachable!());
        c.eq(reply.control_type().into(), TEXT.into())?;
        let said = reply.name();
        let said = said.as_str();
        Line::new().s(NAME.as_bytes()).s(b": answered: ").s(said.as_bytes()).s(b"\r\n").print();
        c.ok(!said.is_empty() && !said.starts_with("No answer"), 0)?;
        let pong = said.as_bytes().windows(4).any(|w| w.eq_ignore_ascii_case(b"pong"));
        c.ok(pong, said.len() as i64)?;
        // The prompt is empty again, and Send gone with the draft.
        c.ok(find("Prompt").is_some_and(|e| e.value().as_str().is_empty()), 0)?;
        c.ok(find("SendButton").is_none(), 0)?;

        // This test's own window is the last click: the screen scenario
        // checks the reply before it.
        let window = uia.window(own);
        c.ok(window.is_some(), 0)?;
        let window = window.unwrap_or_else(|| unreachable!());
        uia_client::announce(NAME, "TestWindow", &window);
        say(b"reply listed");
        c.ok(pump(STEP_MS, || CLICKED.load(Ordering::Relaxed)), 0)?;
        DestroyWindow(own);
    }
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    match run() {
        Ok(()) => {
            say(b"ok");
            // SAFETY: plain call.
            unsafe { ExitProcess(0) }
        }
        // SAFETY: plain call.
        Err(n) => unsafe { ExitProcess(n) },
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xc000_0409) }
}

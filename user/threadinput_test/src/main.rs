//! `AttachThreadInput` and the input state two threads share, as Windows
//! answers it. Word delay-loads `USER32.dll!AttachThreadInput` and ends
//! on 0xc06d007f without it, after its splash is up (ROADMAP OF), so
//! this is the shape gala's user32 has to have.
//!
//! Two threads of this process own a window each: the main thread owns
//! `WA` and a second thread owns `WB` and pumps its messages. Everything
//! the second thread sees of the input state is read in its own window
//! procedure, which the main thread asks for with a posted message and
//! waits for, so no check is timed.
//!
//! What the program asserts, measured on real Windows (the `state` lines
//! are the whole transcript, printed whether or not the checks hold, and
//! name which window each thread sees rather than its handle, so two
//! runs' transcripts can be compared):
//! - the calls that are refused: a thread attached to itself, to and
//!   from thread 0, and to and from a thread id nothing owns, each with
//!   the error it leaves;
//! - a thread that has never called user32 has no message queue, and
//!   what attaching to it answers;
//! - before attaching, each thread's focus is its own window and
//!   `SetFocus` of the other thread's window is refused;
//! - attaching joins the queues: what the merged input state's focus and
//!   active window then are, that both threads see the same, and that
//!   `SetFocus` of the other thread's window now holds for both;
//! - attaching a pair that is already attached, detaching it, what the
//!   focus is once they are apart again, and detaching a pair that is
//!   not attached.
//!
//! Prints `threadinput_test: ok (N checks)`, or `threadinput_test: check
//! N got X (line L)` and exits with N.

#![no_std]
#![no_main]

use core::ffi::c_void;
use core::ptr::null_mut;
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use ntrt as _;

const NAME: &str = "threadinput_test";

/// Every check records its mismatch and goes on, so one run on Windows
/// reports every answer that differs from the table below. The committed
/// program stops at the first (the transcript is printed either way).
const MEASURE: bool = false;

type Handle = *mut c_void;
type Hwnd = usize;
type WndProc = extern "system" fn(Hwnd, u32, usize, isize) -> isize;

#[repr(C)]
struct WndClassExW {
    size: u32,
    style: u32,
    proc_: WndProc,
    class_extra: i32,
    window_extra: i32,
    instance: Handle,
    icon: Handle,
    cursor: Handle,
    background: Handle,
    menu_name: *const u16,
    class_name: *const u16,
    icon_small: Handle,
}

#[repr(C)]
struct Msg {
    hwnd: Hwnd,
    message: u32,
    wparam: usize,
    lparam: isize,
    time: u32,
    pt: [i32; 2],
    private: u32,
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, overlapped: usize) -> i32;
    fn ExitProcess(code: u32) -> !;
    fn GetLastError() -> u32;
    fn SetLastError(err: u32);
    fn GetCurrentThreadId() -> u32;
    fn GetModuleHandleW(name: *const u16) -> Handle;
    fn CreateThread(
        sa: usize,
        stack: usize,
        start: extern "system" fn(*mut c_void) -> u32,
        param: *mut c_void,
        flags: u32,
        id: *mut u32,
    ) -> Handle;
    fn CreateEventW(sa: usize, manual: i32, signalled: i32, name: *const u16) -> Handle;
    fn SetEvent(event: Handle) -> i32;
    fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
    fn CloseHandle(h: Handle) -> i32;
}

#[link(name = "user32", kind = "raw-dylib")]
unsafe extern "system" {
    fn RegisterClassExW(class: *const WndClassExW) -> u16;
    fn CreateWindowExW(
        ex_style: u32,
        class: *const u16,
        name: *const u16,
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
    fn DestroyWindow(hwnd: Hwnd) -> i32;
    fn GetMessageW(msg: *mut Msg, hwnd: Hwnd, min: u32, max: u32) -> i32;
    fn DispatchMessageW(msg: *const Msg) -> isize;
    fn PostMessageW(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> i32;
    fn PostQuitMessage(code: i32);
    fn SetFocus(hwnd: Hwnd) -> Hwnd;
    fn GetFocus() -> Hwnd;
    fn GetActiveWindow() -> Hwnd;
    fn AttachThreadInput(attach: u32, attach_to: u32, fattach: i32) -> i32;
    fn LoadCursorW(instance: Handle, name: usize) -> Handle;
}

const WM_APP: u32 = 0x8000;
/// What the second thread is asked for: it reads the input state in its
/// own window procedure and signals `ACK`.
const WM_ASK: u32 = WM_APP + 1;
/// The second thread's window procedure ends its loop.
const WM_STOP: u32 = WM_APP + 2;
const WS_OVERLAPPEDWINDOW: u32 = 0x00cf_0000;
const IDC_ARROW: usize = 32512;
/// A thread id nothing owns: Windows' are small and a multiple of four,
/// and this is neither in use nor zero (which is its own check).
const NO_SUCH_THREAD: u32 = 0x7fff_fffc;
/// How long a wait on the second thread may take before the run is
/// called broken. It is a give-up, never a measurement: every answer
/// this program checks is read in a window procedure the main thread
/// waited for.
const GIVE_UP_MS: u32 = 30_000;

const ERROR_INVALID_PARAMETER: u32 = 87;
const ERROR_ACCESS_DENIED: u32 = 5;

static WA: AtomicUsize = AtomicUsize::new(0);
static WB: AtomicUsize = AtomicUsize::new(0);
/// What the second thread saw the last time it was asked.
static B_FOCUS: AtomicUsize = AtomicUsize::new(0);
static B_ACTIVE: AtomicUsize = AtomicUsize::new(0);
/// Set by the second thread once its window is up, and after each answer.
static READY: AtomicUsize = AtomicUsize::new(0);
static ACK: AtomicUsize = AtomicUsize::new(0);
/// The thread that owns no window and never calls user32: it waits on
/// this and ends.
static QUIET_STOP: AtomicUsize = AtomicUsize::new(0);
static B_TID: AtomicU32 = AtomicU32::new(0);
static QUIET_TID: AtomicU32 = AtomicU32::new(0);

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

    fn dec(self, v: u64) -> Self {
        let mut d = [0u8; 20];
        let mut i = d.len();
        let mut v = v;
        loop {
            i -= 1;
            d[i] = b'0' + (v % 10) as u8;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        self.s(&d[i..])
    }

    fn hex(self, v: u64) -> Self {
        let mut d = [0u8; 10];
        d[0] = b'0';
        d[1] = b'x';
        for i in 0..8 {
            d[2 + i] = b"0123456789abcdef"[((v >> (28 - 4 * i)) & 0xf) as usize];
        }
        self.s(&d)
    }

    /// Which window a handle is, not its value: a transcript that two
    /// runs can be compared by.
    fn who(self, h: Hwnd) -> Self {
        if h == 0 {
            self.s(b"none")
        } else if h == WA.load(Ordering::Relaxed) {
            self.s(b"WA")
        } else if h == WB.load(Ordering::Relaxed) {
            self.s(b"WB")
        } else {
            self.s(b"other")
        }
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

/// The check counter and the first check that was wrong (0 when none).
struct Checks(u32, u32);

impl Checks {
    #[track_caller]
    fn ok(&mut self, cond: bool, got: u64) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        Line::new()
            .s(NAME.as_bytes())
            .s(b": check ")
            .dec(u64::from(self.0))
            .s(b" got ")
            .hex(got)
            .s(b" (line ")
            .dec(u64::from(core::panic::Location::caller().line()))
            .s(if MEASURE { b", continuing)\r\n" } else { b")\r\n" })
            .print();
        if self.1 == 0 {
            self.1 = self.0;
        }
        if MEASURE { Ok(()) } else { Err(self.0) }
    }

    #[track_caller]
    fn eq(&mut self, got: u64, want: u64) -> Result<(), u32> {
        self.ok(got == want, got)
    }

    /// A window handle against which window it should be: the mismatch
    /// prints the handle, and the transcript line beside it says which
    /// window that was.
    #[track_caller]
    fn win(&mut self, got: Hwnd, want: Hwnd) -> Result<(), u32> {
        self.ok(got == want, got as u64)
    }
}

/// `AttachThreadInput` with the error it leaves, from a known start.
fn attach(a: u32, b: u32, on: bool) -> (i32, u32) {
    // SAFETY: plain calls.
    unsafe {
        SetLastError(0);
        let r = AttachThreadInput(a, b, i32::from(on));
        (r, GetLastError())
    }
}

fn say_call(what: &[u8], r: u64, err: u32) {
    Line::new()
        .s(NAME.as_bytes())
        .s(b": call ")
        .s(what)
        .s(b" ret=")
        .hex(r)
        .s(b" err=")
        .dec(u64::from(err))
        .s(b"\r\n")
        .print();
}

/// `SetFocus` with the error it leaves, from a known start.
fn set_focus(h: Hwnd) -> (Hwnd, u32) {
    // SAFETY: plain calls.
    unsafe {
        SetLastError(0);
        let previous = SetFocus(h);
        (previous, GetLastError())
    }
}

/// A call that answers a window handle: which window it is, and the
/// handle for the record.
fn say_focus(what: &[u8], h: Hwnd, err: u32) {
    Line::new()
        .s(NAME.as_bytes())
        .s(b": call ")
        .s(what)
        .s(b" ret=")
        .who(h)
        .s(b" (")
        .hex(h as u64)
        .s(b") err=")
        .dec(u64::from(err))
        .s(b"\r\n")
        .print();
}

/// What both threads see, as one line. The second thread's half is read
/// in its own window procedure, which this waits for.
fn state(tag: &[u8]) -> (Hwnd, Hwnd, Hwnd, Hwnd) {
    // SAFETY: plain calls.
    let (a_focus, a_active) = unsafe { (GetFocus(), GetActiveWindow()) };
    ask_b();
    let (b_focus, b_active) = (B_FOCUS.load(Ordering::SeqCst), B_ACTIVE.load(Ordering::SeqCst));
    Line::new()
        .s(NAME.as_bytes())
        .s(b": state ")
        .s(tag)
        .s(b" a-focus=")
        .who(a_focus)
        .s(b" a-active=")
        .who(a_active)
        .s(b" b-focus=")
        .who(b_focus)
        .s(b" b-active=")
        .who(b_active)
        .s(b"\r\n")
        .print();
    (a_focus, a_active, b_focus, b_active)
}

/// Asks the second thread for the input state it sees and waits for its
/// answer.
fn ask_b() {
    let ack = ACK.load(Ordering::SeqCst) as Handle;
    // SAFETY: the window and the event are this program's.
    unsafe {
        PostMessageW(WB.load(Ordering::SeqCst), WM_ASK, 0, 0);
        WaitForSingleObject(ack, GIVE_UP_MS);
    }
}

extern "system" fn wndproc(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize {
    match msg {
        WM_ASK => {
            // SAFETY: plain calls, on the thread that owns this window.
            unsafe {
                B_FOCUS.store(GetFocus(), Ordering::SeqCst);
                B_ACTIVE.store(GetActiveWindow(), Ordering::SeqCst);
                SetEvent(ACK.load(Ordering::SeqCst) as Handle);
            }
            0
        }
        WM_STOP => {
            // SAFETY: plain call.
            unsafe { PostQuitMessage(0) };
            0
        }
        // SAFETY: plain call.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// The second GUI thread: its own window, its own queue, and a loop that
/// answers what it is asked.
extern "system" fn gui_thread(_: *mut c_void) -> u32 {
    // SAFETY: the class is registered; the strings are NUL-terminated.
    unsafe {
        B_TID.store(GetCurrentThreadId(), Ordering::SeqCst);
        let w = CreateWindowExW(
            0,
            CLASS.as_ptr(),
            TITLE_B.as_ptr(),
            WS_OVERLAPPEDWINDOW,
            60,
            60,
            200,
            150,
            0,
            null_mut(),
            GetModuleHandleW(null_mut()),
            null_mut(),
        );
        WB.store(w, Ordering::SeqCst);
        SetFocus(w);
        SetEvent(READY.load(Ordering::SeqCst) as Handle);
        let mut msg: Msg = core::mem::zeroed();
        while GetMessageW(&mut msg, 0, 0, 0) > 0 {
            DispatchMessageW(&msg);
        }
        DestroyWindow(w);
    }
    0
}

/// A thread that never calls user32, so it has no message queue.
extern "system" fn quiet_thread(_: *mut c_void) -> u32 {
    // SAFETY: the event is this program's.
    unsafe {
        QUIET_TID.store(GetCurrentThreadId(), Ordering::SeqCst);
        SetEvent(READY.load(Ordering::SeqCst) as Handle);
        WaitForSingleObject(QUIET_STOP.load(Ordering::SeqCst) as Handle, GIVE_UP_MS);
    }
    0
}

/// An ASCII name as the NUL-terminated wide string the window functions
/// take.
const fn wide<const N: usize>(s: &[u8]) -> [u16; N] {
    let mut out = [0u16; N];
    let mut i = 0;
    while i < s.len() {
        out[i] = s[i] as u16;
        i += 1;
    }
    out
}

const CLASS: [u16; 16] = wide(b"GalaThreadInput");
const TITLE_A: [u16; 3] = wide(b"WA");
const TITLE_B: [u16; 3] = wide(b"WB");

/// Returns how many checks held, which the `ok` line prints.
fn run() -> Result<u32, u32> {
    let mut c = Checks(0, 0);
    // SAFETY: the class and the events are this program's; the window
    // procedure has the right signature.
    unsafe {
        let class = WndClassExW {
            size: core::mem::size_of::<WndClassExW>() as u32,
            style: 0,
            proc_: wndproc,
            class_extra: 0,
            window_extra: 0,
            instance: GetModuleHandleW(null_mut()),
            icon: null_mut(),
            cursor: LoadCursorW(null_mut(), IDC_ARROW),
            background: null_mut(),
            menu_name: null_mut(),
            class_name: CLASS.as_ptr(),
            icon_small: null_mut(),
        };
        c.ok(RegisterClassExW(&class) != 0, u64::from(GetLastError()))?;
        READY.store(CreateEventW(0, 0, 0, null_mut()) as usize, Ordering::SeqCst);
        ACK.store(CreateEventW(0, 0, 0, null_mut()) as usize, Ordering::SeqCst);
        QUIET_STOP.store(CreateEventW(0, 1, 0, null_mut()) as usize, Ordering::SeqCst);
        let wa = CreateWindowExW(
            0,
            CLASS.as_ptr(),
            TITLE_A.as_ptr(),
            WS_OVERLAPPEDWINDOW,
            10,
            10,
            200,
            150,
            0,
            null_mut(),
            GetModuleHandleW(null_mut()),
            null_mut(),
        );
        WA.store(wa, Ordering::SeqCst);
        c.ok(wa != 0, u64::from(GetLastError()))?;
        SetFocus(wa);
    }
    let a_tid = {
        // SAFETY: a plain call.
        unsafe { GetCurrentThreadId() }
    };
    let wa = WA.load(Ordering::SeqCst);
    let ready = READY.load(Ordering::SeqCst) as Handle;

    // The two other threads, each waited for by name.
    // SAFETY: the thread functions and the events are this program's.
    let (b_handle, quiet_handle) = unsafe {
        let q = CreateThread(0, 0, quiet_thread, null_mut(), 0, null_mut());
        WaitForSingleObject(ready, GIVE_UP_MS);
        let b = CreateThread(0, 0, gui_thread, null_mut(), 0, null_mut());
        WaitForSingleObject(ready, GIVE_UP_MS);
        (b, q)
    };
    let b_tid = B_TID.load(Ordering::SeqCst);
    let quiet_tid = QUIET_TID.load(Ordering::SeqCst);
    let wb = WB.load(Ordering::SeqCst);
    c.ok(wb != 0, 0)?;
    Line::new()
        .s(NAME.as_bytes())
        .s(b": windows wa=")
        .hex(wa as u64)
        .s(b" wb=")
        .hex(wb as u64)
        .s(b"
")
        .print();
    c.ok(b_tid != 0 && quiet_tid != 0 && b_tid != a_tid, u64::from(b_tid))?;

    // Refused: a thread attached to itself, and thread 0 either way.
    let (r, err) = attach(a_tid, a_tid, true);
    say_call(b"self", u64::from(r as u32), err);
    c.eq(u64::from(r as u32), 0)?;
    c.eq(u64::from(err), u64::from(ERROR_INVALID_PARAMETER))?;
    let (r, err) = attach(a_tid, a_tid, false);
    say_call(b"self-detach", u64::from(r as u32), err);
    c.eq(u64::from(r as u32), 0)?;
    c.eq(u64::from(err), u64::from(ERROR_INVALID_PARAMETER))?;
    let (r, err) = attach(a_tid, 0, true);
    say_call(b"to-zero", u64::from(r as u32), err);
    c.eq(u64::from(r as u32), 0)?;
    c.eq(u64::from(err), u64::from(ERROR_INVALID_PARAMETER))?;
    let (r, err) = attach(0, a_tid, true);
    say_call(b"from-zero", u64::from(r as u32), err);
    c.eq(u64::from(r as u32), 0)?;
    c.eq(u64::from(err), u64::from(ERROR_INVALID_PARAMETER))?;
    let (r, err) = attach(a_tid, NO_SUCH_THREAD, true);
    say_call(b"to-no-such", u64::from(r as u32), err);
    c.eq(u64::from(r as u32), 0)?;
    c.eq(u64::from(err), u64::from(ERROR_INVALID_PARAMETER))?;
    let (r, err) = attach(NO_SUCH_THREAD, a_tid, true);
    say_call(b"from-no-such", u64::from(r as u32), err);
    c.eq(u64::from(r as u32), 0)?;
    c.eq(u64::from(err), u64::from(ERROR_INVALID_PARAMETER))?;

    // A thread that has never called user32 has no message queue.
    let (r, err) = attach(a_tid, quiet_tid, true);
    say_call(b"to-queueless", u64::from(r as u32), err);
    c.eq(u64::from(r as u32), 0)?;
    c.eq(u64::from(err), u64::from(ERROR_INVALID_PARAMETER))?;

    // Apart: each thread's focus is its own window, and neither can set
    // the other's.
    let (a_focus, _, b_focus, _) = state(b"apart");
    c.win(a_focus, wa)?;
    c.win(b_focus, wb)?;
    let (set, err) = set_focus(wb);
    say_focus(b"setfocus-other-apart", set, err);
    c.win(set, 0)?;
    c.eq(u64::from(err), u64::from(ERROR_ACCESS_DENIED))?;
    let (a_focus, _, b_focus, _) = state(b"apart-after-setfocus");
    c.win(a_focus, wa)?;
    c.win(b_focus, wb)?;

    // Within one thread's own queue, `SetFocus` answers the window it
    // took the focus from: none once it has been cleared. This is what
    // tells that answer from the window the call itself focused, which
    // is what the attached calls below answer instead.
    let (set, err) = set_focus(0);
    say_focus(b"setfocus-none-apart", set, err);
    c.win(set, wa)?;
    let (a_focus, _, _, _) = state(b"apart-no-focus");
    c.win(a_focus, 0)?;
    let (set, err) = set_focus(wa);
    say_focus(b"setfocus-back-apart", set, err);
    c.win(set, 0)?;
    let (a_focus, _, _, _) = state(b"apart-focus-back");
    c.win(a_focus, wa)?;

    // Attached: one input state for both.
    let (r, err) = attach(a_tid, b_tid, true);
    say_call(b"attach", u64::from(r as u32), err);
    c.ok(r != 0, u64::from(err))?;
    let (a_focus, a_active, b_focus, b_active) = state(b"attached");
    c.win(a_focus, wb)?;
    c.win(b_focus, wb)?;
    // The active window is the merged state's too, whatever it is here
    // (the transcript's `a-active` says which): both threads see one.
    c.win(b_active, a_active)?;

    // What one thread focuses the other sees, the other's window
    // included -- and the call answers the window it focused, not the
    // one it took the focus from: the window it is given is not the
    // active one of the merged state, so the call activates it first and
    // the focus it replaces is by then its own.
    let (set, err) = set_focus(wa);
    say_focus(b"setfocus-own-attached", set, err);
    c.win(set, wa)?;
    let (a_focus, _, b_focus, _) = state(b"attached-focus-wa");
    c.win(a_focus, wa)?;
    c.win(b_focus, wa)?;
    let (set, err) = set_focus(wb);
    say_focus(b"setfocus-other-attached", set, err);
    c.win(set, wb)?;
    let (a_focus, _, b_focus, _) = state(b"attached-focus-wb");
    c.win(a_focus, wb)?;
    c.win(b_focus, wb)?;

    // Attaching a pair that is attached, and detaching it twice.
    let (r, err) = attach(a_tid, b_tid, true);
    say_call(b"attach-again", u64::from(r as u32), err);
    c.ok(r != 0, u64::from(err))?;
    let (r, err) = attach(a_tid, b_tid, false);
    say_call(b"detach", u64::from(r as u32), err);
    c.ok(r != 0, u64::from(err))?;
    // Detaching does not undo what the merged state had: each thread
    // keeps the focus the two of them shared.
    let (a_focus, _, b_focus, _) = state(b"detached");
    c.win(a_focus, wb)?;
    c.win(b_focus, wb)?;
    let (r, err) = attach(a_tid, b_tid, false);
    say_call(b"detach-again", u64::from(r as u32), err);
    c.ok(r != 0, u64::from(err))?;

    // Let the other threads end.
    // SAFETY: the window, the events and the handles are this program's.
    unsafe {
        PostMessageW(wb, WM_STOP, 0, 0);
        WaitForSingleObject(b_handle, GIVE_UP_MS);
        SetEvent(QUIET_STOP.load(Ordering::SeqCst) as Handle);
        WaitForSingleObject(quiet_handle, GIVE_UP_MS);
        CloseHandle(b_handle);
        CloseHandle(quiet_handle);
        DestroyWindow(wa);
    }

    if c.1 != 0 {
        return Err(c.1);
    }
    Ok(c.0)
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = match run() {
        Ok(checks) => {
            Line::new()
                .s(NAME.as_bytes())
                .s(b": ok (")
                .dec(u64::from(checks))
                .s(b" checks)\r\n")
                .print();
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

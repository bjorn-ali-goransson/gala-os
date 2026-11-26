//! Typing, clicking and turning the wheel on a USB keyboard and mouse
//! (the only ones the machine has: xHCI with QEMU's usb-kbd and
//! usb-mouse, no PS/2 controller), with the results Windows documents:
//!
//! - A WS_OVERLAPPEDWINDOW window, 400 x 300 at (100, 100), painted blue,
//!   prints `usbinput_test: ready` once it has painted.
//! - xtask types "a": WM_KEYDOWN with VK_A (0x41) and, through
//!   TranslateMessage, WM_CHAR 'a' (`usbinput_test: typed a`).
//! - Shift+A: on A's WM_KEYDOWN GetKeyState(VK_SHIFT) says Shift is down,
//!   and WM_CHAR is 'A' (`usbinput_test: typed A`).
//! - The pointer moves from the middle of the 1280 x 800 screen to (300,
//!   250) and the left button goes down and up: WM_LBUTTONDOWN (MK_LBUTTON
//!   in wParam) and WM_LBUTTONUP, each at the client point ScreenToClient
//!   gives for (300, 250) (`usbinput_test: clicked`).
//! - The wheel turns one notch away from the user: WM_MOUSEWHEEL with a
//!   delta of +120 (WHEEL_DELTA) in wParam's high word
//!   (`usbinput_test: wheel`), and the program ends.
//!
//! A thread posts WM_CLOSE after 60 seconds if the input does not come (a
//! failure). It prints `usbinput_test: ok` at the end, or exits with the
//! number of the failing check.

#![no_std]
#![no_main]

use core::ffi::c_void;
use core::sync::atomic::{AtomicI32, AtomicU32, AtomicUsize, Ordering};

type Handle = *mut c_void;
type Hwnd = *mut c_void;
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
#[derive(Clone, Copy, Default)]
struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

#[repr(C)]
struct PaintStruct {
    hdc: Handle,
    erase: i32,
    paint: Rect,
    restore: i32,
    inc_update: i32,
    reserved: [u8; 32],
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
    fn ShowWindow(hwnd: Hwnd, cmd: i32) -> i32;
    fn UpdateWindow(hwnd: Hwnd) -> i32;
    fn GetClientRect(hwnd: Hwnd, rect: *mut Rect) -> i32;
    fn BeginPaint(hwnd: Hwnd, ps: *mut PaintStruct) -> Handle;
    fn EndPaint(hwnd: Hwnd, ps: *const PaintStruct) -> i32;
    fn FillRect(hdc: Handle, rect: *const Rect, brush: Handle) -> i32;
    fn GetMessageW(msg: *mut Msg, hwnd: Hwnd, min: u32, max: u32) -> i32;
    fn DispatchMessageW(msg: *const Msg) -> isize;
    fn PostMessageW(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> i32;
    fn PostQuitMessage(code: i32);
    fn ScreenToClient(hwnd: Hwnd, point: *mut [i32; 2]) -> i32;
    fn GetKeyState(vk: i32) -> i16;
    fn TranslateMessage(msg: *const Msg) -> i32;
    fn LoadCursorW(instance: Handle, name: *const u16) -> Handle;
}

#[link(name = "gdi32", kind = "raw-dylib")]
unsafe extern "system" {
    fn CreateSolidBrush(color: u32) -> Handle;
    fn DeleteObject(object: Handle) -> i32;
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut c_void) -> i32;
    fn ExitProcess(code: u32) -> !;
    fn GetModuleHandleW(name: *const u16) -> Handle;
    fn Sleep(ms: u32);
    fn CreateThread(
        sa: *const c_void,
        stack: usize,
        start: extern "system" fn(*mut c_void) -> u32,
        param: *mut c_void,
        flags: u32,
        id: *mut u32,
    ) -> Handle;
}

// What the compiler's output expects from a C runtime.
#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

#[unsafe(no_mangle)]
pub extern "C" fn __CxxFrameHandler3() -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdeac) }
}

const WM_DESTROY: u32 = 0x0002;
const WM_PAINT: u32 = 0x000f;
const WM_CLOSE: u32 = 0x0010;
const WM_KEYDOWN: u32 = 0x0100;
const WM_CHAR: u32 = 0x0102;
const WM_LBUTTONDOWN: u32 = 0x0201;
const WM_LBUTTONUP: u32 = 0x0202;
const WM_MOUSEWHEEL: u32 = 0x020a;
const MK_LBUTTON: usize = 0x0001;
const VK_SHIFT: i32 = 0x10;
const VK_A: usize = 0x41;
const WHEEL_DELTA: i16 = 120;
const WS_OVERLAPPEDWINDOW: u32 = 0x00cf_0000;
const SW_SHOW: i32 = 5;
const COLOR_WINDOW: usize = 5;
const IDC_ARROW: usize = 32512;
/// The client area's blue, as a COLORREF (0x00BBGGRR): pixel 0x336699.
const BLUE: u32 = 0x0099_6633;
/// Where xtask clicks, on the screen.
const CLICK: [i32; 2] = [300, 250];

/// What the program waits for next.
const TYPE_A: u32 = 0;
const TYPE_SHIFT_A: u32 = 1;
const CLICK_DOWN: u32 = 2;
const CLICK_UP: u32 = 3;
const WHEEL: u32 = 4;
const DONE: u32 = 5;

static WINDOW: AtomicUsize = AtomicUsize::new(0);
static STAGE: AtomicU32 = AtomicU32::new(TYPE_A);
static PAINTS: AtomicU32 = AtomicU32::new(0);
static TIMED_OUT: AtomicU32 = AtomicU32::new(0);
/// Whether Shift was down at Shift+A's WM_KEYDOWN (-1: not seen).
static SHIFT_AT_A: AtomicI32 = AtomicI32::new(-1);
/// Checks run in order; the number of the first that failed, which the
/// program exits with.
static CHECKS: AtomicU32 = AtomicU32::new(0);
static FAILED: AtomicU32 = AtomicU32::new(0);

fn out(s: &[u8]) {
    // SAFETY: the console's handle (STD_OUTPUT_HANDLE) and our buffer.
    unsafe {
        let mut written = 0u32;
        WriteFile(
            GetStdHandle(0xffff_fff5),
            s.as_ptr(),
            s.len() as u32,
            &mut written,
            core::ptr::null_mut(),
        );
    }
}

fn fail(n: u32) {
    let _ = FAILED.compare_exchange(0, n, Ordering::SeqCst, Ordering::SeqCst);
    out(b"usbinput_test: check failed\r\n");
}

fn check(ok: bool) -> Result<(), u32> {
    let n = CHECKS.fetch_add(1, Ordering::SeqCst) + 1;
    if ok {
        return Ok(());
    }
    fail(n);
    Err(n)
}

fn wide<const N: usize>(s: &str) -> [u16; N] {
    let mut w = [0u16; N];
    let mut i = 0;
    while i < s.len() && i + 1 < N {
        w[i] = s.as_bytes()[i] as u16;
        i += 1;
    }
    w
}

/// The client area, blue.
fn paint(hwnd: Hwnd) {
    // SAFETY: a valid window; the structures are ours.
    unsafe {
        let mut ps: PaintStruct = core::mem::zeroed();
        let hdc = BeginPaint(hwnd, &mut ps);
        let mut client = Rect::default();
        GetClientRect(hwnd, &mut client);
        let blue = CreateSolidBrush(BLUE);
        let ok = !hdc.is_null() && !blue.is_null() && FillRect(hdc, &client, blue) != 0;
        DeleteObject(blue);
        EndPaint(hwnd, &ps);
        if !ok {
            fail(80);
        }
    }
}

/// Whether a mouse message's lParam is the client point of `CLICK`.
fn at_click(hwnd: Hwnd, lparam: isize) -> bool {
    let mut p = CLICK;
    // SAFETY: a valid window and our point.
    unsafe { ScreenToClient(hwnd, &mut p) };
    let x = (lparam & 0xffff) as u16 as i16 as i32;
    let y = ((lparam >> 16) & 0xffff) as u16 as i16 as i32;
    [x, y] == p
}

/// Moves from `from` to the next stage; anything else in this order is a
/// failure (check 70 + the stage it came in).
fn advance(from: u32, ok: bool, note: &[u8]) {
    if STAGE.load(Ordering::SeqCst) != from {
        return;
    }
    if !ok {
        fail(70 + from);
        // SAFETY: a plain call.
        unsafe { PostQuitMessage(0) };
        return;
    }
    out(note);
    STAGE.store(from + 1, Ordering::SeqCst);
    if from + 1 == DONE {
        // SAFETY: a plain call.
        unsafe { PostQuitMessage(0) };
    }
}

extern "system" fn window_proc(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize {
    match msg {
        WM_PAINT => {
            paint(hwnd);
            if PAINTS.fetch_add(1, Ordering::SeqCst) == 0 {
                out(b"usbinput_test: ready\r\n");
            }
            0
        }
        WM_KEYDOWN => {
            if wparam == VK_A && STAGE.load(Ordering::SeqCst) == TYPE_SHIFT_A {
                // SAFETY: a plain call.
                let shift = unsafe { GetKeyState(VK_SHIFT) } < 0;
                SHIFT_AT_A.store(shift as i32, Ordering::SeqCst);
            }
            0
        }
        WM_CHAR => {
            match STAGE.load(Ordering::SeqCst) {
                TYPE_A => advance(TYPE_A, wparam == 'a' as usize, b"usbinput_test: typed a\r\n"),
                TYPE_SHIFT_A => advance(
                    TYPE_SHIFT_A,
                    wparam == 'A' as usize && SHIFT_AT_A.load(Ordering::SeqCst) == 1,
                    b"usbinput_test: typed A\r\n",
                ),
                _ => {}
            }
            0
        }
        WM_LBUTTONDOWN => {
            let ok = wparam & MK_LBUTTON != 0 && at_click(hwnd, lparam);
            advance(CLICK_DOWN, ok, b"usbinput_test: button down\r\n");
            0
        }
        WM_LBUTTONUP => {
            let ok = wparam & MK_LBUTTON == 0 && at_click(hwnd, lparam);
            advance(CLICK_UP, ok, b"usbinput_test: clicked\r\n");
            0
        }
        WM_MOUSEWHEEL => {
            let delta = (wparam >> 16) as u16 as i16;
            advance(WHEEL, delta == WHEEL_DELTA, b"usbinput_test: wheel\r\n");
            0
        }
        WM_DESTROY => {
            // SAFETY: a plain call.
            unsafe { PostQuitMessage(0) };
            0
        }
        // SAFETY: the default processing (WM_CLOSE destroys the window).
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

extern "system" fn closer(_: *mut c_void) -> u32 {
    // SAFETY: plain calls; the window is the test's.
    unsafe {
        // xtask's input comes long before.
        Sleep(60_000);
        TIMED_OUT.store(1, Ordering::SeqCst);
        PostMessageW(WINDOW.load(Ordering::SeqCst) as Hwnd, WM_CLOSE, 0, 0);
    }
    0
}

fn run() -> Result<(), u32> {
    let class: [u16; 20] = wide("GalaUsbInput");
    let title: [u16; 32] = wide("USB keyboard and mouse");
    // SAFETY: valid structures, strings and handles throughout.
    unsafe {
        let instance = GetModuleHandleW(core::ptr::null());
        let wc = WndClassExW {
            size: size_of::<WndClassExW>() as u32,
            style: 0,
            proc_: window_proc,
            class_extra: 0,
            window_extra: 0,
            instance,
            icon: core::ptr::null_mut(),
            cursor: LoadCursorW(core::ptr::null_mut(), IDC_ARROW as *const u16),
            background: (COLOR_WINDOW + 1) as Handle,
            menu_name: core::ptr::null(),
            class_name: class.as_ptr(),
            icon_small: core::ptr::null_mut(),
        };
        check(RegisterClassExW(&wc) != 0)?;
        let hwnd = CreateWindowExW(
            0,
            class.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPEDWINDOW,
            100,
            100,
            400,
            300,
            core::ptr::null_mut(),
            core::ptr::null_mut(),
            instance,
            core::ptr::null_mut(),
        );
        check(!hwnd.is_null())?;
        WINDOW.store(hwnd as usize, Ordering::SeqCst);
        check(ShowWindow(hwnd, SW_SHOW) == 0)?; // it was hidden
        check(UpdateWindow(hwnd) != 0)?;
        check(PAINTS.load(Ordering::SeqCst) >= 1)?;
        let t = CreateThread(
            core::ptr::null(),
            0,
            closer,
            core::ptr::null_mut(),
            0,
            core::ptr::null_mut(),
        );
        check(!t.is_null())?;
        let mut msg: Msg = core::mem::zeroed();
        loop {
            let r = GetMessageW(&mut msg, core::ptr::null_mut(), 0, 0);
            if r == 0 {
                break;
            }
            check(r > 0)?;
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        // Everything came, in order, before the thread's WM_CLOSE.
        check(STAGE.load(Ordering::SeqCst) == DONE)?;
        check(TIMED_OUT.load(Ordering::SeqCst) == 0)?;
    }
    match FAILED.load(Ordering::SeqCst) {
        0 => Ok(()),
        n => Err(n),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = match run() {
        Ok(()) => {
            out(b"usbinput_test: ok\r\n");
            0
        }
        Err(id) => id,
    };
    // SAFETY: ends the process.
    unsafe { ExitProcess(code) }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdead) }
}

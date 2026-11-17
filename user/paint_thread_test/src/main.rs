//! What a thread other than the window's draws on a window DC shows on
//! the desktop while the window's thread waits for a message, with the
//! results Windows documents:
//!
//! - A WS_OVERLAPPEDWINDOW window, 400 x 300 at (100, 100), paints its
//!   client area blue in WM_PAINT (`paint_thread_test: painted`) and then
//!   waits for messages.
//! - A second thread waits until the window procedure has handled a
//!   message the main thread posted to itself (so the loop is running and
//!   its last idle moment is past), then draws an orange square at client
//!   (40, 180) - (120, 260) through a DC of its own: GetDC, FillRect,
//!   GetPixel reads the colour back (`paint_thread_test: drew from
//!   another thread`). It keeps the DC, as a Vulkan swapchain does
//!   (Mesa's win32 WSI holds the window's DC and blits each frame into
//!   it, and DXVK does that blit from a presenter thread of its own,
//!   which is this test's shape: `d3d11_test`).
//! - The square reaches the desktop although the window's thread does
//!   nothing: on Windows GDI drawing on a window DC goes to the device as
//!   it is drawn. xtask's screen check reads it from the screen; then a
//!   left click in the client area ends the program (a thread posts
//!   WM_CLOSE after 60 seconds if no click comes: a failure).
//!
//! It prints `paint_thread_test: ok` at the end, or exits with the number
//! of the failing check (the second thread's own are 90 and 91).

#![no_std]
#![no_main]

use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

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
    fn GetDC(hwnd: Hwnd) -> Handle;
    fn LoadCursorW(instance: Handle, name: *const u16) -> Handle;
}

#[link(name = "gdi32", kind = "raw-dylib")]
unsafe extern "system" {
    fn CreateSolidBrush(color: u32) -> Handle;
    fn DeleteObject(object: Handle) -> i32;
    fn GetPixel(hdc: Handle, x: i32, y: i32) -> u32;
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut c_void) -> i32;
    fn ExitProcess(code: u32) -> !;
    fn GetModuleHandleW(name: *const u16) -> Handle;
    fn Sleep(ms: u32);
    fn CreateEventW(sa: *const c_void, manual: i32, signalled: i32, name: *const u16) -> Handle;
    fn SetEvent(event: Handle) -> i32;
    fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
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
const WM_LBUTTONUP: u32 = 0x0202;
const WM_APP: u32 = 0x8000;
const WS_OVERLAPPEDWINDOW: u32 = 0x00cf_0000;
const SW_SHOW: i32 = 5;
const COLOR_WINDOW: usize = 5;
const IDC_ARROW: usize = 32512;
/// The client area's blue and the second thread's orange, as COLORREFs
/// (0x00BBGGRR): pixels 0x336699 and 0xff8000.
const BLUE: u32 = 0x0099_6633;
const ORANGE: u32 = 0x0000_80ff;
/// The square the second thread draws, in client coordinates.
const SQUARE: Rect = Rect { left: 40, top: 180, right: 120, bottom: 260 };
/// How long the second thread leaves the main thread alone once its loop
/// is running, in ms: enough for the idle moment that follows the message
/// it handled (`flush`), so that what this thread draws afterwards shows
/// only because gdi32 tells user32 of it.
const IDLE_MS: u32 = 200;

static WINDOW: AtomicUsize = AtomicUsize::new(0);
static READY: AtomicUsize = AtomicUsize::new(0);
static PAINTS: AtomicU32 = AtomicU32::new(0);
static DREW: AtomicU32 = AtomicU32::new(0);
static TIMED_OUT: AtomicU32 = AtomicU32::new(0);
static CLICKED: AtomicU32 = AtomicU32::new(0);
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
    out(b"paint_thread_test: check failed\r\n");
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

extern "system" fn window_proc(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize {
    match msg {
        WM_PAINT => {
            paint(hwnd);
            if PAINTS.fetch_add(1, Ordering::SeqCst) == 0 {
                out(b"paint_thread_test: painted\r\n");
            }
            0
        }
        // The main thread posted this to itself before its loop: handling
        // it says the loop is running (the second thread waits for it).
        WM_APP => {
            // SAFETY: the event the test made.
            unsafe { SetEvent(READY.load(Ordering::SeqCst) as Handle) };
            0
        }
        WM_LBUTTONUP => {
            CLICKED.fetch_add(1, Ordering::SeqCst);
            // SAFETY: a plain call.
            unsafe { PostQuitMessage(0) };
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

/// Draws the square on the window's DC, from a thread that is not the
/// window's and never releases the DC, as a swapchain's presenter does.
extern "system" fn drawer(_: *mut c_void) -> u32 {
    // SAFETY: plain calls; the window, event and brush are the test's.
    unsafe {
        // WAIT_OBJECT_0 (0): the window procedure handled WM_APP.
        if WaitForSingleObject(READY.load(Ordering::SeqCst) as Handle, 10_000) != 0 {
            fail(90);
            return 1;
        }
        Sleep(IDLE_MS);
        let hdc = GetDC(WINDOW.load(Ordering::SeqCst) as Hwnd);
        let orange = CreateSolidBrush(ORANGE);
        let drew = !hdc.is_null()
            && !orange.is_null()
            && FillRect(hdc, &SQUARE, orange) != 0
            && GetPixel(hdc, SQUARE.left + 20, SQUARE.top + 20) == ORANGE;
        DeleteObject(orange);
        if !drew {
            fail(91);
            return 1;
        }
    }
    DREW.store(1, Ordering::SeqCst);
    out(b"paint_thread_test: drew from another thread\r\n");
    0
}

extern "system" fn closer(_: *mut c_void) -> u32 {
    // SAFETY: plain calls; the window is the test's.
    unsafe {
        // xtask clicks in the client area long before.
        Sleep(60_000);
        TIMED_OUT.store(1, Ordering::SeqCst);
        PostMessageW(WINDOW.load(Ordering::SeqCst) as Hwnd, WM_CLOSE, 0, 0);
    }
    0
}

fn run() -> Result<(), u32> {
    let class: [u16; 20] = wide("GalaPaintThread");
    let title: [u16; 32] = wide("paint from another thread");
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
        let ready = CreateEventW(core::ptr::null(), 1, 0, core::ptr::null());
        check(!ready.is_null())?;
        READY.store(ready as usize, Ordering::SeqCst);
        check(ShowWindow(hwnd, SW_SHOW) == 0)?; // it was hidden
        check(UpdateWindow(hwnd) != 0)?;
        check(PAINTS.load(Ordering::SeqCst) >= 1)?;
        for start in [drawer as extern "system" fn(*mut c_void) -> u32, closer] {
            let t = CreateThread(
                core::ptr::null(),
                0,
                start,
                core::ptr::null_mut(),
                0,
                core::ptr::null_mut(),
            );
            check(!t.is_null())?;
        }
        check(PostMessageW(hwnd, WM_APP, 0, 0) != 0)?;
        let mut msg: Msg = core::mem::zeroed();
        loop {
            let r = GetMessageW(&mut msg, core::ptr::null_mut(), 0, 0);
            if r == 0 {
                break;
            }
            check(r > 0)?;
            DispatchMessageW(&msg);
        }
        // The square was drawn, the screen check saw it, and the click
        // ended the program (not the thread that posts WM_CLOSE).
        check(DREW.load(Ordering::SeqCst) == 1)?;
        check(CLICKED.load(Ordering::SeqCst) >= 1)?;
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
            out(b"paint_thread_test: ok\r\n");
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

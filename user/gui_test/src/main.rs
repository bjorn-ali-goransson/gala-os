//! A window, as a Win32 program makes one (the window manager's first
//! milestone), with the results Windows documents:
//!
//! - RegisterClassExW returns an atom; CreateWindowExW (WS_OVERLAPPEDWINDOW,
//!   400 x 300 at (100, 100)) sends WM_CREATE before it returns, with the
//!   lpCreateParams given; the client rectangle starts at (0, 0) and is
//!   smaller than the window; the window starts hidden (IsWindowVisible).
//! - ShowWindow(SW_SHOW) returns 0 (it was hidden); UpdateWindow sends
//!   WM_PAINT; BeginPaint gives a DC and rcPaint covering the client
//!   area; FillRect paints a red rectangle at (20, 20)-(200, 120) and
//!   TextOutW blue text at (20, 140); EndPaint.
//! - Input, which xtask gives through QEMU's mouse and keyboard: a left
//!   click in the client area is WM_LBUTTONDOWN (MK_LBUTTON in wParam, the
//!   client position in lParam) and WM_LBUTTONUP; the program paints a
//!   green square there (GetDC/FillRect/ReleaseDC). Typing "hi" gives
//!   WM_KEYDOWN/WM_KEYUP and, through TranslateMessage, WM_CHAR 'h' and
//!   'i'; the program draws what was typed in black at (20, 200).
//! - A click on the window's close button is WM_CLOSE; DefWindowProc
//!   destroys the window: WM_DESTROY, where the program calls
//!   PostQuitMessage(7); GetMessageW then returns 0 with wParam 7; IsWindow
//!   is FALSE afterwards. (A thread posts WM_CLOSE after 30 seconds if no
//!   click comes: a failure.)
//!
//! It prints `gui_test: painted` once it has painted (xtask then checks the
//! screen and gives the input), `gui_test: click`, `gui_test: typed hi`, and
//! `gui_test: ok` at the end, or exits with the number of the failing
//! check.

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

#[repr(C)]
struct CreateStructW {
    create_params: *mut c_void,
    instance: Handle,
    menu: Handle,
    parent: Hwnd,
    cy: i32,
    cx: i32,
    y: i32,
    x: i32,
    style: i32,
    name: *const u16,
    class: *const u16,
    ex_style: u32,
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
    fn IsWindowVisible(hwnd: Hwnd) -> i32;
    fn IsWindow(hwnd: Hwnd) -> i32;
    fn GetClientRect(hwnd: Hwnd, rect: *mut Rect) -> i32;
    fn BeginPaint(hwnd: Hwnd, ps: *mut PaintStruct) -> Handle;
    fn EndPaint(hwnd: Hwnd, ps: *const PaintStruct) -> i32;
    fn FillRect(hdc: Handle, rect: *const Rect, brush: Handle) -> i32;
    fn GetMessageW(msg: *mut Msg, hwnd: Hwnd, min: u32, max: u32) -> i32;
    fn TranslateMessage(msg: *const Msg) -> i32;
    fn DispatchMessageW(msg: *const Msg) -> isize;
    fn PostMessageW(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> i32;
    fn PostQuitMessage(code: i32);
    fn GetDC(hwnd: Hwnd) -> Handle;
    fn ReleaseDC(hwnd: Hwnd, hdc: Handle) -> i32;
    fn LoadCursorW(instance: Handle, name: *const u16) -> Handle;
}

#[link(name = "gdi32", kind = "raw-dylib")]
unsafe extern "system" {
    fn CreateSolidBrush(color: u32) -> Handle;
    fn DeleteObject(object: Handle) -> i32;
    fn SetTextColor(hdc: Handle, color: u32) -> u32;
    fn SetBkMode(hdc: Handle, mode: i32) -> i32;
    fn TextOutW(hdc: Handle, x: i32, y: i32, text: *const u16, n: i32) -> i32;
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

#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
}

const WM_CREATE: u32 = 0x0001;
const WM_DESTROY: u32 = 0x0002;
const WM_PAINT: u32 = 0x000f;
const WM_CLOSE: u32 = 0x0010;
const WM_KEYDOWN: u32 = 0x0100;
const WM_CHAR: u32 = 0x0102;
const WM_LBUTTONDOWN: u32 = 0x0201;
const WM_LBUTTONUP: u32 = 0x0202;
const MK_LBUTTON: usize = 1;
const VK_H: usize = 0x48;
const WS_OVERLAPPEDWINDOW: u32 = 0x00cf_0000;
const SW_SHOW: i32 = 5;
const TRANSPARENT: i32 = 1;
const COLOR_WINDOW: usize = 5;
const IDC_ARROW: usize = 32512;
const PARAM: usize = 0x5eed;
/// The text TextOutW draws.
const TEXT: &str = "Hello from gala";

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

/// Checks made in the window procedure: the first failing one's number.
static FAILED: AtomicU32 = AtomicU32::new(0);
static CHECK: AtomicU32 = AtomicU32::new(0);
static CREATED: AtomicU32 = AtomicU32::new(0);
static PAINTS: AtomicU32 = AtomicU32::new(0);
static DESTROYED: AtomicU32 = AtomicU32::new(0);
static WINDOW: AtomicUsize = AtomicUsize::new(0);
static DOWNS: AtomicU32 = AtomicU32::new(0);
static UPS: AtomicU32 = AtomicU32::new(0);
static KEYDOWN_H: AtomicU32 = AtomicU32::new(0);
static TIMED_OUT: AtomicU32 = AtomicU32::new(0);
/// What was typed (WM_CHAR), as ASCII.
static TYPED: [AtomicU32; 16] = [const { AtomicU32::new(0) }; 16];
static TYPED_LEN: AtomicUsize = AtomicUsize::new(0);

fn check(cond: bool, got: u64) -> Result<(), u32> {
    let n = CHECK.fetch_add(1, Ordering::SeqCst) + 1;
    if cond {
        return Ok(());
    }
    out(b"gui_test: check ");
    out(&hex(n));
    out(b" got ");
    out(&hex(got as u32));
    out(b"\r\n");
    let _ = FAILED.compare_exchange(0, n, Ordering::SeqCst, Ordering::SeqCst);
    Err(n)
}

fn wide<const N: usize>(s: &str) -> [u16; N] {
    let mut w = [0u16; N];
    for (d, b) in w.iter_mut().zip(s.bytes()) {
        *d = u16::from(b);
    }
    w
}

fn paint(hwnd: Hwnd) -> Result<(), u32> {
    // SAFETY: valid window, structures and strings.
    unsafe {
        let mut ps: PaintStruct = core::mem::zeroed();
        let hdc = BeginPaint(hwnd, &mut ps);
        check(!hdc.is_null(), 0)?;
        let mut client = Rect::default();
        GetClientRect(hwnd, &mut client);
        let p = ps.paint;
        check(
            p.left <= 0 && p.top <= 0 && p.right >= client.right && p.bottom >= client.bottom,
            p.right as u64,
        )?;
        let red = CreateSolidBrush(0x0000_00ff); // COLORREF 0x00BBGGRR
        check(!red.is_null(), 0)?;
        let r = Rect { left: 20, top: 20, right: 200, bottom: 120 };
        check(FillRect(hdc, &r, red) != 0, 0)?;
        DeleteObject(red);
        SetTextColor(hdc, 0x00ff_0000); // blue
        SetBkMode(hdc, TRANSPARENT);
        let text: [u16; 16] = wide(TEXT);
        check(TextOutW(hdc, 20, 140, text.as_ptr(), TEXT.len() as i32) != 0, 0)?;
        check(EndPaint(hwnd, &ps) != 0, 0)?;
    }
    Ok(())
}

/// A 12 x 12 green square centred on client point (x, y).
fn mark(hwnd: Hwnd, x: i32, y: i32) {
    // SAFETY: valid window and structures.
    unsafe {
        let hdc = GetDC(hwnd);
        let _ = check(!hdc.is_null(), 0);
        let green = CreateSolidBrush(0x0000_c000);
        let r = Rect { left: x - 6, top: y - 6, right: x + 6, bottom: y + 6 };
        let _ = check(FillRect(hdc, &r, green) != 0, 0);
        DeleteObject(green);
        let _ = check(ReleaseDC(hwnd, hdc) == 1, 0);
    }
}

/// Draws what was typed at (20, 200).
fn draw_typed(hwnd: Hwnd) {
    let n = TYPED_LEN.load(Ordering::SeqCst).min(16);
    let mut text = [0u16; 16];
    for (i, t) in text.iter_mut().enumerate().take(n) {
        *t = TYPED[i].load(Ordering::SeqCst) as u16;
    }
    // SAFETY: valid window and text.
    unsafe {
        let hdc = GetDC(hwnd);
        SetTextColor(hdc, 0);
        SetBkMode(hdc, TRANSPARENT);
        TextOutW(hdc, 20, 200, text.as_ptr(), n as i32);
        ReleaseDC(hwnd, hdc);
    }
}

extern "system" fn window_proc(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize {
    match msg {
        WM_LBUTTONDOWN => {
            let (x, y) = (lparam as i16 as i32, (lparam >> 16) as i16 as i32);
            let _ = check(wparam & MK_LBUTTON != 0, wparam as u64);
            let _ = check((0..398).contains(&x) && (0..268).contains(&y), lparam as u64);
            DOWNS.fetch_add(1, Ordering::SeqCst);
            mark(hwnd, x, y);
            out(b"gui_test: click\r\n");
            0
        }
        WM_LBUTTONUP => {
            let _ = check(wparam & MK_LBUTTON == 0, wparam as u64);
            UPS.fetch_add(1, Ordering::SeqCst);
            0
        }
        WM_KEYDOWN => {
            if wparam == VK_H {
                KEYDOWN_H.fetch_add(1, Ordering::SeqCst);
            }
            0
        }
        WM_CHAR => {
            let n = TYPED_LEN.fetch_add(1, Ordering::SeqCst);
            if n < 16 {
                TYPED[n].store(wparam as u32, Ordering::SeqCst);
            }
            draw_typed(hwnd);
            if n == 1
                && TYPED[0].load(Ordering::SeqCst) == u32::from(b'h')
                && wparam == usize::from(b'i')
            {
                out(b"gui_test: typed hi\r\n");
            }
            0
        }
        WM_CREATE => {
            // SAFETY: WM_CREATE's lParam is the CREATESTRUCTW.
            let params = unsafe { (*(lparam as *const CreateStructW)).create_params } as usize;
            let _ = check(params == PARAM, params as u64);
            CREATED.fetch_add(1, Ordering::SeqCst);
            0
        }
        WM_PAINT => {
            let _ = paint(hwnd);
            if PAINTS.fetch_add(1, Ordering::SeqCst) == 0 {
                out(b"gui_test: painted\r\n");
            }
            0
        }
        WM_DESTROY => {
            DESTROYED.fetch_add(1, Ordering::SeqCst);
            // SAFETY: a plain call.
            unsafe { PostQuitMessage(7) };
            0
        }
        // SAFETY: the default processing (WM_CLOSE destroys the window).
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

extern "system" fn closer(_: *mut c_void) -> u32 {
    // SAFETY: plain calls; the window is the test's.
    unsafe {
        // xtask clicks the close button long before.
        Sleep(30_000);
        TIMED_OUT.store(1, Ordering::SeqCst);
        PostMessageW(WINDOW.load(Ordering::SeqCst) as Hwnd, WM_CLOSE, 0, 0);
    }
    0
}

fn run() -> Result<(), u32> {
    let class: [u16; 16] = wide("GalaTestWindow");
    let title: [u16; 16] = wide("gala window");
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
        check(RegisterClassExW(&wc) != 0, 0)?;
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
            PARAM as *mut c_void,
        );
        check(!hwnd.is_null(), 0)?;
        WINDOW.store(hwnd as usize, Ordering::SeqCst);
        check(CREATED.load(Ordering::SeqCst) == 1, 0)?;
        let mut client = Rect::default();
        check(GetClientRect(hwnd, &mut client) != 0, 0)?;
        check(client.left == 0 && client.top == 0, client.left as u64)?;
        check(
            client.right > 0 && client.right < 400 && client.bottom > 0 && client.bottom < 300,
            client.right as u64,
        )?;
        check(IsWindowVisible(hwnd) == 0, 0)?;
        check(ShowWindow(hwnd, SW_SHOW) == 0, 0)?; // was hidden
        check(IsWindowVisible(hwnd) != 0, 0)?;
        check(UpdateWindow(hwnd) != 0, 0)?;
        check(PAINTS.load(Ordering::SeqCst) >= 1, 0)?;

        let t = CreateThread(
            core::ptr::null(),
            0,
            closer,
            core::ptr::null_mut(),
            0,
            core::ptr::null_mut(),
        );
        check(!t.is_null(), 0)?;
        let mut msg: Msg = core::mem::zeroed();
        loop {
            let r = GetMessageW(&mut msg, core::ptr::null_mut(), 0, 0);
            if r == 0 {
                break;
            }
            check(r > 0, r as u64)?;
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        check(msg.wparam == 7, msg.wparam as u64)?;
        check(DESTROYED.load(Ordering::SeqCst) == 1, 0)?;
        check(IsWindow(hwnd) == 0, 0)?;
        // The input arrived; the close button closed it.
        check(DOWNS.load(Ordering::SeqCst) >= 1, 0)?;
        check(UPS.load(Ordering::SeqCst) >= 1, 0)?;
        check(KEYDOWN_H.load(Ordering::SeqCst) == 1, 0)?;
        check(TYPED_LEN.load(Ordering::SeqCst) == 2, TYPED_LEN.load(Ordering::SeqCst) as u64)?;
        check(TIMED_OUT.load(Ordering::SeqCst) == 0, 0)?;
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
            out(b"gui_test: ok\r\n");
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

//! What the common controls share: user32 (imported: the controls are
//! windows), gdi32 (loaded on first use, as the image lists do: the JVM
//! loads comctl32 without gdi32), each control's state (`Store`), and
//! drawing in the Windows 11 style.

// WM_NOTIFY and the like are for the list and tree views (next).
#![allow(dead_code)]

use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::ffi::c_void;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

pub type Hwnd = usize;
pub type WndProc = extern "system" fn(Hwnd, u32, usize, isize) -> isize;

#[link(name = "user32", kind = "raw-dylib")]
unsafe extern "system" {
    pub fn RegisterClassExW(wc: *const WndClassExW) -> u16;
    pub fn DefWindowProcW(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize;
    pub fn SendMessageW(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize;
    pub fn GetParent(hwnd: Hwnd) -> Hwnd;
    pub fn GetClientRect(hwnd: Hwnd, rect: *mut [i32; 4]) -> i32;
    pub fn SetWindowPos(
        hwnd: Hwnd,
        after: Hwnd,
        x: i32,
        y: i32,
        cx: i32,
        cy: i32,
        flags: u32,
    ) -> i32;
    pub fn InvalidateRect(hwnd: Hwnd, rect: *const [i32; 4], erase: i32) -> i32;
    pub fn BeginPaint(hwnd: Hwnd, ps: *mut [u8; 72]) -> usize;
    pub fn EndPaint(hwnd: Hwnd, ps: *const [u8; 72]) -> i32;
    pub fn FillRect(hdc: usize, rect: *const [i32; 4], brush: usize) -> i32;
    pub fn DrawTextW(hdc: usize, text: *const u16, n: i32, rect: *mut [i32; 4], format: u32)
    -> i32;
    pub fn GetWindowLongW(hwnd: Hwnd, index: i32) -> i32;
    pub fn GetWindowLongPtrW(hwnd: Hwnd, index: i32) -> isize;
    pub fn SetWindowLongW(hwnd: Hwnd, index: i32, value: i32) -> i32;
    pub fn SetCapture(hwnd: Hwnd) -> Hwnd;
    pub fn ReleaseCapture() -> i32;
    pub fn GetCapture() -> Hwnd;
    pub fn GetDC(hwnd: Hwnd) -> usize;
    pub fn ReleaseDC(hwnd: Hwnd, hdc: usize) -> i32;
    pub fn CreateWindowExW(
        ex: u32,
        class: *const u16,
        name: *const u16,
        style: u32,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        parent: Hwnd,
        menu: usize,
        instance: usize,
        param: *mut c_void,
    ) -> Hwnd;
    pub fn DestroyWindow(hwnd: Hwnd) -> i32;
}

/// WNDCLASSEXW.
#[repr(C)]
pub struct WndClassExW {
    pub size: u32,
    pub style: u32,
    pub proc_: WndProc,
    pub class_extra: i32,
    pub window_extra: i32,
    pub instance: usize,
    pub icon: usize,
    pub cursor: usize,
    pub background: usize,
    pub menu_name: *const u16,
    pub class_name: *const u16,
    pub icon_small: usize,
}

pub const WM_DESTROY: u32 = 0x0002;
pub const WM_SIZE: u32 = 0x0005;
pub const WM_PAINT: u32 = 0x000f;
pub const WM_ERASEBKGND: u32 = 0x0014;
pub const WM_SETFONT: u32 = 0x0030;
pub const WM_GETFONT: u32 = 0x0031;
pub const WM_NOTIFY: u32 = 0x004e;
pub const WM_COMMAND: u32 = 0x0111;
pub const WM_MOUSEMOVE: u32 = 0x0200;
pub const WM_LBUTTONDOWN: u32 = 0x0201;
pub const WM_LBUTTONUP: u32 = 0x0202;
pub const WM_LBUTTONDBLCLK: u32 = 0x0203;
pub const WM_USER: u32 = 0x0400;
pub const GWL_STYLE: i32 = -16;
pub const GWLP_ID: i32 = -12;
pub const SWP_NOZORDER: u32 = 0x4;
pub const SWP_NOACTIVATE: u32 = 0x10;

/// The controls' common styles (CCS_*).
pub const CCS_TOP: u32 = 0x1;
pub const CCS_NORESIZE: u32 = 0x4;
pub const CCS_NOPARENTALIGN: u32 = 0x8;

/// Colours (COLORREF: 0x00BBGGRR).
pub const fn rgb(r: u32, g: u32, b: u32) -> u32 {
    r | g << 8 | b << 16
}
pub const TEXT: u32 = rgb(0x1b, 0x1b, 0x1b);
pub const TEXT_DIM: u32 = rgb(0x60, 0x60, 0x60);
pub const TEXT_DISABLED: u32 = rgb(0xa0, 0xa0, 0xa0);
pub const BAR: u32 = rgb(0xf3, 0xf3, 0xf3);
pub const LINE: u32 = rgb(0xe5, 0xe5, 0xe5);
pub const HOT: u32 = rgb(0xe9, 0xe9, 0xe9);
pub const PRESSED: u32 = rgb(0xe0, 0xe0, 0xe0);
pub const CHECKED: u32 = rgb(0xdc, 0xe6, 0xf2);
pub const ACCENT: u32 = rgb(0x00, 0x5f, 0xb8);
pub const WHITE: u32 = rgb(0xff, 0xff, 0xff);

pub fn style(hwnd: Hwnd) -> u32 {
    // SAFETY: plain call.
    unsafe { GetWindowLongW(hwnd, GWL_STYLE) as u32 }
}

pub fn client(hwnd: Hwnd) -> [i32; 4] {
    let mut r = [0i32; 4];
    // SAFETY: our RECT.
    unsafe { GetClientRect(hwnd, &mut r) };
    r
}

pub fn redraw(hwnd: Hwnd) {
    // SAFETY: plain call.
    unsafe { InvalidateRect(hwnd, core::ptr::null(), 1) };
}

/// The control's id (GWLP_ID).
pub fn id_of(hwnd: Hwnd) -> usize {
    // SAFETY: plain call.
    unsafe { GetWindowLongPtrW(hwnd, GWLP_ID) as usize }
}

/// NMHDR.
#[repr(C)]
pub struct NmHdr {
    pub from: Hwnd,
    pub id: usize,
    pub code: u32,
}

/// Sends `parent` WM_NOTIFY with `header` (whose first member is an
/// NMHDR); its answer.
pub fn notify<T>(parent: Hwnd, header: &mut T) -> isize {
    // SAFETY: the NMHDR is at the start of `header`.
    let id = unsafe { (*(header as *mut T).cast::<NmHdr>()).id };
    // SAFETY: plain call; the structure lives across it.
    unsafe { SendMessageW(parent, WM_NOTIFY, id, header as *mut T as isize) }
}

/// Registers a class of comctl32's (once, by name).
pub fn register(name: &[u8], proc_: WndProc) {
    let mut wide = [0u16; 32];
    for (w, &b) in wide.iter_mut().zip(name) {
        *w = u16::from(b);
    }
    let wc = WndClassExW {
        size: core::mem::size_of::<WndClassExW>() as u32,
        style: 0x8 | 0x4000, // CS_DBLCLKS | CS_GLOBALCLASS
        proc_,
        class_extra: 0,
        window_extra: 0,
        instance: 0,
        icon: 0,
        cursor: 0,
        background: 0,
        menu_name: core::ptr::null(),
        class_name: wide.as_ptr(),
        icon_small: 0,
    };
    // SAFETY: our WNDCLASSEXW.
    unsafe { RegisterClassExW(&wc) };
}

// ---- State ----

/// Each control's state of one kind, by window.
pub struct Store<T> {
    lock: AtomicBool,
    items: UnsafeCell<Vec<(Hwnd, T)>>,
}

// SAFETY: `items` is only touched under `lock`.
unsafe impl<T> Sync for Store<T> {}

impl<T> Store<T> {
    pub const fn new() -> Store<T> {
        Store { lock: AtomicBool::new(false), items: UnsafeCell::new(Vec::new()) }
    }

    fn locked<R>(&self, f: impl FnOnce(&mut Vec<(Hwnd, T)>) -> R) -> R {
        while self.lock.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err()
        {
            core::hint::spin_loop();
        }
        // SAFETY: under the lock.
        let r = f(unsafe { &mut *self.items.get() });
        self.lock.store(false, Ordering::Release);
        r
    }

    /// Runs `f` on the window's state (made with `new` the first time).
    /// The lock is held: `f` must not send messages.
    pub fn with<R>(
        &self,
        hwnd: Hwnd,
        new: impl FnOnce() -> T,
        f: impl FnOnce(&mut T) -> R,
    ) -> Option<R> {
        self.locked(|items| {
            let i = match items.iter().position(|e| e.0 == hwnd) {
                Some(i) => i,
                None => {
                    items.try_reserve(1).ok()?;
                    items.push((hwnd, new()));
                    items.len() - 1
                }
            };
            Some(f(&mut items[i].1))
        })
    }

    pub fn remove(&self, hwnd: Hwnd) -> Option<T> {
        self.locked(|items| items.iter().position(|e| e.0 == hwnd).map(|i| items.swap_remove(i).1))
    }
}

// ---- Drawing ----

fn gdi(cache: &AtomicUsize, name: &[u8]) -> usize {
    crate::imagelist::resolve(cache, b"gdi32.dll\0", name)
}

macro_rules! gdi_fn {
    ($name:literal, $ty:ty) => {{
        static CACHE: AtomicUsize = AtomicUsize::new(0);
        let f = gdi(&CACHE, concat!($name, "\0").as_bytes());
        // SAFETY: gdi32's function of that name has this signature.
        (f != 0).then(|| unsafe { core::mem::transmute::<usize, $ty>(f) })
    }};
}

pub fn select(hdc: usize, object: usize) -> usize {
    gdi_fn!("SelectObject", extern "system" fn(usize, usize) -> usize).map_or(0, |f| f(hdc, object))
}

pub fn text_color(hdc: usize, color: u32) {
    if let Some(f) = gdi_fn!("SetTextColor", extern "system" fn(usize, u32) -> u32) {
        f(hdc, color);
    }
}

pub fn transparent(hdc: usize) {
    if let Some(f) = gdi_fn!("SetBkMode", extern "system" fn(usize, i32) -> i32) {
        f(hdc, 1);
    }
}

pub fn default_font() -> usize {
    // DEFAULT_GUI_FONT.
    gdi_fn!("GetStockObject", extern "system" fn(i32) -> usize).map_or(0, |f| f(17))
}

pub fn text_height(hdc: usize) -> i32 {
    let mut tm = [0i32; 15];
    if let Some(f) = gdi_fn!("GetTextMetricsW", extern "system" fn(usize, *mut [i32; 15]) -> i32) {
        f(hdc, &mut tm);
    }
    tm[0].max(1)
}

pub fn text_width(hdc: usize, s: &[u16]) -> i32 {
    if s.is_empty() {
        return 0;
    }
    let mut size = [0i32; 2];
    type Extent = extern "system" fn(usize, *const u16, i32, *mut [i32; 2]) -> i32;
    if let Some(f) = gdi_fn!("GetTextExtentPoint32W", Extent) {
        f(hdc, s.as_ptr(), s.len() as i32, &mut size);
    }
    size[0]
}

/// The height of a line in the default font (a screen DC's measure).
pub fn line_height(font: usize) -> i32 {
    // SAFETY: plain calls on the screen DC.
    let hdc = unsafe { GetDC(0) };
    let old = select(hdc, if font != 0 { font } else { default_font() });
    let h = text_height(hdc);
    select(hdc, old);
    // SAFETY: the DC from GetDC.
    unsafe { ReleaseDC(0, hdc) };
    h
}

pub fn fill(hdc: usize, l: i32, t: i32, r: i32, b: i32, color: u32) {
    if r <= l || b <= t {
        return;
    }
    let (Some(brush), Some(delete)) = (
        gdi_fn!("CreateSolidBrush", extern "system" fn(u32) -> usize),
        gdi_fn!("DeleteObject", extern "system" fn(usize) -> i32),
    ) else {
        return;
    };
    let b0 = brush(color);
    // SAFETY: our RECT.
    unsafe { FillRect(hdc, &[l, t, r, b], b0) };
    delete(b0);
}

fn isqrt(v: i32) -> i32 {
    let mut x = 0;
    while (x + 1) * (x + 1) <= v {
        x += 1;
    }
    x
}

/// A rectangle with corners of `radius`, in `color`.
pub fn round_fill(hdc: usize, r: [i32; 4], radius: i32, color: u32) {
    let [l, t, rr, b] = r;
    let radius = radius.min((rr - l) / 2).min((b - t) / 2).max(0);
    for y in t..b {
        let k = if y < t + radius {
            t + radius - y
        } else if y >= b - radius {
            y - (b - radius) + 1
        } else {
            0
        };
        let inset = if k == 0 { 0 } else { radius - isqrt(radius * radius - (k - 1) * (k - 1)) };
        fill(hdc, l + inset, y, rr - inset, y + 1, color);
    }
}

pub fn draw_text(hdc: usize, text: &[u16], r: [i32; 4], format: u32) {
    let mut r = r;
    // SAFETY: our text and RECT.
    unsafe { DrawTextW(hdc, text.as_ptr(), text.len() as i32, &mut r, format) };
}

/// Paints the window: BeginPaint, `font` (or the default) selected,
/// `f(hdc, client)`, EndPaint.
pub fn paint(hwnd: Hwnd, font: usize, f: impl FnOnce(usize, [i32; 4])) {
    let mut ps = [0u8; 72];
    // SAFETY: a PAINTSTRUCT-sized buffer.
    let hdc = unsafe { BeginPaint(hwnd, &mut ps) };
    if hdc == 0 {
        return;
    }
    let old = select(hdc, if font != 0 { font } else { default_font() });
    f(hdc, client(hwnd));
    if old != 0 {
        select(hdc, old);
    }
    // SAFETY: the PAINTSTRUCT BeginPaint filled.
    unsafe { EndPaint(hwnd, &ps) };
}

/// A NUL-terminated wide string's characters (up to `max`).
pub fn wide(p: *const u16, max: usize) -> &'static [u16] {
    if p.is_null() || (p as usize) < 0x10000 {
        return &[];
    }
    let mut n = 0;
    // SAFETY: the caller's NUL-terminated string.
    while n < max && unsafe { *p.add(n) } != 0 {
        n += 1;
    }
    // SAFETY: `n` characters were read.
    unsafe { core::slice::from_raw_parts(p, n) }
}

/// A copy of `s` (empty if out of memory).
pub fn owned(s: &[u16]) -> Vec<u16> {
    let mut v = Vec::new();
    if v.try_reserve(s.len()).is_ok() {
        v.extend_from_slice(s);
    }
    v
}

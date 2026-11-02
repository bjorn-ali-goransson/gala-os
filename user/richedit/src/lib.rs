//! The rich edit control that gala's riched20.dll ("RichEdit20W",
//! "RichEdit20A") and msftedit.dll ("RICHEDIT50W") register: a text
//! control with runs of character formats, written from Microsoft's
//! documentation of rich edit controls and their EM_ messages (clean
//! room). `doc.rs` is the text model (paragraphs ending in CR, runs of
//! formats, CHARFORMAT(2) in and out), `view.rs` lays the text out in
//! lines (word wrap with gdi32's measures), scrolls and paints it,
//! `control.rs` is the window procedure (messages, notifications, keys
//! and mouse). Each control's state is kept per window in a `Store`.

#![no_std]
#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

extern crate alloc;

mod control;
mod doc;
mod view;

use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

// The global allocator (the text, runs and lines).
use gala_heap as _;

pub(crate) type Hwnd = usize;
pub(crate) type WndProc = extern "system" fn(Hwnd, u32, usize, isize) -> isize;

#[link(name = "user32", kind = "raw-dylib")]
unsafe extern "system" {
    pub(crate) fn RegisterClassExW(wc: *const WndClassExW) -> u16;
    pub(crate) fn DefWindowProcW(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize;
    pub(crate) fn SendMessageW(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize;
    pub(crate) fn GetParent(hwnd: Hwnd) -> Hwnd;
    pub(crate) fn GetClientRect(hwnd: Hwnd, rect: *mut [i32; 4]) -> i32;
    pub(crate) fn InvalidateRect(hwnd: Hwnd, rect: *const [i32; 4], erase: i32) -> i32;
    pub(crate) fn BeginPaint(hwnd: Hwnd, ps: *mut [u8; 72]) -> usize;
    pub(crate) fn EndPaint(hwnd: Hwnd, ps: *const [u8; 72]) -> i32;
    pub(crate) fn FillRect(hdc: usize, rect: *const [i32; 4], brush: usize) -> i32;
    pub(crate) fn GetWindowLongW(hwnd: Hwnd, index: i32) -> i32;
    pub(crate) fn SetWindowLongW(hwnd: Hwnd, index: i32, value: i32) -> i32;
    pub(crate) fn GetWindowLongPtrW(hwnd: Hwnd, index: i32) -> isize;
    pub(crate) fn SetCapture(hwnd: Hwnd) -> Hwnd;
    pub(crate) fn ReleaseCapture() -> i32;
    pub(crate) fn GetCapture() -> Hwnd;
    pub(crate) fn GetDC(hwnd: Hwnd) -> usize;
    pub(crate) fn ReleaseDC(hwnd: Hwnd, hdc: usize) -> i32;
    pub(crate) fn SetFocus(hwnd: Hwnd) -> Hwnd;
    pub(crate) fn GetFocus() -> Hwnd;
    pub(crate) fn IsWindowVisible(hwnd: Hwnd) -> i32;
    pub(crate) fn GetSysColor(index: i32) -> u32;
    pub(crate) fn GetKeyState(vk: i32) -> i16;
    pub(crate) fn SetScrollInfo(hwnd: Hwnd, bar: i32, info: *const ScrollInfo, redraw: i32) -> i32;
    pub(crate) fn LoadCursorW(instance: usize, name: usize) -> usize;
    pub(crate) fn SetCursor(cursor: usize) -> usize;
    pub(crate) fn GetCursorPos(point: *mut [i32; 2]) -> i32;
    pub(crate) fn ScreenToClient(hwnd: Hwnd, point: *mut [i32; 2]) -> i32;
}

#[link(name = "gdi32", kind = "raw-dylib")]
unsafe extern "system" {
    pub(crate) fn CreateFontIndirectW(lf: *const LogFont) -> usize;
    pub(crate) fn DeleteObject(object: usize) -> i32;
    pub(crate) fn SelectObject(hdc: usize, object: usize) -> usize;
    pub(crate) fn GetObjectW(object: usize, size: i32, out: *mut u8) -> i32;
    pub(crate) fn GetStockObject(index: i32) -> usize;
    pub(crate) fn GetTextMetricsW(hdc: usize, tm: *mut [i32; 15]) -> i32;
    pub(crate) fn GetTextExtentExPointW(
        hdc: usize,
        text: *const u16,
        n: i32,
        max: i32,
        fit: *mut i32,
        dx: *mut i32,
        size: *mut [i32; 2],
    ) -> i32;
    pub(crate) fn TextOutW(hdc: usize, x: i32, y: i32, text: *const u16, n: i32) -> i32;
    pub(crate) fn SetTextColor(hdc: usize, color: u32) -> u32;
    pub(crate) fn SetBkMode(hdc: usize, mode: i32) -> i32;
    pub(crate) fn CreateSolidBrush(color: u32) -> usize;
    pub(crate) fn IntersectClipRect(hdc: usize, l: i32, t: i32, r: i32, b: i32) -> i32;
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetCurrentThreadId() -> u32;
    pub(crate) fn MultiByteToWideChar(
        cp: u32,
        flags: u32,
        s: *const u8,
        n: i32,
        out: *mut u16,
        room: i32,
    ) -> i32;
    pub(crate) fn WideCharToMultiByte(
        cp: u32,
        flags: u32,
        s: *const u16,
        n: i32,
        out: *mut u8,
        room: i32,
        default: *const u8,
        used: *mut i32,
    ) -> i32;
}

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn GalaRecordMissing(name: *const u8, length: usize, what: u32, detail: u64) -> u32;
}

#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memmove(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
    fn strlen(s: *const u8) -> usize;
    // MSVC-target unwind tables reference it (gala's ntdll provides it).
    fn __CxxFrameHandler3();
    // The stack probe for frames over a page (ntdll exports it).
    fn __chkstk();
}

/// MSVC-target codegen references this marker symbol directly, so it cannot
/// come from an import. (It becomes a harmless extra data export.)
#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}

/// Records an unsupported case in the ledger.
pub(crate) fn unsupported(what: &[u8], detail: u64) {
    // SAFETY: a valid buffer; "unsupported" (2).
    unsafe { GalaRecordMissing(what.as_ptr(), what.len(), 2, detail) };
}

/// WNDCLASSEXW.
#[repr(C)]
pub(crate) struct WndClassExW {
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

/// SCROLLINFO.
#[repr(C)]
pub(crate) struct ScrollInfo {
    pub size: u32,
    pub mask: u32,
    pub min: i32,
    pub max: i32,
    pub page: u32,
    pub pos: i32,
    pub track: i32,
}

/// LOGFONTW.
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct LogFont {
    pub height: i32,
    pub width: i32,
    pub escapement: i32,
    pub orientation: i32,
    pub weight: i32,
    pub italic: u8,
    pub underline: u8,
    pub strike_out: u8,
    pub char_set: u8,
    pub out_precision: u8,
    pub clip_precision: u8,
    pub quality: u8,
    pub pitch_and_family: u8,
    pub face: [u16; 32],
}

/// Registers the classes `names` (wide or ANSI window procedures), global
/// to the process (CS_GLOBALCLASS), as the DLL loads.
pub fn register(instance: usize, names: &[(&str, bool)]) {
    for &(name, ansi) in names {
        let mut wide = [0u16; 32];
        for (w, b) in wide.iter_mut().zip(name.bytes()) {
            *w = u16::from(b);
        }
        let wc = WndClassExW {
            size: core::mem::size_of::<WndClassExW>() as u32,
            // CS_DBLCLKS | CS_GLOBALCLASS
            style: 0x8 | 0x4000,
            proc_: if ansi { control::proc_a } else { control::proc_w },
            class_extra: 0,
            window_extra: 0,
            instance,
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
}

/// Each control's state, by window, under a spin lock that is never held
/// while a message is sent. A thread asking for it again while it holds
/// it (a message sent from inside, against that rule) gets nothing rather
/// than waiting for itself.
pub(crate) struct Store<T> {
    lock: AtomicBool,
    owner: AtomicU32,
    items: UnsafeCell<Vec<(Hwnd, T)>>,
}

// SAFETY: `items` is only touched under `lock`.
unsafe impl<T> Sync for Store<T> {}

impl<T> Store<T> {
    pub const fn new() -> Store<T> {
        Store {
            lock: AtomicBool::new(false),
            owner: AtomicU32::new(0),
            items: UnsafeCell::new(Vec::new()),
        }
    }

    fn locked<R>(&self, f: impl FnOnce(&mut Vec<(Hwnd, T)>) -> Option<R>) -> Option<R> {
        // SAFETY: plain call.
        let me = unsafe { GetCurrentThreadId() };
        if self.owner.load(Ordering::Relaxed) == me {
            return None;
        }
        while self.lock.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err()
        {
            core::hint::spin_loop();
        }
        self.owner.store(me, Ordering::Relaxed);
        // SAFETY: under the lock.
        let r = f(unsafe { &mut *self.items.get() });
        self.owner.store(0, Ordering::Relaxed);
        self.lock.store(false, Ordering::Release);
        r
    }

    /// Runs `f` on the window's state, made with `new` the first time
    /// (None when out of memory). `f` must not send messages.
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
        self.locked(|items| {
            Some(items.iter().position(|e| e.0 == hwnd).map(|i| items.swap_remove(i).1))
        })
        .flatten()
    }
}

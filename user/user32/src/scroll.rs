//! Scroll bars as data: each window's horizontal and vertical bar (and a
//! scroll bar control's own) keeps a range, a page and a position, with the
//! documented results of Set/GetScrollInfo, Set/GetScrollPos and
//! Set/GetScrollRange. The position stays within min..max - (page - 1).
//! Scroll bars are not drawn yet (ShowScrollBar and EnableScrollBar answer
//! TRUE); the window keeps its client area.

use crate::window::{Hwnd, set_last_error, window};
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, Ordering};

const SB_HORZ: i32 = 0;
const SB_CTL: i32 = 2;
const SIF_RANGE: u32 = 0x1;
const SIF_PAGE: u32 = 0x2;
const SIF_POS: u32 = 0x4;
const SIF_TRACKPOS: u32 = 0x10;
const ERROR_INVALID_PARAMETER: u32 = 87;
const ERROR_INVALID_WINDOW_HANDLE: u32 = 1400;

#[derive(Clone, Copy)]
struct Bar {
    hwnd: Hwnd,
    which: i32,
    min: i32,
    max: i32,
    page: u32,
    pos: i32,
    track: i32,
}

const NO_BAR: Bar = Bar { hwnd: 0, which: 0, min: 0, max: 0, page: 0, pos: 0, track: 0 };
const MAX_BARS: usize = 256;

struct Table {
    lock: AtomicBool,
    bars: UnsafeCell<[Bar; MAX_BARS]>,
}

// SAFETY: every access is under `lock`.
unsafe impl Sync for Table {}

static TABLE: Table =
    Table { lock: AtomicBool::new(false), bars: UnsafeCell::new([NO_BAR; MAX_BARS]) };

fn with<R>(f: impl FnOnce(&mut [Bar; MAX_BARS]) -> R) -> R {
    while TABLE.lock.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        core::hint::spin_loop();
    }
    // SAFETY: under the lock.
    let r = f(unsafe { &mut *TABLE.bars.get() });
    TABLE.lock.store(false, Ordering::Release);
    r
}

/// The bar's state, made on first use (a window's bars start at 0..100,
/// as Windows' standard scroll bars do; a control's at 0..0).
fn get(hwnd: Hwnd, which: i32) -> Option<Bar> {
    if window(hwnd).is_none() {
        set_last_error(ERROR_INVALID_WINDOW_HANDLE);
        return None;
    }
    if !(SB_HORZ..=SB_CTL).contains(&which) {
        set_last_error(ERROR_INVALID_PARAMETER);
        return None;
    }
    let found = with(|t| t.iter().find(|b| b.hwnd == hwnd && b.which == which).copied());
    Some(found.unwrap_or(Bar { hwnd, which, max: if which == SB_CTL { 0 } else { 100 }, ..NO_BAR }))
}

fn put(bar: Bar) {
    with(|t| {
        let at = t.iter().position(|b| b.hwnd == bar.hwnd && b.which == bar.which);
        let at = at.or_else(|| t.iter().position(|b| b.hwnd == 0 || window(b.hwnd).is_none()));
        if let Some(i) = at {
            t[i] = bar;
        }
    });
}

/// The highest position: max - (page - 1), not below min.
fn clamp(b: &mut Bar) {
    let top = if b.page > 0 { b.max - (b.page as i32 - 1) } else { b.max };
    b.pos = b.pos.clamp(b.min, top.max(b.min));
}

/// SCROLLINFO: cbSize, fMask, nMin, nMax, nPage, nPos, nTrackPos.
#[repr(C)]
pub struct ScrollInfo {
    size: u32,
    mask: u32,
    min: i32,
    max: i32,
    page: u32,
    pos: i32,
    track: i32,
}

/// SetScrollInfo(hwnd, bar, info, redraw): the new position.
#[unsafe(no_mangle)]
pub extern "system" fn SetScrollInfo(
    hwnd: Hwnd,
    which: i32,
    info: *const ScrollInfo,
    _redraw: i32,
) -> i32 {
    if info.is_null() {
        set_last_error(ERROR_INVALID_PARAMETER);
        return 0;
    }
    let Some(mut b) = get(hwnd, which) else { return 0 };
    // SAFETY: the caller's SCROLLINFO.
    let si = unsafe { &*info };
    if si.mask & SIF_RANGE != 0 {
        b.min = si.min;
        b.max = si.max.max(si.min);
    }
    if si.mask & SIF_PAGE != 0 {
        let span = (i64::from(b.max) - i64::from(b.min) + 1).max(0) as u32;
        b.page = si.page.min(span);
    }
    if si.mask & SIF_POS != 0 {
        b.pos = si.pos;
    }
    clamp(&mut b);
    put(b);
    b.pos
}

/// GetScrollInfo(hwnd, bar, info): the parts fMask asks for.
#[unsafe(no_mangle)]
pub extern "system" fn GetScrollInfo(hwnd: Hwnd, which: i32, info: *mut ScrollInfo) -> i32 {
    if info.is_null() {
        set_last_error(ERROR_INVALID_PARAMETER);
        return 0;
    }
    let Some(b) = get(hwnd, which) else { return 0 };
    // SAFETY: the caller's SCROLLINFO.
    let si = unsafe { &mut *info };
    if si.mask & SIF_RANGE != 0 {
        si.min = b.min;
        si.max = b.max;
    }
    if si.mask & SIF_PAGE != 0 {
        si.page = b.page;
    }
    if si.mask & SIF_POS != 0 {
        si.pos = b.pos;
    }
    if si.mask & SIF_TRACKPOS != 0 {
        si.track = b.track;
    }
    1
}

/// SetScrollPos(hwnd, bar, pos, redraw): the previous position.
#[unsafe(no_mangle)]
pub extern "system" fn SetScrollPos(hwnd: Hwnd, which: i32, pos: i32, _redraw: i32) -> i32 {
    let Some(mut b) = get(hwnd, which) else { return 0 };
    let old = b.pos;
    b.pos = pos;
    clamp(&mut b);
    put(b);
    old
}

#[unsafe(no_mangle)]
pub extern "system" fn GetScrollPos(hwnd: Hwnd, which: i32) -> i32 {
    get(hwnd, which).map_or(0, |b| b.pos)
}

/// SetScrollRange(hwnd, bar, min, max, redraw).
#[unsafe(no_mangle)]
pub extern "system" fn SetScrollRange(
    hwnd: Hwnd,
    which: i32,
    min: i32,
    max: i32,
    _redraw: i32,
) -> i32 {
    let Some(mut b) = get(hwnd, which) else { return 0 };
    b.min = min;
    b.max = max.max(min);
    clamp(&mut b);
    put(b);
    1
}

#[unsafe(no_mangle)]
pub extern "system" fn GetScrollRange(hwnd: Hwnd, which: i32, min: *mut i32, max: *mut i32) -> i32 {
    let Some(b) = get(hwnd, which) else { return 0 };
    if min.is_null() || max.is_null() {
        set_last_error(ERROR_INVALID_PARAMETER);
        return 0;
    }
    // SAFETY: the caller's two ints.
    unsafe {
        *min = b.min;
        *max = b.max;
    }
    1
}

/// ShowScrollBar(hwnd, bar, show): TRUE (bars are not drawn yet).
#[unsafe(no_mangle)]
pub extern "system" fn ShowScrollBar(hwnd: Hwnd, _which: i32, _show: i32) -> i32 {
    crate::window::IsWindow(hwnd)
}

/// EnableScrollBar(hwnd, bar, arrows): TRUE (bars are not drawn yet).
#[unsafe(no_mangle)]
pub extern "system" fn EnableScrollBar(hwnd: Hwnd, _which: u32, _arrows: u32) -> i32 {
    crate::window::IsWindow(hwnd)
}

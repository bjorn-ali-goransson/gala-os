//! The status bar (msctls_statusbar32): at the bottom of its parent, full
//! width (CCS_TOP: at the top; CCS_NORESIZE/CCS_NOPARENTALIGN: where it
//! was put), placed again on WM_SIZE; parts (right edges, -1: to the end)
//! with a text each; a simple mode with one text. Drawn light grey with a
//! line above and between parts.

use crate::common::*;
use alloc::vec::Vec;

const SB_SETTEXTW: u32 = WM_USER + 11;
const SB_GETTEXTLENGTHW: u32 = WM_USER + 12;
const SB_GETTEXTW: u32 = WM_USER + 13;
const SB_SETPARTS: u32 = WM_USER + 4;
const SB_GETPARTS: u32 = WM_USER + 6;
const SB_GETBORDERS: u32 = WM_USER + 7;
const SB_SETMINHEIGHT: u32 = WM_USER + 8;
const SB_SIMPLE: u32 = WM_USER + 9;
const SB_GETRECT: u32 = WM_USER + 10;
const SB_ISSIMPLE: u32 = WM_USER + 14;
const SB_SETICON: u32 = WM_USER + 15;
const SB_SETTIPTEXTW: u32 = WM_USER + 17;
const SB_GETICON: u32 = WM_USER + 20;
const SB_SETBKCOLOR: u32 = 0x2001; // CCM_SETBKCOLOR
/// The part the simple mode shows (SB_SETTEXTW's SB_SIMPLEID).
const SIMPLE_ID: usize = 255;
/// The gap between parts.
const GAP: i32 = 2;

struct Bar {
    /// Right edges (-1: the bar's).
    parts: Vec<i32>,
    /// Each part's text and its drawing type (SBT_*).
    texts: Vec<(Vec<u16>, u32)>,
    simple: Option<Vec<u16>>,
    min_height: i32,
    font: usize,
}

fn new_bar() -> Bar {
    let mut texts = Vec::new();
    if texts.try_reserve(1).is_ok() {
        texts.push((Vec::new(), 0));
    }
    let mut parts = Vec::new();
    if parts.try_reserve(1).is_ok() {
        parts.push(-1);
    }
    Bar { parts, texts, simple: None, min_height: 0, font: 0 }
}

static BARS: Store<Bar> = Store::new();

fn with<R>(hwnd: Hwnd, f: impl FnOnce(&mut Bar) -> R) -> Option<R> {
    BARS.with(hwnd, new_bar, f)
}

/// The bar's height: a line of its font with room around it.
fn height(hwnd: Hwnd) -> i32 {
    let (font, min) = with(hwnd, |b| (b.font, b.min_height)).unwrap_or((0, 0));
    (line_height(font) + 8).max(min)
}

/// Puts the bar at its parent's bottom (or top), full width.
fn place(hwnd: Hwnd) {
    let s = style(hwnd);
    if s & (CCS_NORESIZE | CCS_NOPARENTALIGN) != 0 {
        return;
    }
    // SAFETY: plain call.
    let parent = unsafe { GetParent(hwnd) };
    if parent == 0 {
        return;
    }
    let pc = client(parent);
    let h = height(hwnd);
    let y = if s & CCS_TOP != 0 { 0 } else { pc[3] - h };
    // SAFETY: plain call.
    unsafe { SetWindowPos(hwnd, 0, 0, y, pc[2], h, SWP_NOZORDER | SWP_NOACTIVATE) };
}

/// Part `i`'s rectangle in the bar.
fn part_rect(hwnd: Hwnd, i: usize) -> Option<[i32; 4]> {
    let c = client(hwnd);
    let edges = with(hwnd, |b| b.parts.clone())?;
    let edge = |k: usize| match edges.get(k) {
        Some(&e) if e >= 0 => e.min(c[2]),
        Some(_) => c[2],
        None => c[2],
    };
    if i >= edges.len() {
        return None;
    }
    let left = if i == 0 { 0 } else { edge(i - 1) + GAP };
    Some([left, c[1] + 2, edge(i), c[3]])
}

fn set_text(hwnd: Hwnd, wparam: usize, text: &[u16]) -> isize {
    let part = wparam & 0xff;
    let kind = (wparam & 0xff00) as u32;
    let t = owned(text);
    let ok = with(hwnd, |b| {
        if part == SIMPLE_ID {
            b.simple = Some(t);
            return true;
        }
        match b.texts.get_mut(part) {
            Some(slot) => {
                *slot = (t, kind);
                true
            }
            None => false,
        }
    })
    .unwrap_or(false);
    redraw(hwnd);
    isize::from(ok)
}

pub extern "system" fn proc_(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize {
    match msg {
        0x0001 => {
            // WM_CREATE
            with(hwnd, |_| ());
            place(hwnd);
            0
        }
        WM_SIZE => {
            place(hwnd);
            redraw(hwnd);
            0
        }
        WM_DESTROY => {
            BARS.remove(hwnd);
            // SAFETY: plain call.
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_SETFONT => {
            with(hwnd, |b| b.font = wparam);
            place(hwnd);
            redraw(hwnd);
            0
        }
        WM_GETFONT => with(hwnd, |b| b.font as isize).unwrap_or(0),
        WM_ERASEBKGND => 1,
        SB_SETPARTS => {
            let n = wparam.min(256);
            if n == 0 || lparam == 0 {
                return 0;
            }
            // SAFETY: the caller's `n` ints.
            let edges = unsafe { core::slice::from_raw_parts(lparam as *const i32, n) };
            let ok = with(hwnd, |b| {
                if b.parts.try_reserve(n).is_err() || b.texts.try_reserve(n).is_err() {
                    return false;
                }
                b.parts.clear();
                b.parts.extend_from_slice(edges);
                b.texts.truncate(n);
                while b.texts.len() < n {
                    b.texts.push((Vec::new(), 0));
                }
                true
            })
            .unwrap_or(false);
            redraw(hwnd);
            isize::from(ok)
        }
        SB_GETPARTS => {
            let edges = with(hwnd, |b| b.parts.clone()).unwrap_or_default();
            if lparam != 0 {
                for (i, &e) in edges.iter().take(wparam).enumerate() {
                    // SAFETY: the caller's array of `wparam` ints.
                    unsafe { *(lparam as *mut i32).add(i) = e };
                }
            }
            edges.len() as isize
        }
        SB_SETTEXTW => set_text(hwnd, wparam, wide(lparam as *const u16, 4096)),
        0x0401 => {
            // SB_SETTEXTA: the ANSI code page is UTF-8 here (ASCII kept).
            let mut buf = [0u16; 512];
            let mut n = 0;
            if lparam != 0 {
                // SAFETY: the caller's NUL-terminated string.
                while n < buf.len() && unsafe { *(lparam as *const u8).add(n) } != 0 {
                    buf[n] = u16::from(unsafe { *(lparam as *const u8).add(n) });
                    n += 1;
                }
            }
            set_text(hwnd, wparam, &buf[..n])
        }
        SB_GETTEXTW | SB_GETTEXTLENGTHW => {
            let part = wparam & 0xff;
            let (text, kind) =
                with(hwnd, |b| b.texts.get(part).map(|(t, k)| (t.clone(), *k)).unwrap_or_default())
                    .unwrap_or_default();
            if msg == SB_GETTEXTW && lparam != 0 {
                // SAFETY: the caller's buffer (large enough, as the
                // message requires).
                unsafe {
                    core::ptr::copy_nonoverlapping(text.as_ptr(), lparam as *mut u16, text.len());
                    *(lparam as *mut u16).add(text.len()) = 0;
                }
            }
            (text.len() as isize & 0xffff) | (kind as isize) << 16
        }
        SB_GETRECT => match part_rect(hwnd, wparam) {
            Some(r) if lparam != 0 => {
                // SAFETY: the caller's RECT.
                unsafe { *(lparam as *mut [i32; 4]) = r };
                1
            }
            _ => 0,
        },
        SB_GETBORDERS => {
            if lparam != 0 {
                // SAFETY: the caller's three ints.
                unsafe { *(lparam as *mut [i32; 3]) = [0, 2, GAP] };
            }
            1
        }
        SB_SETMINHEIGHT => {
            with(hwnd, |b| b.min_height = wparam as i32);
            0
        }
        SB_SIMPLE => {
            with(hwnd, |b| {
                if wparam == 0 {
                    b.simple = None;
                } else if b.simple.is_none() {
                    b.simple = Some(Vec::new());
                }
            });
            redraw(hwnd);
            0
        }
        SB_ISSIMPLE => with(hwnd, |b| isize::from(b.simple.is_some())).unwrap_or(0),
        SB_SETICON | SB_SETTIPTEXTW => 1,
        SB_GETICON => 0,
        SB_SETBKCOLOR => -1, // CLR_DEFAULT: the colour before
        WM_PAINT => {
            let font = with(hwnd, |b| b.font).unwrap_or(0);
            let (simple, texts, edges) =
                with(hwnd, |b| (b.simple.clone(), b.texts.clone(), b.parts.clone()))
                    .unwrap_or_default();
            paint(hwnd, font, |hdc, r| {
                fill(hdc, r[0], r[1], r[2], r[3], BAR);
                fill(hdc, r[0], r[1], r[2], r[1] + 1, LINE);
                transparent(hdc);
                text_color(hdc, TEXT);
                // DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX | DT_END_ELLIPSIS.
                let format = 0x20 | 0x4 | 0x800 | 0x8000;
                if let Some(t) = simple {
                    draw_text(hdc, &t, [r[0] + 8, r[1] + 1, r[2] - 8, r[3]], format);
                    return;
                }
                for (i, (t, _)) in texts.iter().enumerate() {
                    let Some(pr) = part_rect(hwnd, i) else { break };
                    draw_text(hdc, t, [pr[0] + 8, pr[1], pr[2] - 6, pr[3]], format);
                    if i + 1 < edges.len() {
                        fill(hdc, pr[2], r[1] + 5, pr[2] + 1, r[3] - 4, LINE);
                    }
                }
            });
            0
        }
        // SAFETY: plain call.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

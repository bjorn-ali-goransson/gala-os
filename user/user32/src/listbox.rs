//! LISTBOX (class "ListBox"): a single column of strings, each with a
//! data value, and a selection, as Microsoft documents the list box
//! messages (corner cases as Wine behaves, no code taken: the return
//! values of LB_SETCURSEL(-1), LB_GETITEMRECT for rows scrolled away).
//!
//! - Single selection (the default): LB_SETCURSEL/LB_GETCURSEL; LB_SETSEL
//!   and the multiple-selection messages fail with LB_ERR.
//! - LBS_MULTIPLESEL (a click toggles) and LBS_EXTENDEDSEL (a click
//!   selects one, Ctrl toggles, Shift selects from the anchor): LB_SETSEL,
//!   LB_GETSELCOUNT, LB_GETSELITEMS; LB_SETCURSEL fails.
//! - LBS_SORT: LB_ADDSTRING inserts in case-insensitive order.
//! - LBS_NOTIFY: the parent gets WM_COMMAND LBN_SELCHANGE when the user
//!   changes the selection (a click, the keys; never for the messages),
//!   LBN_DBLCLK, LBN_SETFOCUS, LBN_KILLFOCUS.
//! - Rows are `item_height` pixels from the top of the client area,
//!   starting at the top index (LB_SETTOPINDEX, the wheel, the keys
//!   scroll); LB_GETITEMRECT gives rows above the top negative offsets.
//!
//! Not there: owner-drawn lists (LBS_OWNERDRAW*: drawn as strings),
//! LBS_NODATA, multiple columns, tab stops, LB_DIR.

use crate::controls::{default_font, fill, rgb, round_fill, select as select_object, transparent};
use crate::window::{DefWindowProcW, Hwnd, SendMessageW};
use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, Ordering};

const WM_DESTROY: u32 = 0x0002;
const WM_SETFOCUS: u32 = 0x0007;
const WM_KILLFOCUS: u32 = 0x0008;
const WM_PAINT: u32 = 0x000f;
const WM_ERASEBKGND: u32 = 0x0014;
const WM_SETFONT: u32 = 0x0030;
const WM_GETFONT: u32 = 0x0031;
const WM_GETDLGCODE: u32 = 0x0087;
const WM_KEYDOWN: u32 = 0x0100;
const WM_COMMAND: u32 = 0x0111;
const WM_VSCROLL: u32 = 0x0115;
const WM_LBUTTONDOWN: u32 = 0x0201;
const WM_LBUTTONUP: u32 = 0x0202;
const WM_LBUTTONDBLCLK: u32 = 0x0203;
const WM_MOUSEWHEEL: u32 = 0x020a;

pub(crate) const LB_ADDSTRING: u32 = 0x0180;
pub(crate) const LB_INSERTSTRING: u32 = 0x0181;
const LB_DELETESTRING: u32 = 0x0182;
const LB_RESETCONTENT: u32 = 0x0184;
const LB_SETSEL: u32 = 0x0185;
const LB_SETCURSEL: u32 = 0x0186;
const LB_GETSEL: u32 = 0x0187;
const LB_GETCURSEL: u32 = 0x0188;
pub(crate) const LB_GETTEXT: u32 = 0x0189;
pub(crate) const LB_GETTEXTLEN: u32 = 0x018a;
const LB_GETCOUNT: u32 = 0x018b;
pub(crate) const LB_SELECTSTRING: u32 = 0x018c;
const LB_GETTOPINDEX: u32 = 0x018e;
pub(crate) const LB_FINDSTRING: u32 = 0x018f;
const LB_GETSELCOUNT: u32 = 0x0190;
pub(crate) const LB_GETSELITEMS: u32 = 0x0191;
const LB_SETTABSTOPS: u32 = 0x0192;
const LB_GETHORIZONTALEXTENT: u32 = 0x0193;
const LB_SETHORIZONTALEXTENT: u32 = 0x0194;
const LB_SETCOLUMNWIDTH: u32 = 0x0195;
const LB_SETTOPINDEX: u32 = 0x0197;
pub(crate) const LB_GETITEMRECT: u32 = 0x0198;
const LB_GETITEMDATA: u32 = 0x0199;
const LB_SETITEMDATA: u32 = 0x019a;
const LB_SELITEMRANGE: u32 = 0x019b;
const LB_SETANCHORINDEX: u32 = 0x019c;
const LB_GETANCHORINDEX: u32 = 0x019d;
const LB_SETCARETINDEX: u32 = 0x019e;
const LB_GETCARETINDEX: u32 = 0x019f;
const LB_SETITEMHEIGHT: u32 = 0x01a0;
const LB_GETITEMHEIGHT: u32 = 0x01a1;
pub(crate) const LB_FINDSTRINGEXACT: u32 = 0x01a2;
const LB_SETLOCALE: u32 = 0x01a5;
const LB_GETLOCALE: u32 = 0x01a6;
const LB_SETCOUNT: u32 = 0x01a7;
const LB_INITSTORAGE: u32 = 0x01a8;
const LB_ITEMFROMPOINT: u32 = 0x01a9;
const LB_GETLISTBOXINFO: u32 = 0x01b2;
const LB_ERR: isize = -1;
const LB_ERRSPACE: isize = -2;

const LBS_NOTIFY: u32 = 0x0001;
const LBS_SORT: u32 = 0x0002;
const LBS_MULTIPLESEL: u32 = 0x0008;
const LBS_EXTENDEDSEL: u32 = 0x0800;
const LBS_NOSEL: u32 = 0x4000;

const LBN_SELCHANGE: u32 = 1;
const LBN_DBLCLK: u32 = 2;
const LBN_SETFOCUS: u32 = 4;
const LBN_KILLFOCUS: u32 = 5;

const MK_SHIFT: usize = 0x4;
const MK_CONTROL: usize = 0x8;

const VK_SPACE: usize = 0x20;
const VK_PRIOR: usize = 0x21;
const VK_NEXT: usize = 0x22;
const VK_END: usize = 0x23;
const VK_HOME: usize = 0x24;
const VK_UP: usize = 0x26;
const VK_DOWN: usize = 0x28;

/// The default row height (a 9-point UI font and a little room).
const ITEM_H: i32 = 18;

const BACK: u32 = rgb(0xff, 0xff, 0xff);
const TEXT: u32 = rgb(0x1b, 0x1b, 0x1b);
const SELECTED: u32 = rgb(0x00, 0x78, 0xd4);
const SELECTED_UNFOCUSED: u32 = rgb(0xd9, 0xd9, 0xd9);
const THUMB: u32 = rgb(0x8a, 0x8a, 0x8a);

struct Item {
    text: Vec<u16>,
    data: usize,
    selected: bool,
}

struct ListBox {
    hwnd: Hwnd,
    items: Vec<Item>,
    /// The single selection's item (-1: none).
    current: isize,
    /// The item with the focus rectangle, and the extended selection's
    /// anchor.
    caret: isize,
    anchor: isize,
    top: isize,
    font: usize,
    item_h: i32,
    extent: usize,
    /// A press changed the selection: LBN_SELCHANGE goes on its release.
    pending: bool,
}

struct Table {
    lock: AtomicBool,
    lists: UnsafeCell<Vec<ListBox>>,
}

// SAFETY: `lists` is only touched under `lock`.
unsafe impl Sync for Table {}

static TABLE: Table = Table { lock: AtomicBool::new(false), lists: UnsafeCell::new(Vec::new()) };

/// Runs `f` on the list box's state (made the first time). The lock is
/// held: `f` must not send messages.
fn with<R>(hwnd: Hwnd, f: impl FnOnce(&mut ListBox) -> R) -> Option<R> {
    while TABLE.lock.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        core::hint::spin_loop();
    }
    // SAFETY: under the lock.
    let lists = unsafe { &mut *TABLE.lists.get() };
    let i = match lists.iter().position(|l| l.hwnd == hwnd) {
        Some(i) => Some(i),
        None if lists.try_reserve(1).is_ok() => {
            lists.push(ListBox {
                hwnd,
                items: Vec::new(),
                current: -1,
                caret: 0,
                anchor: 0,
                top: 0,
                font: 0,
                item_h: ITEM_H,
                extent: 0,
                pending: false,
            });
            Some(lists.len() - 1)
        }
        None => None,
    };
    let r = i.map(|i| f(&mut lists[i]));
    TABLE.lock.store(false, Ordering::Release);
    r
}

fn forget(hwnd: Hwnd) {
    while TABLE.lock.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        core::hint::spin_loop();
    }
    // SAFETY: under the lock.
    unsafe { (*TABLE.lists.get()).retain(|l| l.hwnd != hwnd) };
    TABLE.lock.store(false, Ordering::Release);
}

fn style(hwnd: Hwnd) -> u32 {
    crate::tree::GetWindowLongW(hwnd, -16) as u32
}

fn multiple(hwnd: Hwnd) -> bool {
    style(hwnd) & (LBS_MULTIPLESEL | LBS_EXTENDEDSEL) != 0
}

fn notify(hwnd: Hwnd, code: u32) {
    if style(hwnd) & LBS_NOTIFY == 0 {
        return;
    }
    let parent = crate::tree::GetParent(hwnd);
    if parent != 0 {
        let id = crate::tree::GetWindowLongPtrW(hwnd, -12) as usize & 0xffff;
        SendMessageW(parent, WM_COMMAND, id | (code as usize) << 16, hwnd as isize);
    }
}

fn redraw(hwnd: Hwnd) {
    crate::window::InvalidateRect(hwnd, core::ptr::null(), 1);
}

/// The caller's NUL-terminated string.
fn wide(p: isize) -> Vec<u16> {
    let mut v = Vec::new();
    if p == 0 {
        return v;
    }
    let mut n = 0;
    // SAFETY: the caller's NUL-terminated string.
    while n < 4096 && unsafe { *(p as *const u16).add(n) } != 0 {
        n += 1;
    }
    if v.try_reserve(n).is_ok() {
        // SAFETY: `n` characters.
        v.extend_from_slice(unsafe { core::slice::from_raw_parts(p as *const u16, n) });
    }
    v
}

fn lower(c: u16) -> u16 {
    if (0x41..=0x5a).contains(&c) { c + 32 } else { c }
}

/// Case-insensitive order (ASCII folding).
fn before(a: &[u16], b: &[u16]) -> bool {
    a.iter().map(|&c| lower(c)).lt(b.iter().map(|&c| lower(c)))
}

/// How many whole rows the client area shows (at least one).
fn page(hwnd: Hwnd, item_h: i32) -> isize {
    let mut r = [0i32; 4];
    crate::tree::GetClientRect(hwnd, &mut r);
    ((r[3] - r[1]) / item_h.max(1)).max(1) as isize
}

fn insert(hwnd: Hwnd, at: isize, text: Vec<u16>) -> isize {
    let sorted = style(hwnd) & LBS_SORT != 0;
    let r = with(hwnd, |l| {
        let n = l.items.len() as isize;
        let i = if at == -2 {
            // LB_ADDSTRING: in order, or at the end.
            if sorted {
                l.items.iter().position(|it| before(&text, &it.text)).unwrap_or(l.items.len())
                    as isize
            } else {
                n
            }
        } else if at == -1 {
            n
        } else if at > n || at < 0 {
            return LB_ERR;
        } else {
            at
        };
        if l.items.try_reserve(1).is_err() {
            return LB_ERRSPACE;
        }
        l.items.insert(i as usize, Item { text, data: 0, selected: false });
        if l.current >= i {
            l.current += 1;
        }
        i
    })
    .unwrap_or(LB_ERRSPACE);
    redraw(hwnd);
    r
}

fn delete(hwnd: Hwnd, i: isize) -> isize {
    let r = with(hwnd, |l| {
        if i < 0 || i as usize >= l.items.len() {
            return LB_ERR;
        }
        l.items.remove(i as usize);
        if l.current == i {
            l.current = -1;
        } else if l.current > i {
            l.current -= 1;
        }
        let n = l.items.len() as isize;
        if l.caret >= n {
            l.caret = (n - 1).max(0);
        }
        if l.top >= n {
            l.top = (n - 1).max(0);
        }
        n
    })
    .unwrap_or(LB_ERR);
    redraw(hwnd);
    r
}

/// Scrolls so row `i` shows.
fn show(hwnd: Hwnd, i: isize) {
    let item_h = with(hwnd, |l| l.item_h).unwrap_or(ITEM_H);
    let rows = page(hwnd, item_h);
    with(hwnd, |l| {
        if i < l.top {
            l.top = i.max(0);
        } else if i >= l.top + rows {
            l.top = i - rows + 1;
        }
    });
}

/// LB_SETTOPINDEX and scrolling: the top row, kept within the range.
fn set_top(hwnd: Hwnd, top: isize) {
    let item_h = with(hwnd, |l| l.item_h).unwrap_or(ITEM_H);
    let rows = page(hwnd, item_h);
    with(hwnd, |l| {
        let max = (l.items.len() as isize - rows).max(0);
        l.top = top.clamp(0, max);
    });
    redraw(hwnd);
}

/// The single selection becomes item `i` (-1: none); whether it changed.
fn set_current(hwnd: Hwnd, i: isize) -> bool {
    let changed = with(hwnd, |l| {
        let old = l.current;
        l.current = i;
        for (k, it) in l.items.iter_mut().enumerate() {
            it.selected = k as isize == i;
        }
        if i >= 0 {
            l.caret = i;
            l.anchor = i;
        }
        old != i
    })
    .unwrap_or(false);
    if i >= 0 {
        show(hwnd, i);
    }
    redraw(hwnd);
    changed
}

/// Sets item `i`'s selection (-1: every item's) in a multiple-selection
/// list.
fn set_sel(hwnd: Hwnd, on: bool, i: isize) -> isize {
    let r = with(hwnd, |l| {
        if i == -1 {
            for it in l.items.iter_mut() {
                it.selected = on;
            }
            return 0;
        }
        match l.items.get_mut(i as usize).filter(|_| i >= 0) {
            Some(it) => {
                it.selected = on;
                l.caret = i;
                0
            }
            None => LB_ERR,
        }
    })
    .unwrap_or(LB_ERR);
    redraw(hwnd);
    r
}

fn find(hwnd: Hwnd, start: isize, want: &[u16], exact: bool) -> isize {
    with(hwnd, |l| {
        let n = l.items.len();
        if n == 0 {
            return LB_ERR;
        }
        let first = if start < 0 || start as usize >= n { 0 } else { start as usize + 1 };
        for k in 0..n {
            let i = (first + k) % n;
            let t = &l.items[i].text;
            let long_enough = if exact { t.len() == want.len() } else { t.len() >= want.len() };
            if long_enough && t.iter().zip(want).all(|(&a, &b)| lower(a) == lower(b)) {
                return i as isize;
            }
        }
        LB_ERR
    })
    .unwrap_or(LB_ERR)
}

/// LB_GETITEMRECT: row `i`'s rectangle in client coordinates (rows above
/// the top have negative tops); whether it is in the client area.
fn item_rect(hwnd: Hwnd, i: isize) -> Option<([i32; 4], bool)> {
    let (n, top, h) = with(hwnd, |l| (l.items.len() as isize, l.top, l.item_h))?;
    if i < 0 || (i >= n && !(i == 0 && n == 0)) {
        return None;
    }
    let mut c = [0i32; 4];
    crate::tree::GetClientRect(hwnd, &mut c);
    let y = (i - top) as i32 * h;
    let r = [0, y, c[2], y + h];
    Some((r, r[1] < c[3] && r[3] > 0))
}

fn row_at(hwnd: Hwnd, y: i32) -> Option<isize> {
    let (n, top, h) = with(hwnd, |l| (l.items.len() as isize, l.top, l.item_h))?;
    if y < 0 {
        return None;
    }
    let i = top + (y / h.max(1)) as isize;
    (i < n).then_some(i)
}

/// A press at `y` with `keys` (MK_*): the selection as the style says.
fn press(hwnd: Hwnd, y: i32, keys: usize) {
    crate::tree::SetFocus(hwnd);
    let Some(i) = row_at(hwnd, y) else { return };
    let s = style(hwnd);
    if s & LBS_NOSEL != 0 {
        return;
    }
    let changed = if s & LBS_MULTIPLESEL != 0 {
        with(hwnd, |l| {
            if let Some(it) = l.items.get_mut(i as usize) {
                it.selected = !it.selected;
            }
            l.caret = i;
            true
        })
        .unwrap_or(false)
    } else if s & LBS_EXTENDEDSEL != 0 {
        with(hwnd, |l| {
            if keys & MK_SHIFT != 0 {
                let (a, b) = (l.anchor.min(i), l.anchor.max(i));
                for (k, it) in l.items.iter_mut().enumerate() {
                    let k = k as isize;
                    it.selected = (k >= a && k <= b) || (keys & MK_CONTROL != 0 && it.selected);
                }
            } else if keys & MK_CONTROL != 0 {
                if let Some(it) = l.items.get_mut(i as usize) {
                    it.selected = !it.selected;
                }
                l.anchor = i;
            } else {
                for (k, it) in l.items.iter_mut().enumerate() {
                    it.selected = k as isize == i;
                }
                l.anchor = i;
            }
            l.caret = i;
            true
        })
        .unwrap_or(false)
    } else {
        set_current(hwnd, i)
    };
    if changed {
        with(hwnd, |l| l.pending = true);
    }
    show(hwnd, i);
    redraw(hwnd);
}

/// A navigation key: the caret moves (and the single or extended
/// selection with it); LBN_SELCHANGE when it changed.
fn key(hwnd: Hwnd, vk: usize) {
    let (n, caret, cur, h) = with(hwnd, |l| (l.items.len() as isize, l.caret, l.current, l.item_h))
        .unwrap_or((0, 0, -1, 1));
    if n == 0 {
        return;
    }
    let rows = page(hwnd, h);
    let s = style(hwnd);
    let from = if s & (LBS_MULTIPLESEL | LBS_EXTENDEDSEL) != 0 || cur < 0 { caret } else { cur };
    let to = match vk {
        VK_UP => from - 1,
        VK_DOWN => from + 1,
        VK_PRIOR => from - (rows - 1).max(1),
        VK_NEXT => from + (rows - 1).max(1),
        VK_HOME => 0,
        VK_END => n - 1,
        VK_SPACE if s & LBS_MULTIPLESEL != 0 => {
            with(hwnd, |l| {
                if let Some(it) = l.items.get_mut(caret as usize) {
                    it.selected = !it.selected;
                }
            });
            redraw(hwnd);
            notify(hwnd, LBN_SELCHANGE);
            return;
        }
        _ => return,
    }
    .clamp(0, n - 1);
    let changed = if s & LBS_NOSEL != 0 {
        false
    } else if s & LBS_MULTIPLESEL != 0 {
        with(hwnd, |l| l.caret = to);
        false
    } else if s & LBS_EXTENDEDSEL != 0 {
        with(hwnd, |l| {
            for (k, it) in l.items.iter_mut().enumerate() {
                it.selected = k as isize == to;
            }
            l.caret = to;
            l.anchor = to;
        });
        true
    } else {
        set_current(hwnd, to)
    };
    show(hwnd, to);
    redraw(hwnd);
    if changed {
        notify(hwnd, LBN_SELCHANGE);
    }
}

fn paint(hwnd: Hwnd) {
    let mut ps = crate::window::PaintStruct::EMPTY;
    let hdc = crate::window::BeginPaint(hwnd, &mut ps);
    if hdc.is_null() {
        return;
    }
    let focused = crate::tree::GetFocus() == hwnd;
    let Some((font, n, top, h)) = with(hwnd, |l| (l.font, l.items.len() as isize, l.top, l.item_h))
    else {
        crate::window::EndPaint(hwnd, &ps);
        return;
    };
    let mut r = [0i32; 4];
    crate::tree::GetClientRect(hwnd, &mut r);
    let (w, ch) = (r[2], r[3]);
    let old = select_object(hdc, if font != 0 { font } else { default_font() });
    transparent(hdc);
    fill(hdc, 0, 0, w, ch, BACK);
    let rows = (ch / h.max(1) + 1) as isize;
    let scrolls = n > rows - 1 && n as i32 * h > ch;
    let right = if scrolls { w - 8 } else { w };
    let mut text = [0u16; 256];
    for row in 0..rows {
        let k = top + row;
        if k >= n {
            break;
        }
        let (len, selected) = with(hwnd, |l| {
            l.items.get(k as usize).map_or((0, false), |it| {
                let len = it.text.len().min(text.len());
                text[..len].copy_from_slice(&it.text[..len]);
                (len, it.selected)
            })
        })
        .unwrap_or((0, false));
        let y = row as i32 * h;
        if selected {
            fill(hdc, 0, y, right, y + h, if focused { SELECTED } else { SELECTED_UNFOCUSED });
        }
        let color = if selected && focused { BACK } else { TEXT };
        crate::controls::text_color(hdc, color);
        let mut tr = [4, y, right - 2, y + h];
        crate::text::DrawTextW(hdc, text.as_ptr(), len as i32, &mut tr, 0x24 | 0x800 | 0x8000);
    }
    if scrolls {
        let th = (ch * rows as i32 / n as i32).max(16).min(ch);
        let span = (n - rows + 1).max(1) as i32;
        let ty = (ch - th) * top as i32 / span;
        round_fill(hdc, [w - 6, ty + 2, w - 3, ty + th - 2], 1, THUMB);
    }
    if old != 0 {
        select_object(hdc, old);
    }
    crate::window::EndPaint(hwnd, &ps);
}

/// The LISTBOX window procedure.
pub extern "system" fn proc_(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize {
    match msg {
        WM_DESTROY => {
            forget(hwnd);
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_PAINT => {
            paint(hwnd);
            0
        }
        WM_ERASEBKGND => 1,
        WM_SETFONT => {
            with(hwnd, |l| l.font = wparam);
            if lparam != 0 {
                redraw(hwnd);
            }
            0
        }
        WM_GETFONT => with(hwnd, |l| l.font as isize).unwrap_or(0),
        WM_GETDLGCODE => 0x1 | 0x80, // DLGC_WANTARROWS | DLGC_WANTCHARS
        WM_SETFOCUS => {
            redraw(hwnd);
            notify(hwnd, LBN_SETFOCUS);
            0
        }
        WM_KILLFOCUS => {
            redraw(hwnd);
            notify(hwnd, LBN_KILLFOCUS);
            0
        }
        WM_LBUTTONDOWN => {
            press(hwnd, (lparam >> 16) as i16 as i32, wparam);
            0
        }
        WM_LBUTTONUP => {
            if with(hwnd, |l| core::mem::replace(&mut l.pending, false)).unwrap_or(false) {
                notify(hwnd, LBN_SELCHANGE);
            }
            0
        }
        WM_LBUTTONDBLCLK => {
            if row_at(hwnd, (lparam >> 16) as i16 as i32).is_some() {
                notify(hwnd, LBN_DBLCLK);
            }
            0
        }
        WM_KEYDOWN => {
            key(hwnd, wparam);
            0
        }
        WM_MOUSEWHEEL => {
            let delta = (wparam >> 16) as i16 as isize;
            let top = with(hwnd, |l| l.top).unwrap_or(0);
            set_top(hwnd, top - delta / 120 * 3);
            0
        }
        WM_VSCROLL => {
            let (top, h, n) =
                with(hwnd, |l| (l.top, l.item_h, l.items.len() as isize)).unwrap_or((0, 1, 0));
            let rows = page(hwnd, h);
            let to = match wparam & 0xffff {
                0 => top - 1,
                1 => top + 1,
                2 => top - rows,
                3 => top + rows,
                4 | 5 => (wparam >> 16) as isize,
                6 => 0,
                7 => n,
                _ => top,
            };
            set_top(hwnd, to);
            0
        }
        LB_ADDSTRING => insert(hwnd, -2, wide(lparam)),
        LB_INSERTSTRING => insert(hwnd, wparam as isize, wide(lparam)),
        LB_DELETESTRING => delete(hwnd, wparam as isize),
        LB_RESETCONTENT => {
            with(hwnd, |l| {
                l.items.clear();
                l.current = -1;
                l.caret = 0;
                l.anchor = 0;
                l.top = 0;
            });
            redraw(hwnd);
            0
        }
        LB_GETCOUNT => with(hwnd, |l| l.items.len() as isize).unwrap_or(0),
        LB_GETTEXTLEN => {
            with(hwnd, |l| l.items.get(wparam).map_or(LB_ERR, |it| it.text.len() as isize))
                .unwrap_or(LB_ERR)
        }
        LB_GETTEXT => {
            if lparam == 0 {
                return LB_ERR;
            }
            with(hwnd, |l| match l.items.get(wparam) {
                Some(it) => {
                    let p = lparam as *mut u16;
                    // SAFETY: the caller's buffer, as documented big enough
                    // for LB_GETTEXTLEN's characters and a NUL.
                    unsafe {
                        core::ptr::copy_nonoverlapping(it.text.as_ptr(), p, it.text.len());
                        *p.add(it.text.len()) = 0;
                    }
                    it.text.len() as isize
                }
                None => LB_ERR,
            })
            .unwrap_or(LB_ERR)
        }
        LB_GETITEMDATA => with(hwnd, |l| l.items.get(wparam).map_or(LB_ERR, |it| it.data as isize))
            .unwrap_or(LB_ERR),
        LB_SETITEMDATA => with(hwnd, |l| match l.items.get_mut(wparam) {
            Some(it) => {
                it.data = lparam as usize;
                1
            }
            None => LB_ERR,
        })
        .unwrap_or(LB_ERR),
        LB_GETCURSEL => {
            if multiple(hwnd) {
                with(hwnd, |l| l.caret).unwrap_or(0)
            } else {
                with(hwnd, |l| l.current).unwrap_or(LB_ERR)
            }
        }
        LB_SETCURSEL => {
            if multiple(hwnd) {
                return LB_ERR;
            }
            let i = wparam as isize;
            let n = with(hwnd, |l| l.items.len() as isize).unwrap_or(0);
            if i < 0 || i >= n {
                // -1 clears the selection and returns LB_ERR, as documented.
                set_current(hwnd, -1);
                return LB_ERR;
            }
            set_current(hwnd, i);
            i
        }
        LB_GETSEL => {
            with(hwnd, |l| l.items.get(wparam).map_or(LB_ERR, |it| isize::from(it.selected)))
                .unwrap_or(LB_ERR)
        }
        LB_SETSEL => {
            if !multiple(hwnd) {
                return LB_ERR;
            }
            set_sel(hwnd, wparam != 0, lparam)
        }
        LB_SELITEMRANGE => {
            if !multiple(hwnd) {
                return LB_ERR;
            }
            let (a, b) = (lparam & 0xffff, (lparam >> 16) & 0xffff);
            for i in a.min(b)..=a.max(b) {
                set_sel(hwnd, wparam != 0, i);
            }
            0
        }
        LB_GETSELCOUNT => {
            if !multiple(hwnd) {
                return LB_ERR;
            }
            with(hwnd, |l| l.items.iter().filter(|it| it.selected).count() as isize).unwrap_or(0)
        }
        LB_GETSELITEMS => {
            if !multiple(hwnd) || lparam == 0 {
                return LB_ERR;
            }
            with(hwnd, |l| {
                let out = lparam as *mut i32;
                let mut n = 0;
                for (i, it) in l.items.iter().enumerate() {
                    if it.selected && n < wparam {
                        // SAFETY: the caller's array of `wparam` ints.
                        unsafe { *out.add(n) = i as i32 };
                        n += 1;
                    }
                }
                n as isize
            })
            .unwrap_or(LB_ERR)
        }
        LB_FINDSTRING | LB_FINDSTRINGEXACT => {
            find(hwnd, wparam as isize, &wide(lparam), msg == LB_FINDSTRINGEXACT)
        }
        LB_SELECTSTRING => {
            if multiple(hwnd) {
                return LB_ERR;
            }
            let i = find(hwnd, wparam as isize, &wide(lparam), false);
            if i >= 0 {
                set_current(hwnd, i);
            }
            i
        }
        LB_GETTOPINDEX => with(hwnd, |l| l.top).unwrap_or(0),
        LB_SETTOPINDEX => {
            let n = with(hwnd, |l| l.items.len()).unwrap_or(0);
            if wparam >= n && !(wparam == 0 && n == 0) {
                return LB_ERR;
            }
            set_top(hwnd, wparam as isize);
            0
        }
        LB_GETITEMRECT => {
            if lparam == 0 {
                return LB_ERR;
            }
            match item_rect(hwnd, wparam as isize) {
                Some((r, shown)) => {
                    // SAFETY: the caller's RECT.
                    unsafe { *(lparam as *mut [i32; 4]) = r };
                    isize::from(shown)
                }
                None => LB_ERR,
            }
        }
        LB_GETITEMHEIGHT => with(hwnd, |l| l.item_h as isize).unwrap_or(ITEM_H as isize),
        LB_SETITEMHEIGHT => {
            let h = lparam & 0xffff;
            if h == 0 || h > 255 {
                return LB_ERR;
            }
            with(hwnd, |l| l.item_h = h as i32);
            redraw(hwnd);
            0
        }
        LB_GETCARETINDEX => with(hwnd, |l| l.caret).unwrap_or(0),
        LB_SETCARETINDEX => {
            let n = with(hwnd, |l| l.items.len()).unwrap_or(0);
            if wparam >= n {
                return LB_ERR;
            }
            with(hwnd, |l| l.caret = wparam as isize);
            show(hwnd, wparam as isize);
            redraw(hwnd);
            0
        }
        LB_GETANCHORINDEX => with(hwnd, |l| l.anchor).unwrap_or(0),
        LB_SETANCHORINDEX => {
            with(hwnd, |l| l.anchor = wparam as isize);
            0
        }
        LB_ITEMFROMPOINT => {
            let (x, y) = (lparam as i16 as i32, (lparam >> 16) as i16 as i32);
            let (n, top, h) =
                with(hwnd, |l| (l.items.len() as isize, l.top, l.item_h)).unwrap_or((0, 0, 1));
            let mut c = [0i32; 4];
            crate::tree::GetClientRect(hwnd, &mut c);
            let outside = x < 0 || y < 0 || x >= c[2] || y >= c[3];
            let i = (top + (y.max(0) / h.max(1)) as isize).min((n - 1).max(0));
            let outside = outside || top + (y.max(0) / h.max(1)) as isize >= n;
            (i & 0xffff) | (isize::from(outside) << 16)
        }
        LB_GETHORIZONTALEXTENT => with(hwnd, |l| l.extent as isize).unwrap_or(0),
        LB_SETHORIZONTALEXTENT => {
            with(hwnd, |l| l.extent = wparam);
            0
        }
        LB_SETTABSTOPS => 1,
        LB_SETCOLUMNWIDTH => 0,
        LB_SETLOCALE | LB_GETLOCALE => 0x0409,
        LB_SETCOUNT => LB_ERR,
        LB_INITSTORAGE => {
            with(hwnd, |l| l.items.len() as isize + wparam as isize).unwrap_or(LB_ERRSPACE)
        }
        LB_GETLISTBOXINFO => {
            let h = with(hwnd, |l| l.item_h).unwrap_or(ITEM_H);
            page(hwnd, h)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

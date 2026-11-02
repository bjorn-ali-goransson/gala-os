//! Menus on the screen. The menu bar is drawn into the top rows of its
//! window's buffer (above the client area, `MENU_BAR` high); the popup
//! menus are frameless popup windows of class #32768. A press on the bar
//! (WM_NCLBUTTONDOWN, HTMENU → WM_SYSCOMMAND SC_MOUSEMENU) or
//! TrackPopupMenu(Ex) runs the modal menu loop here: it opens popups
//! (WM_INITMENUPOPUP first, so the program can enable and check items),
//! follows the mouse (the first popup has the desktop's mouse capture, so a
//! press anywhere else ends the loop) and the keys, and ends with the
//! chosen item's WM_COMMAND (or its id, with TPM_RETURNCMD).
//!
//! Drawn as Windows 11 draws classic menus: a light bar whose items light
//! up with a rounded hover, popups with a thin border, rounded item
//! highlights, check marks, accelerator text at the right and a chevron
//! for submenus. Not yet: item bitmaps, keyboard access to the bar (Alt,
//! F10), mnemonic underlines.

use crate::controls::{
    default_font, fill, rgb, round_fill, select, text_color, text_height, text_out, text_width,
    transparent,
};
use crate::menu::{self, View};
use crate::window::{Handle, Hwnd, Msg, SendMessageW, window};
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

const BAR_BACK: u32 = rgb(0xf7, 0xf8, 0xfa);
const BAR_HOT: u32 = rgb(0xe4, 0xe7, 0xeb);
const BAR_OPEN: u32 = rgb(0xd9, 0xdd, 0xe2);
const POPUP_BACK: u32 = rgb(0xf9, 0xf9, 0xf9);
const POPUP_BORDER: u32 = rgb(0xc8, 0xc8, 0xc8);
const POPUP_HOT: u32 = rgb(0xe6, 0xe8, 0xeb);
const SEPARATOR: u32 = rgb(0xdd, 0xdd, 0xdd);
const TEXT: u32 = rgb(0x1b, 0x1b, 0x1b);
const TEXT_OFF: u32 = rgb(0x9d, 0x9d, 0x9d);

/// A bar item's horizontal padding.
const BAR_PAD: i32 = 9;
/// A popup's inner padding, item and separator heights, the text's left
/// edge (after the check mark's column).
const PAD: i32 = 4;
const ITEM_H: i32 = 26;
const SEP_H: i32 = 9;
const TEXT_X: i32 = 36;
const MAX_BAR_ITEMS: usize = 32;
const MAX_LEVELS: usize = 6;

const WM_PAINT: u32 = 0x000f;
const WM_ERASEBKGND: u32 = 0x0014;
const WM_MOUSEACTIVATE: u32 = 0x0021;
const WM_MEASUREITEM: u32 = 0x002c;
const WM_DRAWITEM: u32 = 0x002b;
const WM_COMMAND: u32 = 0x0111;
const WM_INITMENU: u32 = 0x0116;
const WM_INITMENUPOPUP: u32 = 0x0117;
const WM_MENUSELECT: u32 = 0x011f;
const WM_UNINITMENUPOPUP: u32 = 0x0125;
const WM_ENTERMENULOOP: u32 = 0x0211;
const WM_EXITMENULOOP: u32 = 0x0212;
const WM_QUIT: u32 = 0x0012;
const WM_KEYDOWN: u32 = 0x0100;
const WM_KEYUP: u32 = 0x0101;
const WM_CHAR: u32 = 0x0102;
const WM_SYSKEYDOWN: u32 = 0x0104;
const WM_SYSKEYUP: u32 = 0x0105;
const WM_SYSCHAR: u32 = 0x0106;
const WM_MOUSEMOVE: u32 = 0x0200;
const WM_LBUTTONDOWN: u32 = 0x0201;
const WM_LBUTTONUP: u32 = 0x0202;
const WM_RBUTTONDOWN: u32 = 0x0204;
const WM_RBUTTONUP: u32 = 0x0205;
const WM_NCLBUTTONDOWN: u32 = 0x00a1;
const HTMENU: usize = 5;

const TPM_CENTERALIGN: u32 = 0x4;
const TPM_RIGHTALIGN: u32 = 0x8;
const TPM_VCENTERALIGN: u32 = 0x10;
const TPM_BOTTOMALIGN: u32 = 0x20;
const TPM_RIGHTBUTTON: u32 = 0x2;
const TPM_NONOTIFY: u32 = 0x80;
const TPM_RETURNCMD: u32 = 0x100;

const MF_POPUP: usize = 0x10;
const MF_HILITE: usize = 0x80;
const MF_GRAYED: usize = 0x1;
const MF_DISABLED: usize = 0x2;
const MF_CHECKED: usize = 0x8;
const MF_SEPARATOR: usize = 0x800;
const MF_OWNERDRAW: usize = 0x100;
const MF_MOUSESELECT: usize = 0x8000;

const VK_RETURN: usize = 0x0d;
const VK_ESCAPE: usize = 0x1b;
const VK_MENU: usize = 0x12;
const VK_LEFT: usize = 0x25;
const VK_UP: usize = 0x26;
const VK_RIGHT: usize = 0x27;
const VK_DOWN: usize = 0x28;
const VK_F10: usize = 0x79;

// ---- Text ----

/// An item's label without its mnemonic marks ("&&" is an ampersand) and
/// the accelerator text after a tab.
struct Label {
    text: [u16; 64],
    len: usize,
    accel: [u16; 64],
    accel_len: usize,
    /// The mnemonic (lower case), if any.
    key: u16,
}

fn label(v: &View) -> Label {
    let mut l = Label { text: [0; 64], len: 0, accel: [0; 64], accel_len: 0, key: 0 };
    let src = &v.text[..v.len];
    let tab = src.iter().position(|&c| c == u16::from(b'\t')).unwrap_or(src.len());
    let mut k = 0;
    while k < tab {
        let c = src[k];
        if c == u16::from(b'&') && k + 1 < tab {
            k += 1;
            if src[k] != u16::from(b'&') && l.key == 0 {
                l.key = lower(src[k]);
            }
        }
        if l.len < l.text.len() {
            l.text[l.len] = src[k];
            l.len += 1;
        }
        k += 1;
    }
    for &c in src.iter().skip(tab + 1) {
        if l.accel_len < l.accel.len() {
            l.accel[l.accel_len] = c;
            l.accel_len += 1;
        }
    }
    l
}

fn lower(c: u16) -> u16 {
    if (u16::from(b'A')..=u16::from(b'Z')).contains(&c) { c + 32 } else { c }
}

/// Runs `f` with a DC (the menu font selected) for measuring text.
fn measuring<R>(f: impl FnOnce(Handle) -> R) -> Option<R> {
    let mut pixel = [0u32; 1];
    let hdc = crate::window::surface_dc(pixel.as_mut_ptr(), 1, 1, 1);
    if hdc.is_null() {
        return None;
    }
    let old = select(hdc, default_font());
    let r = f(hdc);
    if old != 0 {
        select(hdc, old);
    }
    crate::window::free_surface_dc(hdc);
    Some(r)
}

// ---- The menu bar ----

/// The bar items' left and right edges (in the window's client
/// coordinates); how many.
fn bar_layout(hdc: Handle, menu: usize, out: &mut [(i32, i32); MAX_BAR_ITEMS]) -> usize {
    let n = menu::count(menu).min(MAX_BAR_ITEMS);
    let mut x = 4;
    for (k, slot) in out.iter_mut().take(n).enumerate() {
        let w = menu::view(menu, k).map_or(0, |v| {
            let l = label(&v);
            text_width(hdc, &l.text[..l.len])
        });
        *slot = (x, x + w + 2 * BAR_PAD);
        x = slot.1;
    }
    n
}

/// The bar item at client x, if any.
fn bar_item_at(hwnd: Hwnd, x: i32) -> Option<usize> {
    let menu = menu::GetMenu(hwnd);
    let mut rects = [(0, 0); MAX_BAR_ITEMS];
    let n = measuring(|hdc| bar_layout(hdc, menu, &mut rects))?;
    rects[..n].iter().position(|&(l, r)| x >= l && x < r)
}

/// The bar's lit item: the window and the item (hot: under the pointer;
/// open: its menu is showing).
static HOT: AtomicUsize = AtomicUsize::new(0);
static HOT_ITEM: AtomicUsize = AtomicUsize::new(usize::MAX);
static OPEN: AtomicUsize = AtomicUsize::new(0);
static OPEN_ITEM: AtomicUsize = AtomicUsize::new(usize::MAX);

fn lit(window: &AtomicUsize, item: &AtomicUsize, hwnd: Hwnd) -> Option<usize> {
    (window.load(Ordering::Relaxed) == hwnd)
        .then(|| item.load(Ordering::Relaxed))
        .filter(|&k| k != usize::MAX)
}

/// Draws the window's menu bar into its buffer and shows it.
pub(crate) fn draw_bar(hwnd: Hwnd) {
    let Some(w) = window(hwnd) else { return };
    if w.bar == 0 || w.parent != 0 || w.pixels.is_null() {
        return;
    }
    let menu = menu::GetMenu(hwnd);
    let base = crate::window::buffer_base(&w);
    let hdc = crate::window::surface_dc(base, w.cw, w.bar, w.cw);
    if hdc.is_null() {
        return;
    }
    let old = select(hdc, default_font());
    transparent(hdc);
    fill(hdc, 0, 0, w.cw, w.bar, BAR_BACK);
    let mut rects = [(0, 0); MAX_BAR_ITEMS];
    let n = bar_layout(hdc, menu, &mut rects);
    let th = text_height(hdc);
    let (hot, open) = (lit(&HOT, &HOT_ITEM, hwnd), lit(&OPEN, &OPEN_ITEM, hwnd));
    for (k, &(l, r)) in rects.iter().take(n).enumerate() {
        let Some(v) = menu::view(menu, k) else { continue };
        if open == Some(k) {
            round_fill(hdc, [l, 1, r, w.bar - 1], 4, BAR_OPEN);
        } else if hot == Some(k) {
            round_fill(hdc, [l, 1, r, w.bar - 1], 4, BAR_HOT);
        }
        let lb = label(&v);
        text_color(hdc, if v.disabled { TEXT_OFF } else { TEXT });
        text_out(hdc, l + BAR_PAD, (w.bar - th) / 2, &lb.text[..lb.len]);
    }
    if old != 0 {
        select(hdc, old);
    }
    crate::window::free_surface_dc(hdc);
    crate::window::present_bar(hwnd);
}

fn set_hot(hwnd: Hwnd, item: Option<usize>) {
    let old = lit(&HOT, &HOT_ITEM, hwnd);
    if old == item {
        return;
    }
    let before = HOT.swap(if item.is_some() { hwnd } else { 0 }, Ordering::Relaxed);
    HOT_ITEM.store(item.unwrap_or(usize::MAX), Ordering::Relaxed);
    if before != 0 && before != hwnd {
        draw_bar(before);
    }
    draw_bar(hwnd);
}

/// Mouse input from the desktop for top-level window `top` (lParam in the
/// desktop's client coordinates, which start with the menu bar): the
/// bar's own (hover, a press opens the menu as WM_NCLBUTTONDOWN HTMENU);
/// whether it was.
pub(crate) fn bar_input(top: Hwnd, message: u32, lparam: isize) -> bool {
    let Some(w) = window(top) else { return false };
    if w.bar == 0 {
        return false;
    }
    let (x, y) = (lparam as i16 as i32, (lparam >> 16) as i16 as i32);
    if y >= w.bar || y < 0 {
        if message == WM_MOUSEMOVE && lit(&HOT, &HOT_ITEM, top).is_some() {
            set_hot(top, None);
        }
        return false;
    }
    match message {
        WM_MOUSEMOVE => set_hot(top, bar_item_at(top, x)),
        WM_LBUTTONDOWN => {
            let (b, t) = crate::window::desktop_frame(&w);
            let (sx, sy) = (w.x + b + x, w.y + t + y);
            let pt = ((sy as i16 as u16 as isize) << 16) | sx as i16 as u16 as isize;
            crate::window::PostMessageW(top, WM_NCLBUTTONDOWN, HTMENU, pt);
        }
        _ => {}
    }
    true
}

// ---- The menu loop ----

#[derive(Clone, Copy)]
struct Level {
    menu: usize,
    hwnd: Hwnd,
    /// The popup's screen rectangle (x, y, w, h).
    rect: [i32; 4],
    sel: Option<usize>,
}

const NO_LEVEL: Level = Level { menu: 0, hwnd: 0, rect: [0; 4], sel: None };

struct Tracking {
    owner: Hwnd,
    /// The window whose bar is tracked (0: TrackPopupMenu).
    bar: Hwnd,
    bar_menu: usize,
    bar_sel: Option<usize>,
    levels: [Level; MAX_LEVELS],
    depth: usize,
    notify: bool,
    right: bool,
    chosen: Option<u32>,
    done: bool,
}

const IDLE: Tracking = Tracking {
    owner: 0,
    bar: 0,
    bar_menu: 0,
    bar_sel: None,
    levels: [NO_LEVEL; MAX_LEVELS],
    depth: 0,
    notify: true,
    right: false,
    chosen: None,
    done: false,
};

struct Shared(UnsafeCell<Tracking>);
// SAFETY: only the thread running the menu loop touches it (BUSY).
unsafe impl Sync for Shared {}
static STATE: Shared = Shared(UnsafeCell::new(IDLE));
static BUSY: AtomicBool = AtomicBool::new(false);

/// The loop's state (only while BUSY, on its thread; never kept across a
/// call that could re-enter).
fn st() -> &'static mut Tracking {
    // SAFETY: one menu loop at a time (BUSY), on one thread; callers do not
    // hold the reference across window procedure calls.
    unsafe { &mut *STATE.0.get() }
}

/// The height of item `v` in a popup.
fn item_height(v: &View) -> i32 {
    if v.separator { SEP_H } else { ITEM_H }
}

/// Item k's top in its popup.
fn item_top(menu: usize, k: usize) -> i32 {
    let mut y = PAD;
    for j in 0..k {
        y += menu::view(menu, j).map_or(0, |v| item_height(&v));
    }
    y
}

/// The item at popup-relative y.
fn item_at(menu: usize, y: i32) -> Option<usize> {
    let mut top = PAD;
    for k in 0..menu::count(menu) {
        let h = menu::view(menu, k).map_or(0, |v| item_height(&v));
        if y >= top && y < top + h {
            return Some(k);
        }
        top += h;
    }
    None
}

/// WM_MEASUREITEM for an owner-drawn item: its height (and width).
fn measure_owner_drawn(owner: Hwnd, v: &View) -> (i32, i32) {
    #[repr(C)]
    struct MeasureItem {
        kind: u32,
        ctl_id: u32,
        item_id: u32,
        width: u32,
        height: u32,
        data: usize,
    }
    let mut m =
        MeasureItem { kind: 1, ctl_id: 0, item_id: v.id, width: 0, height: 0, data: v.data };
    SendMessageW(owner, WM_MEASUREITEM, 0, &mut m as *mut MeasureItem as isize);
    (m.width as i32, m.height as i32)
}

/// A popup's size for `menu`.
fn popup_size(owner: Hwnd, menu: usize) -> (i32, i32) {
    let n = menu::count(menu);
    let (mut label_w, mut accel_w, mut h) = (0, 0, 2 * PAD);
    measuring(|hdc| {
        for k in 0..n {
            let Some(v) = menu::view(menu, k) else { continue };
            if v.owner_draw {
                let (ow, oh) = measure_owner_drawn(owner, &v);
                label_w = label_w.max(ow);
                h += oh.max(1);
                continue;
            }
            h += item_height(&v);
            if v.separator {
                continue;
            }
            let l = label(&v);
            label_w = label_w.max(text_width(hdc, &l.text[..l.len]));
            accel_w = accel_w.max(text_width(hdc, &l.accel[..l.accel_len]));
        }
    });
    let w = TEXT_X + label_w + if accel_w > 0 { 40 + accel_w } else { 0 } + 36;
    (w.max(140), h)
}

/// Where a popup of w x h goes for anchor (x, y) with TPM_ alignment,
/// kept on the screen's work area (flipped when it does not fit below or
/// to the right).
fn place(x: i32, y: i32, w: i32, h: i32, flags: u32, flip_x: i32, flip_y: i32) -> (i32, i32) {
    let (_, work) = crate::monitor::screen();
    let mut px = if flags & TPM_RIGHTALIGN != 0 {
        x - w
    } else if flags & TPM_CENTERALIGN != 0 {
        x - w / 2
    } else {
        x
    };
    let mut py = if flags & TPM_BOTTOMALIGN != 0 {
        y - h
    } else if flags & TPM_VCENTERALIGN != 0 {
        y - h / 2
    } else {
        y
    };
    if px + w > work[2] {
        px = if flip_x != i32::MIN { flip_x - w } else { work[2] - w };
    }
    if py + h > work[3] {
        py = if flip_y != i32::MIN { flip_y - h } else { work[3] - h };
    }
    (px.max(work[0]), py.max(work[1]))
}

/// Opens `menu` as popup level `level` at anchor (x, y) (WM_INITMENUPOPUP
/// first); the levels from `level` on are closed before.
#[allow(clippy::too_many_arguments)]
fn open(level: usize, menu: usize, pos: usize, x: i32, y: i32, flags: u32, fx: i32, fy: i32) {
    if level >= MAX_LEVELS {
        return;
    }
    close_from(level);
    let (owner, notify) = (st().owner, st().notify);
    if notify {
        SendMessageW(owner, WM_INITMENUPOPUP, menu, pos as isize);
    }
    if menu::count(menu) == 0 {
        return;
    }
    let (w, h) = popup_size(owner, menu);
    let (px, py) = place(x, y, w, h, flags, fx, fy);
    const WS_POPUP: u32 = 0x8000_0000;
    const WS_EX_TOOLWINDOW: u32 = 0x80;
    const WS_EX_TOPMOST: u32 = 0x8;
    let class: [u16; 7] = [35, 51, 50, 55, 54, 56, 0]; // "#32768"
    let hwnd = crate::window::CreateWindowExW(
        WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
        class.as_ptr(),
        [0u16].as_ptr(),
        WS_POPUP,
        px,
        py,
        w,
        h,
        0,
        core::ptr::null_mut(),
        core::ptr::null_mut(),
        core::ptr::null_mut(),
    );
    if hwnd == 0 {
        return;
    }
    let s = st();
    s.levels[level] = Level { menu, hwnd, rect: [px, py, w, h], sel: None };
    s.depth = level + 1;
    crate::window::ShowWindow(hwnd, 4); // SW_SHOWNOACTIVATE
    if level == 0 {
        // Every press lands here: one outside the menus ends the loop.
        if let Some(pw) = window(hwnd) {
            // SAFETY: our popup's desktop window.
            unsafe { crate::window::GalaWindowCapture(pw.desktop) };
        }
    }
}

/// Closes popup levels `level` and deeper (WM_UNINITMENUPOPUP each).
fn close_from(level: usize) {
    while st().depth > level {
        let s = st();
        s.depth -= 1;
        let l = s.levels[s.depth];
        s.levels[s.depth] = NO_LEVEL;
        let (owner, notify) = (s.owner, s.notify);
        if l.hwnd != 0 {
            crate::window::DestroyWindow(l.hwnd);
        }
        if notify {
            SendMessageW(owner, WM_UNINITMENUPOPUP, l.menu, 0);
        }
    }
}

/// WM_MENUSELECT for item `k` of `menu` (MF_ flags in wParam's high word;
/// a submenu's item gives its position).
fn menu_select(menu: usize, k: usize) {
    let (owner, notify) = (st().owner, st().notify);
    let Some(v) = menu::view(menu, k) else { return };
    if !notify {
        return;
    }
    let mut flags = MF_HILITE | MF_MOUSESELECT;
    if v.submenu != 0 {
        flags |= MF_POPUP;
    }
    if v.disabled {
        flags |= MF_GRAYED | MF_DISABLED;
    }
    if v.checked {
        flags |= MF_CHECKED;
    }
    if v.separator {
        flags |= MF_SEPARATOR;
    }
    if v.owner_draw {
        flags |= MF_OWNERDRAW;
    }
    let id = if v.submenu != 0 { k } else { v.id as usize & 0xffff };
    SendMessageW(owner, WM_MENUSELECT, id | flags << 16, menu as isize);
}

/// Item `k` (or none) of popup level `level` is the selection; a submenu
/// opens to its right.
fn hover(level: usize, k: Option<usize>, open_sub: bool) {
    let l = st().levels[level];
    if l.sel == k {
        if st().depth > level + 1 || !open_sub {
            return;
        }
    } else {
        st().levels[level].sel = k;
        crate::window::InvalidateRect(l.hwnd, core::ptr::null(), 0);
        if let Some(k) = k {
            menu_select(l.menu, k);
        }
    }
    close_from(level + 1);
    let Some(k) = k else { return };
    let Some(v) = menu::view(l.menu, k) else { return };
    if open_sub && v.submenu != 0 && !v.disabled {
        let top = l.rect[1] + item_top(l.menu, k) - PAD;
        open(level + 1, v.submenu, k, l.rect[0] + l.rect[2] - 4, top, 0, l.rect[0] + 4, i32::MIN);
    }
}

/// The bar's item `k` opens its menu (under the item).
fn open_bar(k: usize) {
    let (bar, menu) = (st().bar, st().bar_menu);
    close_from(0);
    st().bar_sel = Some(k);
    OPEN.store(bar, Ordering::Relaxed);
    OPEN_ITEM.store(k, Ordering::Relaxed);
    HOT_ITEM.store(usize::MAX, Ordering::Relaxed);
    draw_bar(bar);
    menu_select(menu, k);
    let Some(v) = menu::view(menu, k) else { return };
    if v.submenu == 0 || v.disabled {
        return;
    }
    let Some(w) = window(bar) else { return };
    let mut rects = [(0, 0); MAX_BAR_ITEMS];
    let n = measuring(|hdc| bar_layout(hdc, menu, &mut rects)).unwrap_or(0);
    if k >= n {
        return;
    }
    let (b, t) = crate::window::desktop_frame(&w);
    let (x, y) = (w.x + b + rects[k].0, w.y + t + w.bar);
    open(0, v.submenu, k, x, y, 0, i32::MIN, w.y + t);
}

/// The bar item under screen point (x, y), if tracking a bar.
fn bar_hit(x: i32, y: i32) -> Option<usize> {
    let bar = st().bar;
    let w = window(bar)?;
    let (b, t) = crate::window::desktop_frame(&w);
    let (cx, cy) = (x - w.x - b, y - w.y - t);
    if cy < 0 || cy >= w.bar {
        return None;
    }
    bar_item_at(bar, cx)
}

/// The deepest popup level under screen point (x, y).
fn level_at(x: i32, y: i32) -> Option<usize> {
    let s = st();
    (0..s.depth).rev().find(|&l| {
        let r = s.levels[l].rect;
        x >= r[0] && y >= r[1] && x < r[0] + r[2] && y < r[1] + r[3]
    })
}

fn choose(id: u32) {
    let s = st();
    s.chosen = Some(id);
    s.done = true;
}

/// Activates item `k` of level `level`: a command ends the loop, a
/// submenu opens (its first item selected when `first`).
fn activate(level: usize, k: usize, first: bool) {
    let menu = st().levels[level].menu;
    let Some(v) = menu::view(menu, k) else { return };
    if v.separator || v.disabled {
        return;
    }
    if v.submenu != 0 {
        hover(level, Some(k), true);
        if first && st().depth > level + 1 {
            step(level + 1, 1);
        }
        return;
    }
    choose(v.id);
}

/// Moves level `level`'s selection by `dir` over the items that are not
/// separators (wrapping).
fn step(level: usize, dir: i32) {
    let l = st().levels[level];
    let n = menu::count(l.menu) as i32;
    if n == 0 {
        return;
    }
    let mut k = l.sel.map_or(if dir > 0 { -1 } else { n }, |s| s as i32);
    for _ in 0..n {
        k = (k + dir).rem_euclid(n);
        if menu::view(l.menu, k as usize).is_some_and(|v| !v.separator) {
            hover(level, Some(k as usize), false);
            return;
        }
    }
}

fn mouse(m: &Msg) {
    let (x, y) = (m.pt[0], m.pt[1]);
    let at = level_at(x, y);
    let right = st().right;
    match m.message {
        WM_MOUSEMOVE => {
            if let Some(l) = at {
                let r = st().levels[l].rect;
                let k = item_at(st().levels[l].menu, y - r[1]);
                let k =
                    k.filter(|&k| menu::view(st().levels[l].menu, k).is_some_and(|v| !v.separator));
                hover(l, k, true);
            } else if let Some(k) = bar_hit(x, y) {
                if st().bar_sel != Some(k) {
                    open_bar(k);
                }
            } else {
                let d = st().depth;
                if d > 0 && st().levels[d - 1].sel.is_some() {
                    let l = st().levels[d - 1];
                    let has_sub =
                        l.sel.and_then(|k| menu::view(l.menu, k)).is_some_and(|v| v.submenu != 0);
                    if !has_sub {
                        hover(d - 1, None, false);
                    }
                }
            }
        }
        WM_LBUTTONDOWN | WM_RBUTTONDOWN => {
            if at.is_some() {
                return;
            }
            match bar_hit(x, y) {
                Some(k) if st().bar_sel == Some(k) => st().done = true,
                Some(k) => open_bar(k),
                None => st().done = true,
            }
        }
        WM_LBUTTONUP | WM_RBUTTONUP => {
            if m.message == WM_RBUTTONUP && !right {
                return;
            }
            let Some(l) = at else { return };
            let r = st().levels[l].rect;
            if let Some(k) = item_at(st().levels[l].menu, y - r[1]) {
                let v = menu::view(st().levels[l].menu, k);
                if v.is_some_and(|v| v.submenu == 0) {
                    activate(l, k, false);
                }
            }
        }
        _ => {}
    }
}

fn key(vk: usize) {
    let d = st().depth;
    let bar = st().bar != 0;
    match vk {
        VK_ESCAPE => {
            if d > 1 {
                close_from(d - 1);
            } else {
                st().done = true;
            }
        }
        VK_MENU | VK_F10 => st().done = true,
        VK_UP | VK_DOWN if d > 0 => step(d - 1, if vk == VK_DOWN { 1 } else { -1 }),
        VK_RIGHT => {
            let l = if d > 0 { Some(st().levels[d - 1]) } else { None };
            let sub = l.and_then(|l| l.sel.and_then(|k| menu::view(l.menu, k)).map(|v| (l, v)));
            match sub {
                Some((l, v)) if v.submenu != 0 => activate(d - 1, l.sel.unwrap_or(0), true),
                _ if bar => next_bar(1),
                _ => {}
            }
        }
        VK_LEFT => {
            if d > 1 {
                close_from(d - 1);
            } else if bar {
                next_bar(-1);
            }
        }
        VK_RETURN if d > 0 => {
            if let Some(k) = st().levels[d - 1].sel {
                activate(d - 1, k, true);
            }
        }
        _ => {}
    }
}

fn next_bar(dir: i32) {
    let n = menu::count(st().bar_menu) as i32;
    if n == 0 {
        return;
    }
    let k = st().bar_sel.map_or(0, |k| (k as i32 + dir).rem_euclid(n));
    open_bar(k as usize);
    if st().depth > 0 {
        step(0, 1);
    }
}

/// A typed character: the deepest popup's item with that mnemonic.
fn mnemonic(c: u16) {
    let d = st().depth;
    if d == 0 {
        return;
    }
    let menu = st().levels[d - 1].menu;
    let c = lower(c);
    for k in 0..menu::count(menu) {
        if menu::view(menu, k).is_some_and(|v| label(&v).key == c) {
            hover(d - 1, Some(k), false);
            activate(d - 1, k, true);
            return;
        }
    }
}

/// The modal loop: until an item is chosen or the menu is dismissed.
fn run() {
    loop {
        if st().done || (st().depth == 0 && st().bar_sel.is_none()) {
            break;
        }
        let mut m =
            Msg { hwnd: 0, message: 0, wparam: 0, lparam: 0, time: 0, pt: [0; 2], private: 0 };
        if crate::window::GetMessageW(&mut m, 0, 0, 0) <= 0 {
            if m.message == WM_QUIT {
                crate::window::PostQuitMessage(m.wparam as i32);
            }
            st().done = true;
            break;
        }
        match m.message {
            0x0200..=0x020d => mouse(&m),
            WM_KEYDOWN | WM_SYSKEYDOWN => key(m.wparam),
            WM_CHAR | WM_SYSCHAR => mnemonic(m.wparam as u16),
            WM_KEYUP | WM_SYSKEYUP => {}
            _ => {
                crate::window::TranslateMessage(&m);
                crate::window::DispatchMessageW(&m);
            }
        }
    }
}

/// Starts a loop for `owner`: WM_ENTERMENULOOP and WM_INITMENU; false if
/// one is already running.
fn begin(owner: Hwnd, bar: Hwnd, menu: usize, flags: u32) -> bool {
    if BUSY.swap(true, Ordering::Acquire) {
        return false;
    }
    let s = st();
    *s = IDLE;
    s.owner = owner;
    s.bar = bar;
    s.bar_menu = if bar != 0 { menu } else { 0 };
    s.notify = flags & TPM_NONOTIFY == 0;
    s.right = flags & TPM_RIGHTBUTTON != 0;
    if s.notify {
        SendMessageW(owner, WM_ENTERMENULOOP, usize::from(bar == 0), 0);
        SendMessageW(owner, WM_INITMENU, menu, 0);
    }
    true
}

/// Ends the loop: the popups go, the capture ends, WM_MENUSELECT (closed)
/// and WM_EXITMENULOOP; the chosen item's id.
fn end() -> Option<u32> {
    close_from(0);
    // SAFETY: a plain call; ends our popup's capture.
    unsafe { crate::window::GalaWindowCapture(0) };
    let (owner, bar, notify, chosen) = (st().owner, st().bar, st().notify, st().chosen);
    OPEN.store(0, Ordering::Relaxed);
    OPEN_ITEM.store(usize::MAX, Ordering::Relaxed);
    HOT.store(0, Ordering::Relaxed);
    HOT_ITEM.store(usize::MAX, Ordering::Relaxed);
    if bar != 0 {
        draw_bar(bar);
    }
    *st() = IDLE;
    BUSY.store(false, Ordering::Release);
    if notify {
        SendMessageW(owner, WM_MENUSELECT, 0xffff << 16, 0);
        SendMessageW(owner, WM_EXITMENULOOP, usize::from(bar == 0), 0);
    }
    chosen
}

/// The menu bar of `hwnd` was pressed at screen (x, y): its menu loop.
pub(crate) fn track_bar(hwnd: Hwnd, x: i32, y: i32) {
    let menu = menu::GetMenu(hwnd);
    if menu == 0 || !begin(hwnd, hwnd, menu, 0) {
        return;
    }
    match bar_hit(x, y) {
        Some(k) => {
            open_bar(k);
            // A command on the bar itself is chosen by the press.
            if let Some(v) = menu::view(menu, k)
                && v.submenu == 0
                && !v.disabled
            {
                choose(v.id);
            }
            run();
        }
        None => st().done = true,
    }
    if let Some(id) = end() {
        crate::window::PostMessageW(hwnd, WM_COMMAND, id as usize & 0xffff, 0);
    }
}

/// TrackPopupMenuEx(menu, flags, x, y, owner, params): the menu at (x, y)
/// until an item is chosen (WM_COMMAND to the owner, or with TPM_RETURNCMD
/// its id as the result) or it is dismissed (0 with TPM_RETURNCMD).
#[unsafe(no_mangle)]
pub extern "system" fn TrackPopupMenuEx(
    menu: usize,
    flags: u32,
    x: i32,
    y: i32,
    owner: Hwnd,
    _params: *const u8,
) -> i32 {
    if menu::IsMenu(menu) == 0 || window(owner).is_none() {
        crate::window::set_last_error(if window(owner).is_none() { 1400 } else { 1401 });
        return 0;
    }
    if !begin(owner, 0, menu, flags) {
        return 0;
    }
    open(0, menu, 0, x, y, flags, x, y);
    run();
    let chosen = end();
    match chosen {
        Some(id) if flags & TPM_RETURNCMD != 0 => id as i32,
        Some(id) => {
            crate::window::PostMessageW(owner, WM_COMMAND, id as usize & 0xffff, 0);
            1
        }
        None => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn TrackPopupMenu(
    menu: usize,
    flags: u32,
    x: i32,
    y: i32,
    _reserved: i32,
    owner: Hwnd,
    _rect: *const [i32; 4],
) -> i32 {
    TrackPopupMenuEx(menu, flags, x, y, owner, core::ptr::null())
}

// ---- The popup windows ----

/// Draws popup level `l`.
fn paint_popup(hwnd: Hwnd, l: Level, owner: Hwnd) {
    let mut ps = crate::window::PaintStruct::EMPTY;
    let hdc = crate::window::BeginPaint(hwnd, &mut ps);
    if hdc.is_null() {
        return;
    }
    let (w, h) = (l.rect[2], l.rect[3]);
    let old = select(hdc, default_font());
    transparent(hdc);
    fill(hdc, 0, 0, w, h, POPUP_BORDER);
    fill(hdc, 1, 1, w - 1, h - 1, POPUP_BACK);
    let th = text_height(hdc);
    let mut y = PAD;
    for k in 0..menu::count(l.menu) {
        let Some(v) = menu::view(l.menu, k) else { continue };
        if v.owner_draw {
            let (_, oh) = measure_owner_drawn(owner, &v);
            draw_owner_drawn(
                owner,
                l.menu,
                &v,
                hdc,
                [1, y, w - 1, y + oh.max(1)],
                l.sel == Some(k),
            );
            y += oh.max(1);
            continue;
        }
        let ih = item_height(&v);
        if v.separator {
            fill(hdc, 1, y + ih / 2, w - 1, y + ih / 2 + 1, SEPARATOR);
            y += ih;
            continue;
        }
        if l.sel == Some(k) {
            round_fill(hdc, [PAD, y + 1, w - PAD, y + ih - 1], 4, POPUP_HOT);
        }
        let color = if v.disabled { TEXT_OFF } else { TEXT };
        text_color(hdc, color);
        let lb = label(&v);
        let ty = y + (ih - th) / 2;
        text_out(hdc, TEXT_X, ty, &lb.text[..lb.len]);
        if lb.accel_len > 0 {
            let aw = text_width(hdc, &lb.accel[..lb.accel_len]);
            text_out(hdc, w - 36 - aw, ty, &lb.accel[..lb.accel_len]);
        }
        let cy = y + ih / 2;
        if v.checked {
            if v.radio {
                round_fill(hdc, [15, cy - 3, 21, cy + 3], 3, color);
            } else {
                check_mark(hdc, 13, cy, color);
            }
        }
        if v.submenu != 0 {
            chevron_right(hdc, w - 18, cy, color);
        }
        y += ih;
    }
    if old != 0 {
        select(hdc, old);
    }
    crate::window::EndPaint(hwnd, &ps);
}

/// A check mark (✓) with its corner at (x + 3, y + 3).
fn check_mark(hdc: Handle, x: i32, y: i32, color: u32) {
    for k in 0..4 {
        fill(hdc, x + k, y + k - 1, x + k + 1, y + k + 1, color);
    }
    for k in 0..7 {
        fill(hdc, x + 3 + k, y + 2 - k, x + 4 + k, y + 4 - k, color);
    }
}

/// A small right-pointing chevron centred at (x, y).
fn chevron_right(hdc: Handle, x: i32, y: i32, color: u32) {
    for k in 0..4 {
        fill(hdc, x + 1 - k, y - k, x + 2 - k, y - k + 1, color);
        fill(hdc, x + 1 - k, y + k, x + 2 - k, y + k + 1, color);
    }
}

/// WM_DRAWITEM for an owner-drawn item.
fn draw_owner_drawn(owner: Hwnd, menu: usize, v: &View, hdc: Handle, r: [i32; 4], selected: bool) {
    #[repr(C)]
    struct DrawItem {
        kind: u32,
        ctl_id: u32,
        item_id: u32,
        action: u32,
        state: u32,
        item: usize,
        hdc: Handle,
        rect: [i32; 4],
        data: usize,
    }
    let state =
        u32::from(selected) | if v.disabled { 6 } else { 0 } | if v.checked { 8 } else { 0 };
    let mut d = DrawItem {
        kind: 1,
        ctl_id: 0,
        item_id: v.id,
        action: 1,
        state,
        item: menu,
        hdc,
        rect: r,
        data: v.data,
    };
    SendMessageW(owner, WM_DRAWITEM, 0, &mut d as *mut DrawItem as isize);
}

/// The #32768 popup menu window: draws its level of the running loop.
pub extern "system" fn popup_proc(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize {
    match msg {
        WM_PAINT => {
            let found = BUSY.load(Ordering::Acquire).then(|| {
                let s = st();
                (s.levels[..s.depth].iter().find(|l| l.hwnd == hwnd).copied(), s.owner)
            });
            match found {
                Some((Some(l), owner)) => paint_popup(hwnd, l, owner),
                _ => {
                    crate::window::DefWindowProcW(hwnd, msg, wparam, lparam);
                }
            }
            0
        }
        WM_ERASEBKGND => 1,
        WM_MOUSEACTIVATE => 3, // MA_NOACTIVATE
        _ => crate::window::DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

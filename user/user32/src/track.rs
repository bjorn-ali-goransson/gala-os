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

/// The screen rectangle (left, top, right, bottom) of `hwnd`'s menu bar
/// (`item` None: the whole bar; an empty bar has an empty one) or of one
/// of its items.
fn bar_rect(hwnd: Hwnd, item: Option<usize>) -> Option<[i32; 4]> {
    let w = window(hwnd)?;
    if w.bar == 0 || w.parent != 0 {
        return None;
    }
    let menu = menu::GetMenu(hwnd);
    let mut rects = [(0, 0); MAX_BAR_ITEMS];
    let n = measuring(|hdc| bar_layout(hdc, menu, &mut rects))?;
    let (b, t) = crate::window::desktop_frame(&w);
    let (x0, y0) = (w.x + b, w.y + t);
    match item {
        None if n == 0 => Some([0; 4]),
        None => Some([x0, y0, x0 + w.cw, y0 + w.bar]),
        Some(k) if k < n => Some([x0 + rects[k].0, y0, x0 + rects[k].1, y0 + w.bar]),
        Some(_) => None,
    }
}

/// MENUBARINFO (x64 layout).
#[repr(C)]
pub struct MenuBarInfo {
    size: u32,
    bar: [i32; 4],
    menu: usize,
    menu_window: Hwnd,
    /// fBarFocused (bit 0) and fFocused (bit 1).
    focus: u32,
}

const OBJID_SYSMENU: i32 = -1;
const OBJID_MENU: i32 = -3;
const OBJID_CLIENT: i32 = -4;

/// GetMenuBarInfo: a window's menu bar (OBJID_MENU; item 0 the whole bar,
/// item n its n-th item) in screen coordinates. No menu, an item past the
/// last or an unknown object: FALSE with the last error untouched, as on
/// Windows; a bad window ERROR_INVALID_WINDOW_HANDLE, a wrong cbSize
/// ERROR_INVALID_PARAMETER, OBJID_CLIENT of a window that is not a popup
/// menu ERROR_INVALID_MENU_HANDLE. The system menu and popup menus'
/// windows are recorded (no rectangle for them yet).
#[unsafe(no_mangle)]
pub extern "system" fn GetMenuBarInfo(
    hwnd: Hwnd,
    object: i32,
    item: i32,
    info: *mut MenuBarInfo,
) -> i32 {
    if crate::window::IsWindow(hwnd) == 0 {
        crate::window::set_last_error(1400); // ERROR_INVALID_WINDOW_HANDLE
        return 0;
    }
    // SAFETY: the caller's MENUBARINFO, its size checked first.
    if info.is_null() || unsafe { (*info).size } as usize != core::mem::size_of::<MenuBarInfo>() {
        crate::window::set_last_error(87); // ERROR_INVALID_PARAMETER
        return 0;
    }
    match object {
        OBJID_MENU => {}
        OBJID_CLIENT => {
            let mut class = [0u16; 8];
            let n = crate::window::GetClassNameW(hwnd, class.as_mut_ptr(), 8);
            if class[..n.max(0) as usize].iter().copied().eq("#32768".encode_utf16()) {
                crate::window::unsupported(
                    b"user32.dll!GetMenuBarInfo(OBJID_CLIENT of a popup menu)",
                    0,
                );
            } else {
                crate::window::set_last_error(1401); // ERROR_INVALID_MENU_HANDLE
            }
            return 0;
        }
        OBJID_SYSMENU => {
            crate::window::unsupported(b"user32.dll!GetMenuBarInfo(OBJID_SYSMENU)", 0);
            return 0;
        }
        _ => return 0,
    }
    let menu = menu::GetMenu(hwnd);
    if menu == 0 || item < 0 {
        return 0;
    }
    let which = if item == 0 { None } else { Some(item as usize - 1) };
    let Some(r) = bar_rect(hwnd, which) else { return 0 };
    let open = lit(&OPEN, &OPEN_ITEM, hwnd);
    let focused = match which {
        None => open.is_some(),
        Some(k) => open == Some(k),
    };
    // SAFETY: the caller's MENUBARINFO (its size checked).
    unsafe {
        (*info).bar = r;
        (*info).menu = menu;
        (*info).menu_window = 0;
        (*info).focus = u32::from(open.is_some()) | (u32::from(focused) << 1);
    }
    1
}

/// GetMenuItemRect: the screen rectangle of an item of a window's menu
/// bar. Popup menus' items (where the popup was last shown) are recorded.
#[unsafe(no_mangle)]
pub extern "system" fn GetMenuItemRect(
    hwnd: Hwnd,
    menu: usize,
    item: u32,
    out: *mut [i32; 4],
) -> i32 {
    if out.is_null() {
        return 0;
    }
    if menu == 0 || menu::GetMenu(hwnd) != menu {
        crate::window::unsupported(b"user32.dll!GetMenuItemRect(not a menu bar)", 0);
        return 0;
    }
    match bar_rect(hwnd, Some(item as usize)) {
        Some(r) => {
            // SAFETY: the caller's RECT.
            unsafe { *out = r };
            1
        }
        None => 0,
    }
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
    // A disabled window (a modal dialog is up) takes no input.
    if w.disabled {
        return true;
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
    // Owned by the window whose menu it is (the bar's window, or
    // TrackPopupMenu's for a submenu), so UI Automation finds it under
    // that window; a context menu's first level has no owner.
    let owner = if level == 0 && st().bar == 0 { 0 } else { st().owner };
    let hwnd = crate::window::CreateWindowExW(
        WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
        class.as_ptr(),
        [0u16].as_ptr(),
        WS_POPUP,
        px,
        py,
        w,
        h,
        owner,
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

// ---- Menus for UI Automation ----

/// gala's own message between UIAutomationCore and user32 (in the system
/// range; user32 answers it itself, the window procedure never sees it):
/// describes and drives a window's menu bar, or a popup level of the
/// running menu loop (its #32768 window), as Windows' UIA menu proxies
/// read menus through win32k. wParam: the operation in the low byte, an
/// item index above it; lParam: for MENU_QUERY, a MENU_INFO_SIZE buffer
/// (marshaled across processes by foreign::send_remote).
pub(crate) const GALA_MENU: u32 = 0x03f0;
pub(crate) const MENU_INFO_SIZE: usize = 8192;
/// The description: count, name (a popup's: the item that opened it),
/// then each item's id, flags, screen rectangle and raw text.
const MENU_QUERY: usize = 1;
/// Opens item k's submenu (as a click on it does).
const MENU_EXPAND: usize = 2;
/// Chooses item k (a command: WM_COMMAND to the owner, as the user's
/// choice does).
const MENU_INVOKE: usize = 3;
/// Closes item k's submenu.
const MENU_COLLAPSE: usize = 4;
const HEADER: usize = 8 + 2 * 64;
const RECORD: usize = 4 + 4 + 16 + 4 + 2 * 64;
const MAX_RECORDS: usize = (MENU_INFO_SIZE - HEADER) / RECORD;
const FLAG_SUBMENU: u32 = 1;
const FLAG_DISABLED: u32 = 2;
const FLAG_SEPARATOR: u32 = 4;
const FLAG_CHECKED: u32 = 8;
const FLAG_OPEN: u32 = 16;
const WM_NULL: u32 = 0;
const MAX_DESCRIBED: usize = if MAX_RECORDS < 48 { MAX_RECORDS } else { 48 };

/// The level of the running loop whose popup is `hwnd`.
fn level_of(hwnd: Hwnd) -> Option<usize> {
    if hwnd == 0 || !BUSY.load(Ordering::Acquire) {
        return None;
    }
    let s = st();
    (0..s.depth).find(|&l| s.levels[l].hwnd == hwnd)
}

/// The bar items' screen rectangles of window `hwnd`; how many.
fn bar_rects(hwnd: Hwnd, menu: usize, out: &mut [[i32; 4]; MAX_DESCRIBED]) -> usize {
    let Some(w) = window(hwnd) else { return 0 };
    let mut rects = [(0, 0); MAX_BAR_ITEMS];
    let n = measuring(|hdc| bar_layout(hdc, menu, &mut rects)).unwrap_or(0);
    let (b, t) = crate::window::desktop_frame(&w);
    for (o, &(l, r)) in out.iter_mut().zip(&rects[..n]) {
        *o = [w.x + b + l, w.y + t, w.x + b + r, w.y + t + w.bar];
    }
    n.min(MAX_DESCRIBED)
}

fn put32(buf: &mut [u8], at: usize, v: u32) {
    buf[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

fn put_text(buf: &mut [u8], at: usize, text: &[u16]) {
    for (k, &c) in text.iter().take(64).enumerate() {
        buf[at + 2 * k..at + 2 * k + 2].copy_from_slice(&c.to_le_bytes());
    }
}

/// The label of item `k` of `menu` (its text without '&' and accelerator).
fn label_of(menu: usize, k: Option<usize>) -> ([u16; 64], usize) {
    match k.and_then(|k| menu::view(menu, k)) {
        Some(v) => {
            let l = label(&v);
            (l.text, l.len)
        }
        None => ([0; 64], 0),
    }
}

/// What MENU_QUERY describes: the menu, its name, the items' screen
/// rectangles and count, and the item whose submenu is open.
struct Described {
    menu: usize,
    name: ([u16; 64], usize),
    rects: [[i32; 4]; MAX_DESCRIBED],
    n: usize,
    open: Option<usize>,
}

/// The popup level `l` of the running loop.
fn describe_level(l: usize) -> Described {
    let s = st();
    let lv = s.levels[l];
    let name = if l > 0 {
        label_of(s.levels[l - 1].menu, s.levels[l - 1].sel)
    } else if s.bar != 0 {
        label_of(s.bar_menu, s.bar_sel)
    } else {
        // A context menu (TrackPopupMenu) is named "Context".
        let mut t = [0u16; 64];
        for (d, c) in t.iter_mut().zip("Context".encode_utf16()) {
            *d = c;
        }
        (t, 7)
    };
    let n = menu::count(lv.menu).min(MAX_DESCRIBED);
    let mut rects = [[0; 4]; MAX_DESCRIBED];
    let mut y = lv.rect[1] + PAD;
    for (k, r) in rects.iter_mut().enumerate().take(n) {
        let h = menu::view(lv.menu, k).map_or(0, |v| item_height(&v));
        *r = [lv.rect[0] + 1, y, lv.rect[0] + lv.rect[2] - 1, y + h];
        y += h;
    }
    let open = if s.depth > l + 1 { lv.sel } else { None };
    Described { menu: lv.menu, name, rects, n, open }
}

/// MENU_QUERY: the bar of `hwnd` or the popup level `hwnd` shows, into
/// `out`; the item count, or -1 for neither.
fn describe(hwnd: Hwnd, out: &mut [u8]) -> isize {
    let d = if let Some(l) = level_of(hwnd) {
        describe_level(l)
    } else {
        let menu = menu::GetMenu(hwnd);
        if menu == 0 || window(hwnd).is_none_or(|w| w.parent != 0) {
            return -1;
        }
        let mut rects = [[0; 4]; MAX_DESCRIBED];
        let n = bar_rects(hwnd, menu, &mut rects);
        let open = if BUSY.load(Ordering::Acquire) && st().bar == hwnd && st().depth > 0 {
            lit(&OPEN, &OPEN_ITEM, hwnd)
        } else {
            None
        };
        Described { menu, name: ([0; 64], 0), rects, n, open }
    };
    if out.len() < HEADER + d.n * RECORD {
        return -1;
    }
    put32(out, 0, d.n as u32);
    put32(out, 4, d.name.1 as u32);
    put_text(out, 8, &d.name.0[..d.name.1]);
    for k in 0..d.n {
        let Some(v) = menu::view(d.menu, k) else { continue };
        let at = HEADER + k * RECORD;
        let flags = if v.submenu != 0 { FLAG_SUBMENU } else { 0 }
            | if v.disabled { FLAG_DISABLED } else { 0 }
            | if v.separator { FLAG_SEPARATOR } else { 0 }
            | if v.checked { FLAG_CHECKED } else { 0 }
            | if d.open == Some(k) { FLAG_OPEN } else { 0 };
        put32(out, at, v.id);
        put32(out, at + 4, flags);
        for (j, &c) in d.rects[k].iter().enumerate() {
            put32(out, at + 8 + 4 * j, c as u32);
        }
        let len = v.len.min(64);
        put32(out, at + 24, len as u32);
        put_text(out, at + 28, &v.text[..len]);
    }
    d.n as isize
}

/// Wakes the menu loop (its GetMessage) so it sees `done`.
fn wake_loop() {
    crate::window::PostMessageW(st().owner, WM_NULL, 0, 0);
}

/// GALA_MENU for window `hwnd` (a window with a menu bar, or a popup of
/// the running loop): what the operation gave (0: it could not).
pub(crate) fn menu_message(hwnd: Hwnd, wparam: usize, lparam: isize) -> isize {
    let (op, k) = (wparam & 0xff, wparam >> 8);
    if op == MENU_QUERY {
        if lparam == 0 {
            return -1;
        }
        // SAFETY: the caller's buffer of MENU_INFO_SIZE bytes (marshaled
        // into ours for another process).
        let out = unsafe { core::slice::from_raw_parts_mut(lparam as *mut u8, MENU_INFO_SIZE) };
        return describe(hwnd, out);
    }
    if let Some(l) = level_of(hwnd) {
        let menu = st().levels[l].menu;
        let Some(v) = menu::view(menu, k) else { return 0 };
        if v.separator || v.disabled {
            return 0;
        }
        match op {
            MENU_EXPAND if v.submenu != 0 => hover(l, Some(k), true),
            MENU_INVOKE if v.submenu == 0 => {
                hover(l, Some(k), false);
                activate(l, k, false);
                wake_loop();
            }
            MENU_COLLAPSE if v.submenu != 0 => {
                if st().levels[l].sel == Some(k) && st().depth > l + 1 {
                    close_from(l + 1);
                }
            }
            _ => return 0,
        }
        return 1;
    }
    let menu = menu::GetMenu(hwnd);
    let Some(v) = menu::view(menu, k) else { return 0 };
    if v.separator || v.disabled || window(hwnd).is_none_or(|w| w.parent != 0) {
        return 0;
    }
    let tracking = BUSY.load(Ordering::Acquire) && st().bar == hwnd;
    match op {
        MENU_EXPAND if v.submenu != 0 => {
            if tracking {
                open_bar(k);
            } else {
                // As a click on the item does: the menu loop starts when
                // the window handles it (not inside this message).
                let mut rects = [[0; 4]; MAX_DESCRIBED];
                let n = bar_rects(hwnd, menu, &mut rects);
                if k >= n {
                    return 0;
                }
                let r = rects[k];
                let (x, y) = ((r[0] + r[2]) / 2, (r[1] + r[3]) / 2);
                let pt = ((y as i16 as u16 as isize) << 16) | x as i16 as u16 as isize;
                crate::window::PostMessageW(hwnd, WM_NCLBUTTONDOWN, HTMENU, pt);
            }
        }
        MENU_INVOKE if v.submenu == 0 => {
            if tracking {
                choose(v.id);
                wake_loop();
            } else {
                crate::window::PostMessageW(hwnd, WM_COMMAND, v.id as usize & 0xffff, 0);
            }
        }
        MENU_COLLAPSE if v.submenu != 0 => {
            if tracking && st().bar_sel == Some(k) {
                st().done = true;
                wake_loop();
            }
        }
        _ => return 0,
    }
    1
}

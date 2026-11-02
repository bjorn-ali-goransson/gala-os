//! COMBOBOX: a list of strings (with data) and a selection, shown as a
//! Windows 11 box with a chevron; CBS_DROPDOWN and CBS_SIMPLE put an EDIT
//! child (id 1001) in it, whose text is the combo box's. The drop-down
//! list is a ComboLBox window (GetComboBoxInfo's hwndList), a frameless
//! desktop popup shown under the combo box (above it when there is no
//! room below) by a press on the box, F4, Alt+Down or CB_SHOWDROPDOWN:
//! CBN_DROPDOWN first, then the list holds the desktop's mouse capture
//! while it shows, so a press outside closes it (CBN_SELENDCANCEL,
//! CBN_CLOSEUP) and the pointer lights the item under it; a click on an
//! item selects it (CBN_SELENDOK, CBN_CLOSEUP, CBN_SELCHANGE, in Wine's
//! order; Windows documents the last two's order as unpredictable). The
//! arrows move the selection (CBN_SELCHANGE) whether it shows or not,
//! Enter closes it keeping the selection, Escape restores the one it
//! dropped with. Not a modal loop: the program's own message loop
//! delivers the list's input, as on Windows. GetComboBoxInfo names the
//! parts.

use crate::controls::{default_font, fill, rgb, round_fill, select as select_object, transparent};
use crate::window::{DefWindowProcW, Hwnd, SendMessageW};
use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, Ordering};

const WM_CREATE: u32 = 0x0001;
const WM_DESTROY: u32 = 0x0002;
const WM_SIZE: u32 = 0x0005;
const WM_SETFOCUS: u32 = 0x0007;
const WM_KILLFOCUS: u32 = 0x0008;
const WM_SETTEXT: u32 = 0x000c;
const WM_GETTEXT: u32 = 0x000d;
const WM_GETTEXTLENGTH: u32 = 0x000e;
const WM_PAINT: u32 = 0x000f;
const WM_ERASEBKGND: u32 = 0x0014;
const WM_SETFONT: u32 = 0x0030;
const WM_GETFONT: u32 = 0x0031;
const WM_KEYDOWN: u32 = 0x0100;
const WM_SYSKEYDOWN: u32 = 0x0104;
const WM_COMMAND: u32 = 0x0111;
const WM_MOUSEMOVE: u32 = 0x0200;
const WM_LBUTTONDOWN: u32 = 0x0201;
const WM_LBUTTONUP: u32 = 0x0202;
const WM_MOUSEWHEEL: u32 = 0x020a;
const WM_MOUSEACTIVATE: u32 = 0x0021;
const WM_GETDLGCODE: u32 = 0x0087;
const WM_CTLCOLOREDIT: u32 = 0x0133;

const CB_GETEDITSEL: u32 = 0x0140;
const CB_LIMITTEXT: u32 = 0x0141;
const CB_SETEDITSEL: u32 = 0x0142;
const CB_ADDSTRING: u32 = 0x0143;
const CB_DELETESTRING: u32 = 0x0144;
const CB_GETCOUNT: u32 = 0x0146;
const CB_GETCURSEL: u32 = 0x0147;
const CB_GETLBTEXT: u32 = 0x0148;
const CB_GETLBTEXTLEN: u32 = 0x0149;
const CB_INSERTSTRING: u32 = 0x014a;
const CB_RESETCONTENT: u32 = 0x014b;
const CB_FINDSTRING: u32 = 0x014c;
const CB_SELECTSTRING: u32 = 0x014d;
const CB_SETCURSEL: u32 = 0x014e;
const CB_SHOWDROPDOWN: u32 = 0x014f;
const CB_GETITEMDATA: u32 = 0x0150;
const CB_SETITEMDATA: u32 = 0x0151;
const CB_GETDROPPEDCONTROLRECT: u32 = 0x0152;
const CB_SETITEMHEIGHT: u32 = 0x0153;
const CB_GETITEMHEIGHT: u32 = 0x0154;
const CB_SETEXTENDEDUI: u32 = 0x0155;
const CB_GETEXTENDEDUI: u32 = 0x0156;
const CB_GETDROPPEDSTATE: u32 = 0x0157;
const CB_FINDSTRINGEXACT: u32 = 0x0158;
const CB_SETDROPPEDWIDTH: u32 = 0x0160;
const CB_GETDROPPEDWIDTH: u32 = 0x015f;
const CB_INITSTORAGE: u32 = 0x0161;
const CB_GETCOMBOBOXINFO: u32 = 0x0164;
const CB_SETMINVISIBLE: u32 = 0x1701;
const CB_GETMINVISIBLE: u32 = 0x1702;
const CB_SETCUEBANNER: u32 = 0x1703;
const CB_ERR: isize = -1;
const CBS_SIMPLE: u32 = 0x1;
const CBS_DROPDOWNLIST: u32 = 0x3;
const CBS_SORT: u32 = 0x100;
const CBN_SELCHANGE: u32 = 1;
const CBN_SETFOCUS: u32 = 3;
const CBN_KILLFOCUS: u32 = 4;
const CBN_EDITCHANGE: u32 = 5;
const CBN_EDITUPDATE: u32 = 6;
const CBN_DROPDOWN: u32 = 7;
const CBN_CLOSEUP: u32 = 8;
const CBN_SELENDOK: u32 = 9;
const CBN_SELENDCANCEL: u32 = 10;
const EN_CHANGE: u32 = 0x0300;
const EN_UPDATE: u32 = 0x0400;
const EDIT_ID: usize = 1001;

const VK_RETURN: usize = 0x0d;
const VK_ESCAPE: usize = 0x1b;
const VK_PRIOR: usize = 0x21;
const VK_NEXT: usize = 0x22;
const VK_END: usize = 0x23;
const VK_HOME: usize = 0x24;
const VK_LEFT: usize = 0x25;
const VK_UP: usize = 0x26;
const VK_RIGHT: usize = 0x27;
const VK_DOWN: usize = 0x28;
const VK_F4: usize = 0x73;

/// The list's item height, inner padding, the width of the combo box's
/// button (the chevron's part), and the default CB_SETMINVISIBLE (30, as
/// documented).
const ITEM_H: i32 = 24;
const PAD: i32 = 3;
const BUTTON_W: i32 = 28;
const MIN_VISIBLE: usize = 30;

const LIST_BACK: u32 = rgb(0xf9, 0xf9, 0xf9);
const LIST_BORDER: u32 = rgb(0xc8, 0xc8, 0xc8);
const LIST_HOT: u32 = rgb(0xe6, 0xe8, 0xeb);
const LIST_SELECTED: u32 = rgb(0xee, 0xef, 0xf1);
const ACCENT: u32 = rgb(0x00, 0x5f, 0xb8);
const THUMB: u32 = rgb(0x8a, 0x8a, 0x8a);

struct Combo {
    hwnd: Hwnd,
    items: Vec<(Vec<u16>, usize)>,
    current: isize,
    edit: Hwnd,
    list: Hwnd,
    dropped: bool,
    font: usize,
    /// While dropped: the item the pointer or the keys light, the first
    /// item shown, how many show, and the selection it dropped with.
    hot: isize,
    top: isize,
    visible: isize,
    saved: isize,
    /// CB_SETMINVISIBLE, CB_SETDROPPEDWIDTH (0: the combo box's width).
    min_visible: usize,
    dropped_width: i32,
}

struct Table {
    lock: AtomicBool,
    combos: UnsafeCell<Vec<Combo>>,
}

// SAFETY: `combos` is only touched under `lock`.
unsafe impl Sync for Table {}

static TABLE: Table = Table { lock: AtomicBool::new(false), combos: UnsafeCell::new(Vec::new()) };

/// Runs `f` on the combo box's state (made the first time). The lock is
/// held: `f` must not send messages.
fn with<R>(hwnd: Hwnd, f: impl FnOnce(&mut Combo) -> R) -> Option<R> {
    while TABLE.lock.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        core::hint::spin_loop();
    }
    // SAFETY: under the lock.
    let combos = unsafe { &mut *TABLE.combos.get() };
    let i = match combos.iter().position(|c| c.hwnd == hwnd) {
        Some(i) => Some(i),
        None if combos.try_reserve(1).is_ok() => {
            combos.push(Combo {
                hwnd,
                items: Vec::new(),
                current: -1,
                edit: 0,
                list: 0,
                dropped: false,
                font: 0,
                hot: -1,
                top: 0,
                visible: 0,
                saved: -1,
                min_visible: MIN_VISIBLE,
                dropped_width: 0,
            });
            Some(combos.len() - 1)
        }
        None => None,
    };
    let r = i.map(|i| f(&mut combos[i]));
    TABLE.lock.store(false, Ordering::Release);
    r
}

/// The combo box whose list (or edit, with `edit`) is `hwnd`, if any.
fn owner_of(hwnd: Hwnd, edit: bool) -> Option<Hwnd> {
    if hwnd == 0 {
        return None;
    }
    while TABLE.lock.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        core::hint::spin_loop();
    }
    // SAFETY: under the lock.
    let combos = unsafe { &*TABLE.combos.get() };
    let r =
        combos.iter().find(|c| if edit { c.edit == hwnd } else { c.list == hwnd }).map(|c| c.hwnd);
    TABLE.lock.store(false, Ordering::Release);
    r
}

fn forget(hwnd: Hwnd) {
    while TABLE.lock.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        core::hint::spin_loop();
    }
    // SAFETY: under the lock.
    unsafe { (*TABLE.combos.get()).retain(|c| c.hwnd != hwnd) };
    TABLE.lock.store(false, Ordering::Release);
}

fn style(hwnd: Hwnd) -> u32 {
    crate::tree::GetWindowLongW(hwnd, -16) as u32
}

fn kind(hwnd: Hwnd) -> u32 {
    style(hwnd) & 0x3
}

fn notify(hwnd: Hwnd, code: u32) {
    let parent = crate::tree::GetParent(hwnd);
    if parent != 0 {
        let id = crate::tree::GetWindowLongPtrW(hwnd, -12) as usize & 0xffff;
        SendMessageW(parent, WM_COMMAND, id | (code as usize) << 16, hwnd as isize);
    }
}

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

fn find(hwnd: Hwnd, start: isize, want: &[u16], exact: bool) -> isize {
    with(hwnd, |c| {
        let n = c.items.len();
        let first = if start < 0 || start as usize >= n { 0 } else { start as usize + 1 };
        for k in 0..n {
            let i = (first + k) % n;
            let t = &c.items[i].0;
            let hit = if exact {
                t.len() == want.len() && t.iter().zip(want).all(|(&a, &b)| lower(a) == lower(b))
            } else {
                t.len() >= want.len() && t.iter().zip(want).all(|(&a, &b)| lower(a) == lower(b))
            };
            if hit {
                return i as isize;
            }
        }
        CB_ERR
    })
    .unwrap_or(CB_ERR)
}

/// Selects item `i` (-1: none); the edit shows its text.
fn select(hwnd: Hwnd, i: isize) -> isize {
    let (ok, edit, text) = with(hwnd, |c| {
        if i < -1 || i >= c.items.len() as isize {
            c.current = -1;
            return (false, c.edit, Vec::new());
        }
        c.current = i;
        let text = if i >= 0 { c.items[i as usize].0.clone() } else { Vec::new() };
        (true, c.edit, text)
    })
    .unwrap_or((false, 0, Vec::new()));
    if edit != 0 && ok {
        let mut z = text;
        if z.try_reserve(1).is_ok() {
            z.push(0);
            SendMessageW(edit, WM_SETTEXT, 0, z.as_ptr() as isize);
        }
    }
    crate::window::InvalidateRect(hwnd, core::ptr::null(), 1);
    if ok { i } else { CB_ERR }
}

/// A user's move to item `i` (CBN_SELCHANGE when it changes); a list
/// that shows lights it and scrolls to it.
fn step_to(hwnd: Hwnd, i: isize) {
    let (n, cur) = with(hwnd, |c| (c.items.len() as isize, c.current)).unwrap_or((0, -1));
    if n == 0 {
        return;
    }
    let i = i.clamp(0, n - 1);
    light(hwnd, i);
    if i != cur {
        select(hwnd, i);
        notify(hwnd, CBN_SELCHANGE);
    }
}

/// While the list shows: item `i` is the lit one, scrolled into view.
fn light(hwnd: Hwnd, i: isize) {
    let list = with(hwnd, |c| {
        if !c.dropped || c.hot == i && i >= c.top && i < c.top + c.visible {
            return 0;
        }
        c.hot = i;
        if i >= 0 {
            if i < c.top {
                c.top = i;
            } else if i >= c.top + c.visible {
                c.top = i - c.visible + 1;
            }
        }
        c.list
    })
    .unwrap_or(0);
    if list != 0 {
        crate::window::InvalidateRect(list, core::ptr::null(), 0);
    }
}

/// Drops the list down (CB_SHOWDROPDOWN, a press, F4, Alt+Down):
/// CBN_DROPDOWN first (the program may fill the list then), then the
/// popup under the combo box (above it when there is more room there),
/// as wide as the combo box (or CB_SETDROPPEDWIDTH), with up to
/// CB_SETMINVISIBLE items, holding the mouse capture.
fn drop_down(hwnd: Hwnd) {
    if kind(hwnd) == CBS_SIMPLE || with(hwnd, |c| c.dropped).unwrap_or(true) {
        return;
    }
    notify(hwnd, CBN_DROPDOWN);
    let Some((list, n, wanted, dropped_width)) =
        with(hwnd, |c| (c.list, c.items.len() as i32, c.min_visible as i32, c.dropped_width))
    else {
        return;
    };
    if list == 0 || crate::window::IsWindow(list) == 0 || crate::window::IsWindow(hwnd) == 0 {
        return;
    }
    let mut r = [0i32; 4];
    crate::tree::GetWindowRect(hwnd, &mut r);
    let (_, work) = crate::monitor::screen();
    let full = n.clamp(1, wanted.max(1)) * ITEM_H + 2 * PAD;
    let (below, above) = (work[3] - r[3], r[1] - work[1]);
    let (y, h) = if full <= below || below >= above {
        (r[3], full.min(below))
    } else {
        let h = full.min(above);
        (r[1] - h, h)
    };
    let visible = ((h - 2 * PAD) / ITEM_H).max(1);
    let h = visible * ITEM_H + 2 * PAD;
    let w = (r[2] - r[0]).max(dropped_width);
    let x = r[0].min(work[2] - w).max(work[0]);
    with(hwnd, |c| {
        c.dropped = true;
        c.saved = c.current;
        c.hot = c.current;
        c.visible = visible as isize;
        let n = c.items.len() as isize;
        c.top = if c.current >= c.visible { c.current - c.visible + 1 } else { 0 };
        c.top = c.top.min((n - c.visible).max(0));
    });
    const SWP_NOACTIVATE: u32 = 0x10;
    crate::tree::SetWindowPos(list, 0, x, y, w, h, SWP_NOACTIVATE);
    crate::window::ShowWindow(list, 4); // SW_SHOWNOACTIVATE
    if let Some(lw) = crate::window::window(list)
        && lw.desktop != 0
    {
        // Every press lands in the list: one outside closes it.
        // SAFETY: a plain call on our popup's desktop window.
        unsafe { crate::window::GalaWindowCapture(lw.desktop) };
    }
    crate::window::InvalidateRect(hwnd, core::ptr::null(), 1);
}

/// Closes the list: CBN_SELENDOK (`ok`) or CBN_SELENDCANCEL, then it goes
/// (and the capture with it), then CBN_CLOSEUP.
fn close_up(hwnd: Hwnd, ok: bool) {
    let Some((true, list)) = with(hwnd, |c| (c.dropped, c.list)) else { return };
    notify(hwnd, if ok { CBN_SELENDOK } else { CBN_SELENDCANCEL });
    let was = with(hwnd, |c| core::mem::replace(&mut c.dropped, false)).unwrap_or(false);
    if !was {
        return; // closed meanwhile (by the notification)
    }
    // SAFETY: a plain call; ends our list's capture.
    unsafe { crate::window::GalaWindowCapture(0) };
    if list != 0 {
        crate::window::ShowWindow(list, 0);
    }
    crate::window::InvalidateRect(hwnd, core::ptr::null(), 1);
    notify(hwnd, CBN_CLOSEUP);
}

/// The user chose item `i` in the list (a click, Enter on a lit item):
/// it is selected, the list closes, CBN_SELCHANGE (Wine's order).
fn pick(hwnd: Hwnd, i: isize) {
    select(hwnd, i);
    close_up(hwnd, true);
    notify(hwnd, CBN_SELCHANGE);
}

/// Escape: the selection the list dropped with comes back, and it closes.
fn cancel(hwnd: Hwnd) {
    let (cur, saved) = with(hwnd, |c| (c.current, c.saved)).unwrap_or((-1, -1));
    if cur != saved && saved >= 0 {
        select(hwnd, saved);
        notify(hwnd, CBN_SELCHANGE);
    }
    close_up(hwnd, false);
}

fn toggle(hwnd: Hwnd) {
    if with(hwnd, |c| c.dropped).unwrap_or(false) {
        close_up(hwnd, true);
    } else {
        drop_down(hwnd);
    }
}

/// A key for the combo box (or its edit, `from_edit`; `alt`: with Alt
/// held): whether it was the combo box's.
fn key(hwnd: Hwnd, vk: usize, alt: bool, from_edit: bool) -> bool {
    let (cur, hot, dropped, visible) =
        with(hwnd, |c| (c.current, c.hot, c.dropped, c.visible)).unwrap_or((-1, -1, false, 0));
    let page = if dropped { (visible - 1).max(1) } else { 8 };
    match vk {
        VK_F4 if !alt => toggle(hwnd),
        VK_UP | VK_DOWN if alt => toggle(hwnd),
        _ if alt => return false,
        VK_RETURN if dropped => {
            if hot >= 0 && hot != cur {
                pick(hwnd, hot);
            } else {
                close_up(hwnd, true);
            }
        }
        VK_ESCAPE if dropped => cancel(hwnd),
        VK_UP => step_to(hwnd, if cur < 0 { 0 } else { cur - 1 }),
        VK_DOWN => step_to(hwnd, cur + 1),
        VK_PRIOR => step_to(hwnd, cur - page),
        VK_NEXT => step_to(hwnd, cur.max(0) + page),
        VK_LEFT if !from_edit => step_to(hwnd, cur - 1),
        VK_RIGHT if !from_edit => step_to(hwnd, cur + 1),
        VK_HOME if !from_edit => step_to(hwnd, 0),
        VK_END if !from_edit => step_to(hwnd, isize::MAX / 2),
        _ => return false,
    }
    true
}

/// A key down in edit `edit` (`alt`: WM_SYSKEYDOWN): a combo box's edit
/// passes F4, Alt+Up/Down, the vertical arrows and pages, and Enter and
/// Escape while the list shows, to its combo box. Whether it did.
pub(crate) fn edit_key(edit: Hwnd, vk: usize, alt: bool) -> bool {
    match owner_of(edit, true) {
        Some(combo) => key(combo, vk, alt, true),
        None => false,
    }
}

/// Whether `edit` is the edit of a combo box whose list shows (it then
/// wants every key: Enter and Escape are the list's).
pub(crate) fn edit_dropped(edit: Hwnd) -> bool {
    owner_of(edit, true).is_some_and(|combo| with(combo, |c| c.dropped).unwrap_or(false))
}

fn layout(hwnd: Hwnd) {
    let edit = with(hwnd, |c| c.edit).unwrap_or(0);
    if edit != 0 {
        let mut r = [0i32; 4];
        crate::tree::GetClientRect(hwnd, &mut r);
        crate::tree::SetWindowPos(edit, 0, 2, 2, (r[2] - BUTTON_W).max(1), (r[3] - 4).max(1), 0x14);
    }
}

pub extern "system" fn combo_proc(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize {
    match msg {
        WM_CREATE => {
            let k = kind(hwnd);
            let mut edit = 0;
            if k != CBS_DROPDOWNLIST {
                let class: [u16; 5] = [b'E' as u16, b'D' as u16, b'I' as u16, b'T' as u16, 0];
                // WS_CHILD | WS_VISIBLE | ES_AUTOHSCROLL.
                edit = crate::window::CreateWindowExW(
                    0,
                    class.as_ptr(),
                    core::ptr::null(),
                    0x5000_0080,
                    2,
                    2,
                    10,
                    10,
                    hwnd,
                    EDIT_ID as crate::window::Handle,
                    core::ptr::null_mut(),
                    core::ptr::null_mut(),
                );
            }
            let class: [u16; 10] =
                core::array::from_fn(|i| u16::from(*b"ComboLBox\0".get(i).unwrap_or(&0)));
            // WS_POPUP | WS_BORDER, hidden.
            let list = crate::window::CreateWindowExW(
                0,
                class.as_ptr(),
                core::ptr::null(),
                0x8080_0000,
                0,
                0,
                10,
                10,
                hwnd,
                core::ptr::null_mut(),
                core::ptr::null_mut(),
                core::ptr::null_mut(),
            );
            with(hwnd, |c| {
                c.edit = edit;
                c.list = list;
            });
            layout(hwnd);
            0
        }
        WM_DESTROY => {
            let (list, dropped) = with(hwnd, |c| (c.list, c.dropped)).unwrap_or((0, false));
            if dropped {
                // SAFETY: a plain call; ends our list's capture.
                unsafe { crate::window::GalaWindowCapture(0) };
            }
            if list != 0 {
                crate::window::DestroyWindow(list);
            }
            forget(hwnd);
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_SIZE => {
            layout(hwnd);
            0
        }
        WM_SETFONT => {
            let edit = with(hwnd, |c| {
                c.font = wparam;
                c.edit
            })
            .unwrap_or(0);
            if edit != 0 {
                SendMessageW(edit, WM_SETFONT, wparam, lparam);
            }
            crate::window::InvalidateRect(hwnd, core::ptr::null(), 1);
            0
        }
        WM_GETFONT => with(hwnd, |c| c.font as isize).unwrap_or(0),
        WM_ERASEBKGND => 1,
        WM_SETTEXT | WM_GETTEXT | WM_GETTEXTLENGTH => {
            let (edit, current) = with(hwnd, |c| (c.edit, c.current)).unwrap_or((0, -1));
            if edit != 0 {
                return SendMessageW(edit, msg, wparam, lparam);
            }
            // A drop-down list: the selected item's text; WM_SETTEXT
            // selects the item with that text.
            match msg {
                WM_SETTEXT => {
                    let want = wide(lparam);
                    let i = find(hwnd, -1, &want, true);
                    if i >= 0 {
                        select(hwnd, i);
                    }
                    isize::from(i >= 0)
                }
                _ => {
                    let text = with(hwnd, |c| {
                        c.items
                            .get(current.max(0) as usize)
                            .filter(|_| current >= 0)
                            .map(|t| t.0.clone())
                    })
                    .flatten()
                    .unwrap_or_default();
                    if msg == WM_GETTEXTLENGTH {
                        return text.len() as isize;
                    }
                    if wparam == 0 || lparam == 0 {
                        return 0;
                    }
                    let n = text.len().min(wparam - 1);
                    // SAFETY: the caller's buffer of `wparam` characters.
                    unsafe {
                        core::ptr::copy_nonoverlapping(text.as_ptr(), lparam as *mut u16, n);
                        *(lparam as *mut u16).add(n) = 0;
                    }
                    n as isize
                }
            }
        }
        CB_ADDSTRING | CB_INSERTSTRING => {
            let text = wide(lparam);
            let sorted = style(hwnd) & CBS_SORT != 0 && msg == CB_ADDSTRING;
            let r = with(hwnd, |c| {
                if c.items.try_reserve(1).is_err() {
                    return -2; // CB_ERRSPACE
                }
                let at = if sorted {
                    c.items
                        .iter()
                        .position(|(t, _)| {
                            t.iter().map(|&x| lower(x)).cmp(text.iter().map(|&x| lower(x))).is_gt()
                        })
                        .unwrap_or(c.items.len())
                } else if msg == CB_INSERTSTRING && (wparam as isize) >= 0 {
                    if wparam > c.items.len() {
                        return CB_ERR;
                    }
                    wparam
                } else {
                    c.items.len()
                };
                c.items.insert(at, (text, 0));
                if c.current >= at as isize {
                    c.current += 1;
                }
                at as isize
            })
            .unwrap_or(-2);
            crate::window::InvalidateRect(hwnd, core::ptr::null(), 1);
            r
        }
        CB_DELETESTRING => {
            let r = with(hwnd, |c| {
                if wparam >= c.items.len() {
                    return CB_ERR;
                }
                c.items.remove(wparam);
                if c.current == wparam as isize {
                    c.current = -1;
                } else if c.current > wparam as isize {
                    c.current -= 1;
                }
                c.items.len() as isize
            })
            .unwrap_or(CB_ERR);
            crate::window::InvalidateRect(hwnd, core::ptr::null(), 1);
            r
        }
        CB_RESETCONTENT => {
            with(hwnd, |c| {
                c.items.clear();
                c.current = -1;
            });
            crate::window::InvalidateRect(hwnd, core::ptr::null(), 1);
            0 // CB_OKAY
        }
        CB_GETCOUNT => with(hwnd, |c| c.items.len() as isize).unwrap_or(0),
        CB_GETCURSEL => with(hwnd, |c| c.current).unwrap_or(CB_ERR),
        CB_SETCURSEL => select(hwnd, wparam as isize),
        CB_GETLBTEXTLEN => {
            with(hwnd, |c| c.items.get(wparam).map_or(CB_ERR, |t| t.0.len() as isize))
                .unwrap_or(CB_ERR)
        }
        CB_GETLBTEXT => {
            let Some(Some(text)) = with(hwnd, |c| c.items.get(wparam).map(|t| t.0.clone())) else {
                return CB_ERR;
            };
            if lparam != 0 {
                // SAFETY: the caller's buffer (CB_GETLBTEXTLEN + 1 long).
                unsafe {
                    core::ptr::copy_nonoverlapping(text.as_ptr(), lparam as *mut u16, text.len());
                    *(lparam as *mut u16).add(text.len()) = 0;
                }
            }
            text.len() as isize
        }
        CB_FINDSTRING => find(hwnd, wparam as isize, &wide(lparam), false),
        CB_FINDSTRINGEXACT => find(hwnd, wparam as isize, &wide(lparam), true),
        CB_SELECTSTRING => {
            let i = find(hwnd, wparam as isize, &wide(lparam), false);
            if i >= 0 { select(hwnd, i) } else { CB_ERR }
        }
        CB_GETITEMDATA => {
            with(hwnd, |c| c.items.get(wparam).map_or(CB_ERR, |t| t.1 as isize)).unwrap_or(CB_ERR)
        }
        CB_SETITEMDATA => with(hwnd, |c| match c.items.get_mut(wparam) {
            Some(t) => {
                t.1 = lparam as usize;
                1
            }
            None => CB_ERR,
        })
        .unwrap_or(CB_ERR),
        CB_SHOWDROPDOWN => {
            if wparam != 0 {
                drop_down(hwnd);
            } else {
                close_up(hwnd, false);
            }
            1
        }
        CB_GETDROPPEDSTATE => with(hwnd, |c| isize::from(c.dropped)).unwrap_or(0),
        CB_GETEDITSEL | CB_SETEDITSEL | CB_LIMITTEXT => {
            let edit = with(hwnd, |c| c.edit).unwrap_or(0);
            if edit == 0 {
                return CB_ERR;
            }
            let m = match msg {
                CB_GETEDITSEL => 0x00b0, // EM_GETSEL
                CB_SETEDITSEL => {
                    return SendMessageW(edit, 0x00b1, (lparam & 0xffff) as usize, lparam >> 16);
                }
                _ => 0x00c5, // EM_LIMITTEXT
            };
            SendMessageW(edit, m, wparam, lparam)
        }
        CB_GETDROPPEDCONTROLRECT => {
            if lparam != 0 {
                let mut r = [0i32; 4];
                crate::tree::GetWindowRect(hwnd, &mut r);
                // SAFETY: the caller's RECT.
                unsafe { *(lparam as *mut [i32; 4]) = r };
            }
            1
        }
        // -1: the selection field; else the list's items.
        CB_GETITEMHEIGHT => {
            if wparam as isize == -1 {
                20
            } else {
                ITEM_H as isize
            }
        }
        CB_SETITEMHEIGHT | CB_SETEXTENDEDUI | CB_INITSTORAGE | CB_SETCUEBANNER => 0,
        CB_SETMINVISIBLE => {
            if (wparam as isize) <= 0 {
                return 0;
            }
            with(hwnd, |c| c.min_visible = wparam);
            1
        }
        CB_GETMINVISIBLE => with(hwnd, |c| c.min_visible as isize).unwrap_or(MIN_VISIBLE as isize),
        CB_GETEXTENDEDUI => 0,
        CB_SETDROPPEDWIDTH | CB_GETDROPPEDWIDTH => {
            if msg == CB_SETDROPPEDWIDTH {
                with(hwnd, |c| c.dropped_width = wparam as i32);
            }
            let mut r = [0i32; 4];
            crate::tree::GetWindowRect(hwnd, &mut r);
            let set = with(hwnd, |c| c.dropped_width).unwrap_or(0);
            (r[2] - r[0]).max(set) as isize
        }
        CB_GETCOMBOBOXINFO => GetComboBoxInfo(hwnd, lparam as *mut ComboBoxInfo) as isize,
        WM_COMMAND => {
            // The edit's changes: CBN_EDITUPDATE, CBN_EDITCHANGE.
            match (wparam >> 16) as u32 {
                EN_UPDATE => notify(hwnd, CBN_EDITUPDATE),
                EN_CHANGE => notify(hwnd, CBN_EDITCHANGE),
                _ => {}
            }
            0
        }
        WM_LBUTTONDOWN => {
            crate::tree::SetFocus(hwnd);
            let mut r = [0i32; 4];
            crate::tree::GetClientRect(hwnd, &mut r);
            if kind(hwnd) == CBS_DROPDOWNLIST || (lparam as i16 as i32) >= r[2] - BUTTON_W {
                toggle(hwnd);
            }
            0
        }
        WM_KEYDOWN => {
            key(hwnd, wparam, false, false);
            0
        }
        WM_SYSKEYDOWN => {
            if key(hwnd, wparam, true, false) {
                return 0;
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_MOUSEWHEEL => {
            let cur = with(hwnd, |c| c.current).unwrap_or(-1);
            let delta = (wparam >> 16) as i16;
            step_to(hwnd, if delta > 0 { cur - 1 } else { cur + 1 });
            0
        }
        WM_SETFOCUS => {
            let edit = with(hwnd, |c| c.edit).unwrap_or(0);
            if edit != 0 {
                crate::tree::SetFocus(edit);
            }
            notify(hwnd, CBN_SETFOCUS);
            crate::window::InvalidateRect(hwnd, core::ptr::null(), 1);
            0
        }
        WM_KILLFOCUS => {
            // The focus leaving for another window (not the edit) closes
            // the list.
            let edit = with(hwnd, |c| c.edit).unwrap_or(0);
            if wparam != edit || edit == 0 {
                close_up(hwnd, false);
            }
            notify(hwnd, CBN_KILLFOCUS);
            crate::window::InvalidateRect(hwnd, core::ptr::null(), 1);
            0
        }
        // DLGC_WANTARROWS | DLGC_WANTCHARS, and every key while the list
        // shows (Enter and Escape are the list's).
        WM_GETDLGCODE => {
            0x1 | 0x80 | if with(hwnd, |c| c.dropped).unwrap_or(false) { 0x4 } else { 0 }
        }
        WM_PAINT => {
            paint(hwnd);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

fn paint(hwnd: Hwnd) {
    let (font, current, edit, text) = with(hwnd, |c| {
        let t = c
            .items
            .get(c.current.max(0) as usize)
            .filter(|_| c.current >= 0)
            .map(|t| t.0.clone())
            .unwrap_or_default();
        (c.font, c.current, c.edit, t)
    })
    .unwrap_or((0, -1, 0, Vec::new()));
    let _ = current;
    let focused = crate::tree::GetFocus() == hwnd;
    crate::controls::paint_box(hwnd, font, |hdc, r| {
        let parent = crate::tree::GetParent(hwnd);
        let brush = if parent == 0 {
            0
        } else {
            SendMessageW(parent, WM_CTLCOLOREDIT, hdc as usize, hwnd as isize)
        };
        let brush = if brush != 0 {
            brush as crate::window::Handle
        } else {
            crate::window::GetSysColorBrush(5)
        };
        crate::window::FillRect(hdc, &r, brush);
        crate::controls::combo_face(hdc, r, focused);
        if edit == 0 {
            crate::controls::draw_label(hdc, &text, [r[0] + 10, r[1], r[2] - 30, r[3]]);
        }
        crate::controls::chevron(hdc, r[2] - 18, (r[1] + r[3]) / 2);
    });
}

/// The drop-down list's window (ComboLBox): draws the combo box's items
/// and takes the mouse while it shows (it has the capture: positions
/// outside it come too).
pub extern "system" fn list_proc(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize {
    match msg {
        WM_PAINT => {
            match owner_of(hwnd, false) {
                Some(combo) => paint_list(hwnd, combo),
                None => {
                    DefWindowProcW(hwnd, msg, wparam, lparam);
                }
            }
            0
        }
        WM_ERASEBKGND => 1,
        WM_MOUSEACTIVATE => 3, // MA_NOACTIVATE
        WM_MOUSEMOVE | WM_LBUTTONDOWN | WM_LBUTTONUP | WM_MOUSEWHEEL => {
            if let Some(combo) = owner_of(hwnd, false) {
                list_mouse(hwnd, combo, msg, wparam, lparam);
            }
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// The list's mouse input: the pointer lights the item under it, a
/// release on an item picks it, a press outside closes the list, the
/// wheel scrolls.
fn list_mouse(list: Hwnd, combo: Hwnd, msg: u32, wparam: usize, lparam: isize) {
    let Some((true, n, top, visible)) =
        with(combo, |c| (c.dropped, c.items.len() as isize, c.top, c.visible))
    else {
        return;
    };
    let (x, y) = (lparam as i16 as i32, (lparam >> 16) as i16 as i32);
    let mut r = [0i32; 4];
    crate::tree::GetClientRect(list, &mut r);
    let inside = x >= 0 && y >= 0 && x < r[2] && y < r[3];
    let row = (y - PAD).div_euclid(ITEM_H) as isize;
    let item = (inside && y >= PAD && row < visible && top + row < n).then_some(top + row);
    match msg {
        WM_MOUSEMOVE => {
            if let Some(k) = item {
                light(combo, k);
            }
        }
        WM_LBUTTONDOWN => match item {
            Some(k) => light(combo, k),
            None if !inside => close_up(combo, false),
            None => {}
        },
        WM_LBUTTONUP => {
            if let Some(k) = item {
                pick(combo, k);
            }
        }
        _ => {
            // WM_MOUSEWHEEL: three items a notch.
            let delta = (wparam >> 16) as i16;
            let by = if delta > 0 { -3 } else { 3 };
            let changed = with(combo, |c| {
                let max = (c.items.len() as isize - c.visible).max(0);
                let t = (c.top + by).clamp(0, max);
                core::mem::replace(&mut c.top, t) != t
            })
            .unwrap_or(false);
            if changed {
                crate::window::InvalidateRect(list, core::ptr::null(), 0);
            }
        }
    }
}

/// Draws the list in the Windows 11 light style: a light box with a 1px
/// border, the lit item as a rounded highlight, the selected one with an
/// accent pill at its left, a thin thumb when not every item shows.
fn paint_list(list: Hwnd, combo: Hwnd) {
    let mut ps = crate::window::PaintStruct::EMPTY;
    let hdc = crate::window::BeginPaint(list, &mut ps);
    if hdc.is_null() {
        return;
    }
    let Some((font, n, top, visible, hot, current)) =
        with(combo, |c| (c.font, c.items.len() as isize, c.top, c.visible, c.hot, c.current))
    else {
        crate::window::EndPaint(list, &ps);
        return;
    };
    let mut r = [0i32; 4];
    crate::tree::GetClientRect(list, &mut r);
    let (w, h) = (r[2], r[3]);
    let old = select_object(hdc, if font != 0 { font } else { default_font() });
    transparent(hdc);
    fill(hdc, 0, 0, w, h, LIST_BORDER);
    fill(hdc, 1, 1, w - 1, h - 1, LIST_BACK);
    let scrolls = n > visible;
    let right = if scrolls { w - 10 } else { w - PAD - 1 };
    let mut text = [0u16; 256];
    for row in 0..visible {
        let k = top + row;
        if k >= n {
            break;
        }
        let len = with(combo, |c| {
            c.items.get(k as usize).map_or(0, |t| {
                let len = t.0.len().min(text.len());
                text[..len].copy_from_slice(&t.0[..len]);
                len
            })
        })
        .unwrap_or(0);
        let y = PAD + row as i32 * ITEM_H;
        let left = PAD + 1;
        if k == hot {
            round_fill(hdc, [left, y + 1, right, y + ITEM_H - 1], 4, LIST_HOT);
        } else if k == current {
            round_fill(hdc, [left, y + 1, right, y + ITEM_H - 1], 4, LIST_SELECTED);
        }
        if k == current {
            round_fill(hdc, [left, y + 7, left + 3, y + ITEM_H - 7], 1, ACCENT);
        }
        crate::controls::draw_label(hdc, &text[..len], [left + 10, y, right - 4, y + ITEM_H]);
    }
    if scrolls {
        let track = h - 2 * PAD;
        let th = (track * visible as i32 / n as i32).max(16).min(track);
        let span = (n - visible).max(1) as i32;
        let ty = PAD + (track - th) * top as i32 / span;
        round_fill(hdc, [w - 7, ty, w - 4, ty + th], 1, THUMB);
    }
    if old != 0 {
        select_object(hdc, old);
    }
    crate::window::EndPaint(list, &ps);
}

/// COMBOBOXINFO.
#[repr(C)]
pub struct ComboBoxInfo {
    size: u32,
    item: [i32; 4],
    button: [i32; 4],
    button_state: u32,
    combo: Hwnd,
    edit: Hwnd,
    list: Hwnd,
}

/// GetComboBoxInfo(hwnd, info): the combo box's parts: the text area,
/// the button, the edit (the combo box itself for a drop-down list) and
/// the list.
#[unsafe(no_mangle)]
pub extern "system" fn GetComboBoxInfo(hwnd: Hwnd, info: *mut ComboBoxInfo) -> i32 {
    if info.is_null() || crate::window::IsWindow(hwnd) == 0 {
        return 0;
    }
    let Some((edit, list)) = with(hwnd, |c| (c.edit, c.list)) else { return 0 };
    let mut r = [0i32; 4];
    crate::tree::GetClientRect(hwnd, &mut r);
    // SAFETY: the caller's COMBOBOXINFO.
    unsafe {
        (*info).item = [r[0] + 2, r[1] + 2, r[2] - 28, r[3] - 2];
        (*info).button = [r[2] - 28, r[1], r[2], r[3]];
        (*info).button_state = 0;
        (*info).combo = hwnd;
        (*info).edit = if edit != 0 { edit } else { hwnd };
        (*info).list = list;
    }
    1
}

/// Whether `class` (a style kind) is CBS_SIMPLE (always shows its list).
#[allow(dead_code)]
fn simple(hwnd: Hwnd) -> bool {
    kind(hwnd) == CBS_SIMPLE
}

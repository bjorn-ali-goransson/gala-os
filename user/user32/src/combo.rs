//! COMBOBOX: a list of strings (with data) and a selection, shown as a
//! Windows 11 box with a chevron; CBS_DROPDOWN and CBS_SIMPLE put an EDIT
//! child (id 1001) in it, whose text is the combo box's. The drop-down
//! list is a hidden ComboLBox popup (GetComboBoxInfo's hwndList): the
//! desktop cannot show a frameless popup yet, so a click on a drop-down
//! list steps to the next item instead (CBN_SELCHANGE), as do the arrow
//! keys and the wheel. GetComboBoxInfo names the parts.

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
const WM_COMMAND: u32 = 0x0111;
const WM_LBUTTONDOWN: u32 = 0x0201;
const WM_MOUSEWHEEL: u32 = 0x020a;
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
const EN_CHANGE: u32 = 0x0300;
const EN_UPDATE: u32 = 0x0400;
const EDIT_ID: usize = 1001;

struct Combo {
    hwnd: Hwnd,
    items: Vec<(Vec<u16>, usize)>,
    current: isize,
    edit: Hwnd,
    list: Hwnd,
    dropped: bool,
    font: usize,
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
            });
            Some(combos.len() - 1)
        }
        None => None,
    };
    let r = i.map(|i| f(&mut combos[i]));
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

/// A user's move to item `i` (CBN_SELCHANGE when it changes).
fn step_to(hwnd: Hwnd, i: isize) {
    let (n, cur) = with(hwnd, |c| (c.items.len() as isize, c.current)).unwrap_or((0, -1));
    if n == 0 {
        return;
    }
    let i = i.clamp(0, n - 1);
    if i != cur {
        select(hwnd, i);
        notify(hwnd, CBN_SELCHANGE);
    }
}

fn layout(hwnd: Hwnd) {
    let edit = with(hwnd, |c| c.edit).unwrap_or(0);
    if edit != 0 {
        let mut r = [0i32; 4];
        crate::tree::GetClientRect(hwnd, &mut r);
        crate::tree::SetWindowPos(edit, 0, 2, 2, (r[2] - 28).max(1), (r[3] - 4).max(1), 0x14);
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
            let list = with(hwnd, |c| c.list).unwrap_or(0);
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
            let was =
                with(hwnd, |c| core::mem::replace(&mut c.dropped, wparam != 0)).unwrap_or(false);
            if was != (wparam != 0) {
                notify(hwnd, if wparam != 0 { CBN_DROPDOWN } else { CBN_CLOSEUP });
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
        CB_GETITEMHEIGHT => 20,
        CB_SETITEMHEIGHT | CB_SETEXTENDEDUI | CB_SETDROPPEDWIDTH | CB_INITSTORAGE
        | CB_SETMINVISIBLE | CB_SETCUEBANNER => 0,
        CB_GETEXTENDEDUI => 0,
        CB_GETDROPPEDWIDTH => {
            let mut r = [0i32; 4];
            crate::tree::GetClientRect(hwnd, &mut r);
            r[2] as isize
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
            if kind(hwnd) == CBS_DROPDOWNLIST
                || (lparam as i16 as i32) > {
                    let mut r = [0i32; 4];
                    crate::tree::GetClientRect(hwnd, &mut r);
                    r[2] - 28
                }
            {
                // The drop-down cannot show: step to the next item.
                notify(hwnd, CBN_DROPDOWN);
                let (n, cur) =
                    with(hwnd, |c| (c.items.len() as isize, c.current)).unwrap_or((0, -1));
                if n > 0 {
                    step_to(hwnd, (cur + 1) % n);
                }
                notify(hwnd, CBN_CLOSEUP);
            }
            0
        }
        WM_KEYDOWN => {
            let cur = with(hwnd, |c| c.current).unwrap_or(-1);
            match wparam {
                0x26 | 0x25 => step_to(hwnd, cur - 1), // VK_UP, VK_LEFT
                0x28 | 0x27 => step_to(hwnd, cur + 1), // VK_DOWN, VK_RIGHT
                0x24 => step_to(hwnd, 0),              // VK_HOME
                0x23 => step_to(hwnd, isize::MAX / 2), // VK_END
                _ => {}
            }
            0
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
            notify(hwnd, CBN_KILLFOCUS);
            crate::window::InvalidateRect(hwnd, core::ptr::null(), 1);
            0
        }
        WM_GETDLGCODE => 0x1 | 0x80,
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

/// The drop-down list's window: nothing to draw yet (it stays hidden).
pub extern "system" fn list_proc(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize {
    DefWindowProcW(hwnd, msg, wparam, lparam)
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

//! Menus as data: HMENU objects holding items (strings, separators,
//! popups with their submenus, ids, states, item data), with the
//! documented results of the menu functions. MF_BYCOMMAND reaches into
//! submenus. A window's menu (SetMenu/GetMenu) and its system menu
//! (GetSystemMenu: Restore, Move, Size, Minimize, Maximize, a separator,
//! Close) go with it. Drawing a menu bar and tracking popups come later
//! (DrawMenuBar only answers TRUE; the client area keeps its size).

use crate::window::Hwnd;
use core::sync::atomic::{AtomicBool, Ordering};

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn VirtualAlloc(
        at: *mut core::ffi::c_void,
        size: usize,
        kind: u32,
        protect: u32,
    ) -> *mut core::ffi::c_void;
    fn VirtualFree(at: *mut core::ffi::c_void, size: usize, kind: u32) -> i32;
}

const MAX_MENUS: usize = 512;
const MAX_ITEMS: usize = 64;
const TEXT: usize = 64;
const HMENU_BASE: usize = 0x0003_0000;

const MF_GRAYED: u32 = 0x1;
const MF_DISABLED: u32 = 0x2;
const MF_BITMAP: u32 = 0x4;
const MF_CHECKED: u32 = 0x8;
const MF_POPUP: u32 = 0x10;
const MF_OWNERDRAW: u32 = 0x100;
const MF_BYPOSITION: u32 = 0x400;
const MF_SEPARATOR: u32 = 0x800;
const MF_HILITE: u32 = 0x80;
const MF_DEFAULT: u32 = 0x1000;
/// What an item's type keeps of the MF_ flags (the rest is state).
const TYPE_FLAGS: u32 = MF_BITMAP | MF_OWNERDRAW | MF_SEPARATOR | 0x20 | 0x40 | 0x200 | 0x4000;
const STATE_FLAGS: u32 = MF_GRAYED | MF_DISABLED | MF_CHECKED | MF_HILITE | MF_DEFAULT;

#[derive(Clone, Copy)]
struct Item {
    id: u32,
    /// MFT_ bits (MF_ type flags).
    kind: u32,
    /// MFS_ bits (MF_ state flags).
    state: u32,
    submenu: usize,
    data: usize,
    bitmap: usize,
    text: [u16; TEXT],
}

const NO_ITEM: Item =
    Item { id: 0, kind: 0, state: 0, submenu: 0, data: 0, bitmap: 0, text: [0; TEXT] };

struct Menu {
    popup: bool,
    count: usize,
    items: [Item; MAX_ITEMS],
}

struct Table {
    lock: AtomicBool,
    menus: core::cell::UnsafeCell<[*mut Menu; MAX_MENUS]>,
    /// (window, menu, system menu).
    windows: core::cell::UnsafeCell<[(Hwnd, usize, usize); 128]>,
}

// SAFETY: every access is under `lock`.
unsafe impl Sync for Table {}

static TABLE: Table = Table {
    lock: AtomicBool::new(false),
    menus: core::cell::UnsafeCell::new([core::ptr::null_mut(); MAX_MENUS]),
    windows: core::cell::UnsafeCell::new([(0, 0, 0); 128]),
};

struct State<'a> {
    menus: &'a mut [*mut Menu; MAX_MENUS],
    windows: &'a mut [(Hwnd, usize, usize); 128],
}

fn with<R>(f: impl FnOnce(&mut State) -> R) -> R {
    while TABLE.lock.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        core::hint::spin_loop();
    }
    // SAFETY: under the lock.
    let r = unsafe {
        f(&mut State { menus: &mut *TABLE.menus.get(), windows: &mut *TABLE.windows.get() })
    };
    TABLE.lock.store(false, Ordering::Release);
    r
}

fn slot(h: usize) -> Option<usize> {
    let d = h.checked_sub(HMENU_BASE)?;
    (d % 0x10 == 0 && d / 0x10 < MAX_MENUS).then_some(d / 0x10)
}

impl State<'_> {
    fn menu(&mut self, h: usize) -> Option<&mut Menu> {
        let p = *self.menus.get(slot(h)?)?;
        // SAFETY: a live menu's memory (freed only when its slot clears).
        (!p.is_null()).then(|| unsafe { &mut *p })
    }

    fn create(&mut self, popup: bool) -> usize {
        let Some(i) = self.menus.iter().position(|p| p.is_null()) else { return 0 };
        // SAFETY: a plain call; zeroed memory is an empty menu.
        let p = unsafe { VirtualAlloc(core::ptr::null_mut(), size_of::<Menu>(), 0x3000, 4) }
            .cast::<Menu>();
        if p.is_null() {
            return 0;
        }
        // SAFETY: fresh memory of the size of a Menu.
        unsafe { (*p).popup = popup };
        self.menus[i] = p;
        HMENU_BASE + i * 0x10
    }

    /// Destroys `h` and its submenus.
    fn destroy(&mut self, h: usize) -> bool {
        let Some(i) = slot(h) else { return false };
        let p = self.menus[i];
        if p.is_null() {
            return false;
        }
        self.menus[i] = core::ptr::null_mut();
        // SAFETY: the menu's memory, no longer reachable from the table.
        let subs: [usize; MAX_ITEMS] = unsafe { core::array::from_fn(|k| (*p).items[k].submenu) };
        // SAFETY: as above.
        let count = unsafe { (*p).count };
        // SAFETY: allocated by `create`.
        unsafe { VirtualFree(p.cast(), 0, 0x8000) };
        for &s in subs.iter().take(count) {
            if s != 0 {
                self.destroy(s);
            }
        }
        true
    }

    /// The item `which` of `h` (by position, or by command searching
    /// submenus): the menu that holds it and its index.
    fn find(&mut self, h: usize, which: u32, by_position: bool) -> Option<(usize, usize)> {
        let m = self.menu(h)?;
        if by_position {
            return ((which as usize) < m.count).then_some((h, which as usize));
        }
        let count = m.count;
        if let Some(k) = m.items.iter().take(count).position(|i| i.submenu == 0 && i.id == which) {
            return Some((h, k));
        }
        for k in 0..count {
            let sub = self.menu(h)?.items[k].submenu;
            if sub != 0
                && let Some(found) = self.find(sub, which, false)
            {
                return Some(found);
            }
        }
        None
    }

    fn item(&mut self, at: (usize, usize)) -> Option<&mut Item> {
        let m = self.menu(at.0)?;
        m.items.get_mut(at.1)
    }

    fn insert(&mut self, h: usize, at: usize, item: Item) -> bool {
        let Some(m) = self.menu(h) else { return false };
        if m.count == MAX_ITEMS {
            return false;
        }
        let at = at.min(m.count);
        m.items.copy_within(at..m.count, at + 1);
        m.items[at] = item;
        m.count += 1;
        true
    }

    fn remove(&mut self, at: (usize, usize)) -> Option<Item> {
        let m = self.menu(at.0)?;
        let item = m.items[at.1];
        m.items.copy_within(at.1 + 1..m.count, at.1);
        m.count -= 1;
        m.items[m.count] = NO_ITEM;
        Some(item)
    }
}

/// A NUL-terminated string (or NULL) as an item's text.
fn text_of(s: *const u16) -> [u16; TEXT] {
    let mut t = [0u16; TEXT];
    if s.is_null() {
        return t;
    }
    for (i, o) in t.iter_mut().take(TEXT - 1).enumerate() {
        // SAFETY: the caller's NUL-terminated string.
        let c = unsafe { *s.add(i) };
        if c == 0 {
            break;
        }
        *o = c;
    }
    t
}

fn text_len(t: &[u16; TEXT]) -> usize {
    t.iter().position(|&c| c == 0).unwrap_or(TEXT)
}

/// An item from MF_ flags, an id (or submenu) and a string.
fn item_of(flags: u32, id: usize, text: *const u16) -> Item {
    let popup = flags & MF_POPUP != 0;
    let kind = flags & TYPE_FLAGS;
    Item {
        id: if popup { 0 } else { id as u32 },
        kind,
        state: flags & STATE_FLAGS,
        submenu: if popup { id } else { 0 },
        data: 0,
        bitmap: if flags & MF_BITMAP != 0 { text as usize } else { 0 },
        text: if kind & (MF_BITMAP | MF_OWNERDRAW | MF_SEPARATOR) == 0 {
            text_of(text)
        } else {
            [0; TEXT]
        },
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn CreateMenu() -> usize {
    with(|s| s.create(false))
}

#[unsafe(no_mangle)]
pub extern "system" fn CreatePopupMenu() -> usize {
    with(|s| s.create(true))
}

#[unsafe(no_mangle)]
pub extern "system" fn DestroyMenu(h: usize) -> i32 {
    with(|s| i32::from(s.destroy(h)))
}

#[unsafe(no_mangle)]
pub extern "system" fn IsMenu(h: usize) -> i32 {
    with(|s| i32::from(s.menu(h).is_some()))
}

#[unsafe(no_mangle)]
pub extern "system" fn GetMenuItemCount(h: usize) -> i32 {
    with(|s| s.menu(h).map_or(-1, |m| m.count as i32))
}

#[unsafe(no_mangle)]
pub extern "system" fn AppendMenuW(h: usize, flags: u32, id: usize, text: *const u16) -> i32 {
    with(|s| i32::from(s.insert(h, usize::MAX, item_of(flags, id, text))))
}

#[unsafe(no_mangle)]
pub extern "system" fn AppendMenuA(h: usize, flags: u32, id: usize, text: *const u8) -> i32 {
    let mut w = [0u16; TEXT];
    let t = crate::window::ansi_into_ptr(text, &mut w);
    AppendMenuW(h, flags, id, t)
}

/// InsertMenuW(h, position, flags, id, text): before the item named by
/// position or command (-1 by position: at the end).
#[unsafe(no_mangle)]
pub extern "system" fn InsertMenuW(
    h: usize,
    position: u32,
    flags: u32,
    id: usize,
    text: *const u16,
) -> i32 {
    let item = item_of(flags, id, text);
    with(|s| {
        if flags & MF_BYPOSITION != 0 {
            let at = if position == u32::MAX { usize::MAX } else { position as usize };
            return i32::from(s.insert(h, at, item));
        }
        match s.find(h, position, false) {
            Some((menu, at)) => i32::from(s.insert(menu, at, item)),
            None => 0,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn ModifyMenuW(
    h: usize,
    position: u32,
    flags: u32,
    id: usize,
    text: *const u16,
) -> i32 {
    let new = item_of(flags, id, text);
    with(|s| {
        let Some(at) = s.find(h, position, flags & MF_BYPOSITION != 0) else { return 0 };
        let Some(item) = s.item(at) else { return 0 };
        let data = item.data;
        *item = Item { data, ..new };
        1
    })
}

/// RemoveMenu: the item goes, its submenu stays.
#[unsafe(no_mangle)]
pub extern "system" fn RemoveMenu(h: usize, position: u32, flags: u32) -> i32 {
    with(|s| match s.find(h, position, flags & MF_BYPOSITION != 0) {
        Some(at) => i32::from(s.remove(at).is_some()),
        None => 0,
    })
}

/// DeleteMenu: the item goes and its submenu is destroyed.
#[unsafe(no_mangle)]
pub extern "system" fn DeleteMenu(h: usize, position: u32, flags: u32) -> i32 {
    with(|s| match s.find(h, position, flags & MF_BYPOSITION != 0).and_then(|at| s.remove(at)) {
        Some(item) => {
            if item.submenu != 0 {
                s.destroy(item.submenu);
            }
            1
        }
        None => 0,
    })
}

/// GetMenuItemID: the id; -1 for a popup or no such item.
#[unsafe(no_mangle)]
pub extern "system" fn GetMenuItemID(h: usize, position: i32) -> u32 {
    with(|s| match s.find(h, position as u32, true).and_then(|at| s.item(at).copied()) {
        Some(item) if item.submenu == 0 => item.id,
        _ => u32::MAX,
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn GetSubMenu(h: usize, position: i32) -> usize {
    with(|s| {
        s.find(h, position as u32, true).and_then(|at| s.item(at).copied()).map_or(0, |i| i.submenu)
    })
}

/// GetMenuState: the item's state and type flags (a popup: its item count
/// in the high byte and MF_POPUP); -1 for no such item.
#[unsafe(no_mangle)]
pub extern "system" fn GetMenuState(h: usize, id: u32, flags: u32) -> u32 {
    with(|s| {
        let Some(item) =
            s.find(h, id, flags & MF_BYPOSITION != 0).and_then(|at| s.item(at).copied())
        else {
            return u32::MAX;
        };
        let bits = item.state | item.kind;
        if item.submenu != 0 {
            let count = s.menu(item.submenu).map_or(0, |m| m.count as u32);
            return (count << 8) | MF_POPUP | (bits & 0xff);
        }
        bits
    })
}

/// EnableMenuItem: MF_ENABLED/MF_GRAYED/MF_DISABLED; the previous ones
/// (-1 for no such item).
#[unsafe(no_mangle)]
pub extern "system" fn EnableMenuItem(h: usize, id: u32, flags: u32) -> u32 {
    with(|s| {
        let Some(item) = s.find(h, id, flags & MF_BYPOSITION != 0).and_then(|at| s.item(at)) else {
            return u32::MAX;
        };
        let previous = item.state & (MF_GRAYED | MF_DISABLED);
        item.state =
            (item.state & !(MF_GRAYED | MF_DISABLED)) | (flags & (MF_GRAYED | MF_DISABLED));
        previous
    })
}

/// CheckMenuItem: MF_CHECKED/MF_UNCHECKED; the previous (-1 for no such
/// item).
#[unsafe(no_mangle)]
pub extern "system" fn CheckMenuItem(h: usize, id: u32, flags: u32) -> u32 {
    with(|s| {
        let Some(item) = s.find(h, id, flags & MF_BYPOSITION != 0).and_then(|at| s.item(at)) else {
            return u32::MAX;
        };
        let previous = item.state & MF_CHECKED;
        item.state = (item.state & !MF_CHECKED) | (flags & MF_CHECKED);
        previous
    })
}

/// CheckMenuRadioItem(h, first, last, check, flags): checks one item of
/// the range and unchecks the others (by position or command).
#[unsafe(no_mangle)]
pub extern "system" fn CheckMenuRadioItem(
    h: usize,
    first: u32,
    last: u32,
    check: u32,
    flags: u32,
) -> i32 {
    let by_position = flags & MF_BYPOSITION != 0;
    with(|s| {
        let Some(m) = s.menu(h) else { return 0 };
        let count = m.count;
        let mut found = false;
        for (k, item) in m.items.iter_mut().take(count).enumerate() {
            let key = if by_position { k as u32 } else { item.id };
            if (first..=last).contains(&key) {
                if key == check {
                    item.state |= MF_CHECKED;
                    item.kind |= 0x200; // MFT_RADIOCHECK
                    found = true;
                } else {
                    item.state &= !MF_CHECKED;
                }
            }
        }
        i32::from(found)
    })
}

/// GetMenuStringW: the item's text (n - 1 characters at most); its length
/// without the NUL (the whole length when `buf` is NULL).
#[unsafe(no_mangle)]
pub extern "system" fn GetMenuStringW(h: usize, id: u32, buf: *mut u16, n: i32, flags: u32) -> i32 {
    with(|s| {
        let Some(item) =
            s.find(h, id, flags & MF_BYPOSITION != 0).and_then(|at| s.item(at).copied())
        else {
            return 0;
        };
        let len = text_len(&item.text);
        if buf.is_null() || n <= 0 {
            return len as i32;
        }
        let k = len.min(n as usize - 1);
        // SAFETY: the caller's buffer of `n` characters.
        unsafe {
            core::ptr::copy_nonoverlapping(item.text.as_ptr(), buf, k);
            *buf.add(k) = 0;
        }
        k as i32
    })
}

/// MENUITEMINFOW (x64 layout).
#[repr(C)]
pub struct MenuItemInfoW {
    size: u32,
    mask: u32,
    kind: u32,
    state: u32,
    id: u32,
    submenu: usize,
    checked: usize,
    unchecked: usize,
    data: usize,
    type_data: *mut u16,
    cch: u32,
    bitmap: usize,
}

const MIIM_STATE: u32 = 0x1;
const MIIM_ID: u32 = 0x2;
const MIIM_SUBMENU: u32 = 0x4;
const MIIM_TYPE: u32 = 0x10;
const MIIM_DATA: u32 = 0x20;
const MIIM_STRING: u32 = 0x40;
const MIIM_BITMAP: u32 = 0x80;
const MIIM_FTYPE: u32 = 0x100;

/// Applies a MENUITEMINFOW to an item.
fn apply(item: &mut Item, m: &MenuItemInfoW) {
    if m.mask & MIIM_STATE != 0 {
        item.state = m.state & STATE_FLAGS;
    }
    if m.mask & MIIM_ID != 0 {
        item.id = m.id;
    }
    if m.mask & MIIM_SUBMENU != 0 {
        item.submenu = m.submenu;
    }
    if m.mask & MIIM_DATA != 0 {
        item.data = m.data;
    }
    if m.mask & (MIIM_FTYPE | MIIM_TYPE) != 0 {
        item.kind = m.kind & TYPE_FLAGS;
    }
    if m.mask & MIIM_BITMAP != 0 {
        item.bitmap = m.bitmap;
    }
    let string = m.mask & MIIM_STRING != 0
        || (m.mask & MIIM_TYPE != 0 && m.kind & (MF_BITMAP | MF_OWNERDRAW | MF_SEPARATOR) == 0);
    if string {
        item.text = text_of(m.type_data);
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn InsertMenuItemW(
    h: usize,
    which: u32,
    by_position: i32,
    info: *const MenuItemInfoW,
) -> i32 {
    if info.is_null() {
        return 0;
    }
    // SAFETY: the caller's MENUITEMINFOW.
    let m = unsafe { &*info };
    let mut item = NO_ITEM;
    apply(&mut item, m);
    with(|s| {
        if by_position != 0 {
            let at = if which == u32::MAX { usize::MAX } else { which as usize };
            return i32::from(s.insert(h, at, item));
        }
        match s.find(h, which, false) {
            Some((menu, at)) => i32::from(s.insert(menu, at, item)),
            None => 0,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn SetMenuItemInfoW(
    h: usize,
    which: u32,
    by_position: i32,
    info: *const MenuItemInfoW,
) -> i32 {
    if info.is_null() {
        return 0;
    }
    // SAFETY: the caller's MENUITEMINFOW.
    let m = unsafe { &*info };
    with(|s| match s.find(h, which, by_position != 0).and_then(|at| s.item(at)) {
        Some(item) => {
            apply(item, m);
            1
        }
        None => 0,
    })
}

/// GetMenuItemInfoW: what the mask asks for; strings into dwTypeData (cch
/// characters at most), cch becoming the length without the NUL.
#[unsafe(no_mangle)]
pub extern "system" fn GetMenuItemInfoW(
    h: usize,
    which: u32,
    by_position: i32,
    info: *mut MenuItemInfoW,
) -> i32 {
    if info.is_null() {
        return 0;
    }
    let Some(item) =
        with(|s| s.find(h, which, by_position != 0).and_then(|at| s.item(at).copied()))
    else {
        return 0;
    };
    // SAFETY: the caller's MENUITEMINFOW.
    let m = unsafe { &mut *info };
    if m.mask & MIIM_STATE != 0 {
        m.state = item.state;
    }
    if m.mask & MIIM_ID != 0 {
        m.id = item.id;
    }
    if m.mask & MIIM_SUBMENU != 0 {
        m.submenu = item.submenu;
    }
    if m.mask & MIIM_DATA != 0 {
        m.data = item.data;
    }
    if m.mask & (MIIM_FTYPE | MIIM_TYPE) != 0 {
        m.kind = item.kind;
    }
    if m.mask & MIIM_BITMAP != 0 {
        m.bitmap = item.bitmap;
    }
    if m.mask & (MIIM_STRING | MIIM_TYPE) != 0 {
        let len = text_len(&item.text);
        if !m.type_data.is_null() && m.cch > 0 {
            let k = len.min(m.cch as usize - 1);
            // SAFETY: the caller's buffer of cch characters.
            unsafe {
                core::ptr::copy_nonoverlapping(item.text.as_ptr(), m.type_data, k);
                *m.type_data.add(k) = 0;
            }
        }
        m.cch = len as u32;
    }
    1
}

#[unsafe(no_mangle)]
pub extern "system" fn SetMenuDefaultItem(h: usize, which: u32, by_position: u32) -> i32 {
    with(|s| {
        let Some(m) = s.menu(h) else { return 0 };
        let count = m.count;
        for (k, item) in m.items.iter_mut().take(count).enumerate() {
            let key = if by_position != 0 { k as u32 } else { item.id };
            if key == which {
                item.state |= MF_DEFAULT;
            } else {
                item.state &= !MF_DEFAULT;
            }
        }
        1
    })
}

// ---- Windows' menus ----

fn entry<'a>(s: &'a mut State, hwnd: Hwnd) -> Option<&'a mut (Hwnd, usize, usize)> {
    if let Some(i) = s.windows.iter().position(|e| e.0 == hwnd) {
        return Some(&mut s.windows[i]);
    }
    let i = s.windows.iter().position(|e| e.0 == 0)?;
    s.windows[i] = (hwnd, 0, 0);
    Some(&mut s.windows[i])
}

#[unsafe(no_mangle)]
pub extern "system" fn SetMenu(hwnd: Hwnd, menu: usize) -> i32 {
    if crate::window::IsWindow(hwnd) == 0 {
        return 0;
    }
    with(|s| {
        if menu != 0 && s.menu(menu).is_none() {
            return 0;
        }
        match entry(s, hwnd) {
            Some(e) => {
                e.1 = menu;
                1
            }
            None => 0,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn GetMenu(hwnd: Hwnd) -> usize {
    with(|s| s.windows.iter().find(|e| e.0 == hwnd && hwnd != 0).map_or(0, |e| e.1))
}

/// DrawMenuBar: TRUE for a window (the bar is not drawn yet).
#[unsafe(no_mangle)]
pub extern "system" fn DrawMenuBar(hwnd: Hwnd) -> i32 {
    crate::window::IsWindow(hwnd)
}

/// GetSystemMenu(hwnd, revert): the window's own copy of the window menu
/// (made on first use); revert: back to the default, NULL.
#[unsafe(no_mangle)]
pub extern "system" fn GetSystemMenu(hwnd: Hwnd, revert: i32) -> usize {
    if crate::window::IsWindow(hwnd) == 0 {
        return 0;
    }
    let existing = with(|s| entry(s, hwnd).map_or(0, |e| e.2));
    if revert != 0 {
        if existing != 0 {
            with(|s| {
                s.destroy(existing);
                if let Some(e) = entry(s, hwnd) {
                    e.2 = 0;
                }
            });
        }
        return 0;
    }
    if existing != 0 {
        return existing;
    }
    let m = CreatePopupMenu();
    let items: [(u32, &[u8]); 7] = [
        (0xf120, b"&Restore"),
        (0xf010, b"&Move"),
        (0xf000, b"&Size"),
        (0xf020, b"Mi&nimize"),
        (0xf030, b"Ma&ximize"),
        (0, b""),
        (0xf060, b"&Close\tAlt+F4"),
    ];
    for (id, text) in items {
        if id == 0 {
            AppendMenuW(m, MF_SEPARATOR, 0, core::ptr::null());
            continue;
        }
        let mut w = [0u16; 32];
        for (o, &c) in w.iter_mut().zip(text) {
            *o = u16::from(c);
        }
        AppendMenuW(m, 0, id as usize, w.as_ptr());
    }
    with(|s| {
        if let Some(e) = entry(s, hwnd) {
            e.2 = m;
        }
    });
    m
}

/// A window went: its menu and system menu are destroyed with it.
pub fn window_destroyed(hwnd: Hwnd) {
    with(|s| {
        if let Some(i) = s.windows.iter().position(|e| e.0 == hwnd) {
            let (_, menu, system) = s.windows[i];
            s.windows[i] = (0, 0, 0);
            for m in [menu, system] {
                if m != 0 {
                    s.destroy(m);
                }
            }
        }
    });
}

//! Menus, as Windows' Win32 menu proxies expose them: a window's menu
//! bar is a MenuBar element (no HWND of its own) whose children are its
//! items (MenuItem); an open popup menu is its #32768 window (a Menu,
//! named after the item that opened it, owned by the menu's window) whose
//! children are its items. Items with a submenu have the ExpandCollapse
//! pattern, the others Invoke.
//!
//! gala's menus live in the program's user32 (on Windows they are win32k
//! objects any process can read), so UIAutomationCore asks the menu's
//! window with gala's GALA_MENU message, which user32 answers itself
//! (track.rs; never the window procedure): a description of the bar or
//! popup (count, name, each item's id, flags, screen rectangle and raw
//! text), or an action on item k (open its submenu as a click on it
//! does, choose it, close its submenu). Across processes user32 marshals
//! the description buffer like WM_GETTEXT's.

use crate::element::Node;
use crate::{Hwnd, SendMessageW, alloc, bstr, free};

const GALA_MENU: u32 = 0x03f0;
const SIZE: usize = 8192;
const HEADER: usize = 8 + 2 * 64;
const RECORD: usize = 4 + 4 + 16 + 4 + 2 * 64;
const QUERY: usize = 1;
pub const EXPAND: usize = 2;
pub const INVOKE: usize = 3;
pub const COLLAPSE: usize = 4;

pub const SUBMENU: u32 = 1;
pub const DISABLED: u32 = 2;
pub const SEPARATOR: u32 = 4;
pub const OPEN: u32 = 16;

/// Node.menu: the menu bar of window `hwnd` (item None) or its item.
pub const BAR: u8 = 1;
/// Node.menu: item k of the popup menu window `hwnd`.
pub const POPUP: u8 = 2;

/// A menu's description, as user32 wrote it.
pub struct Menu {
    buf: *mut u8,
    count: usize,
}

/// One item.
#[derive(Clone, Copy)]
pub struct Item {
    pub flags: u32,
    pub rect: [i32; 4],
    pub text: [u16; 64],
    pub len: usize,
}

impl Menu {
    fn u32_at(&self, at: usize) -> u32 {
        let mut b = [0u8; 4];
        // SAFETY: our buffer of SIZE bytes; callers stay inside it.
        unsafe { core::ptr::copy_nonoverlapping(self.buf.add(at), b.as_mut_ptr(), 4) };
        u32::from_le_bytes(b)
    }

    fn text_at(&self, at: usize, len: usize) -> ([u16; 64], usize) {
        let mut t = [0u16; 64];
        let len = len.min(64);
        for (k, c) in t.iter_mut().enumerate().take(len) {
            let mut b = [0u8; 2];
            // SAFETY: as above.
            unsafe { core::ptr::copy_nonoverlapping(self.buf.add(at + 2 * k), b.as_mut_ptr(), 2) };
            *c = u16::from_le_bytes(b);
        }
        (t, len)
    }

    pub fn count(&self) -> usize {
        self.count
    }

    /// A popup's name: the label of the item that opened it.
    pub fn name(&self) -> ([u16; 64], usize) {
        let len = self.u32_at(4) as usize;
        self.text_at(8, len)
    }

    pub fn item(&self, k: usize) -> Option<Item> {
        if k >= self.count {
            return None;
        }
        let at = HEADER + k * RECORD;
        let mut rect = [0; 4];
        for (j, r) in rect.iter_mut().enumerate() {
            *r = self.u32_at(at + 8 + 4 * j) as i32;
        }
        let (text, len) = self.text_at(at + 28, self.u32_at(at + 24) as usize);
        Some(Item { flags: self.u32_at(at + 4), rect, text, len })
    }
}

impl Drop for Menu {
    fn drop(&mut self) {
        free(self.buf);
    }
}

/// The menu bar of top-level window `h`, or the popup menu window `h`
/// shows (None: neither).
pub fn query(h: Hwnd) -> Option<Menu> {
    let buf = alloc(SIZE);
    if buf.is_null() {
        return None;
    }
    // SAFETY: our buffer of SIZE bytes, which the message fills.
    let n = unsafe {
        core::ptr::write_bytes(buf, 0, SIZE);
        SendMessageW(h, GALA_MENU, QUERY, buf as isize)
    };
    if n <= 0 || n as usize > (SIZE - HEADER) / RECORD {
        free(buf);
        return None;
    }
    Some(Menu { buf, count: n as usize })
}

/// The item a node stands for (None: gone, or not an item).
pub fn item(n: Node) -> Option<Item> {
    query(n.hwnd)?.item(n.item? as usize)
}

/// Whether the node is still there.
pub fn exists(n: Node) -> bool {
    match (query(n.hwnd), n.item) {
        (Some(_), None) => true,
        (Some(m), Some(k)) => (k as usize) < m.count(),
        _ => false,
    }
}

/// An item's label: its text without '&' marks and the accelerator text
/// after a tab (`accel`: that text instead), as a BSTR.
pub fn label(it: &Item, accel: bool) -> *mut u16 {
    let src = &it.text[..it.len];
    let tab = src.iter().position(|&c| c == u16::from(b'\t'));
    if accel {
        return bstr(tab.map_or(&[][..], |t| &src[t + 1..]));
    }
    let src = &src[..tab.unwrap_or(src.len())];
    let mut out = [0u16; 64];
    let (mut i, mut j) = (0, 0);
    while i < src.len() {
        if src[i] == u16::from(b'&') {
            i += 1;
            if i == src.len() {
                break;
            }
        }
        out[j] = src[i];
        i += 1;
        j += 1;
    }
    bstr(&out[..j])
}

/// An action on item k of the bar or popup `h`: whether user32 did it.
pub fn act(h: Hwnd, k: u32, op: usize) -> bool {
    // SAFETY: a plain call.
    unsafe { SendMessageW(h, GALA_MENU, op | (k as usize) << 8, 0) != 0 }
}

//! List items, as Windows' Win32 proxies expose them: a list box's
//! strings (class ListBox) and a list view's items (SysListView32, also
//! LVS_OWNERDATA lists, which answer by asking their parent), read and
//! changed with the controls' own messages.
//!
//! List boxes take the messages with plain local pointers: user32
//! marshals the list box messages below WM_USER across processes (as
//! Windows does). List-view messages (LVM_*, above WM_USER) are not
//! marshaled, so, as Windows' list-view proxy does, the structure goes
//! into memory allocated in the list's process (OpenProcess with
//! PROCESS_VM_OPERATION | VM_READ | VM_WRITE, VirtualAllocEx,
//! WriteProcessMemory), the message carries that address, the answer is
//! read back (ReadProcessMemory) and the memory freed (VirtualFreeEx). A
//! list in this process gets a local buffer instead (`Buffer`).

use core::ffi::c_void;

use crate::{
    ClientToScreen, CloseHandle, GetClientRect, GetCurrentProcessId, GetDlgCtrlID, GetParent,
    GetWindowRect, GetWindowThreadProcessId, HResult, Hwnd, IsWindowEnabled, List, OpenProcess,
    PostMessageW, ReadProcessMemory, S_OK, SendMessageW, UIA_E_ELEMENTNOTENABLED,
    UIA_E_INVALIDOPERATION, VirtualAllocEx, VirtualFreeEx, WriteProcessMemory, alloc, bstr, free,
};

const LB_SETSEL: u32 = 0x0185;
const LB_SETCURSEL: u32 = 0x0186;
const LB_GETSEL: u32 = 0x0187;
const LB_GETCURSEL: u32 = 0x0188;
const LB_GETTEXT: u32 = 0x0189;
const LB_GETTEXTLEN: u32 = 0x018a;
const LB_GETCOUNT: u32 = 0x018b;
const LB_GETSELCOUNT: u32 = 0x0190;
const LB_GETSELITEMS: u32 = 0x0191;
const LB_SETTOPINDEX: u32 = 0x0197;
const LB_GETITEMRECT: u32 = 0x0198;
const LB_SETCARETINDEX: u32 = 0x019e;
const LB_GETCARETINDEX: u32 = 0x019f;
const LBS_NOTIFY: u32 = 0x0001;
const LBS_MULTIPLESEL: u32 = 0x0008;
const LBS_EXTENDEDSEL: u32 = 0x0800;
const LBN_SELCHANGE: usize = 1;
const WM_COMMAND: u32 = 0x0111;

const LVM_GETITEMCOUNT: u32 = 0x1004;
const LVM_GETNEXTITEM: u32 = 0x100c;
const LVM_GETITEMRECT: u32 = 0x100e;
const LVM_ENSUREVISIBLE: u32 = 0x1013;
const LVM_SETITEMSTATE: u32 = 0x102b;
const LVM_GETITEMSTATE: u32 = 0x102c;
const LVM_GETITEMTEXTW: u32 = 0x1073;
const LVS_SINGLESEL: u32 = 0x0004;
const LVIS_FOCUSED: u32 = 0x1;
const LVIS_SELECTED: u32 = 0x2;
const LVNI_FOCUSED: isize = 0x1;
const LVNI_SELECTED: isize = 0x2;
const LVIR_BOUNDS: i32 = 0;
const LVIR_SELECTBOUNDS: i32 = 3;
const WM_LBUTTONUP: u32 = 0x0202;
const WM_LBUTTONDBLCLK: u32 = 0x0203;
const MK_LBUTTON: usize = 0x1;

/// LVITEMW (mingw-w64 commctrl.h, x64): 88 bytes; state at 12, stateMask
/// at 16, pszText at 24, cchTextMax at 32.
const LVITEM_SIZE: usize = 88;
/// The longest item text read (characters).
const TEXT_MAX: usize = 1024;

const PROCESS_VM_OPERATION: u32 = 0x0008;
const PROCESS_VM_READ: u32 = 0x0010;
const PROCESS_VM_WRITE: u32 = 0x0020;

/// The kinds of list whose items are elements.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    ListBox,
    ListView,
}

fn class_is(h: Hwnd, name: &str) -> bool {
    let mut buf = [0u16; 64];
    // SAFETY: our buffer of 64 characters.
    let n = unsafe { crate::GetClassNameW(h, buf.as_mut_ptr(), 64) }.max(0) as usize;
    buf[..n].len() == name.len()
        && buf[..n]
            .iter()
            .zip(name.bytes())
            .all(|(&c, b)| c < 0x80 && (c as u8).eq_ignore_ascii_case(&b))
}

/// The kind of list `h` is (None: not one whose items are elements).
pub fn kind(h: Hwnd) -> Option<Kind> {
    if class_is(h, "ListBox") {
        Some(Kind::ListBox)
    } else if class_is(h, "SysListView32") {
        Some(Kind::ListView)
    } else {
        None
    }
}

fn send(h: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize {
    // SAFETY: a plain call (pointers in lParam are the caller's).
    unsafe { SendMessageW(h, msg, wparam, lparam) }
}

/// How many items the list has.
pub fn count(h: Hwnd, k: Kind) -> u32 {
    let n = match k {
        Kind::ListBox => send(h, LB_GETCOUNT, 0, 0),
        Kind::ListView => send(h, LVM_GETITEMCOUNT, 0, 0),
    };
    n.clamp(0, i32::MAX as isize) as u32
}

// ---- Memory the list can read ----

/// A buffer the list's process can read and write: local memory for a
/// list in this process, else memory allocated in the list's process.
struct Buffer {
    /// The other process (0: this one).
    process: usize,
    base: *mut u8,
}

impl Buffer {
    fn new(h: Hwnd, size: usize) -> Option<Buffer> {
        let mut pid = 0;
        // SAFETY: plain calls; our output.
        unsafe { GetWindowThreadProcessId(h, &mut pid) };
        // SAFETY: a plain call.
        if pid == 0 || pid == unsafe { GetCurrentProcessId() } {
            let base = alloc(size);
            if base.is_null() {
                return None;
            }
            // SAFETY: our fresh buffer of `size` bytes.
            unsafe { core::ptr::write_bytes(base, 0, size) };
            return Some(Buffer { process: 0, base });
        }
        let access = PROCESS_VM_OPERATION | PROCESS_VM_READ | PROCESS_VM_WRITE;
        // SAFETY: plain calls; the memory is committed zeroed read/write.
        unsafe {
            let process = OpenProcess(access, 0, pid);
            if process == 0 {
                crate::missing(b"UIAutomationCore.dll!items(OpenProcess)", u64::from(pid));
                return None;
            }
            let base = VirtualAllocEx(process, core::ptr::null_mut(), size, 0x3000, 0x04);
            if base.is_null() {
                CloseHandle(process);
                return None;
            }
            Some(Buffer { process, base: base.cast() })
        }
    }

    /// The buffer's address in the list's process.
    fn at(&self, offset: usize) -> isize {
        self.base as isize + offset as isize
    }

    fn write(&self, offset: usize, bytes: &[u8]) -> bool {
        if self.process == 0 {
            // SAFETY: our buffer, sized by the caller for this.
            unsafe {
                core::ptr::copy_nonoverlapping(bytes.as_ptr(), self.base.add(offset), bytes.len())
            };
            return true;
        }
        let mut done = 0;
        // SAFETY: our allocation in the other process; our bytes.
        unsafe {
            WriteProcessMemory(
                self.process,
                self.base.add(offset).cast(),
                bytes.as_ptr().cast(),
                bytes.len(),
                &mut done,
            ) != 0
        }
    }

    fn read(&self, offset: usize, out: &mut [u8]) -> bool {
        if self.process == 0 {
            // SAFETY: our buffer, sized by the caller for this.
            unsafe {
                core::ptr::copy_nonoverlapping(self.base.add(offset), out.as_mut_ptr(), out.len())
            };
            return true;
        }
        let mut done = 0;
        // SAFETY: our allocation in the other process; our output.
        unsafe {
            ReadProcessMemory(
                self.process,
                self.base.add(offset).cast::<c_void>(),
                out.as_mut_ptr().cast(),
                out.len(),
                &mut done,
            ) != 0
        }
    }
}

impl Drop for Buffer {
    fn drop(&mut self) {
        if self.process == 0 {
            free(self.base);
            return;
        }
        // SAFETY: our allocation and handle.
        unsafe {
            VirtualFreeEx(self.process, self.base.cast(), 0, 0x8000);
            CloseHandle(self.process);
        }
    }
}

/// An LVITEMW with `state`/`state_mask` and (when `text`) pszText
/// pointing just after it, at `base`, for TEXT_MAX characters.
fn lvitem(base: isize, state: u32, mask: u32, text: bool) -> [u8; LVITEM_SIZE] {
    let mut b = [0u8; LVITEM_SIZE];
    b[0..4].copy_from_slice(&(if text { 1u32 } else { 8u32 }).to_le_bytes()); // LVIF_TEXT / LVIF_STATE
    b[12..16].copy_from_slice(&state.to_le_bytes());
    b[16..20].copy_from_slice(&mask.to_le_bytes());
    if text {
        b[24..32].copy_from_slice(&((base + LVITEM_SIZE as isize) as u64).to_le_bytes());
        b[32..36].copy_from_slice(&(TEXT_MAX as i32).to_le_bytes());
    }
    b
}

// ---- Reading items ----

/// Item `i`'s text (a list view's: column 0) as a BSTR (null without
/// memory).
pub fn text(h: Hwnd, k: Kind, i: u32) -> *mut u16 {
    let mut chars = [0u16; TEXT_MAX];
    let n = match k {
        Kind::ListBox => {
            let len = send(h, LB_GETTEXTLEN, i as usize, 0);
            if len < 0 {
                0
            } else if len as usize >= TEXT_MAX {
                // A longer string: room for all of it.
                let buf = alloc(2 * (len as usize + 1)).cast::<u16>();
                if buf.is_null() {
                    return core::ptr::null_mut();
                }
                let got = send(h, LB_GETTEXT, i as usize, buf as isize).clamp(0, len);
                // SAFETY: `got` characters written.
                let b = bstr(unsafe { core::slice::from_raw_parts(buf, got as usize) });
                free(buf.cast());
                return b;
            } else {
                send(h, LB_GETTEXT, i as usize, chars.as_mut_ptr() as isize).clamp(0, len) as usize
            }
        }
        Kind::ListView => {
            let Some(buf) = Buffer::new(h, LVITEM_SIZE + 2 * TEXT_MAX) else {
                return bstr(&[]);
            };
            if !buf.write(0, &lvitem(buf.at(0), 0, 0, true)) {
                return bstr(&[]);
            }
            let got = send(h, LVM_GETITEMTEXTW, i as usize, buf.at(0))
                .clamp(0, TEXT_MAX as isize - 1) as usize;
            let mut bytes = [0u8; 2 * TEXT_MAX];
            if !buf.read(LVITEM_SIZE, &mut bytes[..2 * got]) {
                return bstr(&[]);
            }
            for (c, pair) in chars.iter_mut().zip(bytes[..2 * got].chunks_exact(2)) {
                *c = u16::from_le_bytes([pair[0], pair[1]]);
            }
            got
        }
    };
    bstr(&chars[..n])
}

/// The list's client area on the screen.
fn client_on_screen(h: Hwnd) -> [i32; 4] {
    let mut c = [0i32; 4];
    let mut origin = [0i32; 2];
    // SAFETY: our outputs.
    let local = unsafe { GetClientRect(h, &mut c) != 0 && ClientToScreen(h, &mut origin) != 0 };
    if local {
        return [origin[0], origin[1], origin[0] + c[2], origin[1] + c[3]];
    }
    // Another process's window: user32 knows its window rectangle, which
    // is a child control's client area on gala (no borders of their own).
    let mut r = [0i32; 4];
    // SAFETY: our output.
    unsafe { GetWindowRect(h, &mut r) };
    r
}

/// Item `i`'s rectangle in the list's client coordinates (unclipped).
fn client_rect(h: Hwnd, k: Kind, i: u32, code: i32) -> Option<[i32; 4]> {
    let mut r = [code, 0, 0, 0];
    match k {
        Kind::ListBox => {
            (send(h, LB_GETITEMRECT, i as usize, r.as_mut_ptr() as isize) != -1).then_some(r)
        }
        Kind::ListView => {
            let buf = Buffer::new(h, 16)?;
            let mut bytes = [0u8; 16];
            bytes[0..4].copy_from_slice(&code.to_le_bytes());
            if !buf.write(0, &bytes) || send(h, LVM_GETITEMRECT, i as usize, buf.at(0)) == 0 {
                return None;
            }
            if !buf.read(0, &mut bytes) {
                return None;
            }
            for (v, b) in r.iter_mut().zip(bytes.chunks_exact(4)) {
                *v = i32::from_le_bytes([b[0], b[1], b[2], b[3]]);
            }
            Some(r)
        }
    }
}

/// Item `i`'s rectangle on the screen, clipped to the list's client area
/// (the part that shows); None when none of it shows.
pub fn rect(h: Hwnd, k: Kind, i: u32) -> Option<[i32; 4]> {
    let r = client_rect(h, k, i, LVIR_BOUNDS)?;
    let c = client_on_screen(h);
    let s = [
        (c[0] + r[0]).max(c[0]),
        (c[1] + r[1]).max(c[1]),
        (c[0] + r[2]).min(c[2]),
        (c[1] + r[3]).min(c[3]),
    ];
    (s[2] > s[0] && s[3] > s[1]).then_some(s)
}

pub fn selected(h: Hwnd, k: Kind, i: u32) -> bool {
    match k {
        Kind::ListBox => send(h, LB_GETSEL, i as usize, 0) > 0,
        Kind::ListView => {
            send(h, LVM_GETITEMSTATE, i as usize, LVIS_SELECTED as isize) as u32 & LVIS_SELECTED
                != 0
        }
    }
}

/// Whether item `i` has the focus within the list (the list's caret).
pub fn focused(h: Hwnd, k: Kind, i: u32) -> bool {
    match k {
        Kind::ListBox => send(h, LB_GETCARETINDEX, 0, 0) == i as isize,
        Kind::ListView => send(h, LVM_GETNEXTITEM, usize::MAX, LVNI_FOCUSED) == i as isize,
    }
}

pub fn can_select_multiple(h: Hwnd, k: Kind) -> bool {
    let s = crate::element::style(h);
    match k {
        Kind::ListBox => s & (LBS_MULTIPLESEL | LBS_EXTENDEDSEL) != 0,
        Kind::ListView => s & LVS_SINGLESEL == 0,
    }
}

/// The selected items, in order.
pub fn selection(h: Hwnd, k: Kind, out: &mut List<u32>) {
    let n = count(h, k);
    match k {
        Kind::ListBox if !can_select_multiple(h, k) => {
            let i = send(h, LB_GETCURSEL, 0, 0);
            if i >= 0 && (i as u32) < n {
                out.push(i as u32);
            }
        }
        Kind::ListBox => {
            let m = send(h, LB_GETSELCOUNT, 0, 0).clamp(0, n as isize) as usize;
            if m == 0 {
                return;
            }
            let buf = alloc(4 * m).cast::<i32>();
            if buf.is_null() {
                return;
            }
            let got = send(h, LB_GETSELITEMS, m, buf as isize).clamp(0, m as isize) as usize;
            // SAFETY: `got` indexes written.
            for &i in unsafe { core::slice::from_raw_parts(buf, got) } {
                if !out.push(i as u32) {
                    break;
                }
            }
            free(buf.cast());
        }
        Kind::ListView => {
            let mut i: isize = -1;
            for _ in 0..n {
                i = send(h, LVM_GETNEXTITEM, i as usize, LVNI_SELECTED);
                if i < 0 || !out.push(i as u32) {
                    break;
                }
            }
        }
    }
}

// ---- Changing the selection ----

/// SelectionItem's Select, AddToSelection, RemoveFromSelection.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum How {
    Only,
    Add,
    Remove,
}

fn set_lv_state(h: Hwnd, i: usize, state: u32, mask: u32) -> bool {
    let Some(buf) = Buffer::new(h, LVITEM_SIZE) else { return false };
    buf.write(0, &lvitem(buf.at(0), state, mask, false))
        && send(h, LVM_SETITEMSTATE, i, buf.at(0)) != 0
}

/// A list box's parent hears of a selection change as it would of the
/// user's (LBN_SELCHANGE, with LBS_NOTIFY): programs act on it.
fn tell_parent(h: Hwnd) {
    if crate::element::style(h) & LBS_NOTIFY == 0 {
        return;
    }
    // SAFETY: plain calls.
    unsafe {
        let parent = GetParent(h);
        let id = GetDlgCtrlID(h) as usize & 0xffff;
        if parent != 0 {
            SendMessageW(parent, WM_COMMAND, id | LBN_SELCHANGE << 16, h as isize);
        }
    }
}

/// Changes item `i`'s selection as `how` says. A single-selection list
/// cannot add a second item (UIA_E_INVALIDOPERATION, as documented).
pub fn select(h: Hwnd, k: Kind, i: u32, how: How) -> HResult {
    // SAFETY: a plain call.
    if unsafe { IsWindowEnabled(h) } == 0 {
        return UIA_E_ELEMENTNOTENABLED;
    }
    let multi = can_select_multiple(h, k);
    let how = match how {
        How::Add if !multi => {
            let mut sel = List::new();
            selection(h, k, &mut sel);
            if sel.as_slice().iter().any(|&s| s != i) {
                return UIA_E_INVALIDOPERATION;
            }
            How::Only
        }
        other => other,
    };
    let i_ = i as usize;
    match (k, how) {
        (Kind::ListView, How::Only) => {
            set_lv_state(h, usize::MAX, 0, LVIS_SELECTED);
            let both = LVIS_SELECTED | LVIS_FOCUSED;
            set_lv_state(h, i_, both, both);
        }
        (Kind::ListView, How::Add) => {
            set_lv_state(h, i_, LVIS_SELECTED, LVIS_SELECTED);
        }
        (Kind::ListView, How::Remove) => {
            set_lv_state(h, i_, 0, LVIS_SELECTED);
        }
        (Kind::ListBox, How::Only) if multi => {
            send(h, LB_SETSEL, 0, -1);
            send(h, LB_SETSEL, 1, i as isize);
            send(h, LB_SETCARETINDEX, i_, 0);
            tell_parent(h);
        }
        (Kind::ListBox, How::Only) => {
            if send(h, LB_GETCURSEL, 0, 0) != i as isize {
                send(h, LB_SETCURSEL, i_, 0);
                tell_parent(h);
            }
        }
        (Kind::ListBox, how) if multi => {
            send(h, LB_SETSEL, usize::from(how == How::Add), i as isize);
            tell_parent(h);
        }
        (Kind::ListBox, _) => {
            if send(h, LB_GETCURSEL, 0, 0) == i as isize {
                send(h, LB_SETCURSEL, usize::MAX, 0);
                tell_parent(h);
            }
        }
    }
    S_OK
}

/// ScrollItem's ScrollIntoView: LVM_ENSUREVISIBLE; a list box scrolls
/// the item to the top unless it shows already (LB_SETTOPINDEX).
pub fn scroll_into_view(h: Hwnd, k: Kind, i: u32) -> HResult {
    match k {
        Kind::ListView => {
            send(h, LVM_ENSUREVISIBLE, i as usize, 0);
        }
        Kind::ListBox => {
            let c = client_on_screen(h);
            let whole = rect(h, k, i).is_some_and(|r| {
                client_rect(h, k, i, 0).is_some_and(|u| u[3] - u[1] == r[3] - r[1])
                    && r[1] >= c[1]
                    && r[3] <= c[3]
            });
            if !whole {
                send(h, LB_SETTOPINDEX, i as usize, 0);
            }
        }
    }
    S_OK
}

/// Invoke on a list-view item: it is selected and shown, then a double
/// click is posted at it (the list view sends its parent NM_DBLCLK and
/// LVN_ITEMACTIVATE when it handles it; Invoke may return before that).
pub fn activate(h: Hwnd, i: u32) -> HResult {
    let hr = select(h, Kind::ListView, i, How::Only);
    if hr != S_OK {
        return hr;
    }
    send(h, LVM_ENSUREVISIBLE, i as usize, 0);
    let Some(r) = client_rect(h, Kind::ListView, i, LVIR_SELECTBOUNDS) else {
        return crate::UIA_E_ELEMENTNOTAVAILABLE;
    };
    let (x, y) = ((r[0] + r[2]) / 2, (r[1] + r[3]) / 2);
    let at = ((y as u16 as u32) << 16 | x as u16 as u32) as isize;
    // SAFETY: plain calls.
    unsafe {
        PostMessageW(h, WM_LBUTTONDBLCLK, MK_LBUTTON, at);
        PostMessageW(h, WM_LBUTTONUP, 0, at);
    }
    S_OK
}

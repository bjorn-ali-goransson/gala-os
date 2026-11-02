//! Window subclassing (SetWindowSubclass, GetWindowSubclass,
//! RemoveWindowSubclass, DefSubclassProc), as documented: each window's
//! subclass procedures (by procedure and id, with reference data) form a
//! chain, the newest first; the window's procedure becomes `chain_proc`,
//! which calls the newest, and DefSubclassProc calls the next one, then
//! the window's original procedure. Removing the last one gives the window
//! its procedure back (when nothing replaced ours meanwhile); the chain
//! goes with the window (WM_NCDESTROY). Tooltips use it for TTF_SUBCLASS.

use crate::common::*;
use alloc::vec::Vec;

/// SUBCLASSPROC.
pub type SubclassProc = extern "system" fn(Hwnd, u32, usize, isize, usize, usize) -> isize;

const GWLP_WNDPROC: i32 = -4;
const WM_NCDESTROY: u32 = 0x0082;

#[derive(Clone, Copy)]
struct Entry {
    proc_: SubclassProc,
    id: usize,
    data: usize,
}

struct Chain {
    /// The window's procedure before the first subclass.
    original: isize,
    /// Newest first.
    entries: Vec<Entry>,
    /// The entry running now (-1: before the first: DefSubclassProc
    /// calls `pos + 1`).
    pos: isize,
    /// Calls of `chain_proc` in progress.
    depth: u32,
    /// The window has its own procedure back (the last subclass went).
    restored: bool,
}

static CHAINS: Store<Chain> = Store::new();

/// The procedure of every subclassed window.
extern "system" fn chain_proc(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize {
    let Some((saved, first, original)) = CHAINS.find(hwnd, |c| {
        let saved = core::mem::replace(&mut c.pos, 0);
        c.depth += 1;
        (saved, c.entries.first().copied(), c.original)
    }) else {
        // SAFETY: plain call.
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    };
    let r = match first {
        Some(e) => (e.proc_)(hwnd, msg, wparam, lparam, e.id, e.data),
        // SAFETY: the window's own procedure.
        None => unsafe { CallWindowProcW(original, hwnd, msg, wparam, lparam) },
    };
    let done = CHAINS
        .find(hwnd, |c| {
            c.pos = saved;
            c.depth -= 1;
            c.depth == 0 && (c.restored || msg == WM_NCDESTROY)
        })
        .unwrap_or(false);
    if done {
        CHAINS.remove(hwnd);
    }
    r
}

/// SetWindowSubclass(hwnd, proc, id, data): adds the subclass (or
/// changes its data when that procedure and id are there); TRUE.
#[unsafe(no_mangle)]
pub extern "system" fn SetWindowSubclass(
    hwnd: Hwnd,
    proc_: Option<SubclassProc>,
    id: usize,
    data: usize,
) -> i32 {
    let Some(proc_) = proc_ else { return 0 };
    // SAFETY: plain call.
    if hwnd == 0 || unsafe { IsWindow(hwnd) } == 0 {
        return 0;
    }
    let ours = chain_proc as WndProc as usize as isize;
    let existing = CHAINS.find(hwnd, |c| {
        if let Some(e) =
            c.entries.iter_mut().find(|e| e.proc_ as usize == proc_ as usize && e.id == id)
        {
            e.data = data;
            return true;
        }
        if c.entries.try_reserve(1).is_err() {
            return false;
        }
        c.entries.insert(0, Entry { proc_, id, data });
        c.restored = false;
        if c.pos >= 0 {
            c.pos += 1;
        }
        true
    });
    if let Some(ok) = existing {
        return i32::from(ok);
    }
    // SAFETY: plain calls.
    let original = unsafe { GetWindowLongPtrW(hwnd, GWLP_WNDPROC) };
    if original == 0 {
        return 0;
    }
    let mut entries = Vec::new();
    if entries.try_reserve(1).is_err() {
        return 0;
    }
    entries.push(Entry { proc_, id, data });
    let made = CHAINS.with(
        hwnd,
        || Chain { original, entries: Vec::new(), pos: -1, depth: 0, restored: false },
        |c| {
            c.original = original;
            c.entries = entries;
            c.restored = false;
        },
    );
    if made.is_none() {
        return 0;
    }
    // SAFETY: plain call; our procedure is the window's now.
    unsafe { SetWindowLongPtrW(hwnd, GWLP_WNDPROC, ours) };
    1
}

/// GetWindowSubclass(hwnd, proc, id, *data): whether the subclass is
/// there (its data in *data).
#[unsafe(no_mangle)]
pub extern "system" fn GetWindowSubclass(
    hwnd: Hwnd,
    proc_: Option<SubclassProc>,
    id: usize,
    data: *mut usize,
) -> i32 {
    let Some(proc_) = proc_ else { return 0 };
    let found = CHAINS
        .find(hwnd, |c| {
            c.entries
                .iter()
                .find(|e| e.proc_ as usize == proc_ as usize && e.id == id)
                .map(|e| e.data)
        })
        .flatten();
    if !data.is_null() {
        // SAFETY: the caller's output.
        unsafe { *data = found.unwrap_or(0) };
    }
    i32::from(found.is_some())
}

/// RemoveWindowSubclass(hwnd, proc, id): TRUE if it was there.
#[unsafe(no_mangle)]
pub extern "system" fn RemoveWindowSubclass(
    hwnd: Hwnd,
    proc_: Option<SubclassProc>,
    id: usize,
) -> i32 {
    let Some(proc_) = proc_ else { return 0 };
    let result = CHAINS.find(hwnd, |c| {
        let i = c.entries.iter().position(|e| e.proc_ as usize == proc_ as usize && e.id == id)?;
        c.entries.remove(i);
        if i as isize <= c.pos {
            c.pos -= 1;
        }
        Some((c.entries.is_empty(), c.depth, c.original))
    });
    let Some(Some((empty, depth, original))) = result else { return 0 };
    if empty {
        let ours = chain_proc as WndProc as usize as isize;
        // SAFETY: plain calls; the window's own procedure back, unless
        // another one replaced ours meanwhile (then ours stays in its
        // chain and calls the original).
        let restored = unsafe {
            GetWindowLongPtrW(hwnd, GWLP_WNDPROC) == ours
                && SetWindowLongPtrW(hwnd, GWLP_WNDPROC, original) == ours
        };
        if restored {
            if depth == 0 {
                CHAINS.remove(hwnd);
            } else {
                CHAINS.find(hwnd, |c| c.restored = true);
            }
        }
    }
    1
}

/// DefSubclassProc: the next subclass in the chain, or the window's
/// original procedure.
#[unsafe(no_mangle)]
pub extern "system" fn DefSubclassProc(
    hwnd: Hwnd,
    msg: u32,
    wparam: usize,
    lparam: isize,
) -> isize {
    let Some((saved, next, original)) = CHAINS.find(hwnd, |c| {
        let saved = c.pos;
        c.pos += 1;
        (saved, c.entries.get(c.pos.max(0) as usize).copied(), c.original)
    }) else {
        // SAFETY: plain call.
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    };
    let r = match next {
        Some(e) => (e.proc_)(hwnd, msg, wparam, lparam, e.id, e.data),
        // SAFETY: the window's own procedure.
        None => unsafe { CallWindowProcW(original, hwnd, msg, wparam, lparam) },
    };
    CHAINS.find(hwnd, |c| c.pos = saved);
    r
}

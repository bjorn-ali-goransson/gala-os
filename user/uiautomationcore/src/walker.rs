//! Tree walkers (IUIAutomationTreeWalker): the tree seen through a
//! condition. An element that does not meet it is left out and its
//! children take its place among its siblings; the root is in every view.

use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, Ordering};

use crate::condition::{self, Condition};
use crate::element::{self, children, desktop, hwnd_of, parent, put_element};
use crate::{
    E_INVALIDARG, E_NOINTERFACE, E_OUTOFMEMORY, E_POINTER, Guid, HResult, Hwnd, IID_IUNKNOWN, List,
    S_OK, VTable, boxed, free, guid, iid_at, put,
};

const IID_IUIAUTOMATIONTREEWALKER: Guid =
    guid(0x4042c624, 0x389c, 0x4afc, [0xa6, 0x30, 0x9d, 0xf8, 0x54, 0xa5, 0x41, 0xfc]);

#[repr(C)]
struct Walker {
    vtable: *const VTable<16>,
    refs: AtomicU32,
    cond: *mut Condition,
}

/// Stores a new walker over `cond` (one of ours) at `out`.
pub fn create(cond: *mut c_void, out: *mut *mut c_void) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    let Some(cond) = condition::as_condition(cond) else { return E_INVALIDARG };
    let w = boxed(Walker { vtable: &VTABLE, refs: AtomicU32::new(1), cond });
    if w.is_null() {
        return E_OUTOFMEMORY;
    }
    condition::add_ref(cond);
    put(out, w.cast());
    S_OK
}

fn cond_of(this: *mut Walker) -> *mut Condition {
    // SAFETY: our walker.
    unsafe { (*this).cond }
}

fn in_view(c: *mut Condition, h: Hwnd) -> bool {
    h == desktop() || condition::matches(c, h)
}

fn view_parent(c: *mut Condition, h: Hwnd) -> Hwnd {
    let mut p = parent(h);
    for _ in 0..64 {
        if p == 0 || in_view(c, p) {
            break;
        }
        p = parent(p);
    }
    p
}

fn view_children(c: *mut Condition, h: Hwnd, out: &mut List, depth: u32) {
    if depth > 64 {
        return;
    }
    let mut kids = List::new();
    children(h, &mut kids);
    for &k in kids.as_slice() {
        if condition::matches(c, k) {
            if !out.push(k) {
                return;
            }
        } else {
            view_children(c, k, out, depth + 1);
        }
    }
}

/// Runs `step` on the element's window and stores the result (NULL for
/// none).
fn navigate(
    this: *mut Walker,
    el: *mut c_void,
    out: *mut *mut c_void,
    step: fn(*mut Condition, Hwnd) -> Hwnd,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    let Some(h) = hwnd_of(el) else { return E_INVALIDARG };
    if !element::alive(h) {
        return crate::UIA_E_ELEMENTNOTAVAILABLE;
    }
    match step(cond_of(this), h) {
        0 => S_OK,
        found => put_element(out, found),
    }
}

fn first_child(c: *mut Condition, h: Hwnd) -> Hwnd {
    let mut list = List::new();
    view_children(c, h, &mut list, 0);
    list.as_slice().first().copied().unwrap_or(0)
}

fn last_child(c: *mut Condition, h: Hwnd) -> Hwnd {
    let mut list = List::new();
    view_children(c, h, &mut list, 0);
    list.as_slice().last().copied().unwrap_or(0)
}

fn sibling(c: *mut Condition, h: Hwnd, next: bool) -> Hwnd {
    let p = view_parent(c, h);
    if p == 0 {
        return 0;
    }
    let mut list = List::new();
    view_children(c, p, &mut list, 0);
    let items = list.as_slice();
    let Some(at) = items.iter().position(|&x| x == h) else { return 0 };
    let to = if next { at.checked_add(1) } else { at.checked_sub(1) };
    to.and_then(|i| items.get(i)).copied().unwrap_or(0)
}

fn next_sibling(c: *mut Condition, h: Hwnd) -> Hwnd {
    sibling(c, h, true)
}

fn previous_sibling(c: *mut Condition, h: Hwnd) -> Hwnd {
    sibling(c, h, false)
}

fn normalize(c: *mut Condition, h: Hwnd) -> Hwnd {
    if in_view(c, h) { h } else { view_parent(c, h) }
}

extern "system" fn get_parent(
    this: *mut Walker,
    el: *mut c_void,
    out: *mut *mut c_void,
) -> HResult {
    navigate(this, el, out, view_parent)
}

extern "system" fn get_first_child(
    this: *mut Walker,
    el: *mut c_void,
    out: *mut *mut c_void,
) -> HResult {
    navigate(this, el, out, first_child)
}

extern "system" fn get_last_child(
    this: *mut Walker,
    el: *mut c_void,
    out: *mut *mut c_void,
) -> HResult {
    navigate(this, el, out, last_child)
}

extern "system" fn get_next_sibling(
    this: *mut Walker,
    el: *mut c_void,
    out: *mut *mut c_void,
) -> HResult {
    navigate(this, el, out, next_sibling)
}

extern "system" fn get_previous_sibling(
    this: *mut Walker,
    el: *mut c_void,
    out: *mut *mut c_void,
) -> HResult {
    navigate(this, el, out, previous_sibling)
}

extern "system" fn get_normalized(
    this: *mut Walker,
    el: *mut c_void,
    out: *mut *mut c_void,
) -> HResult {
    navigate(this, el, out, normalize)
}

// The BuildCache forms: the cache request is not used (no cache).
extern "system" fn get_parent_cache(
    this: *mut Walker,
    el: *mut c_void,
    _c: usize,
    out: *mut *mut c_void,
) -> HResult {
    get_parent(this, el, out)
}

extern "system" fn get_first_child_cache(
    this: *mut Walker,
    el: *mut c_void,
    _c: usize,
    out: *mut *mut c_void,
) -> HResult {
    get_first_child(this, el, out)
}

extern "system" fn get_last_child_cache(
    this: *mut Walker,
    el: *mut c_void,
    _c: usize,
    out: *mut *mut c_void,
) -> HResult {
    get_last_child(this, el, out)
}

extern "system" fn get_next_sibling_cache(
    this: *mut Walker,
    el: *mut c_void,
    _c: usize,
    out: *mut *mut c_void,
) -> HResult {
    get_next_sibling(this, el, out)
}

extern "system" fn get_previous_sibling_cache(
    this: *mut Walker,
    el: *mut c_void,
    _c: usize,
    out: *mut *mut c_void,
) -> HResult {
    get_previous_sibling(this, el, out)
}

extern "system" fn get_normalized_cache(
    this: *mut Walker,
    el: *mut c_void,
    _c: usize,
    out: *mut *mut c_void,
) -> HResult {
    get_normalized(this, el, out)
}

extern "system" fn get_condition(this: *mut Walker, out: *mut *mut Condition) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    let c = cond_of(this);
    condition::add_ref(c);
    put(out, c);
    S_OK
}

extern "system" fn query(this: *mut Walker, iid: *const Guid, out: *mut *mut c_void) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    match iid_at(iid) {
        IID_IUNKNOWN | IID_IUIAUTOMATIONTREEWALKER => {
            add_ref(this);
            put(out, this.cast());
            S_OK
        }
        _ => {
            put(out, core::ptr::null_mut());
            E_NOINTERFACE
        }
    }
}

extern "system" fn add_ref(this: *mut Walker) -> u32 {
    // SAFETY: our walker.
    unsafe { (*this).refs.fetch_add(1, Ordering::Relaxed) + 1 }
}

extern "system" fn release(this: *mut Walker) -> u32 {
    // SAFETY: our walker, freed (with its condition reference) with its
    // last reference.
    unsafe {
        let left = (*this).refs.fetch_sub(1, Ordering::AcqRel) - 1;
        if left == 0 {
            condition::release((*this).cond);
            free(this.cast());
        }
        left
    }
}

static VTABLE: VTable<16> = VTable([
    query as *const (),
    add_ref as *const (),
    release as *const (),
    get_parent as *const (),
    get_first_child as *const (),
    get_last_child as *const (),
    get_next_sibling as *const (),
    get_previous_sibling as *const (),
    get_normalized as *const (),
    get_parent_cache as *const (),
    get_first_child_cache as *const (),
    get_last_child_cache as *const (),
    get_next_sibling_cache as *const (),
    get_previous_sibling_cache as *const (),
    get_normalized_cache as *const (),
    get_condition as *const (),
]);

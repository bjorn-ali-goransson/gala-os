//! Control patterns over standard controls: Invoke (push buttons), Value
//! (edits) and Toggle (check boxes). A pattern object keeps the element's
//! window and acts on it with the control's messages.

use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, Ordering};

use crate::element::{self, BUTTON, CHECK_BOX, EDIT};
use crate::{
    E_NOINTERFACE, E_POINTER, Guid, HResult, Hwnd, IID_IUNKNOWN, IsWindowEnabled, PostMessageW,
    S_OK, SendMessageW, UIA_E_ELEMENTNOTAVAILABLE, UIA_E_ELEMENTNOTENABLED, UIA_E_INVALIDOPERATION,
    VTable, Variant, boxed, bstr, free, guid, iid_at, put,
};

pub const INVOKE: i32 = 10000;
pub const VALUE: i32 = 10002;
pub const TOGGLE: i32 = 10015;

const IID_IUIAUTOMATIONINVOKEPATTERN: Guid =
    guid(0xfb377fbe, 0x8ea6, 0x46d5, [0x9c, 0x73, 0x64, 0x99, 0x64, 0x2d, 0x30, 0x59]);
const IID_IUIAUTOMATIONVALUEPATTERN: Guid =
    guid(0xa94cd8b1, 0x0844, 0x4cd6, [0x9d, 0x2d, 0x64, 0x05, 0x37, 0xab, 0x39, 0xe9]);
const IID_IUIAUTOMATIONTOGGLEPATTERN: Guid =
    guid(0x94cf8058, 0x9b8d, 0x4ab9, [0x8b, 0xfd, 0x4c, 0xd0, 0xa3, 0x3c, 0x8c, 0x70]);

// Pattern properties.
const VALUE_VALUE: i32 = 30045;
const VALUE_IS_READ_ONLY: i32 = 30046;
const TOGGLE_STATE: i32 = 30086;

const WM_SETTEXT: u32 = 0x000c;
const BM_GETCHECK: u32 = 0x00f0;
const BM_CLICK: u32 = 0x00f5;
const ES_READONLY: u32 = 0x0800;

/// Whether the element has the pattern.
pub fn supports(h: Hwnd, id: i32) -> bool {
    if h == element::desktop() {
        return false;
    }
    match id {
        INVOKE => element::control_type(h) == BUTTON,
        VALUE => element::control_type(h) == EDIT,
        TOGGLE => element::control_type(h) == CHECK_BOX,
        _ => false,
    }
}

fn read_only(h: Hwnd) -> bool {
    element::style(h) & ES_READONLY != 0
}

fn toggle_state(h: Hwnd) -> i32 {
    // BST_UNCHECKED 0 (ToggleState_Off), BST_CHECKED 1 (_On),
    // BST_INDETERMINATE 2 (_Indeterminate).
    // SAFETY: a plain call.
    (unsafe { SendMessageW(h, BM_GETCHECK, 0, 0) } as i32).clamp(0, 2)
}

/// A pattern property's value: None if `id` is no pattern property this
/// file knows; VT_EMPTY if the element lacks the pattern.
pub fn property(h: Hwnd, id: i32) -> Option<Variant> {
    Some(match id {
        VALUE_VALUE if supports(h, VALUE) => Variant::bstr(element::window_text(h, false)),
        VALUE_IS_READ_ONLY if supports(h, VALUE) => Variant::boolean(read_only(h)),
        TOGGLE_STATE if supports(h, TOGGLE) => Variant::int(toggle_state(h)),
        VALUE_VALUE | VALUE_IS_READ_ONLY | TOGGLE_STATE => Variant::EMPTY,
        _ => return None,
    })
}

#[repr(C)]
struct Pattern {
    vtable: *const (),
    refs: AtomicU32,
    hwnd: Hwnd,
    iid: Guid,
}

/// A new pattern object (one reference) for the element, or null when it
/// lacks the pattern (or memory runs out).
pub fn create(h: Hwnd, id: i32) -> *mut c_void {
    if !supports(h, id) {
        return core::ptr::null_mut();
    }
    let (vtable, iid): (*const (), Guid) = match id {
        INVOKE => ((&INVOKE_VTABLE as *const VTable<4>).cast(), IID_IUIAUTOMATIONINVOKEPATTERN),
        VALUE => ((&VALUE_VTABLE as *const VTable<8>).cast(), IID_IUIAUTOMATIONVALUEPATTERN),
        _ => ((&TOGGLE_VTABLE as *const VTable<6>).cast(), IID_IUIAUTOMATIONTOGGLEPATTERN),
    };
    boxed(Pattern { vtable, refs: AtomicU32::new(1), hwnd: h, iid }).cast()
}

pub fn query(p: *mut c_void, iid: *const Guid, out: *mut *mut c_void) -> HResult {
    pattern_query(p.cast(), iid, out)
}

pub fn release(p: *mut c_void) {
    pattern_release(p.cast());
}

extern "system" fn pattern_query(
    this: *mut Pattern,
    iid: *const Guid,
    out: *mut *mut c_void,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    let iid = iid_at(iid);
    // SAFETY: our pattern.
    if iid == IID_IUNKNOWN || iid == unsafe { (*this).iid } {
        pattern_add_ref(this);
        put(out, this.cast());
        S_OK
    } else {
        put(out, core::ptr::null_mut());
        E_NOINTERFACE
    }
}

extern "system" fn pattern_add_ref(this: *mut Pattern) -> u32 {
    // SAFETY: our pattern.
    unsafe { (*this).refs.fetch_add(1, Ordering::Relaxed) + 1 }
}

extern "system" fn pattern_release(this: *mut Pattern) -> u32 {
    // SAFETY: our pattern, freed with its last reference.
    unsafe {
        let left = (*this).refs.fetch_sub(1, Ordering::AcqRel) - 1;
        if left == 0 {
            free(this.cast());
        }
        left
    }
}

/// The window if it can be acted on: alive and enabled.
fn usable(this: *mut Pattern) -> Result<Hwnd, HResult> {
    // SAFETY: our pattern.
    let h = unsafe { (*this).hwnd };
    if !element::alive(h) {
        return Err(UIA_E_ELEMENTNOTAVAILABLE);
    }
    // SAFETY: a plain call.
    if unsafe { IsWindowEnabled(h) } == 0 {
        return Err(UIA_E_ELEMENTNOTENABLED);
    }
    Ok(h)
}

fn alive(this: *mut Pattern) -> Result<Hwnd, HResult> {
    // SAFETY: our pattern.
    let h = unsafe { (*this).hwnd };
    if element::alive(h) { Ok(h) } else { Err(UIA_E_ELEMENTNOTAVAILABLE) }
}

// ---- Invoke ----

/// Invoke: a click posted to the button (Invoke is documented to return
/// possibly before the control has acted, so a dialog the click opens
/// does not block the client).
extern "system" fn invoke(this: *mut Pattern) -> HResult {
    match usable(this) {
        // SAFETY: a plain call.
        Ok(h) => {
            unsafe { PostMessageW(h, BM_CLICK, 0, 0) };
            S_OK
        }
        Err(e) => e,
    }
}

static INVOKE_VTABLE: VTable<4> = VTable([
    pattern_query as *const (),
    pattern_add_ref as *const (),
    pattern_release as *const (),
    invoke as *const (),
]);

// ---- Value ----

extern "system" fn set_value(this: *mut Pattern, value: *const u16) -> HResult {
    let h = match usable(this) {
        Ok(h) => h,
        Err(e) => return e,
    };
    if read_only(h) {
        return UIA_E_INVALIDOPERATION;
    }
    // A BSTR is NUL-terminated; NULL is the empty string.
    let empty = [0u16];
    let text = if value.is_null() { empty.as_ptr() } else { value };
    // SAFETY: a plain call with a NUL-terminated string.
    unsafe { SendMessageW(h, WM_SETTEXT, 0, text as isize) };
    S_OK
}

extern "system" fn get_value(this: *mut Pattern, out: *mut *mut u16) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    match alive(this) {
        Ok(h) => {
            let s = element::window_text(h, false);
            put(out, if s.is_null() { bstr(&[]) } else { s });
            S_OK
        }
        Err(e) => e,
    }
}

extern "system" fn get_read_only(this: *mut Pattern, out: *mut i32) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, 0);
    match alive(this) {
        Ok(h) => {
            put(out, i32::from(read_only(h)));
            S_OK
        }
        Err(e) => e,
    }
}

/// The cached getters read current values (no cache).
static VALUE_VTABLE: VTable<8> = VTable([
    pattern_query as *const (),
    pattern_add_ref as *const (),
    pattern_release as *const (),
    set_value as *const (),
    get_value as *const (),
    get_read_only as *const (),
    get_value as *const (),
    get_read_only as *const (),
]);

// ---- Toggle ----

/// Toggle: a click (sent, so the new state can be read at once).
extern "system" fn toggle(this: *mut Pattern) -> HResult {
    match usable(this) {
        // SAFETY: a plain call.
        Ok(h) => {
            unsafe { SendMessageW(h, BM_CLICK, 0, 0) };
            S_OK
        }
        Err(e) => e,
    }
}

extern "system" fn get_toggle_state(this: *mut Pattern, out: *mut i32) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, 0);
    match alive(this) {
        Ok(h) => {
            put(out, toggle_state(h));
            S_OK
        }
        Err(e) => e,
    }
}

static TOGGLE_VTABLE: VTable<6> = VTable([
    pattern_query as *const (),
    pattern_add_ref as *const (),
    pattern_release as *const (),
    toggle as *const (),
    get_toggle_state as *const (),
    get_toggle_state as *const (),
]);

//! Cache requests (IUIAutomationCacheRequest): what a client asks to be
//! cached with the elements a BuildCache call returns. gala's elements
//! read every value live, so the Cached getters already answer with
//! current values; a request only keeps what it was told (properties,
//! patterns, scope, filter, mode) so clients can read it back and clone
//! it, as documented.

use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, Ordering};

use crate::condition;
use crate::{
    E_INVALIDARG, E_NOINTERFACE, E_OUTOFMEMORY, E_POINTER, Guid, HResult, IID_IUNKNOWN, List, S_OK,
    VTable, boxed, free, guid, iid_at, put,
};

const IID_IUIAUTOMATIONCACHEREQUEST: Guid =
    guid(0xb32a92b5, 0xbc25, 0x4078, [0x9c, 0x08, 0xd7, 0xee, 0x95, 0xc4, 0x8e, 0x03]);

const TREE_SCOPE_ELEMENT: i32 = 1;
const AUTOMATION_ELEMENT_MODE_FULL: i32 = 1;

#[repr(C)]
struct CacheRequest {
    vtable: *const VTable<12>,
    refs: AtomicU32,
    scope: i32,
    mode: i32,
    /// The tree filter (one reference held).
    filter: *mut c_void,
    properties: List,
    patterns: List,
}

/// A new cache request at `out` (scope Element, mode Full, the control
/// view as its filter: the documented defaults).
pub fn create(out: *mut *mut c_void) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    let mut filter = core::ptr::null_mut();
    let hr = crate::automation::control_view(&mut filter);
    if hr != S_OK {
        return hr;
    }
    make(TREE_SCOPE_ELEMENT, AUTOMATION_ELEMENT_MODE_FULL, filter, List::new(), List::new(), out)
}

/// A request from its parts; takes the filter's reference.
fn make(
    scope: i32,
    mode: i32,
    filter: *mut c_void,
    properties: List,
    patterns: List,
    out: *mut *mut c_void,
) -> HResult {
    let c = boxed(CacheRequest {
        vtable: &VTABLE,
        refs: AtomicU32::new(1),
        scope,
        mode,
        filter,
        properties,
        patterns,
    });
    if c.is_null() {
        release_condition(filter);
        return E_OUTOFMEMORY;
    }
    put(out, c.cast());
    S_OK
}

fn release_condition(c: *mut c_void) {
    if let Some(c) = condition::as_condition(c) {
        condition::release(c);
    }
}

extern "system" fn query(
    this: *mut CacheRequest,
    iid: *const Guid,
    out: *mut *mut c_void,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    match iid_at(iid) {
        IID_IUNKNOWN | IID_IUIAUTOMATIONCACHEREQUEST => {
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

extern "system" fn add_ref(this: *mut CacheRequest) -> u32 {
    // SAFETY: our request.
    unsafe { (*this).refs.fetch_add(1, Ordering::Relaxed) + 1 }
}

extern "system" fn release(this: *mut CacheRequest) -> u32 {
    // SAFETY: our request, dropped and freed with its last reference.
    unsafe {
        let left = (*this).refs.fetch_sub(1, Ordering::AcqRel) - 1;
        if left == 0 {
            release_condition((*this).filter);
            core::ptr::drop_in_place(this);
            free(this.cast());
        }
        left
    }
}

/// AddProperty: property ids are 30000 and up (custom ones too).
extern "system" fn add_property(this: *mut CacheRequest, id: i32) -> HResult {
    if id < 30000 {
        return E_INVALIDARG;
    }
    // SAFETY: our request.
    if unsafe { (*this).properties.push(id as usize) } { S_OK } else { E_OUTOFMEMORY }
}

/// AddPattern: pattern ids are 10000 and up.
extern "system" fn add_pattern(this: *mut CacheRequest, id: i32) -> HResult {
    if id < 10000 {
        return E_INVALIDARG;
    }
    // SAFETY: our request.
    if unsafe { (*this).patterns.push(id as usize) } { S_OK } else { E_OUTOFMEMORY }
}

fn copy(list: &List) -> Option<List> {
    let mut c = List::new();
    for &v in list.as_slice() {
        if !c.push(v) {
            return None;
        }
    }
    Some(c)
}

extern "system" fn clone(this: *mut CacheRequest, out: *mut *mut c_void) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    // SAFETY: our request.
    let r = unsafe { &*this };
    let (Some(properties), Some(patterns)) = (copy(&r.properties), copy(&r.patterns)) else {
        return E_OUTOFMEMORY;
    };
    if let Some(c) = condition::as_condition(r.filter) {
        condition::add_ref(c);
    }
    make(r.scope, r.mode, r.filter, properties, patterns, out)
}

extern "system" fn get_scope(this: *mut CacheRequest, out: *mut i32) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: our request.
    put(out, unsafe { (*this).scope });
    S_OK
}

/// put_TreeScope: Element, Children, Descendants (and Subtree); Parent
/// and Ancestors are not supported, as documented.
extern "system" fn put_scope(this: *mut CacheRequest, scope: i32) -> HResult {
    if scope & !7 != 0 || scope & 7 == 0 {
        return E_INVALIDARG;
    }
    // SAFETY: our request.
    unsafe { (*this).scope = scope };
    S_OK
}

extern "system" fn get_filter(this: *mut CacheRequest, out: *mut *mut c_void) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: our request.
    let f = unsafe { (*this).filter };
    if let Some(c) = condition::as_condition(f) {
        condition::add_ref(c);
    }
    put(out, f);
    S_OK
}

/// put_TreeFilter: one of our conditions.
extern "system" fn put_filter(this: *mut CacheRequest, filter: *mut c_void) -> HResult {
    let Some(c) = condition::as_condition(filter) else { return E_INVALIDARG };
    condition::add_ref(c);
    // SAFETY: our request.
    unsafe {
        release_condition((*this).filter);
        (*this).filter = filter;
    }
    S_OK
}

extern "system" fn get_mode(this: *mut CacheRequest, out: *mut i32) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: our request.
    put(out, unsafe { (*this).mode });
    S_OK
}

/// put_AutomationElementMode: None (0) or Full (1).
extern "system" fn put_mode(this: *mut CacheRequest, mode: i32) -> HResult {
    if !(0..=1).contains(&mode) {
        return E_INVALIDARG;
    }
    // SAFETY: our request.
    unsafe { (*this).mode = mode };
    S_OK
}

static VTABLE: VTable<12> = VTable([
    query as *const (),
    add_ref as *const (),
    release as *const (),
    add_property as *const (),
    add_pattern as *const (),
    clone as *const (),
    get_scope as *const (),
    put_scope as *const (),
    get_filter as *const (),
    put_filter as *const (),
    get_mode as *const (),
    put_mode as *const (),
]);

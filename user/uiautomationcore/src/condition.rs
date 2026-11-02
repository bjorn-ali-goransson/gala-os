//! Conditions (IUIAutomationCondition and its kinds): true and false,
//! property conditions (the property's current value compared with a
//! VARIANT: strings exactly, or ignoring case or as a substring by the
//! flags; integers; booleans), and, or, not. One object type serves all
//! kinds; each kind has its own vtable (and IID).

use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, Ordering};

use crate::element;
use crate::{
    CoTaskMemAlloc, E_INVALIDARG, E_NOINTERFACE, E_OUTOFMEMORY, E_POINTER, Guid, HResult, Hwnd,
    IID_IUNKNOWN, S_OK, VT_ARRAY, VT_BOOL, VT_BSTR, VT_EMPTY, VT_I4, VTable, Variant, VariantCopy,
    alloc, boxed, bstr_chars, free, guid, iid_at, missing, put,
};

const IID_IUIAUTOMATIONCONDITION: Guid =
    guid(0x352ffba8, 0x0973, 0x437c, [0xa6, 0x1f, 0xf6, 0x4c, 0xaf, 0xd8, 0x1d, 0xf9]);
const IID_IUIAUTOMATIONBOOLCONDITION: Guid =
    guid(0x1b4e1f2e, 0x75eb, 0x4d0b, [0x89, 0x52, 0x5a, 0x69, 0x98, 0x8e, 0x23, 0x07]);
const IID_IUIAUTOMATIONPROPERTYCONDITION: Guid =
    guid(0x99ebf2cb, 0x5578, 0x4267, [0x9a, 0xd4, 0xaf, 0xd6, 0xea, 0x77, 0xe9, 0x4b]);
const IID_IUIAUTOMATIONANDCONDITION: Guid =
    guid(0xa7d0af36, 0xb912, 0x45fe, [0x98, 0x55, 0x09, 0x1d, 0xdc, 0x17, 0x4a, 0xec]);
const IID_IUIAUTOMATIONORCONDITION: Guid =
    guid(0x8753f032, 0x3db1, 0x47b5, [0xa1, 0xfc, 0x6e, 0x34, 0xa2, 0x66, 0xc7, 0x12]);
const IID_IUIAUTOMATIONNOTCONDITION: Guid =
    guid(0xf528b657, 0x847b, 0x498c, [0x88, 0x96, 0xd5, 0x2b, 0x56, 0x54, 0x07, 0xa1]);

/// PropertyConditionFlags.
const IGNORE_CASE: u32 = 1;
const MATCH_SUBSTRING: u32 = 2;

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    True,
    False,
    Property,
    And,
    Or,
    Not,
}

#[repr(C)]
pub struct Condition {
    vtable: *const (),
    refs: AtomicU32,
    kind: Kind,
    property: i32,
    flags: u32,
    value: Variant,
    /// And/or: the conditions (each with a reference); not: one.
    children: *mut *mut Condition,
    count: usize,
}

/// One of our conditions (None for anything else).
pub fn as_condition(p: *mut c_void) -> Option<*mut Condition> {
    if p.is_null() {
        return None;
    }
    // SAFETY: an interface pointer, whose first field is its vtable.
    let v = unsafe { *p.cast::<*const ()>() };
    let ours = [
        (&BOOL_VTABLE as *const VTable<4>).cast::<()>(),
        (&PROPERTY_VTABLE as *const VTable<6>).cast(),
        (&AND_VTABLE as *const VTable<6>).cast(),
        (&OR_VTABLE as *const VTable<6>).cast(),
        (&NOT_VTABLE as *const VTable<4>).cast(),
    ];
    ours.contains(&v).then_some(p.cast())
}

fn make(kind: Kind, property: i32, flags: u32, children: &[*mut Condition]) -> *mut Condition {
    let vtable: *const () = match kind {
        Kind::True | Kind::False => (&BOOL_VTABLE as *const VTable<4>).cast(),
        Kind::Property => (&PROPERTY_VTABLE as *const VTable<6>).cast(),
        Kind::And => (&AND_VTABLE as *const VTable<6>).cast(),
        Kind::Or => (&OR_VTABLE as *const VTable<6>).cast(),
        Kind::Not => (&NOT_VTABLE as *const VTable<4>).cast(),
    };
    let list = if children.is_empty() {
        core::ptr::null_mut()
    } else {
        let p = alloc(size_of_val(children)).cast::<*mut Condition>();
        if p.is_null() {
            return p.cast();
        }
        for (i, &c) in children.iter().enumerate() {
            add_ref(c);
            // SAFETY: room for `children.len()` pointers.
            unsafe { p.add(i).write(c) };
        }
        p
    };
    let c = boxed(Condition {
        vtable,
        refs: AtomicU32::new(1),
        kind,
        property,
        flags,
        value: Variant::EMPTY,
        children: list,
        count: children.len(),
    });
    if c.is_null() {
        for &child in children {
            release(child);
        }
        free(list.cast());
    }
    c
}

fn finish(out: *mut *mut c_void, c: *mut Condition) -> HResult {
    put(out, c.cast());
    if c.is_null() { E_OUTOFMEMORY } else { S_OK }
}

pub fn create_bool(value: bool, out: *mut *mut c_void) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    finish(out, make(if value { Kind::True } else { Kind::False }, 0, 0, &[]))
}

pub fn create_property(
    property: i32,
    value: *const Variant,
    flags: u32,
    out: *mut *mut c_void,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    if value.is_null() || flags & !(IGNORE_CASE | MATCH_SUBSTRING) != 0 {
        return E_INVALIDARG;
    }
    let c = make(Kind::Property, property, flags, &[]);
    if c.is_null() {
        return E_OUTOFMEMORY;
    }
    // SAFETY: our new condition and the caller's VARIANT.
    let hr = unsafe { VariantCopy(&mut (*c).value, value) };
    if hr < 0 {
        release(c);
        return hr;
    }
    finish(out, c)
}

/// And/or of the caller's conditions; not of one.
fn create_group(kind: Kind, items: &[*mut c_void], out: *mut *mut c_void) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    let mut conditions = [core::ptr::null_mut(); 64];
    if items.len() > conditions.len() {
        missing(b"UIAutomationCore.dll!CreateAndCondition(over 64)", items.len() as u64);
        return E_INVALIDARG;
    }
    for (slot, &item) in conditions.iter_mut().zip(items) {
        let Some(c) = as_condition(item) else { return E_INVALIDARG };
        *slot = c;
    }
    finish(out, make(kind, 0, 0, &conditions[..items.len()]))
}

pub fn create_and(items: &[*mut c_void], out: *mut *mut c_void) -> HResult {
    create_group(Kind::And, items, out)
}

pub fn create_or(items: &[*mut c_void], out: *mut *mut c_void) -> HResult {
    create_group(Kind::Or, items, out)
}

pub fn create_not(item: *mut c_void, out: *mut *mut c_void) -> HResult {
    create_group(Kind::Not, &[item], out)
}

// ---- Evaluation ----

fn fold(c: u16) -> u16 {
    if (u16::from(b'A')..=u16::from(b'Z')).contains(&c) { c + 32 } else { c }
}

fn text_matches(actual: &[u16], wanted: &[u16], flags: u32) -> bool {
    let same = |a: &[u16]| {
        a.len() == wanted.len()
            && a.iter()
                .zip(wanted)
                .all(|(&x, &y)| if flags & IGNORE_CASE != 0 { fold(x) == fold(y) } else { x == y })
    };
    if flags & MATCH_SUBSTRING != 0 {
        wanted.is_empty()
            || (wanted.len() <= actual.len() && actual.windows(wanted.len()).any(same))
    } else {
        same(actual)
    }
}

fn equal(actual: &Variant, wanted: &Variant, flags: u32) -> bool {
    match (actual.vt, wanted.vt) {
        (VT_BSTR, VT_BSTR) => {
            text_matches(bstr_chars(actual.as_bstr()), bstr_chars(wanted.as_bstr()), flags)
        }
        (VT_I4, VT_I4) => actual.as_int() == wanted.as_int(),
        (VT_BOOL, VT_BOOL) => actual.as_bool() == wanted.as_bool(),
        (VT_EMPTY, VT_EMPTY) => true,
        (a, b) if a & VT_ARRAY != 0 && a == b => {
            missing(b"UIAutomationCore.dll!PropertyCondition(array value)", u64::from(a));
            false
        }
        _ => false,
    }
}

fn child(c: *mut Condition, i: usize) -> *mut Condition {
    // SAFETY: our condition's list of `count` children.
    unsafe { *(*c).children.add(i) }
}

/// Whether the element `h` meets the condition.
pub fn matches(c: *mut Condition, h: Hwnd) -> bool {
    // SAFETY: our condition.
    let (kind, count) = unsafe { ((*c).kind, (*c).count) };
    match kind {
        Kind::True => true,
        Kind::False => false,
        Kind::Property => {
            // SAFETY: as above.
            let (property, flags, wanted) = unsafe { ((*c).property, (*c).flags, &(*c).value) };
            let mut actual = element::property(h, property);
            let r = equal(&actual, wanted, flags);
            actual.clear();
            r
        }
        Kind::And => (0..count).all(|i| matches(child(c, i), h)),
        Kind::Or => (0..count).any(|i| matches(child(c, i), h)),
        Kind::Not => !matches(child(c, 0), h),
    }
}

// ---- The COM object ----

extern "system" fn query(this: *mut Condition, iid: *const Guid, out: *mut *mut c_void) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: our condition.
    let own = match unsafe { (*this).kind } {
        Kind::True | Kind::False => IID_IUIAUTOMATIONBOOLCONDITION,
        Kind::Property => IID_IUIAUTOMATIONPROPERTYCONDITION,
        Kind::And => IID_IUIAUTOMATIONANDCONDITION,
        Kind::Or => IID_IUIAUTOMATIONORCONDITION,
        Kind::Not => IID_IUIAUTOMATIONNOTCONDITION,
    };
    let iid = iid_at(iid);
    if iid == IID_IUNKNOWN || iid == IID_IUIAUTOMATIONCONDITION || iid == own {
        add_ref(this);
        put(out, this.cast());
        S_OK
    } else {
        put(out, core::ptr::null_mut());
        E_NOINTERFACE
    }
}

pub extern "system" fn add_ref(this: *mut Condition) -> u32 {
    // SAFETY: our condition.
    unsafe { (*this).refs.fetch_add(1, Ordering::Relaxed) + 1 }
}

pub extern "system" fn release(this: *mut Condition) -> u32 {
    // SAFETY: our condition, freed (with what it holds) with its last
    // reference.
    unsafe {
        let left = (*this).refs.fetch_sub(1, Ordering::AcqRel) - 1;
        if left == 0 {
            (*this).value.clear();
            for i in 0..(*this).count {
                release(child(this, i));
            }
            free((*this).children.cast());
            free(this.cast());
        }
        left
    }
}

extern "system" fn boolean_value(this: *mut Condition, out: *mut i32) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: our condition.
    put(out, i32::from(unsafe { (*this).kind } == Kind::True));
    S_OK
}

extern "system" fn property_id(this: *mut Condition, out: *mut i32) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: our condition.
    put(out, unsafe { (*this).property });
    S_OK
}

extern "system" fn property_value(this: *mut Condition, out: *mut Variant) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, Variant::EMPTY);
    // SAFETY: our condition's VARIANT copied to the caller's.
    unsafe { VariantCopy(out, &(*this).value) }
}

extern "system" fn property_flags(this: *mut Condition, out: *mut u32) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: our condition.
    put(out, unsafe { (*this).flags });
    S_OK
}

extern "system" fn child_count(this: *mut Condition, out: *mut i32) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: our condition.
    put(out, unsafe { (*this).count } as i32);
    S_OK
}

/// GetChildrenAsNativeArray: a CoTaskMemAlloc'd array of the children,
/// each with a reference for the caller.
extern "system" fn children_native(
    this: *mut Condition,
    out: *mut *mut *mut Condition,
    count: *mut i32,
) -> HResult {
    if out.is_null() || count.is_null() {
        return E_POINTER;
    }
    // SAFETY: our condition.
    let n = unsafe { (*this).count };
    // SAFETY: a plain call.
    let p = unsafe { CoTaskMemAlloc(n.max(1) * size_of::<usize>()) }.cast::<*mut Condition>();
    if p.is_null() {
        put(out, core::ptr::null_mut());
        put(count, 0);
        return E_OUTOFMEMORY;
    }
    for i in 0..n {
        let c = child(this, i);
        add_ref(c);
        // SAFETY: room for `n` pointers.
        unsafe { p.add(i).write(c) };
    }
    put(out, p);
    put(count, n as i32);
    S_OK
}

not_impl!(children_safearray, b"UIAutomationCore.dll!IUIAutomationAndCondition::GetChildren");

extern "system" fn not_child(this: *mut Condition, out: *mut *mut Condition) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    let c = child(this, 0);
    add_ref(c);
    put(out, c);
    S_OK
}

static BOOL_VTABLE: VTable<4> = VTable([
    query as *const (),
    add_ref as *const (),
    release as *const (),
    boolean_value as *const (),
]);

static PROPERTY_VTABLE: VTable<6> = VTable([
    query as *const (),
    add_ref as *const (),
    release as *const (),
    property_id as *const (),
    property_value as *const (),
    property_flags as *const (),
]);

static AND_VTABLE: VTable<6> = VTable([
    query as *const (),
    add_ref as *const (),
    release as *const (),
    child_count as *const (),
    children_native as *const (),
    children_safearray as *const (),
]);

/// The same methods as and's (a distinct table tells the kinds apart).
static OR_VTABLE: VTable<6> = VTable([
    query as *const (),
    add_ref as *const (),
    release as *const (),
    child_count as *const (),
    children_native as *const (),
    children_safearray as *const (),
]);

static NOT_VTABLE: VTable<4> = VTable([
    query as *const (),
    add_ref as *const (),
    release as *const (),
    not_child as *const (),
]);

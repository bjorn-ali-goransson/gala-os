//! The CUIAutomation object (IUIAutomation): elements from windows, the
//! root, points and the focus; conditions; tree walkers.

use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, Ordering};

use crate::element::{self, desktop, hwnd_of, put_element};
use crate::{
    E_INVALIDARG, E_NOINTERFACE, E_OUTOFMEMORY, E_POINTER, GetFocus, Guid, HResult, Hwnd,
    IID_IUNKNOWN, IsWindow, Point, S_OK, VT_I4, VT_R8, VTable, Variant, WindowFromPoint, boxed,
    condition, free, guid, iid_at, put, walker,
};

const IID_IUIAUTOMATION: Guid =
    guid(0x30cbe57d, 0xd9d0, 0x452a, [0xab, 0x13, 0x7a, 0xc5, 0xac, 0x48, 0x25, 0xee]);
const IS_CONTROL_ELEMENT: i32 = 30016;
const IS_CONTENT_ELEMENT: i32 = 30017;

#[repr(C)]
struct Automation {
    vtable: *const VTable<58>,
    refs: AtomicU32,
}

/// A new CUIAutomation object's `iid` interface at `out`.
pub fn create(iid: *const Guid, out: *mut *mut c_void) -> HResult {
    let a = boxed(Automation { vtable: &VTABLE, refs: AtomicU32::new(1) });
    if a.is_null() {
        return E_OUTOFMEMORY;
    }
    let hr = query(a, iid, out);
    release(a);
    hr
}

extern "system" fn query(
    this: *mut Automation,
    iid: *const Guid,
    out: *mut *mut c_void,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    match iid_at(iid) {
        IID_IUNKNOWN | IID_IUIAUTOMATION => {
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

extern "system" fn add_ref(this: *mut Automation) -> u32 {
    // SAFETY: our object.
    unsafe { (*this).refs.fetch_add(1, Ordering::Relaxed) + 1 }
}

extern "system" fn release(this: *mut Automation) -> u32 {
    // SAFETY: our object, freed with its last reference.
    unsafe {
        let left = (*this).refs.fetch_sub(1, Ordering::AcqRel) - 1;
        if left == 0 {
            free(this.cast());
        }
        left
    }
}

// ---- Elements ----

extern "system" fn compare_elements(
    _this: usize,
    a: *mut c_void,
    b: *mut c_void,
    same: *mut i32,
) -> HResult {
    if same.is_null() {
        return E_POINTER;
    }
    let (Some(a), Some(b)) = (hwnd_of(a), hwnd_of(b)) else {
        put(same, 0);
        return E_INVALIDARG;
    };
    put(same, i32::from(a == b));
    S_OK
}

fn element_or_null(out: *mut *mut c_void, h: Hwnd) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put_element(out, h)
}

extern "system" fn root(_this: usize, out: *mut *mut c_void) -> HResult {
    element_or_null(out, desktop())
}

extern "system" fn from_handle(_this: usize, h: Hwnd, out: *mut *mut c_void) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    // SAFETY: a plain call.
    if h == 0 || unsafe { IsWindow(h) } == 0 {
        return E_INVALIDARG;
    }
    put_element(out, h)
}

/// ElementFromPoint (the POINT comes by value, in one register).
extern "system" fn from_point(_this: usize, point: Point, out: *mut *mut c_void) -> HResult {
    // SAFETY: a plain call.
    let h = unsafe { WindowFromPoint(point) };
    element_or_null(out, if h == 0 { desktop() } else { h })
}

extern "system" fn focused(_this: usize, out: *mut *mut c_void) -> HResult {
    // SAFETY: a plain call.
    let h = unsafe { GetFocus() };
    element_or_null(out, if h == 0 { desktop() } else { h })
}

// The BuildCache forms: the cache request is not used (no cache).
extern "system" fn root_cache(this: usize, _cache: usize, out: *mut *mut c_void) -> HResult {
    root(this, out)
}

extern "system" fn from_handle_cache(
    this: usize,
    h: Hwnd,
    _cache: usize,
    out: *mut *mut c_void,
) -> HResult {
    from_handle(this, h, out)
}

extern "system" fn from_point_cache(
    this: usize,
    point: Point,
    _cache: usize,
    out: *mut *mut c_void,
) -> HResult {
    from_point(this, point, out)
}

extern "system" fn focused_cache(this: usize, _cache: usize, out: *mut *mut c_void) -> HResult {
    focused(this, out)
}

// ---- Views and walkers ----

fn view_condition(kind: u32, out: *mut *mut c_void) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    match kind {
        // Raw: everything.
        0 => condition::create_bool(true, out),
        // Control: IsControlElement.
        1 => condition::create_property(IS_CONTROL_ELEMENT, &Variant::boolean(true), 0, out),
        // Content: IsControlElement and IsContentElement.
        _ => {
            let mut control = core::ptr::null_mut();
            let mut content = core::ptr::null_mut();
            let mut hr = condition::create_property(
                IS_CONTROL_ELEMENT,
                &Variant::boolean(true),
                0,
                &mut control,
            );
            if hr == S_OK {
                hr = condition::create_property(
                    IS_CONTENT_ELEMENT,
                    &Variant::boolean(true),
                    0,
                    &mut content,
                );
            }
            if hr == S_OK {
                hr = condition::create_and(&[control, content], out);
            }
            for c in [control, content] {
                if let Some(c) = condition::as_condition(c) {
                    condition::release(c);
                }
            }
            hr
        }
    }
}

fn view_walker(kind: u32, out: *mut *mut c_void) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    let mut c = core::ptr::null_mut();
    let hr = view_condition(kind, &mut c);
    if hr != S_OK {
        return hr;
    }
    let hr = walker::create(c, out);
    if let Some(c) = condition::as_condition(c) {
        condition::release(c);
    }
    hr
}

extern "system" fn create_tree_walker(
    _this: usize,
    cond: *mut c_void,
    out: *mut *mut c_void,
) -> HResult {
    walker::create(cond, out)
}

extern "system" fn control_view_walker(_this: usize, out: *mut *mut c_void) -> HResult {
    view_walker(1, out)
}

extern "system" fn content_view_walker(_this: usize, out: *mut *mut c_void) -> HResult {
    view_walker(2, out)
}

extern "system" fn raw_view_walker(_this: usize, out: *mut *mut c_void) -> HResult {
    view_walker(0, out)
}

extern "system" fn raw_view_condition(_this: usize, out: *mut *mut c_void) -> HResult {
    view_condition(0, out)
}

extern "system" fn control_view_condition(_this: usize, out: *mut *mut c_void) -> HResult {
    view_condition(1, out)
}

extern "system" fn content_view_condition(_this: usize, out: *mut *mut c_void) -> HResult {
    view_condition(2, out)
}

// ---- Conditions ----

extern "system" fn true_condition(_this: usize, out: *mut *mut c_void) -> HResult {
    condition::create_bool(true, out)
}

extern "system" fn false_condition(_this: usize, out: *mut *mut c_void) -> HResult {
    condition::create_bool(false, out)
}

/// CreatePropertyCondition (the VARIANT, 24 bytes, comes by reference).
extern "system" fn property_condition(
    _this: usize,
    id: i32,
    value: *const Variant,
    out: *mut *mut c_void,
) -> HResult {
    condition::create_property(id, value, 0, out)
}

extern "system" fn property_condition_ex(
    _this: usize,
    id: i32,
    value: *const Variant,
    flags: u32,
    out: *mut *mut c_void,
) -> HResult {
    condition::create_property(id, value, flags, out)
}

extern "system" fn and_condition(
    _this: usize,
    a: *mut c_void,
    b: *mut c_void,
    out: *mut *mut c_void,
) -> HResult {
    condition::create_and(&[a, b], out)
}

extern "system" fn or_condition(
    _this: usize,
    a: *mut c_void,
    b: *mut c_void,
    out: *mut *mut c_void,
) -> HResult {
    condition::create_or(&[a, b], out)
}

fn native_array<'a>(items: *const *mut c_void, count: i32) -> Option<&'a [*mut c_void]> {
    if count < 0 || (count > 0 && items.is_null()) {
        return None;
    }
    if count == 0 {
        return Some(&[]);
    }
    // SAFETY: the caller's array of `count` interface pointers.
    Some(unsafe { core::slice::from_raw_parts(items, count as usize) })
}

extern "system" fn and_native(
    _this: usize,
    items: *const *mut c_void,
    count: i32,
    out: *mut *mut c_void,
) -> HResult {
    match native_array(items, count) {
        Some(items) => condition::create_and(items, out),
        None => E_INVALIDARG,
    }
}

extern "system" fn or_native(
    _this: usize,
    items: *const *mut c_void,
    count: i32,
    out: *mut *mut c_void,
) -> HResult {
    match native_array(items, count) {
        Some(items) => condition::create_or(items, out),
        None => E_INVALIDARG,
    }
}

extern "system" fn not_condition(_this: usize, c: *mut c_void, out: *mut *mut c_void) -> HResult {
    condition::create_not(c, out)
}

// ---- Helpers ----

extern "system" fn remove_all_event_handlers(_this: usize) -> HResult {
    S_OK
}

extern "system" fn int_array_to_safearray(
    _this: usize,
    items: *const i32,
    count: i32,
    out: *mut *mut c_void,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    if count < 0 || (count > 0 && items.is_null()) {
        return E_INVALIDARG;
    }
    let items: &[i32] = if count == 0 {
        &[]
    } else {
        // SAFETY: the caller's array of `count` ints.
        unsafe { core::slice::from_raw_parts(items, count as usize) }
    };
    let v = element::array(VT_I4, items);
    put(out, v.data[0] as usize as *mut c_void);
    if v.vt == 0 { E_OUTOFMEMORY } else { S_OK }
}

/// RectToVariant: VT_R8 | VT_ARRAY of left, top, width, height (the RECT,
/// 16 bytes, comes by reference).
extern "system" fn rect_to_variant(
    _this: usize,
    rect: *const [i32; 4],
    out: *mut Variant,
) -> HResult {
    if rect.is_null() || out.is_null() {
        return E_POINTER;
    }
    // SAFETY: the caller's RECT.
    let r = unsafe { *rect };
    let v = element::array(VT_R8, &[r[0], r[1], r[2] - r[0], r[3] - r[1]].map(f64::from));
    put(out, v);
    if v.vt == 0 { E_OUTOFMEMORY } else { S_OK }
}

/// CheckNotSupported: gala never gives out the not-supported value.
extern "system" fn check_not_supported(
    _this: usize,
    _value: *const Variant,
    out: *mut i32,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, 0);
    S_OK
}

not_impl!(compare_runtime_ids, b"UIAutomationCore.dll!IUIAutomation::CompareRuntimeIds");
not_impl!(create_cache_request, b"UIAutomationCore.dll!IUIAutomation::CreateCacheRequest");
not_impl!(and_from_array, b"UIAutomationCore.dll!IUIAutomation::CreateAndConditionFromArray");
not_impl!(or_from_array, b"UIAutomationCore.dll!IUIAutomation::CreateOrConditionFromArray");
not_impl!(add_event_handler, b"UIAutomationCore.dll!IUIAutomation::AddAutomationEventHandler");
not_impl!(
    remove_event_handler,
    b"UIAutomationCore.dll!IUIAutomation::RemoveAutomationEventHandler"
);
not_impl!(
    add_property_handler_native,
    b"UIAutomationCore.dll!IUIAutomation::AddPropertyChangedEventHandlerNativeArray"
);
not_impl!(
    add_property_handler,
    b"UIAutomationCore.dll!IUIAutomation::AddPropertyChangedEventHandler"
);
not_impl!(
    remove_property_handler,
    b"UIAutomationCore.dll!IUIAutomation::RemovePropertyChangedEventHandler"
);
not_impl!(
    add_structure_handler,
    b"UIAutomationCore.dll!IUIAutomation::AddStructureChangedEventHandler"
);
not_impl!(
    remove_structure_handler,
    b"UIAutomationCore.dll!IUIAutomation::RemoveStructureChangedEventHandler"
);
not_impl!(add_focus_handler, b"UIAutomationCore.dll!IUIAutomation::AddFocusChangedEventHandler");
not_impl!(
    remove_focus_handler,
    b"UIAutomationCore.dll!IUIAutomation::RemoveFocusChangedEventHandler"
);
not_impl!(
    int_safearray_to_native,
    b"UIAutomationCore.dll!IUIAutomation::IntSafeArrayToNativeArray"
);
not_impl!(variant_to_rect, b"UIAutomationCore.dll!IUIAutomation::VariantToRect");
not_impl!(safearray_to_rects, b"UIAutomationCore.dll!IUIAutomation::SafeArrayToRectNativeArray");
not_impl!(
    create_proxy_factory_entry,
    b"UIAutomationCore.dll!IUIAutomation::CreateProxyFactoryEntry"
);
not_impl!(proxy_factory_mapping, b"UIAutomationCore.dll!IUIAutomation::get_ProxyFactoryMapping");
not_impl!(property_name, b"UIAutomationCore.dll!IUIAutomation::GetPropertyProgrammaticName");
not_impl!(pattern_name, b"UIAutomationCore.dll!IUIAutomation::GetPatternProgrammaticName");
not_impl!(poll_patterns, b"UIAutomationCore.dll!IUIAutomation::PollForPotentialSupportedPatterns");
not_impl!(
    poll_properties,
    b"UIAutomationCore.dll!IUIAutomation::PollForPotentialSupportedProperties"
);
not_impl!(
    not_supported_value,
    b"UIAutomationCore.dll!IUIAutomation::get_ReservedNotSupportedValue"
);
not_impl!(
    mixed_attribute_value,
    b"UIAutomationCore.dll!IUIAutomation::get_ReservedMixedAttributeValue"
);
not_impl!(from_iaccessible, b"UIAutomationCore.dll!IUIAutomation::ElementFromIAccessible");
not_impl!(
    from_iaccessible_cache,
    b"UIAutomationCore.dll!IUIAutomation::ElementFromIAccessibleBuildCache"
);

static VTABLE: VTable<58> = VTable([
    query as *const (),
    add_ref as *const (),
    release as *const (),
    compare_elements as *const (),
    compare_runtime_ids as *const (),
    root as *const (),
    from_handle as *const (),
    from_point as *const (),
    focused as *const (),
    root_cache as *const (),
    from_handle_cache as *const (),
    from_point_cache as *const (),
    focused_cache as *const (),
    create_tree_walker as *const (),
    control_view_walker as *const (),
    content_view_walker as *const (),
    raw_view_walker as *const (),
    raw_view_condition as *const (),
    control_view_condition as *const (),
    content_view_condition as *const (),
    create_cache_request as *const (),
    true_condition as *const (),
    false_condition as *const (),
    property_condition as *const (),
    property_condition_ex as *const (),
    and_condition as *const (),
    and_from_array as *const (),
    and_native as *const (),
    or_condition as *const (),
    or_from_array as *const (),
    or_native as *const (),
    not_condition as *const (),
    add_event_handler as *const (),
    remove_event_handler as *const (),
    add_property_handler_native as *const (),
    add_property_handler as *const (),
    remove_property_handler as *const (),
    add_structure_handler as *const (),
    remove_structure_handler as *const (),
    add_focus_handler as *const (),
    remove_focus_handler as *const (),
    remove_all_event_handlers as *const (),
    int_array_to_safearray as *const (),
    int_safearray_to_native as *const (),
    rect_to_variant as *const (),
    variant_to_rect as *const (),
    safearray_to_rects as *const (),
    create_proxy_factory_entry as *const (),
    proxy_factory_mapping as *const (),
    property_name as *const (),
    pattern_name as *const (),
    poll_patterns as *const (),
    poll_properties as *const (),
    check_not_supported as *const (),
    not_supported_value as *const (),
    mixed_attribute_value as *const (),
    from_iaccessible as *const (),
    from_iaccessible_cache as *const (),
]);

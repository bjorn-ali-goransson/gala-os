//! The CUIAutomation object (IUIAutomation): elements from windows, the
//! root, points and the focus; conditions; tree walkers; cache requests.
//! CUIAutomation8 is the same object answering IUIAutomation2..6 too,
//! whose methods extend IUIAutomation's vtable (one table serves all six
//! interfaces): timeouts and options are kept and read back, the event
//! methods are E_NOTIMPL (no events yet).

use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, Ordering};

use crate::element::{self, Node, desktop, node_of, put_element};
use crate::{
    E_INVALIDARG, E_NOINTERFACE, E_OUTOFMEMORY, E_POINTER, GetFocus, Guid, HResult, Hwnd,
    IID_IUNKNOWN, IsWindow, Point, S_OK, SafeArrayAccessData, SafeArrayGetDim,
    SafeArrayGetElemsize, SafeArrayGetLBound, SafeArrayGetUBound, SafeArrayUnaccessData, VT_I4,
    VT_R8, VTable, Variant, WindowFromPoint, boxed, cache, condition, free, guid, iid_at,
    is_not_supported, mixed_attribute, not_supported, put, walker,
};

const IID_IUIAUTOMATION: Guid =
    guid(0x30cbe57d, 0xd9d0, 0x452a, [0xab, 0x13, 0x7a, 0xc5, 0xac, 0x48, 0x25, 0xee]);
/// IUIAutomation2..6 (UIAutomationClient.h), each extending the one
/// before.
const IID_IUIAUTOMATION_NEWER: [Guid; 5] = [
    guid(0x34723aff, 0x0c9d, 0x49d0, [0x98, 0x96, 0x7a, 0xb5, 0x2d, 0xf8, 0xcd, 0x8a]),
    guid(0x73d768da, 0x9b51, 0x4b89, [0x93, 0x6e, 0xc2, 0x09, 0x29, 0x09, 0x73, 0xe7]),
    guid(0x1189c02a, 0x05f8, 0x4319, [0x8e, 0x21, 0xe8, 0x17, 0xe3, 0xdb, 0x28, 0x60]),
    guid(0x25f700c8, 0xd816, 0x4057, [0xa9, 0xdc, 0x3c, 0xbd, 0xee, 0x77, 0xe2, 0x56]),
    guid(0xaae072da, 0x29e3, 0x413d, [0x87, 0xa7, 0x19, 0x2d, 0xbf, 0x81, 0xed, 0x10]),
];
const IS_CONTROL_ELEMENT: i32 = 30016;
const IS_CONTENT_ELEMENT: i32 = 30017;

/// IUIAutomation's 58 methods and IUIAutomation2..6's 21.
const METHODS: usize = 79;

#[repr(C)]
struct Automation {
    vtable: *const VTable<METHODS>,
    refs: AtomicU32,
    /// CUIAutomation8: answers IUIAutomation2..6.
    v8: bool,
    auto_set_focus: AtomicU32,
    connection_timeout: AtomicU32,
    transaction_timeout: AtomicU32,
    connection_recovery: AtomicU32,
    coalesce_events: AtomicU32,
}

/// A new CUIAutomation (CUIAutomation8 with `v8`) object's `iid`
/// interface at `out`. The documented defaults: AutoSetFocus TRUE,
/// ConnectionTimeout 2 s, TransactionTimeout 20 s.
pub fn create(v8: bool, iid: *const Guid, out: *mut *mut c_void) -> HResult {
    let a = boxed(Automation {
        vtable: &VTABLE,
        refs: AtomicU32::new(1),
        v8,
        auto_set_focus: AtomicU32::new(1),
        connection_timeout: AtomicU32::new(2000),
        transaction_timeout: AtomicU32::new(20000),
        connection_recovery: AtomicU32::new(0),
        coalesce_events: AtomicU32::new(0),
    });
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
    let iid = iid_at(iid);
    // SAFETY: our object.
    let v8 = unsafe { (*this).v8 };
    if iid == IID_IUNKNOWN
        || iid == IID_IUIAUTOMATION
        || (v8 && IID_IUIAUTOMATION_NEWER.contains(&iid))
    {
        add_ref(this);
        put(out, this.cast());
        S_OK
    } else {
        put(out, core::ptr::null_mut());
        E_NOINTERFACE
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
    let (Some(a), Some(b)) = (node_of(a), node_of(b)) else {
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
    put_element(out, Node::window(h))
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
    put_element(out, Node::window(h))
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

/// The control view's condition (a new reference at `out`).
pub fn control_view(out: *mut *mut c_void) -> HResult {
    view_condition(1, out)
}

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

/// The interface pointers of a one-dimensional SAFEARRAY of VT_UNKNOWN
/// (or VT_DISPATCH), as the array-based condition factories take them,
/// handed to `f` while the array's data is locked.
fn with_unknowns(sa: *mut c_void, f: impl FnOnce(&[*mut c_void]) -> HResult) -> HResult {
    if sa.is_null() {
        return E_POINTER;
    }
    // SAFETY: the caller's SAFEARRAY; its data stays locked (accessed)
    // while `f` reads it.
    unsafe {
        if SafeArrayGetDim(sa) != 1 || SafeArrayGetElemsize(sa) as usize != size_of::<usize>() {
            return E_INVALIDARG;
        }
        let (mut lower, mut upper) = (0, 0);
        if SafeArrayGetLBound(sa, 1, &mut lower) != S_OK
            || SafeArrayGetUBound(sa, 1, &mut upper) != S_OK
        {
            return E_INVALIDARG;
        }
        let count = (i64::from(upper) - i64::from(lower) + 1).max(0) as usize;
        let mut data = core::ptr::null_mut();
        if SafeArrayAccessData(sa, &mut data) != S_OK {
            return E_INVALIDARG;
        }
        let items: &[*mut c_void] =
            if count == 0 { &[] } else { core::slice::from_raw_parts(data.cast(), count) };
        let hr = f(items);
        SafeArrayUnaccessData(sa);
        hr
    }
}

extern "system" fn and_from_array(_this: usize, sa: *mut c_void, out: *mut *mut c_void) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    with_unknowns(sa, |items| condition::create_and(items, out))
}

extern "system" fn or_from_array(_this: usize, sa: *mut c_void, out: *mut *mut c_void) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    with_unknowns(sa, |items| condition::create_or(items, out))
}

extern "system" fn create_cache_request(_this: usize, out: *mut *mut c_void) -> HResult {
    cache::create(out)
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

/// CheckNotSupported: whether the value (a VARIANT, by reference) is the
/// not-supported value.
extern "system" fn check_not_supported(
    _this: usize,
    value: *const Variant,
    out: *mut i32,
) -> HResult {
    if out.is_null() || value.is_null() {
        return E_POINTER;
    }
    // SAFETY: the caller's VARIANT.
    put(out, i32::from(is_not_supported(unsafe { &*value })));
    S_OK
}

/// get_ReservedNotSupportedValue: the one not-supported IUnknown.
extern "system" fn not_supported_value(_this: usize, out: *mut *mut c_void) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, not_supported());
    S_OK
}

/// get_ReservedMixedAttributeValue: the one mixed-attribute IUnknown.
extern "system" fn mixed_attribute_value(_this: usize, out: *mut *mut c_void) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, mixed_attribute());
    S_OK
}

// ---- IUIAutomation2..6: options ----

macro_rules! option {
    ($get:ident, $put:ident, $field:ident) => {
        extern "system" fn $get(this: *mut Automation, out: *mut u32) -> HResult {
            if out.is_null() {
                return E_POINTER;
            }
            // SAFETY: our object.
            put(out, unsafe { (*this).$field.load(Ordering::Relaxed) });
            S_OK
        }

        extern "system" fn $put(this: *mut Automation, value: u32) -> HResult {
            // SAFETY: our object.
            unsafe { (*this).$field.store(value, Ordering::Relaxed) };
            S_OK
        }
    };
}

option!(get_auto_set_focus, put_auto_set_focus, auto_set_focus);
option!(get_connection_timeout, put_connection_timeout, connection_timeout);
option!(get_transaction_timeout, put_transaction_timeout, transaction_timeout);
option!(get_connection_recovery, put_connection_recovery, connection_recovery);
option!(get_coalesce_events, put_coalesce_events, coalesce_events);

not_impl!(compare_runtime_ids, b"UIAutomationCore.dll!IUIAutomation::CompareRuntimeIds");
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
not_impl!(from_iaccessible, b"UIAutomationCore.dll!IUIAutomation::ElementFromIAccessible");
not_impl!(
    from_iaccessible_cache,
    b"UIAutomationCore.dll!IUIAutomation::ElementFromIAccessibleBuildCache"
);
not_impl!(
    add_text_edit_handler,
    b"UIAutomationCore.dll!IUIAutomation3::AddTextEditTextChangedEventHandler"
);
not_impl!(
    remove_text_edit_handler,
    b"UIAutomationCore.dll!IUIAutomation3::RemoveTextEditTextChangedEventHandler"
);
not_impl!(add_changes_handler, b"UIAutomationCore.dll!IUIAutomation4::AddChangesEventHandler");
not_impl!(
    remove_changes_handler,
    b"UIAutomationCore.dll!IUIAutomation4::RemoveChangesEventHandler"
);
not_impl!(
    add_notification_handler,
    b"UIAutomationCore.dll!IUIAutomation5::AddNotificationEventHandler"
);
not_impl!(
    remove_notification_handler,
    b"UIAutomationCore.dll!IUIAutomation5::RemoveNotificationEventHandler"
);
not_impl!(create_handler_group, b"UIAutomationCore.dll!IUIAutomation6::CreateEventHandlerGroup");
not_impl!(add_handler_group, b"UIAutomationCore.dll!IUIAutomation6::AddEventHandlerGroup");
not_impl!(remove_handler_group, b"UIAutomationCore.dll!IUIAutomation6::RemoveEventHandlerGroup");
not_impl!(
    add_text_position_handler,
    b"UIAutomationCore.dll!IUIAutomation6::AddActiveTextPositionChangedEventHandler"
);
not_impl!(
    remove_text_position_handler,
    b"UIAutomationCore.dll!IUIAutomation6::RemoveActiveTextPositionChangedEventHandler"
);

static VTABLE: VTable<METHODS> = VTable([
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
    // IUIAutomation2.
    get_auto_set_focus as *const (),
    put_auto_set_focus as *const (),
    get_connection_timeout as *const (),
    put_connection_timeout as *const (),
    get_transaction_timeout as *const (),
    put_transaction_timeout as *const (),
    // IUIAutomation3.
    add_text_edit_handler as *const (),
    remove_text_edit_handler as *const (),
    // IUIAutomation4.
    add_changes_handler as *const (),
    remove_changes_handler as *const (),
    // IUIAutomation5.
    add_notification_handler as *const (),
    remove_notification_handler as *const (),
    // IUIAutomation6.
    create_handler_group as *const (),
    add_handler_group as *const (),
    remove_handler_group as *const (),
    get_connection_recovery as *const (),
    put_connection_recovery as *const (),
    get_coalesce_events as *const (),
    put_coalesce_events as *const (),
    add_text_position_handler as *const (),
    remove_text_position_handler as *const (),
]);

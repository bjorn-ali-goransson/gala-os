//! Elements (IUIAutomationElement) and element arrays: an element is an
//! HWND. The desktop (GetDesktopWindow) is the root; its children are the
//! visible top-level windows, a window's children its visible child
//! windows in z order (GW_CHILD, GW_HWNDNEXT). Properties are read from
//! the window each time (nothing is cached).

use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, Ordering};

use crate::condition::{self, Condition};
use crate::{
    E_INVALIDARG, E_NOINTERFACE, E_OUTOFMEMORY, E_POINTER, EnumWindows, GetAncestor, GetClassNameW,
    GetDesktopWindow, GetDlgCtrlID, GetFocus, GetWindow, GetWindowLongW, GetWindowRect,
    GetWindowThreadProcessId, Guid, HResult, Hwnd, IID_IUNKNOWN, IsWindow, IsWindowEnabled,
    IsWindowVisible, List, S_OK, SafeArrayCreateVector, SafeArrayPutElement, SendMessageW,
    SetFocus, SetForegroundWindow, UIA_E_ELEMENTNOTAVAILABLE, VT_BOOL, VT_BSTR, VT_I4, VT_R8,
    VTable, Variant, alloc, boxed, bstr, bstr_str, free, guid, iid_at, missing, pattern, put,
};

pub const IID_IUIAUTOMATIONELEMENT: Guid =
    guid(0xd22108aa, 0x8ac5, 0x49a5, [0x83, 0x7b, 0x37, 0xbb, 0xb3, 0xd7, 0x59, 0x1e]);
const IID_IUIAUTOMATIONELEMENTARRAY: Guid =
    guid(0x14314595, 0xb4bc, 0x4055, [0x95, 0xf2, 0x58, 0xf2, 0xe4, 0x2c, 0x98, 0x55]);

// Property ids (UIAutomationClient.h).
const RUNTIME_ID: i32 = 30000;
const BOUNDING_RECTANGLE: i32 = 30001;
const PROCESS_ID: i32 = 30002;
const CONTROL_TYPE: i32 = 30003;
const LOCALIZED_CONTROL_TYPE: i32 = 30004;
const NAME: i32 = 30005;
const ACCELERATOR_KEY: i32 = 30006;
const ACCESS_KEY: i32 = 30007;
const HAS_KEYBOARD_FOCUS: i32 = 30008;
const IS_KEYBOARD_FOCUSABLE: i32 = 30009;
const IS_ENABLED: i32 = 30010;
const AUTOMATION_ID: i32 = 30011;
const CLASS_NAME: i32 = 30012;
const HELP_TEXT: i32 = 30013;
const CULTURE: i32 = 30015;
const IS_CONTROL_ELEMENT: i32 = 30016;
const IS_CONTENT_ELEMENT: i32 = 30017;
const IS_PASSWORD: i32 = 30019;
const NATIVE_WINDOW_HANDLE: i32 = 30020;
const ITEM_TYPE: i32 = 30021;
const IS_OFFSCREEN: i32 = 30022;
const ORIENTATION: i32 = 30023;
const FRAMEWORK_ID: i32 = 30024;
const IS_REQUIRED_FOR_FORM: i32 = 30025;
const ITEM_STATUS: i32 = 30026;
const ARIA_ROLE: i32 = 30101;
const ARIA_PROPERTIES: i32 = 30102;
const IS_DATA_VALID_FOR_FORM: i32 = 30103;
const PROVIDER_DESCRIPTION: i32 = 30107;

/// The Is<Pattern>PatternAvailable properties 30027-30044 and their
/// patterns.
const PATTERN_AVAILABLE: [i32; 18] = [
    10011, 10005, 10007, 10006, 10000, 10008, 10003, 10004, 10017, 10010, 10001, 10012, 10013,
    10014, 10015, 10016, 10002, 10009,
];

// Control types.
pub const BUTTON: i32 = 50000;
pub const CHECK_BOX: i32 = 50002;
pub const COMBO_BOX: i32 = 50003;
pub const EDIT: i32 = 50004;
const HYPERLINK: i32 = 50005;
const IMAGE: i32 = 50006;
const LIST: i32 = 50008;
const PROGRESS_BAR: i32 = 50012;
pub const RADIO_BUTTON: i32 = 50013;
const SCROLL_BAR: i32 = 50014;
const SLIDER: i32 = 50015;
const STATUS_BAR: i32 = 50017;
const TAB: i32 = 50018;
pub const TEXT: i32 = 50020;
const TOOL_BAR: i32 = 50021;
const TREE: i32 = 50023;
const GROUP: i32 = 50026;
const WINDOW: i32 = 50032;
const PANE: i32 = 50033;

const GWL_STYLE: i32 = -16;
const WS_CAPTION: u32 = 0x00c0_0000;
const WS_TABSTOP: u32 = 0x0001_0000;
const ES_PASSWORD: u32 = 0x20;
const GW_HWNDNEXT: u32 = 2;
const GW_HWNDPREV: u32 = 3;
const GW_CHILD: u32 = 5;
const GA_PARENT: u32 = 1;
/// UiaAppendRuntimeId's first part for HWND-based elements.
const RUNTIME_ID_HWND: i32 = 42;

// ---- The tree ----

pub fn desktop() -> Hwnd {
    // SAFETY: a plain call.
    unsafe { GetDesktopWindow() }
}

pub fn alive(h: Hwnd) -> bool {
    // SAFETY: a plain call.
    unsafe { IsWindow(h) != 0 }
}

fn visible(h: Hwnd) -> bool {
    // SAFETY: a plain call.
    unsafe { IsWindowVisible(h) != 0 }
}

pub fn style(h: Hwnd) -> u32 {
    // SAFETY: a plain call.
    unsafe { GetWindowLongW(h, GWL_STYLE) as u32 }
}

fn top_level(h: Hwnd) -> bool {
    // SAFETY: a plain call.
    h != desktop() && unsafe { GetAncestor(h, GA_PARENT) } == 0
}

/// The element's parent (0 for the root).
pub fn parent(h: Hwnd) -> Hwnd {
    if h == desktop() {
        return 0;
    }
    // SAFETY: a plain call.
    match unsafe { GetAncestor(h, GA_PARENT) } {
        0 => desktop(),
        p => p,
    }
}

extern "system" fn collect(h: Hwnd, data: isize) -> i32 {
    if visible(h) {
        // SAFETY: `children` passes its list.
        unsafe { (*(data as *mut List)).push(h) };
    }
    1
}

/// The element's children, in order.
pub fn children(h: Hwnd, out: &mut List) {
    if h == desktop() {
        // SAFETY: the callback gets our list for the call's duration.
        unsafe { EnumWindows(collect, out as *mut List as isize) };
        return;
    }
    // SAFETY: plain calls.
    let mut c = unsafe { GetWindow(h, GW_CHILD) };
    for _ in 0..4096 {
        if c == 0 {
            break;
        }
        if visible(c) && !out.push(c) {
            return;
        }
        // SAFETY: as above.
        c = unsafe { GetWindow(c, GW_HWNDNEXT) };
    }
}

// ---- What an element is ----

fn class_name(h: Hwnd) -> ([u16; 64], usize) {
    if h == desktop() {
        return crate::wide("#32769");
    }
    let mut buf = [0u16; 64];
    // SAFETY: our buffer of 64 characters.
    let n = unsafe { GetClassNameW(h, buf.as_mut_ptr(), 64) }.max(0) as usize;
    (buf, n)
}

fn same_ascii(a: &[u16], b: &str) -> bool {
    a.len() == b.len()
        && a.iter().zip(b.bytes()).all(|(&x, y)| x < 0x80 && (x as u8).eq_ignore_ascii_case(&y))
}

/// The control type: by class (and a button's or static's style); other
/// windows are Window (top-level with a caption) or Pane.
pub fn control_type(h: Hwnd) -> i32 {
    if h == desktop() {
        return PANE;
    }
    let (buf, n) = class_name(h);
    let c = &buf[..n];
    let s = style(h);
    let is = |name: &str| same_ascii(c, name);
    if is("Button") {
        return match s & 0xf {
            2 | 3 | 5 | 6 => CHECK_BOX, // BS_(AUTO)CHECKBOX, BS_(AUTO)3STATE
            4 | 9 => RADIO_BUTTON,      // BS_(AUTO)RADIOBUTTON
            7 => GROUP,                 // BS_GROUPBOX
            _ => BUTTON,
        };
    }
    if is("Static") {
        // SS_ICON, SS_BITMAP.
        return if matches!(s & 0x1f, 3 | 0xe) { IMAGE } else { TEXT };
    }
    let by_class: [(&str, i32); 12] = [
        ("Edit", EDIT),
        ("ComboBox", COMBO_BOX),
        ("ListBox", LIST),
        ("ComboLBox", LIST),
        ("SysListView32", LIST),
        ("SysTreeView32", TREE),
        ("msctls_progress32", PROGRESS_BAR),
        ("SysTabControl32", TAB),
        ("ScrollBar", SCROLL_BAR),
        ("msctls_statusbar32", STATUS_BAR),
        ("ToolbarWindow32", TOOL_BAR),
        ("msctls_trackbar32", SLIDER),
    ];
    if let Some(&(_, t)) = by_class.iter().find(|(name, _)| is(name)) {
        return t;
    }
    if is("SysLink") {
        return HYPERLINK;
    }
    if top_level(h) && s & WS_CAPTION == WS_CAPTION { WINDOW } else { PANE }
}

fn localized_control_type(t: i32) -> &'static str {
    match t {
        BUTTON => "button",
        CHECK_BOX => "check box",
        COMBO_BOX => "combo box",
        EDIT => "edit",
        HYPERLINK => "link",
        IMAGE => "image",
        LIST => "list",
        PROGRESS_BAR => "progress bar",
        RADIO_BUTTON => "radio button",
        SCROLL_BAR => "scroll bar",
        SLIDER => "slider",
        STATUS_BAR => "status bar",
        TAB => "tab",
        TEXT => "text",
        TOOL_BAR => "tool bar",
        TREE => "tree",
        GROUP => "group",
        WINDOW => "window",
        _ => "pane",
    }
}

/// The window's text as a BSTR; with `strip`, '&' mnemonic markers are
/// removed ("&&" is a literal '&').
const WM_GETTEXT: u32 = 0x000d;
const WM_GETTEXTLENGTH: u32 = 0x000e;

pub fn window_text(h: Hwnd, strip: bool) -> *mut u16 {
    // SAFETY: plain calls; the buffer holds `n + 1` characters.
    unsafe {
        // WM_GETTEXT(LENGTH), as Windows' Win32 proxies read a control:
        // the control's current text, also in another process (where
        // GetWindowText gives only the window's own text).
        let n = SendMessageW(h, WM_GETTEXTLENGTH, 0, 0).max(0) as usize;
        let buf = alloc((n + 1) * 2).cast::<u16>();
        if buf.is_null() {
            return core::ptr::null_mut();
        }
        let got = (SendMessageW(h, WM_GETTEXT, n + 1, buf as isize).max(0) as usize).min(n);
        let s = core::slice::from_raw_parts_mut(buf, got);
        let mut len = got;
        if strip {
            let (mut i, mut j) = (0, 0);
            while i < got {
                if s[i] == u16::from(b'&') {
                    i += 1;
                    if i == got {
                        break;
                    }
                }
                s[j] = s[i];
                i += 1;
                j += 1;
            }
            len = j;
        }
        let b = bstr(&s[..len]);
        free(buf.cast());
        b
    }
}

/// The static label just before a control (its Name), or 0.
pub fn label_of(h: Hwnd) -> Hwnd {
    // SAFETY: a plain call.
    let p = unsafe { GetWindow(h, GW_HWNDPREV) };
    if p != 0 && control_type(p) == TEXT { p } else { 0 }
}

fn name(h: Hwnd) -> *mut u16 {
    if h == desktop() {
        return bstr_str("Desktop 1");
    }
    match control_type(h) {
        // A field's text is its value; its name is its label's.
        EDIT | COMBO_BOX | LIST | TREE | SLIDER | PROGRESS_BAR => match label_of(h) {
            0 => bstr(&[]),
            l => window_text(l, true),
        },
        BUTTON | CHECK_BOX | RADIO_BUTTON | GROUP | TEXT => window_text(h, true),
        _ => window_text(h, false),
    }
}

/// The control id in decimal (none for top-level windows and id 0).
fn automation_id(h: Hwnd) -> *mut u16 {
    // SAFETY: a plain call.
    let id = if h == desktop() || top_level(h) { 0 } else { unsafe { GetDlgCtrlID(h) } };
    if id == 0 {
        return bstr(&[]);
    }
    let mut digits = [0u16; 12];
    let mut n = 0;
    let mut v = i64::from(id).unsigned_abs();
    loop {
        digits[n] = u16::from(b'0') + (v % 10) as u16;
        n += 1;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    if id < 0 {
        digits[n] = u16::from(b'-');
        n += 1;
    }
    digits[..n].reverse();
    bstr(&digits[..n])
}

fn window_rect(h: Hwnd) -> [i32; 4] {
    let mut r = [0; 4];
    // SAFETY: our RECT.
    unsafe { GetWindowRect(h, &mut r) };
    r
}

pub fn array<T>(vt: u16, items: &[T]) -> Variant {
    // SAFETY: a vector of `items.len()` elements of type `vt`, filled in.
    unsafe {
        let sa = SafeArrayCreateVector(vt, 0, items.len() as u32);
        if sa.is_null() {
            return Variant::EMPTY;
        }
        for (i, item) in items.iter().enumerate() {
            let index = i as i32;
            SafeArrayPutElement(sa, &index, (item as *const T).cast());
        }
        Variant { vt: crate::VT_ARRAY | vt, reserved: [0; 3], data: [sa as u64, 0] }
    }
}

fn focusable(h: Hwnd) -> bool {
    if h == desktop() || !visible(h) {
        return false;
    }
    // SAFETY: a plain call.
    if unsafe { IsWindowEnabled(h) } == 0 {
        return false;
    }
    top_level(h)
        || style(h) & WS_TABSTOP != 0
        || matches!(control_type(h), EDIT | COMBO_BOX | LIST | TREE)
}

/// A property's current value (VT_EMPTY for one not supported, recorded
/// unless it is a known pattern's property the element lacks).
pub fn property(h: Hwnd, id: i32) -> Variant {
    lookup(h, id).unwrap_or(Variant::EMPTY)
}

/// A property's current value; None when the element does not support it.
fn lookup(h: Hwnd, id: i32) -> Option<Variant> {
    Some(match id {
        RUNTIME_ID => array(VT_I4, &[RUNTIME_ID_HWND, h as i32]),
        BOUNDING_RECTANGLE => {
            let r = window_rect(h);
            let v = [r[0], r[1], r[2] - r[0], r[3] - r[1]].map(f64::from);
            array(VT_R8, &v)
        }
        PROCESS_ID => {
            let mut pid = 0;
            // SAFETY: our output.
            unsafe { GetWindowThreadProcessId(h, &mut pid) };
            Variant::int(pid as i32)
        }
        CONTROL_TYPE => Variant::int(control_type(h)),
        LOCALIZED_CONTROL_TYPE => Variant::bstr(bstr_str(localized_control_type(control_type(h)))),
        NAME => Variant::bstr(name(h)),
        ACCELERATOR_KEY | ACCESS_KEY | HELP_TEXT | ITEM_TYPE | ITEM_STATUS | ARIA_ROLE
        | ARIA_PROPERTIES => Variant::bstr(bstr(&[])),
        // SAFETY: a plain call.
        HAS_KEYBOARD_FOCUS => Variant::boolean(unsafe { GetFocus() } == h),
        IS_KEYBOARD_FOCUSABLE => Variant::boolean(focusable(h)),
        // SAFETY: a plain call.
        IS_ENABLED => Variant::boolean(h == desktop() || unsafe { IsWindowEnabled(h) } != 0),
        AUTOMATION_ID => Variant::bstr(automation_id(h)),
        CLASS_NAME => {
            let (buf, n) = class_name(h);
            Variant::bstr(bstr(&buf[..n]))
        }
        CULTURE | ORIENTATION => Variant::int(0),
        IS_CONTROL_ELEMENT | IS_CONTENT_ELEMENT | IS_DATA_VALID_FOR_FORM => Variant::boolean(true),
        IS_PASSWORD => Variant::boolean(control_type(h) == EDIT && style(h) & ES_PASSWORD != 0),
        NATIVE_WINDOW_HANDLE => Variant::int(h as i32),
        IS_OFFSCREEN => Variant::boolean(h != desktop() && !visible(h)),
        FRAMEWORK_ID => Variant::bstr(bstr_str("Win32")),
        IS_REQUIRED_FOR_FORM => Variant::boolean(false),
        PROVIDER_DESCRIPTION => Variant::bstr(bstr_str("[gala: HWND element]")),
        30027..=30044 => {
            Variant::boolean(pattern::supports(h, PATTERN_AVAILABLE[(id - 30027) as usize]))
        }
        _ => {
            if !pattern::known_property(id) {
                missing(b"UIAutomationCore.dll!GetCurrentPropertyValue(property)", id as u64);
            }
            return pattern::property(h, id);
        }
    })
}

// ---- Finding ----

/// Visits `h`'s children (and with `deep` their descendants) in pre-order,
/// adding those that match; true once the search is over.
fn walk(
    h: Hwnd,
    cond: *mut Condition,
    deep: bool,
    first: bool,
    out: &mut List,
    depth: u32,
) -> bool {
    if depth > 64 {
        return false;
    }
    let mut kids = List::new();
    children(h, &mut kids);
    for &c in kids.as_slice() {
        if condition::matches(cond, c) && (!out.push(c) || first) {
            return true;
        }
        if deep && walk(c, cond, deep, first, out, depth + 1) {
            return true;
        }
    }
    false
}

/// FindFirst/FindAll: the matches in `scope` (TreeScope_Element 1,
/// _Children 2, _Descendants 4; _Parent and _Ancestors are not supported,
/// as documented).
fn find(h: Hwnd, scope: i32, cond: *mut c_void, first: bool, out: &mut List) -> HResult {
    let Some(cond) = condition::as_condition(cond) else { return E_INVALIDARG };
    if scope & !7 != 0 || scope & 7 == 0 {
        return E_INVALIDARG;
    }
    if !alive(h) {
        return UIA_E_ELEMENTNOTAVAILABLE;
    }
    if scope & 1 != 0 && condition::matches(cond, h) && (!out.push(h) || first) {
        return S_OK;
    }
    if scope & 6 != 0 {
        walk(h, cond, scope & 4 != 0, first, out, 0);
    }
    S_OK
}

// ---- The element object ----

#[repr(C)]
pub struct Element {
    vtable: *const VTable<85>,
    refs: AtomicU32,
    hwnd: Hwnd,
}

/// The window of one of our elements (None for anything else).
pub fn hwnd_of(p: *mut c_void) -> Option<Hwnd> {
    if p.is_null() {
        return None;
    }
    let e = p.cast::<Element>();
    // SAFETY: an interface pointer, whose first field is its vtable; only
    // our elements have ours.
    unsafe { core::ptr::eq((*e).vtable, &VTABLE).then(|| (*e).hwnd) }
}

/// Stores a new element for `h` at `out`.
pub fn put_element(out: *mut *mut c_void, h: Hwnd) -> HResult {
    let e = boxed(Element { vtable: &VTABLE, refs: AtomicU32::new(1), hwnd: h });
    put(out, e.cast());
    if e.is_null() { E_OUTOFMEMORY } else { S_OK }
}

fn this_hwnd(this: *mut Element) -> Hwnd {
    // SAFETY: our element.
    unsafe { (*this).hwnd }
}

extern "system" fn query(this: *mut Element, iid: *const Guid, out: *mut *mut c_void) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    match iid_at(iid) {
        IID_IUNKNOWN | IID_IUIAUTOMATIONELEMENT => {
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

extern "system" fn add_ref(this: *mut Element) -> u32 {
    // SAFETY: our element.
    unsafe { (*this).refs.fetch_add(1, Ordering::Relaxed) + 1 }
}

extern "system" fn release(this: *mut Element) -> u32 {
    // SAFETY: our element, freed with its last reference.
    unsafe {
        let left = (*this).refs.fetch_sub(1, Ordering::AcqRel) - 1;
        if left == 0 {
            free(this.cast());
        }
        left
    }
}

extern "system" fn set_focus(this: *mut Element) -> HResult {
    let h = this_hwnd(this);
    if !alive(h) {
        return UIA_E_ELEMENTNOTAVAILABLE;
    }
    // SAFETY: plain calls.
    unsafe {
        if top_level(h) {
            SetForegroundWindow(h);
        } else if h != desktop() {
            SetFocus(h);
        }
    }
    S_OK
}

extern "system" fn get_runtime_id(this: *mut Element, out: *mut *mut c_void) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    let v = property(this_hwnd(this), RUNTIME_ID);
    put(out, v.data[0] as usize as *mut c_void);
    if v.vt == 0 { E_OUTOFMEMORY } else { S_OK }
}

extern "system" fn find_first(
    this: *mut Element,
    scope: i32,
    cond: *mut c_void,
    out: *mut *mut c_void,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    let mut found = List::new();
    let hr = find(this_hwnd(this), scope, cond, true, &mut found);
    match found.as_slice().first() {
        Some(&h) if hr == S_OK => put_element(out, h),
        _ => hr,
    }
}

extern "system" fn find_all(
    this: *mut Element,
    scope: i32,
    cond: *mut c_void,
    out: *mut *mut c_void,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    let mut found = List::new();
    let hr = find(this_hwnd(this), scope, cond, false, &mut found);
    if hr != S_OK {
        return hr;
    }
    put_array(out, found)
}

extern "system" fn find_first_cache(
    this: *mut Element,
    scope: i32,
    cond: *mut c_void,
    _cache: *mut c_void,
    out: *mut *mut c_void,
) -> HResult {
    find_first(this, scope, cond, out)
}

extern "system" fn find_all_cache(
    this: *mut Element,
    scope: i32,
    cond: *mut c_void,
    _cache: *mut c_void,
    out: *mut *mut c_void,
) -> HResult {
    find_all(this, scope, cond, out)
}

extern "system" fn build_updated_cache(
    this: *mut Element,
    _cache: *mut c_void,
    out: *mut *mut c_void,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    add_ref(this);
    put(out, this.cast());
    S_OK
}

extern "system" fn get_property(this: *mut Element, id: i32, out: *mut Variant) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, Variant::EMPTY);
    let h = this_hwnd(this);
    if !alive(h) {
        return UIA_E_ELEMENTNOTAVAILABLE;
    }
    put(out, property(h, id));
    S_OK
}

/// GetCurrentPropertyValueEx: with `ignore_default`, a property the
/// element does not support gives the not-supported value (FlaUI asks so
/// and compares the result with ReservedNotSupportedValue).
extern "system" fn get_property_ex(
    this: *mut Element,
    id: i32,
    ignore_default: i32,
    out: *mut Variant,
) -> HResult {
    if ignore_default == 0 {
        return get_property(this, id, out);
    }
    if out.is_null() {
        return E_POINTER;
    }
    put(out, Variant::EMPTY);
    let h = this_hwnd(this);
    if !alive(h) {
        return UIA_E_ELEMENTNOTAVAILABLE;
    }
    put(out, lookup(h, id).unwrap_or_else(Variant::not_supported));
    S_OK
}

extern "system" fn get_pattern_as(
    this: *mut Element,
    id: i32,
    iid: *const Guid,
    out: *mut *mut c_void,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    let h = this_hwnd(this);
    if !alive(h) {
        return UIA_E_ELEMENTNOTAVAILABLE;
    }
    let p = pattern::create(h, id);
    if p.is_null() {
        return S_OK;
    }
    let hr = pattern::query(p, iid, out);
    pattern::release(p);
    hr
}

extern "system" fn get_pattern(this: *mut Element, id: i32, out: *mut *mut c_void) -> HResult {
    get_pattern_as(this, id, &IID_IUNKNOWN, out)
}

extern "system" fn get_parent(this: *mut Element, out: *mut *mut c_void) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    match parent(this_hwnd(this)) {
        0 => S_OK,
        p => put_element(out, p),
    }
}

extern "system" fn get_children(this: *mut Element, out: *mut *mut c_void) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    let mut kids = List::new();
    children(this_hwnd(this), &mut kids);
    put_array(out, kids)
}

fn get_bstr(this: *mut Element, id: i32, out: *mut *mut u16) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    let h = this_hwnd(this);
    if !alive(h) {
        return UIA_E_ELEMENTNOTAVAILABLE;
    }
    let mut v = property(h, id);
    if v.vt == VT_BSTR {
        put(out, v.as_bstr());
    } else {
        v.clear();
        put(out, bstr(&[]));
    }
    S_OK
}

fn get_int(this: *mut Element, id: i32, out: *mut i32) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, 0);
    let h = this_hwnd(this);
    if !alive(h) {
        return UIA_E_ELEMENTNOTAVAILABLE;
    }
    let v = property(h, id);
    match v.vt {
        VT_I4 => put(out, v.as_int()),
        VT_BOOL => put(out, i32::from(v.as_bool())),
        _ => {}
    }
    S_OK
}

macro_rules! getters {
    ($($name:ident: $how:ident($ty:ty) = $id:expr;)*) => {
        $(
            extern "system" fn $name(this: *mut Element, out: *mut $ty) -> HResult {
                $how(this, $id, out)
            }
        )*
    };
}

getters! {
    get_process_id: get_int(i32) = PROCESS_ID;
    get_control_type: get_int(i32) = CONTROL_TYPE;
    get_localized_control_type: get_bstr(*mut u16) = LOCALIZED_CONTROL_TYPE;
    get_name: get_bstr(*mut u16) = NAME;
    get_accelerator_key: get_bstr(*mut u16) = ACCELERATOR_KEY;
    get_access_key: get_bstr(*mut u16) = ACCESS_KEY;
    get_has_keyboard_focus: get_int(i32) = HAS_KEYBOARD_FOCUS;
    get_is_keyboard_focusable: get_int(i32) = IS_KEYBOARD_FOCUSABLE;
    get_is_enabled: get_int(i32) = IS_ENABLED;
    get_automation_id: get_bstr(*mut u16) = AUTOMATION_ID;
    get_class_name: get_bstr(*mut u16) = CLASS_NAME;
    get_help_text: get_bstr(*mut u16) = HELP_TEXT;
    get_culture: get_int(i32) = CULTURE;
    get_is_control_element: get_int(i32) = IS_CONTROL_ELEMENT;
    get_is_content_element: get_int(i32) = IS_CONTENT_ELEMENT;
    get_is_password: get_int(i32) = IS_PASSWORD;
    get_item_type: get_bstr(*mut u16) = ITEM_TYPE;
    get_is_offscreen: get_int(i32) = IS_OFFSCREEN;
    get_orientation: get_int(i32) = ORIENTATION;
    get_framework_id: get_bstr(*mut u16) = FRAMEWORK_ID;
    get_is_required_for_form: get_int(i32) = IS_REQUIRED_FOR_FORM;
    get_item_status: get_bstr(*mut u16) = ITEM_STATUS;
    get_aria_role: get_bstr(*mut u16) = ARIA_ROLE;
    get_aria_properties: get_bstr(*mut u16) = ARIA_PROPERTIES;
    get_is_data_valid_for_form: get_int(i32) = IS_DATA_VALID_FOR_FORM;
    get_provider_description: get_bstr(*mut u16) = PROVIDER_DESCRIPTION;
}

/// NativeWindowHandle (a UIA_HWND, pointer-sized).
extern "system" fn get_native_window_handle(this: *mut Element, out: *mut usize) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    let h = this_hwnd(this);
    put(out, h);
    if alive(h) { S_OK } else { UIA_E_ELEMENTNOTAVAILABLE }
}

extern "system" fn get_bounding_rectangle(this: *mut Element, out: *mut [i32; 4]) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, [0; 4]);
    let h = this_hwnd(this);
    if !alive(h) {
        return UIA_E_ELEMENTNOTAVAILABLE;
    }
    put(out, window_rect(h));
    S_OK
}

extern "system" fn get_labeled_by(this: *mut Element, out: *mut *mut c_void) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    let h = this_hwnd(this);
    if h == desktop() || !matches!(control_type(h), EDIT | COMBO_BOX | LIST | TREE) {
        return S_OK;
    }
    match label_of(h) {
        0 => S_OK,
        l => put_element(out, l),
    }
}

/// ControllerFor, DescribedBy, FlowsTo: none for windows.
extern "system" fn get_empty_array(_this: *mut Element, out: *mut *mut c_void) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    put_array(out, List::new())
}

extern "system" fn get_clickable_point(
    this: *mut Element,
    point: *mut [i32; 2],
    got: *mut i32,
) -> HResult {
    if point.is_null() || got.is_null() {
        return E_POINTER;
    }
    let h = this_hwnd(this);
    if !alive(h) {
        return UIA_E_ELEMENTNOTAVAILABLE;
    }
    let r = window_rect(h);
    put(point, [(r[0] + r[2]) / 2, (r[1] + r[3]) / 2]);
    put(got, i32::from(visible(h) && r[2] > r[0] && r[3] > r[1]));
    S_OK
}

/// The cached getters (52-83) read current values: gala keeps no cache.
static VTABLE: VTable<85> = VTable([
    query as *const (),
    add_ref as *const (),
    release as *const (),
    set_focus as *const (),
    get_runtime_id as *const (),
    find_first as *const (),
    find_all as *const (),
    find_first_cache as *const (),
    find_all_cache as *const (),
    build_updated_cache as *const (),
    get_property as *const (),
    get_property_ex as *const (),
    get_property as *const (),    // GetCachedPropertyValue
    get_property_ex as *const (), // GetCachedPropertyValueEx
    get_pattern_as as *const (),
    get_pattern_as as *const (), // GetCachedPatternAs
    get_pattern as *const (),
    get_pattern as *const (),  // GetCachedPattern
    get_parent as *const (),   // GetCachedParent
    get_children as *const (), // GetCachedChildren
    // Current (20-51).
    get_process_id as *const (),
    get_control_type as *const (),
    get_localized_control_type as *const (),
    get_name as *const (),
    get_accelerator_key as *const (),
    get_access_key as *const (),
    get_has_keyboard_focus as *const (),
    get_is_keyboard_focusable as *const (),
    get_is_enabled as *const (),
    get_automation_id as *const (),
    get_class_name as *const (),
    get_help_text as *const (),
    get_culture as *const (),
    get_is_control_element as *const (),
    get_is_content_element as *const (),
    get_is_password as *const (),
    get_native_window_handle as *const (),
    get_item_type as *const (),
    get_is_offscreen as *const (),
    get_orientation as *const (),
    get_framework_id as *const (),
    get_is_required_for_form as *const (),
    get_item_status as *const (),
    get_bounding_rectangle as *const (),
    get_labeled_by as *const (),
    get_aria_role as *const (),
    get_aria_properties as *const (),
    get_is_data_valid_for_form as *const (),
    get_empty_array as *const (), // ControllerFor
    get_empty_array as *const (), // DescribedBy
    get_empty_array as *const (), // FlowsTo
    get_provider_description as *const (),
    // Cached (52-83).
    get_process_id as *const (),
    get_control_type as *const (),
    get_localized_control_type as *const (),
    get_name as *const (),
    get_accelerator_key as *const (),
    get_access_key as *const (),
    get_has_keyboard_focus as *const (),
    get_is_keyboard_focusable as *const (),
    get_is_enabled as *const (),
    get_automation_id as *const (),
    get_class_name as *const (),
    get_help_text as *const (),
    get_culture as *const (),
    get_is_control_element as *const (),
    get_is_content_element as *const (),
    get_is_password as *const (),
    get_native_window_handle as *const (),
    get_item_type as *const (),
    get_is_offscreen as *const (),
    get_orientation as *const (),
    get_framework_id as *const (),
    get_is_required_for_form as *const (),
    get_item_status as *const (),
    get_bounding_rectangle as *const (),
    get_labeled_by as *const (),
    get_aria_role as *const (),
    get_aria_properties as *const (),
    get_is_data_valid_for_form as *const (),
    get_empty_array as *const (),
    get_empty_array as *const (),
    get_empty_array as *const (),
    get_provider_description as *const (),
    get_clickable_point as *const (),
]);

// ---- Element arrays ----

#[repr(C)]
struct ElementArray {
    vtable: *const VTable<5>,
    refs: AtomicU32,
    items: List,
}

/// Stores a new array of `items`' elements at `out`.
pub fn put_array(out: *mut *mut c_void, items: List) -> HResult {
    let a = boxed(ElementArray { vtable: &ARRAY_VTABLE, refs: AtomicU32::new(1), items });
    put(out, a.cast());
    if a.is_null() { E_OUTOFMEMORY } else { S_OK }
}

extern "system" fn array_query(
    this: *mut ElementArray,
    iid: *const Guid,
    out: *mut *mut c_void,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    match iid_at(iid) {
        IID_IUNKNOWN | IID_IUIAUTOMATIONELEMENTARRAY => {
            array_add_ref(this);
            put(out, this.cast());
            S_OK
        }
        _ => {
            put(out, core::ptr::null_mut());
            E_NOINTERFACE
        }
    }
}

extern "system" fn array_add_ref(this: *mut ElementArray) -> u32 {
    // SAFETY: our array.
    unsafe { (*this).refs.fetch_add(1, Ordering::Relaxed) + 1 }
}

extern "system" fn array_release(this: *mut ElementArray) -> u32 {
    // SAFETY: our array, dropped and freed with its last reference.
    unsafe {
        let left = (*this).refs.fetch_sub(1, Ordering::AcqRel) - 1;
        if left == 0 {
            core::ptr::drop_in_place(this);
            free(this.cast());
        }
        left
    }
}

extern "system" fn array_length(this: *mut ElementArray, out: *mut i32) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: our array.
    put(out, unsafe { (*this).items.as_slice().len() } as i32);
    S_OK
}

extern "system" fn array_element(
    this: *mut ElementArray,
    index: i32,
    out: *mut *mut c_void,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: our array.
    let items = unsafe { (*this).items.as_slice() };
    match usize::try_from(index).ok().and_then(|i| items.get(i)) {
        Some(&h) => put_element(out, h),
        None => E_INVALIDARG,
    }
}

static ARRAY_VTABLE: VTable<5> = VTable([
    array_query as *const (),
    array_add_ref as *const (),
    array_release as *const (),
    array_length as *const (),
    array_element as *const (),
]);

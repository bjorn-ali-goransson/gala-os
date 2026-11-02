//! Control patterns over standard controls: Invoke (push buttons,
//! list-view items), Value (edits), Toggle (check boxes), Selection (list
//! boxes and list views), SelectionItem and ScrollItem (their items),
//! ExpandCollapse and Invoke on menu items (menus.rs), Window and
//! Transform on top-level windows and Invoke on title-bar buttons
//! (frame.rs). A
//! pattern object keeps the element's node and acts on the window with
//! the control's messages.

use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, Ordering};

use crate::element::{self, BUTTON, CHECK_BOX, EDIT, Node};
use crate::frame::{self, TITLE};
use crate::items::{self, How, Kind};
use crate::menus;
use crate::{
    E_NOINTERFACE, E_OUTOFMEMORY, E_POINTER, Guid, HResult, Hwnd, IID_IUNKNOWN, IsWindowEnabled,
    List, PostMessageW, S_OK, SendMessageW, UIA_E_ELEMENTNOTAVAILABLE, UIA_E_ELEMENTNOTENABLED,
    UIA_E_INVALIDOPERATION, VTable, Variant, boxed, bstr, free, guid, iid_at, put,
};

pub const INVOKE: i32 = 10000;
pub const SELECTION: i32 = 10001;
pub const VALUE: i32 = 10002;
pub const EXPAND_COLLAPSE: i32 = 10005;
pub const SELECTION_ITEM: i32 = 10010;
pub const TOGGLE: i32 = 10015;
pub const SCROLL_ITEM: i32 = 10017;

const IID_IUIAUTOMATIONINVOKEPATTERN: Guid =
    guid(0xfb377fbe, 0x8ea6, 0x46d5, [0x9c, 0x73, 0x64, 0x99, 0x64, 0x2d, 0x30, 0x59]);
const IID_IUIAUTOMATIONVALUEPATTERN: Guid =
    guid(0xa94cd8b1, 0x0844, 0x4cd6, [0x9d, 0x2d, 0x64, 0x05, 0x37, 0xab, 0x39, 0xe9]);
const IID_IUIAUTOMATIONTOGGLEPATTERN: Guid =
    guid(0x94cf8058, 0x9b8d, 0x4ab9, [0x8b, 0xfd, 0x4c, 0xd0, 0xa3, 0x3c, 0x8c, 0x70]);
const IID_IUIAUTOMATIONSELECTIONPATTERN: Guid =
    guid(0x5ed5202e, 0xb2ac, 0x47a6, [0xb6, 0x38, 0x4b, 0x0b, 0xf1, 0x40, 0xd7, 0x8e]);
const IID_IUIAUTOMATIONSELECTIONITEMPATTERN: Guid =
    guid(0xa8efa66a, 0x0fda, 0x421a, [0x91, 0x94, 0x38, 0x02, 0x1f, 0x35, 0x78, 0xea]);
const IID_IUIAUTOMATIONEXPANDCOLLAPSEPATTERN: Guid =
    guid(0x619be086, 0x1f4e, 0x4ee4, [0xba, 0xfa, 0x21, 0x01, 0x28, 0x73, 0x87, 0x30]);
const IID_IUIAUTOMATIONSCROLLITEMPATTERN: Guid =
    guid(0xb488300f, 0xd015, 0x4f19, [0x9c, 0x29, 0xbb, 0x59, 0x5e, 0x36, 0x45, 0xef]);

// Pattern properties.
const VALUE_VALUE: i32 = 30045;
const VALUE_IS_READ_ONLY: i32 = 30046;
const SELECTION_SELECTION: i32 = 30059;
const SELECTION_CAN_SELECT_MULTIPLE: i32 = 30060;
const SELECTION_IS_SELECTION_REQUIRED: i32 = 30061;
const SELECTION_ITEM_IS_SELECTED: i32 = 30079;
const SELECTION_ITEM_CONTAINER: i32 = 30080;
const TOGGLE_STATE: i32 = 30086;
const EXPAND_COLLAPSE_STATE: i32 = 30070;

const WM_SETTEXT: u32 = 0x000c;
const BM_GETCHECK: u32 = 0x00f0;
const BM_CLICK: u32 = 0x00f5;
const ES_READONLY: u32 = 0x0800;

/// Whether the element has the pattern.
pub fn supports(n: Node, id: i32) -> bool {
    let h = n.hwnd;
    if n.menu == TITLE {
        // A title-bar button: Invoke.
        return id == INVOKE && n.item.is_some();
    }
    if n.menu != 0 {
        // A menu item: ExpandCollapse with a submenu, else Invoke
        // (separators neither; the bar itself has none).
        let Some(it) = menus::item(n) else { return false };
        if it.flags & menus::SEPARATOR != 0 {
            return false;
        }
        let sub = it.flags & menus::SUBMENU != 0;
        return match id {
            EXPAND_COLLAPSE => sub,
            INVOKE => !sub,
            _ => false,
        };
    }
    if n.item.is_some() {
        let Some(k) = items::kind(h) else { return false };
        return match id {
            SELECTION_ITEM | SCROLL_ITEM => true,
            INVOKE => k == Kind::ListView,
            _ => false,
        };
    }
    if h == element::desktop() {
        return false;
    }
    match id {
        INVOKE => element::control_type(h) == BUTTON,
        VALUE => element::control_type(h) == EDIT,
        TOGGLE => element::control_type(h) == CHECK_BOX,
        SELECTION => items::kind(h).is_some(),
        frame::WINDOW | frame::TRANSFORM => frame::is_frame(n),
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

/// Whether `id` is a pattern property this file knows.
pub fn known_property(id: i32) -> bool {
    matches!(
        id,
        VALUE_VALUE
            | VALUE_IS_READ_ONLY
            | TOGGLE_STATE
            | SELECTION_SELECTION
            | SELECTION_CAN_SELECT_MULTIPLE
            | SELECTION_IS_SELECTION_REQUIRED
            | SELECTION_ITEM_IS_SELECTED
            | SELECTION_ITEM_CONTAINER
            | EXPAND_COLLAPSE_STATE
    )
}

/// A list's selected items (None: not a list).
fn selection(h: Hwnd) -> Option<List<Node>> {
    let k = items::kind(h)?;
    let mut indexes = List::new();
    items::selection(h, k, &mut indexes);
    let mut out = List::new();
    for &i in indexes.as_slice() {
        if !out.push(Node::item(h, i)) {
            break;
        }
    }
    Some(out)
}

/// A pattern property's value: None if the element lacks the pattern (or
/// `id` is no pattern property this file knows). Element-valued ones are
/// VT_UNKNOWN: an element (SelectionContainer) or an element array
/// (Selection), as UIA's client API gives them (FlaUI converts them so).
pub fn property(n: Node, id: i32) -> Option<Variant> {
    let h = n.hwnd;
    if n.menu == TITLE {
        return None;
    }
    if n.menu != 0 {
        return (id == EXPAND_COLLAPSE_STATE && supports(n, EXPAND_COLLAPSE))
            .then(|| Variant::int(expand_state(n)));
    }
    let list = items::kind(h);
    Some(match (id, n.item, list) {
        (SELECTION_ITEM_IS_SELECTED, Some(i), Some(k)) => {
            Variant::boolean(items::selected(h, k, i))
        }
        (SELECTION_ITEM_CONTAINER, Some(_), Some(_)) => {
            Variant::unknown(element::new_element(Node::window(h)))
        }
        (SELECTION_SELECTION, None, Some(_)) => {
            Variant::unknown(element::new_array(selection(h).unwrap_or_default()))
        }
        (SELECTION_CAN_SELECT_MULTIPLE, None, Some(k)) => {
            Variant::boolean(items::can_select_multiple(h, k))
        }
        (SELECTION_IS_SELECTION_REQUIRED, None, Some(_)) => Variant::boolean(false),
        (_, Some(_), _) => return None,
        (VALUE_VALUE, None, _) if supports(n, VALUE) => {
            Variant::bstr(element::window_text(h, false))
        }
        (VALUE_IS_READ_ONLY, None, _) if supports(n, VALUE) => Variant::boolean(read_only(h)),
        (TOGGLE_STATE, None, _) if supports(n, TOGGLE) => Variant::int(toggle_state(h)),
        _ => return None,
    })
}

#[repr(C)]
pub(crate) struct Pattern {
    vtable: *const (),
    refs: AtomicU32,
    node: Node,
    iid: Guid,
}

/// A new pattern object (one reference) for the element, or null when it
/// lacks the pattern (or memory runs out).
pub fn create(n: Node, id: i32) -> *mut c_void {
    if !supports(n, id) {
        return core::ptr::null_mut();
    }
    let (vtable, iid): (*const (), Guid) = match id {
        INVOKE => ((&INVOKE_VTABLE as *const VTable<4>).cast(), IID_IUIAUTOMATIONINVOKEPATTERN),
        VALUE => ((&VALUE_VTABLE as *const VTable<8>).cast(), IID_IUIAUTOMATIONVALUEPATTERN),
        SELECTION => {
            ((&SELECTION_VTABLE as *const VTable<9>).cast(), IID_IUIAUTOMATIONSELECTIONPATTERN)
        }
        SELECTION_ITEM => (
            (&SELECTION_ITEM_VTABLE as *const VTable<10>).cast(),
            IID_IUIAUTOMATIONSELECTIONITEMPATTERN,
        ),
        SCROLL_ITEM => {
            ((&SCROLL_ITEM_VTABLE as *const VTable<4>).cast(), IID_IUIAUTOMATIONSCROLLITEMPATTERN)
        }
        EXPAND_COLLAPSE => (
            (&EXPAND_COLLAPSE_VTABLE as *const VTable<7>).cast(),
            IID_IUIAUTOMATIONEXPANDCOLLAPSEPATTERN,
        ),
        frame::WINDOW | frame::TRANSFORM => frame::vtable(id),
        _ => ((&TOGGLE_VTABLE as *const VTable<6>).cast(), IID_IUIAUTOMATIONTOGGLEPATTERN),
    };
    boxed(Pattern { vtable, refs: AtomicU32::new(1), node: n, iid }).cast()
}

pub fn query(p: *mut c_void, iid: *const Guid, out: *mut *mut c_void) -> HResult {
    pattern_query(p.cast(), iid, out)
}

pub fn release(p: *mut c_void) {
    pattern_release(p.cast());
}

pub(crate) extern "system" fn pattern_query(
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

pub(crate) extern "system" fn pattern_add_ref(this: *mut Pattern) -> u32 {
    // SAFETY: our pattern.
    unsafe { (*this).refs.fetch_add(1, Ordering::Relaxed) + 1 }
}

pub(crate) extern "system" fn pattern_release(this: *mut Pattern) -> u32 {
    // SAFETY: our pattern, freed with its last reference.
    unsafe {
        let left = (*this).refs.fetch_sub(1, Ordering::AcqRel) - 1;
        if left == 0 {
            free(this.cast());
        }
        left
    }
}

pub(crate) fn node(this: *mut Pattern) -> Node {
    // SAFETY: our pattern.
    unsafe { (*this).node }
}

/// The window if it can be acted on: alive and enabled.
fn usable(this: *mut Pattern) -> Result<Hwnd, HResult> {
    let n = node(this);
    if !element::exists(n) {
        return Err(UIA_E_ELEMENTNOTAVAILABLE);
    }
    // SAFETY: a plain call.
    if unsafe { IsWindowEnabled(n.hwnd) } == 0 {
        return Err(UIA_E_ELEMENTNOTENABLED);
    }
    Ok(n.hwnd)
}

pub(crate) fn alive(this: *mut Pattern) -> Result<Hwnd, HResult> {
    let n = node(this);
    if element::exists(n) { Ok(n.hwnd) } else { Err(UIA_E_ELEMENTNOTAVAILABLE) }
}

/// The list, its kind and the index of an item's pattern (if the item is
/// still there).
fn item(this: *mut Pattern) -> Result<(Hwnd, Kind, u32), HResult> {
    let n = node(this);
    match (n.item, items::kind(n.hwnd)) {
        (Some(i), Some(k)) if element::exists(n) => Ok((n.hwnd, k, i)),
        _ => Err(UIA_E_ELEMENTNOTAVAILABLE),
    }
}

// ---- Invoke ----

/// Invoke: a click posted to the button (Invoke is documented to return
/// possibly before the control has acted, so a dialog the click opens
/// does not block the client); a list-view item is activated (a double
/// click posted at it: items::activate).
extern "system" fn invoke(this: *mut Pattern) -> HResult {
    let h = match usable(this) {
        Ok(h) => h,
        Err(e) => return e,
    };
    let n = node(this);
    if n.menu == TITLE {
        return frame::invoke(n);
    }
    if n.menu != 0 {
        return menu_action(n, menus::INVOKE);
    }
    if let Some(i) = n.item {
        return items::activate(h, i);
    }
    // SAFETY: a plain call.
    unsafe { PostMessageW(h, BM_CLICK, 0, 0) };
    S_OK
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

// ---- Selection (a list) ----

extern "system" fn get_selection(this: *mut Pattern, out: *mut *mut c_void) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    let h = match alive(this) {
        Ok(h) => h,
        Err(e) => return e,
    };
    match selection(h) {
        Some(list) => element::put_array(out, list),
        None => UIA_E_ELEMENTNOTAVAILABLE,
    }
}

fn list_flag(this: *mut Pattern, out: *mut i32, id: i32) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, 0);
    if let Err(e) = alive(this) {
        return e;
    }
    match property(node(this), id) {
        Some(v) => {
            put(out, i32::from(v.as_bool()));
            S_OK
        }
        None => UIA_E_ELEMENTNOTAVAILABLE,
    }
}

extern "system" fn get_can_select_multiple(this: *mut Pattern, out: *mut i32) -> HResult {
    list_flag(this, out, SELECTION_CAN_SELECT_MULTIPLE)
}

extern "system" fn get_is_selection_required(this: *mut Pattern, out: *mut i32) -> HResult {
    list_flag(this, out, SELECTION_IS_SELECTION_REQUIRED)
}

/// The cached getters read current values (no cache).
static SELECTION_VTABLE: VTable<9> = VTable([
    pattern_query as *const (),
    pattern_add_ref as *const (),
    pattern_release as *const (),
    get_selection as *const (),
    get_can_select_multiple as *const (),
    get_is_selection_required as *const (),
    get_selection as *const (),
    get_can_select_multiple as *const (),
    get_is_selection_required as *const (),
]);

// ---- SelectionItem (an item) ----

fn change(this: *mut Pattern, how: How) -> HResult {
    match item(this) {
        Ok((h, k, i)) => items::select(h, k, i, how),
        Err(e) => e,
    }
}

extern "system" fn select(this: *mut Pattern) -> HResult {
    change(this, How::Only)
}

extern "system" fn add_to_selection(this: *mut Pattern) -> HResult {
    change(this, How::Add)
}

extern "system" fn remove_from_selection(this: *mut Pattern) -> HResult {
    change(this, How::Remove)
}

extern "system" fn get_is_selected(this: *mut Pattern, out: *mut i32) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, 0);
    match item(this) {
        Ok((h, k, i)) => {
            put(out, i32::from(items::selected(h, k, i)));
            S_OK
        }
        Err(e) => e,
    }
}

extern "system" fn get_selection_container(this: *mut Pattern, out: *mut *mut c_void) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    match item(this) {
        Ok((h, _, _)) => {
            let e = element::new_element(Node::window(h));
            put(out, e);
            if e.is_null() { E_OUTOFMEMORY } else { S_OK }
        }
        Err(e) => e,
    }
}

/// The cached getters read current values (no cache).
static SELECTION_ITEM_VTABLE: VTable<10> = VTable([
    pattern_query as *const (),
    pattern_add_ref as *const (),
    pattern_release as *const (),
    select as *const (),
    add_to_selection as *const (),
    remove_from_selection as *const (),
    get_is_selected as *const (),
    get_selection_container as *const (),
    get_is_selected as *const (),
    get_selection_container as *const (),
]);

// ---- ScrollItem (an item) ----

extern "system" fn scroll_into_view(this: *mut Pattern) -> HResult {
    match item(this) {
        Ok((h, k, i)) => items::scroll_into_view(h, k, i),
        Err(e) => e,
    }
}

static SCROLL_ITEM_VTABLE: VTable<4> = VTable([
    pattern_query as *const (),
    pattern_add_ref as *const (),
    pattern_release as *const (),
    scroll_into_view as *const (),
]);

// ---- ExpandCollapse (a menu item with a submenu) ----

/// ExpandCollapseState_Collapsed 0, _Expanded 1.
fn expand_state(n: Node) -> i32 {
    i32::from(menus::item(n).is_some_and(|it| it.flags & menus::OPEN != 0))
}

/// An action on a menu item (menus::act): its menu opens (as a click on
/// it opens it), it is chosen, or its menu closes; these return before
/// the program has acted, as Invoke may.
fn menu_action(n: Node, op: usize) -> HResult {
    let Some(it) = menus::item(n) else { return UIA_E_ELEMENTNOTAVAILABLE };
    if it.flags & menus::DISABLED != 0 {
        return UIA_E_ELEMENTNOTENABLED;
    }
    match n.item {
        Some(k) if menus::act(n.hwnd, k, op) => S_OK,
        _ => UIA_E_INVALIDOPERATION,
    }
}

extern "system" fn expand(this: *mut Pattern) -> HResult {
    match usable(this) {
        Ok(_) if expand_state(node(this)) == 1 => S_OK,
        Ok(_) => menu_action(node(this), menus::EXPAND),
        Err(e) => e,
    }
}

extern "system" fn collapse(this: *mut Pattern) -> HResult {
    match usable(this) {
        Ok(_) if expand_state(node(this)) == 0 => S_OK,
        Ok(_) => menu_action(node(this), menus::COLLAPSE),
        Err(e) => e,
    }
}

extern "system" fn get_expand_state(this: *mut Pattern, out: *mut i32) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, 0);
    match alive(this) {
        Ok(_) => {
            put(out, expand_state(node(this)));
            S_OK
        }
        Err(e) => e,
    }
}

static EXPAND_COLLAPSE_VTABLE: VTable<7> = VTable([
    pattern_query as *const (),
    pattern_add_ref as *const (),
    pattern_release as *const (),
    expand as *const (),
    collapse as *const (),
    get_expand_state as *const (),
    get_expand_state as *const (),
]);

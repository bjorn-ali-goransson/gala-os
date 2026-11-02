//! Top-level windows as Windows' Win32 window proxy exposes them: the
//! Window pattern (IUIAutomationWindowPattern), the Transform pattern
//! (IUIAutomationTransformPattern) and the title bar.
//!
//! - A top-level window with a caption (ControlType Window) has both
//!   patterns. Window: Close posts WM_SYSCOMMAND SC_CLOSE (the window
//!   closes as from its close button; posted, so a "save changes?"
//!   dialog does not block the client); SetWindowVisualState sends
//!   SC_RESTORE, SC_MAXIMIZE or SC_MINIMIZE (UIA_E_INVALIDOPERATION
//!   without WS_MAXIMIZEBOX / WS_MINIMIZEBOX, as IWindowProvider
//!   documents for an unsupported state); CanMaximize and CanMinimize are
//!   those styles; IsModal: owned by a disabled window; IsTopmost:
//!   WS_EX_TOPMOST; WindowVisualState from IsIconic and IsZoomed;
//!   WindowInteractionState BlockedByModalWindow while disabled, else
//!   ReadyForUserInteraction. Transform: Move and Resize are SetWindowPos
//!   (another process's window too: user32 sends it there); CanMove
//!   unless maximized or minimized, CanResize also needs WS_THICKFRAME;
//!   CanRotate FALSE and Rotate UIA_E_INVALIDOPERATION, as
//!   ITransformProvider documents when CanRotate is FALSE.
//! - The title bar: a TitleBar element (no HWND; Node.menu TITLE, item
//!   None), the window's first child, named by the window's title,
//!   AutomationId "TitleBar"; with WS_SYSMENU its buttons (item 0, 1, 2):
//!   Minimize and Maximize (named and identified "Restore" while the
//!   window is maximized) when the window has WS_MINIMIZEBOX or
//!   WS_MAXIMIZEBOX (each enabled with its own style), Close always.
//!   They are Buttons whose AutomationIds are their names: what FlaUI's
//!   TitleBar looks them up by (MinimizeButton = AutomationId "Minimize",
//!   etc.; Window.Close invokes CloseButton first). The names and ids are
//!   what Windows' proxy is reported to give; unverified on Windows here.
//!   Invoke posts the button's SC_ command. Rectangles: the caption band
//!   (SM_CYCAPTION below the top border) and the buttons at its right
//!   (SM_CXSIZE wide each).

use crate::element::{self, Node};
use crate::pattern::{Pattern, alive, node, pattern_add_ref, pattern_query, pattern_release};
use crate::{
    E_INVALIDARG, E_POINTER, GetSystemMetrics, GetWindow, GetWindowLongW, GetWindowRect, Guid,
    HResult, Hwnd, IsIconic, IsWindowEnabled, IsZoomed, PostMessageW, S_OK, SendMessageW,
    SetWindowPos, UIA_E_ELEMENTNOTENABLED, UIA_E_INVALIDOPERATION, VTable, Variant, guid, put,
};

pub const WINDOW: i32 = 10009;
pub const TRANSFORM: i32 = 10016;

pub const IID_IUIAUTOMATIONWINDOWPATTERN: Guid =
    guid(0x0faef453, 0x9208, 0x43ef, [0xbb, 0xb2, 0x3b, 0x48, 0x51, 0x77, 0x86, 0x4f]);
pub const IID_IUIAUTOMATIONTRANSFORMPATTERN: Guid =
    guid(0xa9b55844, 0xa55d, 0x4ef0, [0x92, 0x6d, 0x56, 0x9c, 0x16, 0xff, 0x89, 0xbb]);

// Pattern properties (UIAutomationClient.h).
const CAN_MAXIMIZE: i32 = 30073;
const CAN_MINIMIZE: i32 = 30074;
const VISUAL_STATE: i32 = 30075;
const INTERACTION_STATE: i32 = 30076;
const IS_MODAL: i32 = 30077;
const IS_TOPMOST: i32 = 30078;
const CAN_MOVE: i32 = 30087;
const CAN_RESIZE: i32 = 30088;
const CAN_ROTATE: i32 = 30089;

/// Node.menu: the title bar of window `hwnd` (item None) or its button.
pub const TITLE: u8 = 3;
/// The buttons' items.
pub const MINIMIZE: u32 = 0;
pub const MAXIMIZE: u32 = 1;
pub const CLOSE: u32 = 2;

const GWL_EXSTYLE: i32 = -20;
const WS_SYSMENU: u32 = 0x0008_0000;
const WS_THICKFRAME: u32 = 0x0004_0000;
const WS_MINIMIZEBOX: u32 = 0x0002_0000;
const WS_MAXIMIZEBOX: u32 = 0x0001_0000;
const WS_EX_TOPMOST: u32 = 0x0000_0008;
const GW_OWNER: u32 = 4;
const SM_CXBORDER: i32 = 5;
const SM_CYCAPTION: i32 = 4;
const SM_CXSIZE: i32 = 30;
const WM_SYSCOMMAND: u32 = 0x0112;
const SC_MINIMIZE: usize = 0xf020;
const SC_MAXIMIZE: usize = 0xf030;
const SC_CLOSE: usize = 0xf060;
const SC_RESTORE: usize = 0xf120;
const SWP_NOSIZE: u32 = 0x1;
const SWP_NOMOVE: u32 = 0x2;
const SWP_NOZORDER: u32 = 0x4;
const SWP_NOACTIVATE: u32 = 0x10;

// WindowVisualState and WindowInteractionState.
const NORMAL: i32 = 0;
const MAXIMIZED: i32 = 1;
const MINIMIZED: i32 = 2;
const READY_FOR_USER_INTERACTION: i32 = 2;
const BLOCKED_BY_MODAL_WINDOW: i32 = 3;

fn style(h: Hwnd) -> u32 {
    element::style(h)
}

fn ex_style(h: Hwnd) -> u32 {
    // SAFETY: a plain call.
    unsafe { GetWindowLongW(h, GWL_EXSTYLE) as u32 }
}

fn zoomed(h: Hwnd) -> bool {
    // SAFETY: a plain call.
    unsafe { IsZoomed(h) != 0 }
}

fn iconic(h: Hwnd) -> bool {
    // SAFETY: a plain call.
    unsafe { IsIconic(h) != 0 }
}

fn enabled(h: Hwnd) -> bool {
    // SAFETY: a plain call.
    unsafe { IsWindowEnabled(h) != 0 }
}

/// Whether the node is a window that has the Window and Transform
/// patterns and a title bar: a top-level window with a caption.
pub fn is_frame(n: Node) -> bool {
    n.menu == 0
        && n.item.is_none()
        && n.hwnd != element::desktop()
        && element::control_type(n.hwnd) == element::WINDOW
}

pub fn visual_state(h: Hwnd) -> i32 {
    if iconic(h) {
        MINIMIZED
    } else if zoomed(h) {
        MAXIMIZED
    } else {
        NORMAL
    }
}

fn is_modal(h: Hwnd) -> bool {
    // SAFETY: a plain call.
    let owner = unsafe { GetWindow(h, GW_OWNER) };
    owner != 0 && owner != h && !enabled(owner)
}

fn can_move(h: Hwnd) -> bool {
    !zoomed(h) && !iconic(h)
}

fn can_resize(h: Hwnd) -> bool {
    can_move(h) && style(h) & WS_THICKFRAME != 0
}

/// A Window or Transform pattern property of a frame (None for another
/// property or element).
pub fn property(n: Node, id: i32) -> Option<Variant> {
    if !is_frame(n) {
        return None;
    }
    let h = n.hwnd;
    Some(match id {
        CAN_MAXIMIZE => Variant::boolean(style(h) & WS_MAXIMIZEBOX != 0),
        CAN_MINIMIZE => Variant::boolean(style(h) & WS_MINIMIZEBOX != 0),
        VISUAL_STATE => Variant::int(visual_state(h)),
        INTERACTION_STATE => Variant::int(if enabled(h) {
            READY_FOR_USER_INTERACTION
        } else {
            BLOCKED_BY_MODAL_WINDOW
        }),
        IS_MODAL => Variant::boolean(is_modal(h)),
        IS_TOPMOST => Variant::boolean(ex_style(h) & WS_EX_TOPMOST != 0),
        CAN_MOVE => Variant::boolean(can_move(h)),
        CAN_RESIZE => Variant::boolean(can_resize(h)),
        CAN_ROTATE => Variant::boolean(false),
        _ => return None,
    })
}

pub fn known_property(id: i32) -> bool {
    matches!(id, CAN_MAXIMIZE..=IS_TOPMOST | CAN_MOVE..=CAN_ROTATE)
}

// ---- The title bar ----

/// Which buttons window `h`'s title bar has: Minimize, Maximize, Close.
pub fn buttons(h: Hwnd) -> [bool; 3] {
    let s = style(h);
    let menu = s & WS_SYSMENU != 0;
    let boxes = menu && s & (WS_MINIMIZEBOX | WS_MAXIMIZEBOX) != 0;
    [boxes, boxes, menu]
}

/// Whether a title-bar node is there.
pub fn exists(n: Node) -> bool {
    let h = n.hwnd;
    element::alive(h)
        && is_frame(Node::window(h))
        && n.item.is_none_or(|k| buttons(h).get(k as usize).copied().unwrap_or(false))
}

/// The title bar's buttons that are there, in order.
pub fn children(n: Node, out: &mut crate::List<Node>) {
    if n.item.is_some() {
        return;
    }
    for (k, &there) in buttons(n.hwnd).iter().enumerate() {
        if there && !out.push(Node::menu(n.hwnd, Some(k as u32), TITLE)) {
            return;
        }
    }
}

/// A button's name and AutomationId: "Minimize", "Maximize" or
/// "Restore" (a maximized window's), "Close".
pub fn button_name(n: Node) -> &'static str {
    match n.item {
        Some(MINIMIZE) => "Minimize",
        Some(MAXIMIZE) if zoomed(n.hwnd) => "Restore",
        Some(MAXIMIZE) => "Maximize",
        _ => "Close",
    }
}

/// Whether the title bar or button is enabled: the window, and the
/// button's own style (WS_MINIMIZEBOX, WS_MAXIMIZEBOX).
pub fn is_enabled(n: Node) -> bool {
    let h = n.hwnd;
    enabled(h)
        && match n.item {
            Some(MINIMIZE) => style(h) & WS_MINIMIZEBOX != 0,
            Some(MAXIMIZE) => style(h) & WS_MAXIMIZEBOX != 0,
            _ => true,
        }
}

/// The title bar's band or a button's rectangle on the screen.
pub fn rect(n: Node) -> [i32; 4] {
    let mut r = [0; 4];
    // SAFETY: our RECT; plain calls.
    let (b, cap, w) = unsafe {
        GetWindowRect(n.hwnd, &mut r);
        (GetSystemMetrics(SM_CXBORDER), GetSystemMetrics(SM_CYCAPTION), GetSystemMetrics(SM_CXSIZE))
    };
    let bar = [r[0] + b, r[1] + b, r[2] - b, r[1] + b + cap];
    match n.item {
        None => bar,
        // From the right: Close, Maximize, Minimize.
        Some(k) => {
            let right = bar[2] - w * (2 - k as i32);
            [right - w, bar[1], right, bar[3]]
        }
    }
}

/// Invoke on a title-bar button: its command, posted (Invoke may return
/// before the window has acted).
fn press(n: Node) -> HResult {
    if !is_enabled(n) {
        return UIA_E_ELEMENTNOTENABLED;
    }
    let sc = match n.item {
        Some(MINIMIZE) => SC_MINIMIZE,
        Some(MAXIMIZE) if zoomed(n.hwnd) => SC_RESTORE,
        Some(MAXIMIZE) => SC_MAXIMIZE,
        Some(CLOSE) => SC_CLOSE,
        _ => return UIA_E_INVALIDOPERATION,
    };
    // SAFETY: a plain call.
    unsafe { PostMessageW(n.hwnd, WM_SYSCOMMAND, sc, 0) };
    S_OK
}

/// The Invoke pattern's call for a title-bar node (pattern.rs).
pub fn invoke(n: Node) -> HResult {
    press(n)
}

// ---- The pattern objects ----

/// The pattern's window if it is still a frame.
fn frame(this: *mut Pattern) -> Result<Hwnd, HResult> {
    let h = alive(this)?;
    if is_frame(node(this)) { Ok(h) } else { Err(crate::UIA_E_ELEMENTNOTAVAILABLE) }
}

fn flag(this: *mut Pattern, id: i32, out: *mut i32) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, 0);
    if let Err(e) = frame(this) {
        return e;
    }
    let v = property(node(this), id).unwrap_or(Variant::EMPTY);
    put(
        out,
        if id == VISUAL_STATE || id == INTERACTION_STATE {
            v.as_int()
        } else {
            i32::from(v.as_bool())
        },
    );
    S_OK
}

extern "system" fn close(this: *mut Pattern) -> HResult {
    match frame(this) {
        Ok(h) => {
            // SAFETY: a plain call.
            unsafe { PostMessageW(h, WM_SYSCOMMAND, SC_CLOSE, 0) };
            S_OK
        }
        Err(e) => e,
    }
}

/// WaitForInputIdle: gala does not track input idleness; a window that is
/// there counts as idle (success TRUE at once). gala's choice.
extern "system" fn wait_for_input_idle(this: *mut Pattern, _ms: i32, out: *mut i32) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, 0);
    match frame(this) {
        Ok(_) => {
            put(out, 1);
            S_OK
        }
        Err(e) => e,
    }
}

/// SetWindowVisualState: the window's command, sent (the state can be
/// read back when the call returns, unless the program defers it).
extern "system" fn set_visual_state(this: *mut Pattern, state: i32) -> HResult {
    let h = match frame(this) {
        Ok(h) => h,
        Err(e) => return e,
    };
    let s = style(h);
    let sc = match state {
        NORMAL if visual_state(h) == NORMAL => return S_OK,
        NORMAL => SC_RESTORE,
        MAXIMIZED if s & WS_MAXIMIZEBOX == 0 => return UIA_E_INVALIDOPERATION,
        MAXIMIZED => SC_MAXIMIZE,
        MINIMIZED if s & WS_MINIMIZEBOX == 0 => return UIA_E_INVALIDOPERATION,
        MINIMIZED => SC_MINIMIZE,
        _ => return E_INVALIDARG,
    };
    if !enabled(h) {
        return UIA_E_ELEMENTNOTENABLED;
    }
    // SAFETY: a plain call.
    unsafe { SendMessageW(h, WM_SYSCOMMAND, sc, 0) };
    S_OK
}

macro_rules! flags {
    ($($name:ident = $id:expr;)*) => {
        $(
            extern "system" fn $name(this: *mut Pattern, out: *mut i32) -> HResult {
                flag(this, $id, out)
            }
        )*
    };
}

flags! {
    get_can_maximize = CAN_MAXIMIZE;
    get_can_minimize = CAN_MINIMIZE;
    get_is_modal = IS_MODAL;
    get_is_topmost = IS_TOPMOST;
    get_visual_state = VISUAL_STATE;
    get_interaction_state = INTERACTION_STATE;
    get_can_move = CAN_MOVE;
    get_can_resize = CAN_RESIZE;
    get_can_rotate = CAN_ROTATE;
}

/// The cached getters read current values (no cache).
pub static WINDOW_VTABLE: VTable<18> = VTable([
    pattern_query as *const (),
    pattern_add_ref as *const (),
    pattern_release as *const (),
    close as *const (),
    wait_for_input_idle as *const (),
    set_visual_state as *const (),
    get_can_maximize as *const (),
    get_can_minimize as *const (),
    get_is_modal as *const (),
    get_is_topmost as *const (),
    get_visual_state as *const (),
    get_interaction_state as *const (),
    get_can_maximize as *const (),
    get_can_minimize as *const (),
    get_is_modal as *const (),
    get_is_topmost as *const (),
    get_visual_state as *const (),
    get_interaction_state as *const (),
]);

/// SetWindowPos with `flags`, if the window allows (`can`).
fn place(this: *mut Pattern, can: fn(Hwnd) -> bool, v: [f64; 2], flags: u32) -> HResult {
    let h = match frame(this) {
        Ok(h) => h,
        Err(e) => return e,
    };
    if !can(h) {
        return UIA_E_INVALIDOPERATION;
    }
    let (a, b) = (v[0] as i32, v[1] as i32);
    let flags = flags | SWP_NOZORDER | SWP_NOACTIVATE;
    // SAFETY: a plain call (user32 moves another process's window there).
    let ok = unsafe {
        if flags & SWP_NOSIZE != 0 {
            SetWindowPos(h, 0, a, b, 0, 0, flags)
        } else {
            SetWindowPos(h, 0, 0, 0, a, b, flags)
        }
    };
    if ok != 0 { S_OK } else { UIA_E_INVALIDOPERATION }
}

/// Move(x, y): the window's top-left corner on the screen.
extern "system" fn move_to(this: *mut Pattern, x: f64, y: f64) -> HResult {
    place(this, can_move, [x, y], SWP_NOSIZE)
}

/// Resize(width, height): the window's outer size.
extern "system" fn resize(this: *mut Pattern, w: f64, h: f64) -> HResult {
    if w < 0.0 || h < 0.0 {
        return E_INVALIDARG;
    }
    place(this, can_resize, [w, h], SWP_NOMOVE)
}

/// Rotate: windows cannot (CanRotate FALSE).
extern "system" fn rotate(this: *mut Pattern, _degrees: f64) -> HResult {
    match frame(this) {
        Ok(_) => UIA_E_INVALIDOPERATION,
        Err(e) => e,
    }
}

/// The cached getters read current values (no cache).
pub static TRANSFORM_VTABLE: VTable<12> = VTable([
    pattern_query as *const (),
    pattern_add_ref as *const (),
    pattern_release as *const (),
    move_to as *const (),
    resize as *const (),
    rotate as *const (),
    get_can_move as *const (),
    get_can_resize as *const (),
    get_can_rotate as *const (),
    get_can_move as *const (),
    get_can_resize as *const (),
    get_can_rotate as *const (),
]);

/// For pattern.rs: the vtable and IID of the Window or Transform pattern.
pub fn vtable(id: i32) -> (*const (), Guid) {
    if id == WINDOW {
        ((&WINDOW_VTABLE as *const VTable<18>).cast(), IID_IUIAUTOMATIONWINDOWPATTERN)
    } else {
        ((&TRANSFORM_VTABLE as *const VTable<12>).cast(), IID_IUIAUTOMATIONTRANSFORMPATTERN)
    }
}

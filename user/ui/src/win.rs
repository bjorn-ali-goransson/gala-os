//! A window holding a component (Windows only): its view laid out with
//! gdi32's text measure and drawn with gdi32, a click or typing turned
//! into the component's message (`click`, `typed`), logged, given to
//! `update`, and the view built again; its UI Automation provider made
//! from the same laid-out list (`uia`), answering WM_GETOBJECT.
//!
//! - The window's thread owns the component. What other threads read
//!   (UIA clients calling the providers, [`draw`]) is a copy of the
//!   laid-out list, the focus and the window handle, behind a spin lock
//!   held only to read or replace them.
//! - Providers: one per element asked for, naming it by its id, so an
//!   element stays the same element across new views while its id is
//!   there; once gone (or the window is) its provider answers
//!   UIA_E_ELEMENTNOTAVAILABLE. The window's provider is the fragment
//!   root over the elements, its host provider the window's.
//! - Re-entry: while the window's host handles a message, the messages
//!   a window call it makes sends back to the window (ShowWindow's
//!   WM_ACTIVATE, SetWindowPos's WM_SIZE) are DefWindowProc's, never
//!   the host's a second time; a WM_SIZE among them lays the view out
//!   again once the handling ends, and a DestroyWindow among them frees
//!   the host then.
//! - Pictures: an `image` element shows the picture its `src` names,
//!   given to the window with [`picture`] (straight-alpha 0xAARRGGBB
//!   pixels, blended over what is under it); one never given draws
//!   nothing.
//! - Logged: each message handled (`click <id>: <msg>`, `right click
//!   <id>: <msg>`, `input <id>: <msg>`), and a view that could not be laid out (`view: <error>`:
//!   the window keeps the last one), through the program's `log`.

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::ffi::c_void;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use crate::{
    Canvas, Component, Measure, Placed, Set, click, hit, paint, place, right_click, typed, uia,
};

/// A window handle.
pub type Hwnd = *mut c_void;
type Handle = *mut c_void;
type HResult = i32;
type Guid = [u8; 16];

#[repr(C)]
struct WndClassW {
    style: u32,
    proc_: extern "system" fn(Hwnd, u32, usize, isize) -> isize,
    class_extra: i32,
    window_extra: i32,
    instance: Handle,
    icon: Handle,
    cursor: Handle,
    background: Handle,
    menu_name: *const u16,
    class_name: *const u16,
}

#[repr(C)]
struct Msg {
    hwnd: Hwnd,
    message: u32,
    wparam: usize,
    lparam: isize,
    time: u32,
    pt: [i32; 2],
    private: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Point {
    x: i32,
    y: i32,
}

#[repr(C)]
struct PaintStruct {
    hdc: Handle,
    erase: i32,
    paint: Rect,
    restore: i32,
    inc_update: i32,
    reserved: [u8; 32],
}

/// UiaRect.
#[repr(C)]
struct UiaRect {
    left: f64,
    top: f64,
    width: f64,
    height: f64,
}

/// VARIANT (x64).
#[repr(C)]
struct Variant {
    vt: u16,
    reserved: [u16; 3],
    value: u64,
    extra: u64,
}

#[link(name = "user32", kind = "raw-dylib")]
unsafe extern "system" {
    fn RegisterClassW(class: *const WndClassW) -> u16;
    #[allow(clippy::too_many_arguments)]
    fn CreateWindowExW(
        ex_style: u32,
        class: *const u16,
        name: *const u16,
        style: u32,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        parent: Hwnd,
        menu: Handle,
        instance: Handle,
        param: *mut c_void,
    ) -> Hwnd;
    fn DefWindowProcW(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize;
    fn ShowWindow(hwnd: Hwnd, cmd: i32) -> i32;
    fn GetMessageW(msg: *mut Msg, hwnd: Hwnd, min: u32, max: u32) -> i32;
    fn TranslateMessage(msg: *const Msg) -> i32;
    fn DispatchMessageW(msg: *const Msg) -> isize;
    fn PostQuitMessage(code: i32);
    fn BeginPaint(hwnd: Hwnd, ps: *mut PaintStruct) -> Handle;
    fn EndPaint(hwnd: Hwnd, ps: *const PaintStruct) -> i32;
    fn FillRect(dc: Handle, rect: *const Rect, brush: Handle) -> i32;
    fn FrameRect(dc: Handle, rect: *const Rect, brush: Handle) -> i32;
    fn GetClientRect(hwnd: Hwnd, rect: *mut Rect) -> i32;
    fn InvalidateRect(hwnd: Hwnd, rect: *const Rect, erase: i32) -> i32;
    fn SendMessageW(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize;
    fn RegisterWindowMessageW(name: *const u16) -> u32;
    fn LoadCursorW(instance: Handle, name: *const u16) -> Handle;
    fn GetDC(hwnd: Hwnd) -> Handle;
    fn ReleaseDC(hwnd: Hwnd, dc: Handle) -> i32;
    fn DrawTextW(dc: Handle, text: *const u16, n: i32, rect: *mut Rect, format: u32) -> i32;
    fn ClientToScreen(hwnd: Hwnd, p: *mut Point) -> i32;
    fn ScreenToClient(hwnd: Hwnd, p: *mut Point) -> i32;
    fn SetWindowLongPtrW(hwnd: Hwnd, index: i32, value: isize) -> isize;
    fn GetWindowLongPtrW(hwnd: Hwnd, index: i32) -> isize;
}

#[link(name = "gdi32", kind = "raw-dylib")]
unsafe extern "system" {
    fn CreateSolidBrush(color: u32) -> Handle;
    fn CreatePen(style: i32, width: i32, color: u32) -> Handle;
    fn GetStockObject(which: i32) -> Handle;
    #[allow(clippy::too_many_arguments)]
    fn CreateFontW(
        height: i32,
        width: i32,
        escapement: i32,
        orientation: i32,
        weight: i32,
        italic: u32,
        underline: u32,
        strike: u32,
        charset: u32,
        out_precision: u32,
        clip_precision: u32,
        quality: u32,
        pitch: u32,
        face: *const u16,
    ) -> Handle;
    fn SelectObject(dc: Handle, object: Handle) -> Handle;
    fn DeleteObject(object: Handle) -> i32;
    fn SetTextColor(dc: Handle, color: u32) -> u32;
    fn SetBkMode(dc: Handle, mode: i32) -> i32;
    fn RoundRect(dc: Handle, l: i32, t: i32, r: i32, b: i32, w: i32, h: i32) -> i32;
    fn CreateCompatibleDC(dc: Handle) -> Handle;
    fn DeleteDC(dc: Handle) -> i32;
    fn CreateDIBSection(
        dc: Handle,
        info: *const BitmapInfoHeader,
        usage: u32,
        bits: *mut *mut c_void,
        section: Handle,
        offset: u32,
    ) -> Handle;
    #[allow(clippy::too_many_arguments)]
    fn GdiAlphaBlend(
        dst: Handle,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        src: Handle,
        sx: i32,
        sy: i32,
        sw: i32,
        sh: i32,
        blend: u32,
    ) -> i32;
}

/// BITMAPINFOHEADER (a 32-bit DIB needs no colour table).
#[repr(C)]
struct BitmapInfoHeader {
    size: u32,
    width: i32,
    height: i32,
    planes: u16,
    bit_count: u16,
    compression: u32,
    size_image: u32,
    x_ppm: i32,
    y_ppm: i32,
    clr_used: u32,
    clr_important: u32,
}

#[link(name = "oleaut32", kind = "raw-dylib")]
unsafe extern "system" {
    fn SysAllocStringLen(s: *const u16, n: u32) -> *mut u16;
    fn SafeArrayCreateVector(vt: u16, lower: i32, n: u32) -> *mut c_void;
    fn SafeArrayAccessData(sa: *mut c_void, data: *mut *mut c_void) -> HResult;
    fn SafeArrayUnaccessData(sa: *mut c_void) -> HResult;
}

#[link(name = "uiautomationcore", kind = "raw-dylib")]
unsafe extern "system" {
    fn UiaReturnRawElementProvider(
        hwnd: Hwnd,
        wparam: usize,
        lparam: isize,
        provider: *mut c_void,
    ) -> isize;
    fn UiaHostProviderFromHwnd(hwnd: Hwnd, out: *mut *mut c_void) -> HResult;
}

const WM_DESTROY: u32 = 0x0002;
const WM_SIZE: u32 = 0x0005;
const WM_ACTIVATE: u32 = 0x0006;
const WM_HOTKEY: u32 = 0x0312;
const WM_PAINT: u32 = 0x000f;
const WM_ERASEBKGND: u32 = 0x0014;
const WM_GETOBJECT: u32 = 0x003d;
const WM_CHAR: u32 = 0x0102;
const WM_LBUTTONDOWN: u32 = 0x0201;
const WM_LBUTTONUP: u32 = 0x0202;
const WM_RBUTTONUP: u32 = 0x0205;
/// BLENDFUNCTION {AC_SRC_OVER, 0, 255, AC_SRC_ALPHA} as a DWORD.
const BLEND_PER_PIXEL: u32 = 0x01ff_0000;
const WS_OVERLAPPEDWINDOW: u32 = 0x00cf_0000;
const WS_EX_TOPMOST: u32 = 0x0000_0008;
const WS_EX_NOACTIVATE: u32 = 0x0800_0000;
const WS_POPUP: u32 = 0x8000_0000;
const WM_COPYDATA: u32 = 0x004a;
const WM_USER: u32 = 0x0400;
const SW_SHOW: i32 = 5;
const SW_SHOWNOACTIVATE: i32 = 4;
const GWLP_USERDATA: i32 = -21;
/// The class's extra bytes: the window's shared part (`shared_of`).
const SHARED_SLOT: i32 = 0;
const IDC_ARROW: usize = 32512;
const UIA_ROOT_OBJECT_ID: i32 = -25;
const TRANSPARENT: i32 = 1;
const NULL_BRUSH: i32 = 5;
const NULL_PEN: i32 = 8;
const DT_CENTER: u32 = 0x1;
const DT_VCENTER: u32 = 0x4;
const DT_WORDBREAK: u32 = 0x10;
const DT_SINGLELINE: u32 = 0x20;
const DT_CALCRECT: u32 = 0x400;
const DT_NOPREFIX: u32 = 0x800;
const DT_END_ELLIPSIS: u32 = 0x8000;
const S_OK: HResult = 0;
const E_NOINTERFACE: HResult = 0x8000_4002_u32 as i32;
const E_INVALIDARG: HResult = 0x8007_0057_u32 as i32;
const E_FAIL: HResult = 0x8000_4005_u32 as i32;
/// UIA_E_ELEMENTNOTENABLED: SetValue on what is not an input.
const UIA_E_ELEMENTNOTENABLED: HResult = 0x8004_0200_u32 as i32;
/// The Value pattern (UIA_ValuePatternId).
const UIA_VALUE_PATTERN: i32 = 10002;
const UIA_E_ELEMENTNOTAVAILABLE: HResult = 0x8004_0201_u32 as i32;
const VT_I4: u16 = 3;
const VT_BSTR: u16 = 8;
const VT_BOOL: u16 = 11;
const UIA_CONTROL_TYPE: i32 = 30003;
const UIA_NAME: i32 = 30005;
const UIA_HAS_KEYBOARD_FOCUS: i32 = 30008;
const UIA_IS_KEYBOARD_FOCUSABLE: i32 = 30009;
const UIA_AUTOMATION_ID: i32 = 30011;
/// ProviderOptions_ServerSideProvider.
const SERVER_SIDE_PROVIDER: i32 = 1;
/// UiaAppendRuntimeId.
const APPEND_RUNTIME_ID: i32 = 3;
/// The window's background, where the view does not paint.
const WINDOW_BG: u32 = 0xf3f3f3;
/// The typeface everything is set in.
const FACE: &str = "Segoe UI";

const fn guid(a: u32, b: u16, c: u16, d: [u8; 8]) -> Guid {
    let (a, b, c) = (a.to_le_bytes(), b.to_le_bytes(), c.to_le_bytes());
    [a[0], a[1], a[2], a[3], b[0], b[1], c[0], c[1], d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7]]
}

/// IID_IUnknown {00000000-0000-0000-c000-000000000046}.
const IID_IUNKNOWN: Guid = guid(0, 0, 0, [0xc0, 0, 0, 0, 0, 0, 0, 0x46]);
/// IID_IRawElementProviderSimple {d6dd68d1-86fd-4332-8666-9abedea2d24c}.
const IID_SIMPLE: Guid =
    guid(0xd6dd68d1, 0x86fd, 0x4332, [0x86, 0x66, 0x9a, 0xbe, 0xde, 0xa2, 0xd2, 0x4c]);
/// IID_IRawElementProviderFragment {f7063da8-8359-439c-9297-bbc5299a7d87}.
const IID_FRAGMENT: Guid =
    guid(0xf7063da8, 0x8359, 0x439c, [0x92, 0x97, 0xbb, 0xc5, 0x29, 0x9a, 0x7d, 0x87]);
/// IID_IRawElementProviderFragmentRoot {620ce2a5-ab8f-40a9-86cb-de3c75599b58}.
const IID_FRAGMENT_ROOT: Guid =
    guid(0x620ce2a5, 0xab8f, 0x40a9, [0x86, 0xcb, 0xde, 0x3c, 0x75, 0x59, 0x9b, 0x58]);
/// IID_IValueProvider {c7935180-6fb3-4201-b174-7df73adbf64a}.
const IID_VALUE: Guid =
    guid(0xc7935180, 0x6fb3, 0x4201, [0xb1, 0x74, 0x7d, 0xf7, 0x3a, 0xdb, 0xf6, 0x4a]);

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// 0xRRGGBB as a COLORREF (0x00BBGGRR).
fn colorref(rgb: u32) -> u32 {
    ((rgb & 0xff) << 16) | (rgb & 0xff00) | ((rgb >> 16) & 0xff)
}

/// Where and how the window opens.
pub struct Options<'a> {
    pub title: &'a str,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    /// Above other windows (WS_EX_TOPMOST).
    pub topmost: bool,
    /// Without a frame and a caption (WS_POPUP): a panel the program
    /// places itself; not on the taskbar.
    pub popup: bool,
    /// Never made the active window, by a click or by being shown
    /// (WS_EX_NOACTIVATE, SW_SHOWNOACTIVATE): a taskbar, which acts on
    /// the window that is active.
    pub no_activate: bool,
    /// Made without being shown: a menu the program shows when asked
    /// (ShowWindow).
    pub hidden: bool,
    /// The window's class name, when others find the window by it
    /// (FindWindow: a shell's taskbar is `Shell_TrayWnd`); None for
    /// "GalaUiWindow".
    pub class: Option<&'a str>,
}

/// A picture an `image` element shows (its `src`): `width` x `height`
/// straight-alpha 0xAARRGGBB pixels, top row first.
#[derive(Clone)]
struct Picture {
    src: String,
    width: i32,
    height: i32,
    pixels: Vec<u32>,
}

/// What other threads read of the window, behind `busy`.
struct Shared {
    busy: AtomicBool,
    state: UnsafeCell<State>,
}

// SAFETY: `state` is only touched while `busy` is held (`Shared::with`).
unsafe impl Sync for Shared {}
// SAFETY: as above; the handle is a window handle, valid on any thread.
unsafe impl Send for Shared {}

struct State {
    hwnd: Hwnd,
    /// The laid-out view, without the program's messages.
    tree: Vec<Placed<()>>,
    /// The input that has the keyboard focus.
    focus: Option<String>,
    /// False once the window is destroyed.
    alive: bool,
    /// A WM_SIZE came while the host was handling a message: laid out
    /// again once it is done.
    resized: bool,
    /// The pictures given ([`picture`]).
    pictures: Vec<Picture>,
}

impl Shared {
    fn with<R>(&self, f: impl FnOnce(&mut State) -> R) -> R {
        while self.busy.swap(true, Ordering::Acquire) {
            core::hint::spin_loop();
        }
        // SAFETY: held exclusively (`busy`).
        let r = f(unsafe { &mut *self.state.get() });
        self.busy.store(false, Ordering::Release);
        r
    }
}

/// The window's part on its thread.
trait Window {
    fn message(&mut self, hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> Option<isize>;
    fn shared(&self) -> &Arc<Shared>;
}

struct Host<C: Component> {
    component: C,
    placed: Vec<Placed<C::Msg>>,
    shared: Arc<Shared>,
    log: fn(&str),
}

/// The list without the program's messages: what other threads read.
fn strip<M>(placed: &[Placed<M>]) -> Vec<Placed<()>> {
    placed
        .iter()
        .map(|p| Placed {
            kind: p.kind,
            id: p.id.clone(),
            rect: p.rect,
            text: p.text.clone(),
            placeholder: p.placeholder.clone(),
            src: p.src.clone(),
            on_click: p.on_click.as_ref().map(|_| ()),
            on_right_click: p.on_right_click.as_ref().map(|_| ()),
            on_input: None,
            background: p.background,
            border: p.border,
            radius: p.radius,
            color: p.color,
            size: p.size,
            bold: p.bold,
            padding: p.padding,
            ellipsis: p.ellipsis,
            parent: p.parent,
        })
        .collect()
}

impl<C: Component> Host<C> {
    /// Lays the view out again for the client area and shows it.
    fn layout(&mut self, hwnd: Hwnd) {
        let mut r = Rect::default();
        // SAFETY: our window; a DC for measuring, released below.
        let dc = unsafe {
            GetClientRect(hwnd, &mut r);
            GetDC(hwnd)
        };
        let view = self.component.view();
        let mut measure = Gdi::new(dc);
        let placed = place(&view, r.right as f32, Some(r.bottom as f32), &mut measure);
        drop(measure);
        // SAFETY: the DC from GetDC above.
        unsafe { ReleaseDC(hwnd, dc) };
        match placed {
            Ok(p) => {
                let tree = strip(&p);
                self.placed = p;
                self.shared.with(|s| {
                    s.tree = tree;
                    // The focus goes with its input.
                    if let Some(f) = &s.focus
                        && uia::find(&s.tree, f).is_none()
                    {
                        s.focus = None;
                    }
                });
            }
            Err(e) => (self.log)(&format!("view: {e}")),
        }
        // SAFETY: our window.
        unsafe { InvalidateRect(hwnd, core::ptr::null(), 0) };
    }

    fn handle(&mut self, hwnd: Hwnd, what: &str, id: &str, msg: C::Msg) {
        (self.log)(&format!("{what} {id}: {msg:?}"));
        self.component.update(msg);
        self.layout(hwnd);
    }
}

impl<C: Component> Window for Host<C> {
    fn shared(&self) -> &Arc<Shared> {
        &self.shared
    }

    fn message(&mut self, hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> Option<isize> {
        let (x, y) = ((lparam & 0xffff) as i16 as i32, ((lparam >> 16) & 0xffff) as i16 as i32);
        // UIA's SetValue on an input: typed (1), or not an input (0).
        if msg == set_value_message() && msg != 0 && lparam != 0 {
            // SAFETY: `set_value`'s request, alive for its send.
            let v = unsafe { &*(lparam as *const SetValue) };
            let input = self
                .placed
                .iter()
                .any(|p| p.kind == crate::Kind::Input && p.id == Some(v.id.clone()));
            if !input {
                return Some(0);
            }
            if let Some(m) = typed(&self.placed, &v.id, v.text.clone()) {
                self.handle(hwnd, "set value", &v.id, m);
            }
            return Some(1);
        }
        match msg {
            WM_SIZE => self.layout(hwnd),
            WM_LBUTTONDOWN => {
                // A click on an input gives it the keyboard focus; anywhere
                // else takes it away.
                let to = hit(&self.placed, x, y)
                    .filter(|p| p.kind == crate::Kind::Input)
                    .and_then(|p| p.id.clone());
                if self.shared.with(|s| core::mem::replace(&mut s.focus, to.clone()) != to) {
                    // SAFETY: our window.
                    unsafe { InvalidateRect(hwnd, core::ptr::null(), 0) };
                }
            }
            WM_LBUTTONUP => {
                if let Some((id, m)) = click(&self.placed, x, y) {
                    let id = String::from(id);
                    self.handle(hwnd, "click", &id, m);
                }
            }
            WM_RBUTTONUP => {
                let (id, m) = right_click(&self.placed, x, y)?;
                let id = String::from(id);
                self.handle(hwnd, "right click", &id, m);
            }
            WM_CHAR => {
                let focus = self.shared.with(|s| s.focus.clone())?;
                let p = self.placed.iter().find(|p| p.id.as_deref() == Some(&focus))?;
                let mut t = p.text.clone();
                match wparam {
                    0x08 => {
                        t.pop()?;
                    }
                    // Printable characters (one UTF-16 unit: surrogate
                    // pairs are not taken yet).
                    c if c >= 0x20 && c != 0x7f => t.push(char::from_u32(c as u32)?),
                    _ => return Some(0),
                }
                if let Some(m) = typed(&self.placed, &focus, t) {
                    self.handle(hwnd, "input", &focus, m);
                }
            }
            WM_PAINT => {
                // SAFETY: the paint structure BeginPaint fills, ended below.
                let mut ps: PaintStruct = unsafe { core::mem::zeroed() };
                // SAFETY: our window.
                let dc = unsafe { BeginPaint(hwnd, &mut ps) };
                draw_into(&self.shared, hwnd, dc);
                // SAFETY: as above.
                unsafe { EndPaint(hwnd, &ps) };
            }
            WM_ERASEBKGND => return Some(1),
            WM_GETOBJECT if lparam as i32 == UIA_ROOT_OBJECT_ID => {
                let root = Provider::new(self.shared.clone(), None);
                // SAFETY: our provider (UiaReturnRawElementProvider takes
                // its own reference); ours is released after.
                let r = unsafe {
                    UiaReturnRawElementProvider(hwnd, wparam, lparam, root.cast::<c_void>())
                };
                release(root);
                return Some(r);
            }
            // Activation and hot keys: the program's to act on, and
            // WM_ACTIVATE's default handling (the focus) still done.
            WM_ACTIVATE | WM_HOTKEY => {
                if let Some(m) = self.component.message(hwnd as usize, msg, wparam, lparam) {
                    self.handle(hwnd, "message", &format!("{msg:#x}"), m);
                }
                return (msg == WM_HOTKEY).then_some(0);
            }
            WM_COPYDATA | WM_USER.. => {
                if let Some(r) = self.component.answer(hwnd as usize, msg, wparam, lparam) {
                    self.layout(hwnd);
                    return Some(r);
                }
                let m = self.component.message(hwnd as usize, msg, wparam, lparam)?;
                self.handle(hwnd, "message", &format!("{msg:#x}"), m);
                // WM_COPYDATA's TRUE: taken.
                return Some(1);
            }
            _ => return None,
        }
        Some(0)
    }
}

/// The window's shared part, for any thread: the class's extra bytes
/// hold an `Arc<Shared>` reference (`open`), given up at WM_DESTROY.
fn shared_of(hwnd: Hwnd) -> Option<Arc<Shared>> {
    // SAFETY: our class's extra bytes: 0 or `Arc::into_raw` of the
    // window's shared part, alive while the window is.
    unsafe {
        let raw = GetWindowLongPtrW(hwnd, SHARED_SLOT) as *const Shared;
        if raw.is_null() {
            return None;
        }
        Arc::increment_strong_count(raw);
        Some(Arc::from_raw(raw))
    }
}

extern "system" fn wndproc(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize {
    if msg == WM_DESTROY {
        // Marked gone first: a host taken out to handle a message (the
        // DestroyWindow came from it) is freed when that ends.
        if let Some(shared) = shared_of(hwnd) {
            shared.with(|s| s.alive = false);
        }
        // SAFETY: our window: the user data is cleared, then freed.
        let raw = unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) } as *mut Box<dyn Window>;
        if !raw.is_null() {
            // SAFETY: leaked by `open`, cleared above: freed once.
            drop(unsafe { Box::from_raw(raw) });
        }
        // SAFETY: the extra bytes' reference (`open`), cleared, then given up.
        unsafe {
            let raw = SetWindowLongPtrW(hwnd, SHARED_SLOT, 0) as *const Shared;
            if !raw.is_null() {
                drop(Arc::from_raw(raw));
            }
        }
        // SAFETY: disconnects the window's providers (all UIA documents).
        unsafe { UiaReturnRawElementProvider(hwnd, 0, 0, core::ptr::null_mut()) };
        // SAFETY: ends the message loop.
        unsafe { PostQuitMessage(0) };
        return 0;
    }
    // The host is taken out of the user data while it handles the
    // message (module docs, "Re-entry"): a message sent back meanwhile
    // finds none.
    // SAFETY: our window class's user data: 0 or a `Box<Box<dyn Window>>`
    // leaked by `open`, freed at WM_DESTROY when set, else below; only
    // the window's thread (its procedure) takes it.
    let raw = unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) } as *mut Box<dyn Window>;
    if raw.is_null() {
        if msg == WM_SIZE
            && let Some(shared) = shared_of(hwnd)
        {
            shared.with(|s| s.resized = true);
        }
        // SAFETY: the default handling.
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    }
    // SAFETY: taken out above: ours alone until it is put back or freed.
    let w = unsafe { &mut *raw };
    let r = w.message(hwnd, msg, wparam, lparam);
    let (alive, resized) = w.shared().with(|s| (s.alive, core::mem::take(&mut s.resized)));
    if !alive {
        // Destroyed while handling: WM_DESTROY found no host to free.
        // SAFETY: leaked by `open`, no longer in the user data: freed once.
        drop(unsafe { Box::from_raw(raw) });
        return r.unwrap_or(0);
    }
    // SAFETY: our window: the host put back.
    unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, raw as isize) };
    if resized {
        wndproc(hwnd, WM_SIZE, 0, 0);
    }
    match r {
        Some(r) => r,
        // SAFETY: the default handling.
        None => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// Gives the keyboard focus to input `id` of window `hwnd` (a window
/// from [`open`]), drawn with it: false when the window has no such
/// input.
pub fn focus(hwnd: Hwnd, id: &str) -> bool {
    let Some(shared) = shared_of(hwnd) else { return false };
    let ok = shared.with(|s| {
        let input = uia::find(&s.tree, id).is_some_and(|i| s.tree[i].kind == crate::Kind::Input);
        if input {
            s.focus = Some(String::from(id));
        }
        input
    });
    if ok {
        repaint(hwnd);
    }
    ok
}

/// Where element `id` of window `hwnd` (a window from [`open`]) is, in
/// the window's client coordinates (x, y, w, h), as last laid out.
pub fn element_rect(hwnd: Hwnd, id: &str) -> Option<[i32; 4]> {
    let shared = shared_of(hwnd)?;
    shared.with(|s| uia::find(&s.tree, id).map(|i| s.tree[i].rect))
}

/// Asks for window `hwnd`'s WM_PAINT (a handle, never dereferenced here).
fn repaint(hwnd: Hwnd) {
    // SAFETY: a window handle; InvalidateRect checks it.
    unsafe { InvalidateRect(hwnd, core::ptr::null(), 0) };
}

/// Gives window `hwnd` (a window from [`open`]) the picture `src` names
/// (`width` x `height` straight-alpha 0xAARRGGBB pixels, top row
/// first), replacing one of the same name, and draws it again: false
/// when `hwnd` is no such window or the pixels are not that many.
pub fn picture(hwnd: Hwnd, src: &str, width: i32, height: i32, pixels: &[u32]) -> bool {
    if width <= 0 || height <= 0 || pixels.len() != (width * height) as usize {
        return false;
    }
    let Some(shared) = shared_of(hwnd) else { return false };
    let mut copy = Vec::new();
    if copy.try_reserve_exact(pixels.len()).is_err() {
        return false;
    }
    copy.extend_from_slice(pixels);
    let p = Picture { src: String::from(src), width, height, pixels: copy };
    shared.with(|s| {
        s.pictures.retain(|q| q.src != p.src);
        s.pictures.push(p);
    });
    repaint(hwnd);
    true
}

/// Forgets window `hwnd`'s picture `src` (its elements draw nothing).
pub fn forget_picture(hwnd: Hwnd, src: &str) {
    if let Some(shared) = shared_of(hwnd) {
        shared.with(|s| s.pictures.retain(|q| q.src != src));
    }
}

static REGISTERED: AtomicBool = AtomicBool::new(false);

/// Opens a window holding `component`, shown unless `o.hidden`. `log`
/// gets each message handled and each view refused (module docs). None when the window
/// could not be made. Closing the window ends [`pump`].
pub fn open<C: Component + 'static>(component: C, o: &Options, log: fn(&str)) -> Option<Hwnd> {
    let class = wide(o.class.unwrap_or("GalaUiWindow"));
    // A class of the program's own is registered for its window.
    if o.class.is_some() || !REGISTERED.swap(true, Ordering::AcqRel) {
        let wc = WndClassW {
            style: 0,
            proc_: wndproc,
            class_extra: 0,
            window_extra: PTR as i32,
            instance: core::ptr::null_mut(),
            icon: core::ptr::null_mut(),
            // SAFETY: a system cursor.
            cursor: unsafe { LoadCursorW(core::ptr::null_mut(), IDC_ARROW as *const u16) },
            background: core::ptr::null_mut(),
            menu_name: core::ptr::null(),
            class_name: class.as_ptr(),
        };
        // SAFETY: a class structure naming a live string and our procedure.
        if unsafe { RegisterClassW(&wc) } == 0 {
            log(&format!("RegisterClassW {} failed", o.class.unwrap_or("GalaUiWindow")));
            if o.class.is_none() {
                REGISTERED.store(false, Ordering::Release);
            }
            return None;
        }
    }
    let title = wide(o.title);
    let ex = if o.topmost { WS_EX_TOPMOST } else { 0 }
        | if o.no_activate { WS_EX_NOACTIVATE } else { 0 };
    // SAFETY: the registered class; created hidden, shown once its host is set.
    let hwnd = unsafe {
        CreateWindowExW(
            ex,
            class.as_ptr(),
            title.as_ptr(),
            if o.popup { WS_POPUP } else { WS_OVERLAPPEDWINDOW },
            o.x,
            o.y,
            o.width,
            o.height,
            core::ptr::null_mut(),
            core::ptr::null_mut(),
            core::ptr::null_mut(),
            core::ptr::null_mut(),
        )
    };
    if hwnd.is_null() {
        log("CreateWindowExW failed");
        return None;
    }
    let shared = Arc::new(Shared {
        busy: AtomicBool::new(false),
        state: UnsafeCell::new(State {
            hwnd,
            tree: Vec::new(),
            focus: None,
            alive: true,
            resized: false,
            pictures: Vec::new(),
        }),
    });
    let mut h = Host { component, placed: Vec::new(), shared: shared.clone(), log };
    h.layout(hwnd);
    let w: Box<Box<dyn Window>> = Box::new(Box::new(h));
    // SAFETY: our window; both given up at WM_DESTROY.
    unsafe {
        SetWindowLongPtrW(hwnd, SHARED_SLOT, Arc::into_raw(shared) as isize);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(w) as isize);
        if !o.hidden {
            ShowWindow(hwnd, if o.no_activate { SW_SHOWNOACTIVATE } else { SW_SHOW });
        }
    }
    Some(hwnd)
}

/// Runs the thread's message loop until a window from [`open`] closes
/// (WM_QUIT); its exit code.
pub fn pump() -> usize {
    // SAFETY: a message structure GetMessageW fills.
    let mut m: Msg = unsafe { core::mem::zeroed() };
    // SAFETY: the thread's messages.
    while unsafe { GetMessageW(&mut m, core::ptr::null_mut(), 0, 0) } > 0 {
        // SAFETY: the message just taken.
        unsafe {
            TranslateMessage(&m);
            DispatchMessageW(&m);
        }
    }
    m.wparam
}

/// Draws the window `hwnd` (one [`open`] made, still open) as it is now
/// into `dc`, at (0, 0): what WM_PAINT draws. From any thread. False
/// when `hwnd` is not such a window.
pub fn draw(hwnd: Hwnd, dc: Handle) -> bool {
    let Some(shared) = shared_of(hwnd) else { return false };
    draw_into(&shared, hwnd, dc);
    true
}

fn draw_into(shared: &Shared, hwnd: Hwnd, dc: Handle) {
    let (tree, focus, pictures) =
        shared.with(|s| (s.tree.clone(), s.focus.clone(), s.pictures.clone()));
    let mut r = Rect::default();
    // SAFETY: our window; a brush for the background, deleted after.
    unsafe {
        GetClientRect(hwnd, &mut r);
        let b = CreateSolidBrush(colorref(WINDOW_BG));
        FillRect(dc, &r, b);
        DeleteObject(b);
    }
    let mut g = Gdi::new(dc);
    g.pictures = pictures;
    paint(&tree, focus.as_deref(), &mut g);
}

/// gdi32 as the measure and the canvas: fonts made as asked (Segoe UI,
/// by size and weight), deleted with it and the DC's own font put back.
/// A program that draws its window itself lays a view out and paints it
/// into its DC with this (Explorer's "Change this program..." cards).
pub struct Gdi {
    dc: Handle,
    fonts: Vec<((u32, bool), Handle)>,
    old_font: Handle,
    /// What `image` elements show (a window's, [`picture`]).
    pictures: Vec<Picture>,
}

impl Gdi {
    /// On `dc`, which outlives it.
    pub fn new(dc: Handle) -> Gdi {
        Gdi { dc, fonts: Vec::new(), old_font: core::ptr::null_mut(), pictures: Vec::new() }
    }

    fn font(&mut self, size: f32, bold: bool) {
        let key = (size.to_bits(), bold);
        let f = match self.fonts.iter().find(|(k, _)| *k == key) {
            Some((_, f)) => *f,
            None => {
                let face = wide(FACE);
                // SAFETY: a font by its pixel height (negative: the
                // character's), deleted in `drop`.
                let f = unsafe {
                    CreateFontW(
                        -((size + 0.5) as i32),
                        0,
                        0,
                        0,
                        if bold { 700 } else { 400 },
                        0,
                        0,
                        0,
                        1,
                        0,
                        0,
                        5,
                        0,
                        face.as_ptr(),
                    )
                };
                self.fonts.push((key, f));
                f
            }
        };
        // SAFETY: our DC and font.
        let old = unsafe { SelectObject(self.dc, f) };
        if self.old_font.is_null() {
            self.old_font = old;
        }
    }

    fn draw_text(&mut self, text: &str, r: &mut Rect, format: u32) {
        let w: Vec<u16> = text.encode_utf16().collect();
        // SAFETY: our DC; the text and its length.
        unsafe { DrawTextW(self.dc, w.as_ptr(), w.len() as i32, r, format | DT_NOPREFIX) };
    }
}

impl Drop for Gdi {
    fn drop(&mut self) {
        // SAFETY: the DC's font back, then ours deleted.
        unsafe {
            if !self.old_font.is_null() {
                SelectObject(self.dc, self.old_font);
            }
            for (_, f) in &self.fonts {
                DeleteObject(*f);
            }
        }
    }
}

impl Measure for Gdi {
    fn text(&mut self, text: &str, size: f32, bold: bool, max: Option<f32>) -> (f32, f32) {
        self.font(size, bold);
        let (right, format) = match max {
            Some(m) => (m as i32, DT_CALCRECT | DT_WORDBREAK),
            None => (0, DT_CALCRECT | DT_SINGLELINE),
        };
        let mut r = Rect { left: 0, top: 0, right, bottom: 0 };
        // An empty text is a line high.
        self.draw_text(if text.is_empty() { " " } else { text }, &mut r, format);
        let w = if text.is_empty() { 0 } else { r.right - r.left };
        (w as f32, (r.bottom - r.top) as f32)
    }
}

fn rect([x, y, w, h]: [i32; 4]) -> Rect {
    Rect { left: x, top: y, right: x + w, bottom: y + h }
}

impl Canvas for Gdi {
    fn fill(&mut self, at: [i32; 4], color: u32, radius: i32) {
        let r = rect(at);
        // SAFETY: our DC; the brush deleted after, the DC's objects back.
        unsafe {
            let b = CreateSolidBrush(colorref(color));
            if radius <= 0 {
                FillRect(self.dc, &r, b);
            } else {
                let ob = SelectObject(self.dc, b);
                let op = SelectObject(self.dc, GetStockObject(NULL_PEN));
                // Without a pen the shape stops a pixel short: one more.
                RoundRect(
                    self.dc,
                    r.left,
                    r.top,
                    r.right + 1,
                    r.bottom + 1,
                    radius * 2,
                    radius * 2,
                );
                SelectObject(self.dc, op);
                SelectObject(self.dc, ob);
            }
            DeleteObject(b);
        }
    }

    fn frame(&mut self, at: [i32; 4], color: u32, radius: i32) {
        let r = rect(at);
        // SAFETY: as in `fill`.
        unsafe {
            if radius <= 0 {
                let b = CreateSolidBrush(colorref(color));
                FrameRect(self.dc, &r, b);
                DeleteObject(b);
            } else {
                let p = CreatePen(0, 1, colorref(color));
                let op = SelectObject(self.dc, p);
                let ob = SelectObject(self.dc, GetStockObject(NULL_BRUSH));
                RoundRect(self.dc, r.left, r.top, r.right, r.bottom, radius * 2, radius * 2);
                SelectObject(self.dc, ob);
                SelectObject(self.dc, op);
                DeleteObject(p);
            }
        }
    }

    fn text(&mut self, at: [i32; 4], text: &str, color: u32, size: f32, bold: bool, set: Set) {
        self.font(size, bold);
        let mut r = rect(at);
        let format = match set {
            Set::Wrap => DT_WORDBREAK,
            Set::Centre => DT_SINGLELINE | DT_CENTER | DT_VCENTER,
            Set::Line => DT_SINGLELINE | DT_VCENTER,
            Set::Cut => DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
        };
        // SAFETY: our DC.
        unsafe {
            SetTextColor(self.dc, colorref(color));
            SetBkMode(self.dc, TRANSPARENT);
        }
        self.draw_text(text, &mut r, format);
    }

    /// The picture `src` names, scaled to `at` and blended over what is
    /// there (GdiAlphaBlend with its premultiplied pixels).
    fn image(&mut self, at: [i32; 4], src: &str) {
        let Some(p) = self.pictures.iter().find(|p| p.src == src) else { return };
        let info = BitmapInfoHeader {
            size: size_of::<BitmapInfoHeader>() as u32,
            width: p.width,
            // Negative: top row first.
            height: -p.height,
            planes: 1,
            bit_count: 32,
            compression: 0,
            size_image: 0,
            x_ppm: 0,
            y_ppm: 0,
            clr_used: 0,
            clr_important: 0,
        };
        let mut bits: *mut c_void = core::ptr::null_mut();
        // SAFETY: a DC and a DIB section of the picture's size, both
        // deleted below; the section's bits are width x height u32s.
        unsafe {
            let mem = CreateCompatibleDC(self.dc);
            if mem.is_null() {
                return;
            }
            let dib = CreateDIBSection(mem, &info, 0, &mut bits, core::ptr::null_mut(), 0);
            if dib.is_null() || bits.is_null() {
                DeleteDC(mem);
                return;
            }
            let out = core::slice::from_raw_parts_mut(bits.cast::<u32>(), p.pixels.len());
            for (o, &px) in out.iter_mut().zip(&p.pixels) {
                *o = premultiplied(px);
            }
            let old = SelectObject(mem, dib);
            let [x, y, w, h] = at;
            GdiAlphaBlend(self.dc, x, y, w, h, mem, 0, 0, p.width, p.height, BLEND_PER_PIXEL);
            SelectObject(mem, old);
            DeleteObject(dib);
            DeleteDC(mem);
        }
    }
}

/// A straight-alpha 0xAARRGGBB pixel with its colour scaled by its alpha.
fn premultiplied(px: u32) -> u32 {
    let a = px >> 24;
    let scale = |shift: u32| ((((px >> shift) & 0xff) * a + 127) / 255) << shift;
    a << 24 | scale(16) | scale(8) | scale(0)
}

// ---- UI Automation: the providers ----

#[repr(C)]
struct VTable<const N: usize>([*const (); N]);
// SAFETY: function pointers only.
unsafe impl<const N: usize> Sync for VTable<N> {}

/// One element's provider: IRawElementProviderSimple,
/// IRawElementProviderFragment and, for the window's (`id` None),
/// IRawElementProviderFragmentRoot, each a vtable pointer here.
#[repr(C)]
struct Provider {
    simple: *const VTable<7>,
    fragment: *const VTable<9>,
    root: *const VTable<5>,
    /// IValueProvider: an input's (its text; SetValue types it).
    value: *const VTable<6>,
    refs: AtomicU32,
    shared: Arc<Shared>,
    /// The element's id; None for the window's (the fragment root).
    id: Option<String>,
}

const PTR: usize = core::mem::size_of::<usize>();

impl Provider {
    /// A provider with one reference, as its IRawElementProviderSimple.
    fn new(shared: Arc<Shared>, id: Option<String>) -> *mut Provider {
        Box::into_raw(Box::new(Provider {
            simple: &SIMPLE,
            fragment: &FRAGMENT,
            root: &ROOT,
            value: &VALUE,
            refs: AtomicU32::new(1),
            shared,
            id,
        }))
    }

    /// The provider an interface pointer at `offset` in it belongs to.
    ///
    /// # Safety
    /// `this` points at that field of a live provider.
    unsafe fn of<'a>(this: *mut c_void, offset: usize) -> &'a Provider {
        // SAFETY: the caller's.
        unsafe { &*this.cast::<u8>().sub(offset).cast::<Provider>() }
    }

    fn as_fragment(p: *mut Provider) -> *mut c_void {
        p.cast::<u8>().wrapping_add(PTR).cast()
    }

    fn as_root(p: *mut Provider) -> *mut c_void {
        p.cast::<u8>().wrapping_add(2 * PTR).cast()
    }

    fn as_value(p: *mut Provider) -> *mut c_void {
        p.cast::<u8>().wrapping_add(3 * PTR).cast()
    }

    /// Whether this element is an input (it has the Value pattern).
    fn is_input(&self) -> bool {
        self.shared.with(|s| {
            self.index(s).ok().flatten().is_some_and(|i| s.tree[i].kind == crate::Kind::Input)
        })
    }

    /// This element's index in the current list, or why there is none.
    fn index(&self, s: &State) -> Result<Option<usize>, HResult> {
        if !s.alive {
            return Err(UIA_E_ELEMENTNOTAVAILABLE);
        }
        match &self.id {
            None => Ok(None),
            Some(id) => uia::find(&s.tree, id).map(Some).ok_or(UIA_E_ELEMENTNOTAVAILABLE),
        }
    }

    /// A new provider for element `at` (None: the window's), as its
    /// IRawElementProviderFragment.
    fn fragment_for(&self, s: &State, at: Option<usize>) -> *mut c_void {
        let id = at.and_then(|i| s.tree[i].id.clone());
        Provider::as_fragment(Provider::new(self.shared.clone(), id))
    }
}

fn release(p: *mut Provider) {
    // SAFETY: a provider we hold a reference to.
    unsafe { p_release(p.cast(), 0) };
}

static SIMPLE: VTable<7> = VTable([
    simple_query as *const (),
    simple_add_ref as *const (),
    simple_release as *const (),
    provider_options as *const (),
    pattern_provider as *const (),
    property_value as *const (),
    host_provider as *const (),
]);

static FRAGMENT: VTable<9> = VTable([
    fragment_query as *const (),
    fragment_add_ref as *const (),
    fragment_release as *const (),
    navigate as *const (),
    runtime_id as *const (),
    bounding_rectangle as *const (),
    embedded_roots as *const (),
    set_focus as *const (),
    fragment_root as *const (),
]);

static VALUE: VTable<6> = VTable([
    value_query as *const (),
    value_add_ref as *const (),
    value_release as *const (),
    set_value as *const (),
    get_value as *const (),
    get_is_read_only as *const (),
]);

static ROOT: VTable<5> = VTable([
    root_query as *const (),
    root_add_ref as *const (),
    root_release as *const (),
    from_point as *const (),
    get_focus as *const (),
]);

/// QueryInterface on the provider whose interface at `offset` is `this`.
///
/// # Safety
/// As [`Provider::of`]; `iid` and `out` valid.
unsafe fn p_query(
    this: *mut c_void,
    offset: usize,
    iid: *const Guid,
    out: *mut *mut c_void,
) -> HResult {
    // SAFETY: the caller's.
    let (p, iid) = unsafe { (Provider::of(this, offset), &*iid) };
    let base = (p as *const Provider).cast_mut();
    let to = if *iid == IID_IUNKNOWN || *iid == IID_SIMPLE {
        base.cast()
    } else if *iid == IID_FRAGMENT {
        Provider::as_fragment(base)
    } else if *iid == IID_FRAGMENT_ROOT && p.id.is_none() {
        Provider::as_root(base)
    } else if *iid == IID_VALUE && p.is_input() {
        Provider::as_value(base)
    } else {
        // SAFETY: the caller's.
        unsafe { *out = core::ptr::null_mut() };
        return E_NOINTERFACE;
    };
    p.refs.fetch_add(1, Ordering::Relaxed);
    // SAFETY: the caller's.
    unsafe { *out = to };
    S_OK
}

/// # Safety
/// As [`Provider::of`].
unsafe fn p_add_ref(this: *mut c_void, offset: usize) -> u32 {
    // SAFETY: the caller's.
    unsafe { Provider::of(this, offset) }.refs.fetch_add(1, Ordering::Relaxed) + 1
}

/// # Safety
/// As [`Provider::of`]; the caller's reference is given up.
unsafe fn p_release(this: *mut c_void, offset: usize) -> u32 {
    // SAFETY: the caller's.
    let p = unsafe { Provider::of(this, offset) };
    let n = p.refs.fetch_sub(1, Ordering::AcqRel) - 1;
    if n == 0 {
        // SAFETY: the last reference: made by `Provider::new`.
        drop(unsafe { Box::from_raw((p as *const Provider).cast_mut()) });
    }
    n
}

extern "system" fn simple_query(
    this: *mut c_void,
    iid: *const Guid,
    out: *mut *mut c_void,
) -> HResult {
    // SAFETY: COM calls this on our IRawElementProviderSimple.
    unsafe { p_query(this, 0, iid, out) }
}
extern "system" fn simple_add_ref(this: *mut c_void) -> u32 {
    // SAFETY: as above.
    unsafe { p_add_ref(this, 0) }
}
extern "system" fn simple_release(this: *mut c_void) -> u32 {
    // SAFETY: as above.
    unsafe { p_release(this, 0) }
}
extern "system" fn fragment_query(
    this: *mut c_void,
    iid: *const Guid,
    out: *mut *mut c_void,
) -> HResult {
    // SAFETY: COM calls this on our IRawElementProviderFragment.
    unsafe { p_query(this, PTR, iid, out) }
}
extern "system" fn fragment_add_ref(this: *mut c_void) -> u32 {
    // SAFETY: as above.
    unsafe { p_add_ref(this, PTR) }
}
extern "system" fn fragment_release(this: *mut c_void) -> u32 {
    // SAFETY: as above.
    unsafe { p_release(this, PTR) }
}
extern "system" fn root_query(
    this: *mut c_void,
    iid: *const Guid,
    out: *mut *mut c_void,
) -> HResult {
    // SAFETY: COM calls this on our IRawElementProviderFragmentRoot.
    unsafe { p_query(this, 2 * PTR, iid, out) }
}
extern "system" fn root_add_ref(this: *mut c_void) -> u32 {
    // SAFETY: as above.
    unsafe { p_add_ref(this, 2 * PTR) }
}
extern "system" fn root_release(this: *mut c_void) -> u32 {
    // SAFETY: as above.
    unsafe { p_release(this, 2 * PTR) }
}

extern "system" fn provider_options(_: *mut c_void, out: *mut i32) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's out pointer.
    unsafe { *out = SERVER_SIDE_PROVIDER };
    S_OK
}

/// The Value pattern for an input; no other patterns yet (ARCHITECTURE
/// "UI components": limits).
extern "system" fn pattern_provider(this: *mut c_void, id: i32, out: *mut *mut c_void) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: COM calls this on our IRawElementProviderSimple.
    let p = unsafe { Provider::of(this, 0) };
    let to = if id == UIA_VALUE_PATTERN && p.is_input() {
        p.refs.fetch_add(1, Ordering::Relaxed);
        Provider::as_value((p as *const Provider).cast_mut())
    } else {
        core::ptr::null_mut()
    };
    // SAFETY: the caller's out pointer; the reference is its.
    unsafe { *out = to };
    S_OK
}

extern "system" fn value_query(
    this: *mut c_void,
    iid: *const Guid,
    out: *mut *mut c_void,
) -> HResult {
    // SAFETY: COM calls this on our IValueProvider.
    unsafe { p_query(this, 3 * PTR, iid, out) }
}
extern "system" fn value_add_ref(this: *mut c_void) -> u32 {
    // SAFETY: as above.
    unsafe { p_add_ref(this, 3 * PTR) }
}
extern "system" fn value_release(this: *mut c_void) -> u32 {
    // SAFETY: as above.
    unsafe { p_release(this, 3 * PTR) }
}

/// IValueProvider::get_Value: the input's text.
extern "system" fn get_value(this: *mut c_void, out: *mut *mut u16) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: COM calls this on our IValueProvider.
    let p = unsafe { Provider::of(this, 3 * PTR) };
    let r = p.shared.with(|s| p.index(s).map(|i| i.map(|i| s.tree[i].text.clone())));
    match r {
        Err(hr) => hr,
        Ok(t) => {
            // SAFETY: the caller's out pointer; the BSTR is its.
            unsafe { *out = bstr(t.as_deref().unwrap_or("")) as *mut u16 };
            S_OK
        }
    }
}

/// IValueProvider::get_IsReadOnly: an input is not.
extern "system" fn get_is_read_only(_: *mut c_void, out: *mut i32) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's out pointer.
    unsafe { *out = 0 };
    S_OK
}

/// What IValueProvider::SetValue hands the window's thread
/// (`set_value_message`, sent: the window's own update path, as typing).
struct SetValue {
    id: String,
    text: String,
}

/// The message SetValue sends the window (registered, so no program's
/// own WM_USER message collides with it).
fn set_value_message() -> u32 {
    static M: AtomicU32 = AtomicU32::new(0);
    let m = M.load(Ordering::Relaxed);
    if m != 0 {
        return m;
    }
    let name: Vec<u16> = "GalaUiSetValue\0".encode_utf16().collect();
    // SAFETY: a NUL-terminated name.
    let m = unsafe { RegisterWindowMessageW(name.as_ptr()) };
    M.store(m, Ordering::Relaxed);
    m
}

/// IValueProvider::SetValue: the input's text becomes `text`, as if typed
/// (its program's message for the new text).
extern "system" fn set_value(this: *mut c_void, text: *const u16) -> HResult {
    if text.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: COM calls this on our IValueProvider.
    let p = unsafe { Provider::of(this, 3 * PTR) };
    let mut n = 0;
    // SAFETY: the caller's NUL-terminated string.
    while unsafe { *text.add(n) } != 0 {
        n += 1;
    }
    // SAFETY: as above, `n` units.
    let t = String::from_utf16_lossy(unsafe { core::slice::from_raw_parts(text, n) });
    let Some(id) = p.id.clone() else { return UIA_E_ELEMENTNOTENABLED };
    let hwnd = match p.shared.with(|s| p.index(s).map(|_| s.hwnd)) {
        Ok(h) => h,
        Err(hr) => return hr,
    };
    let m = set_value_message();
    if m == 0 {
        return E_FAIL;
    }
    let v = SetValue { id, text: t };
    // SAFETY: our window; the request outlives the (synchronous) send.
    let r = unsafe { SendMessageW(hwnd, m, 0, (&v as *const SetValue) as isize) };
    if r == 1 { S_OK } else { UIA_E_ELEMENTNOTENABLED }
}

fn bstr(s: &str) -> u64 {
    let w: Vec<u16> = s.encode_utf16().collect();
    // SAFETY: the text and its length; the caller frees the BSTR.
    unsafe { SysAllocStringLen(w.as_ptr(), w.len() as u32) as u64 }
}

extern "system" fn property_value(this: *mut c_void, property: i32, out: *mut Variant) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's out pointer: VT_EMPTY unless set below.
    let out = unsafe {
        out.write(Variant { vt: 0, reserved: [0; 3], value: 0, extra: 0 });
        &mut *out
    };
    // SAFETY: COM calls this on our IRawElementProviderSimple.
    let p = unsafe { Provider::of(this, 0) };
    let r = p.shared.with(|s| {
        // The window's element: the host provider's properties.
        let Some(i) = p.index(s)? else { return Ok(None) };
        let e = &s.tree[i];
        let input = e.kind == crate::Kind::Input;
        Ok(match property {
            UIA_CONTROL_TYPE => Some((VT_I4, uia::control_type(e.kind) as u32 as u64, None)),
            UIA_NAME => Some((VT_BSTR, 0, Some(String::from(uia::name(e))))),
            UIA_AUTOMATION_ID => Some((VT_BSTR, 0, e.id.clone())),
            UIA_IS_KEYBOARD_FOCUSABLE => Some((VT_BOOL, if input { 0xffff } else { 0 }, None)),
            UIA_HAS_KEYBOARD_FOCUS => {
                let has = input && s.focus.is_some() && s.focus == e.id;
                Some((VT_BOOL, if has { 0xffff } else { 0 }, None))
            }
            _ => None,
        })
    });
    match r {
        Err(hr) => hr,
        Ok(None) => S_OK,
        Ok(Some((vt, v, text))) => {
            out.vt = vt;
            out.value = match text {
                Some(t) => bstr(&t),
                None => v,
            };
            S_OK
        }
    }
}

extern "system" fn host_provider(this: *mut c_void, out: *mut *mut c_void) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: COM calls this on our IRawElementProviderSimple.
    let p = unsafe { Provider::of(this, 0) };
    // SAFETY: the caller's out pointer.
    unsafe { *out = core::ptr::null_mut() };
    if p.id.is_some() {
        return S_OK;
    }
    let hwnd = p.shared.with(|s| s.hwnd);
    // SAFETY: our window; the caller's out pointer.
    unsafe { UiaHostProviderFromHwnd(hwnd, out) }
}

/// NavigateDirection: Parent 0, NextSibling 1, PreviousSibling 2,
/// FirstChild 3, LastChild 4.
extern "system" fn navigate(this: *mut c_void, dir: i32, out: *mut *mut c_void) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's out pointer.
    unsafe { *out = core::ptr::null_mut() };
    // SAFETY: COM calls this on our IRawElementProviderFragment.
    let p = unsafe { Provider::of(this, PTR) };
    let r = p.shared.with(|s| {
        let at = p.index(s)?;
        let to: Option<Option<usize>> = match (dir, at) {
            (0, Some(i)) => Some(uia::parent(&s.tree, i)),
            (1, Some(i)) => uia::sibling(&s.tree, i, true).map(Some),
            (2, Some(i)) => uia::sibling(&s.tree, i, false).map(Some),
            (3, _) => uia::children(&s.tree, at).next().map(Some),
            (4, _) => uia::children(&s.tree, at).last().map(Some),
            _ => None,
        };
        Ok(to.map(|t| p.fragment_for(s, t)))
    });
    match r {
        Err(hr) => hr,
        Ok(f) => {
            // SAFETY: the caller's out pointer; the new provider's reference is its.
            unsafe { *out = f.unwrap_or(core::ptr::null_mut()) };
            S_OK
        }
    }
}

extern "system" fn runtime_id(this: *mut c_void, out: *mut *mut c_void) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's out pointer.
    unsafe { *out = core::ptr::null_mut() };
    // SAFETY: COM calls this on our IRawElementProviderFragment.
    let p = unsafe { Provider::of(this, PTR) };
    if let Err(hr) = p.shared.with(|s| p.index(s)) {
        return hr;
    }
    // The window's comes from its host provider.
    let Some(id) = &p.id else { return S_OK };
    // SAFETY: a two-element VT_I4 vector, filled and handed to the caller.
    unsafe {
        let sa = SafeArrayCreateVector(VT_I4, 0, 2);
        if sa.is_null() {
            return 0x8007_000e_u32 as i32; // E_OUTOFMEMORY
        }
        let mut data = core::ptr::null_mut();
        SafeArrayAccessData(sa, &mut data);
        let v = data.cast::<i32>();
        *v = APPEND_RUNTIME_ID;
        *v.add(1) = uia::runtime_id(id);
        SafeArrayUnaccessData(sa);
        *out = sa;
    }
    S_OK
}

extern "system" fn bounding_rectangle(this: *mut c_void, out: *mut UiaRect) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: COM calls this on our IRawElementProviderFragment.
    let p = unsafe { Provider::of(this, PTR) };
    let r = p.shared.with(|s| Ok::<_, HResult>((p.index(s)?.map(|i| s.tree[i].rect), s.hwnd)));
    let (at, hwnd) = match r {
        Ok(v) => v,
        Err(hr) => return hr,
    };
    // The window's comes from its host provider: empty here.
    let [x, y, w, h] = at.unwrap_or([0; 4]);
    let mut o = Point::default();
    if at.is_some() {
        // SAFETY: our window.
        unsafe { ClientToScreen(hwnd, &mut o) };
    }
    // SAFETY: the caller's out pointer.
    unsafe {
        out.write(UiaRect {
            left: f64::from(x + o.x),
            top: f64::from(y + o.y),
            width: f64::from(w),
            height: f64::from(h),
        })
    };
    S_OK
}

extern "system" fn embedded_roots(_: *mut c_void, out: *mut *mut c_void) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's out pointer.
    unsafe { *out = core::ptr::null_mut() };
    S_OK
}

/// An input takes the keyboard focus; other elements cannot have it.
extern "system" fn set_focus(this: *mut c_void) -> HResult {
    // SAFETY: COM calls this on our IRawElementProviderFragment.
    let p = unsafe { Provider::of(this, PTR) };
    let r = p.shared.with(|s| {
        let i = p.index(s)?;
        if let Some(i) = i
            && s.tree[i].kind == crate::Kind::Input
        {
            s.focus = s.tree[i].id.clone();
        }
        Ok::<_, HResult>(s.hwnd)
    });
    match r {
        Err(hr) => hr,
        Ok(hwnd) => {
            // SAFETY: our window: drawn again with the focus.
            unsafe { InvalidateRect(hwnd, core::ptr::null(), 0) };
            S_OK
        }
    }
}

extern "system" fn fragment_root(this: *mut c_void, out: *mut *mut c_void) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: COM calls this on our IRawElementProviderFragment.
    let p = unsafe { Provider::of(this, PTR) };
    let root = Provider::new(p.shared.clone(), None);
    // SAFETY: the caller's out pointer; the new provider's reference is its.
    unsafe { *out = Provider::as_root(root) };
    S_OK
}

extern "system" fn from_point(this: *mut c_void, x: f64, y: f64, out: *mut *mut c_void) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's out pointer.
    unsafe { *out = core::ptr::null_mut() };
    // SAFETY: COM calls this on our IRawElementProviderFragmentRoot.
    let p = unsafe { Provider::of(this, 2 * PTR) };
    let hwnd = p.shared.with(|s| s.hwnd);
    let mut at = Point { x: x as i32, y: y as i32 };
    // SAFETY: our window.
    unsafe { ScreenToClient(hwnd, &mut at) };
    let f = p.shared.with(|s| {
        if !s.alive {
            return Err(UIA_E_ELEMENTNOTAVAILABLE);
        }
        let i = hit(&s.tree, at.x, at.y)
            .and_then(|e| e.id.as_deref())
            .and_then(|id| uia::find(&s.tree, id));
        Ok(i.map(|i| p.fragment_for(s, Some(i))))
    });
    match f {
        Err(hr) => hr,
        Ok(f) => {
            // SAFETY: the caller's out pointer; None: the root itself.
            unsafe { *out = f.unwrap_or(core::ptr::null_mut()) };
            S_OK
        }
    }
}

extern "system" fn get_focus(this: *mut c_void, out: *mut *mut c_void) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: COM calls this on our IRawElementProviderFragmentRoot.
    let p = unsafe { Provider::of(this, 2 * PTR) };
    let f = p.shared.with(|s| {
        let i = s.focus.as_deref().and_then(|f| uia::find(&s.tree, f));
        i.map(|i| p.fragment_for(s, Some(i)))
    });
    // SAFETY: the caller's out pointer.
    unsafe { *out = f.unwrap_or(core::ptr::null_mut()) };
    S_OK
}

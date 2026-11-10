//! Windows, classes and message queues (the window manager's first
//! milestone). Top-level windows are the desktop's (the kernel draws their
//! frame and composites them: GalaWindow* services); their client area is
//! a pixel buffer here, which gdi32 draws into (GalaCreateSurfaceDC) and
//! EndPaint/ReleaseDC present. One message queue holds every thread's
//! posted messages (each tagged with its thread); GetMessage takes the
//! calling thread's, else makes WM_PAINT for one of its invalid windows,
//! else waits on the queue's event. SendMessage calls the window
//! procedure directly. Metrics: a 1-pixel border and a 30-pixel caption
//! (kernel/src/nt/desktop.rs).

use core::cell::UnsafeCell;
use core::ffi::c_void;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};

pub(crate) type Handle = *mut c_void;
pub(crate) type Hwnd = usize;
pub type WndProc = extern "system" fn(Hwnd, u32, usize, isize) -> isize;

#[cfg_attr(
    target_arch = "x86",
    link(name = "ntdll", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "ntdll", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GalaWindowCreate(
        x: i32,
        y: i32,
        w: u32,
        h: u32,
        title: *const u16,
        len: usize,
        id: *mut u32,
        style: u32,
    ) -> u32;
    fn GalaWindowShow(id: u32, show: u8) -> u32;
    fn GalaWindowDrag(id: u32) -> u32;
    fn GalaWindowResize(id: u32, edges: u32) -> u32;
    fn GalaWindowPresent(
        id: u32,
        x: i32,
        y: i32,
        w: u32,
        h: u32,
        pixels: *const u32,
        stride: usize,
    ) -> u32;
    fn GalaWindowDestroy(id: u32) -> u32;
    fn GalaWindowListen(event: Handle) -> u32;
    fn GalaWindowInput(records: *mut u8, max: u32, count: *mut u32) -> u32;
    pub(crate) fn GalaWindowCapture(id: u32) -> u32;
    fn GalaWindowSetIcon(id: u32, pixels: *const u32, size: usize) -> u32;
    fn GalaRecordMissing(name: *const u8, length: usize, what: u32, detail: u64) -> u32;
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GetCurrentThreadId() -> u32;
    fn CreateEventW(sa: *const c_void, manual: i32, initial: i32, name: *const u16) -> Handle;
    fn SetEvent(h: Handle) -> i32;
    fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
    fn WaitForMultipleObjects(count: u32, handles: *const Handle, all: i32, ms: u32) -> u32;
    fn LoadLibraryW(name: *const u16) -> Handle;
    fn GetProcAddress(module: Handle, name: *const u8) -> *const c_void;
    fn GetTickCount() -> u32;
    fn SetLastError(code: u32);
}

pub const BORDER: i32 = 1;
pub const CAPTION: i32 = 32;
/// A menu bar's height (SM_CYMENU).
pub const MENU_BAR: i32 = 20;
const MAX_CLASSES: usize = 32;
pub(crate) const MAX_WINDOWS: usize = 512;
const QUEUE: usize = 256;

const WM_NCCREATE: u32 = 0x0081;
const WM_NCDESTROY: u32 = 0x0082;
const WM_CREATE: u32 = 0x0001;
const WM_DESTROY: u32 = 0x0002;
const WM_PAINT: u32 = 0x000f;
const WM_CLOSE: u32 = 0x0010;
const WM_QUIT: u32 = 0x0012;
const WM_ERASEBKGND: u32 = 0x0014;
const WM_GETICON: u32 = 0x007f;
/// The desktop's request to move a window (Snap; kernel desktop
/// GALA_WM_PLACE): wParam MAKELONG(left, top), lParam MAKELONG(width,
/// height), as WM_MOVE and WM_SIZE (signed positions, so they fit a
/// 32-bit program's WPARAM/LPARAM too).
const GALA_WM_PLACE: u32 = 0x0001_0001;
/// The desktop minimized (wParam 1) or restored (0) the window itself
/// (its caption button, its taskbar button, Win+Down): user32 keeps
/// IsIconic.
const GALA_WM_MINIMIZED: u32 = 0x0001_0002;
/// A click activates the top-level window (WA_CLICKACTIVE) on its own
/// thread, before the click itself is dispatched.
const GALA_WM_ACTIVATE: u32 = 0x0001_0003;
/// A left-button press in a window that draws its own frame: its
/// WM_NCHITTEST decides, on its thread, whether it is a caption drag
/// (HTCAPTION) or the click it was (wParam, lParam: the click's).
const GALA_WM_NCDOWN: u32 = 0x0001_0004;
const WM_NCHITTEST: u32 = 0x0084;
const HTCLIENT: isize = 1;
const HTCAPTION: isize = 2;
const WM_ACTIVATE: u32 = 0x0006;
const WM_NCACTIVATE: u32 = 0x0086;
const WA_INACTIVE: usize = 0;
const WA_ACTIVE: usize = 1;
const WA_CLICKACTIVE: usize = 2;
const WM_SETICON: u32 = 0x0080;
const WS_VISIBLE: u32 = 0x1000_0000;
const WS_CHILD: u32 = 0x4000_0000;
const CW_USEDEFAULT: i32 = 0x8000_0000u32 as i32;
const ERROR_INVALID_WINDOW_HANDLE: u32 = 1400;
const ERROR_CLASS_DOES_NOT_EXIST: u32 = 1411;
const ERROR_CLASS_ALREADY_EXISTS: u32 = 1410;

#[derive(Clone, Copy)]
pub(crate) struct Class {
    used: bool,
    atom: u16,
    name: [u16; 64],
    /// The name as registered (GetClassName).
    pub(crate) spelled: [u16; 64],
    pub(crate) proc_: Option<WndProc>,
    background: usize,
    style: u32,
    /// The class's icons (WNDCLASSEX hIcon, hIconSm).
    pub(crate) icon: usize,
    pub(crate) icon_small: usize,
    /// Its windows' extra bytes (cbWndExtra; extra.rs).
    pub(crate) window_extra: u32,
}

#[derive(Clone, Copy)]
pub(crate) struct Window {
    pub(crate) used: bool,
    pub(crate) class: usize,
    pub(crate) proc_: Option<WndProc>,
    /// The desktop's id (0: no screen; nothing is shown).
    pub(crate) desktop: u32,
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) w: i32,
    pub(crate) h: i32,
    pub(crate) style: u32,
    pub(crate) visible: bool,
    pub(crate) invalid: bool,
    pub(crate) erase: bool,
    pub(crate) thread: u32,
    pub(crate) pixels: *mut u32,
    /// The section the buffer is in (`surface`; top-level windows).
    pub(crate) section: Handle,
    pub(crate) cw: i32,
    pub(crate) ch: i32,
    pub(crate) user_data: isize,
    /// The title (NUL-terminated, truncated to 127 characters).
    pub(crate) title: [u16; 128],
    /// A child window's parent (0: a top-level window). A child's x and y
    /// are relative to the parent's client area and it has no frame (its
    /// client area is the whole window).
    pub(crate) parent: Hwnd,
    /// A child's id (CreateWindowEx's hMenu).
    pub(crate) id: usize,
    pub(crate) ex_style: u32,
    /// Disabled (EnableWindow FALSE, WS_DISABLED). Kept negative so an
    /// unused slot is all zeros (the table goes in .bss, not the DLL).
    pub(crate) disabled: bool,
    /// Z order among siblings: smaller is higher (children are added at
    /// the bottom, top-level windows at the top).
    pub(crate) z: i64,
    pub(crate) instance: usize,
    /// Maximized (SC_MAXIMIZE) and the rectangle SC_RESTORE goes back to.
    pub(crate) zoomed: bool,
    pub(crate) restore: [i32; 4],
    /// Minimized (SC_MINIMIZE, ShowWindow SW_MINIMIZE, or the desktop's
    /// minimize button): IsIconic.
    pub(crate) iconic: bool,
    /// A frameless desktop popup (a top-level WS_POPUP window without
    /// WS_CAPTION: menus, drop-down lists, tooltips): all client area.
    pub(crate) popup: bool,
    /// The menu bar's height (0: no menu). A top-level window's buffer
    /// holds the bar's rows above the client area's; `pixels` points at
    /// the client area's first row.
    pub(crate) bar: i32,
    /// Its icons (WM_SETICON ICON_BIG, ICON_SMALL; 0: none, the class's).
    pub(crate) icon: usize,
    pub(crate) icon_small: usize,
    /// A top-level window's part (client coordinates) drawn since it was
    /// last shown on the desktop (empty: right <= left), and since when
    /// (GetTickCount): see `present`.
    pub(crate) dirty: [i32; 4],
    pub(crate) dirty_at: u32,
    /// A top-level window's owner (0: none): the top-level window of
    /// CreateWindowEx's hWndParent for a window without WS_CHILD
    /// (GetWindow GW_OWNER).
    pub(crate) owner: Hwnd,
    /// A frame the window's procedure chose (WM_NCCALCSIZE): the left,
    /// right and bottom border and the height above the client area.
    /// The desktop then shows a frameless window over the client area
    /// and the program draws its own frame (Chromium); None: gala's frame.
    pub(crate) custom: Option<(i32, i32)>,
    /// SetLayeredWindowAttributes' colour key, alpha and LWA_* flags (a
    /// WS_EX_LAYERED window that was given them).
    pub(crate) layered: Option<(u32, u8, u32)>,
    /// The owned popup activated last (GetLastActivePopup).
    pub(crate) last_popup: Hwnd,
    /// Hidden by ShowOwnedPopups(owner, FALSE), shown again by TRUE.
    pub(crate) hidden_with_owner: bool,
    /// The class's extra bytes for this window (extra.rs; null for none).
    pub(crate) extra: *mut u8,
    pub(crate) extra_len: u32,
}

/// WM_NCCALCSIZE.
pub(crate) const WM_NCCALCSIZE: u32 = 0x0083;

/// NCCALCSIZE_PARAMS: three rectangles, then the WINDOWPOS.
#[repr(C)]
struct NcCalcSizeParams {
    rects: [[i32; 4]; 3],
    pos: *mut c_void,
}

/// Asks top-level window `hwnd`'s procedure for its client area within
/// its rectangle (WM_NCCALCSIZE; wParam TRUE when the frame changed:
/// SetWindowPos's SWP_FRAMECHANGED) and takes its frame: gala's
/// (DefWindowProc's answer) or the program's own (`custom`).
pub(crate) fn calc_frame(hwnd: Hwnd, changed: bool) {
    let Some(i) = index_of(hwnd) else { return };
    let w = with(|s| s.windows[i]);
    if !w.used || w.parent != 0 || (w.popup && w.custom.is_none()) {
        return;
    }
    let rect = [w.x, w.y, w.x + w.w, w.y + w.h];
    let mut params = NcCalcSizeParams { rects: [rect; 3], pos: core::ptr::null_mut() };
    SendMessageW(hwnd, WM_NCCALCSIZE, usize::from(changed), (&raw mut params) as isize);
    let [l, t, r, b] = params.rects[0];
    let insets = (l - w.x, t - w.y, w.x + w.w - r, w.y + w.h - b);
    let custom = if insets == (BORDER, BORDER + CAPTION + w.bar, BORDER, BORDER) {
        None
    } else {
        if insets.0 != insets.2 || insets.0 != insets.3 || w.bar != 0 {
            // gala's frames have equal sides and bottom; a menu bar is
            // gala's to draw.
            unsupported(b"user32.dll!WM_NCCALCSIZE(uneven frame or menu bar)", 0);
        }
        Some((insets.0.clamp(0, w.w / 2), insets.1.clamp(0, w.h)))
    };
    if custom != w.custom {
        set_frame(i, custom);
    }
}

/// Where top-level window `w` at (x, y), w x h is on the desktop: its
/// rectangle, or its client area's for a frame of its own.
pub(crate) fn desktop_rect(w: &Window, x: i32, y: i32, wd: i32, ht: i32) -> (i32, i32, i32, i32) {
    match w.custom {
        Some((b, t)) => (x + b, y + t, (wd - 2 * b).max(1), (ht - t - b).max(1)),
        None => (x, y, wd, ht),
    }
}

/// Gives window `i` a frame (`calc_frame`): a new client buffer and a
/// desktop window of the right kind (frameless for the program's own
/// frame), shown again if it was.
fn set_frame(i: usize, custom: Option<(i32, i32)>) {
    with(|s| s.windows[i].custom = custom);
    let w = with(|s| s.windows[i]);
    let (b, top) = frame(&w);
    crate::tree::resize_client(i, (w.w - 2 * b).max(1), (w.h - top - b).max(1));
    let w = with(|s| s.windows[i]);
    let len = w.title.iter().position(|&c| c == 0).unwrap_or(0);
    let (x, y, wd, ht) = desktop_rect(&w, w.x, w.y, w.w, w.h);
    let mut id = 0;
    // SAFETY: plain calls on our desktop window and title.
    unsafe {
        if w.desktop != 0 {
            GalaWindowDestroy(w.desktop);
        }
        // The desktop's styles: 1 a popup, 2 an application window that
        // draws its own frame.
        let style = if w.popup {
            1
        } else if w.custom.is_some() {
            2
        } else {
            0
        };
        let _ = GalaWindowCreate(x, y, wd as u32, ht as u32, w.title.as_ptr(), len, &mut id, style);
        if id != 0 && w.visible {
            GalaWindowShow(id, 1);
        }
    }
    with(|s| s.windows[i].desktop = id);
    let hwnd = hwnd_of(i);
    crate::foreign::sync(hwnd);
    show_icon(hwnd);
    InvalidateRect(hwnd, core::ptr::null(), 1);
}

/// A top-level window's frame: the left, right and bottom border, and the
/// height above the client area (the caption and the menu bar).
pub(crate) fn frame(w: &Window) -> (i32, i32) {
    let (b, top) = desktop_frame(w);
    (b, top + w.bar)
}

/// The frame the desktop draws (the menu bar is in the desktop's client
/// area, drawn by user32).
pub(crate) fn desktop_frame(w: &Window) -> (i32, i32) {
    match w.custom {
        Some(f) => f,
        None if w.popup => (0, 0),
        None => (BORDER, BORDER + CAPTION),
    }
}

/// The start of a top-level window's buffer (the menu bar's first row).
pub(crate) fn buffer_base(w: &Window) -> *mut u32 {
    if w.pixels.is_null() {
        return w.pixels;
    }
    // SAFETY: the buffer holds `bar` rows of `cw` before the client area.
    unsafe { w.pixels.sub((w.bar * w.cw) as usize) }
}

/// Whether a top-level window of `style` is a frameless popup.
pub(crate) fn is_popup(style: u32) -> bool {
    const WS_POPUP: u32 = 0x8000_0000;
    const WS_CAPTION: u32 = 0x00c0_0000;
    style & WS_POPUP != 0 && style & WS_CAPTION != WS_CAPTION
}

#[derive(Clone, Copy)]
#[repr(C)]
pub struct Msg {
    pub hwnd: Hwnd,
    pub message: u32,
    pub wparam: usize,
    pub lparam: isize,
    pub time: u32,
    pub pt: [i32; 2],
    pub private: u32,
}

/// A posted message waiting in the queue. `seq` orders the queue: the
/// slots are reused in any order, and messages come out oldest first.
#[derive(Clone, Copy)]
struct Queued {
    used: bool,
    thread: u32,
    seq: u64,
    /// Input from the desktop (`pump`), which comes out only when no
    /// posted message waits, as from Windows' input queue.
    input: bool,
    msg: Msg,
}

const NO_CLASS: Class = Class {
    used: false,
    atom: 0,
    name: [0; 64],
    spelled: [0; 64],
    proc_: None,
    background: 0,
    style: 0,
    icon: 0,
    icon_small: 0,
    window_extra: 0,
};
pub(crate) const NO_WINDOW: Window = Window {
    used: false,
    class: 0,
    proc_: None,
    desktop: 0,
    x: 0,
    y: 0,
    w: 0,
    h: 0,
    style: 0,
    visible: false,
    invalid: false,
    erase: false,
    thread: 0,
    pixels: core::ptr::null_mut(),
    section: core::ptr::null_mut(),
    cw: 0,
    ch: 0,
    user_data: 0,
    title: [0; 128],
    parent: 0,
    id: 0,
    ex_style: 0,
    disabled: false,
    z: 0,
    instance: 0,
    zoomed: false,
    restore: [0; 4],
    iconic: false,
    popup: false,
    bar: 0,
    icon: 0,
    icon_small: 0,
    dirty: [0; 4],
    dirty_at: 0,
    owner: 0,
    custom: None,
    layered: None,
    last_popup: 0,
    hidden_with_owner: false,
    extra: core::ptr::null_mut(),
    extra_len: 0,
};
const NO_MSG: Msg =
    Msg { hwnd: 0, message: 0, wparam: 0, lparam: 0, time: 0, pt: [0; 2], private: 0 };

pub(crate) struct State {
    pub(crate) classes: [Class; MAX_CLASSES],
    pub(crate) windows: [Window; MAX_WINDOWS],
    queue: [Queued; QUEUE],
    /// Messages posted so far (the next one's `seq`).
    posted: u64,
    pub(crate) next_atom: u16,
}

struct Shared {
    lock: AtomicBool,
    state: UnsafeCell<State>,
}

// SAFETY: every access is under `lock`.
unsafe impl Sync for Shared {}

static SHARED: Shared = Shared {
    lock: AtomicBool::new(false),
    state: UnsafeCell::new(State {
        classes: [NO_CLASS; MAX_CLASSES],
        windows: [NO_WINDOW; MAX_WINDOWS],
        queue: [Queued { used: false, thread: 0, seq: 0, input: false, msg: NO_MSG }; QUEUE],
        posted: 0,
        next_atom: 0,
    }),
};

/// The queue's event (auto-reset), made on first use.
static EVENT: AtomicUsize = AtomicUsize::new(0);

pub(crate) fn with<R>(f: impl FnOnce(&mut State) -> R) -> R {
    while SHARED.lock.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        core::hint::spin_loop();
    }
    // SAFETY: under the lock.
    let r = f(unsafe { &mut *SHARED.state.get() });
    SHARED.lock.store(false, Ordering::Release);
    r
}

fn event() -> Handle {
    let e = EVENT.load(Ordering::Acquire);
    if e != 0 {
        return e as Handle;
    }
    // SAFETY: a plain call.
    let new = unsafe { CreateEventW(core::ptr::null(), 0, 0, core::ptr::null()) } as usize;
    match EVENT.compare_exchange(0, new, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => {
            // The desktop signals it when input arrives for our windows
            // (STATUS_NOT_SUPPORTED without a screen: nothing comes).
            // SAFETY: our event.
            unsafe { GalaWindowListen(new as Handle) };
            new as Handle
        }
        Err(other) => other as Handle,
    }
}

/// The process's queue event exists and the kernel sets it (WinEvent
/// hooks need it before any window is made).
pub(crate) fn listen() {
    event();
}

pub(crate) fn wake() {
    // SAFETY: our event.
    unsafe { SetEvent(event()) };
}

pub(crate) fn set_last_error(code: u32) {
    // SAFETY: a plain call.
    unsafe { SetLastError(code) };
}

pub(crate) fn me() -> u32 {
    // SAFETY: a plain call.
    unsafe { GetCurrentThreadId() }
}

/// The window slot of `hwnd` (HWNDs come from the kernel's registry:
/// crate::foreign).
pub(crate) fn index_of(hwnd: Hwnd) -> Option<usize> {
    crate::foreign::index_of(hwnd)
}

pub(crate) fn hwnd_of(i: usize) -> Hwnd {
    crate::foreign::hwnd_of(i)
}

/// The window's state, if `hwnd` is one.
pub(crate) fn window(hwnd: Hwnd) -> Option<Window> {
    let i = index_of(hwnd)?;
    with(|s| s.windows[i].used.then_some(s.windows[i]))
}

pub(crate) fn unsupported(what: &[u8], detail: u64) {
    // SAFETY: our string.
    unsafe { GalaRecordMissing(what.as_ptr(), what.len(), 2, detail) };
}

// ---- gdi32, loaded when first needed ----

pub(crate) struct Gdi {
    create_dc: extern "system" fn(*mut u32, i32, i32, i32) -> Handle,
    delete_dc: extern "system" fn(Handle) -> i32,
    solid_brush: extern "system" fn(u32) -> Handle,
    select: extern "system" fn(Handle, Handle) -> Handle,
    pat_blt: extern "system" fn(Handle, i32, i32, i32, i32, u32) -> i32,
    create_child_dc: ChildDc,
    /// SetBoundsRect and GetBoundsRect (what a DC drew: `surface`).
    pub(crate) set_bounds: extern "system" fn(Handle, *const [i32; 4], u32) -> u32,
    pub(crate) get_bounds: extern "system" fn(Handle, *mut [i32; 4], u32) -> u32,
}

type ChildDc = extern "system" fn(*mut u32, i32, i32, i32, i32, i32, *const [i32; 4]) -> Handle;

static GDI: AtomicUsize = AtomicUsize::new(0);
static GDI_FNS: Shared2 = Shared2(UnsafeCell::new(None));

struct Shared2(UnsafeCell<Option<Gdi>>);
// SAFETY: written once before GDI is set (Release), read after (Acquire).
unsafe impl Sync for Shared2 {}

pub(crate) fn gdi() -> Option<&'static Gdi> {
    if GDI.load(Ordering::Acquire) == 2 {
        // SAFETY: set before GDI became 2.
        return unsafe { (*GDI_FNS.0.get()).as_ref() };
    }
    let name: [u16; 10] = {
        let mut n = [0u16; 10];
        for (d, b) in n.iter_mut().zip(b"gdi32.dll") {
            *d = u16::from(*b);
        }
        n
    };
    // SAFETY: a loaded DLL's exports, with the signatures gdi32 defines.
    unsafe {
        let m = LoadLibraryW(name.as_ptr());
        if m.is_null() {
            return None;
        }
        let f = |n: &[u8]| GetProcAddress(m, n.as_ptr());
        let (a, b, c, d, e) = (
            f(b"GalaCreateSurfaceDC\0"),
            f(b"GalaDeleteSurfaceDC\0"),
            f(b"CreateSolidBrush\0"),
            f(b"SelectObject\0"),
            f(b"PatBlt\0"),
        );
        let child = f(b"GalaCreateChildDC\0");
        let (set_bounds, get_bounds) = (f(b"SetBoundsRect\0"), f(b"GetBoundsRect\0"));
        if [a, b, c, d, e, child, set_bounds, get_bounds].iter().any(|p| p.is_null()) {
            return None;
        }
        if GDI.compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire).is_ok() {
            *GDI_FNS.0.get() = Some(Gdi {
                create_dc: core::mem::transmute::<
                    *const c_void,
                    extern "system" fn(*mut u32, i32, i32, i32) -> Handle,
                >(a),
                delete_dc: core::mem::transmute::<*const c_void, extern "system" fn(Handle) -> i32>(
                    b,
                ),
                solid_brush: core::mem::transmute::<*const c_void, extern "system" fn(u32) -> Handle>(
                    c,
                ),
                select: core::mem::transmute::<
                    *const c_void,
                    extern "system" fn(Handle, Handle) -> Handle,
                >(d),
                pat_blt: core::mem::transmute::<
                    *const c_void,
                    extern "system" fn(Handle, i32, i32, i32, i32, u32) -> i32,
                >(e),
                create_child_dc: core::mem::transmute::<*const c_void, ChildDc>(child),
                set_bounds: core::mem::transmute::<
                    *const c_void,
                    extern "system" fn(Handle, *const [i32; 4], u32) -> u32,
                >(set_bounds),
                get_bounds: core::mem::transmute::<
                    *const c_void,
                    extern "system" fn(Handle, *mut [i32; 4], u32) -> u32,
                >(get_bounds),
            });
            GDI.store(2, Ordering::Release);
        }
        while GDI.load(Ordering::Acquire) != 2 {
            core::hint::spin_loop();
        }
        (*GDI_FNS.0.get()).as_ref()
    }
}

// ---- Classes ----

#[repr(C)]
pub struct WndClassExW {
    size: u32,
    style: u32,
    proc_: Option<WndProc>,
    class_extra: i32,
    window_extra: i32,
    instance: Handle,
    icon: Handle,
    cursor: Handle,
    background: Handle,
    menu_name: *const u16,
    class_name: *const u16,
    icon_small: Handle,
}

impl WndClassExW {
    /// A zeroed WNDCLASSEXW with its cbSize, for GetClassInfoExW to fill.
    pub(crate) const EMPTY: WndClassExW = WndClassExW {
        size: size_of::<WndClassExW>() as u32,
        style: 0,
        proc_: None,
        class_extra: 0,
        window_extra: 0,
        instance: core::ptr::null_mut(),
        icon: core::ptr::null_mut(),
        cursor: core::ptr::null_mut(),
        background: core::ptr::null_mut(),
        menu_name: core::ptr::null(),
        class_name: core::ptr::null(),
        icon_small: core::ptr::null_mut(),
    };

    /// The WNDCLASSW of the same class (GetClassInfoW).
    pub(crate) fn without_size(&self) -> WndClassW {
        WndClassW {
            style: self.style,
            proc_: self.proc_,
            class_extra: self.class_extra,
            window_extra: self.window_extra,
            instance: self.instance,
            icon: self.icon,
            cursor: self.cursor,
            background: self.background,
            menu_name: self.menu_name,
            class_name: self.class_name,
        }
    }

    /// A system class (user32's controls): its name, its procedure, the
    /// arrow cursor's class style, no background (the control paints it).
    pub(crate) fn system(name: *const u16, proc_: WndProc) -> WndClassExW {
        WndClassExW {
            size: core::mem::size_of::<WndClassExW>() as u32,
            style: 0x8 | 0x4000, // CS_DBLCLKS | CS_GLOBALCLASS
            proc_: Some(proc_),
            class_extra: 0,
            window_extra: 0,
            instance: core::ptr::null_mut(),
            icon: core::ptr::null_mut(),
            cursor: core::ptr::null_mut(),
            background: core::ptr::null_mut(),
            menu_name: core::ptr::null(),
            class_name: name,
            icon_small: core::ptr::null_mut(),
        }
    }
}

/// The NUL-terminated name, case-folded, in a fixed buffer.
fn class_key(name: *const u16) -> [u16; 64] {
    let mut out = [0u16; 64];
    for (i, o) in out.iter_mut().take(63).enumerate() {
        // SAFETY: the caller's NUL-terminated string.
        let c = unsafe { *name.add(i) };
        if c == 0 {
            break;
        }
        *o = if (u16::from(b'a')..=u16::from(b'z')).contains(&c) { c - 32 } else { c };
    }
    out
}

/// A name as given (up to 63 characters).
fn spelled(name: *const u16) -> [u16; 64] {
    let mut out = [0u16; 64];
    for (i, o) in out.iter_mut().take(63).enumerate() {
        // SAFETY: the caller's NUL-terminated string.
        let c = unsafe { *name.add(i) };
        if c == 0 {
            break;
        }
        *o = c;
    }
    out
}

/// GetClassNameW(hwnd, buf, n): the window's class name, truncated to
/// n - 1 characters; the characters copied (without the NUL), 0 for a
/// window that is not one.
#[unsafe(no_mangle)]
pub extern "system" fn GetClassNameW(hwnd: Hwnd, buf: *mut u16, n: i32) -> i32 {
    let Some(w) = window(hwnd) else {
        if let Some(info) = crate::foreign::remote(hwnd) {
            return crate::foreign::copy_out(info.class(), buf, n);
        }
        set_last_error(ERROR_INVALID_WINDOW_HANDLE);
        return 0;
    };
    if buf.is_null() || n <= 0 {
        return 0;
    }
    let name = with(|s| s.classes[w.class].spelled);
    let len = name.iter().position(|&c| c == 0).unwrap_or(63).min(n as usize - 1);
    // SAFETY: the caller's buffer of `n` characters.
    unsafe {
        core::ptr::copy_nonoverlapping(name.as_ptr(), buf, len);
        *buf.add(len) = 0;
    }
    len as i32
}

#[unsafe(no_mangle)]
pub extern "system" fn GetClassNameA(hwnd: Hwnd, buf: *mut u8, n: i32) -> i32 {
    let mut w = [0u16; 64];
    let len = GetClassNameW(hwnd, w.as_mut_ptr(), 64);
    if buf.is_null() || n <= 0 || len == 0 {
        return 0;
    }
    let k = (len as usize).min(n as usize - 1);
    for (i, &c) in w.iter().take(k).enumerate() {
        // SAFETY: the caller's buffer of `n` bytes.
        unsafe { *buf.add(i) = if c < 0x80 { c as u8 } else { b'?' } };
    }
    // SAFETY: as above.
    unsafe { *buf.add(k) = 0 };
    k as i32
}

/// Whether `class` is registered; if not, the ledger gets
/// `user32.dll!CreateWindowExW(<class>)` (so a program's missing control
/// classes show by name) and the last error is
/// ERROR_CANNOT_FIND_WND_CLASS.
fn class_known(class: *const u16) -> bool {
    crate::controls::register_system_classes();
    if with(|s| find_class(s, class).is_some()) {
        return true;
    }
    let mut text = [0u8; 96];
    let prefix = b"user32.dll!CreateWindowExW(";
    text[..prefix.len()].copy_from_slice(prefix);
    let mut n = prefix.len();
    if (class as usize) < 0x1_0000 {
        for &b in b"atom" {
            text[n] = b;
            n += 1;
        }
    } else {
        let mut i = 0;
        // SAFETY: the caller's NUL-terminated class name.
        while n < text.len() - 1 && unsafe { *class.add(i) } != 0 {
            let c = unsafe { *class.add(i) };
            text[n] = if c < 0x80 { c as u8 } else { b'?' };
            n += 1;
            i += 1;
        }
    }
    text[n] = b')';
    // The atom as the detail (0 for a name: one line per class).
    let atom = if (class as usize) < 0x1_0000 { class as usize as u64 } else { 0 };
    unsupported(&text[..=n], atom);
    set_last_error(1407); // ERROR_CANNOT_FIND_WND_CLASS
    false
}

/// A class by name (or atom: a value under 0x10000).
pub(crate) fn find_class(s: &State, name: *const u16) -> Option<usize> {
    if (name as usize) < 0x1_0000 {
        let atom = name as usize as u16;
        return s.classes.iter().position(|c| c.used && c.atom == atom);
    }
    let key = class_key(name);
    s.classes.iter().position(|c| c.used && c.name == key)
}

/// UnregisterClassW(name, instance): the class (a name or an atom) goes;
/// ERROR_CLASS_HAS_WINDOWS while a window of it is there. gala's classes are
/// process-wide, so the instance is not compared.
#[unsafe(no_mangle)]
pub extern "system" fn UnregisterClassW(name: *const u16, _instance: usize) -> i32 {
    const ERROR_CLASS_HAS_WINDOWS: u32 = 1412;
    let r = with(|s| {
        let Some(i) = find_class(s, name) else { return Err(ERROR_CLASS_DOES_NOT_EXIST) };
        if s.windows.iter().any(|w| w.used && w.class == i) {
            return Err(ERROR_CLASS_HAS_WINDOWS);
        }
        s.classes[i].used = false;
        Ok(())
    });
    match r {
        Ok(()) => 1,
        Err(e) => {
            set_last_error(e);
            0
        }
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn RegisterClassExW(wc: *const WndClassExW) -> u16 {
    // SAFETY: the caller's WNDCLASSEXW.
    let wc = unsafe { &*wc };
    let r = with(|s| {
        if find_class(s, wc.class_name).is_some() {
            return Err(ERROR_CLASS_ALREADY_EXISTS);
        }
        let Some(i) = s.classes.iter().position(|c| !c.used) else { return Err(8) };
        // Class atoms start at 0xC000 (kept as an offset: zero-initialized).
        let atom = 0xc000 + s.next_atom;
        s.next_atom += 1;
        s.classes[i] = Class {
            used: true,
            atom,
            name: class_key(wc.class_name),
            spelled: spelled(wc.class_name),
            proc_: wc.proc_,
            background: wc.background as usize,
            style: wc.style,
            icon: wc.icon as usize,
            icon_small: wc.icon_small as usize,
            window_extra: crate::extra::class_extra(wc.window_extra),
        };
        Ok(atom)
    });
    match r {
        Ok(atom) => atom,
        Err(e) => {
            // SAFETY: a plain call.
            unsafe { SetLastError(e) };
            0
        }
    }
}

/// RegisterClassExA: RegisterClassExW with the class and menu names
/// widened (WNDCLASSEXA has WNDCLASSEXW's layout; an id stays an id).
/// As with gala's other A functions, the procedure gets the wide forms of
/// messages (gala's windows are all Unicode).
#[unsafe(no_mangle)]
pub extern "system" fn RegisterClassExA(wc: *const WndClassExW) -> u16 {
    if wc.is_null() {
        set_last_error(87); // ERROR_INVALID_PARAMETER
        return 0;
    }
    let (mut class, mut menu) = ([0u16; 256], [0u16; 256]);
    // SAFETY: the caller's WNDCLASSEXA, whose names are char pointers.
    let mut w = unsafe { wc.read() };
    w.class_name = ansi_into_ptr(w.class_name.cast(), &mut class);
    if !w.menu_name.is_null() {
        w.menu_name = ansi_into_ptr(w.menu_name.cast(), &mut menu);
    }
    RegisterClassExW(&w)
}

/// WNDCLASSW: WNDCLASSEXW without the size and the small icon.
#[repr(C)]
pub struct WndClassW {
    style: u32,
    proc_: Option<WndProc>,
    class_extra: i32,
    window_extra: i32,
    instance: Handle,
    icon: Handle,
    cursor: Handle,
    background: Handle,
    menu_name: *const u16,
    class_name: *const u16,
}

#[unsafe(no_mangle)]
pub extern "system" fn RegisterClassW(wc: *const WndClassW) -> u16 {
    // SAFETY: the caller's WNDCLASSW.
    let wc = unsafe { &*wc };
    let ex = WndClassExW {
        size: size_of::<WndClassExW>() as u32,
        style: wc.style,
        proc_: wc.proc_,
        class_extra: wc.class_extra,
        window_extra: wc.window_extra,
        instance: wc.instance,
        icon: wc.icon,
        cursor: wc.cursor,
        background: wc.background,
        menu_name: wc.menu_name,
        class_name: wc.class_name,
        icon_small: core::ptr::null_mut(),
    };
    RegisterClassExW(&ex)
}

/// RegisterClassA: RegisterClassW with the names widened (WNDCLASSA has
/// WNDCLASSW's layout), as RegisterClassExA.
#[unsafe(no_mangle)]
pub extern "system" fn RegisterClassA(wc: *const WndClassW) -> u16 {
    if wc.is_null() {
        set_last_error(87); // ERROR_INVALID_PARAMETER
        return 0;
    }
    // SAFETY: the caller's WNDCLASSA.
    let wc = unsafe { &*wc };
    let ex = WndClassExW {
        size: size_of::<WndClassExW>() as u32,
        style: wc.style,
        proc_: wc.proc_,
        class_extra: wc.class_extra,
        window_extra: wc.window_extra,
        instance: wc.instance,
        icon: wc.icon,
        cursor: wc.cursor,
        background: wc.background,
        menu_name: wc.menu_name,
        class_name: wc.class_name,
        icon_small: core::ptr::null_mut(),
    };
    RegisterClassExA(&ex)
}

// ---- Windows ----

#[repr(C)]
struct CreateStructW {
    create_params: *mut c_void,
    instance: Handle,
    menu: Handle,
    parent: Hwnd,
    cy: i32,
    cx: i32,
    y: i32,
    x: i32,
    style: i32,
    name: *const u16,
    class: *const u16,
    ex_style: u32,
}

/// comctl32's tooltip class.
const TOOLTIPS: [u16; 17] = {
    let name = b"tooltips_class32\0";
    let mut w = [0u16; 17];
    let mut i = 0;
    while i < 17 {
        w[i] = name[i] as u16;
        i += 1;
    }
    w
};

#[allow(clippy::too_many_arguments)]
#[unsafe(no_mangle)]
pub extern "system" fn CreateWindowExW(
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
) -> Hwnd {
    if !class_known(class) {
        return 0;
    }
    // A tooltip is always a popup (Windows' tooltip class makes itself
    // WS_POPUP when created: wx creates one with neither WS_POPUP nor
    // WS_CHILD); gala chooses the frame before WM_NCCREATE, so here.
    let style = if class as usize >= 0x1_0000 && class_key(class) == class_key(TOOLTIPS.as_ptr()) {
        (style | 0x8000_0000) & !(WS_CHILD | 0x00c0_0000)
    } else {
        style
    };
    if style & WS_CHILD != 0 {
        return crate::tree::create_child(
            ex_style,
            class,
            name,
            style,
            x,
            y,
            w,
            h,
            parent,
            menu as usize,
            instance,
            param,
        );
    }
    let (x, y) = if x == CW_USEDEFAULT { (60, 60) } else { (x, y) };
    // A top-level window's hWndParent is its owner: the top-level window
    // of the one named (as Windows documents; HWND_MESSAGE and the
    // desktop give none).
    let owner = match parent {
        0 | crate::tree::DESKTOP => 0,
        p if p as isize == -3 => 0,
        p => crate::tree::GetAncestor(p, 2),
    };
    let popup = is_popup(style);
    // A top-level window's hMenu is its menu bar.
    let bar = if !popup && !menu.is_null() && crate::menu::IsMenu(menu as usize) != 0 {
        MENU_BAR
    } else {
        0
    };
    let (b, top) = if popup { (0, 0) } else { (BORDER, BORDER + CAPTION) };
    let (w, h) =
        if w == CW_USEDEFAULT { (640, 480) } else { (w.max(2 * b + 1), h.max(top + bar + b + 1)) };
    let (cw, ch) = (w - 2 * b, h - top - bar - b);
    // SAFETY: plain calls; the title is the caller's NUL-terminated string.
    let (pixels, section, desktop) = unsafe {
        let Some((base, section)) = crate::surface::alloc(cw, ch, bar) else {
            return 0;
        };
        let pixels = base.add((bar * cw) as usize);
        let mut len = 0;
        if !name.is_null() {
            while len < 256 && *name.add(len) != 0 {
                len += 1;
            }
        }
        let mut id = 0;
        // Without a screen the window exists but is never shown.
        let _ = GalaWindowCreate(x, y, w as u32, h as u32, name, len, &mut id, u32::from(popup));
        (pixels, section, id)
    };
    let mut title = [0u16; 128];
    if !name.is_null() {
        // SAFETY: the caller's NUL-terminated string.
        unsafe {
            let mut i = 0;
            while i < 127 && *name.add(i) != 0 {
                title[i] = *name.add(i);
                i += 1;
            }
        }
    }
    let created = with(|s| {
        let class = find_class(s, class)?;
        let i = s.windows.iter().position(|w| !w.used)?;
        s.windows[i] = Window {
            used: true,
            class,
            proc_: s.classes[class].proc_,
            desktop,
            x,
            y,
            w,
            h,
            style,
            visible: false,
            invalid: true,
            erase: true,
            thread: me(),
            pixels,
            section,
            cw,
            ch,
            user_data: 0,
            title,
            parent: 0,
            id: 0,
            ex_style,
            disabled: false,
            z: crate::tree::next_z(false),
            instance: instance as usize,
            zoomed: false,
            restore: [0; 4],
            iconic: false,
            popup,
            bar,
            icon: 0,
            icon_small: 0,
            dirty: [0; 4],
            dirty_at: 0,
            owner,
            custom: None,
            layered: None,
            last_popup: 0,
            hidden_with_owner: false,
            extra: crate::extra::alloc(s.classes[class].window_extra),
            extra_len: s.classes[class].window_extra,
        };
        if s.windows[i].extra.is_null() {
            s.windows[i].extra_len = 0;
        }
        Some(i)
    });
    let Some(i) = created else {
        // SAFETY: our buffer and window.
        unsafe {
            crate::surface::free(pixels.sub((bar * cw) as usize), section);
            if desktop != 0 {
                GalaWindowDestroy(desktop);
            }
            SetLastError(ERROR_CLASS_DOES_NOT_EXIST);
        }
        return 0;
    };
    crate::foreign::assign(i);
    crate::surface::register(hwnd_of(i), section, cw, ch, bar);
    if bar != 0 {
        crate::menu::attach(hwnd_of(i), menu as usize);
    }
    show_icon(hwnd_of(i));
    finish_create(i, ex_style, class, name, style, x, y, w, h, parent, menu, instance, param)
}

/// What CreateWindowEx does once the window exists: WH_CBT, WM_NCCREATE,
/// WM_CREATE, WS_VISIBLE; the window or 0.
#[allow(clippy::too_many_arguments)]
pub(crate) fn finish_create(
    i: usize,
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
) -> Hwnd {
    let hwnd = hwnd_of(i);
    let cs = CreateStructW {
        create_params: param,
        instance,
        menu,
        parent,
        cy: h,
        cx: w,
        y,
        x,
        style: style as i32,
        name,
        class,
        ex_style,
    };
    let lp = &cs as *const CreateStructW as isize;
    // WH_CBT first: HCBT_CREATEWND with a CBT_CREATEWND (the
    // CREATESTRUCT, the window it goes after); nonzero refuses the window,
    // which then goes without a message.
    if crate::hook::any(crate::hook::WH_CBT) {
        let cbt: [isize; 2] = [lp, 0];
        let refused = crate::hook::call(
            crate::hook::WH_CBT,
            crate::hook::HCBT_CREATEWND,
            hwnd,
            cbt.as_ptr() as isize,
        );
        if refused != 0 {
            let gone = with(|s| core::mem::replace(&mut s.windows[i], NO_WINDOW));
            crate::extra::free(gone.extra);
            // SAFETY: the window's buffer and desktop window.
            unsafe {
                if gone.desktop != 0 {
                    GalaWindowDestroy(gone.desktop);
                }
                if !gone.pixels.is_null() {
                    crate::surface::free(buffer_base(&gone), gone.section);
                }
            }
            return 0;
        }
    }
    if SendMessageW(hwnd, WM_NCCREATE, 0, lp) == 0 {
        DestroyWindow(hwnd);
        return 0;
    }
    // The frame, as on Windows between WM_NCCREATE and WM_CREATE.
    calc_frame(hwnd, false);
    if SendMessageW(hwnd, WM_CREATE, 0, lp) == -1 {
        DestroyWindow(hwnd);
        return 0;
    }
    use crate::winevent::{CHILDID_SELF, EVENT_OBJECT_CREATE, OBJID_WINDOW, raise};
    raise(EVENT_OBJECT_CREATE, hwnd, OBJID_WINDOW, CHILDID_SELF);
    if style & WS_VISIBLE != 0 {
        ShowWindow(hwnd, 5);
    }
    hwnd
}

/// DestroyWindow: WM_DESTROY, WM_NCDESTROY, then the window is gone.
#[unsafe(no_mangle)]
pub extern "system" fn DestroyWindow(hwnd: Hwnd) -> i32 {
    if window(hwnd).is_none() {
        // SAFETY: a plain call.
        unsafe { SetLastError(ERROR_INVALID_WINDOW_HANDLE) };
        return 0;
    }
    crate::hook::call(crate::hook::WH_CBT, crate::hook::HCBT_DESTROYWND, hwnd, 0);
    let was_shown = IsWindowVisible(hwnd) != 0;
    let parent = window(hwnd).map_or(0, |w| w.parent);
    use crate::winevent::{
        CHILDID_SELF, EVENT_OBJECT_DESTROY, EVENT_OBJECT_HIDE, OBJID_WINDOW, raise,
    };
    if was_shown {
        raise(EVENT_OBJECT_HIDE, hwnd, OBJID_WINDOW, CHILDID_SELF);
    }
    // WM_DESTROY to the window, then its children (and theirs).
    SendMessageW(hwnd, WM_DESTROY, 0, 0);
    for child in crate::tree::children_of(hwnd).iter().copied().filter(|&c| c != 0) {
        DestroyWindow(child);
    }
    SendMessageW(hwnd, WM_NCDESTROY, 0, 0);
    raise(EVENT_OBJECT_DESTROY, hwnd, OBJID_WINDOW, CHILDID_SELF);
    let Some(i) = index_of(hwnd) else { return 0 };
    free_slot(i);
    // Its area of the parent shows the parent again.
    if parent != 0 && was_shown {
        InvalidateRect(parent, core::ptr::null(), 1);
    }
    1
}

/// Window slot `i` is free: its HWND unregistered, its buffer, desktop
/// window, timers and posted messages gone (no messages sent).
fn free_slot(i: usize) {
    let hwnd = hwnd_of(i);
    crate::tree::forget(hwnd);
    crate::timer::window_destroyed(hwnd);
    crate::menu::window_destroyed(hwnd);
    crate::foreign::release(i);
    let gone = with(|s| {
        let w = s.windows[i];
        s.windows[i] = NO_WINDOW;
        // Its posted messages go too.
        for q in s.queue.iter_mut() {
            if q.used && q.msg.hwnd == hwnd {
                q.used = false;
            }
        }
        w
    });
    crate::extra::free(gone.extra);
    // SAFETY: the window's buffer and desktop window.
    unsafe {
        if gone.desktop != 0 {
            GalaWindowDestroy(gone.desktop);
        }
        if !gone.pixels.is_null() {
            crate::surface::free(buffer_base(&gone), gone.section);
        }
    }
}

/// The calling thread is ending (DLL_THREAD_DETACH): its windows are
/// destroyed with it, as on Windows (without messages: the thread is
/// going), and its queue's messages go.
pub(crate) fn thread_ended() {
    let thread = me();
    for _ in 0..MAX_WINDOWS {
        let next = with(|s| {
            (0..s.windows.len()).find(|&i| s.windows[i].used && s.windows[i].thread == thread)
        });
        let Some(i) = next else { break };
        free_slot(i);
    }
    with(|s| {
        for q in s.queue.iter_mut() {
            if q.used && q.thread == thread {
                q.used = false;
            }
        }
    });
    crate::foreign::thread_ended();
}

#[unsafe(no_mangle)]
pub extern "system" fn IsWindow(hwnd: Hwnd) -> i32 {
    i32::from(
        hwnd == crate::tree::DESKTOP
            || window(hwnd).is_some()
            || crate::foreign::remote(hwnd).is_some(),
    )
}

#[unsafe(no_mangle)]
pub extern "system" fn IsWindowVisible(hwnd: Hwnd) -> i32 {
    if let Some(info) = crate::foreign::remote(hwnd) {
        return i32::from(crate::foreign::remote_shown(&info));
    }
    i32::from(with(|s| {
        index_of(hwnd).is_some_and(|i| s.windows[i].used && crate::tree::shown(s, i))
    }))
}

/// ShowWindow: whether it was visible before.
#[unsafe(no_mangle)]
pub extern "system" fn ShowWindow(hwnd: Hwnd, cmd: i32) -> i32 {
    if hwnd != 0 && index_of(hwnd).is_none() && crate::foreign::remote(hwnd).is_some() {
        // Another process's window: it shows it (GALA_SHOWWINDOW).
        return SendMessageW(hwnd, crate::tree::GALA_SHOWWINDOW, cmd as u32 as usize, 0) as i32;
    }
    let Some(i) = index_of(hwnd) else { return 0 };
    let show = cmd != 0; // SW_HIDE
    if window(hwnd).is_some_and(|w| w.parent != 0) {
        return crate::tree::show_child(hwnd, show);
    }
    let (was, desktop, iconic) = with(|s| {
        let w = &mut s.windows[i];
        let was = w.visible;
        if w.used {
            w.visible = show;
            if show && !was {
                w.invalid = true;
                w.erase = true;
            }
        }
        (was, w.desktop, w.iconic)
    });
    crate::foreign::sync(hwnd);
    {
        use crate::winevent::{
            CHILDID_SELF, EVENT_OBJECT_HIDE, EVENT_OBJECT_SHOW, OBJID_WINDOW, raise,
        };
        if show != was {
            let event = if show { EVENT_OBJECT_SHOW } else { EVENT_OBJECT_HIDE };
            raise(event, hwnd, OBJID_WINDOW, CHILDID_SELF);
        }
    }
    if desktop != 0 {
        // SAFETY: our desktop window (2: shown minimized).
        unsafe {
            GalaWindowShow(
                desktop,
                if !show {
                    0
                } else if iconic {
                    2
                } else {
                    1
                },
            )
        };
        if show {
            crate::track::draw_bar(hwnd);
        }
    }
    if show {
        // A popup (a menu, a drop-down list) leaves the focus and the
        // foreground where they are.
        if !window(hwnd).is_some_and(|w| w.popup) {
            // SW_SHOWNOACTIVATE (4), SW_SHOWMINNOACTIVE (7) and SW_SHOWNA
            // (8) leave the active window as it is.
            if !matches!(cmd, 4 | 7 | 8) {
                set_foreground(hwnd);
            }
            crate::tree::shown_top_level(hwnd);
        }
        InvalidateRect(hwnd, core::ptr::null(), 1);
        wake();
        // SW_MINIMIZE, SW_SHOWMAXIMIZED, SW_RESTORE...
        crate::tree::show_state(hwnd, cmd);
    }
    i32::from(was)
}

/// UpdateWindow: sends WM_PAINT now if the window needs painting.
#[unsafe(no_mangle)]
pub extern "system" fn UpdateWindow(hwnd: Hwnd) -> i32 {
    if window(hwnd).is_none() {
        return 0;
    }
    // The window, then its children that need painting.
    let mut order = [0usize; 64];
    let n = crate::tree::subtree(hwnd, &mut order);
    for &h in &order[..n] {
        if window(h).is_some_and(|w| w.invalid) && IsWindowVisible(h) != 0 {
            SendMessageW(h, WM_PAINT, 0, 0);
        }
    }
    1
}

/// InvalidateRect(hwnd, rect, erase): the whole client area needs
/// painting (the rectangle is not tracked yet).
#[unsafe(no_mangle)]
pub extern "system" fn InvalidateRect(hwnd: Hwnd, _rect: *const [i32; 4], erase: i32) -> i32 {
    if window(hwnd).is_none() {
        return 0;
    }
    // Children paint over their parent's area: they are invalid with it.
    let mut all = [0usize; 64];
    let n = crate::tree::subtree(hwnd, &mut all);
    with(|s| {
        for &h in &all[..n] {
            if let Some(i) = index_of(h) {
                let w = &mut s.windows[i];
                w.invalid |= w.used;
                w.erase |= w.used && (erase != 0 || h != hwnd);
            }
        }
    });
    wake();
    1
}

#[unsafe(no_mangle)]
pub extern "system" fn ValidateRect(hwnd: Hwnd, _rect: *const [i32; 4]) -> i32 {
    let Some(i) = index_of(hwnd) else { return 0 };
    with(|s| {
        s.windows[i].invalid = false;
        i32::from(s.windows[i].used)
    })
}

/// RedrawWindow(hwnd, rect, region, flags): RDW_INVALIDATE (1) or
/// RDW_INTERNALPAINT (2) invalidate the window (with its children, as
/// InvalidateRect here; RDW_ERASE (4) asks for WM_ERASEBKGND),
/// RDW_VALIDATE (8) validates it, RDW_UPDATENOW (0x100) paints what is
/// invalid before returning. The rectangle and region are not kept:
/// the whole window is (in)validated. A NULL window is the desktop's,
/// which the compositor repaints itself.
#[unsafe(no_mangle)]
pub extern "system" fn RedrawWindow(
    hwnd: Hwnd,
    rect: *const [i32; 4],
    _region: Handle,
    flags: u32,
) -> i32 {
    if hwnd == 0 {
        return 1;
    }
    if window(hwnd).is_none() {
        return 0;
    }
    if flags & 3 != 0 {
        InvalidateRect(hwnd, rect, i32::from(flags & 4 != 0));
    }
    if flags & 8 != 0 {
        ValidateRect(hwnd, rect);
    }
    if flags & 0x100 != 0 {
        UpdateWindow(hwnd);
    }
    1
}

// ---- Painting ----

#[repr(C)]
pub struct PaintStruct {
    hdc: Handle,
    erase: i32,
    paint: [i32; 4],
    restore: i32,
    inc_update: i32,
    reserved: [u8; 32],
}

impl PaintStruct {
    /// For BeginPaint to fill.
    pub(crate) const EMPTY: PaintStruct = PaintStruct {
        hdc: core::ptr::null_mut(),
        erase: 0,
        paint: [0; 4],
        restore: 0,
        inc_update: 0,
        reserved: [0; 32],
    };
}

/// A DC on the window's client area (a child's: its part of the
/// top-level window's buffer), registered as handed out (`keep_dc`).
fn client_dc(hwnd: Hwnd) -> Handle {
    let Some(g) = gdi() else { return core::ptr::null_mut() };
    let Some((root, offset, visible)) = crate::tree::placement(hwnd) else {
        return core::ptr::null_mut();
    };
    let hdc = if offset == (0, 0) && window(hwnd).is_some_and(|w| w.parent == 0) {
        (g.create_dc)(root.pixels, root.cw, root.ch, root.cw)
    } else {
        (g.create_child_dc)(root.pixels, root.cw, root.ch, root.cw, offset.0, offset.1, &visible)
    };
    keep_dc(hdc, hwnd);
    hdc
}

// ---- Window DCs a program keeps ----
//
// What a window's DC draws goes to the window's buffer and shows at
// ReleaseDC or EndPaint (`present`). A program may keep its DC instead
// (CS_OWNDC; an OpenGL program, whose SwapBuffers has the ICD draw the
// frame into it): at the thread's idle moments (`flush`) a DC still out
// shows the whole window the first time and from then on what it drew
// (its bounds, which gdi32 accumulates once asked: short GetDC/ReleaseDC
// pairs never pay for that). The table also answers WindowFromDC.

/// DCs out at once; more are not tracked (in the ledger: their drawing
/// shows at ReleaseDC only, and WindowFromDC gives NULL for them).
const KEPT_DCS: usize = 64;

#[derive(Clone, Copy)]
struct KeptDc {
    hdc: usize,
    hwnd: Hwnd,
    /// Bounds accumulate (it was out at an idle moment).
    bounded: bool,
}

const NO_KEPT: KeptDc = KeptDc { hdc: 0, hwnd: 0, bounded: false };

struct KeptTable {
    lock: AtomicBool,
    dcs: UnsafeCell<[KeptDc; KEPT_DCS]>,
}

// SAFETY: `dcs` is only reached under `lock`.
unsafe impl Sync for KeptTable {}

static KEPT: KeptTable =
    KeptTable { lock: AtomicBool::new(false), dcs: UnsafeCell::new([NO_KEPT; KEPT_DCS]) };
/// How many entries are in use (`flush` skips the table when none).
static KEPT_COUNT: AtomicUsize = AtomicUsize::new(0);

fn with_kept<R>(f: impl FnOnce(&mut [KeptDc; KEPT_DCS]) -> R) -> R {
    while KEPT.lock.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        core::hint::spin_loop();
    }
    // SAFETY: the lock is held.
    let r = f(unsafe { &mut *KEPT.dcs.get() });
    KEPT.lock.store(false, Ordering::Release);
    r
}

fn keep_dc(hdc: Handle, hwnd: Hwnd) {
    if hdc.is_null() {
        return;
    }
    let kept = with_kept(|t| match t.iter_mut().find(|k| k.hdc == 0) {
        Some(k) => {
            *k = KeptDc { hdc: hdc as usize, hwnd, bounded: false };
            KEPT_COUNT.fetch_add(1, Ordering::Relaxed);
            true
        }
        None => false,
    });
    if !kept {
        unsupported(b"user32.dll!GetDC(more DCs out than tracked)", KEPT_DCS as u64);
    }
}

/// The DC is released (ReleaseDC, EndPaint).
fn forget_dc(hdc: Handle) {
    with_kept(|t| {
        if let Some(k) = t.iter_mut().find(|k| k.hdc == hdc as usize) {
            *k = NO_KEPT;
            KEPT_COUNT.fetch_sub(1, Ordering::Relaxed);
        }
    });
}

/// The window a DC handed out draws on (WindowFromDC), or 0.
pub(crate) fn dc_window(hdc: usize) -> Hwnd {
    if hdc == 0 || KEPT_COUNT.load(Ordering::Relaxed) == 0 {
        return 0;
    }
    with_kept(|t| t.iter().find(|k| k.hdc == hdc).map_or(0, |k| k.hwnd))
}

/// Marks what DCs still out drew as due for the desktop (`flush`).
fn present_kept() {
    if KEPT_COUNT.load(Ordering::Relaxed) == 0 {
        return;
    }
    let Some(g) = gdi() else { return };
    let kept = with_kept(|t| *t);
    for k in kept.iter().filter(|k| k.hdc != 0) {
        if window(k.hwnd).is_none() {
            // Its window is gone: nothing to show.
            continue;
        }
        let hdc = k.hdc as Handle;
        let drew = if k.bounded {
            let mut drawn = [0i32; 4];
            // DCB_RESET (1); DCB_SET (3) when something was drawn.
            (g.get_bounds)(hdc, &mut drawn, 1) & 3 == 3
        } else {
            // DCB_ENABLE | DCB_RESET: accumulate from now on; what it drew
            // before is not known, so all of it shows once.
            (g.set_bounds)(hdc, core::ptr::null(), 4 | 1);
            with_kept(|t| {
                if let Some(e) = t.iter_mut().find(|e| e.hdc == k.hdc) {
                    e.bounded = true;
                }
            });
            true
        };
        if drew {
            mark_dirty(k.hwnd);
        }
    }
}

/// A DC over `pixels` (w x h, rows `stride` pixels apart), or null.
pub(crate) fn surface_dc(pixels: *mut u32, w: i32, h: i32, stride: i32) -> Handle {
    gdi().map_or(core::ptr::null_mut(), |g| (g.create_dc)(pixels, w, h, stride))
}

pub(crate) fn free_surface_dc(hdc: Handle) {
    if let Some(g) = gdi() {
        (g.delete_dc)(hdc);
    }
}

/// Shows a top-level window's menu bar rows on the desktop.
pub(crate) fn present_bar(hwnd: Hwnd) {
    let Some(w) = window(hwnd) else { return };
    if w.desktop == 0 || !w.visible || w.bar == 0 || w.pixels.is_null() {
        return;
    }
    // SAFETY: the window's own buffer: `bar` rows of `cw` pixels first.
    unsafe {
        GalaWindowPresent(
            w.desktop,
            0,
            0,
            w.cw as u32,
            w.bar as u32,
            buffer_base(&w),
            (w.cw * 4) as usize,
        )
    };
}

/// How long drawing that goes on without a break (no idle moment) may
/// stay off the screen, in ms.
const PRESENT_LAG: u32 = 50;

/// Shows the window's area (of its top-level window's buffer) on the
/// desktop: not at once, but at the thread's next idle moment (`flush`:
/// no message to retrieve, or about to wait), so the many draws of one
/// change reach the desktop as one copy. A resize repaints every child,
/// and every GetDC/ReleaseDC pair ends here, also one that only measured
/// text (wx does that often); copying and composing each of them on its
/// own made FileZilla's repaint after a snap take seconds under emulation
/// (a screendump then caught it half done). Drawing that goes on without
/// a break still shows every PRESENT_LAG ms.
fn present(hwnd: Hwnd) {
    if let Some((root, true)) = mark_dirty(hwnd) {
        flush_window(root);
    }
}

/// Adds the window's area to its top-level window's damage: that
/// window's slot, and whether its damage is older than PRESENT_LAG.
fn mark_dirty(hwnd: Hwnd) -> Option<(usize, bool)> {
    let i = index_of(hwnd)?;
    // SAFETY: a plain call.
    let now = unsafe { GetTickCount() };
    with(|s| {
        if !s.windows[i].used {
            return None;
        }
        let (root, _, [l, t, r, b]) = crate::tree::place_in(s, i);
        if r <= l || b <= t {
            return None;
        }
        let w = &mut s.windows[root];
        w.dirty = if w.dirty[2] <= w.dirty[0] {
            w.dirty_at = now;
            [l, t, r, b]
        } else {
            crate::rect::union(w.dirty, [l, t, r, b])
        };
        Some((root, now.wrapping_sub(w.dirty_at) >= PRESENT_LAG))
    })
}

/// Copies top-level window `i`'s drawn part to the desktop.
fn flush_window(i: usize) {
    let taken = with(|s| {
        let w = &mut s.windows[i];
        if !w.used || w.dirty[2] <= w.dirty[0] {
            return None;
        }
        let d = core::mem::take(&mut w.dirty);
        Some((*w, d))
    });
    let Some((root, d)) = taken else { return };
    // Within the buffer (it may have shrunk since).
    let [l, t, r, b] = [d[0].max(0), d[1].max(0), d[2].min(root.cw), d[3].min(root.ch)];
    if r <= l || b <= t || root.desktop == 0 || root.pixels.is_null() {
        return;
    }
    // SAFETY: the top-level window's own buffer; the rectangle is inside.
    unsafe {
        GalaWindowPresent(
            root.desktop,
            l,
            t + root.bar,
            (r - l) as u32,
            (b - t) as u32,
            root.pixels.add((t * root.cw + l) as usize),
            (root.cw * 4) as usize,
        )
    };
}

/// Shows what was drawn and not yet shown, for every window (`present`,
/// and what DCs still out drew: `present_kept`).
pub(crate) fn flush() {
    present_kept();
    let mut pending = [0u64; MAX_WINDOWS / 64];
    let any = with(|s| {
        let mut any = false;
        for (i, w) in s.windows.iter().enumerate() {
            if w.used && w.dirty[2] > w.dirty[0] {
                pending[i / 64] |= 1 << (i % 64);
                any = true;
            }
        }
        any
    });
    if !any {
        return;
    }
    for i in 0..MAX_WINDOWS {
        if pending[i / 64] & (1 << (i % 64)) != 0 {
            flush_window(i);
        }
    }
}

/// BeginPaint: a DC on the client area, the area to paint (all of it),
/// WM_ERASEBKGND first if the background needs erasing; the window is
/// valid again.
#[unsafe(no_mangle)]
pub extern "system" fn BeginPaint(hwnd: Hwnd, ps: *mut PaintStruct) -> Handle {
    let Some(i) = index_of(hwnd) else { return core::ptr::null_mut() };
    let Some(w) = window(hwnd) else { return core::ptr::null_mut() };
    let hdc = client_dc(hwnd);
    if hdc.is_null() {
        return hdc;
    }
    let erase = with(|s| {
        let w = &mut s.windows[i];
        w.invalid = false;
        core::mem::replace(&mut w.erase, false)
    });
    let erased = erase && SendMessageW(hwnd, WM_ERASEBKGND, hdc as usize, 0) != 0;
    // SAFETY: the caller's PAINTSTRUCT.
    unsafe {
        *ps = PaintStruct {
            hdc,
            erase: i32::from(erase && !erased),
            paint: [0, 0, w.cw, w.ch],
            restore: 0,
            inc_update: 0,
            reserved: [0; 32],
        };
    }
    hdc
}

#[unsafe(no_mangle)]
pub extern "system" fn EndPaint(hwnd: Hwnd, ps: *const PaintStruct) -> i32 {
    if window(hwnd).is_none() {
        return 0;
    }
    present(hwnd);
    // SAFETY: the PAINTSTRUCT BeginPaint filled.
    let hdc = unsafe { (*ps).hdc };
    forget_dc(hdc);
    if let Some(g) = gdi() {
        (g.delete_dc)(hdc);
    }
    1
}

#[unsafe(no_mangle)]
pub extern "system" fn GetDC(hwnd: Hwnd) -> Handle {
    if hwnd == 0 {
        return crate::icon::screen_dc();
    }
    match window(hwnd) {
        Some(_) => client_dc(hwnd),
        // Another process's window: its surface.
        None => crate::surface::foreign_dc(hwnd),
    }
}

/// ReleaseDC: what was drawn shows.
#[unsafe(no_mangle)]
pub extern "system" fn ReleaseDC(hwnd: Hwnd, hdc: Handle) -> i32 {
    if hwnd == 0 {
        return gdi().map_or(0, |g| (g.delete_dc)(hdc));
    }
    if window(hwnd).is_none() {
        return crate::surface::release_foreign(hwnd, hdc).unwrap_or(0);
    }
    present(hwnd);
    forget_dc(hdc);
    gdi().map_or(0, |g| (g.delete_dc)(hdc))
}

/// GetSysColor: gala's colours (COLORREF).
#[unsafe(no_mangle)]
pub extern "system" fn GetSysColor(index: i32) -> u32 {
    match index {
        5 => 0x00ff_ffff,      // COLOR_WINDOW
        8 | 18 => 0x0000_0000, // COLOR_WINDOWTEXT, COLOR_BTNTEXT
        15 => 0x00f0_f0f0,     // COLOR_BTNFACE
        13 => 0x00d7_7800,     // COLOR_HIGHLIGHT
        14 => 0x00ff_ffff,     // COLOR_HIGHLIGHTTEXT
        1 => 0x00a5_6e3a,      // COLOR_DESKTOP (the desktop's colour)
        _ => 0x00c0_c0c0,
    }
}

static SYS_BRUSHES: [AtomicUsize; 32] = [const { AtomicUsize::new(0) }; 32];

#[unsafe(no_mangle)]
pub extern "system" fn GetSysColorBrush(index: i32) -> Handle {
    let Some(slot) = SYS_BRUSHES.get(index as usize) else { return core::ptr::null_mut() };
    let b = slot.load(Ordering::Acquire);
    if b != 0 {
        return b as Handle;
    }
    let Some(g) = gdi() else { return core::ptr::null_mut() };
    let new = (g.solid_brush)(GetSysColor(index)) as usize;
    match slot.compare_exchange(0, new, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => new as Handle,
        Err(other) => other as Handle,
    }
}

/// FillRect(hdc, rect, brush): the brush (or system colour index + 1)
/// over the rectangle.
#[unsafe(no_mangle)]
pub extern "system" fn FillRect(hdc: Handle, rect: *const [i32; 4], brush: Handle) -> i32 {
    let Some(g) = gdi() else { return 0 };
    let brush = if (brush as usize) <= 32 { GetSysColorBrush(brush as i32 - 1) } else { brush };
    if brush.is_null() {
        return 0;
    }
    // SAFETY: the caller's RECT.
    let [l, t, r, b] = unsafe { *rect };
    let old = (g.select)(hdc, brush);
    let ok = (g.pat_blt)(hdc, l, t, r - l, b - t, 0x00f0_0021); // PATCOPY
    if !old.is_null() {
        (g.select)(hdc, old);
    }
    ok
}

// ---- Messages ----

/// DefWindowProcW: WM_NCCREATE succeeds, WM_CLOSE destroys the window,
/// WM_ERASEBKGND fills the class background, WM_PAINT validates.
/// Shows a top-level window's icon on its taskbar button: its own
/// (WM_SETICON, big before small) or its class's.
pub(crate) fn show_icon(hwnd: Hwnd) {
    let Some(w) = window(hwnd) else { return };
    if w.desktop == 0 || w.parent != 0 {
        return;
    }
    let class = with(|s| s.classes[w.class]);
    let h = [w.icon, w.icon_small, class.icon, class.icon_small]
        .into_iter()
        .find(|&h| h != 0)
        .unwrap_or(0);
    let mut px = [0u32; 64 * 64];
    let size = if h == 0 { None } else { crate::icon::argb(h, &mut px) };
    // SAFETY: our pixels, size x size of them (none: no icon).
    unsafe {
        match size {
            Some(n) => GalaWindowSetIcon(w.desktop, px.as_ptr(), n),
            None => GalaWindowSetIcon(w.desktop, core::ptr::null(), 0),
        }
    };
}

#[unsafe(no_mangle)]
pub extern "system" fn DefWindowProcW(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize {
    if let Some(r) = crate::tree::def_window_proc(hwnd, msg, wparam, lparam) {
        return r;
    }
    match msg {
        WM_NCCREATE => 1,
        // Activated and not minimized: the window takes the focus (unless
        // it is already in it).
        WM_ACTIVATE => {
            if wparam & 0xffff != WA_INACTIVE && wparam >> 16 == 0 {
                let focus = crate::tree::GetFocus();
                if focus == 0 || crate::tree::GetAncestor(focus, 2) != hwnd {
                    crate::tree::SetFocus(hwnd);
                }
            }
            0
        }
        // The desktop draws gala's frames and handles clicks on them, so
        // what reaches a window is its client area.
        WM_NCHITTEST => HTCLIENT,
        // The caption's state: nothing to draw (the desktop draws it);
        // TRUE lets the change proceed.
        WM_NCACTIVATE => 1,
        // Alt+F4 (the Alt bit is lParam's bit 29) while the Alt key is
        // held: WM_SYSCOMMAND SC_CLOSE posted to the top-level window. The
        // message alone, given to DefWindowProc while Alt is not held,
        // leaves the window on Windows (measured on 26200, issue #59);
        // that the thread's key state is what it checks is gala's reading
        // of that (not measured with Alt held).
        WM_SYSKEYDOWN if wparam == 0x73 && lparam & (1 << 29) != 0 && key_down(VK_MENU) => {
            let top = crate::tree::GetAncestor(hwnd, 2);
            PostMessageW(if top == 0 { hwnd } else { top }, 0x0112, 0xf060, 0);
            0
        }
        // gala's frame around a top-level window's client area: the
        // border, the caption and a menu bar.
        WM_NCCALCSIZE => {
            if let Some(w) = window(hwnd)
                && w.parent == 0
                && !w.popup
                && lparam != 0
            {
                // SAFETY: the RECT (or NCCALCSIZE_PARAMS, which starts
                // with it) the message carries.
                let r = unsafe { &mut *(lparam as *mut [i32; 4]) };
                r[0] += BORDER;
                r[1] += BORDER + CAPTION + w.bar;
                r[2] -= BORDER;
                r[3] -= BORDER;
            }
            0
        }
        // WM_SETICON: ICON_BIG (1) or ICON_SMALL (0); the previous one.
        WM_SETICON => {
            let Some(i) = index_of(hwnd) else { return 0 };
            let old = with(|s| {
                let w = &mut s.windows[i];
                let slot = if wparam == 1 { &mut w.icon } else { &mut w.icon_small };
                core::mem::replace(slot, lparam as usize)
            });
            show_icon(hwnd);
            old as isize
        }
        // WM_GETICON: ICON_BIG, ICON_SMALL, or ICON_SMALL2 (2: the small
        // one, else the big one).
        WM_GETICON => {
            let Some(w) = window(hwnd) else { return 0 };
            (match wparam {
                1 => w.icon,
                2 if w.icon_small == 0 => w.icon,
                _ => w.icon_small,
            }) as isize
        }
        // The controls' colours: text in COLOR_WINDOWTEXT on the face
        // (COLOR_3DFACE), or on COLOR_WINDOW for edits and list boxes.
        0x0132..=0x0138 => {
            let face = if msg == 0x0133 || msg == 0x0134 { 5 } else { 15 };
            static SET_TEXT: AtomicUsize = AtomicUsize::new(0);
            static SET_BK: AtomicUsize = AtomicUsize::new(0);
            let (t, b) = (
                crate::icon::gdi_proc(&SET_TEXT, b"SetTextColor\0"),
                crate::icon::gdi_proc(&SET_BK, b"SetBkColor\0"),
            );
            if t != 0 && b != 0 {
                type SetColor = extern "system" fn(Handle, u32) -> u32;
                // SAFETY: gdi32's functions of those names.
                let (t, b) = unsafe {
                    (
                        core::mem::transmute::<usize, SetColor>(t),
                        core::mem::transmute::<usize, SetColor>(b),
                    )
                };
                t(wparam as Handle, GetSysColor(8));
                b(wparam as Handle, GetSysColor(face));
            }
            GetSysColorBrush(face) as isize
        }
        WM_CLOSE => {
            DestroyWindow(hwnd);
            0
        }
        WM_ERASEBKGND => {
            let Some(w) = window(hwnd) else { return 0 };
            let background = with(|s| s.classes[w.class].background);
            if background == 0 {
                return 0;
            }
            FillRect(wparam as Handle, &[0, 0, w.cw, w.ch], background as Handle);
            1
        }
        WM_PAINT => {
            let mut ps = PaintStruct {
                hdc: core::ptr::null_mut(),
                erase: 0,
                paint: [0; 4],
                restore: 0,
                inc_update: 0,
                reserved: [0; 32],
            };
            if !BeginPaint(hwnd, &mut ps).is_null() {
                EndPaint(hwnd, &ps);
            }
            0
        }
        _ => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn DefWindowProcA(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize {
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

#[unsafe(no_mangle)]
pub extern "system" fn SendMessageA(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize {
    SendMessageW(hwnd, msg, wparam, lparam)
}

/// The window with the mouse capture (SetCapture), the one last shown or
/// clicked (GetForegroundWindow), and the pointer's last known screen
/// position (from mouse messages).
static CAPTURE: AtomicUsize = AtomicUsize::new(0);
static FOREGROUND: AtomicUsize = AtomicUsize::new(0);

/// The foreground window changes: EVENT_SYSTEM_FOREGROUND when it is
/// another. ShowWindow, SetForegroundWindow and SetWindowPos activate
/// through it.
pub(crate) fn set_foreground(hwnd: Hwnd) {
    activate(hwnd, WA_ACTIVE);
}

/// A left-button press at client point `lparam` of top-level window
/// `top`, which draws its own frame: HTCAPTION from its WM_NCHITTEST (the
/// screen point) makes the desktop drag it, a border (HTLEFT..
/// HTBOTTOMRIGHT) resize it; otherwise the press goes to the window under
/// it as usual.
fn nc_down(top: Hwnd, wparam: usize, lparam: isize) {
    let Some(w) = window(top) else { return };
    let (cx, cy) = (lparam as i16 as i32, (lparam >> 16) as i16 as i32);
    let (b, t) = desktop_frame(&w);
    let (sx, sy) = (w.x + b + cx, w.y + t + cy);
    let screen = (sx as u16 as isize) | (sy as u16 as isize) << 16;
    let hit = SendMessageW(top, WM_NCHITTEST, 0, screen);
    if hit == HTCAPTION && w.desktop != 0 {
        // SAFETY: our desktop window.
        unsafe { GalaWindowDrag(w.desktop) };
        return;
    }
    // HTLEFT..HTBOTTOMRIGHT (10..17): the desktop's edges (1 left, 2 top,
    // 4 right, 8 bottom).
    let edges = match hit {
        10 => 1,
        11 => 4,
        12 => 2,
        13 => 2 | 1,
        14 => 2 | 4,
        15 => 8,
        16 => 8 | 1,
        17 => 8 | 4,
        _ => 0,
    };
    if edges != 0 && w.desktop != 0 {
        // SAFETY: our desktop window.
        unsafe { GalaWindowResize(w.desktop, edges) };
        return;
    }
    let (to, lparam) = crate::tree::route(top, 0x0201, lparam);
    SendMessageW(to, 0x0201, wparam, lparam);
}

/// Top-level window `hwnd` becomes the active (and foreground) window, as
/// on Windows: the one it replaces gets WM_NCACTIVATE (FALSE) and
/// WM_ACTIVATE (WA_INACTIVE, lParam the new one) and loses the focus;
/// `hwnd` gets WM_NCACTIVATE (TRUE) and WM_ACTIVATE (`state`, lParam the
/// old one), whose DefWindowProc gives it the focus. Programs keep their
/// keyboard focus by these (Chromium's views take keys only once their
/// window is activated).
pub(crate) fn activate(hwnd: Hwnd, state: usize) {
    let old = FOREGROUND.swap(hwnd, Ordering::Relaxed);
    if old == hwnd {
        return;
    }
    use crate::winevent::{CHILDID_SELF, EVENT_SYSTEM_FOREGROUND, OBJID_WINDOW, raise};
    raise(EVENT_SYSTEM_FOREGROUND, hwnd, OBJID_WINDOW, CHILDID_SELF);
    if let Some(w) = window(old) {
        SendMessageW(old, WM_NCACTIVATE, 0, 0);
        SendMessageW(old, WM_ACTIVATE, WA_INACTIVE | usize::from(w.iconic) << 16, hwnd as isize);
        // The focus leaves the window when no window of ours takes it.
        let focus = crate::tree::GetFocus();
        if window(hwnd).is_none() && focus != 0 && crate::tree::GetAncestor(focus, 2) == old {
            crate::tree::SetFocus(0);
        }
    }
    if let Some(w) = window(hwnd) {
        // Its owner's last active popup (GetLastActivePopup).
        if let Some(o) = (w.owner != 0).then(|| index_of(w.owner)).flatten() {
            with(|s| s.windows[o].last_popup = hwnd);
        }
        SendMessageW(hwnd, WM_NCACTIVATE, 1, 0);
        SendMessageW(hwnd, WM_ACTIVATE, state | usize::from(w.iconic) << 16, old as isize);
    }
}
static CURSOR: AtomicUsize = AtomicUsize::new(0);
/// The time and pointer position (packed as GetMessagePos's) of the last
/// message GetMessage or PeekMessage retrieved.
static LAST_TIME: AtomicU32 = AtomicU32::new(0);
static LAST_POS: AtomicUsize = AtomicUsize::new(0);
/// The window TrackMouseEvent's TME_LEAVE watches (WM_MOUSELEAVE when the
/// pointer goes to another window).
static TRACKED: AtomicUsize = AtomicUsize::new(0);

const WM_MOUSELEAVE: u32 = 0x02a3;
const TME_LEAVE: u32 = 0x2;
const TME_CANCEL: u32 = 0x8000_0000;
const TME_QUERY: u32 = 0x4000_0000;

/// TrackMouseEvent(tme): TME_LEAVE, a WM_MOUSELEAVE when the pointer leaves
/// the window (once); TME_CANCEL ends it, TME_QUERY tells it. Hover
/// tracking (TME_HOVER) is accepted but sends nothing yet.
#[unsafe(no_mangle)]
pub extern "system" fn TrackMouseEvent(tme: *mut u8) -> i32 {
    if tme.is_null() {
        set_last_error(87);
        return 0;
    }
    // SAFETY: the caller's TRACKMOUSEEVENT: cbSize, dwFlags, hwndTrack,
    // dwHoverTime.
    let (flags_at, hwnd_at) = unsafe { (tme.add(4) as *mut u32, tme.add(8) as *mut Hwnd) };
    // SAFETY: as above.
    let (flags, hwnd) = unsafe { (flags_at.read_unaligned(), hwnd_at.read_unaligned()) };
    if flags & TME_QUERY != 0 {
        let t = TRACKED.load(Ordering::Relaxed);
        // SAFETY: as above.
        unsafe {
            flags_at.write_unaligned(if t != 0 { TME_LEAVE } else { 0 });
            hwnd_at.write_unaligned(t);
        }
        return 1;
    }
    if window(hwnd).is_none() {
        set_last_error(ERROR_INVALID_WINDOW_HANDLE);
        return 0;
    }
    if flags & TME_LEAVE != 0 {
        if flags & TME_CANCEL != 0 {
            let _ = TRACKED.compare_exchange(hwnd, 0, Ordering::Relaxed, Ordering::Relaxed);
        } else {
            TRACKED.store(hwnd, Ordering::Relaxed);
        }
    }
    1
}

#[unsafe(no_mangle)]
pub extern "system" fn GetMessageTime() -> i32 {
    LAST_TIME.load(Ordering::Relaxed) as i32
}

#[unsafe(no_mangle)]
pub extern "system" fn GetMessagePos() -> u32 {
    LAST_POS.load(Ordering::Relaxed) as u32
}

#[unsafe(no_mangle)]
pub extern "system" fn SetCapture(hwnd: Hwnd) -> Hwnd {
    CAPTURE.swap(hwnd, Ordering::Relaxed)
}

/// A window that goes no longer has the capture.
pub(crate) fn release_capture_of(hwnd: Hwnd) {
    let _ = CAPTURE.compare_exchange(hwnd, 0, Ordering::Relaxed, Ordering::Relaxed);
}

#[unsafe(no_mangle)]
pub extern "system" fn ReleaseCapture() -> i32 {
    CAPTURE.store(0, Ordering::Relaxed);
    1
}

#[unsafe(no_mangle)]
pub extern "system" fn GetCapture() -> Hwnd {
    CAPTURE.load(Ordering::Relaxed)
}

/// GetForegroundWindow: the window of ours last shown or clicked (gala's
/// user32 sees only its own process's windows), else NULL.
#[unsafe(no_mangle)]
pub extern "system" fn GetForegroundWindow() -> Hwnd {
    let h = FOREGROUND.load(Ordering::Relaxed);
    if window(h).is_some() { h } else { 0 }
}

/// GetCursorPos: where the last mouse message put the pointer (screen
/// coordinates); (0, 0) before any.
#[unsafe(no_mangle)]
pub extern "system" fn GetCursorPos(point: *mut [i32; 2]) -> i32 {
    if point.is_null() {
        return 0;
    }
    let packed = CURSOR.load(Ordering::Relaxed);
    // SAFETY: the caller's POINT.
    unsafe { *point = [(packed & 0xffff) as i16 as i32, (packed >> 16) as i16 as i32] };
    1
}

/// FindWindowW: a top-level window of ours with that class (NULL: any)
/// and title (NULL: any); gala's user32 sees only its own windows, so
/// another program's (e.g. Pageant's) is not found: NULL.
#[unsafe(no_mangle)]
pub extern "system" fn FindWindowW(class: *const u16, title: *const u16) -> Hwnd {
    // Every process's top-level windows (the kernel's registry).
    let mut probe = [0; 256];
    if crate::foreign::children(0, &mut probe) > 0 {
        return crate::foreign::find(0, 0, class, title);
    }
    let want_class = (!class.is_null() && class as usize > 0xffff).then(|| class_key(class));
    let found = with(|s| {
        s.windows.iter().position(|w| {
            w.used
                && want_class.is_none_or(|k| s.classes[w.class].name == k)
                && (title.is_null() || title_is(w, title))
        })
    });
    found.map_or(0, hwnd_of)
}

/// FindWindowExW(parent, after, class, title): the next child of
/// `parent` (NULL: the desktop's top-level windows; HWND_MESSAGE: none
/// here) after `after` with that class and title.
#[unsafe(no_mangle)]
pub extern "system" fn FindWindowExW(
    parent: Hwnd,
    after: Hwnd,
    class: *const u16,
    title: *const u16,
) -> Hwnd {
    const HWND_MESSAGE: Hwnd = -3isize as Hwnd;
    if parent == HWND_MESSAGE {
        return 0;
    }
    let parent = if parent == crate::tree::DESKTOP { 0 } else { parent };
    crate::foreign::find(parent, after, class, title)
}

#[unsafe(no_mangle)]
pub extern "system" fn FindWindowExA(
    parent: Hwnd,
    after: Hwnd,
    class: *const u8,
    title: *const u8,
) -> Hwnd {
    let mut c = [0u16; 64];
    let mut t = [0u16; 256];
    let wc = if class.is_null() || (class as usize) <= 0xffff {
        class.cast()
    } else {
        ansi_into(class, &mut c)
    };
    let wt = if title.is_null() { core::ptr::null() } else { ansi_into(title, &mut t) };
    FindWindowExW(parent, after, wc, wt)
}

/// The class atom of our window `hwnd`.
pub(crate) fn class_atom_of(hwnd: Hwnd) -> Option<u16> {
    let w = window(hwnd)?;
    Some(with(|s| s.classes[w.class].atom))
}

#[unsafe(no_mangle)]
pub extern "system" fn FindWindowA(class: *const u8, title: *const u8) -> Hwnd {
    let mut c = [0u16; 64];
    let mut t = [0u16; 256];
    let wc = if class.is_null() { core::ptr::null() } else { ansi_into(class, &mut c) };
    let wt = if title.is_null() { core::ptr::null() } else { ansi_into(title, &mut t) };
    FindWindowW(wc, wt)
}

/// Whether window `w`'s title is `title` (NUL-terminated).
fn title_is(w: &Window, title: *const u16) -> bool {
    for (i, &c) in w.title.iter().enumerate() {
        // SAFETY: the caller's NUL-terminated string, read up to its NUL.
        let t = unsafe { *title.add(i) };
        if t != c {
            return false;
        }
        if c == 0 {
            return true;
        }
    }
    false
}

/// An ANSI string (or NULL, or an id below 0x10000) as UTF-16 in `buf`.
pub(crate) fn ansi_into_ptr(s: *const u8, buf: &mut [u16]) -> *const u16 {
    if (s as usize) < 0x10000 {
        return s.cast();
    }
    ansi_into(s, buf)
}

/// `s` (ANSI, NUL-terminated) as UTF-16 in `buf` (Latin-1, truncated).
fn ansi_into(s: *const u8, buf: &mut [u16]) -> *const u16 {
    let mut i = 0;
    // SAFETY: the caller's NUL-terminated string.
    unsafe {
        while i + 1 < buf.len() && *s.add(i) != 0 {
            buf[i] = u16::from(*s.add(i));
            i += 1;
        }
    }
    buf[i] = 0;
    buf.as_ptr()
}

const QS_KEY: u32 = 0x1;
const QS_MOUSEMOVE: u32 = 0x2;
const QS_MOUSEBUTTON: u32 = 0x4;
const QS_POSTMESSAGE: u32 = 0x8;
const QS_PAINT: u32 = 0x20;
const QS_TIMER: u32 = 0x10;

/// GetQueueStatus(flags): what is in this thread's queue (high word) and,
/// as gala keeps no "since last call" state, the same in the low word.
#[unsafe(no_mangle)]
pub extern "system" fn GetQueueStatus(flags: u32) -> u32 {
    pump();
    let thread = me();
    let mut have = with(|s| {
        let mut have = 0;
        for q in s.queue.iter().filter(|q| q.used && q.thread == thread) {
            have |= match q.msg.message {
                0x0100..=0x0109 => QS_KEY,
                0x0200 => QS_MOUSEMOVE,
                0x0201..=0x020d => QS_MOUSEBUTTON,
                _ => QS_POSTMESSAGE,
            };
        }
        if (0..s.windows.len()).any(|i| {
            let w = &s.windows[i];
            w.used && w.thread == thread && w.invalid && crate::tree::shown(s, i)
        }) {
            have |= QS_PAINT;
        }
        have
    });
    if crate::timer::wait_ms() == 0 {
        have |= QS_TIMER;
    }
    have &= flags;
    have << 16 | have
}

/// SendMessageW: the window procedure, called now on this thread, or on
/// the window's thread (another one of ours or another process's) through
/// the kernel. 0 with ERROR_INVALID_WINDOW_HANDLE for no window (a
/// thread's windows go when it ends).
#[unsafe(no_mangle)]
pub extern "system" fn SendMessageW(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize {
    match send_with(hwnd, msg, wparam, lparam, crate::foreign::WAIT) {
        Ok(r) => r,
        Err(e) => {
            set_last_error(e);
            0
        }
    }
}

/// Whether `hwnd` is a window of the calling thread, or one of ours whose
/// message user32 itself answers on any thread.
fn handled_here(hwnd: Hwnd, msg: u32) -> bool {
    window(hwnd).is_some_and(|w| {
        w.thread == me()
            || msg == crate::track::GALA_MENU
            || msg == crate::uia::GALA_UIA
            || msg == crate::tree::GALA_SETWINDOWPOS
            || msg == crate::tree::GALA_SHOWWINDOW
    })
}

/// SendMessage the way `mode` says (crate::foreign::Mode): the result, or
/// the error code. A window of the calling thread is called directly
/// whatever the mode.
pub(crate) fn send_with(
    hwnd: Hwnd,
    msg: u32,
    wparam: usize,
    lparam: isize,
    mode: crate::foreign::Mode,
) -> Result<isize, u32> {
    // Another process's window: through the kernel, the pointers
    // marshaled.
    if hwnd != 0
        && index_of(hwnd).is_none()
        && let Some(r) = crate::foreign::send_remote(hwnd, msg, wparam, lparam, mode)
    {
        return r;
    }
    // gala's menu message for UI Automation: user32's own.
    if msg == crate::track::GALA_MENU && window(hwnd).is_some() {
        return Ok(crate::track::menu_message(hwnd, wparam, lparam));
    }
    // Another process's UI Automation asking our provider: user32's own,
    // handed to our UIAutomationCore.
    if msg == crate::uia::GALA_UIA && window(hwnd).is_some() {
        return Ok(crate::uia::uia_message(hwnd, wparam, lparam));
    }
    // Another process's SetWindowPos of our window: user32's own too.
    if msg == crate::tree::GALA_SETWINDOWPOS && window(hwnd).is_some() {
        return Ok(crate::tree::set_window_pos_message(hwnd, lparam));
    }
    // And its ShowWindow of our window.
    if msg == crate::tree::GALA_SHOWWINDOW && window(hwnd).is_some() {
        return Ok(ShowWindow(hwnd, wparam as u32 as i32) as isize);
    }
    // A window of another thread of ours: its procedure runs on that
    // thread, when it next calls a message function.
    if let Some(w) = window(hwnd)
        && w.thread != me()
    {
        return crate::foreign::send_to_thread(hwnd, msg, wparam, lparam, mode);
    }
    // WH_CALLWNDPROC sees it first: a CWPSTRUCT (lParam, wParam, message,
    // window), wParam nonzero (sent by this thread).
    if crate::hook::any(crate::hook::WH_CALLWNDPROC) {
        #[repr(C)]
        struct CwpStruct {
            lparam: isize,
            wparam: usize,
            message: u32,
            hwnd: Hwnd,
        }
        let cwp = CwpStruct { lparam, wparam, message: msg, hwnd };
        crate::hook::call(crate::hook::WH_CALLWNDPROC, 0, 1, &cwp as *const CwpStruct as isize);
    }
    match window(hwnd) {
        Some(Window { proc_: Some(p), .. }) => Ok(p(hwnd, msg, wparam, lparam)),
        Some(_) => Ok(DefWindowProcW(hwnd, msg, wparam, lparam)),
        None => Err(ERROR_INVALID_WINDOW_HANDLE),
    }
}

const HWND_BROADCAST: Hwnd = 0xffff;
const ERROR_MESSAGE_SYNC_ONLY: u32 = 1159;

/// SendMessageTimeoutW(hwnd, msg, wParam, lParam, flags, ms, *result):
/// nonzero with the result in *result; 0 with ERROR_TIMEOUT when the
/// window's thread did not answer within `ms` (it still handles the
/// message) or is hung (SMTO_ABORTIFHUNG: a message sent to it has
/// waited 5 s). SMTO_BLOCK: messages sent to this thread wait meanwhile.
/// A window of this thread is called directly. HWND_BROADCAST reaches no
/// window yet (TODO) and succeeds.
#[unsafe(no_mangle)]
pub extern "system" fn SendMessageTimeoutW(
    hwnd: Hwnd,
    msg: u32,
    wparam: usize,
    lparam: isize,
    flags: u32,
    ms: u32,
    result: *mut usize,
) -> isize {
    const SMTO_BLOCK: u32 = 1;
    const SMTO_ABORTIFHUNG: u32 = 2;
    const SMTO_NOTIMEOUTIFNOTHUNG: u32 = 8;
    let put = |r: isize| {
        if !result.is_null() {
            // SAFETY: the caller's result.
            unsafe { *result = r as usize };
        }
    };
    if hwnd == HWND_BROADCAST {
        put(0);
        return 1;
    }
    let ms = if flags & SMTO_NOTIMEOUTIFNOTHUNG != 0 { 0xffff_ffff } else { ms };
    let mode = crate::foreign::Mode::Wait {
        ms,
        block: flags & SMTO_BLOCK != 0,
        hung: flags & (SMTO_ABORTIFHUNG | SMTO_NOTIMEOUTIFNOTHUNG) != 0,
    };
    match send_with(hwnd, msg, wparam, lparam, mode) {
        Ok(r) => {
            put(r);
            1
        }
        Err(e) => {
            set_last_error(e);
            0
        }
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn SendMessageTimeoutA(
    hwnd: Hwnd,
    msg: u32,
    wparam: usize,
    lparam: isize,
    flags: u32,
    ms: u32,
    result: *mut usize,
) -> isize {
    SendMessageTimeoutW(hwnd, msg, wparam, lparam, flags, ms, result)
}

/// The checks of the asynchronous send functions (SendNotifyMessage,
/// SendMessageCallback) for a window of another thread: Err(code) for no
/// window, or a system message whose parameters carry a pointer.
fn async_target(hwnd: Hwnd, msg: u32) -> Result<(), u32> {
    if window(hwnd).is_none() && crate::foreign::remote(hwnd).is_none() {
        return Err(ERROR_INVALID_WINDOW_HANDLE);
    }
    if crate::foreign::carries_pointer(msg) {
        return Err(ERROR_MESSAGE_SYNC_ONLY);
    }
    Ok(())
}

/// SendNotifyMessageW: a window of this thread is called now (returning
/// when its procedure has); another thread's gets the message as a sent
/// one (handled before posted ones, when that thread next calls a message
/// function) and this returns at once.
#[unsafe(no_mangle)]
pub extern "system" fn SendNotifyMessageW(
    hwnd: Hwnd,
    msg: u32,
    wparam: usize,
    lparam: isize,
) -> i32 {
    if hwnd == HWND_BROADCAST {
        return 1;
    }
    if handled_here(hwnd, msg) {
        SendMessageW(hwnd, msg, wparam, lparam);
        return 1;
    }
    let sent = async_target(hwnd, msg)
        .and_then(|()| send_with(hwnd, msg, wparam, lparam, crate::foreign::Mode::Notify));
    match sent {
        Ok(_) => 1,
        Err(e) => {
            set_last_error(e);
            0
        }
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn SendNotifyMessageA(
    hwnd: Hwnd,
    msg: u32,
    wparam: usize,
    lparam: isize,
) -> i32 {
    SendNotifyMessageW(hwnd, msg, wparam, lparam)
}

/// SendMessageCallbackW(hwnd, msg, wParam, lParam, callback, data): a
/// window of this thread is called now and the callback right after;
/// another thread's gets the message as a sent one and this returns at
/// once; the callback (window, message, data, result) runs when this
/// thread next calls GetMessage, PeekMessage or WaitMessage after the
/// reply.
#[unsafe(no_mangle)]
pub extern "system" fn SendMessageCallbackW(
    hwnd: Hwnd,
    msg: u32,
    wparam: usize,
    lparam: isize,
    callback: Option<crate::foreign::SendAsyncProc>,
    data: usize,
) -> i32 {
    if handled_here(hwnd, msg) {
        let r = SendMessageW(hwnd, msg, wparam, lparam);
        if let Some(cb) = callback {
            cb(hwnd, msg, data, r);
        }
        return 1;
    }
    let mode = crate::foreign::Mode::Callback { callback, data };
    let sent = async_target(hwnd, msg).and_then(|()| send_with(hwnd, msg, wparam, lparam, mode));
    match sent {
        Ok(_) => 1,
        Err(e) => {
            set_last_error(e);
            0
        }
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn SendMessageCallbackA(
    hwnd: Hwnd,
    msg: u32,
    wparam: usize,
    lparam: isize,
    callback: Option<crate::foreign::SendAsyncProc>,
    data: usize,
) -> i32 {
    SendMessageCallbackW(hwnd, msg, wparam, lparam, callback, data)
}

/// PostThreadMessageW(thread, msg, wParam, lParam): into the queue of any
/// process's thread (ours too), without a window. ERROR_INVALID_THREAD_ID
/// when it is no thread or has no message queue (a thread gets one with
/// its first window or message function call).
#[unsafe(no_mangle)]
pub extern "system" fn PostThreadMessageW(
    thread: u32,
    msg: u32,
    wparam: usize,
    lparam: isize,
) -> i32 {
    crate::foreign::post_thread(thread, msg, wparam, lparam)
}

#[unsafe(no_mangle)]
pub extern "system" fn PostThreadMessageA(
    thread: u32,
    msg: u32,
    wparam: usize,
    lparam: isize,
) -> i32 {
    PostThreadMessageW(thread, msg, wparam, lparam)
}

fn post(thread: u32, msg: Msg) -> bool {
    queue(thread, msg, false)
}

/// Queues the desktop's input for `thread` (after posted messages).
fn post_input(thread: u32, msg: Msg) -> bool {
    queue(thread, msg, true)
}

fn queue(thread: u32, msg: Msg, input: bool) -> bool {
    let ok = with(|s| {
        let seq = s.posted;
        match s.queue.iter_mut().find(|q| !q.used) {
            Some(q) => {
                *q = Queued { used: true, thread, seq, input, msg };
                s.posted += 1;
                true
            }
            None => false,
        }
    });
    if ok {
        wake();
    }
    ok
}

/// Posts `m` to our thread `thread`'s queue (for crate::foreign).
pub(crate) fn post_to(thread: u32, m: Msg) -> bool {
    post(thread, m)
}

pub(crate) fn message(hwnd: Hwnd, message: u32, wparam: usize, lparam: isize) -> Msg {
    // SAFETY: a plain call.
    let time = unsafe { GetTickCount() };
    let c = CURSOR.load(Ordering::Relaxed);
    let pt = [c as u16 as i16 as i32, (c >> 16) as u16 as i16 as i32];
    Msg { hwnd, message, wparam, lparam, time, pt, private: 0 }
}

/// PostMessageW: to the window's thread (a null window: this thread).
#[unsafe(no_mangle)]
pub extern "system" fn PostMessageW(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> i32 {
    let thread = if hwnd == 0 {
        me()
    } else {
        match window(hwnd) {
            Some(w) => w.thread,
            None => return crate::foreign::post_remote(hwnd, msg, wparam, lparam),
        }
    };
    i32::from(post(thread, message(hwnd, msg, wparam, lparam)))
}

#[unsafe(no_mangle)]
pub extern "system" fn PostMessageA(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> i32 {
    PostMessageW(hwnd, msg, wparam, lparam)
}

#[unsafe(no_mangle)]
pub extern "system" fn PostQuitMessage(code: i32) {
    post(me(), message(0, WM_QUIT, code as usize, 0));
}

/// Keys held (by virtual key), from the input seen: GetKeyState.
static KEYS: [AtomicBool; 256] = [const { AtomicBool::new(false) }; 256];
/// Caps Lock's toggle.
static CAPS: AtomicBool = AtomicBool::new(false);
/// The keys as the messages read so far say (GetKeyState, TranslateMessage):
/// Windows documents that this state changes as the thread reads key
/// messages, unlike GetAsyncKeyState's (KEYS, the input as it arrives).
/// gala keeps one per process, not per thread.
static SYNC_KEYS: [AtomicBool; 256] = [const { AtomicBool::new(false) }; 256];
static SYNC_CAPS: AtomicBool = AtomicBool::new(false);

/// A key message taken from the queue: the synchronous key state follows.
fn read_key(m: &Msg) {
    if matches!(m.message, WM_KEYDOWN | WM_SYSKEYDOWN | WM_KEYUP | WM_SYSKEYUP) {
        let down = matches!(m.message, WM_KEYDOWN | WM_SYSKEYDOWN);
        if let Some(k) = SYNC_KEYS.get(m.wparam & 0xff) {
            if down && m.wparam & 0xff == VK_CAPITAL && !k.load(Ordering::Relaxed) {
                SYNC_CAPS.fetch_xor(true, Ordering::Relaxed);
            }
            k.store(down, Ordering::Relaxed);
        }
    }
}

const WM_KEYDOWN: u32 = 0x0100;
const WM_KEYUP: u32 = 0x0101;
const WM_CHAR: u32 = 0x0102;
const WM_SYSKEYDOWN: u32 = 0x0104;
const WM_SYSKEYUP: u32 = 0x0105;
const WM_SYSCHAR: u32 = 0x0106;
const VK_SHIFT: usize = 0x10;
const VK_CONTROL: usize = 0x11;
const VK_MENU: usize = 0x12;
const VK_CAPITAL: usize = 0x14;

/// Moves the desktop's input for our windows into the queue (to each
/// window's thread), keeping the key state.
fn pump() {
    crate::foreign::take_posted();
    crate::foreign::take_sent();
    crate::foreign::take_callbacks();
    crate::winevent::deliver();
    let mut records = [0u8; 24 * 32];
    loop {
        let mut n = 0u32;
        // SAFETY: our buffer of 32 records.
        let status = unsafe { GalaWindowInput(records.as_mut_ptr(), 32, &mut n) };
        if status != 0 || n == 0 {
            return;
        }
        for r in records.chunks_exact(24).take(n as usize) {
            let id = u32::from_le_bytes([r[0], r[1], r[2], r[3]]);
            let message = u32::from_le_bytes([r[4], r[5], r[6], r[7]]);
            let wparam = u64::from_le_bytes(r[8..16].try_into().unwrap_or([0; 8])) as usize;
            let lparam = u64::from_le_bytes(r[16..24].try_into().unwrap_or([0; 8])) as isize;
            if matches!(message, WM_KEYDOWN | WM_SYSKEYDOWN | WM_KEYUP | WM_SYSKEYUP) {
                let down = matches!(message, WM_KEYDOWN | WM_SYSKEYDOWN);
                if let Some(k) = KEYS.get(wparam & 0xff) {
                    if down && wparam == VK_CAPITAL && !k.load(Ordering::Relaxed) {
                        CAPS.fetch_xor(true, Ordering::Relaxed);
                    }
                    k.store(down, Ordering::Relaxed);
                }
            }
            if (0x0200..=0x020d).contains(&message) {
                // The pointer's screen position: the window's client origin
                // plus the client position.
                if let Some(w) =
                    with(|s| s.windows.iter().find(|w| w.used && w.desktop == id).copied())
                {
                    let (cx, cy) = (lparam as i16 as i32, (lparam >> 16) as i16 as i32);
                    let (b, top) = desktop_frame(&w);
                    let (sx, sy) = (w.x + b + cx, w.y + top + cy);
                    CURSOR.store(
                        (sx as u16 as usize) | (sy as u16 as usize) << 16,
                        Ordering::Relaxed,
                    );
                }
            }
            let target = with(|s| {
                s.windows
                    .iter()
                    .position(|w| w.used && w.desktop == id && id != 0)
                    .map(|i| (i, s.windows[i].thread))
            });
            if let Some((i, thread)) = target {
                let top = hwnd_of(i);
                // A click activates the window, on its own thread, before
                // the click is dispatched; a popup (a menu, a drop-down
                // list) leaves the activation where it is, as ShowWindow
                // does.
                if (message == 0x0201 || message == 0x0204) && !with(|s| s.windows[i].popup) {
                    post_input(thread, message_at(top, GALA_WM_ACTIVATE, WA_CLICKACTIVE, 0));
                }
                // In a frame the program draws, its WM_NCHITTEST says
                // whether a press is on its caption.
                if message == 0x0201 && with(|s| s.windows[i].custom.is_some()) {
                    post_input(thread, message_at(top, GALA_WM_NCDOWN, wparam, lparam));
                    continue;
                }
                // The desktop moved the window (a drag): its position.
                if message == 0x0003 {
                    crate::tree::moved_by_desktop(top, lparam);
                }
                // Snap: the window's thread moves it (DispatchMessage).
                if message == GALA_WM_PLACE || message == GALA_WM_MINIMIZED {
                    post_input(thread, message_at(top, message, wparam, lparam));
                    continue;
                }
                // Mouse input goes to the child under the pointer (or the
                // capture), keys to the focus.
                // The menu bar (above the client area) is user32's.
                if (0x0200..=0x020d).contains(&message)
                    && crate::track::bar_input(top, message, lparam)
                {
                    continue;
                }
                let (to, lparam) = crate::tree::route(top, message, lparam);
                // WM_MOUSEWHEEL's position is the screen's.
                let lparam = if message == 0x020a {
                    CURSOR.load(Ordering::Relaxed) as isize
                } else {
                    lparam
                };
                // The pointer left the window TrackMouseEvent watches.
                let tracked = TRACKED.load(Ordering::Relaxed);
                if message == 0x0200 && tracked != 0 && tracked != to {
                    TRACKED.store(0, Ordering::Relaxed);
                    if let Some(w) = window(tracked) {
                        post_input(w.thread, message_at(tracked, WM_MOUSELEAVE, 0, 0));
                    }
                }
                // Keys go to the focus as it is when the message is taken
                // (a click queued just before may move the focus first):
                // `next` routes them.
                if (0x0100..=0x0109).contains(&message) {
                    let mut m = message_at(top, message, wparam, lparam);
                    m.private = ROUTE_KEY;
                    post_input(thread, m);
                    continue;
                }
                post_input(thread, message_at(to, message, wparam, lparam));
            }
        }
        if n < 32 {
            return;
        }
    }
}

fn message_at(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> Msg {
    message(hwnd, msg, wparam, lparam)
}

fn async_key_down(vk: usize) -> bool {
    KEYS.get(vk).is_some_and(|k| k.load(Ordering::Relaxed))
}

fn key_down(vk: usize) -> bool {
    SYNC_KEYS.get(vk).is_some_and(|k| k.load(Ordering::Relaxed))
}

/// The character a key makes (US layout), with Shift, Caps Lock and Ctrl.
fn char_of(vk: usize) -> Option<u16> {
    let shift = key_down(VK_SHIFT);
    let ctrl = key_down(VK_CONTROL);
    let c = match vk {
        0x41..=0x5a => {
            if ctrl {
                return Some((vk - 0x40) as u16);
            }
            let upper = shift != SYNC_CAPS.load(Ordering::Relaxed);
            if upper { vk as u8 } else { vk as u8 + 32 }
        }
        0x30..=0x39 if shift => b")!@#$%^&*("[vk - 0x30],
        0x30..=0x39 => vk as u8,
        0x60..=0x69 => b'0' + (vk - 0x60) as u8, // keypad
        0x20 => b' ',
        0x0d => b'\r',
        0x08 => 0x08,
        0x09 => b'\t',
        0x1b => 0x1b,
        0x6a => b'*',
        0x6b => b'+',
        0x6d => b'-',
        0x6e => b'.',
        0x6f => b'/',
        0xba => {
            if shift {
                b':'
            } else {
                b';'
            }
        }
        0xbb => {
            if shift {
                b'+'
            } else {
                b'='
            }
        }
        0xbc => {
            if shift {
                b'<'
            } else {
                b','
            }
        }
        0xbd => {
            if shift {
                b'_'
            } else {
                b'-'
            }
        }
        0xbe => {
            if shift {
                b'>'
            } else {
                b'.'
            }
        }
        0xbf => {
            if shift {
                b'?'
            } else {
                b'/'
            }
        }
        0xc0 => {
            if shift {
                b'~'
            } else {
                b'`'
            }
        }
        0xdb => {
            if shift {
                b'{'
            } else {
                b'['
            }
        }
        0xdc => {
            if shift {
                b'|'
            } else {
                b'\\'
            }
        }
        0xdd => {
            if shift {
                b'}'
            } else {
                b']'
            }
        }
        0xde => {
            if shift {
                b'"'
            } else {
                b'\''
            }
        }
        _ => return None,
    };
    Some(u16::from(c))
}

/// TranslateMessage: WM_KEYDOWN posts the character the key makes as
/// WM_CHAR (WM_SYSKEYDOWN: WM_SYSCHAR) to the same window; nonzero when it
/// did.
#[unsafe(no_mangle)]
pub extern "system" fn TranslateMessage(msg: *const Msg) -> i32 {
    if msg.is_null() {
        return 0;
    }
    // SAFETY: the caller's MSG.
    let m = unsafe { *msg };
    let kind = match m.message {
        WM_KEYDOWN => WM_CHAR,
        WM_SYSKEYDOWN => WM_SYSCHAR,
        _ => return 0,
    };
    let Some(c) = char_of(m.wparam & 0xff) else { return 0 };
    let thread = window(m.hwnd).map_or_else(me, |w| w.thread);
    i32::from(post(thread, message(m.hwnd, kind, usize::from(c), m.lparam)))
}

/// GetKeyState: bit 15 when the key is down (as the input seen so far
/// says), bit 0 for Caps Lock's toggle.
#[unsafe(no_mangle)]
pub extern "system" fn GetKeyState(vk: i32) -> i16 {
    let vk = vk as usize & 0xff;
    let held = match vk {
        0xa0 | 0xa1 => key_down(VK_SHIFT),
        0xa2 | 0xa3 => key_down(VK_CONTROL),
        0xa4 | 0xa5 => key_down(VK_MENU),
        _ => key_down(vk),
    };
    let toggled = vk == VK_CAPITAL && SYNC_CAPS.load(Ordering::Relaxed);
    (if held { -0x8000i32 } else { 0 } | i32::from(toggled)) as i16
}

/// GetAsyncKeyState: bit 15 when the key is down now (as the input that
/// arrived says, read or not).
#[unsafe(no_mangle)]
pub extern "system" fn GetAsyncKeyState(vk: i32) -> i16 {
    let vk = vk as usize & 0xff;
    let held = match vk {
        0xa0 | 0xa1 => async_key_down(VK_SHIFT),
        0xa2 | 0xa3 => async_key_down(VK_CONTROL),
        0xa4 | 0xa5 => async_key_down(VK_MENU),
        _ => async_key_down(vk),
    };
    if held { -0x8000 } else { 0 }
}

/// The calling thread's next message (matching the filter): posted ones
/// in order (the desktop's input first moved in), then WM_PAINT for an
/// invalid visible window. Removed if `remove`.
/// Marks (in MSG's private field) a key message whose window is the
/// focus at the time it is taken from the queue; `hwnd` holds its
/// top-level window until then.
const ROUTE_KEY: u32 = 0x6761_6b01;

fn next(hwnd: Hwnd, min: u32, max: u32, remove: bool) -> Option<Msg> {
    let mut m = next_raw(hwnd, min, max, remove)?;
    if m.private == ROUTE_KEY {
        m.hwnd = crate::tree::route(m.hwnd, m.message, m.lparam).0;
        m.private = 0;
    }
    // WH_GETMESSAGE sees (and may change) it: wParam PM_REMOVE or
    // PM_NOREMOVE.
    if crate::hook::any(crate::hook::WH_GETMESSAGE) {
        let lp = &mut m as *mut Msg as isize;
        crate::hook::call(crate::hook::WH_GETMESSAGE, 0, usize::from(remove), lp);
    }
    if remove {
        read_key(&m);
    }
    LAST_TIME.store(m.time, Ordering::Relaxed);
    LAST_POS.store((m.pt[0] as u16 as usize) | (m.pt[1] as u16 as usize) << 16, Ordering::Relaxed);
    Some(m)
}

fn next_raw(hwnd: Hwnd, min: u32, max: u32, remove: bool) -> Option<Msg> {
    let m = next_raw_inner(hwnd, min, max, remove);
    // Nothing to do: an idle moment, what was drawn shows.
    if m.is_none() {
        flush();
    }
    m
}

fn next_raw_inner(hwnd: Hwnd, min: u32, max: u32, remove: bool) -> Option<Msg> {
    pump();
    let thread = me();
    let fits = |m: &Msg| {
        (hwnd == 0 || m.hwnd == hwnd || m.message == WM_QUIT)
            && ((min == 0 && max == 0) || (min..=max).contains(&m.message))
    };
    with(|s| {
        if let Some(q) = s
            .queue
            .iter_mut()
            .filter(|q| q.used && q.thread == thread && fits(&q.msg))
            // Posted messages first, oldest first; then input, as
            // GetMessage takes them on Windows (sent, posted, input).
            .min_by_key(|q| (q.input, q.seq))
        {
            if remove {
                q.used = false;
            }
            return Some(q.msg);
        }
        // An invalid shown window whose ancestors are valid (parents paint
        // before their children).
        let i = (0..s.windows.len()).find(|&i| {
            let w = &s.windows[i];
            w.used
                && w.thread == thread
                && w.invalid
                && crate::tree::shown(s, i)
                && !crate::tree::ancestor_invalid(s, i)
        });
        i.map(|i| message(hwnd_of(i), WM_PAINT, 0, 0)).filter(|m| fits(m))
    })
    .or_else(|| crate::timer::due(fits, remove))
}

#[unsafe(no_mangle)]
pub extern "system" fn GetMessageW(msg: *mut Msg, hwnd: Hwnd, min: u32, max: u32) -> i32 {
    loop {
        if let Some(m) = next(hwnd, min, max, true) {
            // SAFETY: the caller's MSG.
            unsafe { *msg = m };
            return i32::from(m.message != WM_QUIT);
        }
        // Until a message comes or the next timer is due.
        // SAFETY: our event.
        unsafe { WaitForSingleObject(event(), crate::timer::wait_ms()) };
    }
}

/// MsgWaitForMultipleObjectsEx(count, handles, ms, mask, flags): until one
/// of the handles is signalled (WAIT_OBJECT_0 + its index), a message of
/// the wake mask is in the queue (WAIT_OBJECT_0 + count; gala does not tell
/// new input from old, so it is as with MWMO_INPUTAVAILABLE), or the
/// time-out. MWMO_WAITALL waits for all the handles alone.
#[unsafe(no_mangle)]
pub extern "system" fn MsgWaitForMultipleObjectsEx(
    count: u32,
    handles: *const Handle,
    ms: u32,
    mask: u32,
    flags: u32,
) -> u32 {
    const MWMO_WAITALL: u32 = 1;
    const WAIT_TIMEOUT: u32 = 258;
    const WAIT_FAILED: u32 = 0xffff_ffff;
    const INFINITE: u32 = 0xffff_ffff;
    if count > 63 || (count > 0 && handles.is_null()) {
        set_last_error(87); // ERROR_INVALID_PARAMETER
        return WAIT_FAILED;
    }
    // About to wait: what was drawn shows.
    flush();
    if flags & MWMO_WAITALL != 0 {
        // SAFETY: the caller's array of `count` handles.
        return unsafe { WaitForMultipleObjects(count, handles, 1, ms) };
    }
    let mut all = [0 as Handle; 64];
    for (i, h) in all.iter_mut().take(count as usize).enumerate() {
        // SAFETY: the caller's array of `count` handles.
        *h = unsafe { *handles.add(i) };
    }
    all[count as usize] = event();
    // SAFETY: a plain call.
    let start = unsafe { GetTickCount() };
    loop {
        if GetQueueStatus(mask) >> 16 != 0 {
            return count;
        }
        // SAFETY: a plain call.
        let gone = unsafe { GetTickCount() }.wrapping_sub(start);
        let left = if ms == INFINITE { INFINITE } else { ms.saturating_sub(gone) };
        let timer = if mask & QS_TIMER != 0 { crate::timer::wait_ms() } else { INFINITE };
        // SAFETY: our array of count + 1 handles.
        let r = unsafe { WaitForMultipleObjects(count + 1, all.as_ptr(), 0, left.min(timer)) };
        match r {
            // A handle of the caller's (or abandoned: WAIT_ABANDONED_0 + i).
            _ if r < count || (0x80..0x80 + count).contains(&r) => return r,
            WAIT_TIMEOUT if timer >= left => return WAIT_TIMEOUT,
            WAIT_FAILED => return r,
            _ => {}
        }
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn MsgWaitForMultipleObjects(
    count: u32,
    handles: *const Handle,
    all: i32,
    ms: u32,
    mask: u32,
) -> u32 {
    MsgWaitForMultipleObjectsEx(count, handles, ms, mask, u32::from(all != 0))
}

/// WaitMessage: until a message is in the queue.
#[unsafe(no_mangle)]
pub extern "system" fn WaitMessage() -> i32 {
    MsgWaitForMultipleObjectsEx(0, core::ptr::null(), 0xffff_ffff, 0x04ff, 0);
    1
}

#[unsafe(no_mangle)]
pub extern "system" fn GetMessageA(msg: *mut Msg, hwnd: Hwnd, min: u32, max: u32) -> i32 {
    GetMessageW(msg, hwnd, min, max)
}

/// PeekMessageW(msg, hwnd, min, max, remove): PM_REMOVE (1) takes it.
#[unsafe(no_mangle)]
pub extern "system" fn PeekMessageW(
    msg: *mut Msg,
    hwnd: Hwnd,
    min: u32,
    max: u32,
    remove: u32,
) -> i32 {
    match next(hwnd, min, max, remove & 1 != 0) {
        Some(m) => {
            // SAFETY: the caller's MSG.
            unsafe { *msg = m };
            1
        }
        None => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn PeekMessageA(
    msg: *mut Msg,
    hwnd: Hwnd,
    min: u32,
    max: u32,
    remove: u32,
) -> i32 {
    PeekMessageW(msg, hwnd, min, max, remove)
}

#[unsafe(no_mangle)]
pub extern "system" fn DispatchMessageW(msg: *const Msg) -> isize {
    // SAFETY: the caller's MSG.
    let m = unsafe { *msg };
    // WM_TIMER with a TIMERPROC: the procedure, not the window's.
    if m.message == crate::timer::WM_TIMER && m.lparam != 0 {
        type TimerProc = extern "system" fn(Hwnd, u32, usize, u32);
        // SAFETY: the TIMERPROC SetTimer was given (WM_TIMER's lParam).
        let p = unsafe { core::mem::transmute::<isize, TimerProc>(m.lparam) };
        // SAFETY: a plain call.
        p(m.hwnd, m.message, m.wparam, unsafe { GetTickCount() });
        return 0;
    }
    if m.hwnd == 0 {
        return 0;
    }
    if m.message == GALA_WM_ACTIVATE {
        activate(m.hwnd, m.wparam);
        return 0;
    }
    if m.message == GALA_WM_NCDOWN {
        nc_down(m.hwnd, m.wparam, m.lparam);
        return 0;
    }
    if m.message == GALA_WM_PLACE {
        let (x, y) = (i32::from(m.wparam as u16 as i16), i32::from((m.wparam >> 16) as u16 as i16));
        let (w, h) = (i32::from(m.lparam as u16), i32::from((m.lparam as usize >> 16) as u16));
        crate::tree::snap(m.hwnd, x, y, w, h);
        return 0;
    }
    if m.message == GALA_WM_MINIMIZED {
        crate::tree::minimize(m.hwnd, m.wparam != 0, false);
        return 0;
    }
    SendMessageW(m.hwnd, m.message, m.wparam, m.lparam)
}

#[unsafe(no_mangle)]
pub extern "system" fn DispatchMessageA(msg: *const Msg) -> isize {
    DispatchMessageW(msg)
}

/// LoadCursorW/LoadIconW: a stand-in handle (there is no pointer yet).
#[unsafe(no_mangle)]
pub extern "system" fn SetCursor(_cursor: Handle) -> Handle {
    0x0001_0003 as Handle
}

/// GetCursor: the cursor SetCursor last set (gala draws no pointer of
/// its own yet: the same stand-in handle).
#[unsafe(no_mangle)]
pub extern "system" fn GetCursor() -> Handle {
    0x0001_0003 as Handle
}

/// IsWindowUnicode: every window gala's user32 makes is a Unicode window
/// (its ANSI functions convert and call the W ones); FALSE for a handle
/// that is not a window.
#[unsafe(no_mangle)]
pub extern "system" fn IsWindowUnicode(hwnd: Hwnd) -> i32 {
    IsWindow(hwnd)
}

/// GetDoubleClickTime: Windows' default, 500 ms (gala has no setting to
/// change it; comctl32's tooltips derive their delays from it).
#[unsafe(no_mangle)]
pub extern "system" fn GetDoubleClickTime() -> u32 {
    500
}

/// GetSystemMetrics: the desktop's frame (SM_CYCAPTION, SM_CXBORDER,
/// SM_CYBORDER, the caption buttons' SM_CXSIZE and SM_CYSIZE) and the
/// screen (crate::monitor).
#[unsafe(no_mangle)]
pub extern "system" fn GetSystemMetrics(index: i32) -> i32 {
    let (screen, work) = crate::monitor::screen();
    match index {
        0 | 78 => screen[2],     // SM_CXSCREEN, SM_CXVIRTUALSCREEN
        1 | 79 => screen[3],     // SM_CYSCREEN, SM_CYVIRTUALSCREEN
        16 => work[2],           // SM_CXFULLSCREEN
        17 => work[3] - CAPTION, // SM_CYFULLSCREEN
        80 => 1,                 // SM_CMONITORS
        4 => CAPTION,
        15 => MENU_BAR, // SM_CYMENU
        5 | 6 => BORDER,
        30 => 46,      // SM_CXSIZE: the desktop's caption buttons
        31 => CAPTION, // SM_CYSIZE
        11..=14 => 32, // SM_CXICON, SM_CYICON, SM_CXCURSOR, SM_CYCURSOR
        49 | 50 => 16, // SM_CXSMICON, SM_CYSMICON
        _ => 0,
    }
}

/// AdjustWindowRectEx(rect, style, menu, ex_style): the window rectangle
/// for that client rectangle: gala's frame (none for a child or a popup
/// without a caption) and the menu bar.
#[unsafe(no_mangle)]
pub extern "system" fn AdjustWindowRectEx(
    rect: *mut [i32; 4],
    style: u32,
    menu: i32,
    _ex: u32,
) -> i32 {
    if rect.is_null() {
        set_last_error(87);
        return 0;
    }
    const WS_CHILD: u32 = 0x4000_0000;
    let framed = style & WS_CHILD == 0 && !is_popup(style);
    let (b, top) = if framed { (BORDER, BORDER + CAPTION) } else { (0, 0) };
    let bar = if framed && menu != 0 { MENU_BAR } else { 0 };
    // SAFETY: the caller's RECT.
    unsafe {
        let r = &mut *rect;
        r[0] -= b;
        r[1] -= top + bar;
        r[2] += b;
        r[3] += b;
    }
    1
}

#[unsafe(no_mangle)]
pub extern "system" fn AdjustWindowRect(rect: *mut [i32; 4], style: u32, menu: i32) -> i32 {
    AdjustWindowRectEx(rect, style, menu, 0)
}

/// CallWindowProcW(proc, hwnd, msg, wparam, lparam): calls the procedure
/// (a subclassed window's previous one); 0 for none.
#[unsafe(no_mangle)]
pub extern "system" fn CallWindowProcW(
    proc_: Option<WndProc>,
    hwnd: Hwnd,
    msg: u32,
    wparam: usize,
    lparam: isize,
) -> isize {
    proc_.map_or(0, |p| p(hwnd, msg, wparam, lparam))
}

/// CallWindowProcA: as CallWindowProcW (the ANSI code page is UTF-8 and
/// gala's procedures are all wide).
#[unsafe(no_mangle)]
pub extern "system" fn CallWindowProcA(
    proc_: Option<WndProc>,
    hwnd: Hwnd,
    msg: u32,
    wparam: usize,
    lparam: isize,
) -> isize {
    CallWindowProcW(proc_, hwnd, msg, wparam, lparam)
}

/// GetUpdateRgn(hwnd, rgn, erase): the region to paint in `rgn` (client
/// coordinates). gala invalidates whole windows, so it is the client area
/// while the window is invalid and empty otherwise; SIMPLEREGION or
/// NULLREGION.
#[unsafe(no_mangle)]
pub extern "system" fn GetUpdateRgn(hwnd: Hwnd, rgn: Handle, _erase: i32) -> i32 {
    let Some(w) = window(hwnd) else { return 0 };
    static SET_RECT_RGN: AtomicUsize = AtomicUsize::new(0);
    let f = crate::icon::gdi_proc(&SET_RECT_RGN, b"SetRectRgn\0");
    if f == 0 {
        return 0;
    }
    type SetRectRgn = extern "system" fn(Handle, i32, i32, i32, i32) -> i32;
    // SAFETY: gdi32's SetRectRgn.
    let set: SetRectRgn = unsafe { core::mem::transmute::<usize, SetRectRgn>(f) };
    if w.invalid {
        set(rgn, 0, 0, w.cw, w.ch);
        2 // SIMPLEREGION
    } else {
        set(rgn, 0, 0, 0, 0);
        1 // NULLREGION
    }
}

/// GetUpdateRect(hwnd, rect, erase): whether the window needs painting,
/// and the client area as the rectangle (or an empty one).
#[unsafe(no_mangle)]
pub extern "system" fn GetUpdateRect(hwnd: Hwnd, rect: *mut [i32; 4], _erase: i32) -> i32 {
    let Some(w) = window(hwnd) else { return 0 };
    if !rect.is_null() {
        // SAFETY: the caller's RECT.
        unsafe { *rect = if w.invalid { [0, 0, w.cw, w.ch] } else { [0; 4] } };
    }
    i32::from(w.invalid)
}

/// GetDialogBaseUnits: the system font's average character width (low
/// word) and height (high word): 8 and 16 at 96 dpi.
#[unsafe(no_mangle)]
pub extern "system" fn GetDialogBaseUnits() -> i32 {
    8 | 16 << 16
}

/// GetActiveWindow: the foreground window, if it is the calling thread's
/// (the active window is per thread; gala's desktop activates one window
/// at a time).
#[unsafe(no_mangle)]
pub extern "system" fn GetActiveWindow() -> Hwnd {
    let h = GetForegroundWindow();
    match window(h) {
        Some(w) if w.thread == me() => h,
        _ => 0,
    }
}

/// FlashWindowEx(info): whether the window's caption was active before
/// (FLASHWINFO: cbSize, hwnd, dwFlags, uCount, dwTimeout). The desktop
/// does not flash captions or taskbar buttons yet.
#[unsafe(no_mangle)]
pub extern "system" fn FlashWindowEx(info: *const [usize; 2]) -> i32 {
    if info.is_null() {
        set_last_error(87);
        return 0;
    }
    // SAFETY: the caller's FLASHWINFO: cbSize (padded to a pointer), hwnd.
    let hwnd = unsafe { info.read_unaligned()[1] };
    i32::from(hwnd != 0 && GetActiveWindow() == hwnd)
}

/// FlashWindow(hwnd, invert): as FlashWindowEx.
#[unsafe(no_mangle)]
pub extern "system" fn FlashWindow(hwnd: Hwnd, _invert: i32) -> i32 {
    i32::from(hwnd != 0 && GetActiveWindow() == hwnd)
}

/// SetActiveWindow: activates a top-level window of the calling thread;
/// the previous active window.
#[unsafe(no_mangle)]
pub extern "system" fn SetActiveWindow(hwnd: Hwnd) -> Hwnd {
    let previous = GetActiveWindow();
    if window(hwnd).is_some() {
        SetForegroundWindow(hwnd);
    }
    previous
}

/// SetForegroundWindow: the window becomes the foreground (and active)
/// window, raised on the desktop.
#[unsafe(no_mangle)]
pub extern "system" fn SetForegroundWindow(hwnd: Hwnd) -> i32 {
    if window(hwnd).is_none() {
        return 0;
    }
    set_foreground(hwnd);
    BringWindowToTop(hwnd)
}

/// BringWindowToTop: to the top of the z order (SetWindowPos HWND_TOP).
#[unsafe(no_mangle)]
pub extern "system" fn BringWindowToTop(hwnd: Hwnd) -> i32 {
    crate::tree::SetWindowPos(hwnd, 0, 0, 0, 0, 0, 0x1 | 0x2 | 0x10) // NOSIZE | NOMOVE | NOACTIVATE
}

/// GetClassInfoExW(hInstance, lpszClass, lpwcx): the class's WNDCLASSEXW
/// (style, procedure, window extra bytes, background, icons; the name as
/// given); its atom, or
/// 0 with ERROR_CLASS_DOES_NOT_EXIST.
#[unsafe(no_mangle)]
pub extern "system" fn GetClassInfoExW(
    instance: Handle,
    name: *const u16,
    out: *mut WndClassExW,
) -> i32 {
    if out.is_null() {
        set_last_error(87);
        return 0;
    }
    crate::controls::register_system_classes();
    let found = with(|s| find_class(s, name).map(|i| s.classes[i]));
    let Some(c) = found else {
        set_last_error(ERROR_CLASS_DOES_NOT_EXIST);
        return 0;
    };
    // SAFETY: the caller's WNDCLASSEXW (its cbSize is theirs).
    unsafe {
        let size = (*out).size;
        out.write(WndClassExW {
            size,
            style: c.style,
            proc_: c.proc_,
            class_extra: 0,
            window_extra: c.window_extra as i32,
            instance,
            icon: c.icon as Handle,
            cursor: core::ptr::null_mut(),
            background: c.background as Handle,
            menu_name: core::ptr::null(),
            class_name: name,
            icon_small: c.icon_small as Handle,
        });
    }
    i32::from(c.atom)
}

/// GetClassLongPtrW(hWnd, nIndex): the window's class's procedure
/// (GCLP_WNDPROC), background (GCLP_HBRBACKGROUND), icons (GCLP_HICON,
/// GCLP_HICONSM), atom (GCW_ATOM) and style (GCL_STYLE); 0 for the
/// others (no extra class bytes, menus or cursors are kept).
#[unsafe(no_mangle)]
pub extern "system" fn GetClassLongPtrW(hwnd: Hwnd, index: i32) -> usize {
    let class = window(hwnd).map(|w| with(|s| s.classes[w.class]));
    let Some(c) = class else {
        set_last_error(1400);
        return 0;
    };
    match index {
        -24 => c.proc_.map_or(0, |p| p as usize),
        -10 => c.background,
        -14 => c.icon,
        -34 => c.icon_small,
        -32 => usize::from(c.atom),
        -26 => c.style as usize,
        _ => 0,
    }
}

/// GetClassLongW(hWnd, nIndex): GetClassLongPtrW's value as a DWORD
/// (what 32-bit programs call: their GetClassLongPtrW is this).
#[unsafe(no_mangle)]
pub extern "system" fn GetClassLongW(hwnd: Hwnd, index: i32) -> u32 {
    GetClassLongPtrW(hwnd, index) as u32
}

/// SetClassLongW(hWnd, nIndex, value): SetClassLongPtrW with a DWORD.
#[unsafe(no_mangle)]
pub extern "system" fn SetClassLongW(hwnd: Hwnd, index: i32, value: i32) -> u32 {
    SetClassLongPtrW(hwnd, index, value as usize) as u32
}

/// SetClassLongPtrW(hWnd, nIndex, value): changes the window's class
/// (for every window of the class, as documented) and returns the old
/// value: the procedure (GCLP_WNDPROC), background (GCLP_HBRBACKGROUND),
/// icons (GCLP_HICON, GCLP_HICONSM) and style (GCL_STYLE), the ones
/// GetClassLongPtrW reads. Another index (extra class bytes, the
/// cursor, the menu name) is not kept: recorded in the ledger, 0 returned
/// with ERROR_INVALID_INDEX.
#[unsafe(no_mangle)]
pub extern "system" fn SetClassLongPtrW(hwnd: Hwnd, index: i32, value: usize) -> usize {
    let Some(w) = window(hwnd) else {
        set_last_error(1400);
        return 0;
    };
    let old = with(|s| {
        let c = &mut s.classes[w.class];
        Some(match index {
            -24 => {
                let old = c.proc_.map_or(0, |p| p as usize);
                // SAFETY: the caller gives a window procedure (or NULL), as
                // SetClassLongPtr requires.
                c.proc_ = unsafe { core::mem::transmute::<usize, Option<WndProc>>(value) };
                old
            }
            -10 => core::mem::replace(&mut c.background, value),
            -14 => core::mem::replace(&mut c.icon, value),
            -34 => core::mem::replace(&mut c.icon_small, value),
            -26 => core::mem::replace(&mut c.style, value as u32) as usize,
            _ => return None,
        })
    });
    match old {
        Some(v) => {
            set_last_error(0);
            v
        }
        None => {
            unsupported(b"user32.dll!SetClassLongPtrW(index)", index as u64);
            set_last_error(1413); // ERROR_INVALID_INDEX
            0
        }
    }
}

/// SetGestureConfig: accepted for an existing window (else
/// ERROR_INVALID_WINDOW_HANDLE, checked first) given at least one
/// GESTURECONFIG of exactly 12 bytes; no entries, a NULL array or another
/// size is ERROR_INVALID_PARAMETER ("reset" is not spelled (0, NULL, 0);
/// Windows 11). gala has no touch input, so no gesture is ever delivered.
#[unsafe(no_mangle)]
pub extern "system" fn SetGestureConfig(
    hwnd: Hwnd,
    _reserved: u32,
    count: u32,
    config: *const u8,
    size: u32,
) -> i32 {
    const GESTURECONFIG_SIZE: u32 = 12;
    // The window first (win32_misc_test: a NULL hwnd with no entries is
    // ERROR_INVALID_WINDOW_HANDLE), then the entries (gui_test).
    if IsWindow(hwnd) == 0 {
        set_last_error(1400); // ERROR_INVALID_WINDOW_HANDLE
        return 0;
    }
    if count == 0 || config.is_null() || size != GESTURECONFIG_SIZE {
        set_last_error(87); // ERROR_INVALID_PARAMETER
        return 0;
    }
    1
}

/// GetGestureInfo: a NULL GESTUREINFO is ERROR_INVALID_PARAMETER before
/// the handle is looked at (Windows 11 25H2); otherwise no WM_GESTURE is
/// ever sent, so no gesture handle is valid (ERROR_INVALID_HANDLE).
#[unsafe(no_mangle)]
pub extern "system" fn GetGestureInfo(_gesture: usize, info: *mut u8) -> i32 {
    set_last_error(if info.is_null() { 87 } else { 6 });
    0
}

/// CloseGestureInfoHandle: as GetGestureInfo, no handle is valid.
#[unsafe(no_mangle)]
pub extern "system" fn CloseGestureInfoHandle(_gesture: usize) -> i32 {
    set_last_error(6);
    0
}

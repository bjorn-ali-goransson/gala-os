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
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

pub(crate) type Handle = *mut c_void;
pub(crate) type Hwnd = usize;
pub type WndProc = extern "system" fn(Hwnd, u32, usize, isize) -> isize;

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn GalaWindowCreate(
        x: i32,
        y: i32,
        w: u32,
        h: u32,
        title: *const u16,
        len: usize,
        id: *mut u32,
    ) -> u32;
    fn GalaWindowShow(id: u32, show: u8) -> u32;
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
    fn GalaRecordMissing(name: *const u8, length: usize, what: u32, detail: u64) -> u32;
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetCurrentThreadId() -> u32;
    fn CreateEventW(sa: *const c_void, manual: i32, initial: i32, name: *const u16) -> Handle;
    fn SetEvent(h: Handle) -> i32;
    fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
    fn VirtualAlloc(at: *mut c_void, size: usize, kind: u32, protect: u32) -> *mut c_void;
    fn VirtualFree(at: *mut c_void, size: usize, kind: u32) -> i32;
    fn LoadLibraryW(name: *const u16) -> Handle;
    fn GetProcAddress(module: Handle, name: *const u8) -> *const c_void;
    fn GetTickCount() -> u32;
    fn SetLastError(code: u32);
}

pub const BORDER: i32 = 1;
pub const CAPTION: i32 = 30;
const MAX_CLASSES: usize = 32;
pub(crate) const MAX_WINDOWS: usize = 512;
const QUEUE: usize = 256;
const HWND_BASE: usize = 0x0001_0000;

const WM_NCCREATE: u32 = 0x0081;
const WM_NCDESTROY: u32 = 0x0082;
const WM_CREATE: u32 = 0x0001;
const WM_DESTROY: u32 = 0x0002;
const WM_PAINT: u32 = 0x000f;
const WM_CLOSE: u32 = 0x0010;
const WM_QUIT: u32 = 0x0012;
const WM_ERASEBKGND: u32 = 0x0014;
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
    spelled: [u16; 64],
    pub(crate) proc_: Option<WndProc>,
    background: usize,
    #[allow(dead_code)]
    style: u32,
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

#[derive(Clone, Copy)]
struct Queued {
    used: bool,
    thread: u32,
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
};
const NO_MSG: Msg =
    Msg { hwnd: 0, message: 0, wparam: 0, lparam: 0, time: 0, pt: [0; 2], private: 0 };

pub(crate) struct State {
    pub(crate) classes: [Class; MAX_CLASSES],
    pub(crate) windows: [Window; MAX_WINDOWS],
    queue: [Queued; QUEUE],
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
        queue: [Queued { used: false, thread: 0, msg: NO_MSG }; QUEUE],
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

pub(crate) fn index_of(hwnd: Hwnd) -> Option<usize> {
    let i = hwnd.checked_sub(HWND_BASE)? / 0x10;
    (hwnd == HWND_BASE + i * 0x10 && i < MAX_WINDOWS).then_some(i)
}

pub(crate) fn hwnd_of(i: usize) -> Hwnd {
    HWND_BASE + i * 0x10
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

struct Gdi {
    create_dc: extern "system" fn(*mut u32, i32, i32, i32) -> Handle,
    delete_dc: extern "system" fn(Handle) -> i32,
    solid_brush: extern "system" fn(u32) -> Handle,
    select: extern "system" fn(Handle, Handle) -> Handle,
    pat_blt: extern "system" fn(Handle, i32, i32, i32, i32, u32) -> i32,
    create_child_dc: ChildDc,
}

type ChildDc = extern "system" fn(*mut u32, i32, i32, i32, i32, i32, *const [i32; 4]) -> Handle;

static GDI: AtomicUsize = AtomicUsize::new(0);
static GDI_FNS: Shared2 = Shared2(UnsafeCell::new(None));

struct Shared2(UnsafeCell<Option<Gdi>>);
// SAFETY: written once before GDI is set (Release), read after (Acquire).
unsafe impl Sync for Shared2 {}

fn gdi() -> Option<&'static Gdi> {
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
        if [a, b, c, d, e, child].iter().any(|p| p.is_null()) {
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

/// A class by name (or atom: a value under 0x10000).
pub(crate) fn find_class(s: &State, name: *const u16) -> Option<usize> {
    if (name as usize) < 0x1_0000 {
        let atom = name as usize as u16;
        return s.classes.iter().position(|c| c.used && c.atom == atom);
    }
    let key = class_key(name);
    s.classes.iter().position(|c| c.used && c.name == key)
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
    let (w, h) = if w == CW_USEDEFAULT {
        (640, 480)
    } else {
        (w.max(2 * BORDER + 1), h.max(2 * BORDER + CAPTION + 1))
    };
    let (cw, ch) = (w - 2 * BORDER, h - 2 * BORDER - CAPTION);
    // SAFETY: plain calls; the title is the caller's NUL-terminated string.
    let (pixels, desktop) = unsafe {
        let pixels =
            VirtualAlloc(core::ptr::null_mut(), (cw * ch * 4) as usize, 0x3000, 4).cast::<u32>();
        if pixels.is_null() {
            return 0;
        }
        let mut len = 0;
        if !name.is_null() {
            while len < 256 && *name.add(len) != 0 {
                len += 1;
            }
        }
        let mut id = 0;
        // Without a screen the window exists but is never shown.
        let _ = GalaWindowCreate(x, y, w as u32, h as u32, name, len, &mut id);
        (pixels, id)
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
        };
        Some(i)
    });
    let Some(i) = created else {
        // SAFETY: our buffer and window.
        unsafe {
            VirtualFree(pixels.cast(), 0, 0x8000);
            if desktop != 0 {
                GalaWindowDestroy(desktop);
            }
            SetLastError(ERROR_CLASS_DOES_NOT_EXIST);
        }
        return 0;
    };
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
            // SAFETY: the window's buffer and desktop window.
            unsafe {
                if gone.desktop != 0 {
                    GalaWindowDestroy(gone.desktop);
                }
                if !gone.pixels.is_null() {
                    VirtualFree(gone.pixels.cast(), 0, 0x8000);
                }
            }
            return 0;
        }
    }
    if SendMessageW(hwnd, WM_NCCREATE, 0, lp) == 0 || SendMessageW(hwnd, WM_CREATE, 0, lp) == -1 {
        DestroyWindow(hwnd);
        return 0;
    }
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
    // WM_DESTROY to the window, then its children (and theirs).
    SendMessageW(hwnd, WM_DESTROY, 0, 0);
    for child in crate::tree::children_of(hwnd).iter().copied().filter(|&c| c != 0) {
        DestroyWindow(child);
    }
    SendMessageW(hwnd, WM_NCDESTROY, 0, 0);
    crate::tree::forget(hwnd);
    let Some(i) = index_of(hwnd) else { return 0 };
    crate::timer::window_destroyed(hwnd);
    crate::menu::window_destroyed(hwnd);
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
    // SAFETY: the window's buffer and desktop window.
    unsafe {
        if gone.desktop != 0 {
            GalaWindowDestroy(gone.desktop);
        }
        if !gone.pixels.is_null() {
            VirtualFree(gone.pixels.cast(), 0, 0x8000);
        }
    }
    // Its area of the parent shows the parent again.
    if parent != 0 && was_shown {
        InvalidateRect(parent, core::ptr::null(), 1);
    }
    1
}

#[unsafe(no_mangle)]
pub extern "system" fn IsWindow(hwnd: Hwnd) -> i32 {
    i32::from(window(hwnd).is_some())
}

#[unsafe(no_mangle)]
pub extern "system" fn IsWindowVisible(hwnd: Hwnd) -> i32 {
    i32::from(with(|s| {
        index_of(hwnd).is_some_and(|i| s.windows[i].used && crate::tree::shown(s, i))
    }))
}

/// ShowWindow: whether it was visible before.
#[unsafe(no_mangle)]
pub extern "system" fn ShowWindow(hwnd: Hwnd, cmd: i32) -> i32 {
    let Some(i) = index_of(hwnd) else { return 0 };
    let show = cmd != 0; // SW_HIDE
    if window(hwnd).is_some_and(|w| w.parent != 0) {
        return crate::tree::show_child(hwnd, show);
    }
    let (was, desktop) = with(|s| {
        let w = &mut s.windows[i];
        let was = w.visible;
        if w.used {
            w.visible = show;
            if show && !was {
                w.invalid = true;
                w.erase = true;
            }
        }
        (was, w.desktop)
    });
    if desktop != 0 {
        // SAFETY: our desktop window.
        unsafe { GalaWindowShow(desktop, u8::from(show)) };
    }
    if show {
        FOREGROUND.store(hwnd, Ordering::Relaxed);
        crate::tree::shown_top_level(hwnd);
        InvalidateRect(hwnd, core::ptr::null(), 1);
        wake();
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

/// A DC on the window's client area (a child's: its part of the
/// top-level window's buffer).
fn client_dc(hwnd: Hwnd) -> Handle {
    let Some(g) = gdi() else { return core::ptr::null_mut() };
    let Some((root, offset, visible)) = crate::tree::placement(hwnd) else {
        return core::ptr::null_mut();
    };
    if offset == (0, 0) && window(hwnd).is_some_and(|w| w.parent == 0) {
        return (g.create_dc)(root.pixels, root.cw, root.ch, root.cw);
    }
    (g.create_child_dc)(root.pixels, root.cw, root.ch, root.cw, offset.0, offset.1, &visible)
}

/// Shows the window's area (of its top-level window's buffer) on the
/// desktop.
fn present(hwnd: Hwnd) {
    let Some((root, _, visible)) = crate::tree::placement(hwnd) else { return };
    if root.desktop == 0 || !root.visible || IsWindowVisible(hwnd) == 0 {
        return;
    }
    let [l, t, r, b] = visible;
    if r <= l || b <= t {
        return;
    }
    // SAFETY: the top-level window's own buffer; the rectangle is inside.
    unsafe {
        GalaWindowPresent(
            root.desktop,
            l,
            t,
            (r - l) as u32,
            (b - t) as u32,
            root.pixels.add((t * root.cw + l) as usize),
            (root.cw * 4) as usize,
        )
    };
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
    if let Some(g) = gdi() {
        // SAFETY: the PAINTSTRUCT BeginPaint filled.
        (g.delete_dc)(unsafe { (*ps).hdc });
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
        None => core::ptr::null_mut(),
    }
}

/// ReleaseDC: what was drawn shows.
#[unsafe(no_mangle)]
pub extern "system" fn ReleaseDC(hwnd: Hwnd, hdc: Handle) -> i32 {
    if hwnd == 0 {
        return gdi().map_or(0, |g| (g.delete_dc)(hdc));
    }
    if window(hwnd).is_none() {
        return 0;
    }
    present(hwnd);
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
#[unsafe(no_mangle)]
pub extern "system" fn DefWindowProcW(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize {
    if let Some(r) = crate::tree::def_window_proc(hwnd, msg, wparam, lparam) {
        return r;
    }
    match msg {
        WM_NCCREATE => 1,
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
static CURSOR: AtomicUsize = AtomicUsize::new(0);

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

/// GetClipboardOwner: NULL (gala has no clipboard yet).
#[unsafe(no_mangle)]
pub extern "system" fn GetClipboardOwner() -> Hwnd {
    0
}

/// FindWindowW: a top-level window of ours with that class (NULL: any)
/// and title (NULL: any); gala's user32 sees only its own windows, so
/// another program's (e.g. Pageant's) is not found: NULL.
#[unsafe(no_mangle)]
pub extern "system" fn FindWindowW(class: *const u16, title: *const u16) -> Hwnd {
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
    have &= flags;
    have << 16 | have
}

/// SendMessageW: the window procedure, called now on this thread.
#[unsafe(no_mangle)]
pub extern "system" fn SendMessageW(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize {
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
        Some(Window { proc_: Some(p), .. }) => p(hwnd, msg, wparam, lparam),
        Some(_) => DefWindowProcW(hwnd, msg, wparam, lparam),
        None => 0,
    }
}

fn post(thread: u32, msg: Msg) -> bool {
    let ok = with(|s| match s.queue.iter_mut().find(|q| !q.used) {
        Some(q) => {
            *q = Queued { used: true, thread, msg };
            true
        }
        None => false,
    });
    if ok {
        wake();
    }
    ok
}

pub(crate) fn message(hwnd: Hwnd, message: u32, wparam: usize, lparam: isize) -> Msg {
    // SAFETY: a plain call.
    Msg { hwnd, message, wparam, lparam, time: unsafe { GetTickCount() }, pt: [0; 2], private: 0 }
}

/// PostMessageW: to the window's thread (a null window: this thread).
#[unsafe(no_mangle)]
pub extern "system" fn PostMessageW(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> i32 {
    let thread = if hwnd == 0 {
        me()
    } else {
        match window(hwnd) {
            Some(w) => w.thread,
            None => {
                // SAFETY: a plain call.
                unsafe { SetLastError(ERROR_INVALID_WINDOW_HANDLE) };
                return 0;
            }
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
                    let (sx, sy) = (w.x + BORDER + cx, w.y + BORDER + CAPTION + cy);
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
                if message == 0x0201 || message == 0x0204 {
                    FOREGROUND.store(top, Ordering::Relaxed);
                }
                // The desktop moved the window (a drag): its position.
                if message == 0x0003 {
                    crate::tree::moved_by_desktop(top, lparam);
                }
                // Mouse input goes to the child under the pointer (or the
                // capture), keys to the focus.
                let (to, lparam) = crate::tree::route(top, message, lparam);
                post(thread, message_at(to, message, wparam, lparam));
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

fn key_down(vk: usize) -> bool {
    KEYS.get(vk).is_some_and(|k| k.load(Ordering::Relaxed))
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
            let upper = shift != CAPS.load(Ordering::Relaxed);
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
    let toggled = vk == VK_CAPITAL && CAPS.load(Ordering::Relaxed);
    (if held { -0x8000i32 } else { 0 } | i32::from(toggled)) as i16
}

#[unsafe(no_mangle)]
pub extern "system" fn GetAsyncKeyState(vk: i32) -> i16 {
    GetKeyState(vk) & -0x8000i16
}

/// The calling thread's next message (matching the filter): posted ones
/// in order (the desktop's input first moved in), then WM_PAINT for an
/// invalid visible window. Removed if `remove`.
fn next(hwnd: Hwnd, min: u32, max: u32, remove: bool) -> Option<Msg> {
    let mut m = next_raw(hwnd, min, max, remove)?;
    // WH_GETMESSAGE sees (and may change) it: wParam PM_REMOVE or
    // PM_NOREMOVE.
    if crate::hook::any(crate::hook::WH_GETMESSAGE) {
        let lp = &mut m as *mut Msg as isize;
        crate::hook::call(crate::hook::WH_GETMESSAGE, 0, usize::from(remove), lp);
    }
    Some(m)
}

fn next_raw(hwnd: Hwnd, min: u32, max: u32, remove: bool) -> Option<Msg> {
    pump();
    let thread = me();
    let fits = |m: &Msg| {
        (hwnd == 0 || m.hwnd == hwnd || m.message == WM_QUIT)
            && ((min == 0 && max == 0) || (min..=max).contains(&m.message))
    };
    with(|s| {
        if let Some(q) =
            s.queue.iter_mut().filter(|q| q.used && q.thread == thread).find(|q| fits(&q.msg))
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

/// GetSystemMetrics: the desktop's frame (SM_CYCAPTION, SM_CXBORDER,
/// SM_CYBORDER) and the screen (crate::monitor).
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
        5 | 6 => BORDER,
        _ => 0,
    }
}

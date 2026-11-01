//! Windows, classes and message queues (the window manager's first
//! milestone). Top-level windows are the desktop's (the kernel draws their
//! frame and composites them: GalaWindow* services); their client area is
//! a pixel buffer here, which gdi32 draws into (GalaCreateSurfaceDC) and
//! EndPaint/ReleaseDC present. One message queue holds every thread's
//! posted messages (each tagged with its thread); GetMessage takes the
//! calling thread's, else makes WM_PAINT for one of its invalid windows,
//! else waits on the queue's event. SendMessage calls the window
//! procedure directly. Metrics: a 1-pixel border and a 24-pixel caption
//! (kernel/src/nt/desktop.rs).

use core::cell::UnsafeCell;
use core::ffi::c_void;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

type Handle = *mut c_void;
type Hwnd = usize;
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
pub const CAPTION: i32 = 24;
const MAX_CLASSES: usize = 32;
const MAX_WINDOWS: usize = 64;
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
struct Class {
    used: bool,
    atom: u16,
    name: [u16; 64],
    proc_: Option<WndProc>,
    background: usize,
    #[allow(dead_code)]
    style: u32,
}

#[derive(Clone, Copy)]
struct Window {
    used: bool,
    class: usize,
    proc_: Option<WndProc>,
    /// The desktop's id (0: no screen; nothing is shown).
    desktop: u32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    #[allow(dead_code)]
    style: u32,
    visible: bool,
    invalid: bool,
    erase: bool,
    thread: u32,
    pixels: *mut u32,
    cw: i32,
    ch: i32,
    user_data: isize,
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

const NO_CLASS: Class =
    Class { used: false, atom: 0, name: [0; 64], proc_: None, background: 0, style: 0 };
const NO_WINDOW: Window = Window {
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
};
const NO_MSG: Msg =
    Msg { hwnd: 0, message: 0, wparam: 0, lparam: 0, time: 0, pt: [0; 2], private: 0 };

struct State {
    classes: [Class; MAX_CLASSES],
    windows: [Window; MAX_WINDOWS],
    queue: [Queued; QUEUE],
    next_atom: u16,
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
        next_atom: 0xc000,
    }),
};

/// The queue's event (auto-reset), made on first use.
static EVENT: AtomicUsize = AtomicUsize::new(0);

fn with<R>(f: impl FnOnce(&mut State) -> R) -> R {
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
        Ok(_) => new as Handle,
        Err(other) => other as Handle,
    }
}

fn wake() {
    // SAFETY: our event.
    unsafe { SetEvent(event()) };
}

fn me() -> u32 {
    // SAFETY: a plain call.
    unsafe { GetCurrentThreadId() }
}

fn index_of(hwnd: Hwnd) -> Option<usize> {
    let i = hwnd.checked_sub(HWND_BASE)? / 0x10;
    (hwnd == HWND_BASE + i * 0x10 && i < MAX_WINDOWS).then_some(i)
}

fn hwnd_of(i: usize) -> Hwnd {
    HWND_BASE + i * 0x10
}

/// The window's state, if `hwnd` is one.
fn window(hwnd: Hwnd) -> Option<Window> {
    let i = index_of(hwnd)?;
    with(|s| s.windows[i].used.then_some(s.windows[i]))
}

fn unsupported(what: &[u8], detail: u64) {
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
}

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
        if [a, b, c, d, e].iter().any(|p| p.is_null()) {
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

/// A class by name (or atom: a value under 0x10000).
fn find_class(s: &State, name: *const u16) -> Option<usize> {
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
        let atom = s.next_atom;
        s.next_atom += 1;
        s.classes[i] = Class {
            used: true,
            atom,
            name: class_key(wc.class_name),
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
        unsupported(b"user32.dll!CreateWindowExW(WS_CHILD)", u64::from(style));
        return 0;
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
    SendMessageW(hwnd, WM_DESTROY, 0, 0);
    SendMessageW(hwnd, WM_NCDESTROY, 0, 0);
    let Some(i) = index_of(hwnd) else { return 0 };
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
        VirtualFree(gone.pixels.cast(), 0, 0x8000);
    }
    1
}

#[unsafe(no_mangle)]
pub extern "system" fn IsWindow(hwnd: Hwnd) -> i32 {
    i32::from(window(hwnd).is_some())
}

#[unsafe(no_mangle)]
pub extern "system" fn IsWindowVisible(hwnd: Hwnd) -> i32 {
    i32::from(window(hwnd).is_some_and(|w| w.visible))
}

/// ShowWindow: whether it was visible before.
#[unsafe(no_mangle)]
pub extern "system" fn ShowWindow(hwnd: Hwnd, cmd: i32) -> i32 {
    let Some(i) = index_of(hwnd) else { return 0 };
    let show = cmd != 0; // SW_HIDE
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
        wake();
    }
    i32::from(was)
}

/// UpdateWindow: sends WM_PAINT now if the window needs painting.
#[unsafe(no_mangle)]
pub extern "system" fn UpdateWindow(hwnd: Hwnd) -> i32 {
    let Some(w) = window(hwnd) else { return 0 };
    if w.invalid && w.visible {
        SendMessageW(hwnd, WM_PAINT, 0, 0);
    }
    1
}

/// InvalidateRect(hwnd, rect, erase): the whole client area needs
/// painting (the rectangle is not tracked yet).
#[unsafe(no_mangle)]
pub extern "system" fn InvalidateRect(hwnd: Hwnd, _rect: *const [i32; 4], erase: i32) -> i32 {
    let Some(i) = index_of(hwnd) else { return 0 };
    let ok = with(|s| {
        let w = &mut s.windows[i];
        w.invalid |= w.used;
        w.erase |= w.used && erase != 0;
        w.used
    });
    wake();
    i32::from(ok)
}

#[unsafe(no_mangle)]
pub extern "system" fn ValidateRect(hwnd: Hwnd, _rect: *const [i32; 4]) -> i32 {
    let Some(i) = index_of(hwnd) else { return 0 };
    with(|s| {
        s.windows[i].invalid = false;
        i32::from(s.windows[i].used)
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn GetClientRect(hwnd: Hwnd, rect: *mut [i32; 4]) -> i32 {
    let Some(w) = window(hwnd) else { return 0 };
    // SAFETY: the caller's RECT.
    unsafe { *rect = [0, 0, w.cw, w.ch] };
    1
}

#[unsafe(no_mangle)]
pub extern "system" fn GetWindowRect(hwnd: Hwnd, rect: *mut [i32; 4]) -> i32 {
    let Some(w) = window(hwnd) else { return 0 };
    // SAFETY: the caller's RECT.
    unsafe { *rect = [w.x, w.y, w.x + w.w, w.y + w.h] };
    1
}

const GWLP_WNDPROC: i32 = -4;
const GWLP_USERDATA: i32 = -21;

#[unsafe(no_mangle)]
pub extern "system" fn GetWindowLongPtrW(hwnd: Hwnd, index: i32) -> isize {
    let Some(w) = window(hwnd) else { return 0 };
    match index {
        GWLP_WNDPROC => w.proc_.map_or(0, |p| p as usize as isize),
        GWLP_USERDATA => w.user_data,
        _ => {
            unsupported(b"user32.dll!GetWindowLongPtrW(index)", index as u64);
            0
        }
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn SetWindowLongPtrW(hwnd: Hwnd, index: i32, value: isize) -> isize {
    let Some(i) = index_of(hwnd) else { return 0 };
    with(|s| {
        let w = &mut s.windows[i];
        if !w.used {
            return 0;
        }
        match index {
            GWLP_WNDPROC => {
                let old = w.proc_.map_or(0, |p| p as usize as isize);
                // SAFETY: the caller's window procedure.
                w.proc_ =
                    (value != 0).then(|| unsafe { core::mem::transmute::<isize, WndProc>(value) });
                old
            }
            GWLP_USERDATA => core::mem::replace(&mut w.user_data, value),
            _ => 0,
        }
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

/// A DC on the window's client area.
fn client_dc(w: &Window) -> Handle {
    match gdi() {
        Some(g) => (g.create_dc)(w.pixels, w.cw, w.ch, w.cw),
        None => core::ptr::null_mut(),
    }
}

/// Shows the client area's pixels on the desktop.
fn present(w: &Window) {
    if w.desktop != 0 && w.visible {
        // SAFETY: the window's own buffer.
        unsafe {
            GalaWindowPresent(
                w.desktop,
                0,
                0,
                w.cw as u32,
                w.ch as u32,
                w.pixels,
                (w.cw * 4) as usize,
            )
        };
    }
}

/// BeginPaint: a DC on the client area, the area to paint (all of it),
/// WM_ERASEBKGND first if the background needs erasing; the window is
/// valid again.
#[unsafe(no_mangle)]
pub extern "system" fn BeginPaint(hwnd: Hwnd, ps: *mut PaintStruct) -> Handle {
    let Some(i) = index_of(hwnd) else { return core::ptr::null_mut() };
    let Some(w) = window(hwnd) else { return core::ptr::null_mut() };
    let hdc = client_dc(&w);
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
    let Some(w) = window(hwnd) else { return 0 };
    present(&w);
    if let Some(g) = gdi() {
        // SAFETY: the PAINTSTRUCT BeginPaint filled.
        (g.delete_dc)(unsafe { (*ps).hdc });
    }
    1
}

#[unsafe(no_mangle)]
pub extern "system" fn GetDC(hwnd: Hwnd) -> Handle {
    match window(hwnd) {
        Some(w) => client_dc(&w),
        None => core::ptr::null_mut(),
    }
}

/// ReleaseDC: what was drawn shows.
#[unsafe(no_mangle)]
pub extern "system" fn ReleaseDC(hwnd: Hwnd, hdc: Handle) -> i32 {
    let Some(w) = window(hwnd) else { return 0 };
    present(&w);
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
pub extern "system" fn DefWindowProcW(
    hwnd: Hwnd,
    msg: u32,
    wparam: usize,
    _lparam: isize,
) -> isize {
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

/// SendMessageW: the window procedure, called now on this thread.
#[unsafe(no_mangle)]
pub extern "system" fn SendMessageW(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize {
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

fn message(hwnd: Hwnd, message: u32, wparam: usize, lparam: isize) -> Msg {
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

/// The calling thread's next message (matching the filter): posted ones
/// in order, then WM_PAINT for an invalid visible window. Removed if
/// `remove`.
fn next(hwnd: Hwnd, min: u32, max: u32, remove: bool) -> Option<Msg> {
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
        let i = s
            .windows
            .iter()
            .position(|w| w.used && w.thread == thread && w.invalid && w.visible)?;
        let m = message(hwnd_of(i), WM_PAINT, 0, 0);
        fits(&m).then_some(m)
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn GetMessageW(msg: *mut Msg, hwnd: Hwnd, min: u32, max: u32) -> i32 {
    loop {
        if let Some(m) = next(hwnd, min, max, true) {
            // SAFETY: the caller's MSG.
            unsafe { *msg = m };
            return i32::from(m.message != WM_QUIT);
        }
        // SAFETY: our event.
        unsafe { WaitForSingleObject(event(), 0xffff_ffff) };
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
pub extern "system" fn LoadCursorW(_instance: Handle, _name: *const u16) -> Handle {
    0x0001_0003 as Handle
}

#[unsafe(no_mangle)]
pub extern "system" fn LoadIconW(_instance: Handle, _name: *const u16) -> Handle {
    0x0001_0005 as Handle
}

#[unsafe(no_mangle)]
pub extern "system" fn SetCursor(_cursor: Handle) -> Handle {
    0x0001_0003 as Handle
}

/// GetSystemMetrics: the desktop's frame (SM_CYCAPTION, SM_CXBORDER,
/// SM_CYBORDER) and an assumed 1280 x 800 screen.
#[unsafe(no_mangle)]
pub extern "system" fn GetSystemMetrics(index: i32) -> i32 {
    match index {
        0 => 1280, // SM_CXSCREEN
        1 => 800,  // SM_CYSCREEN
        4 => CAPTION,
        5 | 6 => BORDER,
        _ => 0,
    }
}

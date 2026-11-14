//! What Chromium's chrome.dll delay-imports from shell32 (resolved with
//! GetProcAddress, which does not return `gala_stubs` stubs), as
//! documented on learn.microsoft.com:
//!
//! - ILCreateFromPathW (#190) makes the item ID list of an existing path
//!   (NULL for one that is not there); ILFree (#155) frees a list (gala's
//!   come from the process heap); ILGetSize (#152) is a list's size with
//!   its terminator (0 for NULL); IsUserAnAdmin (#680) is the token's
//!   membership in BUILTIN\Administrators (CheckTokenMembership). The
//!   ordinals are those of Wine's shell32.spec (build.rs).
//! - SHChangeNotifyRegister (#2) / SHChangeNotifyDeregister (#4): gala's
//!   shell sends no change notifications (SHChangeNotify reaches nobody),
//!   so a registration gets an ID and is never notified (reported).
//! - SHQueryUserNotificationState: QUNS_ACCEPTS_NOTIFICATIONS (nothing
//!   full-screen, no presentation mode).
//! - SHAppBarMessage: the taskbar is shasha's, at the bottom of the screen
//!   below the work area (none without a screen: ABM_GETTASKBARPOS fails), always on top, not auto-hidden; app bars of
//!   one's own (ABM_NEW, ABM_SETPOS, ...) are not supported (reported, 0).
//! - ShellExecuteA: ShellExecuteW with the strings converted (CP_ACP).
//! - SHOpenFolderAndSelectItems, SHGetPropertyStoreForWindow,
//!   SHCreateAssociationRegistration, SHGetStockIconInfo and
//!   SHOpenWithDialog are not there yet: reported, E_NOTIMPL with the
//!   output NULL. (The namespace itself is `namespace.rs`.)

use core::ffi::c_void;

use super::{E_INVALIDARG, S_OK, unsupported};

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GetProcessHeap() -> usize;
    fn HeapFree(heap: usize, flags: u32, p: *mut u8) -> i32;
    fn MultiByteToWideChar(cp: u32, flags: u32, s: *const u8, n: i32, w: *mut u16, m: i32) -> i32;
    fn LoadLibraryW(name: *const u16) -> usize;
    fn GetFileAttributesW(path: *const u16) -> u32;
    fn GetProcAddress(module: usize, name: *const u8) -> *const c_void;
}

const E_NOTIMPL: i32 = 0x8000_4001u32 as i32;

/// The size of an item ID list (each item's cb, then the 2-byte
/// terminator); 0 for NULL.
#[unsafe(no_mangle)]
pub extern "system" fn ILGetSize(list: *const u8) -> u32 {
    if list.is_null() {
        return 0;
    }
    let mut at = 0usize;
    loop {
        // SAFETY: the caller's list: items of `cb` bytes up to a zero cb.
        let cb = unsafe { list.add(at).cast::<u16>().read_unaligned() };
        if cb == 0 {
            return at as u32 + 2;
        }
        at += usize::from(cb);
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn ILFree(list: *mut u8) {
    if !list.is_null() {
        // SAFETY: a list gala's shell32 allocated from the process heap.
        unsafe { HeapFree(GetProcessHeap(), 0, list) };
    }
}

/// ILCreateFromPathW(path): the list for an existing file or directory.
#[unsafe(no_mangle)]
pub extern "system" fn ILCreateFromPathW(path: *const u16) -> *mut u8 {
    if path.is_null() {
        return core::ptr::null_mut();
    }
    // SAFETY: the caller's NUL-terminated path.
    let path = unsafe {
        let mut n = 0;
        while *path.add(n) != 0 {
            n += 1;
        }
        core::slice::from_raw_parts(path, n)
    };
    let mut z = [0u16; super::MAX_PATH + 1];
    if path.is_empty() || path.len() > super::MAX_PATH {
        return core::ptr::null_mut();
    }
    z[..path.len()].copy_from_slice(path);
    // SAFETY: a NUL-terminated path.
    if unsafe { GetFileAttributesW(z.as_ptr()) } == u32::MAX {
        return core::ptr::null_mut();
    }
    super::namespace::list_from_path(path)
}

static NEXT_REGISTRATION: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(1);

/// SHChangeNotifyRegister(window, sources, events, message, count,
/// entries): a registration ID (never 0); no notification follows.
#[unsafe(no_mangle)]
pub extern "system" fn SHChangeNotifyRegister(
    window: usize,
    _sources: i32,
    _events: i32,
    _message: u32,
    _count: i32,
    _entries: *const c_void,
) -> u32 {
    if window == 0 {
        return 0;
    }
    unsupported(b"shell32.dll!SHChangeNotifyRegister(never notified)", 0);
    NEXT_REGISTRATION.fetch_add(1, core::sync::atomic::Ordering::Relaxed)
}

#[unsafe(no_mangle)]
pub extern "system" fn SHChangeNotifyDeregister(id: u32) -> i32 {
    i32::from(id != 0 && id < NEXT_REGISTRATION.load(core::sync::atomic::Ordering::Relaxed))
}

/// BUILTIN\Administrators: S-1-5-32-544.
const ADMINISTRATORS: [u8; 16] = [1, 2, 0, 0, 0, 0, 0, 5, 32, 0, 0, 0, 0x20, 2, 0, 0];

/// IsUserAnAdmin: advapi32's CheckTokenMembership, loaded when first
/// needed (a static import would make every program that loads shell32
/// load advapi32).
#[unsafe(no_mangle)]
pub extern "system" fn IsUserAnAdmin() -> i32 {
    const ADVAPI32: [u16; 13] = [97, 100, 118, 97, 112, 105, 51, 50, 46, 100, 108, 108, 0];
    let mut member = 0;
    // SAFETY: NUL-terminated names; CheckTokenMembership(HANDLE, PSID,
    // PBOOL) with a valid SID and our output.
    let ok = unsafe {
        let module = LoadLibraryW(ADVAPI32.as_ptr());
        let f = if module == 0 {
            core::ptr::null()
        } else {
            GetProcAddress(module, c"CheckTokenMembership".as_ptr().cast())
        };
        if f.is_null() {
            return 0;
        }
        let f: extern "system" fn(usize, *const u8, *mut i32) -> i32 = core::mem::transmute(f);
        f(0, ADMINISTRATORS.as_ptr(), &mut member)
    };
    i32::from(ok != 0 && member != 0)
}

const QUNS_ACCEPTS_NOTIFICATIONS: i32 = 5;

#[unsafe(no_mangle)]
pub extern "system" fn SHQueryUserNotificationState(state: *mut i32) -> i32 {
    if state.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's output.
    unsafe { state.write(QUNS_ACCEPTS_NOTIFICATIONS) };
    S_OK
}

/// A user32 function, loaded when first needed (a static import would
/// make every program that loads shell32 load user32).
fn user32(name: &core::ffi::CStr) -> Option<*const c_void> {
    const USER32: [u16; 11] = [117, 115, 101, 114, 51, 50, 46, 100, 108, 108, 0];
    // SAFETY: NUL-terminated names.
    unsafe {
        let module = LoadLibraryW(USER32.as_ptr());
        if module == 0 {
            return None;
        }
        let f = GetProcAddress(module, name.as_ptr().cast());
        (!f.is_null()).then_some(f)
    }
}

/// APPBARDATA (x64): cbSize, hWnd, uCallbackMessage, uEdge, rc, lParam.
#[repr(C)]
pub struct AppBarData {
    size: u32,
    window: usize,
    message: u32,
    edge: u32,
    rect: [i32; 4],
    param: isize,
}

const ABM_GETSTATE: u32 = 4;
const ABM_GETTASKBARPOS: u32 = 5;
const ABM_GETAUTOHIDEBAR: u32 = 7;
const ABM_GETAUTOHIDEBAREX: u32 = 0xb;
const ABS_ALWAYSONTOP: usize = 2;
const ABE_BOTTOM: u32 = 3;
const SPI_GETWORKAREA: u32 = 0x30;

#[unsafe(no_mangle)]
pub extern "system" fn SHAppBarMessage(message: u32, data: *mut AppBarData) -> usize {
    match message {
        ABM_GETSTATE => ABS_ALWAYSONTOP,
        // No auto-hidden bar on any edge.
        ABM_GETAUTOHIDEBAR | ABM_GETAUTOHIDEBAREX => 0,
        ABM_GETTASKBARPOS => {
            if data.is_null() {
                return 0;
            }
            let mut work = [0i32; 4];
            let (Some(spi), Some(metrics)) =
                (user32(c"SystemParametersInfoW"), user32(c"GetSystemMetrics"))
            else {
                return 0;
            };
            // SAFETY: user32's SystemParametersInfoW(UINT, UINT, PVOID, UINT)
            // and GetSystemMetrics(int); our RECT; SM_CXSCREEN, SM_CYSCREEN.
            let (ok, w, h) = unsafe {
                let spi: extern "system" fn(u32, u32, *mut c_void, u32) -> i32 =
                    core::mem::transmute(spi);
                let metrics: extern "system" fn(i32) -> i32 = core::mem::transmute(metrics);
                (spi(SPI_GETWORKAREA, 0, work.as_mut_ptr().cast(), 0), metrics(0), metrics(1))
            };
            // No screen, no taskbar: the work area is the whole screen.
            if ok == 0 || work[3] >= h {
                return 0;
            }
            // SAFETY: the caller's APPBARDATA.
            unsafe {
                (*data).edge = ABE_BOTTOM;
                (*data).rect = [0, work[3], w, h];
            }
            1
        }
        m => {
            unsupported(b"shell32.dll!SHAppBarMessage(an app bar of its own)", u64::from(m));
            0
        }
    }
}

/// A NUL-terminated ANSI string as UTF-16 in `buf` (None for NULL).
fn wide(s: *const u8, buf: &mut [u16; 1024]) -> Option<*const u16> {
    if s.is_null() {
        return None;
    }
    // SAFETY: the caller's NUL-terminated string (-1: with its NUL); our
    // buffer, its last unit kept for a NUL.
    let n = unsafe { MultiByteToWideChar(0, 0, s, -1, buf.as_mut_ptr(), 1023) };
    if n == 0 {
        buf[0] = 0;
    }
    buf[1023] = 0;
    Some(buf.as_ptr())
}

#[unsafe(no_mangle)]
pub extern "system" fn ShellExecuteA(
    window: usize,
    verb: *const u8,
    file: *const u8,
    params: *const u8,
    dir: *const u8,
    show: i32,
) -> usize {
    let (mut v, mut f, mut p, mut d) = ([0u16; 1024], [0u16; 1024], [0u16; 1024], [0u16; 1024]);
    let none = core::ptr::null();
    super::shellops::ShellExecuteW(
        window,
        wide(verb, &mut v).unwrap_or(none),
        wide(file, &mut f).unwrap_or(none),
        wide(params, &mut p).unwrap_or(none),
        wide(dir, &mut d).unwrap_or(none),
        show,
    )
}

/// Reports `what` and clears the output: E_NOTIMPL.
fn no_namespace(what: &[u8], out: *mut *mut c_void) -> i32 {
    unsupported(what, 0);
    if !out.is_null() {
        // SAFETY: the caller's output pointer.
        unsafe { out.write(core::ptr::null_mut()) };
    }
    E_NOTIMPL
}

#[unsafe(no_mangle)]
pub extern "system" fn SHOpenFolderAndSelectItems(
    _folder: *const u8,
    _count: u32,
    _items: *const *const u8,
    _flags: u32,
) -> i32 {
    no_namespace(b"shell32.dll!SHOpenFolderAndSelectItems", core::ptr::null_mut())
}

#[unsafe(no_mangle)]
pub extern "system" fn SHGetPropertyStoreForWindow(
    _window: usize,
    _iid: *const [u8; 16],
    out: *mut *mut c_void,
) -> i32 {
    no_namespace(b"shell32.dll!SHGetPropertyStoreForWindow", out)
}

#[unsafe(no_mangle)]
pub extern "system" fn SHCreateAssociationRegistration(
    _iid: *const [u8; 16],
    out: *mut *mut c_void,
) -> i32 {
    no_namespace(b"shell32.dll!SHCreateAssociationRegistration", out)
}

#[unsafe(no_mangle)]
pub extern "system" fn SHGetStockIconInfo(_id: u32, _flags: u32, _info: *mut c_void) -> i32 {
    no_namespace(b"shell32.dll!SHGetStockIconInfo", core::ptr::null_mut())
}

#[unsafe(no_mangle)]
pub extern "system" fn SHOpenWithDialog(_window: usize, _info: *const c_void) -> i32 {
    no_namespace(b"shell32.dll!SHOpenWithDialog", core::ptr::null_mut())
}

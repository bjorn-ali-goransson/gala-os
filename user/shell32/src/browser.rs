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
//! - SHAppBarMessage: the taskbar is the desktop's app bar on the bottom
//!   edge, the whole width (ABM_GETTASKBARPOS gives its edge and
//!   rectangle, GalaAppBar; none without a screen: it fails), always on
//!   top, not auto-hidden. A program's own bars (ABM_NEW, ABM_REMOVE, ABM_QUERYPOS,
//!   ABM_SETPOS) are the desktop's (GalaAppBar: the work area gives up
//!   their strips; ARCHITECTURE "App bars"); ABM_ACTIVATE and
//!   ABM_WINDOWPOSCHANGED return TRUE, as documented, with nothing to do
//!   (no auto-hidden bars); auto-hide (ABM_SETAUTOHIDEBAR) is reported,
//!   FALSE.
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

const ABM_NEW: u32 = 0;
const ABM_SETPOS: u32 = 3;
const ABM_ACTIVATE: u32 = 6;
const ABM_WINDOWPOSCHANGED: u32 = 9;
const ABM_GETSTATE: u32 = 4;
const ABM_GETTASKBARPOS: u32 = 5;
const ABM_GETAUTOHIDEBAR: u32 = 7;
const ABM_GETAUTOHIDEBAREX: u32 = 0xb;
const ABS_ALWAYSONTOP: usize = 2;

#[unsafe(no_mangle)]
pub extern "system" fn SHAppBarMessage(message: u32, data: *mut AppBarData) -> usize {
    match message {
        ABM_NEW..=ABM_SETPOS => own_bar(message, data),
        ABM_ACTIVATE | ABM_WINDOWPOSCHANGED => 1,
        ABM_GETSTATE => ABS_ALWAYSONTOP,
        // No auto-hidden bar on any edge.
        ABM_GETAUTOHIDEBAR | ABM_GETAUTOHIDEBAREX => 0,
        ABM_GETTASKBARPOS => own_bar(message, data),
        m => {
            unsupported(b"shell32.dll!SHAppBarMessage(an app bar of its own)", u64::from(m));
            0
        }
    }
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "ntdll", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "ntdll", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GalaAppBar(op: u32, request: *mut u8, length: usize) -> u32;
}

/// ABM_NEW, ABM_REMOVE, ABM_QUERYPOS, ABM_SETPOS and ABM_GETTASKBARPOS:
/// GalaAppBar's request (the window, the callback message, the edge, the
/// rectangle and the result after it), the rectangle given back for
/// QUERYPOS and SETPOS, the taskbar's edge and rectangle for
/// GETTASKBARPOS.
fn own_bar(message: u32, data: *mut AppBarData) -> usize {
    if data.is_null() {
        return 0;
    }
    // SAFETY: the caller's APPBARDATA.
    let d = unsafe { &mut *data };
    let mut r = [0u8; 36];
    r[0..8].copy_from_slice(&(d.window as u64).to_le_bytes());
    r[8..12].copy_from_slice(&d.message.to_le_bytes());
    r[12..16].copy_from_slice(&d.edge.to_le_bytes());
    for (i, v) in d.rect.iter().enumerate() {
        r[16 + 4 * i..20 + 4 * i].copy_from_slice(&v.to_le_bytes());
    }
    // SAFETY: our request of its length.
    let status = unsafe { GalaAppBar(message, r.as_mut_ptr(), r.len()) };
    if status != 0 {
        // No screen, no taskbar: GETTASKBARPOS fails quietly, as it does
        // on Windows without an explorer.
        if message != ABM_GETTASKBARPOS {
            unsupported(
                b"shell32.dll!SHAppBarMessage(no desktop to take a strip from)",
                status.into(),
            );
        }
        return 0;
    }
    let at = |o: usize| i32::from_le_bytes([r[o], r[o + 1], r[o + 2], r[o + 3]]);
    if message >= 2 {
        d.rect = [at(16), at(20), at(24), at(28)];
    }
    if message == ABM_GETTASKBARPOS {
        d.edge = at(12) as u32;
    }
    at(32) as usize
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

//! gdi32's side of OpenGL: pixel formats and SwapBuffers. gdi32 has no
//! OpenGL of its own; these go to the WGL functions of the process's
//! `opengl32.dll` (Mesa's, which gala ships as the system's: ARCHITECTURE
//! "Plan: graphics and GPU"), loading it for the calls that choose,
//! describe or set a format, as an application that calls them first
//! expects OpenGL to be there. The ICD keeps each window's format;
//! gdi32 keeps Windows' rule that a window's format is set once:
//! SetPixelFormat of the format a window already has succeeds and of
//! another fails, without reaching the ICD. A format is only ever set
//! through opengl32, so GetPixelFormat and SwapBuffers do not load it:
//! without it, no format (0) and nothing to swap (FALSE).
//!
//! Without an opengl32 (gala's image ships none unless Mesa is added),
//! each call fails as on a machine with no OpenGL (0 / FALSE,
//! ERROR_NOT_SUPPORTED) and is in the ledger as
//! `gdi32.dll!<name>(no opengl32)`.

use super::unsupported;
use core::ffi::c_void;

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GetModuleHandleW(name: *const u16) -> *mut c_void;
    fn LoadLibraryW(name: *const u16) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> *const c_void;
    fn SetLastError(code: u32);
}

const ERROR_NOT_SUPPORTED: u32 = 50;

type Hdc = *mut c_void;

/// "opengl32.dll", NUL-terminated UTF-16.
const OPENGL32: [u16; 13] = {
    let name = b"opengl32.dll";
    let mut out = [0u16; 13];
    let mut i = 0;
    while i < name.len() {
        out[i] = name[i] as u16;
        i += 1;
    }
    out
};

/// opengl32's export `name` (NUL-terminated), loading the DLL if `load`;
/// None (in the ledger as `caller(no opengl32)` when it was to be
/// loaded) if there is none.
fn wgl(name: &[u8], load: bool, caller: &str) -> Option<*const c_void> {
    // SAFETY: plain calls with NUL-terminated names.
    let module = unsafe {
        let m = GetModuleHandleW(OPENGL32.as_ptr());
        if m.is_null() && load { LoadLibraryW(OPENGL32.as_ptr()) } else { m }
    };
    // SAFETY: as above.
    let f = if module.is_null() {
        core::ptr::null()
    } else {
        unsafe { GetProcAddress(module, name.as_ptr()) }
    };
    if f.is_null() {
        if load {
            let mut what = [0u8; 64];
            let mut n = 0;
            for part in [b"gdi32.dll!".as_slice(), caller.as_bytes(), b"(no opengl32)"] {
                for &b in part {
                    if n < what.len() {
                        what[n] = b;
                        n += 1;
                    }
                }
            }
            unsupported(&what[..n], 0);
        }
        // SAFETY: a plain call.
        unsafe { SetLastError(ERROR_NOT_SUPPORTED) };
        return None;
    }
    Some(f)
}

/// ChoosePixelFormat(hdc, ppfd): opengl32's closest format (1-based), 0
/// for none.
#[unsafe(no_mangle)]
pub extern "system" fn ChoosePixelFormat(hdc: Hdc, pfd: *const c_void) -> i32 {
    let Some(f) = wgl(b"wglChoosePixelFormat\0", true, "ChoosePixelFormat") else { return 0 };
    // SAFETY: opengl32's export, with WGL's signature.
    let f: extern "system" fn(Hdc, *const c_void) -> i32 = unsafe { core::mem::transmute(f) };
    f(hdc, pfd)
}

/// DescribePixelFormat(hdc, format, size, ppfd): the number of formats,
/// the descriptor of `format` filled when `ppfd` is given.
#[unsafe(no_mangle)]
pub extern "system" fn DescribePixelFormat(
    hdc: Hdc,
    format: i32,
    size: u32,
    pfd: *mut c_void,
) -> i32 {
    let Some(f) = wgl(b"wglDescribePixelFormat\0", true, "DescribePixelFormat") else { return 0 };
    // SAFETY: opengl32's export, with WGL's signature.
    let f: extern "system" fn(Hdc, i32, u32, *mut c_void) -> i32 =
        unsafe { core::mem::transmute(f) };
    f(hdc, format, size, pfd)
}

/// GetPixelFormat(hdc): the window's format, 0 if none was set.
#[unsafe(no_mangle)]
pub extern "system" fn GetPixelFormat(hdc: Hdc) -> i32 {
    let Some(f) = wgl(b"wglGetPixelFormat\0", false, "GetPixelFormat") else { return 0 };
    // SAFETY: opengl32's export, with WGL's signature.
    let f: extern "system" fn(Hdc) -> i32 = unsafe { core::mem::transmute(f) };
    f(hdc)
}

/// SetPixelFormat(hdc, format, ppfd): once per window; the same format
/// again succeeds, another fails.
#[unsafe(no_mangle)]
pub extern "system" fn SetPixelFormat(hdc: Hdc, format: i32, pfd: *const c_void) -> i32 {
    let Some(f) = wgl(b"wglSetPixelFormat\0", true, "SetPixelFormat") else { return 0 };
    let current = GetPixelFormat(hdc);
    if current != 0 {
        return i32::from(current == format);
    }
    // SAFETY: opengl32's export, with WGL's signature.
    let f: extern "system" fn(Hdc, i32, *const c_void) -> i32 = unsafe { core::mem::transmute(f) };
    f(hdc, format, pfd)
}

/// SwapBuffers(hdc): the context's back buffer to the window (the ICD
/// draws it into the DC; user32 shows what a window's DC drew).
#[unsafe(no_mangle)]
pub extern "system" fn SwapBuffers(hdc: Hdc) -> i32 {
    let Some(f) = wgl(b"wglSwapBuffers\0", false, "SwapBuffers") else {
        unsupported(b"gdi32.dll!SwapBuffers(no opengl32)", 0);
        return 0;
    };
    // SAFETY: opengl32's export, with WGL's signature.
    let f: extern "system" fn(Hdc) -> i32 = unsafe { core::mem::transmute(f) };
    f(hdc)
}

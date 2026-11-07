//! gala's UI Automation message (0x03f3, in the system range): another
//! process's UIAutomationCore asking for a property of the provider one
//! of our windows gave it (its WM_GETOBJECT answer) or of its fragments.
//! wParam: the operation (UIAutomationCore's providers.rs); lParam: a
//! UIA_VALUE_SIZE buffer (marshaled by `foreign`: back for a value, in
//! with GALA_UIA_INPUT) or 0.
//! user32 answers it itself, never the window procedure, by handing it to
//! this process's UIAutomationCore (GalaUiaServe), which is loaded: it
//! made the cookie.

use core::ffi::c_void;
use core::sync::atomic::{AtomicUsize, Ordering};

use crate::window::Hwnd;

pub(crate) const GALA_UIA: u32 = 0x03f3;
/// The value buffer's size (UIAutomationCore's providers.rs agrees).
pub(crate) const UIA_VALUE_SIZE: usize = 4096;
/// wParam's flag for an operation whose buffer goes in (SetValue's
/// text), not out.
pub(crate) const GALA_UIA_INPUT: usize = 1 << 31;

type Handle = *mut c_void;

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GetModuleHandleW(name: *const u16) -> Handle;
    fn GetProcAddress(module: Handle, name: *const u8) -> *const c_void;
}

type Serve = extern "system" fn(Hwnd, usize, isize) -> isize;
/// GalaUiaServe's address once found (0 before).
static SERVE: AtomicUsize = AtomicUsize::new(0);

/// GALA_UIA for our window `hwnd`: what UIAutomationCore's GalaUiaServe
/// gave (0 when there is no UIAutomationCore in this process).
pub(crate) fn uia_message(hwnd: Hwnd, wparam: usize, lparam: isize) -> isize {
    let mut f = SERVE.load(Ordering::Acquire);
    if f == 0 {
        let mut name = [0u16; 21];
        for (d, b) in name.iter_mut().zip(b"UIAutomationCore.dll") {
            *d = u16::from(*b);
        }
        // SAFETY: a NUL-terminated name; a loaded module's export.
        f = unsafe {
            let m = GetModuleHandleW(name.as_ptr());
            if m.is_null() {
                0
            } else {
                GetProcAddress(m, c"GalaUiaServe".as_ptr().cast()) as usize
            }
        };
        if f == 0 {
            crate::window::unsupported(b"user32.dll!GALA_UIA(no UIAutomationCore)", 0);
            return 0;
        }
        SERVE.store(f, Ordering::Release);
    }
    // SAFETY: GalaUiaServe's address, with the signature it defines.
    let serve: Serve = unsafe { core::mem::transmute(f) };
    serve(hwnd, wparam, lparam)
}

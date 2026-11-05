//! oleacc's functions with no implementation yet, as real exports
//! (Chromium's chrome.dll delay-imports them and resolves them with
//! GetProcAddress, which does not return `gala_stubs` stubs). There is no
//! default IAccessible (MSAA proxy) yet: each reports itself to the
//! ledger ("unsupported") and fails as documented for a failure:
//! E_NOTIMPL with the output NULL for the HRESULT functions,
//! LresultFromObject's negative HRESULT (E_NOTIMPL), and NULL for
//! WindowFromAccessibleObject's window. Programs fall back (UI
//! Automation, uiautomationcore, is the accessibility gala has).

use core::ffi::c_void;

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn GalaRecordMissing(name: *const u8, length: usize, what: u32, detail: u64) -> u32;
}

const E_NOTIMPL: i32 = 0x8000_4001_u32 as i32;

fn absent(what: &str) {
    // SAFETY: a valid buffer; "unsupported" (2).
    unsafe { GalaRecordMissing(what.as_ptr(), what.len(), 2, 0) };
}

fn clear(out: *mut *mut c_void) {
    if !out.is_null() {
        // SAFETY: the caller's output pointer.
        unsafe { out.write(core::ptr::null_mut()) };
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn CreateStdAccessibleObject(
    _window: usize,
    _object: i32,
    _iid: *const [u8; 16],
    out: *mut *mut c_void,
) -> i32 {
    absent("oleacc.dll!CreateStdAccessibleObject");
    clear(out);
    E_NOTIMPL
}

#[unsafe(no_mangle)]
pub extern "system" fn LresultFromObject(
    _iid: *const [u8; 16],
    _wparam: usize,
    _object: *mut c_void,
) -> isize {
    absent("oleacc.dll!LresultFromObject");
    E_NOTIMPL as isize
}

#[unsafe(no_mangle)]
pub extern "system" fn AccessibleObjectFromWindow(
    _window: usize,
    _object: u32,
    _iid: *const [u8; 16],
    out: *mut *mut c_void,
) -> i32 {
    absent("oleacc.dll!AccessibleObjectFromWindow");
    clear(out);
    E_NOTIMPL
}

#[unsafe(no_mangle)]
pub extern "system" fn AccessibleChildren(
    _container: *mut c_void,
    _start: i32,
    _count: i32,
    _children: *mut c_void,
    obtained: *mut i32,
) -> i32 {
    absent("oleacc.dll!AccessibleChildren");
    if !obtained.is_null() {
        // SAFETY: the caller's output.
        unsafe { obtained.write(0) };
    }
    E_NOTIMPL
}

#[unsafe(no_mangle)]
pub extern "system" fn WindowFromAccessibleObject(_object: *mut c_void, window: *mut usize) -> i32 {
    absent("oleacc.dll!WindowFromAccessibleObject");
    if !window.is_null() {
        // SAFETY: the caller's output.
        unsafe { window.write(0) };
    }
    E_NOTIMPL
}

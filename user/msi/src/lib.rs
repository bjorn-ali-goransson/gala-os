//! gala's msi.dll: the Windows Installer functions programs ask before
//! (or instead of) installing anything, answered for gala's machine,
//! where Windows Installer has installed nothing: gala has no installer
//! database and no installer service, so no product or component is
//! known. What the Office Deployment Tool's setup.exe imports (by
//! ordinal: build.rs), as Windows answers for products and components it
//! does not have (`msi_test`, `wow64_msi_test`; Wine's conformance tests):
//! - MsiQueryProductStateW, MsiGetComponentPathW, MsiLocateComponentW:
//!   INSTALLSTATE_UNKNOWN, INSTALLSTATE_INVALIDARG for an argument that
//!   is missing or not a braced GUID;
//! - MsiEnumProductsW: no products (ERROR_NO_MORE_ITEMS);
//! - MsiConfigureProductExW: ERROR_UNKNOWN_PRODUCT;
//! - MsiProvideQualifiedComponentExW, MsiEnumComponentQualifiersW:
//!   ERROR_UNKNOWN_COMPONENT;
//! - MsiSetInternalUI: the process's UI level and window, kept;
//! - MsiCloseHandle: no MSI handle is ever opened (ERROR_INVALID_HANDLE
//!   for any but 0).
//!
//! Each answer that stands for "Windows Installer has nothing here" is
//! recorded in the ledger (`msi.dll!<function>: nothing installed`, kind
//! "other", detail the call's ordinal), so a program that goes on to fail
//! because it expected an installed product says what it asked.
//! Installing (MsiInstallProduct and the rest) is not implemented: those
//! exports are absent, which the loader records.

#![no_std]
#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

// The 32-bit build (SysWOW64) takes the memory routines, 64-bit
// division helpers and unwind personality from ntrt's x86 runtime
// (ARCHITECTURE "Plan: WOW64").
#[cfg(target_arch = "x86")]
use ntrt as _;

/// MSVC-target codegen references this marker symbol directly (ntrt's
/// x86 runtime defines it on x86).
#[cfg(target_arch = "x86_64")]
#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

#[cfg_attr(
    target_arch = "x86",
    link(name = "ntdll", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "ntdll", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GalaRecordMissing(name: *const u8, length: usize, what: u32, detail: u64) -> u32;
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn SetLastError(code: u32);
}

const ERROR_SUCCESS: u32 = 0;
const ERROR_INVALID_HANDLE: u32 = 6;
const ERROR_INVALID_PARAMETER: u32 = 87;
const ERROR_NO_MORE_ITEMS: u32 = 259;
const ERROR_UNKNOWN_PRODUCT: u32 = 1605;
const ERROR_UNKNOWN_COMPONENT: u32 = 1607;
const INSTALLSTATE_INVALIDARG: i32 = -2;
const INSTALLSTATE_UNKNOWN: i32 = -1;

// INSTALLUILEVEL: a level in the low nibble and flags above it.
const INSTALLUILEVEL_NOCHANGE: u32 = 0;
const INSTALLUILEVEL_DEFAULT: u32 = 1;
const INSTALLUILEVEL_FULL: u32 = 5;
/// INSTALLUILEVEL_ENDDIALOG, _PROGRESSONLY, _HIDECANCEL, _SOURCERESONLY,
/// _UACONLY.
const INSTALLUILEVEL_FLAGS: u32 = 0x80 | 0x40 | 0x20 | 0x100 | 0x200;

/// The process's internal UI level (MsiSetInternalUI) and window.
static UI_LEVEL: AtomicU32 = AtomicU32::new(INSTALLUILEVEL_DEFAULT);
static UI_WINDOW: AtomicUsize = AtomicUsize::new(0);

/// Records an answer of "nothing installed" (module comment).
fn nothing_installed(name: &[u8], ordinal: u64) {
    let mut text = [0u8; 96];
    let mut n = 0;
    for part in [&b"msi.dll!"[..], name, b": nothing installed"] {
        let take = part.len().min(text.len() - n);
        text[n..n + take].copy_from_slice(&part[..take]);
        n += take;
    }
    // SAFETY: a valid buffer; 5 is the ledger's "other".
    unsafe { GalaRecordMissing(text.as_ptr(), n, 5, ordinal) };
}

/// Whether `p` is a braced GUID string, `{XXXXXXXX-XXXX-XXXX-XXXX-
/// XXXXXXXXXXXX}` (hexadecimal digits of either case), as product and
/// component codes are.
fn is_guid(p: *const u16) -> bool {
    if p.is_null() {
        return false;
    }
    for i in 0..39 {
        // SAFETY: the caller's NUL-terminated string, read up to its NUL
        // (every unit before index 38 is checked to be non-zero).
        let c = unsafe { *p.add(i) };
        let ok = match i {
            0 => c == u16::from(b'{'),
            9 | 14 | 19 | 24 => c == u16::from(b'-'),
            37 => c == u16::from(b'}'),
            38 => c == 0,
            _ => u8::try_from(c).is_ok_and(|b| b.is_ascii_hexdigit()),
        };
        if !ok {
            return false;
        }
    }
    true
}

/// MsiCloseHandle (#8): gala's msi opens no handles, so 0 (which closes
/// nothing) succeeds and any other is invalid.
#[unsafe(no_mangle)]
pub extern "system" fn MsiCloseHandle(handle: u32) -> u32 {
    if handle == 0 { ERROR_SUCCESS } else { ERROR_INVALID_HANDLE }
}

/// MsiSetInternalUI (#141): sets the process's UI level and returns the
/// previous one; INSTALLUILEVEL_NOCHANGE keeps it; a level above
/// INSTALLUILEVEL_FULL or an unknown flag returns INSTALLUILEVEL_NOCHANGE
/// and keeps the level, but its known flags still replace the current
/// ones (as Windows does: 0xdeadbeef after DEFAULT leaves DEFAULT with
/// flags 0x2e0, `msi_test`). A window passed is swapped with the previous.
#[unsafe(no_mangle)]
pub extern "system" fn MsiSetInternalUI(level: u32, window: *mut usize) -> u32 {
    if level & 0xf > INSTALLUILEVEL_FULL || level & !(0xf | INSTALLUILEVEL_FLAGS) != 0 {
        let current = UI_LEVEL.load(Ordering::Relaxed);
        UI_LEVEL.store(current & 0xf | level & INSTALLUILEVEL_FLAGS, Ordering::Relaxed);
        return INSTALLUILEVEL_NOCHANGE;
    }
    if !window.is_null() {
        // SAFETY: the caller's in/out window handle.
        unsafe { *window = UI_WINDOW.swap(*window, Ordering::Relaxed) };
    }
    if level & 0xf == INSTALLUILEVEL_NOCHANGE && level & INSTALLUILEVEL_FLAGS == 0 {
        return UI_LEVEL.load(Ordering::Relaxed);
    }
    UI_LEVEL.swap(level, Ordering::Relaxed)
}

/// MsiQueryProductStateW (#113): INSTALLSTATE_UNKNOWN for a product code,
/// INSTALLSTATE_INVALIDARG for anything else; the last error 0 either way
/// (as Windows answers a NULL code).
#[unsafe(no_mangle)]
pub extern "system" fn MsiQueryProductStateW(product: *const u16) -> i32 {
    // SAFETY: plain call.
    unsafe { SetLastError(ERROR_SUCCESS) };
    if !is_guid(product) {
        return INSTALLSTATE_INVALIDARG;
    }
    nothing_installed(b"MsiQueryProductStateW", 113);
    INSTALLSTATE_UNKNOWN
}

/// MsiEnumProductsW (#45): no products.
#[unsafe(no_mangle)]
pub extern "system" fn MsiEnumProductsW(index: u32, product: *mut u16) -> u32 {
    if product.is_null() {
        return ERROR_INVALID_PARAMETER;
    }
    if index == 0 {
        nothing_installed(b"MsiEnumProductsW", 45);
    }
    ERROR_NO_MORE_ITEMS
}

/// MsiGetComponentPathW (#173): INSTALLSTATE_UNKNOWN for a product and
/// component (the size untouched); INSTALLSTATE_INVALIDARG for a missing
/// or malformed code, or a buffer without its size.
#[unsafe(no_mangle)]
pub extern "system" fn MsiGetComponentPathW(
    product: *const u16,
    component: *const u16,
    path: *mut u16,
    size: *mut u32,
) -> i32 {
    if !is_guid(product) || !is_guid(component) || (!path.is_null() && size.is_null()) {
        return INSTALLSTATE_INVALIDARG;
    }
    nothing_installed(b"MsiGetComponentPathW", 173);
    INSTALLSTATE_UNKNOWN
}

/// MsiLocateComponentW (#90): as MsiGetComponentPathW without a product.
#[unsafe(no_mangle)]
pub extern "system" fn MsiLocateComponentW(
    component: *const u16,
    path: *mut u16,
    size: *mut u32,
) -> i32 {
    if !is_guid(component) || (!path.is_null() && size.is_null()) {
        return INSTALLSTATE_INVALIDARG;
    }
    nothing_installed(b"MsiLocateComponentW", 90);
    INSTALLSTATE_UNKNOWN
}

/// MsiConfigureProductExW (#190): ERROR_UNKNOWN_PRODUCT for a product
/// code, ERROR_INVALID_PARAMETER for anything else.
#[unsafe(no_mangle)]
pub extern "system" fn MsiConfigureProductExW(
    product: *const u16,
    _level: i32,
    _state: i32,
    _command_line: *const u16,
) -> u32 {
    if !is_guid(product) {
        return ERROR_INVALID_PARAMETER;
    }
    nothing_installed(b"MsiConfigureProductExW", 190);
    ERROR_UNKNOWN_PRODUCT
}

/// MsiProvideQualifiedComponentExW (#203): ERROR_UNKNOWN_COMPONENT (a
/// missing or malformed component is ERROR_INVALID_PARAMETER).
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn MsiProvideQualifiedComponentExW(
    component: *const u16,
    _qualifier: *const u16,
    _mode: u32,
    _product: *const u16,
    _unused1: u32,
    _unused2: u32,
    _path: *mut u16,
    _size: *mut u32,
) -> u32 {
    if !is_guid(component) {
        return ERROR_INVALID_PARAMETER;
    }
    nothing_installed(b"MsiProvideQualifiedComponentExW", 203);
    ERROR_UNKNOWN_COMPONENT
}

/// MsiEnumComponentQualifiersW (#39): ERROR_UNKNOWN_COMPONENT (a missing
/// or malformed component is ERROR_INVALID_PARAMETER).
#[unsafe(no_mangle)]
pub extern "system" fn MsiEnumComponentQualifiersW(
    component: *const u16,
    _index: u32,
    _qualifier: *mut u16,
    _qualifier_size: *mut u32,
    _data: *mut u16,
    _data_size: *mut u32,
) -> u32 {
    if !is_guid(component) {
        return ERROR_INVALID_PARAMETER;
    }
    nothing_installed(b"MsiEnumComponentQualifiersW", 39);
    ERROR_UNKNOWN_COMPONENT
}

/// DllMain (the entry point our link flags name).
#[unsafe(no_mangle)]
pub extern "system" fn mainCRTStartup(_instance: usize, _reason: u32, _reserved: usize) -> i32 {
    1
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}

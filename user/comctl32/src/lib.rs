//! gala's comctl32.dll: InitCommonControls(Ex), which programs call at
//! startup to register the common control window classes: the status bar
//! (`statusbar.rs`), tooltips (`tooltips.rs`) and the toolbar
//! (`toolbar.rs`), drawn in the Windows 11 style (`common.rs`); and image
//! lists (`imagelist.rs`).

#![no_std]
#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

extern crate alloc;

mod common;
mod imagelist;
mod statusbar;
mod toolbar;
mod tooltips;
mod unimplemented;

/// Registers the controls' classes (once).
fn register_classes() {
    static DONE: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
    if DONE.swap(true, core::sync::atomic::Ordering::AcqRel) {
        return;
    }
    common::register(b"msctls_statusbar32\0", statusbar::proc_);
    common::register(b"tooltips_class32\0", tooltips::proc_);
    common::register(b"ToolbarWindow32\0", toolbar::proc_);
}

// The global allocator (the image lists' vectors).
use gala_heap as _;

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn GalaRecordMissing(name: *const u8, length: usize, what: u32, detail: u64) -> u32;
}

/// Records an unsupported case in the ledger.
pub(crate) fn unsupported(what: &[u8], detail: u64) {
    // SAFETY: a valid buffer; "unsupported" (2).
    unsafe { GalaRecordMissing(what.as_ptr(), what.len(), 2, detail) };
}

#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memmove(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
    fn strlen(s: *const u8) -> usize;
    // MSVC-target unwind tables reference it (gala's ntdll provides it).
    fn __CxxFrameHandler3();
    // The stack probe for frames over a page (ntdll exports it).
    fn __chkstk();
}

/// MSVC-target codegen references this marker symbol directly, so it cannot
/// come from an import. (It becomes a harmless extra data export.)
#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

/// InitCommonControlsEx(INITCOMMONCONTROLSEX*): TRUE for a structure of
/// the documented size (8 bytes), FALSE otherwise.
#[unsafe(no_mangle)]
pub extern "system" fn InitCommonControlsEx(icc: *const u32) -> i32 {
    // SAFETY: the caller's structure; dwSize first.
    let ok = !icc.is_null() && unsafe { *icc } == 8;
    if ok {
        register_classes();
    }
    i32::from(ok)
}

#[unsafe(no_mangle)]
pub extern "system" fn InitCommonControls() {
    register_classes();
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

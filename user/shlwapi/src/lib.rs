//! gala's shlwapi.dll (shell path, string and URL helpers): the path
//! functions SQL Server LocalDB and wxWidgets use (`paths`,
//! `paths_more`), string comparisons and searches and SHDeleteKeyW
//! (`strings`); the URL functions, AssocQueryStringW, SHAutoComplete and
//! SHCreateStreamOnFileEx are not implemented yet (`gala_stubs`: each
//! reports itself to the missing-features ledger and ends the process
//! when called).

#![no_std]

mod assoc;
mod paths;
mod paths_more;
mod strings;
mod urls;

#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memmove(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
    // MSVC-target unwind tables reference it (gala's ntdll provides it).
    fn __CxxFrameHandler3();
    fn __chkstk();
}

/// MSVC-target codegen references this marker symbol directly, so it cannot
/// come from an import.
#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

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

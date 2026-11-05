// Shared by gala's GPU stub DLLs (dxgi, d3d11, d3d12, dcomp; ROADMAP
// GPU G0), included by each with `include!`: gala has no Direct3D
// adapter yet (G2 brings software 3D), so the entry points fail as they
// do on Windows when the system cannot provide what is asked:
// DXGI_ERROR_UNSUPPORTED, the output pointers set to NULL, each call
// recorded in the ledger (`<dll>!<function>`, unsupported) so a program
// that gives up afterwards shows why.

/// MSVC-target codegen references this marker symbol directly, so it cannot
/// come from an import.
#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn GalaRecordMissing(name: *const u8, length: usize, what: u32, detail: u64) -> u32;
}

/// DXGI_ERROR_UNSUPPORTED: the system does not support the request.
#[allow(dead_code)]
const DXGI_ERROR_UNSUPPORTED: i32 = 0x887a_0004_u32 as i32;
#[allow(dead_code)]
const E_INVALIDARG: i32 = 0x8007_0057_u32 as i32;

/// Records `what` (a `dll!function` name) as unsupported in the ledger.
fn unsupported(what: &[u8]) {
    // SAFETY: a valid buffer; the service only reads it.
    unsafe { GalaRecordMissing(what.as_ptr(), what.len(), 2, 0) };
}

/// Sets a caller's output pointer to NULL (when it gave one).
fn clear(out: *mut *mut core::ffi::c_void) {
    if !out.is_null() {
        // SAFETY: the caller's output pointer.
        unsafe { *out = core::ptr::null_mut() };
    }
}

/// The DLL entry point: nothing to set up.
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

//! gala's DWrite.dll (DirectWrite). Chromium imports DWriteCreateFactory
//! statically (Skia's font manager and Blink's font lookup are DirectWrite
//! on Windows), so the DLL must exist for chrome.dll to load. gala has no
//! DirectWrite yet: the factory is refused with E_NOTIMPL, which callers
//! that check the HRESULT see as "no DirectWrite", and the ledger records
//! each request with the factory type (0 shared, 1 isolated) as detail.

#![no_std]
#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

/// MSVC-target codegen references this marker symbol directly, so it cannot
/// come from an import.
#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn GalaRecordMissing(name: *const u8, length: usize, what: u32, detail: u64) -> u32;
}

const E_NOTIMPL: i32 = 0x8000_4001_u32 as i32;
const E_POINTER: i32 = 0x8000_4003_u32 as i32;

/// DWriteCreateFactory(factoryType, iid, **factory): not available yet
/// (recorded as unsupported); *factory is set to null.
#[unsafe(no_mangle)]
pub extern "system" fn DWriteCreateFactory(
    factory_type: u32,
    _iid: *const u8,
    factory: *mut *mut core::ffi::c_void,
) -> i32 {
    let name = b"dwrite.dll!DWriteCreateFactory";
    // SAFETY: a valid buffer; the service only reads it.
    unsafe { GalaRecordMissing(name.as_ptr(), name.len(), 2, factory_type.into()) };
    if factory.is_null() {
        return E_POINTER;
    }
    // SAFETY: the caller's output pointer.
    unsafe { *factory = core::ptr::null_mut() };
    E_NOTIMPL
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

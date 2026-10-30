//! A DLL that imports from testdll.dll, for loader tests: loading it must
//! bind its own imports (testdll's exports) too.

#![no_std]

#[link(name = "testdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn gala_add(a: u64, b: u64) -> u64;
    fn gala_get_value() -> u64;
}

#[unsafe(no_mangle)]
pub extern "C" fn gala2_add3(a: u64, b: u64, c: u64) -> u64 {
    // SAFETY: imported from testdll, bound by the loader.
    unsafe { gala_add(gala_add(a, b), c) }
}

#[unsafe(no_mangle)]
pub extern "C" fn gala2_get_value() -> u64 {
    // SAFETY: as above.
    unsafe { gala_get_value() }
}

/// Entry point required by our link flags; a DllMain that does nothing.
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

//! A tiny DLL for image-section tests: exports a function, a data value
//! and a function that reads the value through a pointer stored in the
//! DLL's data (so the image needs a DIR64 base relocation).

#![no_std]

#[unsafe(no_mangle)]
pub static mut GALA_VALUE: u64 = 0x1234_5678_9abc;

/// Absolute address of `GALA_VALUE`: fixed up by base relocation.
#[unsafe(no_mangle)]
// A reference to the static mut itself is denied; this one points at it.
#[allow(clippy::deref_addrof)]
pub static GALA_VALUE_PTR: &u64 = unsafe { &*(&raw const GALA_VALUE) };

#[unsafe(no_mangle)]
pub extern "C" fn gala_add(a: u64, b: u64) -> u64 {
    a.wrapping_add(b)
}

#[unsafe(no_mangle)]
pub extern "C" fn gala_get_value() -> u64 {
    // SAFETY: reads the value through the relocated pointer.
    unsafe { core::ptr::read_volatile(GALA_VALUE_PTR) }
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

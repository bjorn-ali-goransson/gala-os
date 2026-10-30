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

/// Number of DLL_PROCESS_ATTACH calls DllMain has seen.
#[unsafe(no_mangle)]
pub static mut GALA_ATTACH_COUNT: u64 = 0;

/// Entry point required by our link flags; a DllMain that counts attaches.
#[unsafe(no_mangle)]
pub extern "system" fn mainCRTStartup(_instance: usize, reason: u32, _reserved: usize) -> i32 {
    if reason == 1 {
        // SAFETY: the loader calls DllMain with the loader lock held, so
        // nothing else touches the counter at the same time.
        unsafe { GALA_ATTACH_COUNT += 1 };
    }
    1
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}

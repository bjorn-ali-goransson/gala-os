//! gala's oleacc.dll (Active Accessibility): the functions FileZilla and
//! Chromium import, none implemented yet; each reports itself to the
//! missing-features ledger and fails as documented (`unimplemented.rs`).

#![no_std]

mod unimplemented;

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

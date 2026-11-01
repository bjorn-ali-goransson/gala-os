//! gala's webservices.dll (Windows Web Services (SOAP)): for now the functions SQL Server LocalDB
//! imports, none implemented yet. Each reports itself to the
//! missing-features ledger and ends the process when called
//! (`gala_stubs`), so the ledger shows which ones a program uses.

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

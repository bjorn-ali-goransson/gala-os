//! A DLL that imports testdll.dll's ordinal 2, which testdll leaves
//! without an export (its build.rs numbers gala_add 1 and gala_get_value
//! 3): the export address table's slot holds RVA 0. loader_test loads it
//! and expects the load to fail with STATUS_ORDINAL_NOT_FOUND (on gala; on
//! Windows the answer is printed to measure), the import never bound to
//! testdll's base.

#![no_std]

#[link(name = "testdll", kind = "raw-dylib")]
unsafe extern "C" {
    #[link_ordinal(2)]
    fn gala_no_export(a: u64) -> u64;
}

#[unsafe(no_mangle)]
pub extern "C" fn gala_gap_call(a: u64) -> u64 {
    // SAFETY: imported from testdll; never reached unless the loader bound
    // an ordinal without an export.
    unsafe { gala_no_export(a) }
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

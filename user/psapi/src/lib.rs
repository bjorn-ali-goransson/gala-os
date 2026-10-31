//! gala's psapi.dll: forwarders to kernel32's K32* process status
//! functions (see build.rs), as Windows' psapi.dll is. The JNA native
//! library (jnidispatch.dll, Elasticsearch) imports EnumProcessModules
//! from it.

#![no_std]

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

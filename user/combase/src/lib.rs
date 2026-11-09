//! gala's combase.dll: Windows' COM and Windows Runtime base (HSTRINGs,
//! RoGetActivationFactory, error origination, apartments) as forwarders
//! to gala's ole32.dll, which implements them (build.rs). Programs load it
//! by name: C++/WinRT's runtime looks its functions up here.

#![no_std]

// The 32-bit build (SysWOW64) takes its runtime from ntrt.
#[cfg(target_arch = "x86")]
use ntrt as _;

#[cfg(target_arch = "x86_64")]
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

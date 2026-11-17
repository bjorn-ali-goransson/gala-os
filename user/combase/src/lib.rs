//! gala's combase.dll: Windows' COM and Windows Runtime base (HSTRINGs,
//! RoGetActivationFactory, error origination, apartments) as forwarders
//! to gala's ole32.dll, which implements them (build.rs). Programs load it
//! by name: C++/WinRT's runtime looks its functions up here.

#![no_std]

// The 32-bit build (SysWOW64) takes its runtime from ntrt.
#[cfg(target_arch = "x86")]
use ntrt as _;

#[cfg(not(target_arch = "x86"))]
#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

// rpcrt4.dll, which Windows' combase imports (118 names, one by ordinal:
// the NDR stubs, the binding and the async call API; its import table,
// measured on 26100). gala's combase calls nothing in it -- its exports forward to
// ole32 -- but the dependency is part of what a program sees
// (ARCHITECTURE "The module graph"). Nothing here calls it: build.rs pins
// the import on the link command line, so the linker keeps it. gala has no
// 32-bit rpcrt4 yet, so the 32-bit build goes without it.
#[cfg(not(target_arch = "x86"))]
#[allow(dead_code)]
#[link(name = "rpcrt4", kind = "raw-dylib")]
unsafe extern "system" {
    fn RpcStringFreeW(text: *mut *mut u16) -> i32;
}

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

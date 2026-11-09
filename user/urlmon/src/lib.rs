//! gala's urlmon.dll (URL monikers), x64 and x86: the security manager
//! (CoInternetCreateSecurityManager, `secmgr`), which ODT's setup.exe asks
//! for on its download's error path (ROADMAP OF), and the IUri functions
//! SQL Server LocalDB imports (not implemented: each reports itself to the
//! missing-features ledger and ends the process when called, `gala_stubs`).

#![no_std]
#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

mod secmgr;
mod unimplemented;

// The 32-bit build (SysWOW64) takes the memory routines, 64-bit
// division helpers and unwind personality from ntrt's x86 runtime
// (ARCHITECTURE "Plan: WOW64").
#[cfg(target_arch = "x86")]
use ntrt as _;

#[cfg(target_arch = "x86_64")]
#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memmove(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
    fn __CxxFrameHandler3();
    fn __chkstk();
}

/// MSVC-target codegen references this marker symbol directly, so it cannot
/// come from an import.
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

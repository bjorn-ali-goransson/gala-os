//! gala's kernelbase.dll: what programs look up there that kernel32 does
//! not export (Cygwin's runtime autoloads these): the interrupt-time
//! queries and SetThreadDescription. VirtualAlloc2 is not implemented.

#![no_std]

use core::ffi::c_void;

/// MSVC-target codegen references this marker symbol directly, so it cannot
/// come from an import.
#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

/// DllMain (the entry point our link flags name).
#[unsafe(no_mangle)]
pub extern "system" fn mainCRTStartup(_instance: usize, _reason: u32, _reserved: usize) -> i32 {
    1
}

/// KUSER_SHARED_DATA's InterruptTime (100 ns since boot), a KSYSTEM_TIME
/// read until its two high parts agree.
fn interrupt_time() -> u64 {
    const KUSER: usize = 0x7ffe_0000;
    // SAFETY: KUSER_SHARED_DATA is mapped read-only in every process.
    unsafe {
        loop {
            let high1 = core::ptr::read_volatile((KUSER + 0x0c) as *const u32);
            let low = core::ptr::read_volatile((KUSER + 0x08) as *const u32);
            let high2 = core::ptr::read_volatile((KUSER + 0x10) as *const u32);
            if high1 == high2 {
                return u64::from(high1) << 32 | u64::from(low);
            }
        }
    }
}

/// Writes the interrupt time to the caller's output.
fn put(out: *mut u64) {
    if !out.is_null() {
        // SAFETY: the caller's output.
        unsafe { *out = interrupt_time() };
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn QueryInterruptTime(out: *mut u64) {
    put(out);
}

/// The precise forms: the same clock (gala's is not coarser).
#[unsafe(no_mangle)]
pub extern "system" fn QueryInterruptTimePrecise(out: *mut u64) {
    put(out);
}

/// Unbiased: gala does not sleep, so the same.
#[unsafe(no_mangle)]
pub extern "system" fn QueryUnbiasedInterruptTimePrecise(out: *mut u64) {
    put(out);
}

/// SetThreadDescription(thread, description): S_OK; the description is
/// not kept (TODO: GetThreadDescription).
#[unsafe(no_mangle)]
pub extern "system" fn SetThreadDescription(_thread: *mut c_void, _description: *const u16) -> i32 {
    0
}

gala_stubs::unimplemented_exports!("kernelbase.dll":
    VirtualAlloc2
);

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}

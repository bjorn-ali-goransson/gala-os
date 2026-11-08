//! gala's kernelbase.dll, the host of the core API sets (api-ms-win-core-*,
//! ARCHITECTURE "API sets"): it exports what Windows' kernelbase exports,
//! as forwarders to the gala DLL that implements each function (build.rs,
//! exports.txt), and implements what only kernelbase has: CompareObjectHandles
//! and the interrupt-time queries (Cygwin's runtime autoloads those).
//! VirtualAlloc2 is not implemented.

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

#[cfg_attr(
    target_arch = "x86",
    link(name = "ntdll", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "ntdll", kind = "raw-dylib"))]
unsafe extern "system" {
    fn NtCompareObjects(first: usize, second: usize) -> u32;
    fn RtlNtStatusToDosError(status: u32) -> u32;
}

/// TEB.LastErrorValue := RtlNtStatusToDosError(status).
fn set_last_error(status: u32) {
    // SAFETY: a plain function call.
    let error = unsafe { RtlNtStatusToDosError(status) };
    // SAFETY: the running thread's TEB (LastErrorValue at 0x68, 0x34 in
    // the 32-bit TEB).
    unsafe {
        #[cfg(target_arch = "x86_64")]
        core::arch::asm!("mov gs:[0x68], {0:e}", in(reg) error, options(nostack, preserves_flags));
        #[cfg(target_arch = "x86")]
        core::arch::asm!("mov fs:[0x34], {0}", in(reg) error, options(nostack, preserves_flags));
    }
}

/// CompareObjectHandles(hFirstObjectHandle, hSecondObjectHandle): whether
/// both handles refer to the same kernel object (NtCompareObjects). FALSE
/// for different objects, with ERROR_NOT_SAME_OBJECT. Kernelbase's only
/// (documented "DLL: Kernelbase.dll"; Windows' kernel32 does not export
/// it), reached through api-ms-win-core-handle-l1-1-0.
#[unsafe(no_mangle)]
pub extern "system" fn CompareObjectHandles(first: usize, second: usize) -> i32 {
    // SAFETY: the kernel checks both handles.
    let status = unsafe { NtCompareObjects(first, second) };
    if status != 0 {
        set_last_error(status);
        return 0;
    }
    1
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

//! gala's kernelbase.dll, the host of the core API sets (api-ms-win-core-*,
//! ARCHITECTURE "API sets"): it exports what Windows' kernelbase exports,
//! as forwarders to the gala DLL that implements each function (build.rs,
//! exports.txt), and implements what only kernelbase has (Windows' kernel32
//! exports none of it): CompareObjectHandles, the interrupt-time queries
//! (Cygwin's runtime autoloads those), WaitOnAddress and WakeByAddress*
//! (over ntdll's RtlWaitOnAddress and RtlWakeAddress*, where every wait on
//! a word sleeps) and the PathCch functions (`path`). VirtualAlloc2 and four PathCch
//! functions are not implemented.

#![no_std]

mod path;

/// MSVC-target codegen references this marker symbol directly, so it cannot
/// come from an import. On x86, ntrt's.
#[cfg(target_arch = "x86_64")]
#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

// The C runtime helpers the compiler calls (`path`'s buffers are copied,
// zeroed and probed), ntdll's exports as kernel32 takes them. On x86 they
// come from ntrt's x86 runtime.
#[cfg(target_arch = "x86_64")]
#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
    fn memmove(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    // MSVC-target unwind tables reference it (gala's ntdll provides it).
    fn __CxxFrameHandler3();
    // The stack probe for frames over a page (ntdll exports it).
    fn __chkstk();
}
#[cfg(target_arch = "x86")]
use ntrt as _;

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
    fn RtlWaitOnAddress(
        address: *const core::ffi::c_void,
        compare: *const core::ffi::c_void,
        size: usize,
        timeout: *const i64,
    ) -> u32;
    fn RtlWakeAddressSingle(address: *const core::ffi::c_void);
    fn RtlWakeAddressAll(address: *const core::ffi::c_void);
}

/// TEB.LastErrorValue := RtlNtStatusToDosError(status).
fn set_last_error(status: u32) {
    // SAFETY: a plain function call.
    set_last_error_code(unsafe { RtlNtStatusToDosError(status) });
}

/// TEB.LastErrorValue := error.
fn set_last_error_code(error: u32) {
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

/// WaitOnAddress(Address, CompareAddress, AddressSize, dwMilliseconds):
/// waits while the value at Address equals the one at CompareAddress, until
/// a WakeByAddress* call for Address (or a spurious wake) or the timeout
/// (FALSE, ERROR_TIMEOUT). Sizes 1, 2, 4 and 8 (else
/// ERROR_INVALID_PARAMETER).
// The pointers go to ntdll, which checks them.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[unsafe(no_mangle)]
pub extern "system" fn WaitOnAddress(
    address: *const core::ffi::c_void,
    compare: *const core::ffi::c_void,
    size: usize,
    ms: u32,
) -> i32 {
    const INFINITE: u32 = u32::MAX;
    const STATUS_TIMEOUT: u32 = 0x102;
    const ERROR_TIMEOUT: u32 = 1460;
    let relative = -(i64::from(ms) * 10_000);
    let timeout = if ms == INFINITE { core::ptr::null() } else { &raw const relative };
    // SAFETY: ntdll checks the pointers' presence and the size; the
    // timeout lives through the call.
    match unsafe { RtlWaitOnAddress(address, compare, size, timeout) } {
        0 => 1,
        STATUS_TIMEOUT => {
            set_last_error_code(ERROR_TIMEOUT);
            0
        }
        status => {
            set_last_error(status);
            0
        }
    }
}

/// WakeByAddressSingle(Address): wakes one thread waiting on Address.
// The pointers go to ntdll, which checks them.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[unsafe(no_mangle)]
pub extern "system" fn WakeByAddressSingle(address: *const core::ffi::c_void) {
    // SAFETY: only the address's value is used.
    unsafe { RtlWakeAddressSingle(address) };
}

/// WakeByAddressAll(Address): wakes every thread waiting on Address.
// The pointers go to ntdll, which checks them.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[unsafe(no_mangle)]
pub extern "system" fn WakeByAddressAll(address: *const core::ffi::c_void) {
    // SAFETY: only the address's value is used.
    unsafe { RtlWakeAddressAll(address) };
}

gala_stubs::unimplemented_exports!("kernelbase.dll":
    VirtualAlloc2
    PathCchAppend
    PathCchCombine
    PathCchRemoveBackslash
    PathCchRemoveExtension
);

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}

//! gala's powrprof.dll: CallNtPowerInformation for processor information
//! (the JVM reads processor frequencies). gala does no power management:
//! every processor runs at its base frequency, read from CPUID leaf 0x16
//! (0 where the processor does not report it).

#![no_std]
#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

mod unimplemented;

#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memmove(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
    fn strlen(s: *const u8) -> usize;
    // MSVC-target unwind tables reference it (gala's ntdll provides it).
    fn __CxxFrameHandler3();
    // The stack probe for frames over a page (ntdll exports it).
    fn __chkstk();
}

/// MSVC-target codegen references this marker symbol directly, so it cannot
/// come from an import. (It becomes a harmless extra data export.)
#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn GalaRecordMissing(name: *const u8, length: usize, what: u32, detail: u64) -> u32;
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetSystemInfo(info: *mut u8);
}

const STATUS_SUCCESS: u32 = 0;
const STATUS_BUFFER_TOO_SMALL: u32 = 0xc000_0023;
const STATUS_INVALID_PARAMETER: u32 = 0xc000_000d;
const PROCESSOR_INFORMATION: u32 = 11;

/// The base frequency in MHz (CPUID 0x16 EAX), or 0.
fn base_mhz() -> u32 {
    let max = core::arch::x86_64::__cpuid(0).eax;
    if max < 0x16 {
        return 0;
    }
    // Leaf 0x16 exists (checked).
    core::arch::x86_64::__cpuid(0x16).eax & 0xffff
}

/// CallNtPowerInformation: ProcessorInformation (11) gives one
/// PROCESSOR_POWER_INFORMATION (Number, MaxMhz, CurrentMhz, MhzLimit,
/// MaxIdleState, CurrentIdleState) per processor; STATUS_BUFFER_TOO_SMALL
/// for a shorter buffer. Other levels are reported.
#[unsafe(no_mangle)]
pub extern "system" fn CallNtPowerInformation(
    level: u32,
    _input: *const u8,
    _input_length: u32,
    output: *mut u32,
    output_length: u32,
) -> u32 {
    if level != PROCESSOR_INFORMATION {
        let name = b"powrprof.dll!CallNtPowerInformation(level)";
        // SAFETY: a valid buffer.
        unsafe { GalaRecordMissing(name.as_ptr(), name.len(), 2, level.into()) };
        return STATUS_INVALID_PARAMETER;
    }
    let mut info = [0u8; 48];
    // SAFETY: a SYSTEM_INFO buffer.
    unsafe { GetSystemInfo(info.as_mut_ptr()) };
    let cpus = u32::from_le_bytes([info[32], info[33], info[34], info[35]]);
    if output.is_null() || output_length < 24 * cpus {
        return STATUS_BUFFER_TOO_SMALL;
    }
    let mhz = base_mhz();
    for i in 0..cpus as usize {
        // SAFETY: the caller's buffer of at least 24 * cpus bytes.
        unsafe {
            let e = output.add(6 * i);
            for (j, v) in [i as u32, mhz, mhz, mhz, 0, 0].into_iter().enumerate() {
                *e.add(j) = v;
            }
        }
    }
    STATUS_SUCCESS
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

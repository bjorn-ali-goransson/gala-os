//! gala's winmm.dll: the multimedia timer functions programs use to ask
//! for timer resolution (the Go runtime and the JVM load it for
//! timeBeginPeriod). gala's timers run on a 1 ms tick whatever is asked.

#![no_std]
#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memmove(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetTickCount() -> u32;
}

const TIMERR_NOERROR: u32 = 0;
const TIMERR_NOCANDO: u32 = 97;
/// The resolutions timeGetDevCaps reports, in milliseconds.
const PERIOD_MIN: u32 = 1;
const PERIOD_MAX: u32 = 1_000_000;

/// timeBeginPeriod(ms): granted for any period in range (the tick is
/// already 1 ms); TIMERR_NOCANDO otherwise.
#[unsafe(no_mangle)]
pub extern "system" fn timeBeginPeriod(period: u32) -> u32 {
    if (PERIOD_MIN..=PERIOD_MAX).contains(&period) { TIMERR_NOERROR } else { TIMERR_NOCANDO }
}

#[unsafe(no_mangle)]
pub extern "system" fn timeEndPeriod(period: u32) -> u32 {
    timeBeginPeriod(period)
}

/// timeGetTime: milliseconds since the system started (as GetTickCount).
#[unsafe(no_mangle)]
pub extern "system" fn timeGetTime() -> u32 {
    // SAFETY: plain call.
    unsafe { GetTickCount() }
}

/// timeGetDevCaps(TIMECAPS*, size): the periods timeBeginPeriod accepts.
#[unsafe(no_mangle)]
pub extern "system" fn timeGetDevCaps(caps: *mut u32, size: u32) -> u32 {
    if caps.is_null() || size < 8 {
        return TIMERR_NOCANDO;
    }
    // SAFETY: the caller's TIMECAPS { wPeriodMin, wPeriodMax }.
    unsafe {
        *caps = PERIOD_MIN;
        *caps.add(1) = PERIOD_MAX;
    }
    TIMERR_NOERROR
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

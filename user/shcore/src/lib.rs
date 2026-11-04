//! gala's shcore.dll: the DPI scaling functions of
//! api-ms-win-shcore-scaling (shellscalingapi.h), which Chromium imports.
//! gala's monitors are 96 DPI at 100% scale; the process awareness is
//! user32's (SetProcessDpiAwarenessContext), which this DLL forwards to.

#![no_std]
#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

/// MSVC-target codegen references this marker symbol directly, so it cannot
/// come from an import.
#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

#[link(name = "user32", kind = "raw-dylib")]
unsafe extern "system" {
    fn SetProcessDpiAwarenessContext(context: isize) -> i32;
    fn GetThreadDpiAwarenessContext() -> isize;
    fn GetAwarenessFromDpiAwarenessContext(context: isize) -> i32;
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetLastError() -> u32;
}

const S_OK: i32 = 0;
const E_INVALIDARG: i32 = 0x8007_0057_u32 as i32;
const E_ACCESSDENIED: i32 = 0x8007_0005_u32 as i32;

/// GetDpiForMonitor(hmonitor, dpiType, *dpiX, *dpiY): 96 x 96 for every
/// type (effective, angular, raw).
#[unsafe(no_mangle)]
pub extern "system" fn GetDpiForMonitor(
    monitor: usize,
    kind: u32,
    x: *mut u32,
    y: *mut u32,
) -> i32 {
    if monitor == 0 || kind > 2 || x.is_null() || y.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's outputs.
    unsafe {
        *x = 96;
        *y = 96;
    }
    S_OK
}

/// GetScaleFactorForMonitor(hMon, *pScale): SCALE_100_PERCENT.
#[unsafe(no_mangle)]
pub extern "system" fn GetScaleFactorForMonitor(monitor: usize, scale: *mut u32) -> i32 {
    if monitor == 0 || scale.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's output.
    unsafe { *scale = 100 };
    S_OK
}

/// SetProcessDpiAwareness(value): PROCESS_DPI_UNAWARE (0), SYSTEM (1),
/// PER_MONITOR (2), as user32's awareness contexts -1, -2, -3;
/// E_ACCESSDENIED once one was set.
#[unsafe(no_mangle)]
pub extern "system" fn SetProcessDpiAwareness(value: u32) -> i32 {
    if value > 2 {
        return E_INVALIDARG;
    }
    // SAFETY: plain calls.
    unsafe {
        if SetProcessDpiAwarenessContext(-1 - value as isize) != 0 {
            S_OK
        } else if GetLastError() == 5 {
            E_ACCESSDENIED
        } else {
            E_INVALIDARG
        }
    }
}

/// GetProcessDpiAwareness(hprocess, *value): this process's awareness.
#[unsafe(no_mangle)]
pub extern "system" fn GetProcessDpiAwareness(_process: usize, value: *mut u32) -> i32 {
    if value.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: plain calls; the caller's output.
    unsafe {
        let a = GetAwarenessFromDpiAwarenessContext(GetThreadDpiAwarenessContext());
        *value = a.max(0) as u32;
    }
    S_OK
}

/// The DLL entry point: nothing to set up.
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

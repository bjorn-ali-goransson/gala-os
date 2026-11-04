//! DPI awareness (winuser.h, Windows 10's per-monitor forms): gala's
//! screen is 96 DPI and never changes scale, so every DPI asked for is 96
//! and the *ForDpi forms are the plain ones. The awareness a process or
//! thread sets is kept and read back (Chromium sets per-monitor v2).

use core::sync::atomic::{AtomicIsize, Ordering};

const DPI: u32 = 96;
const UNAWARE: isize = -1;
const SYSTEM_AWARE: isize = -2;
const PER_MONITOR_AWARE: isize = -3;
const PER_MONITOR_AWARE_V2: isize = -4;
const UNAWARE_GDISCALED: isize = -5;

/// The process's awareness context (a DPI_AWARENESS_CONTEXT value).
static PROCESS: AtomicIsize = AtomicIsize::new(UNAWARE);
/// Set once a process awareness has been chosen (it cannot change then).
static PROCESS_SET: AtomicIsize = AtomicIsize::new(0);

fn valid(context: isize) -> bool {
    (UNAWARE_GDISCALED..=UNAWARE).contains(&context)
}

/// SetProcessDpiAwarenessContext(value): once per process; then
/// ERROR_ACCESS_DENIED, as on Windows.
#[unsafe(no_mangle)]
pub extern "system" fn SetProcessDpiAwarenessContext(context: isize) -> i32 {
    if !valid(context) {
        set_error(87); // ERROR_INVALID_PARAMETER
        return 0;
    }
    if PROCESS_SET.swap(1, Ordering::AcqRel) != 0 {
        set_error(5); // ERROR_ACCESS_DENIED
        return 0;
    }
    PROCESS.store(context, Ordering::Release);
    1
}

/// SetProcessDPIAware: system awareness (if none was set yet).
#[unsafe(no_mangle)]
pub extern "system" fn SetProcessDPIAware() -> i32 {
    if PROCESS_SET.swap(1, Ordering::AcqRel) == 0 {
        PROCESS.store(SYSTEM_AWARE, Ordering::Release);
    }
    1
}

#[unsafe(no_mangle)]
pub extern "system" fn IsProcessDPIAware() -> i32 {
    i32::from(PROCESS.load(Ordering::Acquire) != UNAWARE)
}

/// GetThreadDpiAwarenessContext: the process's (gala keeps no per-thread
/// override beyond what SetThreadDpiAwarenessContext returns).
#[unsafe(no_mangle)]
pub extern "system" fn GetThreadDpiAwarenessContext() -> isize {
    PROCESS.load(Ordering::Acquire)
}

/// SetThreadDpiAwarenessContext(context): the previous context; the
/// thread goes on with the process's (every DPI is 96 anyway).
#[unsafe(no_mangle)]
pub extern "system" fn SetThreadDpiAwarenessContext(context: isize) -> isize {
    if !valid(context) {
        set_error(87);
        return 0;
    }
    PROCESS.load(Ordering::Acquire)
}

#[unsafe(no_mangle)]
pub extern "system" fn GetWindowDpiAwarenessContext(_hwnd: usize) -> isize {
    PROCESS.load(Ordering::Acquire)
}

#[unsafe(no_mangle)]
pub extern "system" fn IsValidDpiAwarenessContext(context: isize) -> i32 {
    i32::from(valid(context))
}

#[unsafe(no_mangle)]
pub extern "system" fn AreDpiAwarenessContextsEqual(a: isize, b: isize) -> i32 {
    i32::from(a == b)
}

/// GetAwarenessFromDpiAwarenessContext: DPI_AWARENESS (0 unaware, 1
/// system, 2 per monitor; -1 invalid).
#[unsafe(no_mangle)]
pub extern "system" fn GetAwarenessFromDpiAwarenessContext(context: isize) -> i32 {
    match context {
        UNAWARE | UNAWARE_GDISCALED => 0,
        SYSTEM_AWARE => 1,
        PER_MONITOR_AWARE | PER_MONITOR_AWARE_V2 => 2,
        _ => -1,
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn GetDpiFromDpiAwarenessContext(_context: isize) -> u32 {
    DPI
}

#[unsafe(no_mangle)]
pub extern "system" fn GetDpiForSystem() -> u32 {
    DPI
}

#[unsafe(no_mangle)]
pub extern "system" fn GetDpiForWindow(hwnd: usize) -> u32 {
    if hwnd == 0 {
        set_error(1400); // ERROR_INVALID_WINDOW_HANDLE
        return 0;
    }
    DPI
}

#[unsafe(no_mangle)]
pub extern "system" fn GetSystemDpiForProcess(_process: usize) -> u32 {
    DPI
}

#[unsafe(no_mangle)]
pub extern "system" fn EnableNonClientDpiScaling(_hwnd: usize) -> i32 {
    1
}

#[unsafe(no_mangle)]
pub extern "system" fn GetSystemMetricsForDpi(index: i32, _dpi: u32) -> i32 {
    super::window::GetSystemMetrics(index)
}

#[unsafe(no_mangle)]
pub extern "system" fn SystemParametersInfoForDpi(
    action: u32,
    param: u32,
    data: *mut core::ffi::c_void,
    ini: u32,
    _dpi: u32,
) -> i32 {
    super::spi::SystemParametersInfoW(action, param, data.cast(), ini)
}

fn set_error(code: u32) {
    // SAFETY: plain call.
    unsafe { super::SetLastError(code) };
}

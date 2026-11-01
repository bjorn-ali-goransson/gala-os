//! Event Tracing for Windows, the provider side (EventRegister and
//! friends, as documented in "Writing Manifest-based Events"): gala has no
//! trace sessions, so no provider is ever enabled. Registration succeeds
//! and events go nowhere, successfully - what Windows does for a provider
//! no session listens to. Not recorded in the ledger: nothing is lost.

use core::sync::atomic::{AtomicU64, Ordering};

/// REGHANDLE values: never 0 (0 is what failed registration leaves).
static NEXT: AtomicU64 = AtomicU64::new(1);

const ERROR_INVALID_PARAMETER: u32 = 87;

#[unsafe(no_mangle)]
pub extern "system" fn EventRegister(
    provider: *const [u8; 16],
    _callback: *const core::ffi::c_void,
    _context: *mut core::ffi::c_void,
    handle: *mut u64,
) -> u32 {
    if provider.is_null() || handle.is_null() {
        return ERROR_INVALID_PARAMETER;
    }
    // SAFETY: the caller's output.
    unsafe { *handle = NEXT.fetch_add(1, Ordering::Relaxed) };
    0
}

#[unsafe(no_mangle)]
pub extern "system" fn EventUnregister(_handle: u64) -> u32 {
    0
}

/// EventEnabled / EventProviderEnabled: no session enables anything.
#[unsafe(no_mangle)]
pub extern "system" fn EventEnabled(_handle: u64, _descriptor: *const u8) -> u8 {
    0
}

#[unsafe(no_mangle)]
pub extern "system" fn EventProviderEnabled(_handle: u64, _level: u8, _keyword: u64) -> u8 {
    0
}

#[unsafe(no_mangle)]
pub extern "system" fn EventWrite(
    _handle: u64,
    _descriptor: *const u8,
    _count: u32,
    _data: *const u8,
) -> u32 {
    0
}

#[unsafe(no_mangle)]
pub extern "system" fn EventWriteTransfer(
    _handle: u64,
    _descriptor: *const u8,
    _activity: *const u8,
    _related: *const u8,
    _count: u32,
    _data: *const u8,
) -> u32 {
    0
}

#[unsafe(no_mangle)]
pub extern "system" fn EventWriteEx(
    _handle: u64,
    _descriptor: *const u8,
    _filter: u64,
    _flags: u32,
    _activity: *const u8,
    _related: *const u8,
    _count: u32,
    _data: *const u8,
) -> u32 {
    0
}

#[unsafe(no_mangle)]
pub extern "system" fn EventWriteString(
    _handle: u64,
    _level: u8,
    _keyword: u64,
    _text: *const u16,
) -> u32 {
    0
}

/// EventSetInformation: provider traits and the like; kept nowhere.
#[unsafe(no_mangle)]
pub extern "system" fn EventSetInformation(
    _handle: u64,
    _class: u32,
    _info: *const u8,
    _length: u32,
) -> u32 {
    0
}

/// TEB.ActivityId (x64 offset 0x1710): the thread's ETW activity id.
fn activity_id() -> *mut [u8; 16] {
    let teb: usize;
    // SAFETY: gs:[0x30] is the TEB's self pointer.
    unsafe { core::arch::asm!("mov {}, gs:[0x30]", out(reg) teb, options(nostack, readonly)) };
    (teb + 0x1710) as *mut [u8; 16]
}

#[link(name = "bcryptprimitives", kind = "raw-dylib")]
unsafe extern "system" {
    fn ProcessPrng(data: *mut u8, length: usize) -> i32;
}

/// EventActivityIdControl: GET_ID (1), SET_ID (2), CREATE_ID (3, a new
/// version-4 GUID, not stored), GET_SET_ID (4, swap), CREATE_SET_ID (5,
/// store a new one, return the old).
#[unsafe(no_mangle)]
pub extern "system" fn EventActivityIdControl(code: u32, id: *mut [u8; 16]) -> u32 {
    if id.is_null() {
        return ERROR_INVALID_PARAMETER;
    }
    let new_id = || {
        let mut g = [0u8; 16];
        // SAFETY: our buffer.
        unsafe { ProcessPrng(g.as_mut_ptr(), 16) };
        g[7] = (g[7] & 0x0f) | 0x40;
        g[8] = (g[8] & 0x3f) | 0x80;
        g
    };
    let current = activity_id();
    // SAFETY: the caller's GUID; this thread's TEB.
    unsafe {
        match code {
            1 => *id = *current,
            2 => *current = *id,
            3 => *id = new_id(),
            4 => core::ptr::swap(id, current),
            5 => {
                *id = *current;
                *current = new_id();
            }
            _ => return ERROR_INVALID_PARAMETER,
        }
    }
    0
}

// ---- Classic (MOF) providers: RegisterTraceGuids and friends ----
//
// The same: a provider registers; no trace session ever enables it, so its
// control callback is never called, there is no logger handle to log to,
// and logging with a session handle it could not have got fails.

const ERROR_INVALID_HANDLE: u32 = 6;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn SetLastError(code: u32);
}

/// RegisterTraceGuidsW: a registration handle (the control callback is not
/// called: no session enables the provider).
#[allow(clippy::too_many_arguments)]
#[unsafe(no_mangle)]
pub extern "system" fn RegisterTraceGuidsW(
    callback: usize,
    _context: *mut core::ffi::c_void,
    control: *const [u8; 16],
    _count: u32,
    _registrations: *mut [usize; 2],
    _resource_path: *const u16,
    _resource_name: *const u16,
    handle: *mut u64,
) -> u32 {
    if callback == 0 || control.is_null() || handle.is_null() {
        return ERROR_INVALID_PARAMETER;
    }
    // SAFETY: the caller's output.
    unsafe { *handle = NEXT.fetch_add(1, Ordering::Relaxed) };
    0
}

#[unsafe(no_mangle)]
pub extern "system" fn UnregisterTraceGuids(_handle: u64) -> u32 {
    0
}

/// GetTraceLoggerHandle: only the control callback's buffer names a
/// session, and it is never called: INVALID_HANDLE_VALUE.
#[unsafe(no_mangle)]
pub extern "system" fn GetTraceLoggerHandle(_buffer: *mut core::ffi::c_void) -> u64 {
    // SAFETY: plain call.
    unsafe { SetLastError(ERROR_INVALID_HANDLE) };
    u64::MAX
}

/// GetTraceEnableFlags / GetTraceEnableLevel of a session handle there
/// cannot be.
#[unsafe(no_mangle)]
pub extern "system" fn GetTraceEnableFlags(_session: u64) -> u32 {
    // SAFETY: plain call.
    unsafe { SetLastError(ERROR_INVALID_HANDLE) };
    0
}

#[unsafe(no_mangle)]
pub extern "system" fn GetTraceEnableLevel(_session: u64) -> u8 {
    // SAFETY: plain call.
    unsafe { SetLastError(ERROR_INVALID_HANDLE) };
    0
}

/// TraceEvent: no session to log to.
#[unsafe(no_mangle)]
pub extern "system" fn TraceEvent(_session: u64, _header: *mut u8) -> u32 {
    ERROR_INVALID_HANDLE
}

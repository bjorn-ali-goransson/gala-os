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

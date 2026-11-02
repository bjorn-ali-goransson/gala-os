//! gala's mpr.dll (network connections, WNet). gala has no network
//! drives: WNetGetConnectionW says so. The other functions programs import
//! report themselves to the missing-features ledger and end the process
//! when called (`gala_stubs`).

#![no_std]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

mod unimplemented;

/// MSVC-target codegen references this marker symbol directly, so it cannot
/// come from an import.
#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

const ERROR_BAD_DEVICE: u32 = 1200;
const ERROR_NOT_CONNECTED: u32 = 2250;

/// WNetGetConnectionW(local, remote, length): the network name behind a
/// local device. A drive ("X:") is never a network connection here
/// (ERROR_NOT_CONNECTED); any other name is not a device
/// (ERROR_BAD_DEVICE).
#[unsafe(no_mangle)]
pub extern "system" fn WNetGetConnectionW(
    local: *const u16,
    _remote: *mut u16,
    _length: *mut u32,
) -> u32 {
    if local.is_null() {
        return ERROR_BAD_DEVICE;
    }
    // SAFETY: the caller's NUL-terminated name (read up to 3 characters).
    let (a, b, c) = unsafe {
        let a = *local;
        let b = if a == 0 { 0 } else { *local.add(1) };
        let c = if b == 0 { 0 } else { *local.add(2) };
        (a, b, c)
    };
    let letter = (u16::from(b'A')..=u16::from(b'Z')).contains(&(a & !0x20));
    if letter && b == u16::from(b':') && c == 0 { ERROR_NOT_CONNECTED } else { ERROR_BAD_DEVICE }
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

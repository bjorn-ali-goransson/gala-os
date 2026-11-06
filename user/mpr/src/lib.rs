//! gala's mpr.dll (network connections, WNet). gala has no network
//! drives: WNetGetConnectionW says so. The other functions programs import
//! report themselves to the missing-features ledger and end the process
//! when called (`gala_stubs`).

#![no_std]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

mod unimplemented;

// The 32-bit build (SysWOW64) takes the memory routines, 64-bit
// division helpers and unwind personality from ntrt's x86 runtime
// (ARCHITECTURE "Plan: WOW64").
#[cfg(target_arch = "x86")]
use ntrt as _;

/// MSVC-target codegen references this marker symbol directly, so it cannot
/// come from an import.
#[cfg(target_arch = "x86_64")]
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

const ERROR_NO_NETWORK: u32 = 1222;

/// WNetAddConnection2W(resource, password, user, flags): gala has no
/// network provider (ERROR_NO_NETWORK).
#[unsafe(no_mangle)]
pub extern "system" fn WNetAddConnection2W(
    _resource: *const u8,
    _password: *const u16,
    _user: *const u16,
    _flags: u32,
) -> u32 {
    ERROR_NO_NETWORK
}

/// WNetCancelConnection2W(name, flags, force): nothing is connected
/// (ERROR_NOT_CONNECTED).
#[unsafe(no_mangle)]
pub extern "system" fn WNetCancelConnection2W(_name: *const u16, _flags: u32, _force: i32) -> u32 {
    ERROR_NOT_CONNECTED
}

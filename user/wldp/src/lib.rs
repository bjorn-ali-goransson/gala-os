//! gala's wldp.dll (Windows Lockdown Policy, wldp.h): gala enforces no
//! code integrity policy and is no Windows in S mode, so every query
//! answers as an unlocked Windows without App Control (Device Guard) does:
//! no lockdown, nothing in force. ODT's setup.exe loads it to check for S
//! mode. The answers are those Wine's dlls/wldp/tests check on Windows.

#![no_std]
#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

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

const S_OK: i32 = 0;
const E_INVALIDARG: i32 = 0x8007_0057_u32 as i32;
/// WLDP_LOCKDOWN_DEFINED_FLAG: the state is defined, with no CONFIG_CI,
/// UMCIENFORCE or AUDIT flag (no policy in force).
const WLDP_LOCKDOWN_DEFINED_FLAG: u32 = 0x8000_0000;
/// WLDP_WINDOWS_LOCKDOWN_MODE_UNLOCKED: not in S mode.
const WLDP_WINDOWS_LOCKDOWN_MODE_UNLOCKED: u32 = 0;

/// WldpQueryWindowsLockdownMode(*lockdownMode): S_OK, UNLOCKED (a null
/// pointer: E_INVALIDARG, here and below; not measured on Windows).
#[unsafe(no_mangle)]
pub extern "system" fn WldpQueryWindowsLockdownMode(mode: *mut u32) -> i32 {
    if mode.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's WLDP_WINDOWS_LOCKDOWN_MODE.
    unsafe { mode.write(WLDP_WINDOWS_LOCKDOWN_MODE_UNLOCKED) };
    S_OK
}

/// WldpGetLockdownPolicy(*hostInformation, *lockdownState, lockdownFlags):
/// the lockdown state of any host (script engines, MSI, the global one),
/// WLDP_LOCKDOWN_DEFINED_FLAG alone; S_OK. Without host information
/// Windows answers the same state with 0x10000000 (Wine's tests).
#[unsafe(no_mangle)]
pub extern "system" fn WldpGetLockdownPolicy(host: *const u8, state: *mut u32, _flags: u32) -> i32 {
    if state.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's DWORD.
    unsafe { state.write(WLDP_LOCKDOWN_DEFINED_FLAG) };
    if host.is_null() { 0x1000_0000 } else { S_OK }
}

/// WldpIsDynamicCodePolicyEnabled(*isEnabled): FALSE, S_OK (no policy
/// restricts dynamic code).
#[unsafe(no_mangle)]
pub extern "system" fn WldpIsDynamicCodePolicyEnabled(enabled: *mut i32) -> i32 {
    if enabled.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's BOOL.
    unsafe { enabled.write(0) };
    S_OK
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

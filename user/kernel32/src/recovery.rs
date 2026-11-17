//! Application recovery: the callback Windows Error Reporting runs
//! before it ends a program that hung or faulted, as Microsoft documents
//! RegisterApplicationRecoveryCallback and its three companions. gala has
//! no error reporting (ROADMAP OF), so a registration is kept and never
//! called and no process is ever recovering: ApplicationRecoveryInProgress
//! answers E_FAIL, which is Windows' answer outside a recovery, and a
//! registration is noted in the ledger so that a program which waited to
//! be recovered leaves a trace. Word's mso chain imports all four.

use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use super::*;

const S_OK: i32 = 0;
const E_FAIL: i32 = 0x8000_4005_u32 as i32;
const E_INVALIDARG: i32 = 0x8007_0057_u32 as i32;

/// RECOVERY_DEFAULT_PING_INTERVAL, and the longest interval Windows takes
/// (five minutes; 400 seconds is E_INVALIDARG, measured).
const DEFAULT_PING: u32 = 5000;
const MAX_PING: u32 = 5 * 60 * 1000;

/// The process's registration: the callback (none is 0), the parameter it
/// is to be called with and the ping interval it asked for.
static CALLBACK: AtomicUsize = AtomicUsize::new(0);
static PARAMETER: AtomicUsize = AtomicUsize::new(0);
static PING: AtomicU32 = AtomicU32::new(0);

/// APPLICATION_RECOVERY_CALLBACK.
type RecoveryCallback = extern "system" fn(*mut c_void) -> u32;

/// RegisterApplicationRecoveryCallback(callback, parameter, ping, flags):
/// what to run if this process is about to be ended. The flags are
/// reserved (0), and the interval is five minutes at most.
#[unsafe(no_mangle)]
pub extern "system" fn RegisterApplicationRecoveryCallback(
    callback: Option<RecoveryCallback>,
    parameter: *mut c_void,
    ping: u32,
    flags: u32,
) -> i32 {
    let Some(callback) = callback else { return E_INVALIDARG };
    if flags != 0 || ping > MAX_PING {
        return E_INVALIDARG;
    }
    CALLBACK.store(callback as usize, Ordering::SeqCst);
    PARAMETER.store(parameter as usize, Ordering::SeqCst);
    let ping = if ping == 0 { DEFAULT_PING } else { ping };
    PING.store(ping, Ordering::SeqCst);
    note(b"kernel32.dll!RegisterApplicationRecoveryCallback (nothing runs it)", ping.into());
    S_OK
}

/// UnregisterApplicationRecoveryCallback: forget the registration.
#[unsafe(no_mangle)]
pub extern "system" fn UnregisterApplicationRecoveryCallback() -> i32 {
    CALLBACK.store(0, Ordering::SeqCst);
    PARAMETER.store(0, Ordering::SeqCst);
    PING.store(0, Ordering::SeqCst);
    S_OK
}

/// ApplicationRecoveryInProgress(cancelled): whether the user has
/// cancelled the recovery under way. A caller that is not being recovered
/// gets E_FAIL, as on Windows (measured).
#[unsafe(no_mangle)]
pub extern "system" fn ApplicationRecoveryInProgress(cancelled: *mut Bool) -> i32 {
    if cancelled.is_null() {
        return E_INVALIDARG;
    }
    E_FAIL
}

/// ApplicationRecoveryFinished(success): the end of a recovery. With none
/// under way there is nothing to end, and the process lives on.
#[unsafe(no_mangle)]
pub extern "system" fn ApplicationRecoveryFinished(_success: Bool) {}

//! Group policy notifications and the profile type (userenv.h), which
//! Chromium's browser process asks for at start-up (its policy loader
//! watches for group policy changes). gala has no group policy: nothing
//! is ever applied, so a registered event is never signaled, and the
//! "critical policy section" is a lock nobody else takes.

use core::sync::atomic::{AtomicU32, Ordering};

/// Registrations not yet undone (for the record).
static REGISTERED: AtomicU32 = AtomicU32::new(0);

/// RegisterGPNotification(hEvent, bMachine): TRUE; the event would be
/// signaled when policy is applied, which never happens on gala.
#[unsafe(no_mangle)]
pub extern "system" fn RegisterGPNotification(event: usize, _machine: i32) -> i32 {
    if event == 0 {
        invalid();
        return 0;
    }
    REGISTERED.fetch_add(1, Ordering::Relaxed);
    1
}

/// UnregisterGPNotification(hEvent).
#[unsafe(no_mangle)]
pub extern "system" fn UnregisterGPNotification(event: usize) -> i32 {
    if event == 0 {
        invalid();
        return 0;
    }
    REGISTERED.fetch_sub(1, Ordering::Relaxed);
    1
}

/// The handle EnterCriticalPolicySection hands out (any non-null value
/// the caller only passes back).
const POLICY_SECTION: usize = 0x6770_0001;

/// EnterCriticalPolicySection(bMachine): a handle for
/// LeaveCriticalPolicySection; no policy is ever being applied, so it
/// never waits.
#[unsafe(no_mangle)]
pub extern "system" fn EnterCriticalPolicySection(_machine: i32) -> usize {
    POLICY_SECTION
}

#[unsafe(no_mangle)]
pub extern "system" fn LeaveCriticalPolicySection(section: usize) -> i32 {
    if section != POLICY_SECTION {
        invalid();
        return 0;
    }
    1
}

/// GetProfileType(*dwFlags): 0, a local profile (not temporary, roaming
/// or mandatory).
#[unsafe(no_mangle)]
pub extern "system" fn GetProfileType(flags: *mut u32) -> i32 {
    if flags.is_null() {
        invalid();
        return 0;
    }
    // SAFETY: the caller's output.
    unsafe { *flags = 0 };
    1
}

/// Sets ERROR_INVALID_PARAMETER.
fn invalid() {
    // SAFETY: plain call.
    unsafe { super::SetLastError(87) };
}

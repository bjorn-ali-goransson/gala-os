//! Power notifications (winuser.h): RegisterSuspendResumeNotification and
//! RegisterPowerSettingNotification, which Chromium's power monitor calls
//! at start-up. gala neither suspends nor changes power settings, so a
//! registration only has to be kept until it is undone: no
//! WM_POWERBROADCAST is ever sent for it.

use core::sync::atomic::{AtomicUsize, Ordering};

/// Registrations not yet undone; each HPOWERNOTIFY is a distinct non-null
/// value.
static NEXT: AtomicUsize = AtomicUsize::new(0x5057_0000);
static LIVE: AtomicUsize = AtomicUsize::new(0);

fn register(recipient: usize) -> usize {
    if recipient == 0 {
        invalid();
        return 0;
    }
    LIVE.fetch_add(1, Ordering::Relaxed);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

fn unregister(handle: usize) -> i32 {
    if handle == 0 {
        invalid();
        return 0;
    }
    LIVE.fetch_sub(1, Ordering::Relaxed);
    1
}

/// RegisterSuspendResumeNotification(hRecipient, Flags): a window handle
/// (DEVICE_NOTIFY_WINDOW_HANDLE) or a callback (DEVICE_NOTIFY_CALLBACK).
#[unsafe(no_mangle)]
pub extern "system" fn RegisterSuspendResumeNotification(recipient: usize, _flags: u32) -> usize {
    register(recipient)
}

#[unsafe(no_mangle)]
pub extern "system" fn UnregisterSuspendResumeNotification(handle: usize) -> i32 {
    unregister(handle)
}

/// RegisterPowerSettingNotification(hRecipient, PowerSettingGuid, Flags).
#[unsafe(no_mangle)]
pub extern "system" fn RegisterPowerSettingNotification(
    recipient: usize,
    _setting: *const u8,
    _flags: u32,
) -> usize {
    register(recipient)
}

#[unsafe(no_mangle)]
pub extern "system" fn UnregisterPowerSettingNotification(handle: usize) -> i32 {
    unregister(handle)
}

/// Sets ERROR_INVALID_PARAMETER.
fn invalid() {
    // SAFETY: plain call.
    unsafe { super::SetLastError(87) };
}

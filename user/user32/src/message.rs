//! Registered window messages (RegisterWindowMessageW) and clipboard
//! formats (RegisterClipboardFormatW, from the same table): one number in
//! 0xC000..=0xFFFF per name, compared case-insensitively, the same for
//! every call in the process (Windows shares them system-wide; gala's
//! user32 sees one process). Names keep the first registration's
//! spelling (GetClipboardFormatNameW).

use core::sync::atomic::{AtomicBool, Ordering};

const MAX: usize = 256;
const LEN: usize = 64;

struct Table {
    lock: AtomicBool,
    names: core::cell::UnsafeCell<[[u16; LEN]; MAX]>,
    count: core::cell::UnsafeCell<usize>,
}

// SAFETY: every access is under `lock`.
unsafe impl Sync for Table {}

static TABLE: Table = Table {
    lock: AtomicBool::new(false),
    names: core::cell::UnsafeCell::new([[0; LEN]; MAX]),
    count: core::cell::UnsafeCell::new(0),
};

fn upper(c: u16) -> u16 {
    if (u16::from(b'a')..=u16::from(b'z')).contains(&c) { c - 32 } else { c }
}

/// RegisterWindowMessageW: the name's message (0 for an empty name or a
/// full table).
#[unsafe(no_mangle)]
pub extern "system" fn RegisterWindowMessageW(name: *const u16) -> u32 {
    if name.is_null() {
        return 0;
    }
    let mut key = [0u16; LEN];
    for (i, k) in key.iter_mut().take(LEN - 1).enumerate() {
        // SAFETY: the caller's NUL-terminated string.
        let c = unsafe { *name.add(i) };
        if c == 0 {
            break;
        }
        *k = c;
    }
    if key[0] == 0 {
        crate::window::set_last_error(87);
        return 0;
    }
    while TABLE.lock.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        core::hint::spin_loop();
    }
    // SAFETY: under the lock.
    let r = unsafe {
        let names = &mut *TABLE.names.get();
        let count = &mut *TABLE.count.get();
        let same = |n: &[u16; LEN]| n.iter().zip(&key).all(|(&a, &b)| upper(a) == upper(b));
        match names[..*count].iter().position(same) {
            Some(i) => 0xc000 + i as u32,
            None if *count < MAX => {
                names[*count] = key;
                *count += 1;
                0xc000 + (*count - 1) as u32
            }
            None => 0,
        }
    };
    TABLE.lock.store(false, Ordering::Release);
    r
}

#[unsafe(no_mangle)]
pub extern "system" fn RegisterWindowMessageA(name: *const u8) -> u32 {
    let mut w = [0u16; LEN];
    RegisterWindowMessageW(crate::window::ansi_into_ptr(name, &mut w))
}

/// RegisterClipboardFormatW: a registered format (0xC000..=0xFFFF), from
/// the registered messages' table.
#[unsafe(no_mangle)]
pub extern "system" fn RegisterClipboardFormatW(name: *const u16) -> u32 {
    RegisterWindowMessageW(name)
}

#[unsafe(no_mangle)]
pub extern "system" fn RegisterClipboardFormatA(name: *const u8) -> u32 {
    RegisterWindowMessageA(name)
}

/// GetClipboardFormatNameW(format, buffer, max): a registered format's
/// name as first registered, cut to `max - 1` characters; its length,
/// or 0 (predefined formats have no names).
#[unsafe(no_mangle)]
pub extern "system" fn GetClipboardFormatNameW(format: u32, out: *mut u16, max: i32) -> i32 {
    if out.is_null() || max <= 0 {
        return 0;
    }
    while TABLE.lock.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        core::hint::spin_loop();
    }
    // SAFETY: under the lock; the caller's buffer of `max` characters.
    let n = unsafe {
        let names = &*TABLE.names.get();
        let count = *TABLE.count.get();
        match format.checked_sub(0xc000).map(|i| i as usize).filter(|&i| i < count) {
            Some(i) => {
                let name = &names[i];
                let len = name.iter().position(|&c| c == 0).unwrap_or(LEN).min(max as usize - 1);
                core::ptr::copy_nonoverlapping(name.as_ptr(), out, len);
                *out.add(len) = 0;
                len as i32
            }
            None => 0,
        }
    };
    TABLE.lock.store(false, Ordering::Release);
    if n == 0 {
        crate::window::set_last_error(87);
    }
    n
}

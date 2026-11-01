//! Palettes: kept, selected and queried; on gala's true-colour display
//! they do not change drawing (RealizePalette maps nothing,
//! GetNearestColor is the colour itself). Entries live in a side table
//! (32 palettes of up to 256 entries); the object holds its index.

use crate::{Handle, Object, handle, new_object, slot_of, with, with_dc};
use core::sync::atomic::{AtomicBool, Ordering};

const PALETTES: usize = 32;

struct Store {
    lock: AtomicBool,
    used: core::cell::UnsafeCell<[bool; PALETTES]>,
    entries: core::cell::UnsafeCell<[[u32; 256]; PALETTES]>,
    counts: core::cell::UnsafeCell<[u16; PALETTES]>,
}

// SAFETY: every access is under `lock`.
unsafe impl Sync for Store {}

static STORE: Store = Store {
    lock: AtomicBool::new(false),
    used: core::cell::UnsafeCell::new([false; PALETTES]),
    entries: core::cell::UnsafeCell::new([[0; 256]; PALETTES]),
    counts: core::cell::UnsafeCell::new([0; PALETTES]),
};

fn store<R>(
    f: impl FnOnce(&mut [bool; PALETTES], &mut [[u32; 256]; PALETTES], &mut [u16; PALETTES]) -> R,
) -> R {
    while STORE.lock.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        core::hint::spin_loop();
    }
    // SAFETY: under the lock.
    let r =
        unsafe { f(&mut *STORE.used.get(), &mut *STORE.entries.get(), &mut *STORE.counts.get()) };
    STORE.lock.store(false, Ordering::Release);
    r
}

/// The palette store index of palette `h`.
fn index(h: Handle) -> Option<usize> {
    let i = slot_of(h)?;
    with(|slots, _| match slots[i] {
        Object::Palette(p) => Some(p),
        _ => None,
    })
}

/// A palette from PALETTEENTRYs (red, green, blue, flags bytes).
pub fn make(entries: &[u32]) -> Handle {
    let Some(p) = store(|used, store, counts| {
        let p = used.iter().position(|u| !u)?;
        used[p] = true;
        counts[p] = entries.len().min(256) as u16;
        store[p] = [0; 256];
        store[p][..entries.len().min(256)].copy_from_slice(&entries[..entries.len().min(256)]);
        Some(p)
    }) else {
        return core::ptr::null_mut();
    };
    let h = new_object(Object::Palette(p));
    if h.is_null() {
        store(|used, _, _| used[p] = false);
    }
    h
}

/// Frees a palette's entries (DeleteObject).
pub fn free(p: usize) {
    store(|used, _, _| used[p] = false);
}

/// The entry count (GetObject).
pub fn count(p: usize) -> u16 {
    store(|_, _, counts| counts[p])
}

/// LOGPALETTE: version, count, then the entries.
#[unsafe(no_mangle)]
pub extern "system" fn CreatePalette(lp: *const u16) -> Handle {
    if lp.is_null() {
        return core::ptr::null_mut();
    }
    // SAFETY: the caller's LOGPALETTE with `count` entries.
    let entries = unsafe {
        let n = (*lp.add(1)).min(256) as usize;
        core::slice::from_raw_parts(lp.add(2).cast::<u32>(), n)
    };
    let mut copy = [0u32; 256];
    for (d, &e) in copy.iter_mut().zip(entries) {
        *d = e;
    }
    make(&copy[..entries.len()])
}

#[unsafe(no_mangle)]
pub extern "system" fn GetPaletteEntries(h: Handle, start: u32, n: u32, out: *mut u32) -> u32 {
    let Some(p) = index(h) else { return 0 };
    store(|_, entries, counts| {
        let count = u32::from(counts[p]);
        if out.is_null() {
            return count;
        }
        let end = (start + n).min(count);
        for k in start..end {
            // SAFETY: the caller's `n` entries.
            unsafe { *out.add((k - start) as usize) = entries[p][k as usize] };
        }
        end.saturating_sub(start)
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn SetPaletteEntries(h: Handle, start: u32, n: u32, data: *const u32) -> u32 {
    let Some(p) = index(h) else { return 0 };
    store(|_, entries, counts| {
        let end = (start + n).min(u32::from(counts[p]));
        for k in start..end {
            // SAFETY: the caller's `n` entries.
            entries[p][k as usize] = unsafe { *data.add((k - start) as usize) };
        }
        end.saturating_sub(start)
    })
}

/// The entry nearest a COLORREF (squared distance).
#[unsafe(no_mangle)]
pub extern "system" fn GetNearestPaletteIndex(h: Handle, color: u32) -> u32 {
    let Some(p) = index(h) else { return 0xffff_ffff };
    let (r, g, b) =
        ((color & 0xff) as i32, ((color >> 8) & 0xff) as i32, ((color >> 16) & 0xff) as i32);
    store(|_, entries, counts| {
        (0..usize::from(counts[p]))
            .min_by_key(|&k| {
                let e = entries[p][k];
                let (er, eg, eb) =
                    ((e & 0xff) as i32, ((e >> 8) & 0xff) as i32, ((e >> 16) & 0xff) as i32);
                (er - r).pow(2) + (eg - g).pow(2) + (eb - b).pow(2)
            })
            .map_or(0xffff_ffff, |k| k as u32)
    })
}

/// SelectPalette: the previous palette of the DC.
#[unsafe(no_mangle)]
pub extern "system" fn SelectPalette(hdc: Handle, pal: Handle, _background: i32) -> Handle {
    let Some(new) = slot_of(pal).filter(|_| index(pal).is_some()) else {
        return core::ptr::null_mut();
    };
    with_dc(hdc, |dc, _| handle(core::mem::replace(&mut dc.palette, new)))
        .unwrap_or(core::ptr::null_mut())
}

/// RealizePalette: nothing to map on a true-colour display (0 entries).
#[unsafe(no_mangle)]
pub extern "system" fn RealizePalette(hdc: Handle) -> u32 {
    with_dc(hdc, |_, _| 0).unwrap_or(0xffff_ffff)
}

/// GetNearestColor: the colour itself on a true-colour DC.
#[unsafe(no_mangle)]
pub extern "system" fn GetNearestColor(hdc: Handle, color: u32) -> u32 {
    with_dc(hdc, |_, _| color & 0x00ff_ffff).unwrap_or(0xffff_ffff)
}

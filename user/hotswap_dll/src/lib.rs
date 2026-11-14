//! The DLL the hot swap scenarios patch (ROADMAP SN step 1d; xtask
//! `hotswap_machine`, `hotswap_stack`; WORKFLOW "Hot swapping DLLs into a
//! snapshot (at your own risk)"). Built three ways from this source: the
//! first build (no feature), the one swapped in (`v2`) and one whose
//! static changed size (`layout`, refused). hotswap_test uses it; on
//! Windows (and in a run that is not resumed) it is the first build.
//!
//! What `v2` changes, and what each change shows after a hot swap:
//! - `hs_version`: 1 becomes 2, through `version_v2` (a function only
//!   `v2` has) and `CALLS` (a static only `v2` has: fresh storage);
//! - `hs_get_global`: adds 1000 to `GLOBAL`, which the program set
//!   before the snapshot: the new code reads the old image's static;
//! - `obj_get`, the method of an object made before the snapshot: the
//!   call through its vtable (the old image's `VTBL`) reaches new code;
//! - `hs_call_back`: adds 2 instead of 1 to the callback's value; when
//!   the snapshot is taken inside the callback it is on the stack, and
//!   the swap is refused;
//! - `hs_calls`: the number of `hs_version` calls since the swap.

#![no_std]

use core::ptr::{addr_of, addr_of_mut, read_volatile, write_volatile};

/// Set by the program before the snapshot (kept across the swap).
static mut GLOBAL: u64 = 0;

/// Scratch the program's values also go to; `layout` makes it larger.
#[cfg(not(feature = "layout"))]
static mut TABLE: [u64; 4] = [0; 4];
#[cfg(feature = "layout")]
static mut TABLE: [u64; 8] = [0; 8];

/// `v2`'s own static: how often the new `hs_version` ran.
#[cfg(feature = "v2")]
static mut CALLS: u64 = 0;

/// A COM-style object: its vtable first.
#[repr(C)]
pub struct Obj {
    vtbl: *const Vtbl,
    value: u32,
}

#[repr(C)]
pub struct Vtbl {
    get: extern "system" fn(*const Obj) -> u32,
}

// SAFETY: an immutable table of function pointers.
unsafe impl Sync for Vtbl {}

static VTBL: Vtbl = Vtbl { get: obj_get };

#[inline(never)]
extern "system" fn obj_get(this: *const Obj) -> u32 {
    // SAFETY: `this` is an object `hs_create` filled in; GLOBAL as below
    // (read so the function is long enough for the jump: a 4-byte one is
    // refused).
    let (value, g) =
        unsafe { (read_volatile(addr_of!((*this).value)), read_volatile(addr_of!(GLOBAL))) };
    let value = value + (g >> 63) as u32;
    if cfg!(feature = "v2") { value + 100 } else { value }
}

/// Makes the object at `obj` (the caller's memory).
///
/// # Safety
/// `obj` is valid for writing an `Obj`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hs_create(obj: *mut Obj, value: u32) {
    // SAFETY: the caller passes memory for an `Obj`.
    unsafe {
        write_volatile(addr_of_mut!((*obj).vtbl), &VTBL);
        write_volatile(addr_of_mut!((*obj).value), value);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn hs_set_global(value: u64) {
    // SAFETY: the test's one thread.
    unsafe {
        write_volatile(addr_of_mut!(GLOBAL), value);
        let table = &raw mut TABLE;
        let n = (*table).len();
        write_volatile(&mut (*table)[(value as usize) % n], value);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn hs_get_global() -> u64 {
    // SAFETY: as above.
    let g = unsafe { read_volatile(addr_of!(GLOBAL)) };
    if cfg!(feature = "v2") { g + 1000 } else { g }
}

#[cfg(feature = "v2")]
#[inline(never)]
fn version_v2() -> u32 {
    // SAFETY: as above.
    unsafe {
        let calls = read_volatile(addr_of!(CALLS));
        write_volatile(addr_of_mut!(CALLS), calls + 1);
    }
    core::hint::black_box(2)
}

#[unsafe(no_mangle)]
pub extern "C" fn hs_version() -> u32 {
    #[cfg(feature = "v2")]
    let v = version_v2();
    #[cfg(not(feature = "v2"))]
    let v = core::hint::black_box(1);
    // Something to read, so the function is long enough for the jump.
    // SAFETY: as above.
    let g = unsafe { read_volatile(addr_of!(GLOBAL)) };
    v + (g >> 63) as u32
}

#[unsafe(no_mangle)]
pub extern "C" fn hs_calls() -> u64 {
    #[cfg(feature = "v2")]
    // SAFETY: as above.
    let calls = unsafe { read_volatile(addr_of!(CALLS)) };
    #[cfg(not(feature = "v2"))]
    let calls = 0;
    // SAFETY: as above.
    let g = unsafe { read_volatile(addr_of!(GLOBAL)) };
    calls + (g >> 63)
}

/// Calls `cb` and adds to its value (not a tail call: the DLL's frame is
/// on the stack while `cb` runs).
#[unsafe(no_mangle)]
pub extern "C" fn hs_call_back(cb: extern "C" fn() -> u32) -> u32 {
    let v = core::hint::black_box(cb)();
    if cfg!(feature = "v2") { v + 2 } else { v + 1 }
}

/// Entry point required by our link flags (DllMain).
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

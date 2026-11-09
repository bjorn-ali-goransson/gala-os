//! The UCRT's bounds-checked time conversions (`_gmtime64_s` and
//! friends): msvcrt's conversions (time.rs: gala's local time is UTC)
//! into the caller's `struct tm`; EINVAL through the invalid-parameter
//! handler for a NULL argument or a time before 1970 or past 3000-12-31.

use super::{EINVAL, invalid_parameter};
use crate::time::Tm;

/// `_MAX__TIME64_T`: 3000-12-31 23:59:59 UTC.
const MAX_TIME64: i64 = 0x7_9340_6fff;

/// The `_s` rule: `convert(t)` into `*tm`, or EINVAL (the `struct tm`
/// filled with -1 when it exists).
fn convert_s(tm: *mut Tm, t: Option<i64>, convert: impl FnOnce(i64) -> Tm) -> i32 {
    if tm.is_null() {
        invalid_parameter();
        return EINVAL;
    }
    let Some(t) = t.filter(|t| (0..=MAX_TIME64).contains(t)) else {
        // SAFETY: the caller's struct tm, nine ints.
        unsafe { core::ptr::write_bytes(tm.cast::<u8>(), 0xff, size_of::<Tm>()) };
        invalid_parameter();
        return EINVAL;
    };
    // SAFETY: as above.
    unsafe { *tm = convert(t) };
    0
}

fn utc(t: i64) -> Tm {
    // SAFETY: msvcrt's static result, copied out at once.
    unsafe { *crate::time::_gmtime64(&t) }
}

fn local(t: i64) -> Tm {
    // SAFETY: as above.
    unsafe { *crate::time::_localtime64(&t) }
}

/// Reads the caller's time (NULL: None).
fn read<T: Copy + Into<i64>>(t: *const T) -> Option<i64> {
    // SAFETY: the caller's time, if any.
    (!t.is_null()).then(|| unsafe { *t }.into())
}

#[unsafe(no_mangle)]
pub extern "C" fn _gmtime64_s(tm: *mut Tm, t: *const i64) -> i32 {
    convert_s(tm, read(t), utc)
}

#[unsafe(no_mangle)]
pub extern "C" fn _localtime64_s(tm: *mut Tm, t: *const i64) -> i32 {
    convert_s(tm, read(t), local)
}

#[unsafe(no_mangle)]
pub extern "C" fn _gmtime32_s(tm: *mut Tm, t: *const i32) -> i32 {
    convert_s(tm, read(t), utc)
}

#[unsafe(no_mangle)]
pub extern "C" fn _localtime32_s(tm: *mut Tm, t: *const i32) -> i32 {
    convert_s(tm, read(t), local)
}

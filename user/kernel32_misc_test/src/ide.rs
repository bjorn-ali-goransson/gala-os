//! What Inno Setup's IDE (ISIDE.exe) imports from kernel32 besides the
//! rest, as Microsoft's pages for FileTimeToDosDateTime and
//! FindStringOrdinal describe them and Wine's `dlls/kernel32/tests/time.c`
//! and `locale.c` check them on Windows:
//!
//! - FileTimeToDosDateTime: a time before 1980 (FILETIME 0) or a NULL
//!   output is ERROR_INVALID_PARAMETER; the MS-DOS time counts two-second
//!   steps, rounded up (10:20:22.001 is 10:20:24, 10:20:59 is 10:21:00,
//!   10:20:01 is 10:20:02, 10:20:22 stays); the date packs the year since
//!   1980, the month and the day.
//! - FindStringOrdinal: the position of the value in the source (from the
//!   start, from the end, as a prefix, as a suffix), by code unit, with or
//!   without case; -1 when absent; -1 for a length of -1 means
//!   NUL-terminated; an empty value is found at 0. A successful search
//!   (found or not) sets the last error to 0; bad flags are
//!   ERROR_INVALID_FLAGS, a NULL string ERROR_INVALID_PARAMETER.

use super::Checks;

type Bool = i32;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn FileTimeToDosDateTime(ft: *const u64, date: *mut u16, time: *mut u16) -> Bool;
    fn SystemTimeToFileTime(st: *const u16, ft: *mut u64) -> Bool;
    fn FindStringOrdinal(
        flags: u32,
        source: *const u16,
        source_len: i32,
        value: *const u16,
        value_len: i32,
        ignore_case: Bool,
    ) -> i32;
    fn SetLastError(e: u32);
    fn GetLastError() -> u32;
}

const ERROR_INVALID_PARAMETER: u64 = 87;
const ERROR_INVALID_FLAGS: u64 = 1004;
const FIND_STARTSWITH: u32 = 0x0010_0000;
const FIND_ENDSWITH: u32 = 0x0020_0000;
const FIND_FROMSTART: u32 = 0x0040_0000;
const FIND_FROMEND: u32 = 0x0080_0000;

/// The MS-DOS date and time of 2000-01-01 at 10:20:`second`.`ms`.
fn dos(c: &mut Checks, second: u16, ms: u16) -> Result<(u16, u16), u32> {
    let st = [2000u16, 1, 0, 1, 10, 20, second, ms];
    let (mut ft, mut date, mut time) = (0u64, 0u16, 0u16);
    // SAFETY: valid SYSTEMTIME, FILETIME and outputs.
    unsafe {
        c.ok(SystemTimeToFileTime(st.as_ptr(), &mut ft) != 0, 0)?;
        c.ok(FileTimeToDosDateTime(&ft, &mut date, &mut time) != 0, GetLastError().into())?;
    }
    Ok((date, time))
}

/// Flags, source and length, value and length, ignore case, the result
/// and the last error.
type Case = (u32, *const u16, i32, *const u16, i32, Bool, i32, u64);

pub fn ide(c: &mut Checks) -> Result<(), u32> {
    // FileTimeToDosDateTime.
    let (mut date, mut time) = (0u16, 0u16);
    let zero = 0u64;
    let null = core::ptr::null_mut();
    // SAFETY: valid FILETIME; NULL outputs are what is being checked.
    unsafe {
        for (d, t) in [(null, null), (&mut date as *mut u16, null), (null, &mut time as *mut u16)] {
            SetLastError(0xdead_beef);
            c.eq(FileTimeToDosDateTime(&zero, d, t) as u64, 0)?;
            c.eq(GetLastError().into(), ERROR_INVALID_PARAMETER)?;
        }
        SetLastError(0xdead_beef);
        c.eq(FileTimeToDosDateTime(&zero, &mut date, &mut time) as u64, 0)?;
        c.eq(GetLastError().into(), ERROR_INVALID_PARAMETER)?;
    }
    // (seconds field, minutes field) after rounding up to two seconds.
    for (second, ms, half, minute) in
        [(22, 1, 12, 20), (59, 0, 0, 21), (59, 1, 0, 21), (22, 0, 11, 20), (1, 0, 1, 20)]
    {
        let (d, t) = dos(c, second, ms)?;
        c.eq((t & 0x1f).into(), half)?;
        c.eq(((t >> 5) & 0x3f).into(), minute)?;
        c.eq((t >> 11).into(), 10)?;
        // 2000-01-01: year 20 since 1980, month 1, day 1.
        c.eq(d.into(), (20 << 9) | (1 << 5) | 1)?;
    }

    // FindStringOrdinal.
    let s: [u16; 10] = [97, 98, 99, 49, 50, 51, 97, 66, 99, 0]; // "abc123aBc"
    let abc: [u16; 4] = [97, 98, 99, 0];
    let a_bc: [u16; 4] = [97, 66, 99, 0];
    let aaa: [u16; 4] = [97, 97, 97, 0];
    let n = core::ptr::null();
    #[rustfmt::skip]
    let cases: [Case; 20] = [
        (1, s.as_ptr(), 9, abc.as_ptr(), 3, 0, -1, ERROR_INVALID_FLAGS),
        (FIND_FROMSTART, n, 9, abc.as_ptr(), 3, 0, -1, ERROR_INVALID_PARAMETER),
        (FIND_FROMSTART, s.as_ptr(), 9, n, 3, 0, -1, ERROR_INVALID_PARAMETER),
        (FIND_FROMSTART, s.as_ptr(), 9, n, 0, 0, -1, ERROR_INVALID_PARAMETER),
        (FIND_FROMSTART, n, 0, abc.as_ptr(), 3, 0, -1, ERROR_INVALID_PARAMETER),
        (FIND_FROMSTART, n, 0, n, 0, 0, -1, ERROR_INVALID_PARAMETER),
        (FIND_FROMSTART, s.as_ptr(), 9, abc.as_ptr(), 3, 0, 0, 0),
        (FIND_FROMEND, s.as_ptr(), 9, abc.as_ptr(), 3, 0, 0, 0),
        (FIND_STARTSWITH, s.as_ptr(), 9, abc.as_ptr(), 3, 0, 0, 0),
        (FIND_ENDSWITH, s.as_ptr(), 9, abc.as_ptr(), 3, 0, -1, 0),
        (FIND_FROMSTART, s.as_ptr(), 9, a_bc.as_ptr(), 3, 1, 0, 0),
        (FIND_FROMEND, s.as_ptr(), 9, a_bc.as_ptr(), 3, 1, 6, 0),
        (FIND_STARTSWITH, s.as_ptr(), 9, a_bc.as_ptr(), 3, 1, 0, 0),
        (FIND_ENDSWITH, s.as_ptr(), 9, a_bc.as_ptr(), 3, 1, 6, 0),
        (FIND_FROMSTART, s.as_ptr(), 9, aaa.as_ptr(), 3, 0, -1, 0),
        (FIND_FROMSTART, s.as_ptr(), -1, abc.as_ptr(), 3, 0, 0, 0),
        (FIND_FROMSTART, s.as_ptr(), 9, abc.as_ptr(), -1, 0, 0, 0),
        (FIND_FROMSTART, s.as_ptr(), 0, abc.as_ptr(), 3, 0, -1, 0),
        (FIND_FROMSTART, s.as_ptr(), 9, abc.as_ptr(), 0, 0, 0, 0),
        (FIND_FROMSTART, s.as_ptr(), 0, abc.as_ptr(), 0, 0, 0, 0),
    ];
    for (flags, src, src_len, val, val_len, ignore, want, error) in cases {
        // SAFETY: the strings are NUL-terminated and as long as given.
        unsafe {
            SetLastError(0xdead_beef);
            let got = FindStringOrdinal(flags, src, src_len, val, val_len, ignore);
            c.eq(got as i64 as u64, want as i64 as u64)?;
            c.eq(GetLastError().into(), error)?;
        }
    }
    Ok(())
}

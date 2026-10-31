//! Time conversion: `struct tm` from and to 64-bit Unix time, and
//! strftime. gala's time zone is UTC (kernel32's GetTimeZoneInformation
//! says so too), so local time is UTC with no daylight saving.

/// struct tm.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Tm {
    sec: i32,
    min: i32,
    hour: i32,
    mday: i32,
    mon: i32,
    year: i32,
    wday: i32,
    yday: i32,
    isdst: i32,
}

/// The documented per-thread buffer of gmtime/localtime (per process here).
static mut TM: Tm =
    Tm { sec: 0, min: 0, hour: 0, mday: 0, mon: 0, year: 0, wday: 0, yday: 0, isdst: 0 };

/// Days since 1970-01-01 of a civil date (proleptic Gregorian).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn to_tm(t: i64) -> Tm {
    let days = t.div_euclid(86_400);
    let secs = t.rem_euclid(86_400);
    // Civil from days (the inverse of days_from_civil).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    let yday = days - days_from_civil(y, 1, 1);
    Tm {
        sec: (secs % 60) as i32,
        min: (secs / 60 % 60) as i32,
        hour: (secs / 3600) as i32,
        mday: d as i32,
        mon: (m - 1) as i32,
        year: (y - 1900) as i32,
        wday: (days + 4).rem_euclid(7) as i32, // 1970-01-01 was a Thursday
        yday: yday as i32,
        isdst: 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn _gmtime64(t: *const i64) -> *mut Tm {
    if t.is_null() {
        return core::ptr::null_mut();
    }
    // SAFETY: the caller's time; our static buffer.
    unsafe {
        *(&raw mut TM) = to_tm(*t);
        &raw mut TM
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn _localtime64(t: *const i64) -> *mut Tm {
    _gmtime64(t)
}

/// _mktime64(tm): normalizes the fields and returns the time (UTC).
#[unsafe(no_mangle)]
pub extern "C" fn _mktime64(tm: *mut Tm) -> i64 {
    if tm.is_null() {
        return -1;
    }
    // SAFETY: the caller's struct tm.
    let t = unsafe { &mut *tm };
    let months = i64::from(t.year) * 12 + i64::from(t.mon);
    let (year, mon) = (months.div_euclid(12) + 1900, months.rem_euclid(12));
    let days = days_from_civil(year, mon + 1, 1) + i64::from(t.mday) - 1;
    let time = days * 86_400 + i64::from(t.hour) * 3600 + i64::from(t.min) * 60 + i64::from(t.sec);
    *t = to_tm(time);
    time
}

#[unsafe(no_mangle)]
pub extern "C" fn _mkgmtime64(tm: *mut Tm) -> i64 {
    _mktime64(tm)
}

const DAYS: [&[u8]; 7] =
    [b"Sunday", b"Monday", b"Tuesday", b"Wednesday", b"Thursday", b"Friday", b"Saturday"];
const MONTHS: [&[u8]; 12] = [
    b"January",
    b"February",
    b"March",
    b"April",
    b"May",
    b"June",
    b"July",
    b"August",
    b"September",
    b"October",
    b"November",
    b"December",
];

/// strftime(buf, max, fmt, tm) in the "C" locale; 0 if the result (with
/// its terminator) does not fit.
#[unsafe(no_mangle)]
pub extern "C" fn strftime(buf: *mut u8, max: usize, fmt: *const u8, tm: *const Tm) -> usize {
    if buf.is_null() || fmt.is_null() || tm.is_null() || max == 0 {
        return 0;
    }
    // SAFETY: the caller's format and struct tm.
    let (fmt, t) = unsafe { (crate::c_str(fmt), &*tm) };
    let mut n = 0usize;
    let mut put = |s: &[u8]| -> bool {
        if n + s.len() >= max {
            return false;
        }
        // SAFETY: n + len < max bytes of the caller's buffer.
        unsafe { core::ptr::copy_nonoverlapping(s.as_ptr(), buf.add(n), s.len()) };
        n += s.len();
        true
    };
    let two = |v: i32| [b'0' + (v / 10 % 10) as u8, b'0' + (v % 10) as u8];
    let mut i = 0;
    while i < fmt.len() {
        let c = fmt[i];
        i += 1;
        let ok = if c != b'%' {
            put(&[c])
        } else {
            let Some(&spec) = fmt.get(i) else { break };
            i += 1;
            let wday = DAYS[t.wday.rem_euclid(7) as usize];
            let mon = MONTHS[t.mon.rem_euclid(12) as usize];
            let year = t.year + 1900;
            let y4 = [
                b'0' + (year / 1000 % 10) as u8,
                b'0' + (year / 100 % 10) as u8,
                b'0' + (year / 10 % 10) as u8,
                b'0' + (year % 10) as u8,
            ];
            match spec {
                b'a' => put(&wday[..3]),
                b'A' => put(wday),
                b'b' | b'h' => put(&mon[..3]),
                b'B' => put(mon),
                b'd' => put(&two(t.mday)),
                b'H' => put(&two(t.hour)),
                b'I' => put(&two(if t.hour % 12 == 0 { 12 } else { t.hour % 12 })),
                b'j' => {
                    let d = t.yday + 1;
                    put(&[
                        b'0' + (d / 100 % 10) as u8,
                        b'0' + (d / 10 % 10) as u8,
                        b'0' + (d % 10) as u8,
                    ])
                }
                b'm' => put(&two(t.mon + 1)),
                b'M' => put(&two(t.min)),
                b'p' => put(if t.hour < 12 { b"AM" } else { b"PM" }),
                b'S' => put(&two(t.sec)),
                b'w' => put(&[b'0' + t.wday.rem_euclid(7) as u8]),
                b'y' => put(&two(year % 100)),
                b'Y' => put(&y4),
                b'Z' => put(b"UTC"),
                b'z' => put(b"+0000"),
                b'c' => {
                    // "%m/%d/%y %H:%M:%S" in the C locale.
                    put(&two(t.mon + 1))
                        && put(b"/")
                        && put(&two(t.mday))
                        && put(b"/")
                        && put(&two(year % 100))
                        && put(b" ")
                        && put(&two(t.hour))
                        && put(b":")
                        && put(&two(t.min))
                        && put(b":")
                        && put(&two(t.sec))
                }
                b'%' => put(b"%"),
                other => {
                    crate::missing(b"msvcrt.dll!strftime(conversion)", u64::from(other));
                    true
                }
            }
        };
        if !ok {
            return 0;
        }
    }
    // SAFETY: n < max.
    unsafe { *buf.add(n) = 0 };
    n
}

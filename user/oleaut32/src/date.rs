//! Automation dates (the DATE type, as documented): a double counting
//! days from 30 December 1899 (0.0), its fraction the time of day; before
//! 1899 the integer part is negative and the fraction's magnitude is
//! still the time (-1.25 is 29 December 1899 06:00). Valid from 1 January
//! 100 to 31 December 9999, in the proleptic Gregorian calendar.
//!
//! SystemTimeToVariantTime / VariantTimeToSystemTime convert to and from
//! SYSTEMTIME (to the second; milliseconds are ignored and returned as
//! 0), VarDateFromStr reads en-US date and time strings (the forms in
//! `parse_date`; others are reported to the ledger).

use super::{E_INVALIDARG, HResult, S_OK, missing, put, wide_len};
use crate::variant::DISP_E_TYPEMISMATCH;

/// 1 January 100 and 31 December 9999 as day numbers.
const FIRST_DAY: i64 = -657_434;
const LAST_DAY: i64 = 2_958_465;
const SECONDS_PER_DAY: f64 = 86_400.0;

#[repr(C)]
pub struct SystemTime {
    year: u16,
    month: u16,
    day_of_week: u16,
    day: u16,
    hour: u16,
    minute: u16,
    second: u16,
    milliseconds: u16,
}

fn leap(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn month_days(year: i64, month: i64) -> i64 {
    match month {
        2 if leap(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Days from 1 January of year 1 to 1 January of `year`.
fn year_start(year: i64) -> i64 {
    let y = year - 1;
    y * 365 + y.div_euclid(4) - y.div_euclid(100) + y.div_euclid(400)
}

/// The day number (0 = 30 December 1899) of `day` (which may be 0 or
/// past the month's end: it counts on from the month's first) of
/// `month` (1-12) in `year`.
pub(crate) fn day_number(year: i64, month: i64, day: i64) -> i64 {
    let mut days = year_start(year);
    for m in 1..month {
        days += month_days(year, m);
    }
    days + day - 1 - (year_start(1899) + 363)
}

/// The year, month and day of day number `n`.
fn civil(n: i64) -> (i64, i64, i64) {
    let absolute = n + year_start(1899) + 363; // days since 1 January 1
    // A first guess from the mean year length, then corrected.
    let mut year = absolute * 400 / 146_097 + 1;
    while year_start(year) > absolute {
        year -= 1;
    }
    while year_start(year + 1) <= absolute {
        year += 1;
    }
    let mut rest = absolute - year_start(year);
    let mut month = 1;
    while rest >= month_days(year, month) {
        rest -= month_days(year, month);
        month += 1;
    }
    (year, month, rest + 1)
}

/// A two-digit year as Windows 10 1903 and later read it: 0-49 are
/// 2000-2049, 50-99 are 1950-1999.
pub(crate) fn full_year(year: i64) -> i64 {
    match year {
        0..=49 => year + 2000,
        50..=99 => year + 1900,
        _ => year,
    }
}

/// A DATE from a day number and seconds into the day.
pub(crate) fn date_value(day: i64, seconds: i64) -> f64 {
    let time = seconds as f64 / SECONDS_PER_DAY;
    if day < 0 { day as f64 - time } else { day as f64 + time }
}

/// SystemTimeToVariantTime: TRUE and the DATE, or FALSE for a month
/// outside 1-12, a day over 31 or a date outside the DATE range.
#[unsafe(no_mangle)]
pub extern "system" fn SystemTimeToVariantTime(st: *const SystemTime, out: *mut f64) -> i32 {
    if st.is_null() || out.is_null() {
        return 0;
    }
    // SAFETY: the caller's SYSTEMTIME.
    let st = unsafe { &*st };
    if !(1..=12).contains(&st.month) || st.day > 31 || st.year > 9999 {
        return 0;
    }
    let day = day_number(full_year(st.year.into()), st.month.into(), st.day.into());
    if !(FIRST_DAY..=LAST_DAY).contains(&day) {
        return 0;
    }
    let seconds = i64::from(st.hour) * 3600 + i64::from(st.minute) * 60 + i64::from(st.second);
    put(out, date_value(day, seconds));
    1
}

/// VariantTimeToSystemTime: the date and time to the second. A time
/// within a millisecond of the next second is that second (binary
/// fractions of a day are inexact); otherwise the seconds are truncated
/// (31 December 9999 at 23:59:59.91, 2958465.999999, stays in 9999).
#[unsafe(no_mangle)]
pub extern "system" fn VariantTimeToSystemTime(date: f64, st: *mut SystemTime) -> i32 {
    if st.is_null() || date.is_nan() {
        return 0;
    }
    if date <= (FIRST_DAY - 1) as f64 || date >= (LAST_DAY + 1) as f64 {
        return 0;
    }
    let mut day = date as i64; // toward zero
    let time = (date - day as f64).abs() * SECONDS_PER_DAY;
    let mut seconds = time as i64;
    if time - seconds as f64 >= 0.999 {
        seconds += 1;
    }
    if seconds >= 86_400 {
        seconds -= 86_400;
        day += 1;
    }
    let (year, month, d) = civil(day);
    // 30 December 1899 was a Saturday (6).
    let day_of_week = (day.rem_euclid(7) + 6) % 7;
    put(
        st,
        SystemTime {
            year: year as u16,
            month: month as u16,
            day_of_week: day_of_week as u16,
            day: d as u16,
            hour: (seconds / 3600) as u16,
            minute: (seconds / 60 % 60) as u16,
            second: (seconds % 60) as u16,
            milliseconds: 0,
        },
    );
    1
}

const VAR_TIMEVALUEONLY: u32 = 1;
const VAR_DATEVALUEONLY: u32 = 2;

/// A token of a date string.
#[derive(Clone, Copy, PartialEq)]
enum Token {
    /// A number and how many digits it had.
    Number(i64, usize),
    Month(i64),
    Am,
    Pm,
    Colon,
    /// `/`, `-`, `,` or spaces.
    Separator,
}

const MONTHS: [&[u8]; 12] = [
    b"january",
    b"february",
    b"march",
    b"april",
    b"may",
    b"june",
    b"july",
    b"august",
    b"september",
    b"october",
    b"november",
    b"december",
];

/// A word as a token: an English month name (in full or its first three
/// letters or more) or A/AM/P/PM, in any case.
fn word(w: &[u8]) -> Option<Token> {
    let lower = |i: usize| w[i].to_ascii_lowercase();
    let is = |s: &[u8]| w.len() == s.len() && (0..w.len()).all(|i| lower(i) == s[i]);
    if is(b"a") || is(b"am") {
        return Some(Token::Am);
    }
    if is(b"p") || is(b"pm") {
        return Some(Token::Pm);
    }
    MONTHS
        .iter()
        .position(|m| w.len() >= 3 && w.len() <= m.len() && (0..w.len()).all(|i| lower(i) == m[i]))
        .map(|i| Token::Month(i as i64 + 1))
}

/// Why a string is not read as a date.
enum Unread {
    /// Not a date in any form Windows reads.
    NotADate,
    /// A form Windows reads that gala does not yet.
    Unsupported,
}

/// Parses an en-US date string into (day number, seconds into the day,
/// whether a date and a time were given). Forms: a date of three numbers
/// (month/day/year, day/month/year when the first cannot be a month,
/// year-month-day when the first has four digits or is over 31; `/`,
/// `-`, spaces or commas between), or a day, an English month name and a
/// year in either order; a time h:mm[:ss] or h with AM/PM after it. A
/// date without a year (Windows' current year) and `.` separators are
/// not read yet.
fn parse_date(text: &[u16]) -> Result<(i64, i64, bool, bool), Unread> {
    let mut tokens = [Token::Separator; 32];
    let mut count = 0;
    let mut i = 0;
    while i < text.len() {
        let c = text[i];
        let token = if (0x30..=0x39).contains(&c) {
            let (mut v, mut digits) = (0i64, 0);
            while i < text.len() && (0x30..=0x39).contains(&text[i]) {
                v = (v * 10 + i64::from(text[i] - 0x30)).min(1_000_000);
                digits += 1;
                i += 1;
            }
            Token::Number(v, digits)
        } else if c < 128 && (c as u8).is_ascii_alphabetic() {
            let mut w = [0u8; 16];
            let mut n = 0;
            while i < text.len() && text[i] < 128 && (text[i] as u8).is_ascii_alphabetic() {
                if n == w.len() {
                    return Err(Unread::NotADate);
                }
                w[n] = text[i] as u8;
                n += 1;
                i += 1;
            }
            word(&w[..n]).ok_or(Unread::NotADate)?
        } else {
            i += 1;
            match c {
                0x3a => Token::Colon,
                0x2f | 0x2d | 0x2c | 0x20 | 0x09 => Token::Separator,
                0x2e => return Err(Unread::Unsupported),
                _ => return Err(Unread::NotADate),
            }
        };
        if token == Token::Separator && count > 0 && tokens[count - 1] == Token::Separator {
            continue;
        }
        if count == tokens.len() {
            return Err(Unread::NotADate);
        }
        tokens[count] = token;
        count += 1;
    }
    let tokens = &tokens[..count];
    // The time: h:mm[:ss], or a number directly before AM/PM.
    let (mut numbers, mut n) = ([(0i64, 0usize); 3], 0);
    let mut month = None;
    let mut time: Option<[i64; 3]> = None;
    let mut half = None;
    let mut t = 0;
    while t < tokens.len() {
        match tokens[t] {
            Token::Number(v, _) if tokens.get(t + 1) == Some(&Token::Colon) => {
                if time.is_some() {
                    return Err(Unread::NotADate);
                }
                let mut parts = [v, 0, 0];
                let mut k = 1;
                t += 1;
                while tokens.get(t) == Some(&Token::Colon) {
                    let Some(&Token::Number(p, _)) = tokens.get(t + 1) else {
                        return Err(Unread::NotADate);
                    };
                    if k == 3 {
                        return Err(Unread::NotADate);
                    }
                    parts[k] = p;
                    k += 1;
                    t += 2;
                }
                time = Some(parts);
                continue;
            }
            Token::Number(v, _)
                if time.is_none() && matches!(tokens.get(t + 1), Some(Token::Am | Token::Pm))
                    || time.is_none()
                        && tokens.get(t + 1) == Some(&Token::Separator)
                        && matches!(tokens.get(t + 2), Some(Token::Am | Token::Pm)) =>
            {
                time = Some([v, 0, 0]);
            }
            Token::Number(v, digits) => {
                if n == numbers.len() {
                    return Err(Unread::NotADate);
                }
                numbers[n] = (v, digits);
                n += 1;
            }
            Token::Month(m) => {
                if month.replace(m).is_some() {
                    return Err(Unread::NotADate);
                }
            }
            Token::Am | Token::Pm => {
                if time.is_none() || half.replace(tokens[t]).is_some() {
                    return Err(Unread::NotADate);
                }
            }
            Token::Colon => return Err(Unread::NotADate),
            Token::Separator => {}
        }
        t += 1;
    }
    let seconds = match time {
        None => 0,
        Some([h, m, s]) => {
            if h > 23 || m > 59 || s > 59 {
                return Err(Unread::NotADate);
            }
            let h = match half {
                Some(Token::Pm) if h < 12 => h + 12,
                Some(Token::Am) if h == 12 => 0,
                _ => h,
            };
            h * 3600 + m * 60 + s
        }
    };
    let year = |(v, digits): (i64, usize)| if digits <= 2 { full_year(v) } else { v };
    let (y, m, d) = match (month, n) {
        (None, 0) if time.is_some() => return Ok((0, seconds, false, true)),
        (None, 3) => {
            let (a, b, c) = (numbers[0], numbers[1], numbers[2]);
            if a.1 >= 3 || a.0 > 31 {
                (year(a), b.0, c.0)
            } else if a.0 <= 12 {
                (year(c), a.0, b.0)
            } else {
                (year(c), b.0, a.0)
            }
        }
        (Some(m), 2) => {
            let (a, b) = (numbers[0], numbers[1]);
            if a.1 <= 2 && a.0 <= 31 && a.0 >= 1 { (year(b), m, a.0) } else { (year(a), m, b.0) }
        }
        // A day and month without a year (or a month and year): Windows
        // reads these with the current year; not yet.
        (None, 2) if numbers[..2].iter().any(|&(v, _)| (1..=12).contains(&v)) => {
            return Err(Unread::Unsupported);
        }
        (Some(_), 1) => return Err(Unread::Unsupported),
        _ => return Err(Unread::NotADate),
    };
    if !(1..=12).contains(&m) || d < 1 || d > month_days(y, m) || !(100..=9999).contains(&y) {
        return Err(Unread::NotADate);
    }
    Ok((day_number(y, m, d), seconds, true, time.is_some()))
}

/// VarDateFromStr: an en-US date and/or time string as a DATE
/// (`parse_date`'s forms); DISP_E_TYPEMISMATCH for anything else (a form
/// Windows reads and gala does not is reported to the ledger).
#[unsafe(no_mangle)]
pub extern "system" fn VarDateFromStr(
    s: *const u16,
    lcid: u32,
    flags: u32,
    out: *mut f64,
) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    if s.is_null() {
        return DISP_E_TYPEMISMATCH;
    }
    crate::conversions::check_locale(b"oleaut32.dll!VarDateFromStr(locale)", lcid);
    let text = crate::variant::chars(s, wide_len(s));
    match parse_date(text) {
        Ok((day, seconds, _, _)) => {
            let (day, seconds) = match flags & (VAR_TIMEVALUEONLY | VAR_DATEVALUEONLY) {
                VAR_TIMEVALUEONLY => (0, seconds),
                VAR_DATEVALUEONLY => (day, 0),
                _ => (day, seconds),
            };
            put(out, date_value(day, seconds));
            S_OK
        }
        Err(Unread::NotADate) => DISP_E_TYPEMISMATCH,
        Err(Unread::Unsupported) => {
            missing(b"oleaut32.dll!VarDateFromStr(format)", text.len() as u64);
            DISP_E_TYPEMISMATCH
        }
    }
}

//! Calendar information (GetCalendarInfoEx/W, EnumCalendarInfoExEx), as
//! documented, for gala's one locale (en-US, also the invariant locale and
//! the defaults) and its one calendar, the localized Gregorian calendar
//! (CAL_GREGORIAN, 1), with the values Windows gives for en-US.

use super::*;

const CAL_GREGORIAN: u32 = 1;
const ENUM_ALL_CALENDARS: u32 = u32::MAX;
const CAL_RETURN_NUMBER: u32 = 0x2000_0000;
/// CAL_NOUSEROVERRIDE, CAL_USE_CP_ACP, CAL_RETURN_GENITIVE_NAMES.
const CAL_MODIFIERS: u32 = 0x8000_0000 | 0x4000_0000 | 0x1000_0000;
const ERROR_INSUFFICIENT_BUFFER: u32 = 122;
const ERROR_INVALID_FLAGS: u32 = 1004;

const DAYS: [&str; 7] =
    ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];
const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// The values of `kind` (the first is the default; enumerations list all).
fn values(kind: u32) -> Option<&'static [&'static str]> {
    Some(match kind {
        0x1 => &["1"],                  // CAL_ICALINTVALUE
        0x2 => &["Gregorian Calendar"], // CAL_SCALNAME
        0x3 => &["0"],                  // CAL_IYEAROFFSETRANGE
        0x4 => &["A.D."],               // CAL_SERASTRING
        0x5 => {
            &["M/d/yyyy", "M/d/yy", "MM/dd/yy", "MM/dd/yyyy", "yy/MM/dd", "yyyy-MM-dd", "dd-MMM-yy"]
        }
        0x6 => &["dddd, MMMM d, yyyy", "MMMM d, yyyy", "dddd, d MMMM, yyyy", "d MMMM, yyyy"],
        0x2f => &["MMMM yyyy", "MMMM, yyyy"], // CAL_SYEARMONTH
        0x30 => &["2049"],                    // CAL_ITWODIGITYEARMAX
        0x38 => &["MMMM d"],                  // CAL_SMONTHDAY
        0x39 => &["AD"],                      // CAL_SABBREVERASTRING
        0x3a => &["dddd, MMMM d"],            // CAL_SRELATIVELONGDATE
        0x3b => &["A.D."],                    // CAL_SENGLISHERANAME
        0x3c => &["AD"],                      // CAL_SENGLISHABBREVERANAME
        _ => return None,
    })
}

/// A single name: days (Monday first), months (13 for the 13th, empty).
fn name(kind: u32) -> Option<&'static str> {
    let k = kind as usize;
    Some(match kind {
        0x7..=0xd => DAYS[k - 0x7],
        0xe..=0x14 => &DAYS[k - 0xe][..3],
        0x15..=0x20 => MONTHS[k - 0x15],
        0x22..=0x2d => &MONTHS[k - 0x22][..3],
        0x21 | 0x2e => "",
        0x31..=0x37 => &DAYS[k - 0x31][..2],
        _ => return None,
    })
}

/// The first value of `kind`.
fn first_value(kind: u32) -> Option<&'static str> {
    name(kind).or_else(|| values(kind).map(|v| v[0]))
}

/// Whether `locale` is one gala has (en-US, the invariant "", the
/// defaults: null, "!x-sys-default-locale").
fn known_locale(locale: *const u16) -> bool {
    if locale.is_null() {
        return true;
    }
    let name = wide_slice(locale);
    name.is_empty()
        || super::locale::is_en_us(name)
        || name.iter().copied().eq("!x-sys-default-locale".encode_utf16())
}

/// Writes `text` with its NUL into `buf` of `len` characters; the count,
/// or with `len` 0 the count needed.
fn put_text(text: &str, buf: *mut u16, len: i32) -> i32 {
    let need = text.encode_utf16().count() + 1;
    if len == 0 {
        return need as i32;
    }
    if buf.is_null() || (len as usize) < need {
        return fail_with(ERROR_INSUFFICIENT_BUFFER, 0);
    }
    // SAFETY: the caller's buffer of `len` >= `need` characters.
    unsafe {
        for (i, c) in text.encode_utf16().chain(core::iter::once(0)).enumerate() {
            *buf.add(i) = c;
        }
    }
    need as i32
}

/// GetCalendarInfoEx(locale, calendar, reserved, CalType, data, len,
/// value): a string, or with CAL_RETURN_NUMBER a DWORD in `value` (the
/// result is then 2, as documented: the size in characters).
#[unsafe(no_mangle)]
pub extern "system" fn GetCalendarInfoEx(
    locale: *const u16,
    calendar: u32,
    _reserved: *const u16,
    kind: u32,
    data: *mut u16,
    len: i32,
    value: *mut u32,
) -> i32 {
    if !known_locale(locale) || calendar != CAL_GREGORIAN || len < 0 {
        return fail_with(ERROR_INVALID_PARAMETER, 0);
    }
    let base = kind & !(CAL_MODIFIERS | CAL_RETURN_NUMBER);
    let Some(text) = first_value(base) else {
        unsupported(b"kernel32.dll!GetCalendarInfoEx(CalType)", base.into());
        return fail_with(ERROR_INVALID_FLAGS, 0);
    };
    if kind & CAL_RETURN_NUMBER != 0 {
        let Ok(n) = text.parse::<u32>() else { return fail_with(ERROR_INVALID_FLAGS, 0) };
        if value.is_null() {
            return fail_with(ERROR_INVALID_PARAMETER, 0);
        }
        // SAFETY: the caller's output.
        unsafe { *value = n };
        return 2;
    }
    put_text(text, data, len)
}

/// GetCalendarInfoW: the same for an LCID (gala's locale, 0x409, and the
/// defaults).
#[unsafe(no_mangle)]
pub extern "system" fn GetCalendarInfoW(
    lcid: u32,
    calendar: u32,
    kind: u32,
    data: *mut u16,
    len: i32,
    value: *mut u32,
) -> i32 {
    if !matches!(lcid, 0x409 | 0x400 | 0x800 | 0x7f | 0) {
        return fail_with(ERROR_INVALID_PARAMETER, 0);
    }
    GetCalendarInfoEx(core::ptr::null(), calendar, core::ptr::null(), kind, data, len, value)
}

type EnumProcExEx = extern "system" fn(*const u16, u32, *const u16, isize) -> Bool;

/// EnumCalendarInfoExEx: each value of CalType for the calendar (or for
/// every calendar, ENUM_ALL_CALENDARS: Gregorian only), until the
/// callback returns FALSE.
#[unsafe(no_mangle)]
pub extern "system" fn EnumCalendarInfoExEx(
    callback: Option<EnumProcExEx>,
    locale: *const u16,
    calendar: u32,
    _reserved: *const u16,
    kind: u32,
    param: isize,
) -> Bool {
    let Some(callback) = callback else { return fail_with(ERROR_INVALID_PARAMETER, FALSE) };
    if !known_locale(locale) || !(calendar == CAL_GREGORIAN || calendar == ENUM_ALL_CALENDARS) {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    let base = kind & !CAL_MODIFIERS;
    let single = name(base);
    let list: &[&str] = match (values(base), &single) {
        (Some(v), _) => v,
        (None, Some(n)) => core::slice::from_ref(n),
        (None, None) => {
            unsupported(b"kernel32.dll!EnumCalendarInfoExEx(CalType)", base.into());
            return fail_with(ERROR_INVALID_FLAGS, FALSE);
        }
    };
    for text in list {
        let mut wide = [0u16; 64];
        for (d, c) in wide.iter_mut().zip(text.encode_utf16()) {
            *d = c;
        }
        if callback(wide.as_ptr(), CAL_GREGORIAN, core::ptr::null(), param) == FALSE {
            break;
        }
    }
    TRUE
}

type TimeFormatProc = extern "system" fn(*const u16, isize) -> Bool;
type DateFormatProc = extern "system" fn(*const u16, u32, isize) -> Bool;

/// Calls `f` with each of `list` as a NUL-terminated string until it
/// returns FALSE.
fn each(list: &[&str], mut f: impl FnMut(*const u16) -> Bool) {
    for text in list {
        let mut wide = [0u16; 64];
        for (d, c) in wide.iter_mut().zip(text.encode_utf16()) {
            *d = c;
        }
        if f(wide.as_ptr()) == FALSE {
            break;
        }
    }
}

/// EnumTimeFormatsEx: the en-US time formats; TIME_NOSECONDS (2) gives
/// them without seconds.
#[unsafe(no_mangle)]
pub extern "system" fn EnumTimeFormatsEx(
    callback: Option<TimeFormatProc>,
    locale: *const u16,
    flags: u32,
    param: isize,
) -> Bool {
    let Some(callback) = callback else { return fail_with(ERROR_INVALID_PARAMETER, FALSE) };
    if !known_locale(locale) || flags & !2 != 0 {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    let list: &[&str] = if flags & 2 != 0 {
        &["h:mm tt", "hh:mm tt", "H:mm", "HH:mm"]
    } else {
        &["h:mm:ss tt", "hh:mm:ss tt", "H:mm:ss", "HH:mm:ss"]
    };
    each(list, |text| callback(text, param));
    TRUE
}

/// EnumDateFormatsExEx: DATE_SHORTDATE (1), DATE_LONGDATE (2),
/// DATE_YEARMONTH (8) or DATE_MONTHDAY (0x80) of en-US, calendar 1.
#[unsafe(no_mangle)]
pub extern "system" fn EnumDateFormatsExEx(
    callback: Option<DateFormatProc>,
    locale: *const u16,
    flags: u32,
    param: isize,
) -> Bool {
    let Some(callback) = callback else { return fail_with(ERROR_INVALID_PARAMETER, FALSE) };
    if !known_locale(locale) {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    let kind = match flags & !0x8000_0000 {
        1 => 0x5,
        2 => 0x6,
        8 => 0x2f,
        0x80 => 0x38,
        _ => return fail_with(ERROR_INVALID_FLAGS, FALSE),
    };
    let list = values(kind).unwrap_or(&[]);
    each(list, |text| callback(text, CAL_GREGORIAN, param));
    TRUE
}

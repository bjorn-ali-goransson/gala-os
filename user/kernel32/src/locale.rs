//! Locales and local time. gala has one locale, English (United States),
//! LCID 0x409, and one time zone, UTC: the user, system and thread
//! defaults and LOCALE_INVARIANT are all served with en-US data (from
//! the en-US values Windows documents for each LCTYPE), and local time is
//! system time. Linguistic comparison orders by letters first and puts
//! lower case before upper case, as the en-US sort does for these
//! characters; text is classified with Unicode's properties.

use super::text::{GetACP, GetSystemTime};
use super::*;

const ERROR_INSUFFICIENT_BUFFER: u32 = 122;
const ERROR_INVALID_FLAGS: u32 = 1004;

const LOCALE_NOUSEROVERRIDE: u32 = 0x8000_0000;
const LOCALE_USE_CP_ACP: u32 = 0x4000_0000;
const LOCALE_RETURN_NUMBER: u32 = 0x2000_0000;
const LOCALE_RETURN_GENITIVE_NAMES: u32 = 0x1000_0000;
const LOCALE_ALLOW_NEUTRAL_NAMES: u32 = 0x0800_0000;

const LANG_EN_US: u32 = 0x409;

/// Whether `lcid` names gala's locale: en-US, the user and system
/// defaults (0x400, 0x800), LOCALE_NEUTRAL (0) and LOCALE_INVARIANT (0x7f).
fn known_lcid(lcid: u32) -> bool {
    matches!(lcid, LANG_EN_US | 0x400 | 0x800 | 0 | 0x7f)
}

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

/// The en-US value of an LCTYPE (without its flags).
fn locale_value(kind: u32) -> Option<&'static str> {
    Some(match kind {
        0x01 => "0409",                    // LOCALE_ILANGUAGE
        0x02 => "English (United States)", // LOCALE_SLOCALIZEDDISPLAYNAME
        0x03 => "ENU",                     // LOCALE_SABBREVLANGNAME
        0x04 => "English",                 // LOCALE_SNATIVELANGNAME
        0x05 => "1",                       // LOCALE_ICOUNTRY
        0x06 => "United States",           // LOCALE_SLOCALIZEDCOUNTRYNAME
        0x07 => "USA",                     // LOCALE_SABBREVCTRYNAME
        0x08 => "United States",           // LOCALE_SNATIVECOUNTRYNAME
        0x09 => "0409",                    // LOCALE_IDEFAULTLANGUAGE
        0x0a => "1",                       // LOCALE_IDEFAULTCOUNTRY
        0x0b => "437",                     // LOCALE_IDEFAULTCODEPAGE
        0x0c => ",",                       // LOCALE_SLIST
        0x0d => "1",                       // LOCALE_IMEASURE (U.S.)
        0x0e => ".",                       // LOCALE_SDECIMAL
        0x0f => ",",                       // LOCALE_STHOUSAND
        0x10 => "3;0",                     // LOCALE_SGROUPING
        0x11 => "2",                       // LOCALE_IDIGITS
        0x12 => "1",                       // LOCALE_ILZERO
        0x13 => "0123456789",              // LOCALE_SNATIVEDIGITS
        0x14 => "$",                       // LOCALE_SCURRENCY
        0x15 => "USD",                     // LOCALE_SINTLSYMBOL
        0x16 => ".",                       // LOCALE_SMONDECIMALSEP
        0x17 => ",",                       // LOCALE_SMONTHOUSANDSEP
        0x18 => "3;0",                     // LOCALE_SMONGROUPING
        0x19 => "2",                       // LOCALE_ICURRDIGITS
        0x1a => "2",                       // LOCALE_IINTLCURRDIGITS
        0x1b => "0",                       // LOCALE_ICURRENCY
        0x1c => "0",                       // LOCALE_INEGCURR
        0x1d => "/",                       // LOCALE_SDATE
        0x1e => ":",                       // LOCALE_STIME
        0x1f => "M/d/yyyy",                // LOCALE_SSHORTDATE
        0x20 => "dddd, MMMM d, yyyy",      // LOCALE_SLONGDATE
        0x21 => "0",                       // LOCALE_IDATE
        0x22 => "0",                       // LOCALE_ILDATE
        0x23 => "0",                       // LOCALE_ITIME
        0x25 => "1",                       // LOCALE_ICENTURY
        0x26 => "0",                       // LOCALE_ITLZERO
        0x27 => "0",                       // LOCALE_IDAYLZERO
        0x28 => "AM",                      // LOCALE_S1159
        0x29 => "PM",                      // LOCALE_S2359
        0x2a..=0x30 => DAYS[(kind - 0x2a) as usize],
        0x31..=0x37 => &DAYS[(kind - 0x31) as usize][..3],
        0x38..=0x43 => MONTHS[(kind - 0x38) as usize],
        0x44..=0x4f => &MONTHS[(kind - 0x44) as usize][..3],
        0x50 => "",                // LOCALE_SPOSITIVESIGN
        0x51 => "-",               // LOCALE_SNEGATIVESIGN
        0x52 => "3",               // LOCALE_IPOSSIGNPOSN
        0x53 => "0",               // LOCALE_INEGSIGNPOSN
        0x54 => "0",               // LOCALE_IPOSSYMPRECEDES
        0x55 => "0",               // LOCALE_IPOSSEPBYSPACE
        0x56 => "1",               // LOCALE_INEGSYMPRECEDES
        0x57 => "0",               // LOCALE_INEGSEPBYSPACE
        0x59 => "en",              // LOCALE_SISO639LANGNAME
        0x5a => "US",              // LOCALE_SISO3166CTRYNAME
        0x5c => "en-US",           // LOCALE_SNAME
        0x6d => "en",              // LOCALE_SPARENT
        0x71 => "0",               // LOCALE_INEUTRAL (a specific locale)
        0x67 => "eng",             // LOCALE_SISO639LANGNAME2
        0x68 => "USA",             // LOCALE_SISO3166CTRYNAME2
        0x1001 => "English",       // LOCALE_SENGLISHLANGUAGENAME
        0x1002 => "United States", // LOCALE_SENGLISHCOUNTRYNAME
        0x1003 => "h:mm:ss tt",    // LOCALE_STIMEFORMAT
        0x1009 => "6",             // LOCALE_IFIRSTDAYOFWEEK (Sunday)
        0x100c => "0",             // LOCALE_IFIRSTWEEKOFYEAR
        0x1011 => "10000",         // LOCALE_IDEFAULTMACCODEPAGE
        0x1012 => "1",             // LOCALE_IOPTIONALCALENDAR
        0x1014 => "037",           // LOCALE_IDEFAULTEBCDICCODEPAGE
        0x1019 => "MMMM yyyy",     // LOCALE_SYEARMONTH
        _ => return None,
    })
}

/// LOCALE_IDEFAULTANSICODEPAGE: gala's ANSI code page (UTF-8).
const LOCALE_IDEFAULTANSICODEPAGE: u32 = 0x1004;

/// The value of `kind` as UTF-16 in `out`; its length, or None.
fn locale_text(kind: u32, out: &mut [u16; 64]) -> Option<usize> {
    let mut digits = [0u8; 10];
    let text: &str = if kind == LOCALE_IDEFAULTANSICODEPAGE {
        decimal(GetACP(), &mut digits)
    } else {
        locale_value(kind)?
    };
    let mut n = 0;
    for c in text.encode_utf16() {
        *out.get_mut(n)? = c;
        n += 1;
    }
    Some(n)
}

fn decimal(mut v: u32, buf: &mut [u8; 10]) -> &str {
    let mut i = buf.len();
    loop {
        i -= 1;
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    core::str::from_utf8(&buf[i..]).unwrap_or("0")
}

/// Writes `text` and a NUL into `buf` of `size` characters: the count
/// with the NUL; with size 0, the count needed; ERROR_INSUFFICIENT_BUFFER
/// if it does not fit.
fn put<T: Copy + From<u8>>(text: &[T], buf: *mut T, size: i32) -> i32 {
    let need = text.len() + 1;
    if size == 0 {
        return need as i32;
    }
    if buf.is_null() || (size as usize) < need {
        return fail_with(ERROR_INSUFFICIENT_BUFFER, 0);
    }
    // SAFETY: the caller's buffer holds `size` >= `need` characters.
    unsafe {
        core::ptr::copy_nonoverlapping(text.as_ptr(), buf, text.len());
        *buf.add(text.len()) = T::from(0);
    }
    need as i32
}

/// GetLocaleInfoW: the en-US value; LOCALE_RETURN_NUMBER stores a DWORD
/// (2 characters). ERROR_INVALID_FLAGS for an LCTYPE gala does not know
/// (recorded), ERROR_INVALID_PARAMETER for another locale.
#[unsafe(no_mangle)]
pub extern "system" fn GetLocaleInfoW(lcid: u32, kind: u32, buf: *mut u16, size: i32) -> i32 {
    if !known_lcid(lcid) || size < 0 {
        return fail_with(ERROR_INVALID_PARAMETER, 0);
    }
    let base = kind
        & !(LOCALE_NOUSEROVERRIDE
            | LOCALE_USE_CP_ACP
            | LOCALE_RETURN_NUMBER
            | LOCALE_RETURN_GENITIVE_NAMES
            | LOCALE_ALLOW_NEUTRAL_NAMES);
    let mut text = [0u16; 64];
    let Some(n) = locale_text(base, &mut text) else {
        unsupported(b"kernel32.dll!GetLocaleInfoW(LCTYPE)", base.into());
        return fail_with(ERROR_INVALID_FLAGS, 0);
    };
    if kind & LOCALE_RETURN_NUMBER != 0 {
        let mut v = 0u32;
        for &c in &text[..n] {
            match u8::try_from(c).ok().filter(u8::is_ascii_digit) {
                Some(d) => v = v * 10 + u32::from(d - b'0'),
                None => return fail_with(ERROR_INVALID_FLAGS, 0),
            }
        }
        if size == 0 {
            return 2;
        }
        if buf.is_null() || size < 2 {
            return fail_with(ERROR_INSUFFICIENT_BUFFER, 0);
        }
        // SAFETY: the caller's buffer of at least 2 WCHARs.
        unsafe { buf.cast::<u32>().write_unaligned(v) };
        return 2;
    }
    put(&text[..n], buf, size)
}

/// GetLocaleInfoA: as the W form, in bytes (the values are ASCII).
#[unsafe(no_mangle)]
pub extern "system" fn GetLocaleInfoA(lcid: u32, kind: u32, buf: *mut u8, size: i32) -> i32 {
    if kind & LOCALE_RETURN_NUMBER != 0 {
        if size != 0 && size < 4 {
            return fail_with(ERROR_INSUFFICIENT_BUFFER, 0);
        }
        let n = GetLocaleInfoW(lcid, kind, buf.cast(), size / 2);
        return if n == 0 { 0 } else { 4 };
    }
    let mut wide = [0u16; 64];
    let n = GetLocaleInfoW(lcid, kind, wide.as_mut_ptr(), 64);
    if n == 0 {
        return 0;
    }
    let mut narrow = [0u8; 64];
    for (d, &c) in narrow.iter_mut().zip(&wide[..n as usize - 1]) {
        *d = u8::try_from(c).unwrap_or(b'?');
    }
    put(&narrow[..n as usize - 1], buf, size)
}

/// GetLocaleInfoEx(name, ...): "en-US" (and the defaults: null, "",
/// LOCALE_NAME_INVARIANT "").
#[unsafe(no_mangle)]
pub extern "system" fn GetLocaleInfoEx(
    name: *const u16,
    kind: u32,
    buf: *mut u16,
    size: i32,
) -> i32 {
    if !name.is_null() && !is_en_us(wide_slice(name)) && !wide_slice(name).is_empty() {
        return fail_with(ERROR_INVALID_PARAMETER, 0);
    }
    GetLocaleInfoW(LANG_EN_US, kind, buf, size)
}

pub(crate) fn is_en_us(name: &[u16]) -> bool {
    name.len() == 5
        && name
            .iter()
            .zip(b"en-us")
            .all(|(&c, &b)| u8::try_from(c).map(|c| c.to_ascii_lowercase()) == Ok(b))
}

#[unsafe(no_mangle)]
pub extern "system" fn GetUserDefaultLCID() -> u32 {
    LANG_EN_US
}

#[unsafe(no_mangle)]
pub extern "system" fn GetSystemDefaultLCID() -> u32 {
    LANG_EN_US
}

/// The TEB's CurrentLocale (+0x108): the thread's locale, 0 until
/// SetThreadLocale (then the user's default, en-US, is reported).
fn teb_locale() -> *mut u32 {
    (crate::runtime::teb() + 0x108) as *mut u32
}

#[unsafe(no_mangle)]
pub extern "system" fn GetThreadLocale() -> u32 {
    // SAFETY: the calling thread's own TEB.
    match unsafe { *teb_locale() } {
        0 => LANG_EN_US,
        lcid => lcid,
    }
}

/// SetThreadLocale(lcid): the thread's locale (LOCALE_USER_DEFAULT and
/// LOCALE_SYSTEM_DEFAULT are en-US here); FALSE with
/// ERROR_INVALID_PARAMETER for an LCID without a language.
#[unsafe(no_mangle)]
pub extern "system" fn SetThreadLocale(lcid: u32) -> i32 {
    let lcid = match lcid {
        0x0400 | 0x0800 => LANG_EN_US,
        l => l,
    };
    if lcid & 0x3ff == 0 || lcid > 0x000f_ffff {
        crate::SetLastError(87); // ERROR_INVALID_PARAMETER
        return 0;
    }
    // SAFETY: the calling thread's own TEB.
    unsafe { *teb_locale() = lcid };
    1
}

#[unsafe(no_mangle)]
pub extern "system" fn GetUserDefaultLangID() -> u16 {
    LANG_EN_US as u16
}

#[unsafe(no_mangle)]
pub extern "system" fn GetSystemDefaultLangID() -> u16 {
    LANG_EN_US as u16
}

#[unsafe(no_mangle)]
pub extern "system" fn GetUserDefaultUILanguage() -> u16 {
    LANG_EN_US as u16
}

#[unsafe(no_mangle)]
pub extern "system" fn GetSystemDefaultUILanguage() -> u16 {
    LANG_EN_US as u16
}

/// GetUserDefaultLocaleName / GetSystemDefaultLocaleName: "en-US".
#[unsafe(no_mangle)]
pub extern "system" fn GetUserDefaultLocaleName(buf: *mut u16, size: i32) -> i32 {
    let name: [u16; 5] = b"en-US".map(u16::from);
    put(&name, buf, size)
}

#[unsafe(no_mangle)]
pub extern "system" fn GetSystemDefaultLocaleName(buf: *mut u16, size: i32) -> i32 {
    GetUserDefaultLocaleName(buf, size)
}

/// IsValidLocale: gala's locale (and the defaults) only.
#[unsafe(no_mangle)]
pub extern "system" fn IsValidLocale(lcid: u32, _flags: u32) -> Bool {
    if known_lcid(lcid) && lcid != 0 { TRUE } else { FALSE }
}

type LocaleEnumProcW = extern "system" fn(*const u16) -> Bool;
type LocaleEnumProcA = extern "system" fn(*const u8) -> Bool;

/// EnumSystemLocalesW: the one locale, "00000409".
#[unsafe(no_mangle)]
pub extern "system" fn EnumSystemLocalesW(f: Option<LocaleEnumProcW>, _flags: u32) -> Bool {
    let Some(f) = f else { return fail_with(ERROR_INVALID_PARAMETER, FALSE) };
    let name: [u16; 9] = [b'0', b'0', b'0', b'0', b'0', b'4', b'0', b'9', 0].map(u16::from);
    f(name.as_ptr());
    TRUE
}

#[unsafe(no_mangle)]
pub extern "system" fn EnumSystemLocalesA(f: Option<LocaleEnumProcA>, _flags: u32) -> Bool {
    let Some(f) = f else { return fail_with(ERROR_INVALID_PARAMETER, FALSE) };
    f(c"00000409".as_ptr().cast());
    TRUE
}

// ---- Case mapping, comparison and character types ----

const LCMAP_LOWERCASE: u32 = 0x100;
const LCMAP_UPPERCASE: u32 = 0x200;
const LCMAP_LINGUISTIC_CASING: u32 = 0x0100_0000;

/// One UTF-16 unit mapped to upper or lower case, where the mapping is a
/// single BMP character (others are kept).
fn map_case(c: u16, upper: bool) -> u16 {
    let Some(ch) = char::from_u32(c.into()) else { return c };
    let mapped = if upper { single(ch.to_uppercase()) } else { single(ch.to_lowercase()) };
    mapped.and_then(|m| u16::try_from(u32::from(m)).ok()).unwrap_or(c)
}

/// The only item of an iterator of one.
fn single(mut it: impl Iterator<Item = char>) -> Option<char> {
    let first = it.next()?;
    if it.next().is_some() { None } else { Some(first) }
}

/// The units of a (string, length) argument: length -1 means
/// NUL-terminated.
fn units(s: *const u16, n: i32) -> &'static [u16] {
    if s.is_null() {
        return &[];
    }
    if n < 0 {
        return wide_slice(s);
    }
    // SAFETY: the caller's string of `n` units.
    unsafe { core::slice::from_raw_parts(s, n as usize) }
}

/// LCMapStringW: upper or lower case (sort keys and the other forms are
/// not supported, reported). With length -1 the NUL is mapped and counted.
#[unsafe(no_mangle)]
pub extern "system" fn LCMapStringW(
    lcid: u32,
    flags: u32,
    src: *const u16,
    n: i32,
    dst: *mut u16,
    size: i32,
) -> i32 {
    if !known_lcid(lcid) || src.is_null() || n == 0 || size < 0 {
        return fail_with(ERROR_INVALID_PARAMETER, 0);
    }
    let upper = match flags & !LCMAP_LINGUISTIC_CASING {
        LCMAP_UPPERCASE => true,
        LCMAP_LOWERCASE => false,
        other => {
            unsupported(b"kernel32.dll!LCMapStringW(flags)", other.into());
            return fail_with(ERROR_INVALID_FLAGS, 0);
        }
    };
    let text = units(src, n);
    let count = text.len() + usize::from(n < 0);
    if size == 0 {
        return count as i32;
    }
    if dst.is_null() || (size as usize) < count {
        return fail_with(ERROR_INSUFFICIENT_BUFFER, 0);
    }
    for (i, &c) in text.iter().enumerate() {
        // SAFETY: inside the caller's buffer of `size` >= `count` units.
        unsafe { *dst.add(i) = map_case(c, upper) };
    }
    if n < 0 {
        // SAFETY: as above.
        unsafe { *dst.add(text.len()) = 0 };
    }
    count as i32
}

/// LCMapStringEx(name, ...): LCMapStringW for "en-US".
#[unsafe(no_mangle)]
pub extern "system" fn LCMapStringEx(
    _name: *const u16,
    flags: u32,
    src: *const u16,
    n: i32,
    dst: *mut u16,
    size: i32,
    _version: *mut c_void,
    _reserved: *mut c_void,
    _param: isize,
) -> i32 {
    LCMapStringW(LANG_EN_US, flags, src, n, dst, size)
}

const NORM_IGNORECASE: u32 = 1;
const CSTR_LESS_THAN: i32 = 1;
const CSTR_EQUAL: i32 = 2;
const CSTR_GREATER_THAN: i32 = 3;

fn order(o: core::cmp::Ordering) -> i32 {
    match o {
        core::cmp::Ordering::Less => CSTR_LESS_THAN,
        core::cmp::Ordering::Equal => CSTR_EQUAL,
        core::cmp::Ordering::Greater => CSTR_GREATER_THAN,
    }
}

/// The linguistic comparison: letters without case first (a shorter
/// string that is a prefix sorts first), then, unless case is ignored,
/// the first difference in case, lower case first.
fn compare(a: &[u16], b: &[u16], flags: u32) -> i32 {
    let fold = |c: u16| map_case(c, false);
    let primary = a.iter().map(|&c| fold(c)).cmp(b.iter().map(|&c| fold(c)));
    if primary != core::cmp::Ordering::Equal || flags & NORM_IGNORECASE != 0 {
        return order(primary);
    }
    for (&x, &y) in a.iter().zip(b) {
        if x != y {
            // Same letter, different case: lower case sorts first.
            let x_lower = fold(x) == x;
            return if x_lower { CSTR_LESS_THAN } else { CSTR_GREATER_THAN };
        }
    }
    CSTR_EQUAL
}

/// CompareStringW: 0 on failure, else CSTR_LESS_THAN/EQUAL/GREATER_THAN.
#[unsafe(no_mangle)]
pub extern "system" fn CompareStringW(
    lcid: u32,
    flags: u32,
    a: *const u16,
    n: i32,
    b: *const u16,
    m: i32,
) -> i32 {
    if !known_lcid(lcid) || a.is_null() || b.is_null() {
        return fail_with(ERROR_INVALID_PARAMETER, 0);
    }
    compare(units(a, n), units(b, m), flags)
}

/// LCMapStringA: LCMapStringW in the ANSI code page (UTF-8): the text is
/// mapped as UTF-16 and converted back; counts are in bytes.
#[unsafe(no_mangle)]
pub extern "system" fn LCMapStringA(
    lcid: u32,
    flags: u32,
    src: *const u8,
    n: i32,
    dst: *mut u8,
    size: i32,
) -> i32 {
    if src.is_null() || n == 0 || size < 0 {
        return fail_with(ERROR_INVALID_PARAMETER, 0);
    }
    let bytes: &[u8] = if n < 0 {
        // SAFETY: the caller's NUL-terminated string (the NUL included).
        unsafe { core::ffi::CStr::from_ptr(src.cast()).to_bytes_with_nul() }
    } else {
        // SAFETY: the caller's string of `n` bytes.
        unsafe { core::slice::from_raw_parts(src, n as usize) }
    };
    let Ok(text) = core::str::from_utf8(bytes) else {
        return fail_with(ERROR_INVALID_PARAMETER, 0);
    };
    let mut wide = [0u16; 2048];
    let mut count = 0;
    for unit in text.encode_utf16() {
        let Some(slot) = wide.get_mut(count) else {
            unsupported(b"kernel32.dll!LCMapStringA(length)", bytes.len() as u64);
            return fail_with(ERROR_INSUFFICIENT_BUFFER, 0);
        };
        *slot = unit;
        count += 1;
    }
    let mut mapped = [0u16; 2048];
    let m = LCMapStringW(lcid, flags, wide.as_ptr(), count as i32, mapped.as_mut_ptr(), 2048);
    if m == 0 && count != 0 {
        return 0;
    }
    let mut out = [0u8; 8192];
    let mut len = 0;
    for c in char::decode_utf16(mapped[..m as usize].iter().copied()) {
        let c = c.unwrap_or(char::REPLACEMENT_CHARACTER);
        len += c.encode_utf8(&mut out[len..]).len();
    }
    if size == 0 {
        return len as i32;
    }
    if dst.is_null() || (size as usize) < len {
        return fail_with(ERROR_INSUFFICIENT_BUFFER, 0);
    }
    // SAFETY: the caller's buffer of `size` >= `len` bytes.
    unsafe { core::ptr::copy_nonoverlapping(out.as_ptr(), dst, len) };
    len as i32
}

/// The NUL-terminated string at `s` (a null pointer is empty).
fn c_units<'a>(s: *const u16) -> &'a [u16] {
    if s.is_null() {
        return &[];
    }
    let mut n = 0;
    // SAFETY: the caller's NUL-terminated string, read up to its NUL.
    unsafe {
        while *s.add(n) != 0 {
            n += 1;
        }
        core::slice::from_raw_parts(s, n)
    }
}

/// lstrlenW: the length in characters (0 for a null pointer).
#[unsafe(no_mangle)]
pub extern "system" fn lstrlenW(s: *const u16) -> i32 {
    c_units(s).len() as i32
}

/// lstrlenA: the length in bytes (0 for a null pointer).
#[unsafe(no_mangle)]
pub extern "system" fn lstrlenA(s: *const u8) -> i32 {
    if s.is_null() {
        return 0;
    }
    let mut n = 0;
    // SAFETY: the caller's NUL-terminated string, read up to its NUL.
    while unsafe { *s.add(n) } != 0 {
        n += 1;
    }
    n as i32
}

/// lstrcmpW: the linguistic order of CompareStringW (user locale), as
/// negative, zero or positive; a null pointer compares as empty.
#[unsafe(no_mangle)]
pub extern "system" fn lstrcmpW(a: *const u16, b: *const u16) -> i32 {
    compare(c_units(a), c_units(b), 0) - CSTR_EQUAL
}

/// lstrcmpiW: lstrcmpW ignoring case.
#[unsafe(no_mangle)]
pub extern "system" fn lstrcmpiW(a: *const u16, b: *const u16) -> i32 {
    compare(c_units(a), c_units(b), NORM_IGNORECASE) - CSTR_EQUAL
}

/// The ANSI string at `s` as UTF-16 (the ANSI code page is UTF-8; up to
/// 1024 characters compared).
fn widen(s: *const u8, out: &mut [u16; 1024]) -> &[u16] {
    let mut bytes: &[u8] = &[];
    if !s.is_null() {
        // SAFETY: the caller's NUL-terminated string.
        bytes = unsafe { core::ffi::CStr::from_ptr(s.cast()) }.to_bytes();
    }
    let text = core::str::from_utf8(bytes).unwrap_or("");
    let mut n = 0;
    for (slot, unit) in out.iter_mut().zip(text.encode_utf16()) {
        *slot = unit;
        n += 1;
    }
    &out[..n]
}

/// lstrcmpA: lstrcmpW for ANSI strings.
#[unsafe(no_mangle)]
pub extern "system" fn lstrcmpA(a: *const u8, b: *const u8) -> i32 {
    let (mut x, mut y) = ([0u16; 1024], [0u16; 1024]);
    compare(widen(a, &mut x), widen(b, &mut y), 0) - CSTR_EQUAL
}

/// lstrcmpiA: lstrcmpiW for ANSI strings.
#[unsafe(no_mangle)]
pub extern "system" fn lstrcmpiA(a: *const u8, b: *const u8) -> i32 {
    let (mut x, mut y) = ([0u16; 1024], [0u16; 1024]);
    compare(widen(a, &mut x), widen(b, &mut y), NORM_IGNORECASE) - CSTR_EQUAL
}

/// CompareStringEx(name, ...): CompareStringW for "en-US".
#[unsafe(no_mangle)]
pub extern "system" fn CompareStringEx(
    _name: *const u16,
    flags: u32,
    a: *const u16,
    n: i32,
    b: *const u16,
    m: i32,
    _version: *mut c_void,
    _reserved: *mut c_void,
    _param: isize,
) -> i32 {
    CompareStringW(LANG_EN_US, flags, a, n, b, m)
}

/// CompareStringOrdinal: by code unit, optionally ignoring case.
#[unsafe(no_mangle)]
pub extern "system" fn CompareStringOrdinal(
    a: *const u16,
    n: i32,
    b: *const u16,
    m: i32,
    ignore_case: Bool,
) -> i32 {
    if a.is_null() || b.is_null() {
        return fail_with(ERROR_INVALID_PARAMETER, 0);
    }
    let (a, b) = (units(a, n), units(b, m));
    if ignore_case != 0 {
        let up = |c: u16| map_case(c, true);
        order(a.iter().map(|&c| up(c)).cmp(b.iter().map(|&c| up(c))))
    } else {
        order(a.cmp(b))
    }
}

const CT_CTYPE1: u32 = 1;
const CT_CTYPE2: u32 = 2;
const CT_CTYPE3: u32 = 4;

/// The CT_CTYPE1 bits of a character: C1_UPPER 1, C1_LOWER 2, C1_DIGIT 4,
/// C1_SPACE 8, C1_PUNCT 0x10, C1_CNTRL 0x20, C1_BLANK 0x40, C1_XDIGIT
/// 0x80, C1_ALPHA 0x100, C1_DEFINED 0x200.
fn ctype1(c: u16) -> u16 {
    let Some(ch) = char::from_u32(c.into()) else { return 0x200 };
    let mut t = 0x200;
    if ch.is_uppercase() {
        t |= 0x1;
    }
    if ch.is_lowercase() {
        t |= 0x2;
    }
    if ch.is_ascii_digit() {
        t |= 0x4;
    }
    if ch.is_whitespace() || ch == '\u{b}' {
        t |= 0x8;
    }
    if ch.is_ascii_punctuation()
        || (!ch.is_ascii() && !ch.is_alphanumeric() && !ch.is_whitespace() && !ch.is_control())
    {
        t |= 0x10;
    }
    if ch.is_control() {
        t |= 0x20;
    }
    if ch == ' ' || ch == '\t' || ch == '\u{a0}' {
        t |= 0x40;
    }
    if ch.is_ascii_hexdigit() {
        t |= 0x80;
    }
    if ch.is_alphabetic() {
        t |= 0x100;
    }
    t
}

/// GetStringTypeW: CT_CTYPE1 classes; CT_CTYPE3 gives C3_ALPHA (0x8000)
/// for letters; CT_CTYPE2 (bidirectional layout) is all zero here.
#[unsafe(no_mangle)]
pub extern "system" fn GetStringTypeW(kind: u32, s: *const u16, n: i32, out: *mut u16) -> Bool {
    if s.is_null() || out.is_null() {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    let text = units(s, n);
    let count = text.len() + usize::from(n < 0);
    for i in 0..count {
        let c = text.get(i).copied().unwrap_or(0);
        let t = match kind {
            CT_CTYPE1 => ctype1(c),
            CT_CTYPE2 => 0,
            CT_CTYPE3 => {
                if ctype1(c) & 0x100 != 0 {
                    0x8000
                } else {
                    0
                }
            }
            _ => return fail_with(ERROR_INVALID_FLAGS, FALSE),
        };
        // SAFETY: the caller's output of `count` units.
        unsafe { *out.add(i) = t };
    }
    TRUE
}

/// GetStringTypeExW: GetStringTypeW (the locale does not change it).
#[unsafe(no_mangle)]
pub extern "system" fn GetStringTypeExW(
    _lcid: u32,
    kind: u32,
    s: *const u16,
    n: i32,
    out: *mut u16,
) -> Bool {
    GetStringTypeW(kind, s, n, out)
}

/// IsDBCSLeadByte: gala's ANSI code page is UTF-8, which has no DBCS
/// lead bytes.
#[unsafe(no_mangle)]
pub extern "system" fn IsDBCSLeadByte(_b: u8) -> Bool {
    FALSE
}

// ---- Dates and times ----

/// SYSTEMTIME as 8 WORDs: year, month, day of week, day, hour, minute,
/// second, milliseconds.
type SystemTime = [u16; 8];

fn read_time(st: *const u16) -> SystemTime {
    let mut t = [0u16; 8];
    if st.is_null() {
        GetLocalTime(t.as_mut_ptr());
    } else {
        // SAFETY: the caller's SYSTEMTIME.
        unsafe { core::ptr::copy_nonoverlapping(st, t.as_mut_ptr(), 8) };
    }
    t
}

/// Day of the week (0 = Sunday) of a date.
fn day_of_week(t: &SystemTime) -> usize {
    let days = super::text::days_from_civil(t[0].into(), t[1].into(), t[3].into());
    (days + 4).rem_euclid(7) as usize // 1970-01-01 was a Thursday
}

/// Appends `s` to `out` at `*n`; None if it does not fit.
fn push(out: &mut [u16; 128], n: &mut usize, s: &str) -> Option<()> {
    for c in s.encode_utf16() {
        *out.get_mut(*n)? = c;
        *n += 1;
    }
    Some(())
}

/// A two-digit field.
fn two(v: u16, buf: &mut [u8; 10]) -> &str {
    buf[0] = b'0' + (v / 10 % 10) as u8;
    buf[1] = b'0' + (v % 10) as u8;
    core::str::from_utf8(&buf[..2]).unwrap_or("00")
}

/// Expands a date or time picture into `out`: `date` pictures use d, M,
/// y, g; time pictures h, H, m, s, t. Text in single quotes is literal
/// ('' is a quote); other characters are copied.
fn format_picture(
    picture: &[u16],
    t: &SystemTime,
    date: bool,
    out: &mut [u16; 128],
) -> Option<usize> {
    let mut n = 0;
    let mut i = 0;
    while i < picture.len() {
        let c = picture[i];
        if c == u16::from(b'\'') {
            i += 1;
            while i < picture.len() {
                if picture[i] == u16::from(b'\'') {
                    if picture.get(i + 1) == Some(&u16::from(b'\'')) {
                        push(out, &mut n, "'")?;
                        i += 2;
                        continue;
                    }
                    break;
                }
                let mut b = [0u16; 1];
                b[0] = picture[i];
                for ch in char::decode_utf16(b).flatten() {
                    let mut tmp = [0u8; 4];
                    push(out, &mut n, ch.encode_utf8(&mut tmp))?;
                }
                i += 1;
            }
            i += 1;
            continue;
        }
        let mut run = 1;
        while picture.get(i + run) == Some(&c) {
            run += 1;
        }
        let mut digits = [0u8; 10];
        let ch = u8::try_from(c).unwrap_or(0);
        match (date, ch) {
            (true, b'd') => match run {
                1 => push(out, &mut n, decimal(t[3].into(), &mut digits))?,
                2 => push(out, &mut n, two(t[3], &mut digits))?,
                3 => push(out, &mut n, &DAYS[(day_of_week(t) + 6) % 7][..3])?,
                _ => push(out, &mut n, DAYS[(day_of_week(t) + 6) % 7])?,
            },
            (true, b'M') => {
                let m = usize::from(t[1].clamp(1, 12)) - 1;
                match run {
                    1 => push(out, &mut n, decimal(t[1].into(), &mut digits))?,
                    2 => push(out, &mut n, two(t[1], &mut digits))?,
                    3 => push(out, &mut n, &MONTHS[m][..3])?,
                    _ => push(out, &mut n, MONTHS[m])?,
                }
            }
            (true, b'y') => match run {
                1 => push(out, &mut n, decimal(u32::from(t[0] % 100), &mut digits))?,
                2 => push(out, &mut n, two(t[0] % 100, &mut digits))?,
                _ => push(out, &mut n, decimal(t[0].into(), &mut digits))?,
            },
            (true, b'g') => push(out, &mut n, "A.D.")?,
            (false, b'h') | (false, b'H') => {
                let h = if ch == b'h' {
                    let h = t[4] % 12;
                    if h == 0 { 12 } else { h }
                } else {
                    t[4]
                };
                if run == 1 {
                    push(out, &mut n, decimal(h.into(), &mut digits))?
                } else {
                    push(out, &mut n, two(h, &mut digits))?
                }
            }
            (false, b'm') | (false, b's') => {
                let v = if ch == b'm' { t[5] } else { t[6] };
                if run == 1 {
                    push(out, &mut n, decimal(v.into(), &mut digits))?
                } else {
                    push(out, &mut n, two(v, &mut digits))?
                }
            }
            (false, b't') => {
                let marker = if t[4] < 12 { "AM" } else { "PM" };
                push(out, &mut n, if run == 1 { &marker[..1] } else { marker })?
            }
            _ => {
                for _ in 0..run {
                    *out.get_mut(n)? = c;
                    n += 1;
                }
            }
        }
        i += run;
    }
    Some(n)
}

const DATE_LONGDATE: u32 = 2;

/// GetDateFormatW: a picture, or the short (default) or long date.
#[unsafe(no_mangle)]
pub extern "system" fn GetDateFormatW(
    lcid: u32,
    flags: u32,
    st: *const u16,
    format: *const u16,
    buf: *mut u16,
    size: i32,
) -> i32 {
    if !known_lcid(lcid) || size < 0 {
        return fail_with(ERROR_INVALID_PARAMETER, 0);
    }
    let t = read_time(st);
    let default: [u16; 32];
    let picture: &[u16] = if format.is_null() {
        let p = if flags & DATE_LONGDATE != 0 { "dddd, MMMM d, yyyy" } else { "M/d/yyyy" };
        let mut d = [0u16; 32];
        for (x, c) in d.iter_mut().zip(p.encode_utf16()) {
            *x = c;
        }
        default = d;
        &default[..p.len()]
    } else {
        wide_slice(format)
    };
    let mut out = [0u16; 128];
    match format_picture(picture, &t, true, &mut out) {
        Some(n) => put(&out[..n], buf, size),
        None => fail_with(ERROR_INSUFFICIENT_BUFFER, 0),
    }
}

/// GetDateFormatEx(name, ...).
#[unsafe(no_mangle)]
pub extern "system" fn GetDateFormatEx(
    _name: *const u16,
    flags: u32,
    st: *const u16,
    format: *const u16,
    buf: *mut u16,
    size: i32,
    _calendar: *const u16,
) -> i32 {
    GetDateFormatW(LANG_EN_US, flags, st, format, buf, size)
}

const TIME_NOMINUTESORSECONDS: u32 = 1;
const TIME_NOSECONDS: u32 = 2;
const TIME_NOTIMEMARKER: u32 = 4;
const TIME_FORCE24HOURFORMAT: u32 = 8;

/// GetTimeFormatW: a picture, or "h:mm:ss tt" shaped by the TIME_* flags.
#[unsafe(no_mangle)]
pub extern "system" fn GetTimeFormatW(
    lcid: u32,
    flags: u32,
    st: *const u16,
    format: *const u16,
    buf: *mut u16,
    size: i32,
) -> i32 {
    if !known_lcid(lcid) || size < 0 {
        return fail_with(ERROR_INVALID_PARAMETER, 0);
    }
    let t = read_time(st);
    let default: [u16; 16];
    let picture: &[u16] = if format.is_null() {
        let hours = if flags & TIME_FORCE24HOURFORMAT != 0 { "H" } else { "h" };
        let mut p = [0u8; 16];
        let mut len = 0;
        for part in [
            hours,
            if flags & (TIME_NOMINUTESORSECONDS) == 0 { ":mm" } else { "" },
            if flags & (TIME_NOSECONDS | TIME_NOMINUTESORSECONDS) == 0 { ":ss" } else { "" },
            if flags & (TIME_NOTIMEMARKER | TIME_FORCE24HOURFORMAT) == 0 { " tt" } else { "" },
        ] {
            p[len..len + part.len()].copy_from_slice(part.as_bytes());
            len += part.len();
        }
        let mut d = [0u16; 16];
        for (x, &c) in d.iter_mut().zip(&p[..len]) {
            *x = c.into();
        }
        default = d;
        &default[..len]
    } else {
        wide_slice(format)
    };
    let mut out = [0u16; 128];
    match format_picture(picture, &t, false, &mut out) {
        Some(n) => put(&out[..n], buf, size),
        None => fail_with(ERROR_INSUFFICIENT_BUFFER, 0),
    }
}

/// GetTimeFormatEx(name, ...).
#[unsafe(no_mangle)]
pub extern "system" fn GetTimeFormatEx(
    _name: *const u16,
    flags: u32,
    st: *const u16,
    format: *const u16,
    buf: *mut u16,
    size: i32,
) -> i32 {
    GetTimeFormatW(LANG_EN_US, flags, st, format, buf, size)
}

/// GetLocalTime: the time zone is UTC, so the system time.
#[unsafe(no_mangle)]
pub extern "system" fn GetLocalTime(st: *mut u16) {
    GetSystemTime(st);
}

/// SetLocalTime: as SetSystemTime (programs may not set the clock).
#[unsafe(no_mangle)]
pub extern "system" fn SetLocalTime(st: *const u16) -> Bool {
    super::files::SetSystemTime(st)
}

/// GetDynamicTimeZoneInformation: UTC (as GetTimeZoneInformation), key
/// name "UTC", TIME_ZONE_ID_UNKNOWN.
#[unsafe(no_mangle)]
pub extern "system" fn GetDynamicTimeZoneInformation(info: *mut u8) -> u32 {
    let id = GetTimeZoneInformation(info);
    // SAFETY: the caller's 432-byte DYNAMIC_TIME_ZONE_INFORMATION; the
    // first 172 bytes are a TIME_ZONE_INFORMATION.
    unsafe {
        core::ptr::write_bytes(info.add(172), 0, 432 - 172);
        for (i, &b) in b"UTC".iter().enumerate() {
            *(info.add(172 + 2 * i) as *mut u16) = u16::from(b);
        }
    }
    id
}

/// Adds `minutes` to a SYSTEMTIME (through FILETIME).
fn shift(from: *const u16, to: *mut u16, minutes: i64) -> Bool {
    let mut ft = 0u64;
    if super::text::SystemTimeToFileTime(from, &mut ft) == FALSE {
        return FALSE;
    }
    let ft = (ft as i64 + minutes * 600_000_000) as u64;
    super::text::FileTimeToSystemTime(&ft, to)
}

/// The Bias of a TIME_ZONE_INFORMATION (null: the current zone, UTC).
/// Daylight saving rules are not applied (TODO).
fn bias(tz: *const u8) -> i64 {
    if tz.is_null() {
        return 0;
    }
    // SAFETY: the caller's TIME_ZONE_INFORMATION starts with Bias (LONG).
    i64::from(unsafe { tz.cast::<i32>().read_unaligned() })
}

/// SystemTimeToTzSpecificLocalTime: local = UTC - Bias.
#[unsafe(no_mangle)]
pub extern "system" fn SystemTimeToTzSpecificLocalTime(
    tz: *const u8,
    utc: *const u16,
    local: *mut u16,
) -> Bool {
    shift(utc, local, -bias(tz))
}

/// TzSpecificLocalTimeToSystemTime: UTC = local + Bias.
#[unsafe(no_mangle)]
pub extern "system" fn TzSpecificLocalTimeToSystemTime(
    tz: *const u8,
    local: *const u16,
    utc: *mut u16,
) -> Bool {
    shift(local, utc, bias(tz))
}

// ---- Geographical location ----

const GEOID_US: i32 = 244;
const GEOCLASS_NATION: u32 = 16;
const GEOID_NOT_AVAILABLE: i32 = -1;

/// GetUserGeoID: the United States (244) for GEOCLASS_NATION.
#[unsafe(no_mangle)]
pub extern "system" fn GetUserGeoID(class: u32) -> i32 {
    if class == GEOCLASS_NATION { GEOID_US } else { GEOID_NOT_AVAILABLE }
}

fn geo_value(geo: i32, kind: u32) -> Option<&'static str> {
    if geo != GEOID_US {
        return None;
    }
    Some(match kind {
        1 => "244",                      // GEO_NATION
        2 => "39.76",                    // GEO_LATITUDE
        3 => "-98.5",                    // GEO_LONGITUDE
        4 => "US",                       // GEO_ISO2
        5 => "USA",                      // GEO_ISO3
        6 => "en-US",                    // GEO_RFC1766
        7 => "00000409",                 // GEO_LCID
        8 => "United States",            // GEO_FRIENDLYNAME
        9 => "United States of America", // GEO_OFFICIALNAME
        _ => return None,
    })
}

/// GetGeoInfoA/W: the United States' values; ERROR_INVALID_PARAMETER
/// otherwise.
#[unsafe(no_mangle)]
pub extern "system" fn GetGeoInfoA(
    geo: i32,
    kind: u32,
    buf: *mut u8,
    size: i32,
    _lang: u16,
) -> i32 {
    match geo_value(geo, kind) {
        Some(v) => put(v.as_bytes(), buf, size),
        None => fail_with(ERROR_INVALID_PARAMETER, 0),
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn GetGeoInfoW(
    geo: i32,
    kind: u32,
    buf: *mut u16,
    size: i32,
    _lang: u16,
) -> i32 {
    match geo_value(geo, kind) {
        Some(v) => {
            let mut w = [0u16; 32];
            for (d, c) in w.iter_mut().zip(v.encode_utf16()) {
                *d = c;
            }
            put(&w[..v.len()], buf, size)
        }
        None => fail_with(ERROR_INVALID_PARAMETER, 0),
    }
}

// ---- Locale names (the Vista API) ----

/// LCIDToLocaleName: "en-US" for gala's locale and the defaults, "" for
/// LOCALE_INVARIANT; the count includes the NUL.
#[unsafe(no_mangle)]
pub extern "system" fn LCIDToLocaleName(lcid: u32, buf: *mut u16, size: i32, _flags: u32) -> i32 {
    if !known_lcid(lcid) || size < 0 {
        return fail_with(ERROR_INVALID_PARAMETER, 0);
    }
    if lcid == 0x7f {
        return put(&[], buf, size);
    }
    put(&b"en-US".map(u16::from), buf, size)
}

/// LocaleNameToLCID: 0x409 for "en-US" (and the default, null or "");
/// 0 with ERROR_INVALID_PARAMETER for other names.
#[unsafe(no_mangle)]
pub extern "system" fn LocaleNameToLCID(name: *const u16, _flags: u32) -> u32 {
    if name.is_null() || wide_slice(name).is_empty() || is_en_us(wide_slice(name)) {
        return LANG_EN_US;
    }
    fail_with(ERROR_INVALID_PARAMETER, 0)
}

#[unsafe(no_mangle)]
pub extern "system" fn IsValidLocaleName(name: *const u16) -> Bool {
    Bool::from(!name.is_null() && (wide_slice(name).is_empty() || is_en_us(wide_slice(name))))
}

type LocaleEnumProcEx = extern "system" fn(*const u16, u32, isize) -> Bool;

/// EnumSystemLocalesEx: the one locale, "en-US" (LOCALE_SPECIFICDATA).
#[unsafe(no_mangle)]
pub extern "system" fn EnumSystemLocalesEx(
    f: Option<LocaleEnumProcEx>,
    _flags: u32,
    param: isize,
    _reserved: *mut c_void,
) -> Bool {
    let Some(f) = f else { return fail_with(ERROR_INVALID_PARAMETER, FALSE) };
    let mut name = [0u16; 6];
    name[..5].copy_from_slice(&b"en-US".map(u16::from));
    f(name.as_ptr(), 2, param); // LOCALE_SPECIFICDATA
    TRUE
}

/// GetCurrentPackageId: gala's processes are not packaged (Store) apps:
/// APPMODEL_ERROR_NO_PACKAGE, returned (not set as the last error).
#[unsafe(no_mangle)]
pub extern "system" fn GetCurrentPackageId(_length: *mut u32, _buffer: *mut u8) -> i32 {
    15700
}

/// The preferred UI languages: en-US only, as a double-NUL-terminated
/// list - names (MUI_LANGUAGE_NAME, 0x8, the default) or four-digit
/// hexadecimal ids (MUI_LANGUAGE_ID, 0x4). A null buffer asks the size.
fn preferred_ui_languages(flags: u32, count: *mut u32, buf: *mut u16, len: *mut u32) -> Bool {
    if count.is_null() || len.is_null() || flags & !(0x4 | 0x8 | 0x10 | 0x20 | 0x40 | 0x400) != 0 {
        return fail_with(ERROR_INVALID_PARAMETER, 0);
    }
    let list: &[u8] = if flags & 0x4 != 0 { b"0409\0\0" } else { b"en-US\0\0" };
    // SAFETY: the caller's outputs and buffer of `*len` characters.
    unsafe {
        *count = 1;
        if buf.is_null() {
            *len = list.len() as u32;
            return 1;
        }
        if (*len as usize) < list.len() {
            *len = list.len() as u32;
            return fail_with(ERROR_INSUFFICIENT_BUFFER, 0);
        }
        for (i, &b) in list.iter().enumerate() {
            *buf.add(i) = b.into();
        }
        *len = list.len() as u32;
    }
    1
}

#[unsafe(no_mangle)]
pub extern "system" fn GetUserPreferredUILanguages(
    flags: u32,
    count: *mut u32,
    buf: *mut u16,
    len: *mut u32,
) -> Bool {
    preferred_ui_languages(flags, count, buf, len)
}

#[unsafe(no_mangle)]
pub extern "system" fn GetSystemPreferredUILanguages(
    flags: u32,
    count: *mut u32,
    buf: *mut u16,
    len: *mut u32,
) -> Bool {
    preferred_ui_languages(flags, count, buf, len)
}

/// GetThreadPreferredUILanguages: no thread languages are ever set, so
/// the merged user and system fallbacks, en-US.
#[unsafe(no_mangle)]
pub extern "system" fn GetThreadPreferredUILanguages(
    flags: u32,
    count: *mut u32,
    buf: *mut u16,
    len: *mut u32,
) -> Bool {
    preferred_ui_languages(flags, count, buf, len)
}

#[unsafe(no_mangle)]
pub extern "system" fn GetProcessPreferredUILanguages(
    flags: u32,
    count: *mut u32,
    buf: *mut u16,
    len: *mut u32,
) -> Bool {
    preferred_ui_languages(flags, count, buf, len)
}

/// One UTF-16 code unit folded for a case-insensitive comparison (simple
/// lowercase mapping inside the BMP).
fn fold(c: u16) -> u16 {
    match char::from_u32(c.into()) {
        Some(ch) => {
            let mut lower = ch.to_lowercase();
            match (lower.next(), lower.next()) {
                (Some(l), None) if (l as u32) < 0x1_0000 => l as u16,
                _ => c,
            }
        }
        None => c,
    }
}

/// The caller's string: `len` code units, or NUL-terminated for -1.
fn counted<'a>(p: *const u16, len: i32) -> &'a [u16] {
    if p.is_null() {
        return &[];
    }
    if len < 0 {
        return wide_slice(p);
    }
    // SAFETY: the caller's `len` code units.
    unsafe { core::slice::from_raw_parts(p, len as usize) }
}

/// FindNLSStringEx: FIND_STARTSWITH, FIND_ENDSWITH, FIND_FROMSTART or
/// FIND_FROMEND, case-insensitively with NORM_IGNORECASE or
/// LINGUISTIC_IGNORECASE; the index found (-1 if none) and its length.
/// Strings are compared code unit by code unit (no linguistic
/// equivalences; flags asking for them are recorded).
#[unsafe(no_mangle)]
pub extern "system" fn FindNLSStringEx(
    locale: *const u16,
    flags: u32,
    source: *const u16,
    source_len: i32,
    value: *const u16,
    value_len: i32,
    found: *mut i32,
    _version: *mut c_void,
    _reserved: *mut c_void,
    _handle: isize,
) -> i32 {
    const STARTS: u32 = 0x10_0000;
    const ENDS: u32 = 0x20_0000;
    const FROM_START: u32 = 0x40_0000;
    const FROM_END: u32 = 0x80_0000;
    const IGNORE_CASE: u32 = 0x1 | 0x10;
    if !locale.is_null() && !is_en_us(wide_slice(locale)) && !wide_slice(locale).is_empty() {
        return fail_with(ERROR_INVALID_PARAMETER, -1);
    }
    let mode = flags & (STARTS | ENDS | FROM_START | FROM_END);
    if source.is_null() || value.is_null() || mode.count_ones() != 1 {
        return fail_with(ERROR_INVALID_PARAMETER, -1);
    }
    // NORM_IGNORENONSPACE, NORM_IGNORESYMBOLS, LINGUISTIC_IGNOREDIACRITIC,
    // kana and width: equivalences gala does not make.
    if flags & (0x2 | 0x4 | 0x20 | 0x1_0000 | 0x2_0000) != 0 {
        unsupported(b"kernel32.dll!FindNLSStringEx(flags)", flags.into());
    }
    let (src, val) = (counted(source, source_len), counted(value, value_len));
    let fold_case = flags & IGNORE_CASE != 0;
    let same =
        |a: &[u16]| a.iter().zip(val).all(|(&x, &y)| x == y || (fold_case && fold(x) == fold(y)));
    let at = if val.len() > src.len() {
        None
    } else {
        let last = src.len() - val.len();
        match mode {
            STARTS => same(&src[..val.len()]).then_some(0),
            ENDS => same(&src[last..]).then_some(last),
            FROM_START => (0..=last).find(|&i| same(&src[i..i + val.len()])),
            _ => (0..=last).rev().find(|&i| same(&src[i..i + val.len()])),
        }
    };
    SetLastError(0);
    match at {
        Some(i) => {
            if !found.is_null() {
                // SAFETY: the caller's output.
                unsafe { *found = val.len() as i32 };
            }
            i as i32
        }
        None => -1,
    }
}

/// GetNLSVersionEx(function, lpLocaleName, lpVersionInformation): the
/// version of the sorting gala's CompareString uses (one, for every
/// locale: it does not change). NLSVERSIONINFOEX {dwNLSVersionInfoSize,
/// dwNLSVersion, dwDefinedVersion, dwEffectiveId, guidCustomVersion}.
#[unsafe(no_mangle)]
pub extern "system" fn GetNLSVersionEx(function: u32, _locale: *const u16, info: *mut u32) -> Bool {
    const COMPARE_STRING: u32 = 1;
    /// gala's sort version (the defined and the effective one).
    const VERSION: u32 = 0x0001_0000;
    if function != COMPARE_STRING {
        return fail_with(ERROR_INVALID_FLAGS, FALSE);
    }
    if info.is_null() {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    // SAFETY: the caller's NLSVERSIONINFOEX (its size field first).
    unsafe {
        if *info != 32 {
            return fail_with(ERROR_INSUFFICIENT_BUFFER, FALSE);
        }
        *info.add(1) = VERSION;
        *info.add(2) = VERSION;
        *info.add(3) = 0x0409; // the effective locale's LCID (en-US)
        for k in 4..8 {
            *info.add(k) = 0;
        }
    }
    TRUE
}

/// MulDiv(a, b, c): a * b / c with a 64-bit product, rounded half away
/// from zero; -1 when c is 0 or the result does not fit.
#[unsafe(no_mangle)]
pub extern "system" fn MulDiv(a: i32, b: i32, c: i32) -> i32 {
    if c == 0 {
        return -1;
    }
    let p = i64::from(a) * i64::from(b);
    let c = i64::from(c);
    let half = c.abs() / 2;
    let q =
        if (p < 0) == (c < 0) { (p.abs() + half) / c.abs() } else { -((p.abs() + half) / c.abs()) };
    i32::try_from(q).unwrap_or(-1)
}

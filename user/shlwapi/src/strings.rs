//! String helpers (shlwapi.h) for what SQL Server LocalDB and wxWidgets
//! import: comparisons (through kernel32's CompareStringW in the user's
//! locale, as documented: "compares as lstrcmp"), substring searches,
//! the "logical" (numbers by value) order Explorer sorts names in, and
//! SHDeleteKeyW.

#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use crate::paths_more::{chars, fold};

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn CompareStringW(lcid: u32, flags: u32, a: *const u16, n: i32, b: *const u16, m: i32) -> i32;
    fn LoadLibraryW(name: *const u16) -> usize;
    fn GetProcAddress(module: usize, name: *const u8) -> usize;
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "ntdll", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "ntdll", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GalaRecordMissing(name: *const u8, length: usize, what: u32, detail: u64) -> u32;
}

const LOCALE_USER_DEFAULT: u32 = 0x400;
const NORM_IGNORECASE: u32 = 1;

pub(crate) fn unsupported(what: &[u8], detail: u64) {
    // SAFETY: a valid buffer; 2 is GalaRecordMissing's "unsupported".
    unsafe { GalaRecordMissing(what.as_ptr(), what.len(), 2, detail) };
}

/// -1, 0 or 1 for `a` against `b` (CompareStringW in the user's locale;
/// ordinal if kernel32 refuses, recorded).
fn compare(a: &[u16], b: &[u16], flags: u32) -> i32 {
    // SAFETY: the slices' characters.
    let r = unsafe {
        CompareStringW(
            LOCALE_USER_DEFAULT,
            flags,
            a.as_ptr(),
            a.len() as i32,
            b.as_ptr(),
            b.len() as i32,
        )
    };
    if r == 0 {
        unsupported(b"shlwapi.dll!StrCmp(CompareStringW)", flags.into());
        let key = |c: u16| if flags & NORM_IGNORECASE != 0 { fold(c) } else { c };
        return a.iter().map(|&c| key(c)).cmp(b.iter().map(|&c| key(c))) as i32;
    }
    r - 2
}

/// StrCmpW: <0, 0 or >0, case-sensitive.
#[unsafe(no_mangle)]
pub extern "system" fn StrCmpW(a: *const u16, b: *const u16) -> i32 {
    compare(chars(a), chars(b), 0)
}

/// StrCmpIW: as StrCmpW, case ignored.
#[unsafe(no_mangle)]
pub extern "system" fn StrCmpIW(a: *const u16, b: *const u16) -> i32 {
    compare(chars(a), chars(b), NORM_IGNORECASE)
}

/// The first `n` characters of `s` (all of a shorter string).
fn prefix(s: &[u16], n: i32) -> &[u16] {
    &s[..s.len().min(n.max(0) as usize)]
}

/// StrCmpNIW: the first `n` characters compared, case ignored.
#[unsafe(no_mangle)]
pub extern "system" fn StrCmpNIW(a: *const u16, b: *const u16, n: i32) -> i32 {
    compare(prefix(chars(a), n), prefix(chars(b), n), NORM_IGNORECASE)
}

/// The first occurrence of `needle` in `hay` (equal by `eq`), or NULL
/// (also for an empty needle, as Windows answers).
fn find(hay: *const u16, needle: *const u16, eq: fn(u16, u16) -> bool) -> *const u16 {
    let (h, n) = (chars(hay), chars(needle));
    if hay.is_null() || n.is_empty() || n.len() > h.len() {
        return core::ptr::null();
    }
    match h.windows(n.len()).position(|w| w.iter().zip(n).all(|(&a, &b)| eq(a, b))) {
        Some(i) => hay.wrapping_add(i),
        None => core::ptr::null(),
    }
}

/// StrStrW: the first occurrence, case-sensitive.
#[unsafe(no_mangle)]
pub extern "system" fn StrStrW(hay: *const u16, needle: *const u16) -> *const u16 {
    find(hay, needle, |a, b| a == b)
}

/// StrStrIW: the first occurrence, case ignored (ASCII and Latin-1).
#[unsafe(no_mangle)]
pub extern "system" fn StrStrIW(hay: *const u16, needle: *const u16) -> *const u16 {
    find(hay, needle, |a, b| fold(a) == fold(b))
}

fn digit(c: u16) -> bool {
    (0x30..=0x39).contains(&c)
}

/// The run at the start of `s`: digits or not-digits.
fn run(s: &[u16]) -> &[u16] {
    let numeric = digit(s[0]);
    let n = s.iter().position(|&c| digit(c) != numeric).unwrap_or(s.len());
    &s[..n]
}

/// Two digit runs by value: leading zeros skipped, then the longer
/// number is greater, else digit by digit.
fn by_value(a: &[u16], b: &[u16]) -> i32 {
    fn strip(s: &[u16]) -> &[u16] {
        let z = s.iter().position(|&c| c != 0x30).unwrap_or(s.len());
        &s[z..]
    }
    let (a, b) = (strip(a), strip(b));
    a.len().cmp(&b.len()).then_with(|| a.cmp(b)) as i32
}

/// StrCmpLogicalW: -1, 0 or 1; digit runs compare as numbers (and before
/// text), text runs as StrCmpIW (Explorer's order: "2string" <
/// "20string" < "st2ring" < "string2" < "string20").
#[unsafe(no_mangle)]
pub extern "system" fn StrCmpLogicalW(a: *const u16, b: *const u16) -> i32 {
    let (mut a, mut b) = (chars(a), chars(b));
    while !a.is_empty() && !b.is_empty() {
        let (ra, rb) = (run(a), run(b));
        let r = match (digit(ra[0]), digit(rb[0])) {
            (true, true) => by_value(ra, rb),
            (true, false) => -1,
            (false, true) => 1,
            (false, false) => compare(ra, rb, NORM_IGNORECASE),
        };
        if r != 0 {
            return r.signum();
        }
        a = &a[ra.len()..];
        b = &b[rb.len()..];
    }
    (a.len() as i32 - b.len() as i32).signum()
}

/// SHDeleteKeyW: `sub` under `key` and everything below it
/// (advapi32's RegDeleteTreeW, found at run time so that loading
/// shlwapi does not need advapi32); ERROR_FILE_NOT_FOUND when it is not
/// there.
#[unsafe(no_mangle)]
pub extern "system" fn SHDeleteKeyW(key: usize, sub: *const u16) -> i32 {
    let name: [u16; 13] = [
        b'a' as u16,
        b'd' as u16,
        b'v' as u16,
        b'a' as u16,
        b'p' as u16,
        b'i' as u16,
        b'3' as u16,
        b'2' as u16,
        b'.' as u16,
        b'd' as u16,
        b'l' as u16,
        b'l' as u16,
        0,
    ];
    // SAFETY: a NUL-terminated name; then a NUL-terminated export name.
    let f = unsafe {
        let module = LoadLibraryW(name.as_ptr());
        if module == 0 { 0 } else { GetProcAddress(module, c"RegDeleteTreeW".as_ptr().cast()) }
    };
    if f == 0 {
        unsupported(b"shlwapi.dll!SHDeleteKeyW(no advapi32)", 0);
        return 127; // ERROR_PROC_NOT_FOUND
    }
    // SAFETY: RegDeleteTreeW's documented signature.
    let delete_tree: extern "system" fn(usize, *const u16) -> i32 =
        unsafe { core::mem::transmute(f) };
    delete_tree(key, sub)
}

/// SHAutoComplete: gala's edit controls have no autocompletion yet, so
/// it is not attached: E_NOTIMPL (a documented failure; the edit control
/// works without it), recorded.
#[unsafe(no_mangle)]
pub extern "system" fn SHAutoComplete(_edit: usize, flags: u32) -> i32 {
    unsupported(b"shlwapi.dll!SHAutoComplete", flags.into());
    0x8000_4001u32 as i32
}

/// StrToIntW: a decimal number with an optional '-' (no '+', no leading
/// spaces), as far as the digits go; 0 when there are none. Overflow
/// wraps (Wine's string.c tests, checked on Windows: "4294967319" is 23).
#[unsafe(no_mangle)]
pub extern "system" fn StrToIntW(s: *const u16) -> i32 {
    let s = chars(s);
    let (negative, digits) = match s.first() {
        Some(&c) if c == u16::from(b'-') => (true, &s[1..]),
        _ => (false, s),
    };
    let mut v = 0i32;
    for &c in digits.iter().take_while(|&&c| (u16::from(b'0')..=u16::from(b'9')).contains(&c)) {
        v = v.wrapping_mul(10).wrapping_add(i32::from(c - u16::from(b'0')));
    }
    if negative { v.wrapping_neg() } else { v }
}

/// StrTrimW: removes the characters of `trim` from both ends of `s` in
/// place; TRUE when any were removed.
#[unsafe(no_mangle)]
pub extern "system" fn StrTrimW(s: *mut u16, trim: *const u16) -> i32 {
    if s.is_null() || trim.is_null() {
        return 0;
    }
    let set = chars(trim);
    let text = chars(s);
    let start = text.iter().position(|c| !set.contains(c)).unwrap_or(text.len());
    let end = text.iter().rposition(|c| !set.contains(c)).map_or(start, |i| i + 1);
    if start == 0 && end == text.len() {
        return 0;
    }
    // SAFETY: the caller's string: the kept part moves to its start and
    // a NUL follows it.
    unsafe {
        core::ptr::copy(s.add(start), s, end - start);
        *s.add(end - start) = 0;
    }
    1
}

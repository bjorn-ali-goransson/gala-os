//! The rest of kernel32's national-language functions (ROADMAP OF: what
//! Office's apps import once each): FoldStringW (Unicode normalization
//! from the tables `tools/nls_tables.py` writes), number and currency
//! formats (GetNumberFormat*, GetCurrencyFormat*), ResolveLocaleName and
//! the ANSI CompareStringA, GetStringTypeA and GetStringTypeExA. gala has
//! one locale, en-US (locale.rs); its ANSI code page is UTF-8.

use super::locale::{
    GetLocaleInfoW, LANG_EN_US, compare_units, ctype1, is_en_us, known_lcid, put, units,
};
use super::*;

mod tables;

const ERROR_INSUFFICIENT_BUFFER: u32 = 122;
const ERROR_INVALID_FLAGS: u32 = 1004;

// ---- UTF-8 (the ANSI code page) -------------------------------------------

/// The bytes of an ANSI argument: `n` of them, or up to the NUL for -1
/// (with `with_nul`, the NUL too).
fn ansi_bytes<'a>(s: *const u8, n: i32, with_nul: bool) -> &'a [u8] {
    let len = if n < 0 {
        // SAFETY: the caller's NUL-terminated string.
        unsafe { strlen(s) + usize::from(with_nul) }
    } else {
        n as usize
    };
    // SAFETY: the caller's string of `len` bytes.
    unsafe { core::slice::from_raw_parts(s, len) }
}

/// UTF-8 bytes as UTF-16 units; a byte that is not UTF-8 is U+FFFD.
fn utf16_of(bytes: &[u8]) -> impl Iterator<Item = u16> + Clone + '_ {
    bytes.utf8_chunks().flat_map(|chunk| {
        let bad = (!chunk.invalid().is_empty()).then_some(0xfffd);
        chunk.valid().encode_utf16().chain(bad)
    })
}

/// An ANSI string into `out` as UTF-16 with a NUL; None if it does not fit.
fn widen(s: *const u8, out: &mut [u16]) -> Option<&[u16]> {
    let mut n = 0;
    for unit in utf16_of(ansi_bytes(s, -1, false)) {
        *out.get_mut(n)? = unit;
        n += 1;
    }
    *out.get_mut(n)? = 0;
    Some(&out[..n])
}

/// UTF-16 `text` and a NUL into the caller's ANSI buffer of `size` bytes,
/// as `put` does for wide text.
fn put_ansi(text: &[u16], buf: *mut u8, size: i32) -> i32 {
    let mut bytes = [0u8; 1024];
    let mut n = 0;
    for c in char::decode_utf16(text.iter().copied()) {
        let c = c.unwrap_or(char::REPLACEMENT_CHARACTER);
        if n + c.len_utf8() > bytes.len() {
            unsupported(b"kernel32.dll!(ANSI result length)", text.len() as u64);
            return fail_with(ERROR_INSUFFICIENT_BUFFER, 0);
        }
        n += c.encode_utf8(&mut bytes[n..]).len();
    }
    put(&bytes[..n], buf, size)
}

// ---- CompareStringA, GetStringTypeA ---------------------------------------

/// CompareStringA: CompareStringW's comparison of the strings as UTF-16
/// (-1: up to the NUL; 0: the empty string).
#[unsafe(no_mangle)]
pub extern "system" fn CompareStringA(
    lcid: u32,
    flags: u32,
    a: *const u8,
    n: i32,
    b: *const u8,
    m: i32,
) -> i32 {
    if !known_lcid(lcid) || a.is_null() || b.is_null() {
        return fail_with(ERROR_INVALID_PARAMETER, 0);
    }
    let (a, b) = (ansi_bytes(a, n, false), ansi_bytes(b, m, false));
    compare_units(utf16_of(a), utf16_of(b), flags)
}

/// GetStringTypeA(lcid, type, src, count, out): GetStringTypeW's types,
/// one for each character of the ANSI string (-1 counts its NUL).
#[unsafe(no_mangle)]
pub extern "system" fn GetStringTypeA(
    _lcid: u32,
    kind: u32,
    s: *const u8,
    n: i32,
    out: *mut u16,
) -> Bool {
    if s.is_null() || out.is_null() || !matches!(kind, 1 | 2 | 4) {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    for (i, c) in utf16_of(ansi_bytes(s, n, true)).enumerate() {
        let t = match kind {
            1 => ctype1(c),
            2 => 0,
            _ if ctype1(c) & 0x100 != 0 => 0x8000, // C3_ALPHA
            _ => 0,
        };
        // SAFETY: the caller's output, one type for each character.
        unsafe { *out.add(i) = t };
    }
    TRUE
}

/// GetStringTypeExA: GetStringTypeA (the same arguments).
#[unsafe(no_mangle)]
pub extern "system" fn GetStringTypeExA(
    lcid: u32,
    kind: u32,
    s: *const u8,
    n: i32,
    out: *mut u16,
) -> Bool {
    GetStringTypeA(lcid, kind, s, n, out)
}

// ---- FoldStringW ------------------------------------------------------------

const MAP_FOLDCZONE: u32 = 0x10;
const MAP_PRECOMPOSED: u32 = 0x20;
const MAP_COMPOSITE: u32 = 0x40;
const MAP_FOLDDIGITS: u32 = 0x80;
const MAP_EXPAND_LIGATURES: u32 = 0x2000;

/// The digits MAP_FOLDDIGITS maps: (first character, its digit, the last
/// digit of the run), as Windows maps them (Wine's test_FoldStringW,
/// run on Windows).
const DIGITS: &[(u32, u8, u8)] = &[
    (0x0030, 0, 9),
    (0x00b2, 2, 3),
    (0x00b9, 1, 1),
    (0x0660, 0, 9),
    (0x06f0, 0, 9),
    (0x07c0, 0, 9),
    (0x0966, 0, 9),
    (0x09e6, 0, 9),
    (0x0a66, 0, 9),
    (0x0ae6, 0, 9),
    (0x0b66, 0, 9),
    (0x0be6, 0, 9),
    (0x0c66, 0, 9),
    (0x0c78, 0, 3),
    (0x0c7c, 1, 3),
    (0x0ce6, 0, 9),
    (0x0d66, 0, 9),
    (0x0de6, 0, 9),
    (0x0e50, 0, 9),
    (0x0ed0, 0, 9),
    (0x0f20, 0, 9),
    (0x1040, 0, 9),
    (0x1090, 0, 9),
    (0x1369, 1, 9),
    (0x17e0, 0, 9),
    (0x1810, 0, 9),
    (0x1946, 0, 9),
    (0x19d0, 0, 9),
    (0x19da, 1, 1),
    (0x1a80, 0, 9),
    (0x1a90, 0, 9),
    (0x1b50, 0, 9),
    (0x1bb0, 0, 9),
    (0x1c40, 0, 9),
    (0x1c50, 0, 9),
    (0x2070, 0, 0),
    (0x2074, 4, 9),
    (0x2080, 0, 9),
    (0x2460, 1, 9),
    (0x2474, 1, 9),
    (0x2488, 1, 9),
    (0x24ea, 0, 0),
    (0x24f5, 1, 9),
    (0x24ff, 0, 0),
    (0x2776, 1, 9),
    (0x2780, 1, 9),
    (0x278a, 1, 9),
    (0x3007, 0, 0),
    (0x3021, 1, 9),
    (0xa620, 0, 9),
    (0xa8d0, 0, 9),
    (0xa8e0, 0, 9),
    (0xa900, 0, 9),
    (0xa9d0, 0, 9),
    (0xa9f0, 0, 9),
    (0xaa50, 0, 9),
    (0xabf0, 0, 9),
    (0xff10, 0, 9),
    (0x10107, 1, 9),
    (0x10320, 1, 1),
    (0x10321, 5, 5),
    (0x104a0, 0, 9),
    (0x10a40, 1, 4),
    (0x10d30, 0, 9),
    (0x10d40, 0, 9),
    (0x10e60, 1, 9),
    (0x11052, 1, 9),
    (0x11066, 0, 9),
    (0x110f0, 0, 9),
    (0x11136, 0, 9),
    (0x111d0, 0, 9),
    (0x112f0, 0, 9),
    (0x11450, 0, 9),
    (0x114d0, 0, 9),
    (0x11650, 0, 9),
    (0x116c0, 0, 9),
    (0x116d0, 0, 9),
    (0x116da, 0, 9),
    (0x11730, 0, 9),
    (0x118e0, 0, 9),
    (0x11950, 0, 9),
    (0x11bf0, 0, 9),
    (0x11c50, 0, 9),
    (0x11d50, 0, 9),
    (0x11da0, 0, 9),
    (0x11de0, 0, 9),
    (0x11f50, 0, 9),
    (0x16130, 0, 9),
    (0x16a60, 0, 9),
    (0x16ac0, 0, 9),
    (0x16b50, 0, 9),
    (0x16d70, 0, 9),
    (0x1ccf0, 0, 9),
    (0x1d7ce, 0, 9),
    (0x1d7d8, 0, 9),
    (0x1d7e2, 0, 9),
    (0x1d7ec, 0, 9),
    (0x1d7f6, 0, 9),
    (0x1e140, 0, 9),
    (0x1e2f0, 0, 9),
    (0x1e4f0, 0, 9),
    (0x1e5f1, 0, 9),
    (0x1e950, 0, 9),
    (0x1f100, 0, 0),
    (0x1f101, 0, 9),
    (0x1fbf0, 0, 9),
];

/// MAP_EXPAND_LIGATURES: each ligature and its letters, as Windows
/// expands them (Wine's test_FoldStringW, run on Windows).
const LIGATURES: &[(u32, &[u16])] = &[
    (0x00c6, &[0x41, 0x45]),
    (0x00de, &[0x54, 0x48]),
    (0x00df, &[0x73, 0x73]),
    (0x00e6, &[0x61, 0x65]),
    (0x00fe, &[0x74, 0x68]),
    (0x0132, &[0x49, 0x4a]),
    (0x0133, &[0x69, 0x6a]),
    (0x0152, &[0x4f, 0x45]),
    (0x0153, &[0x6f, 0x65]),
    (0x01c4, &[0x44, 0x017d]),
    (0x01c5, &[0x44, 0x017e]),
    (0x01c6, &[0x64, 0x017e]),
    (0x01c7, &[0x4c, 0x4a]),
    (0x01c8, &[0x4c, 0x6a]),
    (0x01c9, &[0x6c, 0x6a]),
    (0x01ca, &[0x4e, 0x4a]),
    (0x01cb, &[0x4e, 0x6a]),
    (0x01cc, &[0x6e, 0x6a]),
    (0x01e2, &[0x0100, 0x0112]),
    (0x01e3, &[0x0101, 0x0113]),
    (0x01f1, &[0x44, 0x5a]),
    (0x01f2, &[0x44, 0x7a]),
    (0x01f3, &[0x64, 0x7a]),
    (0x01fc, &[0x00c1, 0x00c9]),
    (0x01fd, &[0x00e1, 0x00e9]),
    (0x05f0, &[0x05d5, 0x05d5]),
    (0x05f1, &[0x05d5, 0x05d9]),
    (0x05f2, &[0x05d9, 0x05d9]),
    (0xfb00, &[0x66, 0x66]),
    (0xfb01, &[0x66, 0x69]),
    (0xfb02, &[0x66, 0x6c]),
    (0xfb03, &[0x66, 0x66, 0x69]),
    (0xfb04, &[0x66, 0x66, 0x6c]),
    (0xfb05, &[0x017f, 0x74]),
    (0xfb06, &[0x73, 0x74]),
];

// Hangul syllables (Unicode's chapter 3.12): arithmetic, not tabled.
const S_BASE: u32 = 0xac00;
const L_BASE: u32 = 0x1100;
const V_BASE: u32 = 0x1161;
const T_BASE: u32 = 0x11a7;
const L_COUNT: u32 = 19;
const V_COUNT: u32 = 21;
const T_COUNT: u32 = 28;
const N_COUNT: u32 = V_COUNT * T_COUNT;
const S_COUNT: u32 = L_COUNT * N_COUNT;

/// A character's canonical combining class.
fn ccc(c: u32) -> u8 {
    let Ok(c) = u16::try_from(c) else { return 0 };
    tables::CCC.binary_search_by_key(&c, |e| e.0).map_or(0, |i| tables::CCC[i].1)
}

/// A character's full canonical (or with `compat`, compatibility)
/// decomposition as UTF-16, if it has one in the tables.
fn decomposition(c: u32, compat: bool) -> Option<&'static [u16]> {
    let c = u16::try_from(c).ok()?;
    let i = tables::DECOMP.binary_search_by_key(&c, |e| e.0).ok()?;
    let (_, start, len, kstart, klen) = tables::DECOMP[i];
    let (start, len) = if compat { (kstart, klen) } else { (start, len) };
    tables::DATA.get(start as usize..start as usize + len as usize)
}

/// The primary composite of `a` and `b`, if there is one.
fn composite(a: u32, b: u32) -> Option<u32> {
    if (L_BASE..L_BASE + L_COUNT).contains(&a) && (V_BASE..V_BASE + V_COUNT).contains(&b) {
        return Some(S_BASE + ((a - L_BASE) * V_COUNT + (b - V_BASE)) * T_COUNT);
    }
    if (S_BASE..S_BASE + S_COUNT).contains(&a)
        && (a - S_BASE).is_multiple_of(T_COUNT)
        && (T_BASE + 1..T_BASE + T_COUNT).contains(&b)
    {
        return Some(a + (b - T_BASE));
    }
    let (a, b) = (u16::try_from(a).ok()?, u16::try_from(b).ok()?);
    let i = tables::COMPOSE.binary_search_by_key(&(a, b), |e| (e.0, e.1)).ok()?;
    Some(tables::COMPOSE[i].2.into())
}

/// Where FoldStringW's result goes: counted always, written while it fits.
struct Out {
    dst: *mut u16,
    size: usize,
    n: usize,
}

impl Out {
    fn unit(&mut self, u: u16) {
        if self.n < self.size {
            // SAFETY: inside the caller's buffer of `size` units.
            unsafe { *self.dst.add(self.n) = u };
        }
        self.n += 1;
    }

    fn char(&mut self, c: u32) {
        match char::from_u32(c) {
            Some(ch) => {
                let mut b = [0u16; 2];
                for &u in ch.encode_utf16(&mut b).iter() {
                    self.unit(u);
                }
            }
            // An unpaired surrogate, kept as it was.
            None => self.unit(c as u16),
        }
    }
}

/// The longest run of a starter and its combining marks composed as one;
/// a longer run is passed on uncomposed.
const RUN: usize = 32;

/// Composes runs of a starter and its marks (canonical order, then
/// Unicode's canonical composition) and passes them on.
struct Composer {
    run: [u32; RUN],
    len: usize,
}

impl Composer {
    fn push(&mut self, c: u32, out: &mut Out) {
        if ccc(c) == 0 {
            // Two starters: Hangul's L+V and LV+T, and the few pairs of
            // starters that compose.
            if self.len == 1
                && ccc(self.run[0]) == 0
                && let Some(x) = composite(self.run[0], c)
            {
                self.run[0] = x;
                return;
            }
            self.flush(out);
        } else if self.len == RUN {
            unsupported(b"kernel32.dll!FoldStringW(combining run)", RUN as u64);
            self.flush(out);
        }
        self.run[self.len] = c;
        self.len += 1;
    }

    fn flush(&mut self, out: &mut Out) {
        let run = &mut self.run[..self.len];
        // Canonical order: the marks after a starter by combining class
        // (a stable insertion sort).
        let first = usize::from(run.first().is_some_and(|&c| ccc(c) == 0));
        for i in first + 1..run.len() {
            let mut j = i;
            while j > first && ccc(run[j - 1]) > ccc(run[j]) {
                run.swap(j - 1, j);
                j -= 1;
            }
        }
        let mut kept = 0;
        if first == 1 {
            let mut starter = run[0];
            let mut last_cc: Option<u8> = None;
            let mut rest = [0u32; RUN];
            for &c in &run[1..] {
                let cc = ccc(c);
                let blocked = last_cc.is_some_and(|l| l == 0 || l >= cc);
                match composite(starter, c).filter(|_| !blocked) {
                    Some(x) => starter = x,
                    None => {
                        rest[kept] = c;
                        kept += 1;
                        last_cc = Some(cc);
                    }
                }
            }
            out.char(starter);
            for &c in &rest[..kept] {
                out.char(c);
            }
        } else {
            for &c in run.iter() {
                out.char(c);
            }
        }
        self.len = 0;
    }
}

/// The digit MAP_FOLDDIGITS makes of `c`, if it is one.
fn ascii_digit(c: u32) -> Option<u32> {
    let i = DIGITS.partition_point(|d| d.0 <= c).checked_sub(1)?;
    let (start, first, last) = DIGITS[i];
    let offset = c - start;
    (offset <= u32::from(last - first)).then(|| u32::from(b'0' + first) + offset)
}

/// The steps after decomposition: digits, ligatures, then composition
/// or output.
fn fold_tail(flags: u32, c: u32, composer: &mut Option<Composer>, out: &mut Out) {
    let c = if flags & MAP_FOLDDIGITS != 0 { ascii_digit(c).unwrap_or(c) } else { c };
    let mut emit = |c: u32| match composer {
        Some(composer) => composer.push(c, out),
        None => out.char(c),
    };
    match LIGATURES.iter().find(|l| l.0 == c) {
        Some(l) if flags & MAP_EXPAND_LIGATURES != 0 => l.1.iter().for_each(|&u| emit(u.into())),
        _ => emit(c),
    }
}

/// FoldStringW(flags, src, srclen, dst, dstlen): MAP_FOLDCZONE is
/// Unicode's NFKC (compatibility decomposition, then composition),
/// MAP_COMPOSITE its full canonical decomposition, MAP_PRECOMPOSED
/// canonical composition, MAP_FOLDDIGITS and MAP_EXPAND_LIGATURES as
/// Windows maps them; for the Basic Multilingual Plane (other planes are
/// kept, their digits folded). -1 includes the NUL; a dstlen of 0 asks
/// the size.
#[unsafe(no_mangle)]
pub extern "system" fn FoldStringW(
    flags: u32,
    src: *const u16,
    n: i32,
    dst: *mut u16,
    size: i32,
) -> i32 {
    if src.is_null()
        || n == 0
        || size < 0
        || (size != 0 && dst.is_null())
        || src == dst.cast_const()
    {
        return fail_with(ERROR_INVALID_PARAMETER, 0);
    }
    const VALID: u32 =
        MAP_FOLDCZONE | MAP_PRECOMPOSED | MAP_COMPOSITE | MAP_FOLDDIGITS | MAP_EXPAND_LIGATURES;
    let both = |x: u32, y: u32| flags & x != 0 && flags & y != 0;
    if flags == 0
        || flags & !VALID != 0
        || both(MAP_PRECOMPOSED, MAP_COMPOSITE)
        || both(MAP_EXPAND_LIGATURES, MAP_PRECOMPOSED | MAP_COMPOSITE)
    {
        return fail_with(ERROR_INVALID_FLAGS, 0);
    }
    let text = if n < 0 {
        // SAFETY: the caller's NUL-terminated string, its NUL included.
        unsafe { core::slice::from_raw_parts(src, wide_len(src) + 1) }
    } else {
        units(src, n)
    };
    let mut out = Out { dst, size: size as usize, n: 0 };
    let mut composer = (flags & (MAP_FOLDCZONE | MAP_PRECOMPOSED) != 0)
        .then_some(Composer { run: [0; RUN], len: 0 });
    for c in char::decode_utf16(text.iter().copied()) {
        let c = c.map_or_else(|e| u32::from(e.unpaired_surrogate()), u32::from);
        let decomposed = if flags & MAP_FOLDCZONE != 0 {
            decomposition(c, true)
        } else if flags & MAP_COMPOSITE != 0 {
            decomposition(c, false)
        } else {
            None
        };
        let hangul = flags & MAP_COMPOSITE != 0 && (S_BASE..S_BASE + S_COUNT).contains(&c);
        if let Some(units) = decomposed {
            for d in char::decode_utf16(units.iter().copied()) {
                let d = d.map_or_else(|e| u32::from(e.unpaired_surrogate()), u32::from);
                fold_tail(flags, d, &mut composer, &mut out);
            }
        } else if hangul {
            let s = c - S_BASE;
            fold_tail(flags, L_BASE + s / N_COUNT, &mut composer, &mut out);
            fold_tail(flags, V_BASE + (s % N_COUNT) / T_COUNT, &mut composer, &mut out);
            if !s.is_multiple_of(T_COUNT) {
                fold_tail(flags, T_BASE + s % T_COUNT, &mut composer, &mut out);
            }
        } else {
            fold_tail(flags, c, &mut composer, &mut out);
        }
    }
    if let Some(composer) = &mut composer {
        composer.flush(&mut out);
    }
    if size != 0 && out.n > size as usize {
        return fail_with(ERROR_INSUFFICIENT_BUFFER, 0);
    }
    out.n as i32
}

// ---- ResolveLocaleName --------------------------------------------------------

/// ResolveLocaleName(name, buf, size): the specific locale a name stands
/// for, with its NUL: en-US for "en", "en-US" and an English name gala
/// does not know (as Windows resolves an unknown region by its language);
/// "" for the invariant locale and for a language gala has no locale for
/// (recorded: Windows may know it); NULL is the system default.
/// ERROR_INVALID_PARAMETER for a character no locale name has.
#[unsafe(no_mangle)]
pub extern "system" fn ResolveLocaleName(name: *const u16, buf: *mut u16, size: i32) -> i32 {
    let en_us = b"en-US".map(u16::from);
    if size < 0 || (size != 0 && buf.is_null()) {
        return fail_with(ERROR_INVALID_PARAMETER, 0);
    }
    if name.is_null() {
        return put(&en_us, buf, size);
    }
    let name = wide_slice(name);
    let valid = |c: &u16| {
        u8::try_from(*c).is_ok_and(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    };
    if !name.iter().all(valid) {
        return fail_with(ERROR_INVALID_PARAMETER, 0);
    }
    if name.is_empty() {
        return put(&[], buf, size);
    }
    let language = name.split(|&c| c == u16::from(b'-') || c == u16::from(b'_')).next();
    let english = language.is_some_and(|l| {
        l.len() == 2 && (l[0] | 0x20) == u16::from(b'e') && (l[1] | 0x20) == u16::from(b'n')
    });
    if english {
        if !is_en_us(name) && name.len() != 2 {
            // en-GB and en-RR alike: gala knows no English but en-US.
            unsupported(b"kernel32.dll!ResolveLocaleName(English locale)", name.len() as u64);
        }
        return put(&en_us, buf, size);
    }
    unsupported(b"kernel32.dll!ResolveLocaleName(locale)", name.len() as u64);
    put(&[], buf, size)
}

// ---- Number and currency formats --------------------------------------------

/// NUMBERFMTW / NUMBERFMTA.
#[repr(C)]
pub struct NumberFmt<T> {
    digits: u32,
    leading_zero: u32,
    grouping: u32,
    decimal: *const T,
    thousand: *const T,
    negative: u32,
}

/// CURRENCYFMTW / CURRENCYFMTA.
#[repr(C)]
pub struct CurrencyFmt<T> {
    digits: u32,
    leading_zero: u32,
    grouping: u32,
    decimal: *const T,
    thousand: *const T,
    negative: u32,
    positive: u32,
    symbol: *const T,
}

/// A formatted number, built in a fixed buffer.
struct Text {
    b: [u16; 512],
    n: usize,
    over: bool,
}

impl Text {
    fn new() -> Self {
        Text { b: [0; 512], n: 0, over: false }
    }

    fn push(&mut self, s: &[u16]) {
        match self.b.get_mut(self.n..self.n + s.len()) {
            Some(slot) => {
                slot.copy_from_slice(s);
                self.n += s.len();
            }
            None => self.over = true,
        }
    }

    fn get(&self) -> &[u16] {
        &self.b[..self.n]
    }
}

/// Digit grouping: the group sizes from the decimal separator leftwards,
/// and whether the last repeats.
struct Grouping {
    sizes: [u8; 16],
    len: usize,
    repeat: bool,
}

impl Grouping {
    /// NUMBERFMT's Grouping: its decimal digits, most significant first;
    /// a last digit of 0 ends grouping there, another repeats (3 is
    /// 123,456,789; 32 is 12,34,56,789; 30 is 123456,789).
    fn from_number(mut v: u32) -> Self {
        let mut digits = [0u8; 16];
        let mut n = 0;
        loop {
            digits[n] = (v % 10) as u8;
            n += 1;
            v /= 10;
            if v == 0 || n == digits.len() {
                break;
            }
        }
        digits[..n].reverse();
        let repeat = digits[n - 1] != 0;
        let len = if repeat { n } else { n - 1 };
        Grouping { sizes: digits, len, repeat }
    }

    /// LOCALE_SGROUPING's form: sizes separated by ';', a trailing 0 to
    /// repeat the last ("3;0" is 123,456,789; "3" is 123456,789).
    fn from_locale(s: &[u16]) -> Self {
        let mut g = Grouping { sizes: [0; 16], len: 0, repeat: false };
        for part in s.split(|&c| c == u16::from(b';')) {
            if let (Some(&d), true) = (part.first(), g.len < g.sizes.len()) {
                g.sizes[g.len] = (d as u8).wrapping_sub(b'0').min(9);
                g.len += 1;
            }
        }
        // Leading zeros group nothing.
        let skip = g.sizes[..g.len].iter().take_while(|&&s| s == 0).count().min(g.len);
        g.sizes.copy_within(skip..g.len, 0);
        g.len -= skip;
        if g.len > 1 && g.sizes[g.len - 1] == 0 {
            g.len -= 1;
            g.repeat = true;
        }
        g
    }
}

/// What a number is formatted with.
struct Style<'a> {
    digits: u32,
    leading_zero: bool,
    grouping: Grouping,
    decimal: &'a [u16],
    thousand: &'a [u16],
}

/// The number `value` (an optional '-', digits, an optional '.' and
/// digits: nothing else) formatted without its sign: rounded half up to
/// `digits` places, grouped. Its sign as the second value; None for a
/// value that is not a number.
fn format_number(value: &[u16], style: &Style<'_>) -> Option<(Text, bool)> {
    let (negative, value) = match value.split_first() {
        Some((&c, rest)) if c == u16::from(b'-') => (true, rest),
        _ => (false, value),
    };
    let is_digit = |c: &u16| (u16::from(b'0')..=u16::from(b'9')).contains(c);
    let (int, frac) = match value.iter().position(|&c| c == u16::from(b'.')) {
        Some(dot) => (&value[..dot], &value[dot + 1..]),
        None => (value, &[][..]),
    };
    if !int.iter().all(is_digit) || !frac.iter().all(is_digit) {
        return None;
    }
    let int = &int[int.iter().take_while(|&&c| c == u16::from(b'0')).count()..];
    // The digits kept, the integer's first; then rounded.
    let mut digits = [0u8; 400];
    let places = style.digits as usize;
    let total = int.len() + places;
    if total + 1 > digits.len() {
        unsupported(b"kernel32.dll!GetNumberFormat(length)", total as u64);
        return None;
    }
    // One digit of room in front for a carry.
    for (d, &c) in digits[1..].iter_mut().zip(int.iter().chain(frac.iter().take(places))) {
        *d = (c - u16::from(b'0')) as u8;
    }
    let round_up = frac.get(places).is_some_and(|&c| c >= u16::from(b'5'));
    let mut carried = false;
    if round_up {
        let mut i = total;
        loop {
            if i == 0 {
                digits[0] = 1;
                carried = true;
                break;
            }
            if digits[i] == 9 {
                digits[i] = 0;
                i -= 1;
            } else {
                digits[i] += 1;
                break;
            }
        }
    }
    let int_digits: &[u8] = if carried { &digits[..=int.len()] } else { &digits[1..=int.len()] };
    let frac_digits = &digits[int.len() + 1..=total];
    let mut text = Text::new();
    // The integer part, grouped from the right.
    let mut reversed = [0u16; 800];
    let mut r = 0;
    let mut left = int_digits.len();
    let mut group = 0;
    while left > 0 {
        let size = if group < style.grouping.len {
            usize::from(style.grouping.sizes[group])
        } else if style.grouping.repeat && style.grouping.len > 0 {
            usize::from(style.grouping.sizes[style.grouping.len - 1])
        } else {
            left
        };
        group += 1;
        let digits = int_digits[left - size.min(left)..left].iter().rev();
        left -= size.min(left);
        let separator = if left > 0 { style.thousand } else { &[] };
        for c in digits.map(|&d| u16::from(b'0' + d)).chain(separator.iter().rev().copied()) {
            let Some(slot) = reversed.get_mut(r) else {
                unsupported(b"kernel32.dll!GetNumberFormat(length)", int_digits.len() as u64);
                return None;
            };
            *slot = c;
            r += 1;
        }
    }
    if int_digits.is_empty() && style.leading_zero {
        text.push(&[u16::from(b'0')]);
    }
    for &c in reversed[..r].iter().rev() {
        text.push(&[c]);
    }
    if places > 0 {
        text.push(style.decimal);
        for &d in frac_digits {
            text.push(&[u16::from(b'0' + d)]);
        }
    }
    (!text.over).then_some((text, negative))
}

/// The value of a numeric LCTYPE of gala's locale.
fn locale_number(kind: u32) -> u32 {
    let mut v = 0u32;
    // LOCALE_RETURN_NUMBER: a DWORD in two WCHARs.
    GetLocaleInfoW(LANG_EN_US, kind | 0x2000_0000, (&raw mut v).cast(), 2);
    v
}

/// A string LCTYPE of gala's locale into `out`.
fn locale_string(kind: u32, out: &mut [u16; 16]) -> &[u16] {
    let n = GetLocaleInfoW(LANG_EN_US, kind, out.as_mut_ptr(), 16);
    &out[..(n.max(1) - 1) as usize]
}

/// The checks every Get*Format* makes first: the locale, the value and
/// the output.
fn format_arguments_ok<T>(lcid: u32, value: *const T, buf: *mut T, size: i32) -> bool {
    known_lcid(lcid) && !value.is_null() && size >= 0 && !(size != 0 && buf.is_null())
}

/// GetNumberFormatW(lcid, flags, value, format, buf, size): `value`
/// formatted with `format`, or with the locale's LOCALE_IDIGITS,
/// ILZERO, SGROUPING, SDECIMAL, STHOUSAND and INEGNUMBER when it is NULL
/// (then only LOCALE_NOUSEROVERRIDE among the flags counts: gala's
/// locale has no user overrides); with a format, any flag is
/// ERROR_INVALID_FLAGS.
#[unsafe(no_mangle)]
pub extern "system" fn GetNumberFormatW(
    lcid: u32,
    flags: u32,
    value: *const u16,
    format: *const NumberFmt<u16>,
    buf: *mut u16,
    size: i32,
) -> i32 {
    if !format_arguments_ok(lcid, value, buf, size) {
        return fail_with(ERROR_INVALID_PARAMETER, 0);
    }
    let (mut decimal, mut thousand, mut grouping) = ([0u16; 16], [0u16; 16], [0u16; 16]);
    let (style, order) = if format.is_null() {
        let style = Style {
            digits: locale_number(0x11),            // LOCALE_IDIGITS
            leading_zero: locale_number(0x12) != 0, // LOCALE_ILZERO
            grouping: Grouping::from_locale(locale_string(0x10, &mut grouping)),
            decimal: locale_string(0x0e, &mut decimal), // LOCALE_SDECIMAL
            thousand: locale_string(0x0f, &mut thousand), // LOCALE_STHOUSAND
        };
        (style, locale_number(0x1010)) // LOCALE_INEGNUMBER
    } else {
        if flags != 0 {
            return fail_with(ERROR_INVALID_FLAGS, 0);
        }
        // SAFETY: the caller's NUMBERFMTW.
        let f = unsafe { &*format };
        if f.decimal.is_null() || f.thousand.is_null() {
            return fail_with(ERROR_INVALID_PARAMETER, 0);
        }
        let style = Style {
            digits: f.digits,
            leading_zero: f.leading_zero != 0,
            grouping: Grouping::from_number(f.grouping),
            decimal: wide_slice(f.decimal),
            thousand: wide_slice(f.thousand),
        };
        (style, f.negative)
    };
    let Some((number, negative)) = format_number(wide_slice(value), &style) else {
        return fail_with(ERROR_INVALID_PARAMETER, 0);
    };
    let mut text = Text::new();
    let minus = [u16::from(b'-')];
    let (open, close, space) = ([u16::from(b'(')], [u16::from(b')')], [u16::from(b' ')]);
    if negative {
        match order {
            0 => {
                text.push(&open);
                text.push(number.get());
                text.push(&close);
            }
            1 | 2 => {
                text.push(&minus);
                if order == 2 {
                    text.push(&space);
                }
                text.push(number.get());
            }
            3 | 4 => {
                text.push(number.get());
                if order == 4 {
                    text.push(&space);
                }
                text.push(&minus);
            }
            _ => return fail_with(ERROR_INVALID_PARAMETER, 0),
        }
    } else {
        text.push(number.get());
    }
    if text.over {
        return fail_with(ERROR_INVALID_PARAMETER, 0);
    }
    put(text.get(), buf, size)
}

/// GetNumberFormatEx(name, ...): GetNumberFormatW for "en-US" (and the
/// user default, NULL); another name ERROR_INVALID_PARAMETER.
#[unsafe(no_mangle)]
pub extern "system" fn GetNumberFormatEx(
    name: *const u16,
    flags: u32,
    value: *const u16,
    format: *const NumberFmt<u16>,
    buf: *mut u16,
    size: i32,
) -> i32 {
    match locale_of(name) {
        Some(lcid) => GetNumberFormatW(lcid, flags, value, format, buf, size),
        None => fail_with(ERROR_INVALID_PARAMETER, 0),
    }
}

/// The LCID a locale name of the Ex functions stands for: en-US for
/// "en-US", the invariant locale for "", the user default for NULL.
fn locale_of(name: *const u16) -> Option<u32> {
    if name.is_null() {
        return Some(0x400);
    }
    let name = wide_slice(name);
    if name.is_empty() {
        Some(0x7f)
    } else if is_en_us(name) {
        Some(LANG_EN_US)
    } else {
        None
    }
}

/// GetNumberFormatA: GetNumberFormatW of the strings as UTF-16, the
/// result in UTF-8.
#[unsafe(no_mangle)]
pub extern "system" fn GetNumberFormatA(
    lcid: u32,
    flags: u32,
    value: *const u8,
    format: *const NumberFmt<u8>,
    buf: *mut u8,
    size: i32,
) -> i32 {
    if !format_arguments_ok(lcid, value, buf, size) {
        return fail_with(ERROR_INVALID_PARAMETER, 0);
    }
    let (mut v, mut d, mut t) = ([0u16; 256], [0u16; 16], [0u16; 16]);
    let Some(value) = widen(value, &mut v) else {
        return fail_with(ERROR_INVALID_PARAMETER, 0);
    };
    let mut wide_format = None;
    if !format.is_null() {
        // SAFETY: the caller's NUMBERFMTA.
        let f = unsafe { &*format };
        let (decimal, thousand) = if f.decimal.is_null() || f.thousand.is_null() {
            (core::ptr::null(), core::ptr::null())
        } else {
            match (widen(f.decimal, &mut d), widen(f.thousand, &mut t)) {
                (Some(x), Some(y)) => (x.as_ptr(), y.as_ptr()),
                _ => return fail_with(ERROR_INVALID_PARAMETER, 0),
            }
        };
        wide_format = Some(NumberFmt {
            digits: f.digits,
            leading_zero: f.leading_zero,
            grouping: f.grouping,
            decimal,
            thousand,
            negative: f.negative,
        });
    }
    let wf = wide_format.as_ref().map_or(core::ptr::null(), |f| f as *const _);
    let mut out = [0u16; 512];
    let n = GetNumberFormatW(lcid, flags, value.as_ptr(), wf, out.as_mut_ptr(), 512);
    if n == 0 {
        return 0;
    }
    put_ansi(&out[..n as usize - 1], buf, size)
}

/// GetCurrencyFormatW(lcid, flags, value, format, buf, size): as
/// GetNumberFormatW, with the currency symbol and the LOCALE_ICURRDIGITS,
/// SMONGROUPING, SMONDECIMALSEP, SMONTHOUSANDSEP, SCURRENCY, ICURRENCY
/// (positive order 0-3) and INEGCURR (negative order 0-15) values.
#[unsafe(no_mangle)]
pub extern "system" fn GetCurrencyFormatW(
    lcid: u32,
    flags: u32,
    value: *const u16,
    format: *const CurrencyFmt<u16>,
    buf: *mut u16,
    size: i32,
) -> i32 {
    if !format_arguments_ok(lcid, value, buf, size) {
        return fail_with(ERROR_INVALID_PARAMETER, 0);
    }
    let (mut decimal, mut thousand, mut grouping, mut symbol_buf) =
        ([0u16; 16], [0u16; 16], [0u16; 16], [0u16; 16]);
    let (style, symbol, positive, negative_order) = if format.is_null() {
        let style = Style {
            digits: locale_number(0x19),            // LOCALE_ICURRDIGITS
            leading_zero: locale_number(0x12) != 0, // LOCALE_ILZERO
            grouping: Grouping::from_locale(locale_string(0x18, &mut grouping)),
            decimal: locale_string(0x16, &mut decimal), // LOCALE_SMONDECIMALSEP
            thousand: locale_string(0x17, &mut thousand), // LOCALE_SMONTHOUSANDSEP
        };
        let symbol = locale_string(0x14, &mut symbol_buf); // LOCALE_SCURRENCY
        (style, symbol, locale_number(0x1b), locale_number(0x1c))
    } else {
        if flags != 0 {
            return fail_with(ERROR_INVALID_FLAGS, 0);
        }
        // SAFETY: the caller's CURRENCYFMTW.
        let f = unsafe { &*format };
        if f.decimal.is_null() || f.thousand.is_null() || f.symbol.is_null() {
            return fail_with(ERROR_INVALID_PARAMETER, 0);
        }
        let style = Style {
            digits: f.digits,
            leading_zero: f.leading_zero != 0,
            grouping: Grouping::from_number(f.grouping),
            decimal: wide_slice(f.decimal),
            thousand: wide_slice(f.thousand),
        };
        (style, wide_slice(f.symbol), f.positive, f.negative)
    };
    let Some((number, negative)) = format_number(wide_slice(value), &style) else {
        return fail_with(ERROR_INVALID_PARAMETER, 0);
    };
    let n = number.get();
    let (minus, sp, open, close) = (
        &[u16::from(b'-')][..],
        &[u16::from(b' ')][..],
        &[u16::from(b'(')][..],
        &[u16::from(b')')][..],
    );
    let s = symbol;
    // Each arrangement as its pieces (Microsoft's LOCALE_INEGCURR and
    // LOCALE_ICURRENCY tables).
    let pieces: &[&[u16]] = if negative {
        match negative_order {
            0 => &[open, s, n, close],
            1 => &[minus, s, n],
            2 => &[s, minus, n],
            3 => &[s, n, minus],
            4 => &[open, n, s, close],
            5 => &[minus, n, s],
            6 => &[n, minus, s],
            7 => &[n, s, minus],
            8 => &[minus, n, sp, s],
            9 => &[minus, s, sp, n],
            10 => &[n, sp, s, minus],
            11 => &[s, sp, n, minus],
            12 => &[s, sp, minus, n],
            13 => &[n, minus, sp, s],
            14 => &[open, s, sp, n, close],
            15 => &[open, n, sp, s, close],
            _ => return fail_with(ERROR_INVALID_PARAMETER, 0),
        }
    } else {
        match positive {
            0 => &[s, n],
            1 => &[n, s],
            2 => &[s, sp, n],
            3 => &[n, sp, s],
            _ => return fail_with(ERROR_INVALID_PARAMETER, 0),
        }
    };
    let mut text = Text::new();
    for p in pieces {
        text.push(p);
    }
    if text.over {
        return fail_with(ERROR_INVALID_PARAMETER, 0);
    }
    put(text.get(), buf, size)
}

/// GetCurrencyFormatEx(name, ...): GetCurrencyFormatW for "en-US" (and
/// the user default, NULL); another name ERROR_INVALID_PARAMETER.
#[unsafe(no_mangle)]
pub extern "system" fn GetCurrencyFormatEx(
    name: *const u16,
    flags: u32,
    value: *const u16,
    format: *const CurrencyFmt<u16>,
    buf: *mut u16,
    size: i32,
) -> i32 {
    match locale_of(name) {
        Some(lcid) => GetCurrencyFormatW(lcid, flags, value, format, buf, size),
        None => fail_with(ERROR_INVALID_PARAMETER, 0),
    }
}

/// GetCurrencyFormatA: GetCurrencyFormatW of the strings as UTF-16, the
/// result in UTF-8.
#[unsafe(no_mangle)]
pub extern "system" fn GetCurrencyFormatA(
    lcid: u32,
    flags: u32,
    value: *const u8,
    format: *const CurrencyFmt<u8>,
    buf: *mut u8,
    size: i32,
) -> i32 {
    if !format_arguments_ok(lcid, value, buf, size) {
        return fail_with(ERROR_INVALID_PARAMETER, 0);
    }
    let (mut v, mut d, mut t, mut s) = ([0u16; 256], [0u16; 16], [0u16; 16], [0u16; 16]);
    let Some(value) = widen(value, &mut v) else {
        return fail_with(ERROR_INVALID_PARAMETER, 0);
    };
    let mut wide_format = None;
    if !format.is_null() {
        // SAFETY: the caller's CURRENCYFMTA.
        let f = unsafe { &*format };
        let null = core::ptr::null();
        let (decimal, thousand, symbol) =
            if f.decimal.is_null() || f.thousand.is_null() || f.symbol.is_null() {
                (null, null, null)
            } else {
                match (widen(f.decimal, &mut d), widen(f.thousand, &mut t), widen(f.symbol, &mut s))
                {
                    (Some(x), Some(y), Some(z)) => (x.as_ptr(), y.as_ptr(), z.as_ptr()),
                    _ => return fail_with(ERROR_INVALID_PARAMETER, 0),
                }
            };
        wide_format = Some(CurrencyFmt {
            digits: f.digits,
            leading_zero: f.leading_zero,
            grouping: f.grouping,
            decimal,
            thousand,
            negative: f.negative,
            positive: f.positive,
            symbol,
        });
    }
    let wf = wide_format.as_ref().map_or(core::ptr::null(), |f| f as *const _);
    let mut out = [0u16; 512];
    let n = GetCurrencyFormatW(lcid, flags, value.as_ptr(), wf, out.as_mut_ptr(), 512);
    if n == 0 {
        return 0;
    }
    put_ansi(&out[..n as usize - 1], buf, size)
}

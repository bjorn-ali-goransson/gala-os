//! Typed values (`msxml_test`'s `typed`): IXMLDOMNode's nodeTypedValue
//! and dataType. An element is typed by a `dt` attribute in
//! urn:schemas-microsoft-com:datatypes naming a type (any case) of
//! MSXML's data types; its content then reads as that type's VARIANT
//! (MSXML's "nodeTypedValue Property" table). Everything else is
//! untyped: an element gives its text, an attribute, PI, comment, text
//! or CDATA node its value (a text node inside a typed element too), the
//! document and a fragment S_FALSE and VT_NULL. A string type's value is
//! the element's text as `text` gives it (trimmed unless white space is
//! preserved). A document whose typed element names a type MSXML does
//! not know, or holds content not of its type, fails to load. XSD types do not type nodes (MSXML 6 gives VT_BSTR for
//! them), so a schema-validated document reads as strings.
//!
//! Dates are OLE dates (days since 1899-12-30) of whole seconds (a
//! fraction is dropped), a zoned one taken to UTC (`11:13:57+03:00` is
//! 08:13:57), as MSXML 6 answers on Windows (build 26100).

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write;

use super::*;
use crate::{
    LEDGER_OTHER, VT_ARRAY, VT_CY, VT_DATE, VT_I1, VT_I8, VT_R4, VT_R8, VT_UI1, VT_UI2, VT_UI8,
    VT_UINT,
};

/// The namespace of MSXML's data type attribute (`dt:dt`).
pub(super) const DT_NS: &str = "urn:schemas-microsoft-com:datatypes";

/// A data type: what its content reads as.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Ty {
    /// string and the DTD's primitive types, uri: the content as is.
    Str,
    /// number: a string that is a number.
    Number,
    Uuid,
    /// char: the character's code (VT_I4).
    Char,
    Bool,
    /// An integer of the VARIANT type, between the bounds.
    Int(u16, i64, i64),
    /// i8 and ui8: VT_NULL (MSXML gives no value for them).
    Wide,
    R4,
    R8,
    Fixed,
    Date,
    DateTime,
    DateTimeTz,
    Time,
    TimeTz,
    BinHex,
    BinBase64,
}

/// The types by canonical name.
const TYPES: [(&str, Ty); 35] = [
    ("bin.base64", Ty::BinBase64),
    ("bin.hex", Ty::BinHex),
    ("boolean", Ty::Bool),
    ("char", Ty::Char),
    ("date", Ty::Date),
    ("dateTime", Ty::DateTime),
    ("dateTime.tz", Ty::DateTimeTz),
    ("entities", Ty::Str),
    ("entity", Ty::Str),
    ("enumeration", Ty::Str),
    ("fixed.14.4", Ty::Fixed),
    ("float", Ty::R8),
    ("i1", Ty::Int(VT_I1, i8::MIN as i64, i8::MAX as i64)),
    ("i2", Ty::Int(VT_I2, i16::MIN as i64, i16::MAX as i64)),
    ("i4", Ty::Int(VT_I4, i32::MIN as i64, i32::MAX as i64)),
    ("i8", Ty::Wide),
    ("id", Ty::Str),
    ("idref", Ty::Str),
    ("idrefs", Ty::Str),
    ("int", Ty::Int(VT_I4, i32::MIN as i64, i32::MAX as i64)),
    ("nmtoken", Ty::Str),
    ("nmtokens", Ty::Str),
    ("notation", Ty::Str),
    ("number", Ty::Number),
    ("r4", Ty::R4),
    ("r8", Ty::R8),
    ("string", Ty::Str),
    ("time", Ty::Time),
    ("time.tz", Ty::TimeTz),
    ("ui1", Ty::Int(VT_UI1, 0, u8::MAX as i64)),
    ("ui2", Ty::Int(VT_UI2, 0, u16::MAX as i64)),
    ("ui4", Ty::Int(VT_UI4, 0, u32::MAX as i64)),
    ("ui8", Ty::Wide),
    ("uri", Ty::Str),
    ("uuid", Ty::Uuid),
];

fn by_name(name: &str) -> Option<(&'static str, Ty)> {
    TYPES.iter().copied().find(|(n, _)| n.eq_ignore_ascii_case(name))
}

/// Element `el`'s type attribute.
fn type_attr(t: &Tree, el: u32) -> Option<u32> {
    let n = t.node(el);
    if n.kind != Kind::Element {
        return None;
    }
    n.attrs.iter().copied().find(|&a| {
        let an = t.node(a);
        an.ns == DT_NS && an.local() == "dt"
    })
}

/// The type of element `el` (None untyped; a name MSXML does not know is
/// recorded and taken as untyped).
fn type_of(t: &Tree, el: u32) -> Option<(&'static str, Ty)> {
    let a = type_attr(t, el)?;
    let ty = by_name(&t.node(a).value);
    if ty.is_none() {
        record(b"dt:dt (a type MSXML does not name)", LEDGER_UNSUPPORTED, 0);
    }
    ty
}

/// A typed value that does not read as its type: recorded (so a failure
/// it leads to is placed), E_FAIL.
fn malformed(what: &[u8]) -> HResult {
    record(what, LEDGER_OTHER, 0);
    E_FAIL
}

// ---- Reading ----

fn trim(s: &str) -> &str {
    s.trim_matches([' ', '\t', '\n', '\r'])
}

fn integer(s: &str) -> Option<i64> {
    let s = trim(s);
    let digits = s.strip_prefix(['-', '+']).unwrap_or(s);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

/// A real number (digits, a point, an exponent; no names like "inf").
fn real(s: &str) -> Option<f64> {
    let s = trim(s);
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit() || b"+-.eE".contains(&b)) {
        return None;
    }
    s.parse().ok()
}

/// fixed.14.4: at most 14 digits before the point and 4 after, as a
/// currency (the value times 10000).
fn fixed(s: &str) -> Option<i64> {
    let s = trim(s);
    let (neg, body) = match s.strip_prefix('-') {
        Some(b) => (true, b),
        None => (false, s.strip_prefix('+').unwrap_or(s)),
    };
    let (whole, frac) = body.split_once('.').unwrap_or((body, ""));
    let digits = |p: &str| p.bytes().all(|b| b.is_ascii_digit());
    if (whole.is_empty() && frac.is_empty()) || whole.len() > 14 || frac.len() > 4 {
        return None;
    }
    if !digits(whole) || !digits(frac) {
        return None;
    }
    let mut v: i64 = if whole.is_empty() { 0 } else { whole.parse().ok()? };
    for i in 0..4 {
        v = v * 10 + frac.as_bytes().get(i).map_or(0, |b| i64::from(b - b'0'));
    }
    Some(if neg { -v } else { v })
}

fn uuid(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 36
        && b.iter().enumerate().all(|(i, &c)| {
            if matches!(i, 8 | 13 | 18 | 23) { c == b'-' } else { c.is_ascii_hexdigit() }
        })
}

fn hex(s: &str) -> Option<Vec<u8>> {
    let digits: Vec<u8> = s.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    if !digits.len().is_multiple_of(2) {
        return None;
    }
    let mut out = Vec::new();
    out.try_reserve(digits.len() / 2).ok()?;
    for pair in digits.chunks(2) {
        let hi = (pair[0] as char).to_digit(16)?;
        let lo = (pair[1] as char).to_digit(16)?;
        out.push((hi * 16 + lo) as u8);
    }
    Some(out)
}

const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Base64 (RFC 4648's alphabet; white space anywhere is skipped, the
/// padding may be left out).
fn base64(s: &str) -> Option<Vec<u8>> {
    let mut chars: Vec<u8> = s.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    while chars.last() == Some(&b'=') {
        chars.pop();
    }
    if chars.len() % 4 == 1 {
        return None;
    }
    let mut out = Vec::new();
    out.try_reserve(chars.len() * 3 / 4).ok()?;
    let (mut acc, mut bits) = (0u32, 0);
    for c in chars {
        let v = BASE64.iter().position(|&x| x == c)? as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

fn month_days(y: i64, m: i64) -> i64 {
    match m {
        2 if is_leap(y) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// The days from 0001-01-01 (day 1) to y-m-d in the proleptic Gregorian
/// calendar.
fn ordinal(y: i64, m: i64, d: i64) -> i64 {
    let p = y - 1;
    let mut n = p * 365 + p / 4 - p / 100 + p / 400 + d;
    for k in 1..m {
        n += month_days(y, k);
    }
    n
}

/// The OLE date of 1899-12-30, as an ordinal.
fn epoch() -> i64 {
    ordinal(1899, 12, 30)
}

/// `n` digits at the start of `s`, and the rest.
fn number(s: &str, n: usize) -> Option<(i64, &str)> {
    let head = s.get(..n)?;
    if !head.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((head.parse().ok()?, &s[n..]))
}

/// YYYY-MM-DD: the OLE day, and the rest.
fn parse_date(s: &str) -> Option<(i64, &str)> {
    let (y, s) = number(s, 4)?;
    let (m, s) = number(s.strip_prefix('-')?, 2)?;
    let (d, s) = number(s.strip_prefix('-')?, 2)?;
    if !(1..=12).contains(&m) || d < 1 || d > month_days(y, m) || y < 100 {
        return None;
    }
    Some((ordinal(y, m, d) - epoch(), s))
}

/// hh:mm:ss[.fraction]: the whole seconds (the fraction dropped), and
/// the rest.
fn parse_time(s: &str) -> Option<(i64, &str)> {
    let (h, s) = number(s, 2)?;
    let (m, s) = number(s.strip_prefix(':')?, 2)?;
    let (sec, mut s) = number(s.strip_prefix(':')?, 2)?;
    if h > 24 || m > 59 || sec > 59 {
        return None;
    }
    if let Some(f) = s.strip_prefix('.') {
        let n = f.bytes().take_while(u8::is_ascii_digit).count();
        if n == 0 {
            return None;
        }
        s = &f[n..];
    }
    Some((h * 3600 + m * 60 + sec, s))
}

/// A zone (`Z` or ±hh:mm) as seconds, or nothing.
fn parse_zone(s: &str) -> Option<i64> {
    if s.is_empty() || s == "Z" {
        return Some(0);
    }
    let (sign, rest) = match s.as_bytes()[0] {
        b'+' => (1, &s[1..]),
        b'-' => (-1, &s[1..]),
        _ => return None,
    };
    let (h, rest) = number(rest, 2)?;
    let (m, rest) = number(rest.strip_prefix(':')?, 2)?;
    (rest.is_empty() && h <= 14 && m <= 59).then_some(sign * (h * 3600 + m * 60))
}

/// An OLE date from days and seconds (before the epoch, the time counts
/// away from zero, as OLE dates do).
fn ole_date(days: i64, seconds: i64) -> f64 {
    let days = days + seconds.div_euclid(86400);
    let frac = seconds.rem_euclid(86400) as f64 / 86400.0;
    if days < 0 { days as f64 - frac } else { days as f64 + frac }
}

/// A date or time type's text as an OLE date.
fn date(ty: Ty, s: &str) -> Option<f64> {
    let s = trim(s);
    let (days, rest) = match ty {
        Ty::Date | Ty::DateTime | Ty::DateTimeTz => parse_date(s)?,
        _ => (0, s),
    };
    let (seconds, rest) = match ty {
        Ty::Date => (0, rest),
        Ty::DateTime | Ty::DateTimeTz => match rest.strip_prefix('T') {
            Some(t) => parse_time(t)?,
            None => (0, rest),
        },
        _ => parse_time(rest)?,
    };
    let zone = match ty {
        Ty::DateTimeTz | Ty::TimeTz => parse_zone(rest)?,
        _ if rest.is_empty() => 0,
        _ => return None,
    };
    Some(ole_date(days, seconds - zone))
}

/// Whether `s` reads as type `ty` (empty content is no value, allowed).
fn reads_as(ty: Ty, s: &str) -> bool {
    let t = trim(s);
    if t.is_empty() {
        return true;
    }
    match ty {
        Ty::Str | Ty::Wide => true,
        Ty::Number => real(t).is_some(),
        Ty::Uuid => uuid(t),
        Ty::Char => t.chars().count() == 1,
        Ty::Bool => t == "0" || t == "1",
        Ty::Int(_, lo, hi) => integer(t).is_some_and(|n| (lo..=hi).contains(&n)),
        Ty::R4 | Ty::R8 => real(t).is_some(),
        Ty::Fixed => fixed(t).is_some(),
        Ty::Date | Ty::DateTime | Ty::DateTimeTz | Ty::Time | Ty::TimeTz => date(ty, t).is_some(),
        Ty::BinHex => hex(t).is_some(),
        Ty::BinBase64 => base64(t).is_some(),
    }
}

/// A VARIANT holding 8 bytes of value.
fn variant64(vt: u16, bits: u64) -> Variant {
    let mut v = Variant { vt, reserved: [0; 3], value: [0; 2] };
    // SAFETY: the value field holds at least 8 bytes (x86 and x64).
    unsafe { core::ptr::write_unaligned(v.value.as_mut_ptr() as *mut u64, bits) };
    v
}

fn bits64(v: &Variant) -> u64 {
    // SAFETY: as variant64.
    unsafe { core::ptr::read_unaligned(v.value.as_ptr() as *const u64) }
}

/// A new VT_ARRAY|VT_UI1 of `bytes` (None when out of memory).
fn byte_array(bytes: &[u8]) -> Option<Variant> {
    // SAFETY: plain call.
    let sa = unsafe { crate::SafeArrayCreateVector(VT_UI1, 0, bytes.len() as u32) };
    if sa.is_null() {
        return None;
    }
    let mut p: *mut u8 = core::ptr::null_mut();
    // SAFETY: a new vector of bytes.len() bytes; a valid output.
    unsafe {
        crate::SafeArrayAccessData(sa, &mut p);
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
        crate::SafeArrayUnaccessData(sa);
    }
    Some(Variant { vt: VT_ARRAY | VT_UI1, reserved: [0; 3], value: [sa as usize, 0] })
}

/// The bytes of a one-dimension VT_ARRAY|VT_UI1 (None for another
/// array).
fn array_bytes(v: &Variant) -> Option<Vec<u8>> {
    let sa = v.value[0] as *mut c_void;
    if v.vt != VT_ARRAY | VT_UI1 || sa.is_null() {
        return None;
    }
    let (mut lo, mut hi) = (0, -1);
    // SAFETY: the caller's SAFEARRAY; valid outputs.
    unsafe {
        if crate::SafeArrayGetDim(sa) != 1
            || crate::SafeArrayGetLBound(sa, 1, &mut lo) < 0
            || crate::SafeArrayGetUBound(sa, 1, &mut hi) < 0
        {
            return None;
        }
    }
    let n = usize::try_from(i64::from(hi) - i64::from(lo) + 1).unwrap_or(0);
    let mut out = Vec::new();
    out.try_reserve(n).ok()?;
    let mut p: *mut u8 = core::ptr::null_mut();
    // SAFETY: the array holds n bytes (its bounds).
    unsafe {
        if crate::SafeArrayAccessData(sa, &mut p) < 0 {
            return None;
        }
        out.extend_from_slice(core::slice::from_raw_parts(p, n));
        crate::SafeArrayUnaccessData(sa);
    }
    Some(out)
}

/// Content `s` of type `ty` as its VARIANT.
fn typed_value(ty: Ty, s: &str, out: *mut Variant) -> HResult {
    let t = trim(s);
    if t.is_empty() && !matches!(ty, Ty::Str | Ty::Number | Ty::Uuid) {
        variant_null(out);
        return S_OK;
    }
    if !reads_as(ty, s) {
        return malformed(b"IXMLDOMNode::get_nodeTypedValue(content not of its dt:dt type)");
    }
    let v = match ty {
        Ty::Str | Ty::Number | Ty::Uuid => return variant_str(out, s),
        // MSXML 6 gives a 64-bit integer as its text.
        Ty::Wide => return variant_str(out, t),
        Ty::Char => {
            let c = t.chars().next().map_or(0, u32::from);
            Variant { vt: VT_I4, reserved: [0; 3], value: [c as usize, 0] }
        }
        Ty::Bool => {
            let b = if t == "1" { 0xffff } else { 0 };
            Variant { vt: VT_BOOL, reserved: [0; 3], value: [b, 0] }
        }
        Ty::Int(vt, ..) => {
            let n = integer(t).unwrap_or(0);
            let raw = match vt {
                VT_I1 | VT_UI1 => n as u8 as usize,
                VT_I2 | VT_UI2 => n as u16 as usize,
                _ => n as u32 as usize,
            };
            Variant { vt, reserved: [0; 3], value: [raw, 0] }
        }
        Ty::R4 => {
            let f = real(t).unwrap_or(0.0) as f32;
            Variant { vt: VT_R4, reserved: [0; 3], value: [f.to_bits() as usize, 0] }
        }
        Ty::R8 => variant64(VT_R8, real(t).unwrap_or(0.0).to_bits()),
        Ty::Fixed => variant64(VT_CY, fixed(t).unwrap_or(0) as u64),
        Ty::Date | Ty::DateTime | Ty::DateTimeTz | Ty::Time | Ty::TimeTz => {
            variant64(VT_DATE, date(ty, t).unwrap_or(0.0).to_bits())
        }
        Ty::BinHex | Ty::BinBase64 => {
            let bytes = if ty == Ty::BinHex { hex(t) } else { base64(t) };
            match bytes.as_deref().and_then(byte_array) {
                Some(v) => v,
                None => return E_OUTOFMEMORY,
            }
        }
    };
    put(out, v);
    S_OK
}

/// IXMLDOMNode::get_nodeTypedValue: a typed element's content as its
/// type's VARIANT, an untyped element's text (as `text`), other nodes'
/// value as a string; the document and a fragment S_FALSE and VT_NULL.
pub(super) extern "system" fn get_node_typed_value(this: Obj, out: *mut Variant) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    let d = doc(this);
    let i = idx(this);
    let n = d.tree.node(i);
    match n.kind {
        Kind::Document | Kind::Fragment => {
            variant_null(out);
            S_FALSE
        }
        Kind::Element => match type_of(&d.tree, i) {
            Some((_, ty)) => {
                let mut s = String::new();
                d.tree.text(i, &mut s);
                let s = if d.preserve { s.as_str() } else { trim(&s) };
                typed_value(ty, s, out)
            }
            None => {
                let mut s = String::new();
                d.tree.text(i, &mut s);
                let s = if d.preserve { s.as_str() } else { trim(&s) };
                variant_str(out, s)
            }
        },
        _ => variant_str(out, &n.value),
    }
}

/// IXMLDOMNode::get_dataType: a typed element's type name (MSXML's
/// spelling); S_FALSE and VT_NULL for every other node.
pub(super) extern "system" fn get_data_type(this: Obj, out: *mut Variant) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    let d = doc(this);
    match type_of(&d.tree, idx(this)) {
        Some((name, _)) => variant_str(out, name),
        None => {
            variant_null(out);
            S_FALSE
        }
    }
}

// ---- Writing ----

/// A number a VARIANT holds directly (integers, reals, booleans,
/// currency, dates).
enum Num {
    Int(i64),
    Real(f64),
    Cy(i64),
}

fn variant_num(v: &Variant) -> Option<Num> {
    let raw = v.value[0];
    Some(match v.vt {
        VT_I1 => Num::Int(i64::from(raw as u8 as i8)),
        VT_UI1 => Num::Int(i64::from(raw as u8)),
        VT_I2 | VT_BOOL => Num::Int(i64::from(raw as u16 as i16)),
        VT_UI2 => Num::Int(i64::from(raw as u16)),
        VT_I4 | VT_INT => Num::Int(i64::from(raw as u32 as i32)),
        VT_UI4 | VT_UINT => Num::Int(i64::from(raw as u32)),
        VT_I8 => Num::Int(bits64(v) as i64),
        VT_UI8 => Num::Int(i64::try_from(bits64(v)).ok()?),
        VT_R4 => Num::Real(f64::from(f32::from_bits(raw as u32))),
        VT_R8 | VT_DATE => Num::Real(f64::from_bits(bits64(v))),
        VT_CY => Num::Cy(bits64(v) as i64),
        _ => return None,
    })
}

/// Any VARIANT as text: strings as they are, other types through
/// oleaut32's VariantChangeType (None when it cannot).
fn as_text(v: &Variant) -> Option<String> {
    if let Some(s) = variant_text(v) {
        return Some(s);
    }
    let mut b = Variant { vt: VT_EMPTY, reserved: [0; 3], value: [0; 2] };
    // SAFETY: the caller's VARIANT and ours.
    if unsafe { crate::VariantChangeType(&mut b, v, 0, VT_BSTR) } < 0 {
        return None;
    }
    let s = text(b.value[0] as *const u16);
    // SAFETY: the BSTR VariantChangeType made.
    unsafe { crate::VariantClear(&mut b) };
    s
}

/// A currency (value times 10000) as fixed.14.4's text.
fn fixed_text(cy: i64) -> String {
    let mut s = String::new();
    let (sign, a) = if cy < 0 { ("-", cy.unsigned_abs()) } else { ("", cy as u64) };
    let _ = write!(s, "{sign}{}", a / 10000);
    let frac = a % 10000;
    if frac != 0 {
        let mut f = alloc::format!("{frac:04}");
        while f.ends_with('0') {
            f.pop();
        }
        let _ = write!(s, ".{f}");
    }
    s
}

/// An OLE date as `ty`'s text (to the nearest second).
fn date_text(ty: Ty, value: f64) -> Option<String> {
    if !value.is_finite() || !(-657_434.0..=2_958_466.0).contains(&value) {
        return None;
    }
    // Before the epoch the time counts away from zero: the day is the
    // value's whole part, the time its fraction's size.
    let mut days = value as i64;
    let mut seconds = ((value - days as f64).abs() * 86400.0 + 0.5) as i64;
    if seconds >= 86400 {
        days += if value < 0.0 { -1 } else { 1 };
        seconds -= 86400;
    }
    // The civil date of the ordinal.
    let mut n = days + epoch();
    let mut y = n * 400 / 146_097 + 1;
    while ordinal(y, 1, 1) > n {
        y -= 1;
    }
    while ordinal(y + 1, 1, 1) <= n {
        y += 1;
    }
    n -= ordinal(y, 1, 1) - 1;
    let mut m = 1;
    while n > month_days(y, m) {
        n -= month_days(y, m);
        m += 1;
    }
    let mut s = String::new();
    let (h, mi, se) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    let _ = match ty {
        Ty::Date => write!(s, "{y:04}-{m:02}-{n:02}"),
        Ty::Time | Ty::TimeTz => write!(s, "{h:02}:{mi:02}:{se:02}"),
        _ => write!(s, "{y:04}-{m:02}-{n:02}T{h:02}:{mi:02}:{se:02}"),
    };
    Some(s)
}

fn hex_text(bytes: &[u8]) -> String {
    let mut s = String::new();
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

fn base64_text(bytes: &[u8]) -> String {
    let mut s = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().fold(0u32, |a, &b| (a << 8) | u32::from(b)) << (8 * (3 - chunk.len()));
        for k in 0..4 {
            if k <= chunk.len() {
                s.push(BASE64[(n >> (18 - 6 * k) & 63) as usize] as char);
            } else {
                s.push('=');
            }
        }
    }
    s
}

/// Value `v` as the text of type `ty` (None when it is not one).
fn typed_text(ty: Ty, v: &Variant) -> Option<String> {
    if matches!(ty, Ty::BinHex | Ty::BinBase64) && v.vt == VT_ARRAY | VT_UI1 {
        let bytes = array_bytes(v)?;
        return Some(if ty == Ty::BinHex { hex_text(&bytes) } else { base64_text(&bytes) });
    }
    let num = variant_num(v);
    let text = match (ty, num) {
        (_, None) | (Ty::Str | Ty::Uuid | Ty::Char | Ty::Wide | Ty::BinHex | Ty::BinBase64, _) => {
            as_text(v)?
        }
        (Ty::Bool, Some(n)) => {
            let set = match n {
                Num::Int(i) | Num::Cy(i) => i != 0,
                Num::Real(f) => f != 0.0,
            };
            String::from(if set { "1" } else { "0" })
        }
        (Ty::Int(..), Some(Num::Int(i))) => alloc::format!("{i}"),
        (Ty::Int(..), Some(_)) => as_text(v)?,
        (Ty::Fixed, Some(Num::Cy(c))) => fixed_text(c),
        (Ty::Fixed, Some(Num::Int(i))) => fixed_text(i.checked_mul(10000)?),
        (Ty::Fixed, Some(Num::Real(f))) => {
            let c = f * 10000.0;
            fixed_text((if c < 0.0 { c - 0.5 } else { c + 0.5 }) as i64)
        }
        (Ty::R4, Some(n)) => {
            let f = match n {
                Num::Int(i) => i as f32,
                Num::Real(f) => f as f32,
                Num::Cy(c) => c as f32 / 10000.0,
            };
            alloc::format!("{f}")
        }
        (Ty::R8 | Ty::Number, Some(n)) => match n {
            Num::Int(i) => alloc::format!("{i}"),
            Num::Real(f) => alloc::format!("{f}"),
            Num::Cy(c) => fixed_text(c),
        },
        (_, Some(n)) => {
            let f = match n {
                Num::Int(i) => i as f64,
                Num::Real(f) => f,
                Num::Cy(c) => c as f64 / 10000.0,
            };
            date_text(ty, f)?
        }
    };
    reads_as(ty, &text).then_some(text)
}

/// IXMLDOMNode::put_nodeTypedValue: a typed element takes the value as
/// its type's text (a string kept as written once it reads as the type,
/// bytes encoded for bin.hex and bin.base64, other values written in the
/// type's form; E_FAIL when the value is not one of the type); an
/// untyped element and the other nodes take its text. The document has
/// no value (E_INVALIDARG, as on Windows), nor has a fragment (E_FAIL,
/// not measured).
pub(super) extern "system" fn put_node_typed_value(this: Obj, v: Variant) -> HResult {
    let d = doc(this);
    let i = idx(this);
    match d.tree.node(i).kind {
        Kind::Document => E_INVALIDARG,
        Kind::Fragment => E_FAIL,
        Kind::Element => {
            let value = match type_of(&d.tree, i) {
                Some((_, ty)) => match typed_text(ty, &v) {
                    Some(s) => s,
                    None => {
                        return malformed(
                            b"IXMLDOMNode::put_nodeTypedValue(a value not of the dt:dt type)",
                        );
                    }
                },
                None => match as_text(&v) {
                    Some(s) => s,
                    None => return unconvertible(v.vt),
                },
            };
            replace_children(d, i, &value);
            S_OK
        }
        _ => match as_text(&v) {
            Some(s) => set_value(d, i, s),
            None => unconvertible(v.vt),
        },
    }
}

fn unconvertible(vt: u16) -> HResult {
    record(b"IXMLDOMNode::put_nodeTypedValue(variant type)", LEDGER_UNSUPPORTED, vt.into());
    E_FAIL
}

/// IXMLDOMNode::put_dataType: an element's `dt:dt` attribute set to the
/// type's name (declaring `dt` when it is not). E_INVALIDARG for NULL
/// and on text, CDATA, comment, PI and fragment nodes; S_OK on an
/// attribute and for a name MSXML does not know, and on content not of
/// the type (MSXML 6's answers on Windows; what each then leaves is not
/// measured: gala leaves the node as it was in the first two, sets the
/// type in the third, and records each).
pub(super) extern "system" fn put_data_type(this: Obj, name: Bstr) -> HResult {
    let Some(name) = text(name) else { return E_INVALIDARG };
    let d = doc(this);
    let el = idx(this);
    match d.tree.node(el).kind {
        Kind::Element => {}
        Kind::Attribute => {
            record(b"IXMLDOMNode::put_dataType(on an attribute: ignored)", LEDGER_OTHER, 0);
            return S_OK;
        }
        Kind::Document => return E_FAIL,
        _ => return E_INVALIDARG,
    }
    let Some((canonical, ty)) = by_name(&name) else {
        record(b"IXMLDOMNode::put_dataType(a type MSXML does not name: ignored)", LEDGER_OTHER, 0);
        return S_OK;
    };
    let mut content = String::new();
    d.tree.text(el, &mut content);
    if !reads_as(ty, &content) {
        record(b"IXMLDOMNode::put_dataType(content not of the type)", LEDGER_OTHER, 0);
    }
    if let Some(a) = type_attr(&d.tree, el) {
        d.tree.nodes[a as usize].value = canonical.into();
        return S_OK;
    }
    match d.tree.lookup(el, "dt") {
        DT_NS => {}
        "" => {
            let decl = d.tree.create(Kind::Attribute, "xmlns:dt", gala_xml::XMLNS_NS, DT_NS);
            d.tree.insert(el, decl, None);
        }
        _ => {
            record(
                b"IXMLDOMNode::put_dataType(prefix dt bound to another namespace)",
                LEDGER_UNSUPPORTED,
                0,
            );
            return E_FAIL;
        }
    }
    let a = d.tree.create(Kind::Attribute, "dt:dt", DT_NS, canonical);
    d.tree.insert(el, a, None);
    S_OK
}

/// A loaded document's typed elements: the first whose type MSXML does
/// not name, or whose content is not of its type, as the reason loading
/// fails (MSXML 6 fails such a load on Windows; its parse error's code
/// and text are not measured).
pub(super) fn on_load(d: &Doc) -> Option<&'static str> {
    let t = &d.tree;
    for i in 0..t.nodes.len() as u32 {
        let Some(a) = type_attr(t, i) else { continue };
        let Some((_, ty)) = by_name(&t.node(a).value) else {
            record(b"loadXML: dt:dt names no MSXML type", LEDGER_OTHER, i.into());
            return Some("dt:dt names no data type");
        };
        let mut s = String::new();
        t.text(i, &mut s);
        if !reads_as(ty, &s) {
            record(b"loadXML: content not of its dt:dt type", LEDGER_OTHER, i.into());
            return Some("content not of its dt:dt type");
        }
    }
    None
}

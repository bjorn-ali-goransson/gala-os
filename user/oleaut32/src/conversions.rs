//! The VarXxxFromYyy conversions programs import directly and VarCmp,
//! as documented (the running objects are active.rs). They share
//! VariantChangeType's
//! conversions (variant.rs): numbers in the en-US format, integers
//! rounded half to even, DISP_E_OVERFLOW out of range and
//! DISP_E_TYPEMISMATCH for text that is no number. Locales other than
//! English ones (and the neutral/default LCIDs) are read as en-US and
//! reported to the ledger.

use super::{DISP_E_OVERFLOW, E_INVALIDARG, HResult, S_OK, missing, put, wide_len};
use crate::variant::{
    DISP_E_BADVARTYPE, DISP_E_TYPEMISMATCH, VARIANT_ALPHABOOL, VT_BOOL, VT_BSTR, VT_CY, VT_I4,
    VT_R8, VT_UI4, Value, Variant, bstr, chars, convert, decimal,
};

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn CompareStringW(lcid: u32, flags: u32, a: *const u16, n: i32, b: *const u16, m: i32) -> i32;
}

/// VARCMP_LT .. VARCMP_NULL (VarCmp's results).
const VARCMP_LT: HResult = 0;
const VARCMP_EQ: HResult = 1;
const VARCMP_GT: HResult = 2;
const VARCMP_NULL: HResult = 3;

/// Reports `what` (the function and "(locale)") when `lcid` is not one
/// gala formats as it should: the en-US format is used for every locale.
pub(crate) fn check_locale(what: &[u8], lcid: u32) {
    let language = lcid & 0x3ff;
    // LANG_NEUTRAL (the user/system defaults, 0x400/0x800), LANG_INVARIANT
    // (0x7f) and LANG_ENGLISH (any country) share "." and en-US names.
    if !matches!(language, 0 | 0x7f | 0x09) {
        missing(what, lcid.into());
    }
}

/// `s` (the caller's NUL-terminated string) converted to `vt`.
fn from_text(s: *const u16, vt: u16, flags: u32, out: &mut u64) -> HResult {
    if s.is_null() {
        return DISP_E_TYPEMISMATCH;
    }
    match convert(Value::Text(s, wide_len(s)), vt, flags as u16) {
        Ok(v) => {
            *out = v[0];
            S_OK
        }
        Err(hr) => hr,
    }
}

macro_rules! from_str {
    ($name:ident, $vt:expr, $t:ty) => {
        #[doc = concat!(stringify!($name), ": the text as a ", stringify!($t), ".")]
        #[unsafe(no_mangle)]
        pub extern "system" fn $name(
            s: *const u16,
            lcid: u32,
            flags: u32,
            out: *mut $t,
        ) -> HResult {
            if out.is_null() {
                return E_INVALIDARG;
            }
            check_locale(concat!("oleaut32.dll!", stringify!($name), "(locale)").as_bytes(), lcid);
            let mut v = 0u64;
            let hr = from_text(s, $vt, flags, &mut v);
            if hr == S_OK {
                put(out, v as $t);
            }
            hr
        }
    };
}

from_str!(VarI4FromStr, VT_I4, i32);
from_str!(VarUI4FromStr, VT_UI4, u32);
from_str!(VarCyFromStr, VT_CY, i64);
from_str!(VarBoolFromStr, VT_BOOL, i16);

/// VarR8FromStr: the text as a double.
#[unsafe(no_mangle)]
pub extern "system" fn VarR8FromStr(
    s: *const u16,
    lcid: u32,
    flags: u32,
    out: *mut f64,
) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    check_locale(b"oleaut32.dll!VarR8FromStr(locale)", lcid);
    let mut v = 0u64;
    let hr = from_text(s, VT_R8, flags, &mut v);
    if hr == S_OK {
        put(out, f64::from_bits(v));
    }
    hr
}

/// VarCyFromR8: the value times 10000, half to even; DISP_E_OVERFLOW out
/// of the currency's range.
#[unsafe(no_mangle)]
pub extern "system" fn VarCyFromR8(v: f64, out: *mut i64) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    match convert(Value::Real(v, 15), VT_CY, 0) {
        Ok(c) => {
            put(out, c[0] as i64);
            S_OK
        }
        Err(hr) => hr,
    }
}

/// VarBstrFromCy: the currency's value with up to four decimals and no
/// trailing zeros ("1.5", "-0.0009", "922337203685477.5807").
#[unsafe(no_mangle)]
pub extern "system" fn VarBstrFromCy(
    cy: i64,
    lcid: u32,
    _flags: u32,
    out: *mut *mut u16,
) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    check_locale(b"oleaut32.dll!VarBstrFromCy(locale)", lcid);
    let magnitude = cy.unsigned_abs();
    let (whole, mut fraction) = (magnitude / 10000, magnitude % 10000);
    let mut text = [0u8; 32];
    let mut start = decimal(whole.into(), &mut text[..24], 24);
    if cy < 0 {
        start -= 1;
        text[start] = b'-';
    }
    let mut end = 24;
    if fraction != 0 {
        let mut places = 4;
        while fraction % 10 == 0 {
            fraction /= 10;
            places -= 1;
        }
        text[end] = b'.';
        end += 1;
        for i in (0..places).rev() {
            text[end] = b'0' + (fraction / 10u64.pow(i) % 10) as u8;
            end += 1;
        }
    }
    match bstr(&text[start..end]) {
        Ok(b) => {
            put(out, b as *mut u16);
            S_OK
        }
        Err(hr) => hr,
    }
}

/// VarBstrFromBool: "True" or "False" (VARIANT_TRUE is -1, any non-zero
/// value is true).
#[unsafe(no_mangle)]
pub extern "system" fn VarBstrFromBool(
    b: i16,
    lcid: u32,
    _flags: u32,
    out: *mut *mut u16,
) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    check_locale(b"oleaut32.dll!VarBstrFromBool(locale)", lcid);
    match convert(Value::Bool(b != 0), VT_BSTR, VARIANT_ALPHABOOL) {
        Ok(v) => {
            put(out, v[0] as *mut u16);
            S_OK
        }
        Err(hr) => hr,
    }
}

/// DATE's range: 1 January 100 to 31 December 9999 (as days from 30
/// December 1899); a value whose day is outside it is DISP_E_OVERFLOW.
const DATE_MIN: f64 = -657_434.0;
const DATE_MAX: f64 = 2_958_465.0;

/// A DATE from a number of days: DISP_E_OVERFLOW past the range.
fn date(v: f64, out: *mut f64) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    if v.is_nan() || v <= DATE_MIN - 1.0 || v >= DATE_MAX + 1.0 {
        return DISP_E_OVERFLOW;
    }
    put(out, v);
    S_OK
}

/// VarDateFromR8: the same number, within DATE's range.
#[unsafe(no_mangle)]
pub extern "system" fn VarDateFromR8(v: f64, out: *mut f64) -> HResult {
    date(v, out)
}

/// VarDateFromCy: the currency's value (units of 1/10000) as days.
#[unsafe(no_mangle)]
pub extern "system" fn VarDateFromCy(cy: i64, out: *mut f64) -> HResult {
    date(cy as f64 / 10000.0, out)
}

/// VarDateFromBool: VARIANT_TRUE is -1.0, VARIANT_FALSE 0.0.
#[unsafe(no_mangle)]
pub extern "system" fn VarDateFromBool(b: i16, out: *mut f64) -> HResult {
    date(f64::from(b), out)
}

/// VarR8FromDate: the DATE's number.
#[unsafe(no_mangle)]
pub extern "system" fn VarR8FromDate(d: f64, out: *mut f64) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    put(out, d);
    S_OK
}

/// VarR8FromCy: the currency's value (units of 1/10000).
#[unsafe(no_mangle)]
pub extern "system" fn VarR8FromCy(cy: i64, out: *mut f64) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    put(out, cy as f64 / 10000.0);
    S_OK
}

/// VarCyFromDate: as VarCyFromR8 of the DATE's number.
#[unsafe(no_mangle)]
pub extern "system" fn VarCyFromDate(d: f64, out: *mut i64) -> HResult {
    VarCyFromR8(d, out)
}

/// VarBoolFromDate: VARIANT_TRUE for any day but 0.
#[unsafe(no_mangle)]
pub extern "system" fn VarBoolFromDate(d: f64, out: *mut i16) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    put(out, if d != 0.0 { -1 } else { 0 });
    S_OK
}

/// VarBoolFromCy: VARIANT_TRUE for any value but 0.
#[unsafe(no_mangle)]
pub extern "system" fn VarBoolFromCy(cy: i64, out: *mut i16) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    put(out, if cy != 0 { -1 } else { 0 });
    S_OK
}

/// VarBstrFromR8: the number with up to 15 significant digits, fixed
/// while the integer digits and the decimals it needs both fit in 15 and
/// scientific ("5.6789E-12") otherwise (variant.rs `format_real`).
#[unsafe(no_mangle)]
pub extern "system" fn VarBstrFromR8(
    v: f64,
    lcid: u32,
    _flags: u32,
    out: *mut *mut u16,
) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    check_locale(b"oleaut32.dll!VarBstrFromR8(locale)", lcid);
    match convert(Value::Real(v, 15), VT_BSTR, 0) {
        Ok(b) => {
            put(out, b[0] as *mut u16);
            S_OK
        }
        Err(hr) => hr,
    }
}

/// VarBstrFromR4: as VarBstrFromR8, with a float's seven significant
/// digits -- so "0.3333333", and "1.234567E-02" where a double would
/// still be fixed (measured on Windows 11).
#[unsafe(no_mangle)]
pub extern "system" fn VarBstrFromR4(
    v: f32,
    lcid: u32,
    _flags: u32,
    out: *mut *mut u16,
) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    check_locale(b"oleaut32.dll!VarBstrFromR4(locale)", lcid);
    match convert(Value::Real(v.into(), 7), VT_BSTR, 0) {
        Ok(b) => {
            put(out, b[0] as *mut u16);
            S_OK
        }
        Err(hr) => hr,
    }
}

/// VarBstrFromI4: the number in plain decimal. Windows gives no
/// grouping and reads no separator from the locale here, so the locale
/// is not reported either (measured on Windows 11, where 0x40c answers
/// "1234567" as 0x409 does).
#[unsafe(no_mangle)]
pub extern "system" fn VarBstrFromI4(
    v: i32,
    _lcid: u32,
    _flags: u32,
    out: *mut *mut u16,
) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    match convert(Value::Int(v.into()), VT_BSTR, 0) {
        Ok(b) => {
            put(out, b[0] as *mut u16);
            S_OK
        }
        Err(hr) => hr,
    }
}

/// VarR8FromR4: a float widened, which every float has an answer for --
/// infinities and NaNs with the rest, and -0 still -0 (measured on
/// Windows 11).
#[unsafe(no_mangle)]
pub extern "system" fn VarR8FromR4(v: f32, out: *mut f64) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    put(out, f64::from(v));
    S_OK
}

/// VarR4FromR8: a double narrowed to the nearest float, ties to even
/// (16777219 becomes 16777220). DISP_E_OVERFLOW when the result would
/// not be finite -- an infinity or a NaN included -- and then the output
/// is left as the caller had it; a value too small for a float
/// underflows to zero with its sign and S_OK (measured on Windows 11).
#[unsafe(no_mangle)]
pub extern "system" fn VarR4FromR8(v: f64, out: *mut f32) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    let narrowed = v as f32;
    if !narrowed.is_finite() {
        return DISP_E_OVERFLOW;
    }
    put(out, narrowed);
    S_OK
}

const VT_EMPTY: u16 = 0;
const VT_NULL: u16 = 1;
const VT_I2: u16 = 2;
const VT_R4: u16 = 4;
const VT_DATE: u16 = 7;
const VT_ERROR: u16 = 10;
const VT_DECIMAL: u16 = 14;
const VT_UI1: u16 = 17;
const VT_I8: u16 = 20;
const VT_INT: u16 = 22;
/// The flag VarCmp's callers may set on vt to compare strings with
/// numbers differently (Windows' behaviour with it is not known here).
const VT_RESERVED: u16 = 0x8000;

/// A VarCmp operand: a number, a string, or nothing.
enum Operand {
    Int(i128),
    Real(f64),
    Text(*const u16, usize),
    Empty,
}

/// What VarCmp compares in `v`, or the failure for its type. `right`:
/// Windows takes VT_INT on the left only.
fn operand(vt: u16, raw: u64, right: bool) -> Result<Option<Operand>, HResult> {
    Ok(Some(match vt {
        VT_NULL => return Ok(None),
        VT_EMPTY => Operand::Empty,
        VT_I2 => Operand::Int((raw as i16).into()),
        VT_I4 => Operand::Int((raw as i32).into()),
        VT_INT if !right => Operand::Int((raw as i32).into()),
        VT_UI1 => Operand::Int((raw as u8).into()),
        VT_I8 => Operand::Int((raw as i64).into()),
        VT_BOOL => Operand::Int((raw as i16).into()),
        VT_R4 => Operand::Real(f32::from_bits(raw as u32).into()),
        VT_R8 | VT_DATE => Operand::Real(f64::from_bits(raw)),
        VT_CY => Operand::Real(raw as i64 as f64 / 10000.0),
        VT_BSTR => {
            let s = raw as *const u16;
            Operand::Text(s, super::SysStringLen(s) as usize)
        }
        VT_DECIMAL => {
            missing(b"oleaut32.dll!VarCmp(VT_DECIMAL)", 0);
            return Err(DISP_E_BADVARTYPE);
        }
        VT_ERROR => return Err(DISP_E_TYPEMISMATCH),
        _ => return Err(DISP_E_BADVARTYPE),
    }))
}

fn order<T: PartialOrd>(a: T, b: T) -> HResult {
    if a < b {
        VARCMP_LT
    } else if a > b {
        VARCMP_GT
    } else {
        VARCMP_EQ
    }
}

/// VarCmp: VARCMP_LT/EQ/GT, or VARCMP_NULL when either is VT_NULL.
/// Numbers compare by value (VT_EMPTY is 0), strings as CompareStringW
/// compares them with `flags` (NORM_IGNORECASE ...), a string is greater
/// than any number, and an empty string equals VT_EMPTY. VT_ERROR equals
/// only VT_ERROR; interfaces, records, arrays and by-reference variants
/// fail (as on Windows).
#[unsafe(no_mangle)]
pub extern "system" fn VarCmp(
    left: *const Variant,
    right: *const Variant,
    lcid: u32,
    flags: u32,
) -> HResult {
    if left.is_null() || right.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's VARIANTs.
    let (l, r) = unsafe { (&*left, &*right) };
    if (l.vt | r.vt) & VT_RESERVED != 0 {
        missing(b"oleaut32.dll!VarCmp(VT_RESERVED)", 0);
    }
    let (lvt, rvt) = (l.vt & !VT_RESERVED, r.vt & !VT_RESERVED);
    if lvt == VT_ERROR && rvt == VT_ERROR {
        return order(l.value[0] as u32, r.value[0] as u32);
    }
    let (a, b) = match (operand(lvt, l.value[0], false), operand(rvt, r.value[0], true)) {
        (Err(hr), _) | (_, Err(hr)) => return hr,
        (Ok(None), _) | (_, Ok(None)) => return VARCMP_NULL,
        (Ok(Some(a)), Ok(Some(b))) => (a, b),
    };
    use Operand::*;
    match (a, b) {
        (Text(s, n), Text(t, m)) => {
            if n == 0 || m == 0 {
                return order(n, m);
            }
            let flags = flags & !0x8000_0000; // LOCALE_NOUSEROVERRIDE
            // SAFETY: the BSTRs' characters.
            let r = unsafe { CompareStringW(lcid, flags, s, n as i32, t, m as i32) };
            if r == 0 {
                // A locale or flags kernel32 does not know: as ordinal.
                missing(b"oleaut32.dll!VarCmp(CompareStringW)", lcid.into());
                return order(chars(s, n), chars(t, m));
            }
            r - 1
        }
        (Text(_, n), Empty) => order(n, 0),
        (Empty, Text(_, n)) => order(0, n),
        (Text(..), _) => VARCMP_GT,
        (_, Text(..)) => VARCMP_LT,
        (Int(a), Int(b)) => order(a, b),
        (a, b) => {
            let real = |o: Operand| match o {
                Int(i) => i as f64,
                Real(r) => r,
                _ => 0.0,
            };
            order(real(a), real(b))
        }
    }
}

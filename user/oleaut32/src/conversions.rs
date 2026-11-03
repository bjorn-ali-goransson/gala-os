//! The VarXxxFromYyy conversions programs import directly, VarCmp and
//! GetActiveObject, as documented. They share VariantChangeType's
//! conversions (variant.rs): numbers in the en-US format, integers
//! rounded half to even, DISP_E_OVERFLOW out of range and
//! DISP_E_TYPEMISMATCH for text that is no number. Locales other than
//! English ones (and the neutral/default LCIDs) are read as en-US and
//! reported to the ledger.

use super::{E_INVALIDARG, HResult, S_OK, missing, put, wide_len};
use crate::variant::{
    DISP_E_BADVARTYPE, DISP_E_TYPEMISMATCH, VARIANT_ALPHABOOL, VT_BOOL, VT_BSTR, VT_CY, VT_I4,
    VT_R8, VT_UI4, Value, Variant, bstr, chars, convert, decimal,
};
use core::ffi::c_void;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn CompareStringW(lcid: u32, flags: u32, a: *const u16, n: i32, b: *const u16, m: i32) -> i32;
}

const MK_E_UNAVAILABLE: HResult = 0x8004_01e3u32 as i32;

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
    match convert(Value::Real(v), VT_CY, 0) {
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

/// GetActiveObject: gala has no running object table (nothing calls
/// RegisterActiveObject yet), so no class is running: MK_E_UNAVAILABLE,
/// as Windows answers for a class not registered as running. Reported,
/// so a program that expects a running server shows up.
#[unsafe(no_mangle)]
pub extern "system" fn GetActiveObject(
    clsid: *const c_void,
    _reserved: *mut c_void,
    out: *mut *mut c_void,
) -> HResult {
    if out.is_null() || clsid.is_null() {
        return E_INVALIDARG;
    }
    put(out, core::ptr::null_mut());
    missing(b"oleaut32.dll!GetActiveObject(no running object table)", 0);
    MK_E_UNAVAILABLE
}

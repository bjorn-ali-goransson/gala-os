//! VARIANTs (vt, three reserved words, a 16-byte value), as documented:
//! VariantClear frees what the variant owns (BSTRs, interfaces, arrays),
//! VariantCopy duplicates it, VariantChangeType(Ex) converts between the
//! integer types, floating point, VT_BOOL, VT_CY and VT_BSTR (numbers in
//! the invariant/en-US format; VARIANT_ALPHABOOL gives "True"/"False").
//! Dates and decimals are not converted yet (recorded).

use super::{
    DISP_E_OVERFLOW, E_INVALIDARG, E_OUTOFMEMORY, HResult, S_OK, SysAllocStringLen, SysFreeString,
    SysStringLen, add_ref, missing, release,
};
use crate::safearray::{SafeArray, SafeArrayCopy, SafeArrayDestroy};

const DISP_E_BADVARTYPE: HResult = 0x8002_0008u32 as i32;
const DISP_E_TYPEMISMATCH: HResult = 0x8002_0005u32 as i32;

pub const VT_EMPTY: u16 = 0;
pub const VT_NULL: u16 = 1;
pub const VT_I2: u16 = 2;
pub const VT_I4: u16 = 3;
pub const VT_R4: u16 = 4;
pub const VT_R8: u16 = 5;
pub const VT_CY: u16 = 6;
pub const VT_BSTR: u16 = 8;
pub const VT_DISPATCH: u16 = 9;
pub const VT_ERROR: u16 = 10;
pub const VT_BOOL: u16 = 11;
pub const VT_VARIANT: u16 = 12;
pub const VT_UNKNOWN: u16 = 13;
pub const VT_I1: u16 = 16;
pub const VT_UI1: u16 = 17;
pub const VT_UI2: u16 = 18;
pub const VT_UI4: u16 = 19;
pub const VT_I8: u16 = 20;
pub const VT_UI8: u16 = 21;
pub const VT_INT: u16 = 22;
pub const VT_UINT: u16 = 23;
pub const VT_RECORD: u16 = 36;
pub const VT_ARRAY: u16 = 0x2000;
pub const VT_BYREF: u16 = 0x4000;

const VARIANT_ALPHABOOL: u16 = 0x2;
const VARIANT_LOCALBOOL: u16 = 0x10;

#[repr(C)]
pub struct Variant {
    pub vt: u16,
    reserved: [u16; 3],
    pub value: [u64; 2],
}

#[unsafe(no_mangle)]
pub extern "system" fn VariantInit(v: *mut Variant) {
    if !v.is_null() {
        // SAFETY: the caller's VARIANT.
        unsafe { (*v).vt = VT_EMPTY };
    }
}

/// Frees what a variant of type `vt` holding `value` owns.
fn clear_value(vt: u16, value: u64) {
    if vt & VT_BYREF != 0 {
        return;
    }
    if vt & VT_ARRAY != 0 {
        SafeArrayDestroy(value as *mut SafeArray);
        return;
    }
    match vt {
        VT_BSTR => SysFreeString(value as *mut u16),
        VT_UNKNOWN | VT_DISPATCH => release(value as usize),
        VT_RECORD => missing(b"oleaut32.dll!VariantClear(VT_RECORD)", 0),
        _ => {}
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn VariantClear(v: *mut Variant) -> HResult {
    if v.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's VARIANT.
    let v = unsafe { &mut *v };
    clear_value(v.vt, v.value[0]);
    v.vt = VT_EMPTY;
    S_OK
}

/// A copy of the value of a variant of type `vt` (BSTRs duplicated,
/// interfaces referenced, arrays copied).
fn copy_value(vt: u16, value: [u64; 2]) -> Result<[u64; 2], HResult> {
    if vt & VT_BYREF != 0 {
        return Ok(value);
    }
    if vt & VT_ARRAY != 0 {
        let mut copy = core::ptr::null_mut();
        let hr = SafeArrayCopy(value[0] as *const SafeArray, &mut copy);
        return if hr < 0 { Err(hr) } else { Ok([copy as u64, 0]) };
    }
    match vt {
        VT_BSTR if value[0] != 0 => {
            let s = value[0] as *const u16;
            let p = SysAllocStringLen(s, SysStringLen(s));
            if p.is_null() { Err(E_OUTOFMEMORY) } else { Ok([p as u64, 0]) }
        }
        VT_UNKNOWN | VT_DISPATCH => {
            add_ref(value[0] as usize);
            Ok(value)
        }
        VT_RECORD => {
            missing(b"oleaut32.dll!VariantCopy(VT_RECORD)", 0);
            Err(DISP_E_BADVARTYPE)
        }
        _ => Ok(value),
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn VariantCopy(dst: *mut Variant, src: *const Variant) -> HResult {
    if dst.is_null() || src.is_null() {
        return E_INVALIDARG;
    }
    if core::ptr::eq(dst, src) {
        return S_OK;
    }
    // SAFETY: the caller's VARIANTs.
    let (vt, value) = unsafe { ((*src).vt, (*src).value) };
    let copy = match copy_value(vt, value) {
        Ok(c) => c,
        Err(hr) => return hr,
    };
    VariantClear(dst);
    // SAFETY: as above.
    unsafe {
        (*dst).vt = vt;
        (*dst).value = copy;
    }
    S_OK
}

/// VariantCopyInd: a by-reference variant's target copied by value.
#[unsafe(no_mangle)]
pub extern "system" fn VariantCopyInd(dst: *mut Variant, src: *const Variant) -> HResult {
    if dst.is_null() || src.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's VARIANT.
    let (vt, value) = unsafe { ((*src).vt, (*src).value) };
    if vt & VT_BYREF == 0 {
        return VariantCopy(dst, src);
    }
    let base = vt & !VT_BYREF;
    let p = value[0] as *const u64;
    let target = if base == VT_VARIANT {
        // SAFETY: the referenced VARIANT.
        unsafe { (*(p as *const Variant)).value }
    } else {
        let size = element_size(base);
        let mut v = [0u64; 2];
        // SAFETY: the referenced value of that size.
        unsafe {
            core::ptr::copy_nonoverlapping(p.cast::<u8>(), v.as_mut_ptr().cast(), size.min(16))
        };
        v
    };
    let real_vt = if base == VT_VARIANT {
        // SAFETY: as above.
        unsafe { (*(p as *const Variant)).vt }
    } else {
        base
    };
    let copy = match copy_value(real_vt, target) {
        Ok(c) => c,
        Err(hr) => return hr,
    };
    if !core::ptr::eq(dst, src) {
        VariantClear(dst);
    }
    // SAFETY: the caller's VARIANT.
    unsafe {
        (*dst).vt = real_vt;
        (*dst).value = copy;
    }
    S_OK
}

/// The size of a value of type `vt` in arrays and by-reference variants.
pub fn element_size(vt: u16) -> usize {
    match vt & !(VT_BYREF | VT_ARRAY) {
        VT_I1 | VT_UI1 => 1,
        VT_I2 | VT_UI2 | VT_BOOL => 2,
        VT_I4 | VT_UI4 | VT_R4 | VT_INT | VT_UINT | VT_ERROR => 4,
        VT_VARIANT => 24,
        14 => 16, // VT_DECIMAL
        VT_EMPTY | VT_NULL => 0,
        _ => 8, // 64-bit numbers, CY, DATE, BSTR and interface pointers
    }
}

/// A variant's value for conversion.
#[derive(Clone, Copy)]
enum Value {
    Int(i128),
    Real(f64),
    Bool(bool),
    Text(*const u16),
}

fn value_of(vt: u16, v: [u64; 2]) -> Result<Value, HResult> {
    let raw = v[0];
    Ok(match vt {
        VT_EMPTY => Value::Int(0),
        VT_I1 => Value::Int((raw as i8).into()),
        VT_UI1 => Value::Int((raw as u8).into()),
        VT_I2 => Value::Int((raw as i16).into()),
        VT_UI2 => Value::Int((raw as u16).into()),
        VT_I4 | VT_INT => Value::Int((raw as i32).into()),
        VT_UI4 | VT_UINT => Value::Int((raw as u32).into()),
        VT_I8 => Value::Int((raw as i64).into()),
        VT_UI8 => Value::Int(raw.into()),
        VT_R4 => Value::Real(f32::from_bits(raw as u32).into()),
        VT_R8 => Value::Real(f64::from_bits(raw)),
        VT_CY => Value::Real(raw as i64 as f64 / 10000.0),
        VT_BOOL => Value::Bool(raw as i16 != 0),
        VT_BSTR => Value::Text(raw as *const u16),
        VT_NULL => return Err(DISP_E_TYPEMISMATCH),
        _ => {
            missing(b"oleaut32.dll!VariantChangeType(source type)", vt.into());
            return Err(DISP_E_TYPEMISMATCH);
        }
    })
}

/// `v` rounded to an integer, half to even (Automation's rounding).
fn round_even(v: f64) -> Option<i128> {
    if v.is_nan() || v.abs() > 1.8e19 {
        return None;
    }
    let whole = v as i128; // toward zero
    let frac = v - whole as f64;
    let away = if v < 0.0 { -1 } else { 1 };
    Some(match frac.abs() {
        f if f > 0.5 => whole + away,
        f if f == 0.5 && whole % 2 != 0 => whole + away,
        _ => whole,
    })
}

/// Parses a decimal number (spaces around it, a sign, digits, a point,
/// an exponent), as en-US formats them.
fn parse(s: *const u16) -> Option<f64> {
    let len = SysStringLen(s) as usize;
    // SAFETY: the BSTR's characters.
    let text = unsafe { core::slice::from_raw_parts(s, len) };
    let mut chars = text.iter().map(|&c| c as u32).peekable();
    while chars.peek() == Some(&0x20) {
        chars.next();
    }
    let negative = match chars.peek() {
        Some(&0x2d) => {
            chars.next();
            true
        }
        Some(&0x2b) => {
            chars.next();
            false
        }
        _ => false,
    };
    let (mut mantissa, mut scale, mut digits, mut point) = (0f64, 0i32, 0, false);
    while let Some(&c) = chars.peek() {
        match c {
            0x30..=0x39 => {
                mantissa = mantissa * 10.0 + f64::from(c - 0x30);
                digits += 1;
                if point {
                    scale -= 1;
                }
            }
            0x2e if !point => point = true,
            0x2c if !point => {} // thousands separators
            _ => break,
        }
        chars.next();
    }
    if digits == 0 {
        return None;
    }
    if matches!(chars.peek(), Some(&0x65) | Some(&0x45)) {
        chars.next();
        let exp_negative = match chars.peek() {
            Some(&0x2d) => {
                chars.next();
                true
            }
            Some(&0x2b) => {
                chars.next();
                false
            }
            _ => false,
        };
        let mut exp = 0i32;
        let mut any = false;
        while let Some(&c @ 0x30..=0x39) = chars.peek() {
            exp = exp.saturating_mul(10).saturating_add((c - 0x30) as i32);
            any = true;
            chars.next();
        }
        if !any {
            return None;
        }
        scale += if exp_negative { -exp } else { exp };
    }
    while chars.peek() == Some(&0x20) {
        chars.next();
    }
    if chars.next().is_some() {
        return None;
    }
    let mut v = mantissa;
    for _ in 0..scale.unsigned_abs().min(400) {
        if scale > 0 { v *= 10.0 } else { v /= 10.0 }
    }
    Some(if negative { -v } else { v })
}

/// ASCII `text` as a BSTR.
fn bstr(text: &[u8]) -> Result<u64, HResult> {
    let mut wide = [0u16; 64];
    for (w, &b) in wide.iter_mut().zip(text) {
        *w = b.into();
    }
    let p = SysAllocStringLen(wide.as_ptr(), text.len().min(64) as u32);
    if p.is_null() { Err(E_OUTOFMEMORY) } else { Ok(p as u64) }
}

/// The decimal digits of `v` into `out` (from its end); the start.
fn decimal(mut v: u128, out: &mut [u8], mut at: usize) -> usize {
    loop {
        at -= 1;
        out[at] = b'0' + (v % 10) as u8;
        v /= 10;
        if v == 0 {
            return at;
        }
    }
}

/// `v` as Automation prints a double: up to 15 significant digits, no
/// trailing zeros, an exponent ("E+n"/"E-n") outside 1e-5 .. 1e15.
fn format_real(v: f64, out: &mut [u8; 48]) -> usize {
    let mut n = 0;
    if v == 0.0 || v.is_nan() {
        out[0] = b'0';
        return 1;
    }
    let mut x = v;
    if x < 0.0 {
        out[n] = b'-';
        n += 1;
        x = -x;
    }
    // x = m * 10^e with 1 <= m < 10.
    let mut e = 0i32;
    while x >= 10.0 {
        x /= 10.0;
        e += 1;
    }
    while x < 1.0 {
        x *= 10.0;
        e -= 1;
    }
    // 15 significant digits, rounded.
    let mut digits = round_even(x * 1e14).unwrap_or(0) as u64;
    if digits >= 1_000_000_000_000_000 {
        digits /= 10;
        e += 1;
    }
    let mut text = [0u8; 15];
    for i in (0..15).rev() {
        text[i] = b'0' + (digits % 10) as u8;
        digits /= 10;
    }
    let mut last = 15;
    while last > 1 && text[last - 1] == b'0' {
        last -= 1;
    }
    let significant = &text[..last];
    let mut push = |b: u8| {
        out[n] = b;
        n += 1;
    };
    if (-5..15).contains(&e) {
        if e < 0 {
            push(b'0');
            push(b'.');
            for _ in 0..(-e - 1) {
                push(b'0');
            }
            significant.iter().for_each(|&d| push(d));
        } else {
            let whole = (e + 1) as usize;
            for i in 0..whole.max(significant.len()) {
                if i == whole {
                    push(b'.');
                }
                push(*significant.get(i).unwrap_or(&b'0'));
            }
        }
    } else {
        push(significant[0]);
        if significant.len() > 1 {
            push(b'.');
            significant[1..].iter().for_each(|&d| push(d));
        }
        push(b'E');
        push(if e < 0 { b'-' } else { b'+' });
        let mut buf = [0u8; 8];
        let start = decimal(e.unsigned_abs().into(), &mut buf, 8);
        buf[start..].iter().for_each(|&d| push(d));
    }
    n
}

/// Converts `value` to type `vt`.
fn convert(value: Value, vt: u16, flags: u16) -> Result<[u64; 2], HResult> {
    let as_real = |v: Value| -> Result<f64, HResult> {
        Ok(match v {
            Value::Int(i) => i as f64,
            Value::Real(r) => r,
            Value::Bool(b) => {
                if b {
                    -1.0
                } else {
                    0.0
                }
            }
            Value::Text(s) => parse(s).ok_or(DISP_E_TYPEMISMATCH)?,
        })
    };
    let as_int = |v: Value| -> Result<i128, HResult> {
        match v {
            Value::Int(i) => Ok(i),
            Value::Bool(b) => Ok(if b { -1 } else { 0 }),
            other => round_even(as_real(other)?).ok_or(DISP_E_OVERFLOW),
        }
    };
    let ranged = |v: Value, min: i128, max: i128| -> Result<u64, HResult> {
        let i = as_int(v)?;
        if i < min || i > max { Err(DISP_E_OVERFLOW) } else { Ok(i as u64) }
    };
    let bits = match vt {
        VT_EMPTY => 0,
        VT_I1 => ranged(value, i8::MIN.into(), i8::MAX.into())? & 0xff,
        VT_UI1 => ranged(value, 0, u8::MAX.into())?,
        VT_I2 => ranged(value, i16::MIN.into(), i16::MAX.into())? & 0xffff,
        VT_UI2 => ranged(value, 0, u16::MAX.into())?,
        VT_I4 | VT_INT => ranged(value, i32::MIN.into(), i32::MAX.into())? & 0xffff_ffff,
        VT_UI4 | VT_UINT => ranged(value, 0, u32::MAX.into())?,
        VT_I8 => ranged(value, i64::MIN.into(), i64::MAX.into())?,
        VT_UI8 => ranged(value, 0, u64::MAX.into())?,
        VT_R4 => u64::from((as_real(value)? as f32).to_bits()),
        VT_R8 => as_real(value)?.to_bits(),
        VT_CY => {
            let r = as_real(value)? * 10000.0;
            round_even(r)
                .filter(|&c| c >= i64::MIN.into() && c <= i64::MAX.into())
                .ok_or(DISP_E_OVERFLOW)? as u64
        }
        VT_BOOL => {
            let truth = match value {
                Value::Bool(b) => b,
                Value::Text(s) => match text_bool(s) {
                    Some(b) => b,
                    None => as_real(value)? != 0.0,
                },
                other => as_real(other)? != 0.0,
            };
            if truth { 0xffff } else { 0 }
        }
        VT_BSTR => match value {
            Value::Text(s) => {
                let p = SysAllocStringLen(s, SysStringLen(s));
                if p.is_null() {
                    return Err(E_OUTOFMEMORY);
                }
                p as u64
            }
            Value::Bool(b) if flags & (VARIANT_ALPHABOOL | VARIANT_LOCALBOOL) != 0 => {
                bstr(if b { b"True" } else { b"False" })?
            }
            Value::Bool(b) => bstr(if b { b"-1" } else { b"0" })?,
            Value::Int(i) => {
                let mut buf = [0u8; 48];
                let mut start = decimal(i.unsigned_abs(), &mut buf, 48);
                if i < 0 {
                    start -= 1;
                    buf[start] = b'-';
                }
                bstr(&buf[start..])?
            }
            Value::Real(r) => {
                let mut buf = [0u8; 48];
                let n = format_real(r, &mut buf);
                bstr(&buf[..n])?
            }
        },
        _ => {
            missing(b"oleaut32.dll!VariantChangeType(target type)", vt.into());
            return Err(DISP_E_TYPEMISMATCH);
        }
    };
    Ok([bits, 0])
}

/// "True"/"False" in any case.
fn text_bool(s: *const u16) -> Option<bool> {
    let len = SysStringLen(s) as usize;
    // SAFETY: the BSTR's characters.
    let text = unsafe { core::slice::from_raw_parts(s, len) };
    let is = |word: &[u8]| {
        text.len() == word.len()
            && text.iter().zip(word).all(|(&c, &w)| c < 128 && (c as u8).eq_ignore_ascii_case(&w))
    };
    if is(b"true") {
        Some(true)
    } else if is(b"false") {
        Some(false)
    } else {
        None
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn VariantChangeTypeEx(
    dst: *mut Variant,
    src: *const Variant,
    _lcid: u32,
    flags: u16,
    vt: u16,
) -> HResult {
    if dst.is_null() || src.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's VARIANT (dst may be src).
    let (src_vt, src_value) = unsafe { ((*src).vt, (*src).value) };
    if src_vt == vt {
        return VariantCopy(dst, src);
    }
    if src_vt & (VT_BYREF | VT_ARRAY) != 0 || vt & (VT_BYREF | VT_ARRAY) != 0 {
        missing(b"oleaut32.dll!VariantChangeType(arrays and references)", vt.into());
        return DISP_E_TYPEMISMATCH;
    }
    let converted = match value_of(src_vt, src_value).and_then(|v| convert(v, vt, flags)) {
        Ok(c) => c,
        Err(hr) => return hr,
    };
    VariantClear(dst);
    // SAFETY: the caller's VARIANT.
    unsafe {
        (*dst).vt = vt;
        (*dst).value = converted;
    }
    S_OK
}

#[unsafe(no_mangle)]
pub extern "system" fn VariantChangeType(
    dst: *mut Variant,
    src: *const Variant,
    flags: u16,
    vt: u16,
) -> HResult {
    VariantChangeTypeEx(dst, src, 0x400, flags, vt)
}

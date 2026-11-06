//! Variant arithmetic and logic (VarAdd, VarSub, VarMul, VarDiv,
//! VarIdiv, VarMod, VarAnd, VarOr, VarXor, VarNeg, VarNot), and
//! VarR4FromStr and VarBstrFromDate, as Microsoft documents them, with
//! the result types Wine's conformance tests (vartest.c, run on Windows)
//! record:
//!
//! - NULL in either operand gives NULL (VarAnd/VarOr: FALSE And NULL is
//!   FALSE, TRUE Or NULL is TRUE, as their tables say); BSTR plus BSTR is
//!   their concatenation; a BSTR with a number is read as a double.
//! - Add, subtract, multiply: the wider operand type (VT_UI1 < VT_I2 (also
//!   VT_BOOL and VT_EMPTY) < VT_I4 < VT_I8; VT_R4 (VT_R8 with VT_I4 or
//!   VT_I8) < VT_R8 (also a BSTR) < VT_DATE (add and subtract; date minus
//!   date is VT_R8) < VT_CY); an integer result that does not fit its
//!   type moves up (UI1 to I2 to I4 to R8); VT_I8 or VT_CY out of range is
//!   DISP_E_OVERFLOW.
//! - Divide: VT_R8 (VT_R4 when the wider operand is VT_R4 and the other
//!   at most VT_I2; VT_CY by a non-currency number is VT_CY);
//!   DISP_E_DIVBYZERO for a zero divisor, DISP_E_OVERFLOW for 0/0.
//! - Integer division, modulus and the bitwise operations work on the
//!   operands rounded to integers (half to even): VT_UI1 for two VT_UI1s,
//!   VT_I2 for types up to VT_I2, VT_I8 with a VT_I8, else VT_I4 (out of
//!   its range: DISP_E_OVERFLOW); VT_BOOL for two VT_BOOLs (bitwise);
//!   DISP_E_DIVBYZERO for a zero divisor (two VT_EMPTYs too).
//! - VarNeg keeps the type (VT_UI1, VT_BOOL and VT_EMPTY become VT_I2, a
//!   BSTR VT_R8; the minimum integer moves up a type); VarNot keeps
//!   integer types and VT_BOOL, gives VT_I2 -1 for VT_EMPTY and VT_I4 for
//!   real numbers, dates and currency (rounded), VT_BOOL for "True" and
//!   "False" text.
//!
//! Other operand types (VT_DECIMAL, objects, arrays, records, VT_ERROR)
//! are DISP_E_BADVARTYPE and reported to the ledger.

use super::{
    DISP_E_OVERFLOW, E_INVALIDARG, E_OUTOFMEMORY, HResult, S_OK, SysAllocStringLen, SysFreeString,
    SysStringLen, missing, put, wide_len,
};
use crate::variant::{
    DISP_E_BADVARTYPE, DISP_E_TYPEMISMATCH, VT_BOOL, VT_BSTR, VT_BYREF, VT_CY, VT_EMPTY, VT_I1,
    VT_I2, VT_I4, VT_I8, VT_INT, VT_NULL, VT_R4, VT_R8, VT_UI1, VT_UI2, VT_UI4, VT_UI8, VT_UINT,
    Variant, VariantCopyInd, VariantInit, chars, number_value, parse_number, round_even,
};

const VT_DATE: u16 = 7;
const DISP_E_DIVBYZERO: HResult = 0x8002_0012u32 as i32;

/// The operand kinds, narrowest first (the arithmetic order).
#[derive(Clone, Copy, PartialEq, PartialOrd)]
enum Kind {
    Ui1,
    I2,
    I4,
    I8,
    R4,
    R8,
    Date,
    Cy,
}

/// An operand: its type, its kind, its value as an integer when it is
/// one and as a real.
#[derive(Clone, Copy)]
struct Op {
    vt: u16,
    kind: Kind,
    int: Option<i128>,
    real: f64,
    text: Option<(*const u16, usize)>,
}

/// Reads an operand (by-reference ones through VariantCopyInd, the copy
/// cleared after). None: NULL.
fn operand(v: *const Variant, what: &[u8]) -> Result<Option<Op>, HResult> {
    if v.is_null() {
        return Err(E_INVALIDARG);
    }
    // SAFETY: the caller's VARIANT.
    let (mut vt, mut raw) = unsafe { ((*v).vt, (*v).value) };
    if vt & VT_BYREF != 0 {
        let mut copy = Variant::empty();
        VariantInit(&mut copy);
        let hr = VariantCopyInd(&mut copy, v);
        if hr != S_OK {
            return Err(hr);
        }
        // Only plain values come back here: nothing for the copy to own
        // except a BSTR, which the caller's reference keeps alive.
        vt = copy.vt;
        raw = copy.value;
        if vt == VT_BSTR {
            SysFreeString(raw[0] as *mut u16);
            // SAFETY: the by-reference BSTR the caller holds.
            raw[0] = unsafe { *((*v).value[0] as *const u64) };
        }
    }
    let x = raw[0];
    let int = |kind: Kind, i: i128| Op { vt, kind, int: Some(i), real: i as f64, text: None };
    Ok(Some(match vt {
        VT_NULL => return Ok(None),
        VT_EMPTY => int(Kind::I2, 0),
        VT_BOOL => int(Kind::I2, i128::from(x as i16)),
        VT_UI1 => int(Kind::Ui1, i128::from(x as u8)),
        VT_I1 => int(Kind::I4, i128::from(x as i8)),
        VT_I2 => int(Kind::I2, i128::from(x as i16)),
        VT_UI2 => int(Kind::I4, i128::from(x as u16)),
        VT_I4 | VT_INT => int(Kind::I4, i128::from(x as i32)),
        VT_UI4 | VT_UINT => int(Kind::I4, i128::from(x as u32)),
        VT_I8 => int(Kind::I8, i128::from(x as i64)),
        VT_UI8 => int(Kind::I8, i128::from(x)),
        VT_R4 => {
            Op { vt, kind: Kind::R4, int: None, real: f32::from_bits(x as u32).into(), text: None }
        }
        VT_R8 => Op { vt, kind: Kind::R8, int: None, real: f64::from_bits(x), text: None },
        VT_DATE => Op { vt, kind: Kind::Date, int: None, real: f64::from_bits(x), text: None },
        VT_CY => Op { vt, kind: Kind::Cy, int: None, real: x as i64 as f64 / 10000.0, text: None },
        VT_BSTR => {
            let s = x as *const u16;
            let n = SysStringLen(s) as usize;
            let real = parse_number(chars(s, n)).map(|n| number_value(&n));
            Op { vt, kind: Kind::R8, int: None, real: real.unwrap_or(f64::NAN), text: Some((s, n)) }
        }
        _ => {
            missing(what, vt.into());
            return Err(DISP_E_BADVARTYPE);
        }
    }))
}

/// A number operand's real value (a BSTR that is no number: a type
/// mismatch).
fn real(op: &Op) -> Result<f64, HResult> {
    if op.real.is_nan() && op.text.is_some() { Err(DISP_E_TYPEMISMATCH) } else { Ok(op.real) }
}

/// The operand as an integer (reals rounded half to even).
fn integer(op: &Op) -> Result<i128, HResult> {
    match op.int {
        Some(i) => Ok(i),
        None => round_even(real(op)?).ok_or(DISP_E_OVERFLOW),
    }
}

fn store(out: *mut Variant, vt: u16, value: u64) {
    // SAFETY: the caller's result VARIANT.
    unsafe {
        (*out).vt = vt;
        (*out).value = [value, 0];
    }
}

/// An integer result in the narrowest of `kind` and the wider types it
/// moves up to; `real` when even VT_I4 is too small (VT_I8 overflows).
fn store_int(out: *mut Variant, kind: Kind, v: i128) -> HResult {
    let fits = |lo: i128, hi: i128| v >= lo && v <= hi;
    let (vt, bits) = match kind {
        Kind::Ui1 if fits(0, 255) => (VT_UI1, v as u64),
        Kind::Ui1 | Kind::I2 if fits(i16::MIN.into(), i16::MAX.into()) => {
            (VT_I2, v as i16 as u16 as u64)
        }
        Kind::Ui1 | Kind::I2 | Kind::I4 if fits(i32::MIN.into(), i32::MAX.into()) => {
            (VT_I4, v as i32 as u32 as u64)
        }
        Kind::I8 if fits(i64::MIN.into(), i64::MAX.into()) => (VT_I8, v as i64 as u64),
        Kind::I8 => return DISP_E_OVERFLOW,
        _ => (VT_R8, (v as f64).to_bits()),
    };
    store(out, vt, bits);
    S_OK
}

fn store_real(out: *mut Variant, kind: Kind, v: f64) -> HResult {
    match kind {
        Kind::R4 => {
            let f = v as f32;
            if f.is_infinite() && !v.is_infinite() {
                store(out, VT_R8, v.to_bits());
            } else {
                store(out, VT_R4, u64::from(f.to_bits()));
            }
        }
        Kind::Date => store(out, VT_DATE, v.to_bits()),
        Kind::Cy => {
            let Some(c) =
                round_even(v * 10000.0).filter(|c| *c >= i64::MIN.into() && *c <= i64::MAX.into())
            else {
                return DISP_E_OVERFLOW;
            };
            store(out, VT_CY, c as i64 as u64);
        }
        _ => store(out, VT_R8, v.to_bits()),
    }
    S_OK
}

#[derive(Clone, Copy, PartialEq)]
enum Arith {
    Add,
    Sub,
    Mul,
}

fn arith(
    l: *const Variant,
    r: *const Variant,
    out: *mut Variant,
    op: Arith,
    what: &[u8],
) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    let (a, b) = match (operand(l, what), operand(r, what)) {
        (Err(hr), _) | (_, Err(hr)) => return hr,
        (Ok(Some(a)), Ok(Some(b))) => (a, b),
        _ => {
            store(out, VT_NULL, 0);
            return S_OK;
        }
    };
    if let (Arith::Add, Some((sa, na)), Some((sb, nb))) = (op, a.text, b.text) {
        let p = SysAllocStringLen(core::ptr::null(), (na + nb) as u32);
        if p.is_null() {
            return E_OUTOFMEMORY;
        }
        // SAFETY: a new BSTR of na + nb characters; the two sources.
        unsafe {
            core::ptr::copy_nonoverlapping(sa, p, na);
            core::ptr::copy_nonoverlapping(sb, p.add(na), nb);
        }
        store(out, VT_BSTR, p as u64);
        return S_OK;
    }
    let (wide, narrow) = if a.kind >= b.kind { (a.kind, b.kind) } else { (b.kind, a.kind) };
    let kind = match wide {
        Kind::Date if op == Arith::Mul => Kind::R8,
        Kind::Date if op == Arith::Sub && a.kind == Kind::Date && b.kind == Kind::Date => Kind::R8,
        Kind::R4 if narrow >= Kind::I4 => Kind::R8,
        k => k,
    };
    if kind <= Kind::I8 {
        let (x, y) = (a.int.unwrap_or(0), b.int.unwrap_or(0));
        let v = match op {
            Arith::Add => x + y,
            Arith::Sub => x - y,
            Arith::Mul => x * y,
        };
        return store_int(out, kind, v);
    }
    let (x, y) = match (real(&a), real(&b)) {
        (Ok(x), Ok(y)) => (x, y),
        (Err(hr), _) | (_, Err(hr)) => return hr,
    };
    let v = match op {
        Arith::Add => x + y,
        Arith::Sub => x - y,
        Arith::Mul => x * y,
    };
    store_real(out, kind, v)
}

#[unsafe(no_mangle)]
pub extern "system" fn VarAdd(l: *const Variant, r: *const Variant, out: *mut Variant) -> HResult {
    arith(l, r, out, Arith::Add, b"oleaut32.dll!VarAdd(type)")
}

#[unsafe(no_mangle)]
pub extern "system" fn VarSub(l: *const Variant, r: *const Variant, out: *mut Variant) -> HResult {
    arith(l, r, out, Arith::Sub, b"oleaut32.dll!VarSub(type)")
}

#[unsafe(no_mangle)]
pub extern "system" fn VarMul(l: *const Variant, r: *const Variant, out: *mut Variant) -> HResult {
    arith(l, r, out, Arith::Mul, b"oleaut32.dll!VarMul(type)")
}

/// Both operands, or NULL stored (None).
fn pair(
    l: *const Variant,
    r: *const Variant,
    out: *mut Variant,
    what: &[u8],
) -> Result<Option<(Op, Op)>, HResult> {
    if out.is_null() {
        return Err(E_INVALIDARG);
    }
    match (operand(l, what)?, operand(r, what)?) {
        (Some(a), Some(b)) => Ok(Some((a, b))),
        _ => {
            store(out, VT_NULL, 0);
            Ok(None)
        }
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn VarDiv(l: *const Variant, r: *const Variant, out: *mut Variant) -> HResult {
    let (a, b) = match pair(l, r, out, b"oleaut32.dll!VarDiv(type)") {
        Ok(Some(p)) => p,
        Ok(None) => return S_OK,
        Err(hr) => return hr,
    };
    let (x, y) = match (real(&a), real(&b)) {
        (Ok(x), Ok(y)) => (x, y),
        (Err(hr), _) | (_, Err(hr)) => return hr,
    };
    if y == 0.0 {
        return if x == 0.0 { DISP_E_OVERFLOW } else { DISP_E_DIVBYZERO };
    }
    let (wide, narrow) = if a.kind >= b.kind { (a.kind, b.kind) } else { (b.kind, a.kind) };
    let kind = match (wide, narrow) {
        (Kind::R4, n) if n <= Kind::I2 || n == Kind::R4 => Kind::R4,
        (Kind::Cy, Kind::Cy) => Kind::R8,
        (Kind::Cy, _) => Kind::Cy,
        _ => Kind::R8,
    };
    store_real(out, kind, x / y)
}

/// The integer type of an integer operation on kinds `a` and `b` (Ui1,
/// I2, I4 or I8).
fn integer_kind(a: &Op, b: &Op) -> Kind {
    match (a.kind, b.kind) {
        (Kind::I8, _) | (_, Kind::I8) => Kind::I8,
        (Kind::Ui1, Kind::Ui1) => Kind::Ui1,
        (x, y) if x <= Kind::I2 && y <= Kind::I2 => Kind::I2,
        _ => Kind::I4,
    }
}

/// A result of an integer operation, which must fit the type.
fn store_exact(out: *mut Variant, kind: Kind, v: i128) -> HResult {
    let ok = match kind {
        Kind::Ui1 => (0..=255).contains(&v),
        Kind::I2 => v >= i16::MIN.into() && v <= i16::MAX.into(),
        Kind::I4 => v >= i32::MIN.into() && v <= i32::MAX.into(),
        _ => v >= i64::MIN.into() && v <= i64::MAX.into(),
    };
    if !ok {
        return DISP_E_OVERFLOW;
    }
    store_int(out, kind, v)
}

fn int_div(
    l: *const Variant,
    r: *const Variant,
    out: *mut Variant,
    modulus: bool,
    what: &[u8],
) -> HResult {
    let (a, b) = match pair(l, r, out, what) {
        Ok(Some(p)) => p,
        Ok(None) => return S_OK,
        Err(hr) => return hr,
    };
    let (x, y) = match (integer(&a), integer(&b)) {
        (Ok(x), Ok(y)) => (x, y),
        (Err(hr), _) | (_, Err(hr)) => return hr,
    };
    if y == 0 {
        return DISP_E_DIVBYZERO;
    }
    // Rust's / and % truncate toward zero, the remainder taking the
    // dividend's sign, as Automation's.
    store_exact(out, integer_kind(&a, &b), if modulus { x % y } else { x / y })
}

#[unsafe(no_mangle)]
pub extern "system" fn VarIdiv(l: *const Variant, r: *const Variant, out: *mut Variant) -> HResult {
    int_div(l, r, out, false, b"oleaut32.dll!VarIdiv(type)")
}

#[unsafe(no_mangle)]
pub extern "system" fn VarMod(l: *const Variant, r: *const Variant, out: *mut Variant) -> HResult {
    int_div(l, r, out, true, b"oleaut32.dll!VarMod(type)")
}

#[derive(Clone, Copy, PartialEq)]
enum Bits {
    And,
    Or,
    Xor,
}

fn bitwise(
    l: *const Variant,
    r: *const Variant,
    out: *mut Variant,
    op: Bits,
    what: &[u8],
) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    let (a, b) = match (operand(l, what), operand(r, what)) {
        (Err(hr), _) | (_, Err(hr)) => return hr,
        (Ok(a), Ok(b)) => (a, b),
    };
    let (a, b) = match (a, b) {
        (Some(a), Some(b)) => (a, b),
        (None, None) => {
            store(out, VT_NULL, 0);
            return S_OK;
        }
        (Some(x), None) | (None, Some(x)) => {
            // FALSE And NULL is FALSE, TRUE Or NULL is TRUE (the operand
            // itself); anything else with NULL is NULL.
            let v = match integer(&x) {
                Ok(v) => v,
                Err(hr) => return hr,
            };
            let decides = match op {
                Bits::And => v == 0,
                Bits::Or => v == -1 || (x.kind == Kind::Ui1 && v == 255),
                Bits::Xor => false,
            };
            if !decides {
                store(out, VT_NULL, 0);
                return S_OK;
            }
            if x.vt == VT_BOOL {
                store(out, VT_BOOL, v as i16 as u16 as u64);
                return S_OK;
            }
            let k = integer_kind(&x, &x);
            return store_exact(out, k, v);
        }
    };
    let (x, y) = match (integer(&a), integer(&b)) {
        (Ok(x), Ok(y)) => (x, y),
        (Err(hr), _) | (_, Err(hr)) => return hr,
    };
    let v = match op {
        Bits::And => x & y,
        Bits::Or => x | y,
        Bits::Xor => x ^ y,
    };
    if a.vt == VT_BOOL && b.vt == VT_BOOL {
        store(out, VT_BOOL, v as i16 as u16 as u64);
        return S_OK;
    }
    let kind = integer_kind(&a, &b);
    // Within the type the bits of two in-range operands stay in range;
    // check the operands (a real too large for VT_I4 overflows).
    let fits = |k: Kind, v: i128| match k {
        Kind::Ui1 => (0..=255).contains(&v),
        Kind::I2 => v >= i16::MIN.into() && v <= i16::MAX.into(),
        Kind::I4 => v >= i32::MIN.into() && v <= i32::MAX.into(),
        _ => v >= i64::MIN.into() && v <= i64::MAX.into(),
    };
    if !fits(kind, x) || !fits(kind, y) {
        return DISP_E_OVERFLOW;
    }
    store_exact(out, kind, v)
}

#[unsafe(no_mangle)]
pub extern "system" fn VarAnd(l: *const Variant, r: *const Variant, out: *mut Variant) -> HResult {
    bitwise(l, r, out, Bits::And, b"oleaut32.dll!VarAnd(type)")
}

#[unsafe(no_mangle)]
pub extern "system" fn VarOr(l: *const Variant, r: *const Variant, out: *mut Variant) -> HResult {
    bitwise(l, r, out, Bits::Or, b"oleaut32.dll!VarOr(type)")
}

#[unsafe(no_mangle)]
pub extern "system" fn VarXor(l: *const Variant, r: *const Variant, out: *mut Variant) -> HResult {
    bitwise(l, r, out, Bits::Xor, b"oleaut32.dll!VarXor(type)")
}

#[unsafe(no_mangle)]
pub extern "system" fn VarNeg(v: *const Variant, out: *mut Variant) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    let a = match operand(v, b"oleaut32.dll!VarNeg(type)") {
        Ok(Some(a)) => a,
        Ok(None) => {
            store(out, VT_NULL, 0);
            return S_OK;
        }
        Err(hr) => return hr,
    };
    match (a.int, a.kind) {
        (Some(i), Kind::Ui1) => store_int(out, Kind::I2, -i),
        (Some(i), k) => store_int(out, k, -i),
        (None, k) => match real(&a) {
            Ok(x) => store_real(out, k, -x),
            Err(hr) => hr,
        },
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn VarNot(v: *const Variant, out: *mut Variant) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    let a = match operand(v, b"oleaut32.dll!VarNot(type)") {
        Ok(Some(a)) => a,
        Ok(None) => {
            store(out, VT_NULL, 0);
            return S_OK;
        }
        Err(hr) => return hr,
    };
    if a.vt == VT_BOOL {
        store(out, VT_BOOL, !(a.int.unwrap_or(0) as i16) as u16 as u64);
        return S_OK;
    }
    if let Some((s, n)) = a.text {
        let word = chars(s, n);
        let is = |w: &[u8]| {
            word.len() == w.len()
                && word.iter().zip(w).all(|(&c, w)| c < 128 && (c as u8).eq_ignore_ascii_case(w))
        };
        if is(b"true") || is(b"false") {
            let truth: i16 = if is(b"true") { -1 } else { 0 };
            store(out, VT_BOOL, !truth as u16 as u64);
            return S_OK;
        }
    }
    let x = match integer(&a) {
        Ok(x) => x,
        Err(hr) => return hr,
    };
    let kind = match a.kind {
        Kind::Ui1 => {
            store(out, VT_UI1, u64::from(!(x as u8)));
            return S_OK;
        }
        Kind::I2 => Kind::I2,
        Kind::I8 => Kind::I8,
        _ => Kind::I4,
    };
    store_exact(out, kind, !x)
}

/// VarR4FromStr: the text as a float (DISP_E_OVERFLOW beyond its range).
#[unsafe(no_mangle)]
pub extern "system" fn VarR4FromStr(
    s: *const u16,
    lcid: u32,
    _flags: u32,
    out: *mut f32,
) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    if s.is_null() {
        return DISP_E_TYPEMISMATCH;
    }
    crate::conversions::check_locale(b"oleaut32.dll!VarR4FromStr(locale)", lcid);
    let Some(n) = parse_number(chars(s, wide_len(s))) else { return DISP_E_TYPEMISMATCH };
    let v = number_value(&n);
    let f = v as f32;
    if f.is_infinite() {
        return DISP_E_OVERFLOW;
    }
    put(out, f);
    S_OK
}

/// VarBstrFromDate: the date as en-US writes it ("M/D/YYYY h:mm:ss AM"):
/// the date alone for midnight, the time alone for day 0 (30 December
/// 1899); VAR_DATEVALUEONLY and VAR_TIMEVALUEONLY keep one part.
#[unsafe(no_mangle)]
pub extern "system" fn VarBstrFromDate(
    date: f64,
    lcid: u32,
    flags: u32,
    out: *mut *mut u16,
) -> HResult {
    const VAR_TIMEVALUEONLY: u32 = 1;
    const VAR_DATEVALUEONLY: u32 = 2;
    if out.is_null() {
        return E_INVALIDARG;
    }
    crate::conversions::check_locale(b"oleaut32.dll!VarBstrFromDate(locale)", lcid);
    let mut st = crate::date::SystemTime::default();
    if crate::date::VariantTimeToSystemTime(date, &mut st) == 0 {
        return E_INVALIDARG;
    }
    let mut text = [0u8; 48];
    let mut n = 0;
    let mut push = |bytes: &[u8]| {
        for &b in bytes {
            text[n] = b;
            n += 1;
        }
    };
    let number = |v: u16, width: usize| -> ([u8; 8], usize) {
        let mut b = [b'0'; 8];
        let start = crate::variant::decimal(v.into(), &mut b, 8);
        let start = start.min(8 - width);
        (b, start)
    };
    let midnight = st.hour == 0 && st.minute == 0 && st.second == 0;
    let day_zero = date as i64 == 0;
    let show_date = flags & VAR_TIMEVALUEONLY == 0 && (flags & VAR_DATEVALUEONLY != 0 || !day_zero);
    let show_time = flags & VAR_DATEVALUEONLY == 0
        && (flags & VAR_TIMEVALUEONLY != 0 || !midnight || !show_date);
    if show_date {
        let (b, s) = number(st.month, 1);
        push(&b[s..]);
        push(b"/");
        let (b, s) = number(st.day, 1);
        push(&b[s..]);
        push(b"/");
        let (b, s) = number(st.year, 1);
        push(&b[s..]);
    }
    if show_time {
        if show_date {
            push(b" ");
        }
        let hour12 = match st.hour % 12 {
            0 => 12,
            h => h,
        };
        let (b, s) = number(hour12, 1);
        push(&b[s..]);
        push(b":");
        let (b, s) = number(st.minute, 2);
        push(&b[s..]);
        push(b":");
        let (b, s) = number(st.second, 2);
        push(&b[s..]);
        push(if st.hour < 12 { b" AM" } else { b" PM" });
    }
    match crate::variant::bstr(&text[..n]) {
        Ok(p) => {
            put(out, p as *mut u16);
            S_OK
        }
        Err(hr) => hr,
    }
}

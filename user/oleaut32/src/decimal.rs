//! DECIMAL arithmetic (VarDecAdd, VarDecSub, VarDecMul, VarDecDiv,
//! VarDecAbs, VarDecFix, VarDecInt, VarDecNeg, VarDecRound, VarDecCmp)
//! and conversions (VarDecFromI4, VarDecFromR8, VarR8FromDec), as
//! documented and as Wine's vartest.c records Windows' answers: a DECIMAL
//! is a 96-bit magnitude, a power-of-ten scale (0..=28) and a sign. Exact
//! results are kept exact; a result with more digits than 96 bits hold
//! loses digits from its scale, rounded half to even (Automation's
//! banker's rounding); one too large even at scale 0 is DISP_E_OVERFLOW.
//! A quotient carries as many digits as fit, up to scale 28, and no
//! trailing zeros past the exact value. Office's Click-to-Run UI imports
//! them by ordinal (build.rs).

use super::{DISP_E_OVERFLOW, Decimal, E_INVALIDARG, HResult, S_OK, put};

const DISP_E_DIVBYZERO: HResult = 0x8002_0012u32 as i32;
const MAX: u128 = (1 << 96) - 1;
const SIGN: u8 = 0x80;
const VARCMP_LT: HResult = 0;
const VARCMP_EQ: HResult = 1;
const VARCMP_GT: HResult = 2;

/// 10^n for n up to 38.
fn pow10(n: u32) -> u128 {
    10u128.pow(n)
}

/// A decimal value: sign, scale, magnitude (at most MAX).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Dec {
    neg: bool,
    scale: u32,
    m: u128,
}

fn read(d: *const Decimal) -> Option<Dec> {
    if d.is_null() {
        return None;
    }
    // SAFETY: the caller's DECIMAL.
    let d = unsafe { &*d };
    (d.scale <= 28).then(|| Dec {
        neg: d.sign & SIGN != 0,
        scale: d.scale.into(),
        m: (u128::from(d.hi) << 64) | u128::from(d.lo),
    })
}

fn write(out: *mut Decimal, v: Dec) {
    put(
        out,
        Decimal {
            reserved: 0,
            scale: v.scale as u8,
            sign: if v.neg && v.m != 0 { SIGN } else { 0 },
            hi: (v.m >> 64) as u32,
            lo: v.m as u64,
        },
    );
}

/// A 256-bit magnitude, eight 32-bit limbs, least significant first.
type Big = [u32; 8];

fn big(m: u128) -> Big {
    let mut b = [0u32; 8];
    for (i, l) in b.iter_mut().take(4).enumerate() {
        *l = (m >> (32 * i)) as u32;
    }
    b
}

/// `b` times `k`; false when it no longer fits.
fn mul_small(b: &mut Big, k: u32) -> bool {
    let mut carry = 0u64;
    for l in b.iter_mut() {
        let v = u64::from(*l) * u64::from(k) + carry;
        *l = v as u32;
        carry = v >> 32;
    }
    carry == 0
}

/// `b` divided by `k`; the remainder.
fn div_small(b: &mut Big, k: u32) -> u32 {
    let mut rem = 0u64;
    for l in b.iter_mut().rev() {
        let v = (rem << 32) | u64::from(*l);
        *l = (v / u64::from(k)) as u32;
        rem = v % u64::from(k);
    }
    rem as u32
}

fn add(a: &Big, b: &Big) -> Big {
    let mut out = [0u32; 8];
    let mut carry = 0u64;
    for i in 0..8 {
        let v = u64::from(a[i]) + u64::from(b[i]) + carry;
        out[i] = v as u32;
        carry = v >> 32;
    }
    out
}

/// `a - b` for `a >= b`.
fn sub(a: &Big, b: &Big) -> Big {
    let mut out = [0u32; 8];
    let mut borrow = 0i64;
    for i in 0..8 {
        let v = i64::from(a[i]) - i64::from(b[i]) - borrow;
        out[i] = v.rem_euclid(1 << 32) as u32;
        borrow = i64::from(v < 0);
    }
    out
}

fn cmp(a: &Big, b: &Big) -> core::cmp::Ordering {
    a.iter().rev().cmp(b.iter().rev())
}

fn mul(a: u128, b: u128) -> Big {
    let (a, b) = (big(a), big(b));
    let mut out = [0u64; 9];
    for i in 0..4 {
        let mut carry = 0u64;
        for j in 0..4 {
            let v = out[i + j] + u64::from(a[i]) * u64::from(b[j]) + carry;
            out[i + j] = v & 0xffff_ffff;
            carry = v >> 32;
        }
        out[i + 4] += carry;
    }
    let mut r = [0u32; 8];
    for (d, &s) in r.iter_mut().zip(&out) {
        *d = s as u32;
    }
    r
}

fn fits(b: &Big) -> Option<u128> {
    if b[3..].iter().any(|&l| l != 0) {
        return None;
    }
    let m = u128::from(b[0]) | u128::from(b[1]) << 32 | u128::from(b[2]) << 64;
    (m <= MAX).then_some(m)
}

/// The exact value `b` / 10^scale made a DECIMAL: digits dropped from
/// the scale until the magnitude fits 96 bits and the scale 28, rounded
/// half to even. None: too large.
fn reduce(mut b: Big, mut scale: u32, neg: bool) -> Option<Dec> {
    let (mut last, mut sticky) = (0u32, false);
    let mut dropped = false;
    while scale > 28 || (fits(&b).is_none() && scale > 0) {
        sticky |= last != 0;
        last = div_small(&mut b, 10);
        scale -= 1;
        dropped = true;
    }
    let mut m = fits(&b)?;
    if dropped && (last > 5 || (last == 5 && (sticky || m & 1 == 1))) {
        m += 1;
        if m > MAX {
            // 2^96 exactly: one digit more (scale > 0 here, else the
            // value did not fit before rounding either).
            if scale == 0 {
                return None;
            }
            m = m.div_ceil(10);
            scale -= 1;
        }
    }
    Some(Dec { neg, scale, m })
}

fn add_dec(a: Dec, b: Dec) -> Option<Dec> {
    let scale = a.scale.max(b.scale);
    let (mut x, mut y) = (big(a.m), big(b.m));
    for _ in a.scale..scale {
        mul_small(&mut x, 10);
    }
    for _ in b.scale..scale {
        mul_small(&mut y, 10);
    }
    if a.neg == b.neg {
        return reduce(add(&x, &y), scale, a.neg);
    }
    match cmp(&x, &y) {
        core::cmp::Ordering::Less => reduce(sub(&y, &x), scale, b.neg),
        _ => reduce(sub(&x, &y), scale, a.neg),
    }
}

fn mul_dec(a: Dec, b: Dec) -> Option<Dec> {
    reduce(mul(a.m, b.m), a.scale + b.scale, a.neg != b.neg)
}

fn div_dec(a: Dec, b: Dec) -> Result<Dec, HResult> {
    if b.m == 0 {
        return Err(DISP_E_DIVBYZERO);
    }
    let (mut q, mut r) = (a.m / b.m, a.m % b.m);
    let mut scale = i64::from(a.scale) - i64::from(b.scale);
    // A divisor of a larger scale: the quotient's scale is negative until
    // digits are brought down.
    while scale < 0 {
        let digit = (r * 10) / b.m;
        r = (r * 10) % b.m;
        q = q.checked_mul(10).map(|v| v + digit).filter(|&v| v <= MAX).ok_or(DISP_E_OVERFLOW)?;
        scale += 1;
    }
    while r != 0 && scale < 28 {
        let digit = (r * 10) / b.m;
        let next = q * 10 + digit;
        if next > MAX {
            break;
        }
        q = next;
        r = (r * 10) % b.m;
        scale += 1;
    }
    let twice = r * 2;
    if twice > b.m || (twice == b.m && q & 1 == 1) {
        q += 1;
        if q > MAX {
            if scale == 0 {
                return Err(DISP_E_OVERFLOW);
            }
            q = q.div_ceil(10);
            scale -= 1;
        }
    }
    Ok(Dec { neg: a.neg != b.neg, scale: scale as u32, m: q })
}

/// The value with its scale cut to `places` (at most its own), the
/// dropped digits decided by `round`: (whole, dropped, divisor) -> whole.
fn cut(v: Dec, places: u32, round: impl Fn(u128, u128, u128) -> u128) -> Dec {
    if places >= v.scale {
        return v;
    }
    let divisor = pow10(v.scale - places);
    let (whole, rest) = (v.m / divisor, v.m % divisor);
    Dec { neg: v.neg, scale: places, m: round(whole, rest, divisor) }
}

fn binary(
    a: *const Decimal,
    b: *const Decimal,
    out: *mut Decimal,
    f: impl FnOnce(Dec, Dec) -> Result<Dec, HResult>,
) -> HResult {
    let (Some(x), Some(y)) = (read(a), read(b)) else { return E_INVALIDARG };
    if out.is_null() {
        return E_INVALIDARG;
    }
    match f(x, y) {
        Ok(v) => {
            write(out, v);
            S_OK
        }
        Err(e) => e,
    }
}

fn unary(a: *const Decimal, out: *mut Decimal, f: impl FnOnce(Dec) -> Dec) -> HResult {
    let Some(x) = read(a) else { return E_INVALIDARG };
    if out.is_null() {
        return E_INVALIDARG;
    }
    write(out, f(x));
    S_OK
}

#[unsafe(no_mangle)]
pub extern "system" fn VarDecAdd(
    a: *const Decimal,
    b: *const Decimal,
    out: *mut Decimal,
) -> HResult {
    binary(a, b, out, |x, y| add_dec(x, y).ok_or(DISP_E_OVERFLOW))
}

#[unsafe(no_mangle)]
pub extern "system" fn VarDecSub(
    a: *const Decimal,
    b: *const Decimal,
    out: *mut Decimal,
) -> HResult {
    binary(a, b, out, |x, y| add_dec(x, Dec { neg: !y.neg, ..y }).ok_or(DISP_E_OVERFLOW))
}

#[unsafe(no_mangle)]
pub extern "system" fn VarDecMul(
    a: *const Decimal,
    b: *const Decimal,
    out: *mut Decimal,
) -> HResult {
    binary(a, b, out, |x, y| mul_dec(x, y).ok_or(DISP_E_OVERFLOW))
}

#[unsafe(no_mangle)]
pub extern "system" fn VarDecDiv(
    a: *const Decimal,
    b: *const Decimal,
    out: *mut Decimal,
) -> HResult {
    binary(a, b, out, div_dec)
}

#[unsafe(no_mangle)]
pub extern "system" fn VarDecAbs(a: *const Decimal, out: *mut Decimal) -> HResult {
    unary(a, out, |x| Dec { neg: false, ..x })
}

#[unsafe(no_mangle)]
pub extern "system" fn VarDecNeg(a: *const Decimal, out: *mut Decimal) -> HResult {
    unary(a, out, |x| Dec { neg: !x.neg, ..x })
}

/// VarDecFix: the integer part, towards zero.
#[unsafe(no_mangle)]
pub extern "system" fn VarDecFix(a: *const Decimal, out: *mut Decimal) -> HResult {
    unary(a, out, |x| cut(x, 0, |whole, _, _| whole))
}

/// VarDecInt: the integer part, towards negative infinity (a negative
/// value with a fraction is the next integer down).
#[unsafe(no_mangle)]
pub extern "system" fn VarDecInt(a: *const Decimal, out: *mut Decimal) -> HResult {
    unary(a, out, |x| cut(x, 0, |whole, rest, _| whole + u128::from(x.neg && rest != 0)))
}

/// VarDecRound(pdecIn, cDecimals, pdecResult): to `places` decimal
/// places, half to even; a negative count is E_INVALIDARG.
#[unsafe(no_mangle)]
pub extern "system" fn VarDecRound(a: *const Decimal, places: i32, out: *mut Decimal) -> HResult {
    if places < 0 {
        return E_INVALIDARG;
    }
    unary(a, out, |x| {
        cut(x, places as u32, |whole, rest, divisor| {
            let twice = rest * 2;
            whole + u128::from(twice > divisor || (twice == divisor && whole & 1 == 1))
        })
    })
}

/// VarDecCmp: VARCMP_LT, VARCMP_EQ or VARCMP_GT (zero has no sign).
#[unsafe(no_mangle)]
pub extern "system" fn VarDecCmp(a: *const Decimal, b: *const Decimal) -> HResult {
    let (Some(x), Some(y)) = (read(a), read(b)) else { return E_INVALIDARG };
    let neg = |v: Dec| v.neg && v.m != 0;
    let order = match (neg(x), neg(y)) {
        (false, true) => core::cmp::Ordering::Greater,
        (true, false) => core::cmp::Ordering::Less,
        (n, _) => {
            let scale = x.scale.max(y.scale);
            let (mut p, mut q) = (big(x.m), big(y.m));
            for _ in x.scale..scale {
                mul_small(&mut p, 10);
            }
            for _ in y.scale..scale {
                mul_small(&mut q, 10);
            }
            let o = cmp(&p, &q);
            if n { o.reverse() } else { o }
        }
    };
    match order {
        core::cmp::Ordering::Less => VARCMP_LT,
        core::cmp::Ordering::Equal => VARCMP_EQ,
        core::cmp::Ordering::Greater => VARCMP_GT,
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn VarDecFromI4(v: i32, out: *mut Decimal) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    write(out, Dec { neg: v < 0, scale: 0, m: u128::from(v.unsigned_abs()) });
    S_OK
}

/// f64 powers of ten from 10^0 to 10^28.
fn pow10_f(n: u32) -> f64 {
    let mut v = 1.0;
    for _ in 0..n {
        v *= 10.0;
    }
    v
}

/// VarDecFromR8: the double to 15 significant digits (a double's
/// precision as Automation's conversions take it), no trailing zeros.
/// Infinite, NaN or beyond the DECIMAL's range: DISP_E_OVERFLOW.
#[unsafe(no_mangle)]
pub extern "system" fn VarDecFromR8(v: f64, out: *mut Decimal) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    if !v.is_finite() || v.abs() >= 79_228_162_514_264_337_593_543_950_335.0 {
        return DISP_E_OVERFLOW;
    }
    let a = v.abs();
    if a < 1e-29 {
        write(out, Dec { neg: false, scale: 0, m: 0 });
        return S_OK;
    }
    // The exponent e: 10^e <= a < 10^(e+1).
    let mut e: i32 = 0;
    while e < 28 && a >= pow10_f((e + 1) as u32) {
        e += 1;
    }
    while e > -29 && a < if e >= 0 { pow10_f(e as u32) } else { 1.0 / pow10_f((-e) as u32) } {
        e -= 1;
    }
    // Fifteen digits: a * 10^(14 - e), rounded.
    let shift = 14 - e;
    let scaled = if shift >= 0 { a * pow10_f(shift as u32) } else { a / pow10_f((-shift) as u32) };
    let mut m = (scaled + 0.5) as u128;
    let mut scale = shift;
    if m >= pow10(15) {
        m = (m + 5) / 10;
        scale -= 1;
    }
    if scale < 0 {
        m *= pow10((-scale) as u32);
        scale = 0;
    }
    if scale > 28 {
        let d = pow10((scale - 28) as u32);
        m = (m + d / 2) / d;
        scale = 28;
    }
    while scale > 0 && m.is_multiple_of(10) {
        m /= 10;
        scale -= 1;
    }
    write(out, Dec { neg: v < 0.0, scale: scale as u32, m });
    S_OK
}

/// VarR8FromDec: the magnitude over 10^scale as a double.
#[unsafe(no_mangle)]
pub extern "system" fn VarR8FromDec(d: *const Decimal, out: *mut f64) -> HResult {
    let Some(x) = read(d) else { return E_INVALIDARG };
    if out.is_null() {
        return E_INVALIDARG;
    }
    let v = x.m as f64 / pow10_f(x.scale);
    put(out, if x.neg { -v } else { v });
    S_OK
}

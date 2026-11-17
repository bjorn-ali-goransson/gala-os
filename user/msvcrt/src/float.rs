//! printf's floating-point conversions, %f %e %g (and upper case), as
//! msvcrt.dll prints them: the decimal expansion of the double (exact,
//! from a small bignum, user/decfloat) rounded to 17 significant digits, the digits
//! after those printed as zeros (msvcrt's conversion keeps 17: `%.20f` of
//! 0.1 is `0.10000000000000001000`), then rounded half away from zero at
//! the last digit asked for; exponents with a sign and at least three
//! digits (`1e+100`, `2.5e-005`). Precision is capped at `MAX_PRECISION`.
//!
//! The UCRT's way (`Style`, gala's ucrtbase): every digit exact (`%.30f`
//! of 1/3 is `0.333333333333333314829616256247`), still rounded half away
//! from zero, and exponents of at least two digits (`1e+15`; three with
//! `_CRT_INTERNAL_PRINTF_LEGACY_THREE_DIGIT_EXPONENTS`).

/// The longest result: 309 integer digits, a point, the precision.
pub const MAX_OUT: usize = 312 + MAX_PRECISION;

use decfloat::{
    MAX_DIGITS, MAX_PRECISION, Rounding, SIGNIFICANT, expand_limited, keeps_even, round,
    significant,
};

struct Writer<'a> {
    out: &'a mut [u8; MAX_OUT],
    n: usize,
    style: Style,
}

/// msvcrt's conversion or the UCRT's (module comment).
#[derive(Clone, Copy)]
pub struct Style {
    /// Exact digits (UCRT) instead of 17 significant ones (msvcrt).
    pub exact: bool,
    /// The fewest exponent digits: 3 (msvcrt) or 2 (UCRT).
    pub exponent_digits: u8,
    /// An exact tie (a 5 and nothing after it) rounds to the even digit
    /// (the UCRT's _CRT_INTERNAL_PRINTF_STANDARD_ROUNDING), not away from
    /// zero.
    pub ties_even: bool,
}

impl Style {
    pub const MSVCRT: Self = Self { exact: false, exponent_digits: 3, ties_even: false };

    /// How this style rounds the expansion (user/decfloat).
    fn rounding(self) -> Rounding {
        Rounding { exact: self.exact, ties_even: self.ties_even }
    }
}

impl Writer<'_> {
    fn push(&mut self, c: u8) {
        if self.n < MAX_OUT {
            self.out[self.n] = c;
            self.n += 1;
        }
    }

    fn digits(&mut self, d: &[u8]) {
        for &x in d {
            self.push(b'0' + x);
        }
    }

    fn exponent(&mut self, upper: bool, exp: i32) {
        self.push(if upper { b'E' } else { b'e' });
        self.push(if exp < 0 { b'-' } else { b'+' });
        let e = exp.unsigned_abs();
        let digits = [e / 100 % 10, e / 10 % 10, e % 10];
        if e >= 1000 {
            self.push(b'0' + (e / 1000) as u8);
        }
        let skip = usize::from(self.style.exponent_digits < 3 && e < 100);
        for x in &digits[skip..] {
            self.push(b'0' + *x as u8);
        }
    }

    /// Drops trailing zeros of the fraction (and a bare point) from `from`.
    fn strip_zeros(&mut self, from: usize) {
        if !self.out[from..self.n].contains(&b'.') {
            return;
        }
        while self.n > from && self.out[self.n - 1] == b'0' {
            self.n -= 1;
        }
        if self.n > from && self.out[self.n - 1] == b'.' {
            self.n -= 1;
        }
    }
}

/// |v| in fixed point with `p` decimals.
#[inline(never)]
fn fixed(w: &mut Writer<'_>, v: f64, p: usize, alt: bool) {
    let mut d = [0u8; MAX_DIGITS];
    // Enough digits for the 17-digit rounding of the smallest values.
    let exact = w.style.exact;
    let (n, mut int_digits) = expand_limited(v, (p + 1).max(324 + SIGNIFICANT + 1), &mut d, exact);
    let (mut start, mut keep) = (0, int_digits + p);
    if int_digits == 0 {
        // A leading "0" for the integer part (room: expand starts at 0).
        d.copy_within(0..n.min(MAX_DIGITS - 1), 1);
        d[0] = 0;
        int_digits = 1;
        keep += 1;
    }
    let next = if keep < MAX_DIGITS { d[keep] } else { 0 };
    let tail_zero = d.get(keep + 1..).is_none_or(|t| t.iter().all(|&x| x == 0));
    let even = keeps_even(w.style.rounding(), &d[..keep], next, tail_zero);
    if !even && round(&mut d[..keep], next) {
        // 9.99 -> 10.0: one more integer digit.
        start = 0;
        w.push(b'1');
    }
    w.digits(&d[start..int_digits]);
    if p > 0 || alt {
        w.push(b'.');
    }
    w.digits(&d[int_digits..keep]);
}

/// |v| as d.ddd e+xxx with `p` decimals.
#[inline(never)]
fn scientific(w: &mut Writer<'_>, v: f64, p: usize, alt: bool, upper: bool) {
    let mut sig = [0u8; MAX_PRECISION + 1];
    let exp = significant(v, p + 1, &mut sig, w.style.rounding());
    w.digits(&sig[..1]);
    if p > 0 || alt {
        w.push(b'.');
    }
    w.digits(&sig[1..=p]);
    w.exponent(upper, exp);
}

/// %g: `p` significant digits, fixed point if the exponent is in
/// [-4, p), else scientific; trailing zeros dropped unless `alt`.
#[inline(never)]
fn general(w: &mut Writer<'_>, v: f64, p: usize, alt: bool, upper: bool) {
    let p = p.max(1);
    let mut sig = [0u8; MAX_PRECISION + 1];
    let exp = significant(v, p, &mut sig, w.style.rounding());
    let from = w.n;
    if exp < -4 || exp >= p as i32 {
        w.digits(&sig[..1]);
        if p > 1 || alt {
            w.push(b'.');
        }
        w.digits(&sig[1..p]);
        if !alt {
            w.strip_zeros(from);
        }
        w.exponent(upper, exp);
    } else {
        if exp >= 0 {
            let int = exp as usize + 1;
            w.digits(&sig[..int]);
            if p > int || alt {
                w.push(b'.');
            }
            w.digits(&sig[int..p]);
        } else {
            w.push(b'0');
            w.push(b'.');
            for _ in 0..(-exp - 1) {
                w.push(b'0');
            }
            w.digits(&sig[..p]);
        }
        if !alt {
            w.strip_zeros(from);
        }
    }
}

/// Formats `v` for conversion `conv` (f F e E g G) into `out` in `style`;
/// returns the length and whether the value is negative (the caller adds
/// the sign and padding).
pub fn format(
    v: f64,
    conv: u8,
    precision: Option<usize>,
    alt: bool,
    out: &mut [u8; MAX_OUT],
    style: Style,
) -> (usize, bool) {
    let negative = v.is_sign_negative() && !v.is_nan();
    let upper = conv.is_ascii_uppercase();
    let mut w = Writer { out, n: 0, style };
    if !v.is_finite() {
        let s: &[u8] = match (v.is_nan(), upper) {
            (true, false) => b"nan",
            (true, true) => b"NAN",
            (false, false) => b"inf",
            (false, true) => b"INF",
        };
        for &c in s {
            w.push(c);
        }
        return (w.n, negative);
    }
    let v = v.abs();
    let p = precision.unwrap_or(6).min(MAX_PRECISION);
    match conv.to_ascii_lowercase() {
        b'e' => scientific(&mut w, v, p, alt, upper),
        b'g' => general(&mut w, v, p, alt, upper),
        _ => fixed(&mut w, v, p, alt),
    }
    (w.n, negative)
}

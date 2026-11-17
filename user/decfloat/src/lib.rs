//! The exact decimal expansion of a double, and the rounding Windows'
//! conversions do on it: shared by the C runtime's printf (user/msvcrt,
//! the %f %e %g of both styles) and by Automation's number-to-string
//! (user/oleaut32's VarBstrFromR4 and VarBstrFromR8), which on Windows
//! is the UCRT's conversion as well.
//!
//! The digits come out of a small bignum, so they are the double's own
//! decimal expansion and a tie is still a tie: scaling in floating point
//! instead loses exactly those (12345675 to seven digits is
//! 1.234568E+07, and dividing by a power of ten first makes the half
//! disappear). One expansion for both callers, so both round alike.
#![no_std]

/// Bignum words: 1152 bits, enough for 2^1024 and for a fraction of up to
/// 1074 bits times 10.
const WORDS: usize = 36;
/// The most digits an expansion holds: 309 integer digits, or up to 323
/// leading fraction zeros plus the precision and a rounding digit.
pub const MAX_DIGITS: usize = 1000;
/// Significant digits msvcrt's conversion produces.
pub const SIGNIFICANT: usize = 17;
pub const MAX_PRECISION: usize = 340;

struct Big {
    w: [u32; WORDS],
}

impl Big {
    fn from_u64(v: u64) -> Self {
        let mut w = [0u32; WORDS];
        w[0] = v as u32;
        w[1] = (v >> 32) as u32;
        Self { w }
    }

    fn is_zero(&self) -> bool {
        self.w.iter().all(|&x| x == 0)
    }

    fn shl(&mut self, bits: usize) {
        let (words, bits) = (bits / 32, bits % 32);
        for i in (0..WORDS).rev() {
            let lo = i.checked_sub(words).map_or(0, |j| self.w[j]);
            let below = i.checked_sub(words + 1).map_or(0, |j| self.w[j]);
            self.w[i] = if bits == 0 { lo } else { (lo << bits) | (below >> (32 - bits)) };
        }
    }

    /// Divides by 10 in place; the remainder.
    fn divmod10(&mut self) -> u32 {
        let mut rem = 0u64;
        for i in (0..WORDS).rev() {
            let cur = (rem << 32) | u64::from(self.w[i]);
            self.w[i] = (cur / 10) as u32;
            rem = cur % 10;
        }
        rem as u32
    }

    fn mul10(&mut self) {
        let mut carry = 0u64;
        for x in &mut self.w {
            let cur = u64::from(*x) * 10 + carry;
            *x = cur as u32;
            carry = cur >> 32;
        }
    }

    /// The bits at and above `s` (a small number), which are then cleared.
    fn take_above(&mut self, s: usize) -> u32 {
        let (word, bit) = (s / 32, s % 32);
        let lo = u64::from(self.w[word]);
        let hi = self.w.get(word + 1).map_or(0, |&x| u64::from(x));
        let digit = (((hi << 32) | lo) >> bit) as u32;
        self.w[word] &= if bit == 0 { 0 } else { (1u32 << bit) - 1 };
        for x in &mut self.w[word + 1..] {
            *x = 0;
        }
        digit
    }
}

/// The exact decimal digits (values 0-9) of |v| (finite): the integer
/// digits without leading zeros (none if the integer part is 0), then
/// `frac` fraction digits (truncated). Returns (total, integer digits).
#[inline(never)]
fn expand(v: f64, frac: usize, d: &mut [u8; MAX_DIGITS]) -> (usize, usize) {
    let bits = v.to_bits();
    let exp_bits = ((bits >> 52) & 0x7ff) as i32;
    let fraction = bits & ((1 << 52) - 1);
    let (m, e) =
        if exp_bits == 0 { (fraction, -1074) } else { (fraction | 1 << 52, exp_bits - 1075) };
    let (mut int, mut frac_part, s) = if e >= 0 {
        let mut i = Big::from_u64(m);
        i.shl(e as usize);
        (i, Big::from_u64(0), 0usize)
    } else {
        let s = (-e) as usize;
        let int = if s >= 64 { 0 } else { m >> s };
        let rest = if s >= 64 { m } else { m & ((1u64 << s) - 1) };
        (Big::from_u64(int), Big::from_u64(rest), s)
    };
    let mut n = 0;
    while !int.is_zero() && n < MAX_DIGITS {
        d[n] = int.divmod10() as u8;
        n += 1;
    }
    d[..n].reverse();
    let int_digits = n;
    for _ in 0..frac.min(MAX_DIGITS - n) {
        d[n] = if s == 0 || frac_part.is_zero() {
            0
        } else {
            frac_part.mul10();
            frac_part.take_above(s) as u8
        };
        n += 1;
    }
    (n, int_digits)
}

/// `expand`, then rounded to SIGNIFICANT digits with the rest zero, as
/// msvcrt converts (with `exact`, as the UCRT does: not rounded).
/// Returns (total, integer digits).
pub fn expand_limited(
    v: f64,
    frac: usize,
    d: &mut [u8; MAX_DIGITS],
    exact: bool,
) -> (usize, usize) {
    let (mut n, mut int_digits) = expand(v, frac, d);
    if exact {
        return (n, int_digits);
    }
    let Some(first) = d[..n].iter().position(|&x| x != 0) else { return (n, int_digits) };
    let cut = first + SIGNIFICANT;
    if n <= cut {
        return (n, int_digits);
    }
    let next = d[cut];
    d[cut..n].fill(0);
    if round(&mut d[first..cut], next) {
        if first > 0 {
            // 0.0999.. -> 0.1000..: the zero before becomes a one.
            d[first - 1] = 1;
        } else {
            // 999.. -> 1000..: one more integer digit.
            d.copy_within(0..n.min(MAX_DIGITS - 1), 1);
            d[0] = 1;
            int_digits += 1;
            n = (n + 1).min(MAX_DIGITS);
        }
    }
    (n, int_digits)
}

/// Rounds digits `d[..keep]` up if `next` is 5 or more (half away from
/// zero); true if a carry ran out of the first digit (all became 0).
pub fn round(d: &mut [u8], next: u8) -> bool {
    if next < 5 {
        return false;
    }
    for x in d.iter_mut().rev() {
        if *x == 9 {
            *x = 0;
        } else {
            *x += 1;
            return false;
        }
    }
    true
}

/// `count` significant digits of |v| rounded, and the decimal exponent of
/// the first (0 for zero).
pub fn significant(
    v: f64,
    count: usize,
    sig: &mut [u8; MAX_PRECISION + 1],
    rounding: Rounding,
) -> i32 {
    let exact = rounding.exact;
    let mut d = [0u8; MAX_DIGITS];
    if v == 0.0 {
        sig[..count].fill(0);
        return 0;
    }
    // Enough fraction digits for the leading zeros of the smallest
    // doubles (323) and `count` + 1 more.
    let (n, int_digits) = expand_limited(v, 324 + count + 1, &mut d, exact);
    let first = d[..n].iter().position(|&x| x != 0).unwrap_or(0);
    let mut exp = int_digits as i32 - first as i32 - 1;
    for (i, s) in sig[..count].iter_mut().enumerate() {
        *s = d.get(first + i).copied().unwrap_or(0);
    }
    let next = d.get(first + count).copied().unwrap_or(0);
    let tail_zero = d.get(first + count + 1..).is_none_or(|t| t.iter().all(|&x| x == 0));
    if !keeps_even(rounding, &sig[..count], next, tail_zero) && round(&mut sig[..count], next) {
        sig[0] = 1;
        exp += 1;
    }
    exp
}

/// How a conversion rounds the expansion it has.
#[derive(Clone, Copy)]
pub struct Rounding {
    /// Exact digits (the UCRT, and Automation) instead of msvcrt's 17
    /// significant ones.
    pub exact: bool,
    /// An exact tie (a 5 and nothing after it) rounds to the even digit
    /// (the UCRT's _CRT_INTERNAL_PRINTF_STANDARD_ROUNDING) instead of
    /// away from zero.
    pub ties_even: bool,
}

/// Whether rounding `kept` with the next digit `next` (and only zeros
/// after it when `tail_zero`) keeps it as it is because it is an exact
/// tie and the last kept digit is even (`rounding.ties_even`).
pub fn keeps_even(rounding: Rounding, kept: &[u8], next: u8, tail_zero: bool) -> bool {
    rounding.ties_even && next == 5 && tail_zero && kept.last().is_none_or(|&x| x % 2 == 0)
}

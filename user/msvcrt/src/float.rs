//! printf's floating-point conversions, %f %e %g (and upper case), as
//! msvcrt.dll prints them: the decimal expansion of the double (exact,
//! from a small bignum) rounded to 17 significant digits, the digits
//! after those printed as zeros (msvcrt's conversion keeps 17: `%.20f` of
//! 0.1 is `0.10000000000000001000`), then rounded half away from zero at
//! the last digit asked for; exponents with a sign and at least three
//! digits (`1e+100`, `2.5e-005`). Precision is capped at `MAX_PRECISION`.

/// Bignum words: 1152 bits, enough for 2^1024 and for a fraction of up to
/// 1074 bits times 10.
const WORDS: usize = 36;
/// The most digits an expansion holds: 309 integer digits, or up to 323
/// leading fraction zeros plus the precision and a rounding digit.
const MAX_DIGITS: usize = 1000;
/// Significant digits msvcrt's conversion produces.
const SIGNIFICANT: usize = 17;
pub const MAX_PRECISION: usize = 340;
/// The longest result: 309 integer digits, a point, the precision.
pub const MAX_OUT: usize = 312 + MAX_PRECISION;

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
/// msvcrt converts. Returns (total, integer digits).
fn expand_limited(v: f64, frac: usize, d: &mut [u8; MAX_DIGITS]) -> (usize, usize) {
    let (mut n, mut int_digits) = expand(v, frac, d);
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
fn round(d: &mut [u8], next: u8) -> bool {
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
fn significant(v: f64, count: usize, sig: &mut [u8; MAX_PRECISION + 1]) -> i32 {
    let mut d = [0u8; MAX_DIGITS];
    if v == 0.0 {
        sig[..count].fill(0);
        return 0;
    }
    // Enough fraction digits for the leading zeros of the smallest
    // doubles (323) and `count` + 1 more.
    let (n, int_digits) = expand_limited(v, 324 + count + 1, &mut d);
    let first = d[..n].iter().position(|&x| x != 0).unwrap_or(0);
    let mut exp = int_digits as i32 - first as i32 - 1;
    for (i, s) in sig[..count].iter_mut().enumerate() {
        *s = d.get(first + i).copied().unwrap_or(0);
    }
    let next = d.get(first + count).copied().unwrap_or(0);
    if round(&mut sig[..count], next) {
        sig[0] = 1;
        exp += 1;
    }
    exp
}

struct Writer<'a> {
    out: &'a mut [u8; MAX_OUT],
    n: usize,
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
        for x in digits {
            self.push(b'0' + x as u8);
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
    let (n, mut int_digits) = expand_limited(v, (p + 1).max(324 + SIGNIFICANT + 1), &mut d);
    let (mut start, mut keep) = (0, int_digits + p);
    if int_digits == 0 {
        // A leading "0" for the integer part (room: expand starts at 0).
        d.copy_within(0..n.min(MAX_DIGITS - 1), 1);
        d[0] = 0;
        int_digits = 1;
        keep += 1;
    }
    let next = if keep < MAX_DIGITS { d[keep] } else { 0 };
    if round(&mut d[..keep], next) {
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
    let exp = significant(v, p + 1, &mut sig);
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
    let exp = significant(v, p, &mut sig);
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

/// Formats `v` for conversion `conv` (f F e E g G) into `out`; returns
/// the length and whether the value is negative (the caller adds the
/// sign and padding).
pub fn format(
    v: f64,
    conv: u8,
    precision: Option<usize>,
    alt: bool,
    out: &mut [u8; MAX_OUT],
) -> (usize, bool) {
    let negative = v.is_sign_negative() && !v.is_nan();
    let upper = conv.is_ascii_uppercase();
    let mut w = Writer { out, n: 0 };
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

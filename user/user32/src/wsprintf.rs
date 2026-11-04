//! wsprintfW and wvsprintfW: user32's small printf, as documented (the
//! format `%[-][#][0][width][.precision][size]type`: types c, C, d, i,
//! u, x, X, s, S and p; size h, l, w, I (pointer-sized), I64, I32; `%%`; at most 1024
//! characters written, NUL included, and the count without the NUL
//! returned). NSIS installers format their numbers and paths with it.
//!
//! The arguments come one 8-byte slot each, as x64 passes variadic
//! arguments: wvsprintfW's `va_list` is a pointer to them; wsprintfW
//! takes up to sixteen (a C variadic function cannot be written in stable
//! Rust; on x64 the caller's slots are those of a function with that many
//! fixed arguments, and slots it did not fill are never read unless the
//! format asks for them).

const LIMIT: usize = 1024;

/// The next argument slot.
trait Args {
    fn next(&mut self) -> u64;
}

struct Slots<'a>(&'a [usize], usize);

impl Args for Slots<'_> {
    fn next(&mut self) -> u64 {
        let v = self.0.get(self.1).copied().unwrap_or(0);
        self.1 += 1;
        v as u64
    }
}

struct VaList(*const u64);

impl Args for VaList {
    fn next(&mut self) -> u64 {
        if self.0.is_null() {
            return 0;
        }
        // SAFETY: the caller's va_list: one 8-byte slot per argument the
        // format names.
        let v = unsafe { *self.0 };
        self.0 = self.0.wrapping_add(1);
        v
    }
}

struct Out {
    buf: *mut u16,
    n: usize,
}

impl Out {
    fn push(&mut self, u: u16) {
        if self.n < LIMIT - 1 {
            // SAFETY: the caller's buffer of LIMIT characters (the
            // documented maximum).
            unsafe { *self.buf.add(self.n) = u };
            self.n += 1;
        }
    }
}

fn format(buf: *mut u16, fmt: *const u16, args: &mut dyn Args) -> i32 {
    if buf.is_null() || fmt.is_null() {
        return 0;
    }
    let mut out = Out { buf, n: 0 };
    let at = |i: usize| -> u16 {
        // SAFETY: the caller's NUL-terminated format, read up to its NUL.
        unsafe { *fmt.add(i) }
    };
    let mut i = 0;
    while at(i) != 0 {
        let c = at(i);
        i += 1;
        if c != u16::from(b'%') {
            out.push(c);
            continue;
        }
        let (mut left, mut alt, mut zero) = (false, false, false);
        loop {
            match at(i) {
                0x2d => left = true, // '-'
                0x23 => alt = true,  // '#'
                0x30 => zero = true, // '0'
                _ => break,
            }
            i += 1;
        }
        let mut width = 0usize;
        while (0x30..=0x39).contains(&at(i)) {
            width = width * 10 + usize::from(at(i) - 0x30);
            i += 1;
        }
        let mut precision: Option<usize> = None;
        if at(i) == u16::from(b'.') {
            i += 1;
            let mut p = 0;
            while (0x30..=0x39).contains(&at(i)) {
                p = p * 10 + usize::from(at(i) - 0x30);
                i += 1;
            }
            precision = Some(p);
        }
        // Size: h (narrow strings/chars), l and w (wide), I64, I32.
        let (mut wide64, mut narrow, mut wide) = (false, false, false);
        loop {
            match at(i) {
                0x68 => narrow = true,      // 'h'
                0x6c | 0x77 => wide = true, // 'l', 'w'
                0x49 if at(i + 1) == u16::from(b'6') && at(i + 2) == u16::from(b'4') => {
                    wide64 = true;
                    i += 2;
                }
                0x49 if at(i + 1) == u16::from(b'3') && at(i + 2) == u16::from(b'2') => i += 2,
                // I alone: pointer-sized (64 bits on x64; NSIS prints its
                // handles with %Id).
                0x49 => wide64 = true,
                _ => break,
            }
            i += 1;
        }
        let ty = at(i);
        if ty == 0 {
            break;
        }
        i += 1;
        let mut text = [0u16; 66];
        let mut len = 0;
        let mut prefix: &[u8] = b"";
        let mut numeric = true;
        let mut long_text: Option<(*const u8, bool)> = None;
        match ty {
            0x25 => {
                out.push(ty);
                continue;
            }
            0x63 | 0x43 => {
                // c: a WCHAR (C or hc: a char).
                numeric = false;
                let v = args.next();
                let as_char = (ty == 0x43 && !wide) || narrow;
                text[0] = if as_char { u16::from(v as u8) } else { v as u16 };
                len = 1;
            }
            0x73 | 0x53 => {
                // s: a wide string (S or hs: an ANSI one); NULL is "(null)".
                numeric = false;
                let p = args.next() as usize as *const u8;
                let ansi = (ty == 0x53 && !wide) || narrow;
                long_text = Some((p, ansi));
            }
            0x64 | 0x69 | 0x75 | 0x78 | 0x58 | 0x70 => {
                let raw = args.next();
                let (signed, base) = match ty {
                    0x64 | 0x69 => (true, 10),
                    0x75 => (false, 10),
                    _ => (false, 16),
                };
                let bits64 = wide64 || ty == 0x70;
                let (neg, mut v) = if signed {
                    let s = if bits64 { raw as i64 } else { i64::from(raw as u32 as i32) };
                    (s < 0, s.unsigned_abs())
                } else if bits64 {
                    (false, raw)
                } else {
                    (false, u64::from(raw as u32))
                };
                let upper = ty == 0x58 || ty == 0x70;
                let mut digits = [0u8; 20];
                let mut d = 0;
                loop {
                    let r = (v % base) as u8;
                    digits[d] = if r < 10 {
                        b'0' + r
                    } else if upper {
                        b'A' + r - 10
                    } else {
                        b'a' + r - 10
                    };
                    d += 1;
                    v /= base;
                    if v == 0 {
                        break;
                    }
                }
                let min_digits = if ty == 0x70 { 16 } else { precision.unwrap_or(1) };
                while d < min_digits && d < digits.len() {
                    digits[d] = b'0';
                    d += 1;
                }
                if neg {
                    prefix = b"-";
                } else if alt && base == 16 && raw != 0 && ty != 0x70 {
                    prefix = if upper { b"0X" } else { b"0x" };
                }
                for k in (0..d).rev() {
                    text[len] = u16::from(digits[k]);
                    len += 1;
                }
            }
            other => {
                // Not a type: written as it is (the % dropped).
                out.push(other);
                continue;
            }
        }
        // The text of a string argument, cut to the precision.
        let mut s_len = len;
        if let Some((p, ansi)) = long_text {
            s_len = string_len(p, ansi, precision);
        }
        let body = prefix.len() + s_len;
        let pad = width.saturating_sub(body);
        let fill = if zero && !left && numeric { u16::from(b'0') } else { u16::from(b' ') };
        if !left && fill == u16::from(b' ') {
            for _ in 0..pad {
                out.push(fill);
            }
        }
        for &b in prefix {
            out.push(u16::from(b));
        }
        if !left && fill == u16::from(b'0') {
            for _ in 0..pad {
                out.push(fill);
            }
        }
        match long_text {
            Some((p, ansi)) => {
                for k in 0..s_len {
                    out.push(string_at(p, ansi, k));
                }
            }
            None => {
                for &u in &text[..len] {
                    out.push(u);
                }
            }
        }
        if left {
            for _ in 0..pad {
                out.push(u16::from(b' '));
            }
        }
    }
    // SAFETY: the caller's buffer; `out.n` < LIMIT.
    unsafe { *buf.add(out.n) = 0 };
    out.n as i32
}

const NULL_TEXT: &[u8] = b"(null)";

/// The `k`-th character of a string argument (ANSI ones taken as Latin-1
/// bytes: gala's ANSI code page is UTF-8, and wsprintf copies bytes).
fn string_at(p: *const u8, ansi: bool, k: usize) -> u16 {
    if p.is_null() {
        return u16::from(NULL_TEXT[k]);
    }
    // SAFETY: the caller's NUL-terminated string, read below its length.
    unsafe { if ansi { u16::from(*p.add(k)) } else { *p.cast::<u16>().add(k) } }
}

fn string_len(p: *const u8, ansi: bool, precision: Option<usize>) -> usize {
    let max = precision.unwrap_or(LIMIT);
    if p.is_null() {
        return NULL_TEXT.len().min(max);
    }
    let mut n = 0;
    while n < max && string_at(p, ansi, n) != 0 {
        n += 1;
    }
    n
}

/// wvsprintfW(buf, format, va_list).
#[unsafe(no_mangle)]
pub extern "system" fn wvsprintfW(buf: *mut u16, fmt: *const u16, args: *const u64) -> i32 {
    format(buf, fmt, &mut VaList(args))
}

/// wsprintfW(buf, format, ...): up to sixteen arguments (see the module
/// comment).
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "C" fn wsprintfW(
    buf: *mut u16,
    fmt: *const u16,
    a0: usize,
    a1: usize,
    a2: usize,
    a3: usize,
    a4: usize,
    a5: usize,
    a6: usize,
    a7: usize,
    a8: usize,
    a9: usize,
    a10: usize,
    a11: usize,
    a12: usize,
    a13: usize,
    a14: usize,
    a15: usize,
) -> i32 {
    let slots = [a0, a1, a2, a3, a4, a5, a6, a7, a8, a9, a10, a11, a12, a13, a14, a15];
    format(buf, fmt, &mut Slots(&slots, 0))
}

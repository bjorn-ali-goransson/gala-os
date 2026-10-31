//! sscanf: conversions %d %i %u %x %X %o %p %c %s %[...] %n %f %e %g %%,
//! with `*` (no assignment), a width, and the sizes hh h l ll I64 L z j.
//! Whitespace in the format matches any amount of input whitespace; other
//! characters match themselves. Returns the number of assignments, or -1
//! if the input ends before the first conversion.
//!
//! sscanf is C-variadic: the assembly at the end passes its arguments as a
//! Win64 `va_list` to vsscanf, as stdio.rs does for printf.

/// The argument pointers, one per assigning conversion.
struct Ptrs(*const u64);

impl Ptrs {
    fn next<T>(&mut self) -> *mut T {
        // SAFETY: the caller passed a pointer for each conversion.
        unsafe {
            let p = *self.0 as *mut T;
            self.0 = self.0.add(1);
            p
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Size {
    Char,
    Short,
    Int,
    Long64,
}

fn is_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}

/// Parses an integer in `base` (0: by prefix) from `s`, at most `width`
/// bytes; (value, bytes used).
fn integer(s: &[u8], base: u32, width: usize) -> Option<(i128, usize)> {
    let s = &s[..s.len().min(width)];
    let mut i = 0;
    let negative = match s.first() {
        Some(b'-') => {
            i = 1;
            true
        }
        Some(b'+') => {
            i = 1;
            false
        }
        _ => false,
    };
    let mut base = base;
    let has_0x = s.get(i) == Some(&b'0') && matches!(s.get(i + 1), Some(b'x' | b'X'));
    if (base == 16 || base == 0) && has_0x && s.get(i + 2).is_some_and(|c| c.is_ascii_hexdigit()) {
        i += 2;
        base = 16;
    } else if base == 0 {
        base = if s.get(i) == Some(&b'0') { 8 } else { 10 };
    }
    let start = i;
    let mut v: i128 = 0;
    while let Some(d) = s.get(i).and_then(|&c| (c as char).to_digit(base)) {
        v = v.saturating_mul(i128::from(base)).saturating_add(i128::from(d));
        i += 1;
    }
    if i == start {
        return None;
    }
    Some((if negative { -v } else { v }, i))
}

/// Parses a decimal floating-point number (sign, digits with an optional
/// point, optional exponent) from at most `width` bytes; (value, bytes
/// used). The value is correctly rounded (core's parser).
pub fn float(s: &[u8], width: usize) -> Option<(f64, usize)> {
    let s = &s[..s.len().min(width)];
    let digits_from = |mut i: usize| {
        while s.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        i
    };
    let mut i = usize::from(matches!(s.first(), Some(b'-' | b'+')));
    let int_end = digits_from(i);
    let mut seen = int_end > i;
    i = int_end;
    if s.get(i) == Some(&b'.') {
        let frac_end = digits_from(i + 1);
        seen |= frac_end > i + 1;
        i = frac_end;
    }
    if !seen {
        return None;
    }
    // An exponent only counts with at least one digit.
    if matches!(s.get(i), Some(b'e' | b'E')) {
        let sign = usize::from(matches!(s.get(i + 1), Some(b'-' | b'+')));
        let exp_end = digits_from(i + 1 + sign);
        if exp_end > i + 1 + sign {
            i = exp_end;
        }
    }
    let text = core::str::from_utf8(&s[..i]).ok()?;
    // core rejects a trailing point ("5."): C accepts it.
    let text = text.strip_suffix('.').unwrap_or(text);
    let v: f64 = text.parse().ok()?;
    Some((v, i))
}

/// vsscanf(input, format, args).
#[unsafe(no_mangle)]
pub extern "C" fn vsscanf(input: *const u8, format: *const u8, args: *const u64) -> i32 {
    // SAFETY: the caller's NUL-terminated strings.
    let (s, f) = unsafe { (crate::c_str(input), crate::c_str(format)) };
    let mut ptrs = Ptrs(args);
    let (mut i, mut k) = (0usize, 0usize);
    let mut assigned = 0;
    let mut converted_any = false;
    while k < f.len() {
        let c = f[k];
        if is_space(c) {
            while i < s.len() && is_space(s[i]) {
                i += 1;
            }
            k += 1;
            continue;
        }
        if c != b'%' || f.get(k + 1) == Some(&b'%') {
            let want = if c == b'%' {
                k += 1;
                b'%'
            } else {
                c
            };
            if c == b'%' {
                while i < s.len() && is_space(s[i]) {
                    i += 1;
                }
            }
            if s.get(i) != Some(&want) {
                break;
            }
            i += 1;
            k += 1;
            continue;
        }
        k += 1;
        let suppress = f.get(k) == Some(&b'*');
        if suppress {
            k += 1;
        }
        let mut width = 0usize;
        while let Some(d) = f.get(k).and_then(|&c| (c as char).to_digit(10)) {
            width = width * 10 + d as usize;
            k += 1;
        }
        let width = if width == 0 { usize::MAX } else { width };
        let mut size = Size::Int;
        let mut long_float = false;
        loop {
            match f.get(k) {
                Some(b'h') => {
                    size = if size == Size::Short { Size::Char } else { Size::Short };
                }
                Some(b'l') => {
                    // Windows: long is 32 bits; ll is 64.
                    if long_float {
                        size = Size::Long64;
                    }
                    long_float = true;
                }
                Some(b'L' | b'z' | b'j') => size = Size::Long64,
                Some(b'I') if f.get(k + 1..k + 3) == Some(b"64") => {
                    size = Size::Long64;
                    k += 2;
                }
                _ => break,
            }
            k += 1;
        }
        let Some(&conv) = f.get(k) else { break };
        k += 1;
        if !matches!(conv, b'c' | b'[' | b'n') {
            while i < s.len() && is_space(s[i]) {
                i += 1;
            }
        }
        if i >= s.len() && conv != b'n' {
            if !converted_any {
                return -1;
            }
            break;
        }
        match conv {
            b'd' | b'i' | b'u' | b'x' | b'X' | b'o' | b'p' => {
                if conv == b'p' {
                    // A pointer: 64-bit hex (what printf's %p writes).
                    size = Size::Long64;
                }
                let base = match conv {
                    b'd' | b'u' => 10,
                    b'i' => 0,
                    b'o' => 8,
                    _ => 16,
                };
                let Some((v, used)) = integer(&s[i..], base, width) else { break };
                i += used;
                if !suppress {
                    // SAFETY: the caller's pointer of the given size.
                    unsafe {
                        match size {
                            Size::Char => *ptrs.next::<u8>() = v as u8,
                            Size::Short => *ptrs.next::<u16>() = v as u16,
                            Size::Int => *ptrs.next::<u32>() = v as u32,
                            Size::Long64 => *ptrs.next::<u64>() = v as u64,
                        }
                    }
                    assigned += 1;
                }
            }
            b'e' | b'f' | b'g' | b'E' | b'G' | b'a' => {
                let Some((v, used)) = float(&s[i..], width) else { break };
                i += used;
                if !suppress {
                    // SAFETY: a float or (l, L) double pointer.
                    unsafe {
                        if long_float || size == Size::Long64 {
                            *ptrs.next::<f64>() = v;
                        } else {
                            *ptrs.next::<f32>() = v as f32;
                        }
                    }
                    assigned += 1;
                }
            }
            b'c' => {
                let n = if width == usize::MAX { 1 } else { width };
                if i + n > s.len() {
                    break;
                }
                if !suppress {
                    let p = ptrs.next::<u8>();
                    // SAFETY: the caller's buffer of at least `n` bytes.
                    unsafe { core::ptr::copy_nonoverlapping(s[i..].as_ptr(), p, n) };
                    assigned += 1;
                }
                i += n;
            }
            b's' | b'[' => {
                // %[set] / %[^set] (']' first is part of the set).
                let mut set: &[u8] = &[];
                let mut negate = false;
                if conv == b'[' {
                    negate = f.get(k) == Some(&b'^');
                    if negate {
                        k += 1;
                    }
                    let start = k;
                    if f.get(k) == Some(&b']') {
                        k += 1;
                    }
                    while k < f.len() && f[k] != b']' {
                        k += 1;
                    }
                    set = &f[start..k.min(f.len())];
                    k += 1;
                }
                let in_set = |c: u8| {
                    if conv == b's' {
                        return !is_space(c);
                    }
                    let mut hit = false;
                    let mut j = 0;
                    while j < set.len() {
                        if j + 2 < set.len() && set[j + 1] == b'-' {
                            hit |= (set[j]..=set[j + 2]).contains(&c);
                            j += 3;
                        } else {
                            hit |= set[j] == c;
                            j += 1;
                        }
                    }
                    hit != negate
                };
                let start = i;
                while i < s.len() && i - start < width && in_set(s[i]) {
                    i += 1;
                }
                if i == start {
                    break;
                }
                if !suppress {
                    let p = ptrs.next::<u8>();
                    // SAFETY: the caller's buffer, big enough by contract.
                    unsafe {
                        core::ptr::copy_nonoverlapping(s[start..].as_ptr(), p, i - start);
                        *p.add(i - start) = 0;
                    }
                    assigned += 1;
                }
            }
            b'n' => {
                if !suppress {
                    // SAFETY: the caller's int pointer.
                    unsafe { *ptrs.next::<i32>() = i as i32 };
                }
            }
            _ => break,
        }
        converted_any = true;
    }
    assigned
}

core::arch::global_asm!(
    ".globl sscanf",
    "sscanf:",
    "mov [rsp+16], rdx",
    "mov [rsp+24], r8",
    "mov [rsp+32], r9",
    "lea r8, [rsp+24]",
    "sub rsp, 40",
    "call {vsscanf}",
    "add rsp, 40",
    "ret",
    vsscanf = sym vsscanf,
);

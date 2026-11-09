//! sscanf: conversions %d %i %u %x %X %o %p %c %s %[...] %n %f %e %g %%,
//! with `*` (no assignment), a width, and the sizes hh h l ll I64 L z j.
//! Whitespace in the format matches any amount of input whitespace; other
//! characters match themselves. Returns the number of assignments, or -1
//! if the input ends before the first conversion.
//!
//! sscanf is C-variadic: the architecture's entry point (`x64.rs`,
//! `x86.rs`) passes its arguments as a `va_list` to vsscanf, as for
//! printf. The arguments are pointers, one per assigning conversion.
//!
//! The UCRT's `__stdio_common_vsscanf` and `vswscanf` (ucrt/stdio.rs)
//! use `scan` with a `ScanStyle`: the secure form (`sscanf_s`: a buffer
//! size after each %s, %c and %[ pointer) and strings stored as UTF-16
//! (the wide functions' %s, and %ls / %S); a wide input is scanned as
//! UTF-8.

use crate::va::VaList;

#[derive(Clone, Copy, PartialEq)]
enum Size {
    Char,
    Short,
    Int,
    /// Pointer-sized: `z` and `%p` (32 bits on x86, 64 on x64).
    Pointer,
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
pub extern "C" fn vsscanf(input: *const u8, format: *const u8, args: VaList) -> i32 {
    // SAFETY: the caller's NUL-terminated strings.
    let (s, f) = unsafe { (crate::c_str(input), crate::c_str(format)) };
    scan(s, f, args, ScanStyle { secure: false, wide: false })
}

/// How `scan` stores strings.
#[derive(Clone, Copy)]
pub(crate) struct ScanStyle {
    /// sscanf_s: each %s, %c and %[ pointer is followed by its buffer's
    /// size (an unsigned int, in characters).
    pub(crate) secure: bool,
    /// %s, %c and %[ store UTF-16 unless `h` (a wide function's rule);
    /// `l` and %S store UTF-16 in either.
    pub(crate) wide: bool,
}

/// Stores `bytes` (UTF-8) at `p` as a string: UTF-16 when `wide`, and NUL-
/// terminated unless `chars` (%c). False if `size` (characters, secure
/// form) is too small: then an empty string is stored if there is room.
fn store(p: *mut u8, bytes: &[u8], wide: bool, chars: bool, size: Option<usize>) -> bool {
    let terminator = usize::from(!chars);
    if wide {
        let text = core::str::from_utf8(bytes).unwrap_or("");
        let units = text.encode_utf16().count();
        if size.is_some_and(|n| units + terminator > n) {
            if size.is_some_and(|n| n > 0) {
                // SAFETY: room for one character.
                unsafe { *p.cast::<u16>() = 0 };
            }
            return false;
        }
        let q = p.cast::<u16>();
        for (i, u) in text.encode_utf16().enumerate() {
            // SAFETY: the caller's buffer of at least `units` characters.
            unsafe { q.add(i).write_unaligned(u) };
        }
        if !chars {
            // SAFETY: room for the terminator (by contract or `size`).
            unsafe { q.add(units).write_unaligned(0) };
        }
        return true;
    }
    if size.is_some_and(|n| bytes.len() + terminator > n) {
        if size.is_some_and(|n| n > 0) {
            // SAFETY: room for one byte.
            unsafe { *p = 0 };
        }
        return false;
    }
    // SAFETY: the caller's buffer, big enough by contract or `size`.
    unsafe {
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
        if !chars {
            *p.add(bytes.len()) = 0;
        }
    }
    true
}

/// Scans `s` with the format `f`, storing through `args` in `style`.
pub(crate) fn scan(s: &[u8], f: &[u8], args: VaList, style: ScanStyle) -> i32 {
    let mut ptrs = args;
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
        let (mut narrow_str, mut wide_str) = (false, false);
        loop {
            match f.get(k) {
                Some(b'h') => {
                    size = if size == Size::Short { Size::Char } else { Size::Short };
                    narrow_str = true;
                }
                Some(b'w') => wide_str = true,
                Some(b'l') => {
                    // Windows: long is 32 bits; ll is 64.
                    if long_float {
                        size = Size::Long64;
                    }
                    long_float = true;
                    wide_str = true;
                }
                Some(b'L' | b'j') => size = Size::Long64,
                Some(b'z') => size = Size::Pointer,
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
        // Where a string conversion stores UTF-16, and its secure size.
        let utf16 = match conv {
            b'S' | b'C' => !narrow_str,
            _ => wide_str || (style.wide && !narrow_str),
        };
        let conv = match conv {
            b'S' => b's',
            b'C' => b'c',
            other => other,
        };
        let secure_size = |ptrs: &mut VaList| -> Option<usize> {
            if style.secure && !suppress { Some(ptrs.int() as usize) } else { None }
        };
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
                    // A pointer, in hex (what printf's %p writes).
                    size = Size::Pointer;
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
                            Size::Char => *ptrs.ptr::<u8>() = v as u8,
                            Size::Short => *ptrs.ptr::<u16>() = v as u16,
                            Size::Int => *ptrs.ptr::<u32>() = v as u32,
                            Size::Pointer => *ptrs.ptr::<usize>() = v as usize,
                            Size::Long64 => *ptrs.ptr::<u64>() = v as u64,
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
                            *ptrs.ptr::<f64>() = v;
                        } else {
                            *ptrs.ptr::<f32>() = v as f32;
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
                    let p = ptrs.ptr::<u8>();
                    let size = secure_size(&mut ptrs);
                    if !store(p, &s[i..i + n], utf16, true, size) {
                        break;
                    }
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
                    let p = ptrs.ptr::<u8>();
                    let size = secure_size(&mut ptrs);
                    if !store(p, &s[start..i], utf16, false, size) {
                        break;
                    }
                    assigned += 1;
                }
            }
            b'n' => {
                if !suppress {
                    // SAFETY: the caller's int pointer.
                    unsafe { *ptrs.ptr::<i32>() = i as i32 };
                }
            }
            _ => break,
        }
        converted_any = true;
    }
    assigned
}

//! stdio on the console handles: the three FILE objects of `__iob_func`
//! (stdin, stdout, stderr; unbuffered), fwrite/fputs, and the printf
//! family with its formatting engine.
//!
//! fprintf and printf are C-variadic, which stable Rust cannot define: the
//! assembly below stores the register arguments in the caller's home area
//! (which, with the stack arguments after it, is the Win64 `va_list`) and
//! calls the `v` variant.

use core::ffi::c_void;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> *mut c_void;
    fn WriteFile(
        h: *mut c_void,
        buf: *const u8,
        len: u32,
        written: *mut u32,
        ov: *mut c_void,
    ) -> i32;
}

/// msvcrt's FILE is 48 bytes on x64; only the address matters here.
const FILE_SIZE: usize = 48;
static mut IOB: [[u8; FILE_SIZE]; 3] = [[0; FILE_SIZE]; 3];

/// __iob_func(): the stdin, stdout and stderr FILE objects.
#[unsafe(no_mangle)]
pub extern "C" fn __iob_func() -> *mut u8 {
    (&raw mut IOB).cast()
}

/// The console handle of one of our FILEs (None for other pointers).
fn handle_of(file: *mut u8) -> Option<*mut c_void> {
    let base = (&raw mut IOB) as usize;
    let at = (file as usize).checked_sub(base)?;
    let which = match at / FILE_SIZE {
        0 => -10i32,
        1 => -11,
        2 => -12,
        _ => return None,
    };
    // SAFETY: plain call.
    Some(unsafe { GetStdHandle(which as u32) })
}

fn write_handle(h: *mut c_void, data: &[u8]) -> bool {
    let mut written = 0;
    // SAFETY: a valid buffer.
    unsafe {
        WriteFile(h, data.as_ptr(), data.len() as u32, &mut written, core::ptr::null_mut()) != 0
    }
}

pub(crate) fn write_stderr(data: &[u8]) {
    // SAFETY: plain call.
    write_handle(unsafe { GetStdHandle(-12i32 as u32) }, data);
}

#[unsafe(no_mangle)]
pub extern "C" fn fwrite(buf: *const u8, size: usize, count: usize, file: *mut u8) -> usize {
    let Some(h) = handle_of(file) else {
        crate::missing(b"msvcrt.dll!fwrite(file)", 0);
        return 0;
    };
    let total = size.saturating_mul(count);
    if total == 0 {
        return 0;
    }
    // SAFETY: the caller's buffer of size * count bytes.
    let data = unsafe { core::slice::from_raw_parts(buf, total) };
    if write_handle(h, data) { count } else { 0 }
}

#[unsafe(no_mangle)]
pub extern "C" fn fputs(s: *const u8, file: *mut u8) -> i32 {
    // SAFETY: the caller's NUL-terminated string.
    let s = unsafe { crate::c_str(s) };
    if fwrite(s.as_ptr(), 1, s.len(), file) == s.len() { 0 } else { -1 }
}

#[unsafe(no_mangle)]
pub extern "C" fn fflush(_file: *mut u8) -> i32 {
    0 // unbuffered
}

/// Where formatted output goes: a console handle, in 256-byte pieces.
struct Out {
    handle: *mut c_void,
    buf: [u8; 256],
    len: usize,
    total: usize,
}

impl Out {
    fn put(&mut self, mut data: &[u8]) {
        self.total += data.len();
        while !data.is_empty() {
            let n = data.len().min(self.buf.len() - self.len);
            self.buf[self.len..self.len + n].copy_from_slice(&data[..n]);
            self.len += n;
            data = &data[n..];
            if self.len == self.buf.len() {
                self.flush();
            }
        }
    }

    fn flush(&mut self) {
        if self.len > 0 {
            write_handle(self.handle, &self.buf[..self.len]);
            self.len = 0;
        }
    }
}

/// The variadic arguments: 8-byte slots (Win64 `va_list`).
struct Args(*const u64);

impl Args {
    fn next(&mut self) -> u64 {
        // SAFETY: the caller passed an argument for each conversion.
        unsafe {
            let v = *self.0;
            self.0 = self.0.add(1);
            v
        }
    }
}

/// Formats `fmt` with the C printf rules (flags `-+ #0`, width and
/// precision with `*`, lengths hh h l ll I I32 I64 z j t, conversions
/// d i u o x X c s p % and f/e/g as fixed point).
fn format(out: &mut Out, fmt: *const u8, mut args: Args) {
    // SAFETY: the caller's NUL-terminated format.
    let fmt = unsafe { crate::c_str(fmt) };
    let mut i = 0;
    while i < fmt.len() {
        if fmt[i] != b'%' {
            let start = i;
            while i < fmt.len() && fmt[i] != b'%' {
                i += 1;
            }
            out.put(&fmt[start..i]);
            continue;
        }
        i += 1;
        let (mut left, mut plus, mut space, mut alt, mut zero) =
            (false, false, false, false, false);
        while i < fmt.len() {
            match fmt[i] {
                b'-' => left = true,
                b'+' => plus = true,
                b' ' => space = true,
                b'#' => alt = true,
                b'0' => zero = true,
                _ => break,
            }
            i += 1;
        }
        let mut width = 0usize;
        if i < fmt.len() && fmt[i] == b'*' {
            let w = args.next() as i32;
            if w < 0 {
                left = true;
            }
            width = w.unsigned_abs() as usize;
            i += 1;
        }
        while i < fmt.len() && fmt[i].is_ascii_digit() {
            width = width * 10 + usize::from(fmt[i] - b'0');
            i += 1;
        }
        let mut precision = None;
        if i < fmt.len() && fmt[i] == b'.' {
            i += 1;
            let mut p = 0usize;
            if i < fmt.len() && fmt[i] == b'*' {
                let v = args.next() as i32;
                p = if v < 0 { 0 } else { v as usize };
                i += 1;
            }
            while i < fmt.len() && fmt[i].is_ascii_digit() {
                p = p * 10 + usize::from(fmt[i] - b'0');
                i += 1;
            }
            precision = Some(p);
        }
        // Length: 64-bit or not (`l` is 32-bit on Windows).
        let mut wide64 = false;
        loop {
            let rest = &fmt[i..];
            if rest.starts_with(b"I64") {
                wide64 = true;
                i += 3;
            } else if rest.starts_with(b"I32") {
                i += 3;
            } else if rest.starts_with(b"ll") || rest.starts_with(b"hh") {
                wide64 |= rest[0] == b'l';
                i += 2;
            } else if matches!(rest.first(), Some(b'I' | b'z' | b'j' | b't')) {
                wide64 = true;
                i += 1;
            } else if matches!(rest.first(), Some(b'h' | b'l' | b'L' | b'w')) {
                i += 1;
            } else {
                break;
            }
        }
        let Some(&conv) = fmt.get(i) else { break };
        i += 1;
        let mut digits = [0u8; 72];
        let (body, prefix): (&[u8], &[u8]) = match conv {
            b'd' | b'i' => {
                let raw = args.next();
                let v = if wide64 { raw as i64 } else { i64::from(raw as i32) };
                let s = number(&mut digits, v.unsigned_abs(), 10, false, precision);
                let sign: &[u8] = if v < 0 {
                    b"-"
                } else if plus {
                    b"+"
                } else if space {
                    b" "
                } else {
                    b""
                };
                (s, sign)
            }
            b'u' | b'o' | b'x' | b'X' => {
                let raw = args.next();
                let v = if wide64 { raw } else { u64::from(raw as u32) };
                let base = match conv {
                    b'o' => 8,
                    b'u' => 10,
                    _ => 16,
                };
                let s = number(&mut digits, v, base, conv == b'X', precision);
                let p: &[u8] = match conv {
                    b'x' if alt && v != 0 => b"0x",
                    b'X' if alt && v != 0 => b"0X",
                    b'o' if alt => b"0",
                    _ => b"",
                };
                (s, p)
            }
            b'p' => (number(&mut digits, args.next(), 16, true, Some(16)), b""),
            b'c' => {
                digits[0] = args.next() as u8;
                (&digits[..1], b"")
            }
            b's' => {
                let p = args.next() as *const u8;
                // SAFETY: the caller's NUL-terminated string (or NULL).
                let s: &[u8] = if p.is_null() { b"(null)" } else { unsafe { crate::c_str(p) } };
                (&s[..precision.map_or(s.len(), |n| n.min(s.len()))], b"")
            }
            b'f' | b'F' | b'e' | b'E' | b'g' | b'G' => {
                let v = f64::from_bits(args.next());
                let (s, negative) = fixed(&mut digits, v, precision.unwrap_or(6));
                (s, if negative { b"-" } else { b"" })
            }
            b'%' => (b"%".as_slice(), b""),
            b'n' => {
                // %n writes memory: not supported (as in the secure CRT).
                args.next();
                crate::missing(b"msvcrt.dll!printf(%n)", 0);
                continue;
            }
            other => {
                crate::missing(b"msvcrt.dll!printf(conversion)", u64::from(other));
                continue;
            }
        };
        let len = prefix.len() + body.len();
        let pad = width.saturating_sub(len);
        let zero_pad = zero && !left && precision.is_none() && conv != b's' && conv != b'c';
        if !left && !zero_pad {
            pad_with(out, b' ', pad);
        }
        out.put(prefix);
        if zero_pad {
            pad_with(out, b'0', pad);
        }
        out.put(body);
        if left {
            pad_with(out, b' ', pad);
        }
    }
}

fn pad_with(out: &mut Out, c: u8, n: usize) {
    for _ in 0..n {
        out.put(&[c]);
    }
}

/// Digits of `v` in `base`, at least `precision` of them.
fn number(
    buf: &mut [u8; 72],
    mut v: u64,
    base: u64,
    upper: bool,
    precision: Option<usize>,
) -> &[u8] {
    let set: &[u8; 16] = if upper { b"0123456789ABCDEF" } else { b"0123456789abcdef" };
    let mut at = buf.len();
    let min = precision.unwrap_or(1).min(64);
    while v > 0 || buf.len() - at < min {
        at -= 1;
        buf[at] = set[(v % base) as usize];
        v /= base;
    }
    &buf[at..]
}

/// `v` in fixed point with `precision` decimals (values up to 2^64; the
/// last digits may be off by rounding).
fn fixed(buf: &mut [u8; 72], v: f64, precision: usize) -> (&[u8], bool) {
    let precision = precision.min(18);
    let negative = v.is_sign_negative();
    let v = if negative { -v } else { v };
    if v.is_nan() || v.is_infinite() {
        let s: &[u8] = if v.is_nan() { b"nan" } else { b"inf" };
        buf[..s.len()].copy_from_slice(s);
        return (&buf[..s.len()], negative && !v.is_nan());
    }
    let mut scale = 1u64;
    for _ in 0..precision {
        scale *= 10;
    }
    let scaled = v * scale as f64 + 0.5;
    let whole = (scaled / scale as f64) as u64;
    let frac = (scaled as u64).wrapping_sub(whole.wrapping_mul(scale));
    let mut tmp = [0u8; 72];
    let int_part = number(&mut tmp, whole, 10, false, None);
    let mut n = int_part.len();
    buf[..n].copy_from_slice(int_part);
    if precision > 0 {
        buf[n] = b'.';
        n += 1;
        let mut tmp2 = [0u8; 72];
        let f = number(&mut tmp2, frac, 10, false, Some(precision));
        buf[n..n + f.len()].copy_from_slice(f);
        n += f.len();
    }
    (&buf[..n], negative)
}

#[unsafe(no_mangle)]
pub extern "C" fn vfprintf(file: *mut u8, fmt: *const u8, args: *const u64) -> i32 {
    let Some(handle) = handle_of(file) else {
        crate::missing(b"msvcrt.dll!vfprintf(file)", 0);
        return -1;
    };
    let mut out = Out { handle, buf: [0; 256], len: 0, total: 0 };
    format(&mut out, fmt, Args(args));
    out.flush();
    out.total as i32
}

#[unsafe(no_mangle)]
pub extern "C" fn vprintf(fmt: *const u8, args: *const u64) -> i32 {
    vfprintf(gala_stdout(), fmt, args)
}

/// stdout's FILE (for printf).
fn gala_stdout() -> *mut u8 {
    // SAFETY: inside IOB.
    unsafe { __iob_func().add(FILE_SIZE) }
}

// fprintf(file, fmt, ...): rcx = file, rdx = fmt, r8/r9 and the stack =
// the variadic arguments. Spill rdx, r8, r9 to the caller's home area, so
// [rsp+24] (r8's slot) starts the va_list, then vfprintf(file, fmt, list).
// printf(fmt, ...): the same with fmt in rcx and the list at [rsp+16].
core::arch::global_asm!(
    ".globl fprintf",
    "fprintf:",
    "mov [rsp+16], rdx",
    "mov [rsp+24], r8",
    "mov [rsp+32], r9",
    "lea r8, [rsp+24]",
    "sub rsp, 40",
    "call {vfprintf}",
    "add rsp, 40",
    "ret",
    ".globl printf",
    "printf:",
    "mov [rsp+8], rcx",
    "mov [rsp+16], rdx",
    "mov [rsp+24], r8",
    "mov [rsp+32], r9",
    "lea rdx, [rsp+16]",
    "sub rsp, 40",
    "call {vprintf}",
    "add rsp, 40",
    "ret",
    vfprintf = sym vfprintf,
    vprintf = sym vprintf,
);

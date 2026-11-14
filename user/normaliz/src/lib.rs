//! gala's normaliz.dll (libcurl imports its IDN functions from here):
//! IdnToAscii forwards to kernel32 (build.rs); IdnToUnicode is here,
//! since gala's kernel32 does not have it yet: each `xn--` label decoded
//! from Punycode as RFC 3492 section 6.2 specifies, other labels copied.

#![no_std]
#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

// The 32-bit build (SysWOW64) takes the memory routines, 64-bit
// division helpers and unwind personality from ntrt's x86 runtime
// (ARCHITECTURE "Plan: WOW64").
#[cfg(target_arch = "x86")]
use ntrt as _;

#[cfg(target_arch = "x86_64")]
#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memmove(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
    // MSVC-target unwind tables reference it (gala's ntdll provides it).
    fn __CxxFrameHandler3();
    // The stack probe for frames over a page (ntdll exports it).
    fn __chkstk();
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn SetLastError(e: u32);
}

/// MSVC-target codegen references this marker symbol directly, so it cannot
/// come from an import.
#[cfg(target_arch = "x86_64")]
#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

/// DllMain (the entry point our link flags name).
#[unsafe(no_mangle)]
pub extern "system" fn mainCRTStartup(_instance: usize, _reason: u32, _reserved: usize) -> i32 {
    1
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}

const ERROR_INSUFFICIENT_BUFFER: u32 = 122;
const ERROR_INVALID_NAME: u32 = 123;
const ERROR_INVALID_PARAMETER: u32 = 87;
/// A DNS name's limit (255 characters), with room for the decoded form.
const MAX: usize = 512;

/// RFC 3492's parameters for IDNA.
const BASE: u32 = 36;
const TMIN: u32 = 1;
const TMAX: u32 = 26;
const SKEW: u32 = 38;
const DAMP: u32 = 700;
const INITIAL_BIAS: u32 = 72;
const INITIAL_N: u32 = 128;

fn adapt(mut delta: u32, points: u32, first: bool) -> u32 {
    delta = if first { delta / DAMP } else { delta / 2 };
    delta += delta / points;
    let mut k = 0;
    while delta > ((BASE - TMIN) * TMAX) / 2 {
        delta /= BASE - TMIN;
        k += BASE;
    }
    k + (BASE - TMIN + 1) * delta / (delta + SKEW)
}

fn digit(c: u16) -> Option<u32> {
    match c {
        0x30..=0x39 => Some(u32::from(c) - 0x30 + 26),
        0x41..=0x5a => Some(u32::from(c) - 0x41),
        0x61..=0x7a => Some(u32::from(c) - 0x61),
        _ => None,
    }
}

/// Decodes the Punycode `input` (after `xn--`) into code points in `out`;
/// their number.
fn decode(input: &[u16], out: &mut [u32; MAX]) -> Option<usize> {
    let (mut n, mut i, mut bias) = (INITIAL_N, 0u32, INITIAL_BIAS);
    // The basic code points: everything before the last delimiter.
    let (basic, rest) = match input.iter().rposition(|&c| c == u16::from(b'-')) {
        Some(d) => (&input[..d], &input[d + 1..]),
        None => (&[][..], input),
    };
    let mut len = 0;
    for &c in basic {
        if c >= 0x80 || len == MAX {
            return None;
        }
        out[len] = c.into();
        len += 1;
    }
    let mut pos = 0;
    while pos < rest.len() {
        let old_i = i;
        let mut w = 1u32;
        let mut k = BASE;
        loop {
            let d = digit(*rest.get(pos)?)?;
            pos += 1;
            i = i.checked_add(d.checked_mul(w)?)?;
            let t = if k <= bias {
                TMIN
            } else if k >= bias + TMAX {
                TMAX
            } else {
                k - bias
            };
            if d < t {
                break;
            }
            w = w.checked_mul(BASE - t)?;
            k += BASE;
        }
        let points = len as u32 + 1;
        bias = adapt(i - old_i, points, old_i == 0);
        n = n.checked_add(i / points)?;
        i %= points;
        if len == MAX || n > 0x10ffff || (0xd800..=0xdfff).contains(&n) {
            return None;
        }
        out.copy_within(i as usize..len, i as usize + 1);
        out[i as usize] = n;
        len += 1;
        i += 1;
    }
    Some(len)
}

/// The Unicode form of `name` into `out`; its length in UTF-16 units.
fn to_unicode(name: &[u16], out: &mut [u16; MAX]) -> Option<usize> {
    let mut len = 0;
    let mut push = |c: u16| -> Option<()> {
        *out.get_mut(len)? = c;
        len += 1;
        Some(())
    };
    for (index, label) in name.split(|&c| c == u16::from(b'.')).enumerate() {
        if index > 0 {
            push(u16::from(b'.'))?;
        }
        let ace = label.len() > 4
            && label[..4]
                .iter()
                .zip(b"xn--")
                .all(|(&c, &p)| c < 128 && (c as u8).eq_ignore_ascii_case(&p));
        if !ace {
            for &c in label {
                push(c)?;
            }
            continue;
        }
        let mut points = [0u32; MAX];
        let n = decode(&label[4..], &mut points)?;
        for &p in &points[..n] {
            if p >= 0x10000 {
                let v = p - 0x10000;
                push(0xd800 + (v >> 10) as u16)?;
                push(0xdc00 + (v & 0x3ff) as u16)?;
            } else {
                push(p as u16)?;
            }
        }
    }
    Some(len)
}

fn fail(e: u32) -> i32 {
    // SAFETY: plain call.
    unsafe { SetLastError(e) };
    0
}

/// IdnToUnicode: the Unicode form of an IDN (ASCII-compatible) name;
/// `count` -1 means NUL-terminated (the result's count includes the NUL).
/// With `out_count` 0 the size needed. ERROR_INVALID_NAME for a label
/// that is not valid Punycode, ERROR_INSUFFICIENT_BUFFER when it does not
/// fit.
#[unsafe(no_mangle)]
pub extern "system" fn IdnToUnicode(
    _flags: u32,
    name: *const u16,
    count: i32,
    out: *mut u16,
    out_count: i32,
) -> i32 {
    if name.is_null()
        || count == 0
        || count < -1
        || out_count < 0
        || (out.is_null() && out_count != 0)
    {
        return fail(ERROR_INVALID_PARAMETER);
    }
    let (len, with_nul) = if count == -1 {
        let mut n = 0;
        // SAFETY: the caller's NUL-terminated string.
        while unsafe { *name.add(n) } != 0 {
            n += 1;
        }
        (n, true)
    } else {
        (count as usize, false)
    };
    if len > 255 {
        return fail(ERROR_INVALID_NAME);
    }
    // SAFETY: the caller's `len` characters.
    let text = unsafe { core::slice::from_raw_parts(name, len) };
    let mut result = [0u16; MAX];
    let Some(mut n) = to_unicode(text, &mut result) else { return fail(ERROR_INVALID_NAME) };
    if with_nul {
        if n == MAX {
            return fail(ERROR_INVALID_NAME);
        }
        result[n] = 0;
        n += 1;
    }
    if out_count == 0 {
        return n as i32;
    }
    if n > out_count as usize {
        return fail(ERROR_INSUFFICIENT_BUFFER);
    }
    // SAFETY: the caller's buffer of `out_count` >= `n` characters.
    unsafe { core::ptr::copy_nonoverlapping(result.as_ptr(), out, n) };
    n as i32
}

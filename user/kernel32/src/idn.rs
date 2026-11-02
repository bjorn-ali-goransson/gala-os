//! IdnToAscii: internationalized host names in their ASCII form (RFC 3490):
//! each label that is not all ASCII is case-folded and encoded in Punycode
//! (RFC 3492) behind "xn--"; ASCII labels are kept as they are. The full
//! nameprep tables (RFC 3491) are not applied: only lowercasing.

use super::*;

const ERROR_INSUFFICIENT_BUFFER: u32 = 122;
const ERROR_INVALID_PARAMETER: u32 = 87;
const ERROR_INVALID_NAME: u32 = 123;

/// Longest name and label (RFC 1034), in characters.
const MAX_NAME: usize = 255;
const MAX_LABEL: usize = 63;

const BASE: u32 = 36;
const TMIN: u32 = 1;
const TMAX: u32 = 26;
const SKEW: u32 = 38;
const DAMP: u32 = 700;

fn adapt(delta: u32, points: u32, first: bool) -> u32 {
    let mut delta = if first { delta / DAMP } else { delta / 2 };
    delta += delta / points;
    let mut k = 0;
    while delta > ((BASE - TMIN) * TMAX) / 2 {
        delta /= BASE - TMIN;
        k += BASE;
    }
    k + (BASE - TMIN + 1) * delta / (delta + SKEW)
}

fn digit(d: u32) -> u16 {
    (if d < 26 { u32::from(b'a') + d } else { u32::from(b'0') + d - 26 }) as u16
}

/// Appends to `out` (length `n`), or None when it is full.
fn push(out: &mut [u16], n: &mut usize, c: u16) -> Option<()> {
    *out.get_mut(*n)? = c;
    *n += 1;
    Some(())
}

/// Punycode of the code points `label` (RFC 3492, 6.3) after "xn--".
fn punycode(label: &[u32], out: &mut [u16], n: &mut usize) -> Option<()> {
    for &c in b"xn--" {
        push(out, n, u16::from(c))?;
    }
    let basic = label.iter().filter(|&&c| c < 0x80).count() as u32;
    for &c in label.iter().filter(|&&c| c < 0x80) {
        push(out, n, c as u16)?;
    }
    if basic > 0 {
        push(out, n, u16::from(b'-'))?;
    }
    let (mut code, mut delta, mut bias, mut handled) = (0x80u32, 0u32, 72u32, basic);
    while (handled as usize) < label.len() {
        let m = label.iter().copied().filter(|&c| c >= code).min()?;
        delta = delta.checked_add((m - code).checked_mul(handled + 1)?)?;
        code = m;
        for &c in label {
            if c < code {
                delta = delta.checked_add(1)?;
            }
            if c == code {
                let mut q = delta;
                let mut k = BASE;
                loop {
                    let t = if k <= bias {
                        TMIN
                    } else if k >= bias + TMAX {
                        TMAX
                    } else {
                        k - bias
                    };
                    if q < t {
                        break;
                    }
                    push(out, n, digit(t + (q - t) % (BASE - t)))?;
                    q = (q - t) / (BASE - t);
                    k += BASE;
                }
                push(out, n, digit(q))?;
                bias = adapt(delta, handled + 1, handled == basic);
                delta = 0;
                handled += 1;
            }
        }
        delta = delta.checked_add(1)?;
        code += 1;
    }
    Some(())
}

/// The dots IDNA separates labels with (RFC 3490, 3.1).
fn is_dot(c: u32) -> bool {
    matches!(c, 0x2e | 0x3002 | 0xff0e | 0xff61)
}

/// The ASCII form of `name` (UTF-16, no NUL) into `out`; its length, or
/// the error.
fn to_ascii(name: &[u16], out: &mut [u16; MAX_NAME + 1]) -> Result<usize, u32> {
    let mut points = [0u32; MAX_NAME + 1];
    let mut count = 0;
    for c in char::decode_utf16(name.iter().copied()) {
        let c = c.map_err(|_| ERROR_INVALID_NAME)?;
        *points.get_mut(count).ok_or(ERROR_INVALID_NAME)? = c as u32;
        count += 1;
    }
    let mut n = 0;
    for (i, label) in points[..count].split(|&c| is_dot(c)).enumerate() {
        if i > 0 {
            push(out, &mut n, u16::from(b'.')).ok_or(ERROR_INVALID_NAME)?;
        }
        let start = n;
        if label.iter().all(|&c| c < 0x80) {
            for &c in label {
                push(out, &mut n, c as u16).ok_or(ERROR_INVALID_NAME)?;
            }
        } else {
            // Case folding (the part of nameprep applied).
            let mut folded = [0u32; MAX_LABEL * 2];
            let mut len = 0;
            for &c in label {
                let ch = char::from_u32(c).ok_or(ERROR_INVALID_NAME)?;
                for l in ch.to_lowercase() {
                    *folded.get_mut(len).ok_or(ERROR_INVALID_NAME)? = l as u32;
                    len += 1;
                }
            }
            punycode(&folded[..len], out, &mut n).ok_or(ERROR_INVALID_NAME)?;
        }
        if n - start > MAX_LABEL {
            return Err(ERROR_INVALID_NAME);
        }
    }
    Ok(n)
}

/// IdnToAscii(flags, unicode, n, ascii, size): the name's ASCII form;
/// characters written (the NUL included when `n` is -1), the size needed
/// when `size` is 0, or 0 with the error.
#[unsafe(no_mangle)]
pub extern "system" fn IdnToAscii(
    _flags: u32,
    unicode: *const u16,
    n: i32,
    ascii: *mut u16,
    size: i32,
) -> i32 {
    if unicode.is_null() || n == 0 || n < -1 || size < 0 || (size > 0 && ascii.is_null()) {
        SetLastError(ERROR_INVALID_PARAMETER);
        return 0;
    }
    let len = if n == -1 {
        let mut k = 0;
        // SAFETY: the caller's NUL-terminated string.
        while unsafe { *unicode.add(k) } != 0 {
            k += 1;
            if k > MAX_NAME {
                SetLastError(ERROR_INVALID_NAME);
                return 0;
            }
        }
        k
    } else {
        n as usize
    };
    // SAFETY: the caller's `len` characters.
    let name = unsafe { core::slice::from_raw_parts(unicode, len) };
    let mut out = [0u16; MAX_NAME + 1];
    let written = match to_ascii(name, &mut out) {
        Ok(w) => w,
        Err(e) => {
            SetLastError(e);
            return 0;
        }
    };
    let total = written + usize::from(n == -1);
    if size == 0 {
        return total as i32;
    }
    if total > size as usize {
        SetLastError(ERROR_INSUFFICIENT_BUFFER);
        return 0;
    }
    // SAFETY: the caller's buffer of `size` characters; out[written] is 0.
    unsafe { core::ptr::copy_nonoverlapping(out.as_ptr(), ascii, total) };
    total as i32
}

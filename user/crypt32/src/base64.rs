//! Base64 through CryptBinaryToString and CryptStringToBinary (A and W),
//! with the documented contracts: CRYPT_STRING_BASE64 only (other formats
//! are recorded and fail with ERROR_INVALID_PARAMETER); lines of 64
//! characters each ending in CRLF unless CRYPT_STRING_NOCRLF (or
//! CRYPT_STRING_NOCR: LF only); a null output asks for the size (with the
//! NUL), a written string's length excludes it; too small a buffer is
//! ERROR_MORE_DATA. Decoding skips whitespace and fails with
//! ERROR_INVALID_DATA on anything else that is not base64.

use super::{SetLastError, missing_detail};

const BASE64: u32 = 1;
const NOCRLF: u32 = 0x4000_0000;
const NOCR: u32 = 0x8000_0000;
/// The format bits of the flags.
const FORMAT: u32 = 0xff;

const ERROR_INVALID_PARAMETER: u32 = 87;
const ERROR_INVALID_DATA: u32 = 13;
const ERROR_MORE_DATA: u32 = 234;

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn error(code: u32) -> i32 {
    // SAFETY: a plain call.
    unsafe { SetLastError(code) };
    0
}

/// Calls `put(index, char)` for each character of the encoding of `data`
/// (with line breaks per `flags`); returns the count.
fn encode(data: &[u8], flags: u32, mut put: impl FnMut(usize, u8)) -> usize {
    let mut n = 0;
    let mut line = 0;
    let mut emit = |c: u8, n: &mut usize| {
        put(*n, c);
        *n += 1;
    };
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let v = u32::from(b[0]) << 16 | u32::from(b[1]) << 8 | u32::from(b[2]);
        for i in 0..4 {
            let c =
                if i <= chunk.len() { ALPHABET[(v >> (18 - 6 * i)) as usize & 63] } else { b'=' };
            emit(c, &mut n);
        }
        line += 4;
        if line == 64 && flags & NOCRLF == 0 {
            if flags & NOCR == 0 {
                emit(b'\r', &mut n);
            }
            emit(b'\n', &mut n);
            line = 0;
        }
    }
    if line != 0 && flags & NOCRLF == 0 {
        if flags & NOCR == 0 {
            emit(b'\r', &mut n);
        }
        emit(b'\n', &mut n);
    }
    n
}

/// The shared body of CryptBinaryToStringA/W: `write(index, char)` stores
/// into the caller's string.
fn binary_to_string(
    name: &[u8],
    data: *const u8,
    len: u32,
    flags: u32,
    has_out: bool,
    chars: *mut u32,
    mut write: impl FnMut(usize, u8),
) -> i32 {
    if flags & FORMAT != BASE64 {
        missing_detail(name, flags.into());
        return error(ERROR_INVALID_PARAMETER);
    }
    if chars.is_null() || (data.is_null() && len != 0) {
        return error(ERROR_INVALID_PARAMETER);
    }
    let data = if len == 0 {
        &[][..]
    } else {
        // SAFETY: the caller's `len` bytes.
        unsafe { core::slice::from_raw_parts(data, len as usize) }
    };
    let needed = encode(data, flags, |_, _| {});
    // SAFETY: the caller's in/out count.
    let room = unsafe { *chars } as usize;
    if !has_out {
        // SAFETY: as above.
        unsafe { *chars = needed as u32 + 1 };
        return 1;
    }
    if room < needed + 1 {
        // SAFETY: as above.
        unsafe { *chars = needed as u32 + 1 };
        return error(ERROR_MORE_DATA);
    }
    encode(data, flags, &mut write);
    write(needed, 0);
    // SAFETY: as above.
    unsafe { *chars = needed as u32 };
    1
}

/// CryptBinaryToStringA(pbBinary, cbBinary, dwFlags, pszString, pcchString)
#[unsafe(no_mangle)]
pub extern "system" fn CryptBinaryToStringA(
    data: *const u8,
    len: u32,
    flags: u32,
    text: *mut u8,
    chars: *mut u32,
) -> i32 {
    binary_to_string(
        b"crypt32.dll!CryptBinaryToStringA(format)",
        data,
        len,
        flags,
        !text.is_null(),
        chars,
        // SAFETY: inside the caller's buffer (its size was checked).
        |i, c| unsafe { *text.add(i) = c },
    )
}

/// CryptBinaryToStringW: as the A function, in UTF-16.
#[unsafe(no_mangle)]
pub extern "system" fn CryptBinaryToStringW(
    data: *const u8,
    len: u32,
    flags: u32,
    text: *mut u16,
    chars: *mut u32,
) -> i32 {
    binary_to_string(
        b"crypt32.dll!CryptBinaryToStringW(format)",
        data,
        len,
        flags,
        !text.is_null(),
        chars,
        // SAFETY: inside the caller's buffer (its size was checked).
        |i, c| unsafe { *text.add(i) = c.into() },
    )
}

fn value(c: u32) -> Option<u32> {
    let c = u8::try_from(c).ok()?;
    ALPHABET.iter().position(|&a| a == c).map(|v| v as u32)
}

/// Decodes `count` characters (`at(i)`), calling `put(index, byte)`;
/// the number of bytes, or None if it is not base64.
fn decode(
    count: usize,
    at: impl Fn(usize) -> u32,
    mut put: impl FnMut(usize, u8),
) -> Option<usize> {
    let (mut acc, mut bits, mut n, mut padding) = (0u32, 0, 0, 0);
    for i in 0..count {
        let c = at(i);
        if matches!(c, 0x20 | 0x09 | 0x0d | 0x0a) {
            continue;
        }
        if c == u32::from(b'=') {
            padding += 1;
            continue;
        }
        if padding != 0 {
            return None; // data after the padding
        }
        acc = acc << 6 | value(c)?;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            put(n, (acc >> bits) as u8);
            n += 1;
        }
    }
    if padding > 2 || bits >= 6 {
        return None;
    }
    Some(n)
}

/// The shared body of CryptStringToBinaryA/W; `at(i)` reads character i
/// and `count` is the length (the string up to its NUL when the caller
/// passes 0).
#[allow(clippy::too_many_arguments)]
fn string_to_binary(
    name: &[u8],
    count: usize,
    at: impl Fn(usize) -> u32,
    flags: u32,
    out: *mut u8,
    len: *mut u32,
    skipped: *mut u32,
    format: *mut u32,
) -> i32 {
    if flags & FORMAT != BASE64 {
        missing_detail(name, flags.into());
        return error(ERROR_INVALID_PARAMETER);
    }
    if len.is_null() {
        return error(ERROR_INVALID_PARAMETER);
    }
    let Some(needed) = decode(count, &at, |_, _| {}) else { return error(ERROR_INVALID_DATA) };
    // SAFETY: the caller's in/out size.
    let room = unsafe { *len } as usize;
    // SAFETY: the caller's outputs.
    unsafe {
        *len = needed as u32;
        if !skipped.is_null() {
            *skipped = 0;
        }
        if !format.is_null() {
            *format = BASE64;
        }
    }
    if out.is_null() {
        return 1;
    }
    if room < needed {
        return error(ERROR_MORE_DATA);
    }
    // SAFETY: inside the caller's buffer (its size was checked).
    decode(count, at, |i, b| unsafe { *out.add(i) = b });
    1
}

/// CryptStringToBinaryA(pszString, cchString, dwFlags, pbBinary,
/// pcbBinary, pdwSkip, pdwFlags)
#[unsafe(no_mangle)]
pub extern "system" fn CryptStringToBinaryA(
    text: *const u8,
    chars: u32,
    flags: u32,
    out: *mut u8,
    len: *mut u32,
    skipped: *mut u32,
    format: *mut u32,
) -> i32 {
    if text.is_null() {
        return error(ERROR_INVALID_PARAMETER);
    }
    let count = if chars == 0 {
        // SAFETY: a NUL-terminated string.
        unsafe { super::strlen(text) }
    } else {
        chars as usize
    };
    string_to_binary(
        b"crypt32.dll!CryptStringToBinaryA(format)",
        count,
        // SAFETY: within the caller's `count` characters.
        |i| u32::from(unsafe { *text.add(i) }),
        flags,
        out,
        len,
        skipped,
        format,
    )
}

/// CryptStringToBinaryW: as the A function, in UTF-16.
#[unsafe(no_mangle)]
pub extern "system" fn CryptStringToBinaryW(
    text: *const u16,
    chars: u32,
    flags: u32,
    out: *mut u8,
    len: *mut u32,
    skipped: *mut u32,
    format: *mut u32,
) -> i32 {
    if text.is_null() {
        return error(ERROR_INVALID_PARAMETER);
    }
    let count = if chars == 0 {
        let mut n = 0;
        // SAFETY: a NUL-terminated string.
        while unsafe { *text.add(n) } != 0 {
            n += 1;
        }
        n
    } else {
        chars as usize
    };
    string_to_binary(
        b"crypt32.dll!CryptStringToBinaryW(format)",
        count,
        // SAFETY: within the caller's `count` characters.
        |i| u32::from(unsafe { *text.add(i) }),
        flags,
        out,
        len,
        skipped,
        format,
    )
}

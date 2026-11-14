//! The PathCch functions programs import from the `api-ms-win-core-path`
//! API set (pathcch.h's documented contracts): kernelbase's alone, as on
//! Windows, whose kernel32 exports none of them. PathCchAppend,
//! PathCchCombine, PathCchRemoveBackslash and PathCchRemoveExtension are
//! not implemented yet (lib.rs's stubs).

const S_OK: i32 = 0;
const S_FALSE: i32 = 1;
const E_INVALIDARG: i32 = 0x8007_0057u32 as i32;
/// HRESULT_FROM_WIN32(ERROR_INSUFFICIENT_BUFFER).
const HR_INSUFFICIENT_BUFFER: i32 = 0x8007_007au32 as i32;
/// PATHCCH_MAX_CCH: the longest path the PathCch functions take.
const PATHCCH_MAX_CCH: usize = 0x8000;
const SLASH: u16 = b'\\' as u16;

/// The characters of the NUL-terminated `p` (empty for null), at most
/// PATHCCH_MAX_CCH.
fn chars<'a>(p: *const u16) -> &'a [u16] {
    if p.is_null() {
        return &[];
    }
    let mut n = 0;
    // SAFETY: the caller's NUL-terminated string.
    while n < PATHCCH_MAX_CCH && unsafe { *p.add(n) } != 0 {
        n += 1;
    }
    // SAFETY: `n` characters were just read.
    unsafe { core::slice::from_raw_parts(p, n) }
}

fn is_drive(p: &[u16]) -> bool {
    p.len() >= 2
        && p[1] == u16::from(b':')
        && (p[0] as u8 as char).is_ascii_alphabetic()
        && p[0] < 0x80
}

/// How many characters the root takes: `X:\` (or `X:`), `\\server\share\`
/// (or without the last backslash), `\\?\X:\`, `\\?\UNC\server\share\`,
/// a lone `\`; None for a relative path.
fn root_len(p: &[u16]) -> Option<usize> {
    let starts = |s: &[u8]| p.len() >= s.len() && p.iter().zip(s).all(|(&a, &b)| a == u16::from(b));
    if starts(b"\\\\?\\UNC\\") {
        return Some(unc_root(p, 8));
    }
    if starts(b"\\\\?\\") {
        let rest = &p[4..];
        if is_drive(rest) {
            return Some(4 + if rest.get(2) == Some(&SLASH) { 3 } else { 2 });
        }
        return Some(4);
    }
    if starts(b"\\\\") {
        return Some(unc_root(p, 2));
    }
    if is_drive(p) {
        return Some(if p.get(2) == Some(&SLASH) { 3 } else { 2 });
    }
    if p.first() == Some(&SLASH) {
        return Some(1);
    }
    None
}

/// The end of `server\share\` starting at `from`.
fn unc_root(p: &[u16], from: usize) -> usize {
    let mut i = from;
    let mut parts = 0;
    while i < p.len() {
        if p[i] == SLASH {
            parts += 1;
            if parts == 2 {
                return i + 1;
            }
        }
        i += 1;
    }
    p.len()
}

/// PathCchSkipRoot(pszPath, ppszRootEnd): the first character after the
/// root; E_INVALIDARG for a path without one.
#[unsafe(no_mangle)]
pub extern "system" fn PathCchSkipRoot(path: *const u16, root_end: *mut *const u16) -> i32 {
    if path.is_null() || root_end.is_null() {
        return E_INVALIDARG;
    }
    let p = chars(path);
    match root_len(p) {
        Some(n) => {
            // SAFETY: within the caller's string; the caller's output.
            unsafe { *root_end = path.add(n) };
            S_OK
        }
        None => E_INVALIDARG,
    }
}

/// Canonicalizes `p` into `out`: `.` dropped, `..` removes the component
/// before it (never the root), separators single; a root alone keeps its
/// backslash (`C:\`).
fn canonical(p: &[u16], out: &mut [u16; PATHCCH_MAX_CCH]) -> Option<usize> {
    fn push(out: &mut [u16], n: &mut usize, c: u16) -> Option<()> {
        *out.get_mut(*n)? = c;
        *n += 1;
        Some(())
    }
    let root = root_len(p).unwrap_or(0);
    let mut n = 0;
    for &c in &p[..root] {
        push(out, &mut n, c)?;
    }
    let base = n;
    let trailing = p.len() > root && p.last() == Some(&SLASH);
    for part in p[root..].split(|&c| c == SLASH).filter(|s| !s.is_empty()) {
        if part == [u16::from(b'.')] {
            continue;
        }
        if part == [u16::from(b'.'), u16::from(b'.')] {
            // Back to the separator before the last component.
            while n > base && out[n - 1] != SLASH {
                n -= 1;
            }
            if n > base {
                n -= 1;
            }
            continue;
        }
        if n > base || (n > 0 && out[n - 1] != SLASH) {
            push(out, &mut n, SLASH)?;
        }
        for &c in part {
            push(out, &mut n, c)?;
        }
    }
    if (trailing && n > base) || (n == 2 && is_drive(&out[..2])) {
        push(out, &mut n, SLASH)?;
    }
    Some(n)
}

/// PathCchCombineEx(pszPathOut, cchPathOut, pszPathIn, pszMore, dwFlags):
/// `pszMore` appended to `pszPathIn` (or replacing it when absolute, or
/// keeping only its root when rooted), canonicalized.
#[unsafe(no_mangle)]
pub extern "system" fn PathCchCombineEx(
    out: *mut u16,
    cch: usize,
    a: *const u16,
    b: *const u16,
    _flags: u32,
) -> i32 {
    if out.is_null() || cch == 0 || cch > PATHCCH_MAX_CCH {
        return E_INVALIDARG;
    }
    let (a, b) = (chars(a), chars(b));
    let mut joined = [0u16; PATHCCH_MAX_CCH];
    let mut n = 0;
    let mut put = |s: &[u16], n: &mut usize| -> bool {
        if *n + s.len() > joined.len() {
            return false;
        }
        joined[*n..*n + s.len()].copy_from_slice(s);
        *n += s.len();
        true
    };
    let b_root = root_len(b);
    let fits = match b_root {
        // Absolute (a drive or UNC root): `pszMore` alone.
        Some(r) if r > 1 => put(b, &mut n),
        // Rooted (`\x`): the first path's drive or share, then it.
        Some(_) => {
            let keep =
                root_len(a).map_or(0, |r| if a.get(r - 1) == Some(&SLASH) { r - 1 } else { r });
            put(&a[..keep], &mut n) && put(b, &mut n)
        }
        None => {
            put(a, &mut n)
                && (a.is_empty()
                    || b.is_empty()
                    || a.last() == Some(&SLASH)
                    || put(&[SLASH], &mut n))
                && put(b, &mut n)
        }
    };
    let mut canon = [0u16; PATHCCH_MAX_CCH];
    let len = if fits { canonical(&joined[..n], &mut canon) } else { None };
    let Some(len) = len.filter(|&len| len < cch) else {
        // SAFETY: the caller's buffer of at least one character.
        unsafe { *out = 0 };
        return HR_INSUFFICIENT_BUFFER;
    };
    // SAFETY: the caller's buffer of `cch` > `len` characters.
    unsafe {
        core::ptr::copy_nonoverlapping(canon.as_ptr(), out, len);
        *out.add(len) = 0;
    }
    S_OK
}

/// PathCchRemoveFileSpec(pszPath, cchPath): removes the last element, file
/// or directory, with the backslash before it, never the root (`C:\a` is
/// `C:\`); S_OK when something was removed, S_FALSE when not (a root
/// alone, an empty path); E_INVALIDARG for no path or a size of 0 or over
/// PATHCCH_MAX_CCH.
#[unsafe(no_mangle)]
pub extern "system" fn PathCchRemoveFileSpec(path: *mut u16, cch: usize) -> i32 {
    if path.is_null() || cch == 0 || cch > PATHCCH_MAX_CCH {
        return E_INVALIDARG;
    }
    let p = chars(path);
    let root = root_len(p).unwrap_or(0);
    if p.len() <= root {
        return S_FALSE;
    }
    // The last backslash after the root ends the kept part; with none, the
    // root (or nothing, for a relative name) stays.
    let end = match p[root..].iter().rposition(|&c| c == SLASH) {
        Some(i) => root + i,
        None => root,
    };
    // SAFETY: `end` is within the caller's string.
    unsafe { *path.add(end) = 0 };
    S_OK
}

// ---- PathCchCanonicalize(Ex) ----
//
// Adapted from Wine's PathAllocCanonicalize and PathCchCanonicalizeEx
// (dlls/kernelbase/path.c, LGPL-2.1+; THIRD_PARTY.md), whose rules its
// tests (dlls/kernelbase/tests/path.c, run on Windows) pin: the root
// copied (a `\\?\` disk or UNC prefix stripped, `X:` completed to
// `X:\`), `.` and `..` segments removed only between backslashes (one
// `.` kept after `*`), trailing dots stripped, an empty result `\`.

const MAX_PATH: usize = 260;
/// HRESULT_FROM_WIN32(ERROR_FILENAME_EXCED_RANGE).
const HR_FILENAME_EXCED_RANGE: i32 = 0x8007_00ceu32 as i32;
const PATHCCH_ALLOW_LONG_PATHS: u32 = 0x1;
const PATHCCH_FORCE_ENABLE_LONG_NAME_PROCESS: u32 = 0x2;
const PATHCCH_FORCE_DISABLE_LONG_NAME_PROCESS: u32 = 0x4;
const PATHCCH_DO_NOT_NORMALIZE_SEGMENTS: u32 = 0x8;
const PATHCCH_ENSURE_IS_EXTENDED_LENGTH_PATH: u32 = 0x10;
const PATHCCH_ENSURE_TRAILING_SLASH: u32 = 0x20;
const DOT: u16 = b'.' as u16;

fn drive_spec(p: &[u16]) -> bool {
    p.len() >= 2 && p[0] < 0x80 && (p[0] as u8).is_ascii_alphabetic() && p[1] == u16::from(b':')
}

fn starts_ci(p: &[u16], s: &[u8]) -> bool {
    p.len() >= s.len()
        && p.iter().zip(s).all(|(&a, &b)| a < 0x80 && (a as u8).eq_ignore_ascii_case(&b))
}

fn prefixed_disk(p: &[u16]) -> bool {
    starts_ci(p, b"\\\\?\\") && drive_spec(&p[4..])
}

/// `\\?\Volume{xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx}`.
fn prefixed_volume(p: &[u16]) -> bool {
    if !starts_ci(p, b"\\\\?\\Volume") || p.len() < 48 {
        return false;
    }
    p[10..48].iter().enumerate().all(|(i, &c)| match i {
        0 => c == u16::from(b'{'),
        9 | 14 | 19 | 24 => c == u16::from(b'-'),
        37 => c == u16::from(b'}'),
        _ => c < 0x80 && (c as u8).is_ascii_hexdigit(),
    })
}

/// The index of the root's last character, if the path has a root.
fn root_end(p: &[u16]) -> Option<usize> {
    let at = |i: usize| p.get(i).copied().unwrap_or(0);
    if prefixed_volume(p) {
        Some(if at(48) == SLASH { 48 } else { 47 })
    } else if starts_ci(p, b"\\\\?\\UNC\\") {
        Some(7)
    } else if prefixed_disk(p) {
        Some(if at(6) == SLASH { 6 } else { 5 })
    } else if at(0) == SLASH && at(1) == SLASH {
        Some(1)
    } else if at(0) == SLASH {
        Some(0)
    } else if drive_spec(p) {
        Some(if at(2) == SLASH { 2 } else { 1 })
    } else {
        None
    }
}

/// The canonical form of `p` in `buf` (its length), or the HRESULT.
fn canonicalize(p: &[u16], flags: u32, buf: &mut [u16; PATHCCH_MAX_CCH + 8]) -> Result<usize, i32> {
    let long = PATHCCH_FORCE_ENABLE_LONG_NAME_PROCESS | PATHCCH_FORCE_DISABLE_LONG_NAME_PROCESS;
    if flags & long == long
        || (flags & long != 0 && flags & PATHCCH_ALLOW_LONG_PATHS == 0)
        || (flags & PATHCCH_ENSURE_IS_EXTENDED_LENGTH_PATH != 0
            && flags & PATHCCH_ALLOW_LONG_PATHS != 0)
    {
        return Err(E_INVALIDARG);
    }
    let allow_long =
        flags & (PATHCCH_ALLOW_LONG_PATHS | PATHCCH_ENSURE_IS_EXTENDED_LENGTH_PATH) != 0;
    if (p.len() + 1 > MAX_PATH && !allow_long) || p.len() + 1 > PATHCCH_MAX_CCH {
        return Err(HR_FILENAME_EXCED_RANGE);
    }
    let normalize =
        flags & (PATHCCH_DO_NOT_NORMALIZE_SEGMENTS | PATHCCH_ENSURE_IS_EXTENDED_LENGTH_PATH) == 0;
    buf.fill(0);
    let src_at = |i: usize| p.get(i).copied().unwrap_or(0);
    let mut src = 0;
    let mut dst = 0;
    let mut root: Option<usize> = None;
    if let Some(r) = root_end(p) {
        buf[..=r].copy_from_slice(&p[..=r]);
        src = r + 1;
        // The prefix stripped: `\\?\UNC\a` is `\\a`, `\\?\C:\` is `C:\`.
        let stripped = if starts_ci(&buf[..=r], b"\\\\?\\UNC\\") {
            buf.copy_within(8..=r, 2);
            let n = r + 1 - 6;
            buf[n..=r].fill(0);
            Some(n)
        } else if prefixed_disk(&buf[..=r]) {
            buf.copy_within(4..=r, 0);
            let n = r + 1 - 4;
            buf[n..=r].fill(0);
            Some(n)
        } else {
            None
        };
        match stripped {
            Some(mut n) => {
                if drive_spec(&buf[..n]) && buf[2] != SLASH {
                    buf[2] = SLASH;
                    buf[3] = 0;
                    n = 3;
                }
                dst = n;
                root = Some(n);
            }
            None => {
                dst = r + 1;
                root = Some(r);
            }
        }
    }
    // `root`: the index of the root's last character, except after a
    // stripped prefix, where (as Wine's) it is the end of what was kept.
    while src < p.len() {
        if p[src] == DOT {
            if src_at(src + 1) == DOT {
                // One . kept after *.
                if dst > 0 && buf[dst - 1] == u16::from(b'*') {
                    buf[dst] = p[src];
                    dst += 1;
                    src += 1;
                    continue;
                }
                // .. not between backslashes: kept.
                let next = src_at(src + 2);
                if (next != SLASH && next != 0) || (dst > 0 && buf[dst - 1] != SLASH) {
                    buf[dst] = p[src];
                    buf[dst + 1] = p[src + 1];
                    dst += 2;
                    src += 2;
                    continue;
                }
                if dst > 0 && buf[dst - 1] == SLASH && root.is_none_or(|r| dst - 1 > r) {
                    // The \ before .. and the segment before it go.
                    dst -= 1;
                    buf[dst] = 0;
                    while dst > 0 {
                        dst -= 1;
                        let was = buf[dst];
                        buf[dst] = 0;
                        if was == SLASH {
                            break;
                        }
                    }
                } else if next == SLASH {
                    src += 1;
                }
                src += 2;
            } else {
                let next = src_at(src + 1);
                if (next != SLASH && next != 0) || (dst > 0 && buf[dst - 1] != SLASH) {
                    buf[dst] = p[src];
                    dst += 1;
                    src += 1;
                    continue;
                }
                if dst > 0 && buf[dst - 1] == SLASH && root.is_none_or(|r| dst - 1 > r) {
                    dst -= 1;
                } else if next == SLASH {
                    src += 1;
                }
                src += 1;
            }
            // An incomplete X: completed to X:\ (the next \ used for it).
            if drive_spec(&buf[..]) && buf[2] != SLASH {
                root = Some(2);
                dst = 3;
                buf[2] = SLASH;
                if src_at(src) == SLASH {
                    src += 1;
                }
            }
        } else {
            buf[dst] = p[src];
            dst += 1;
            src += 1;
        }
    }
    buf[dst..].fill(0);
    if normalize {
        // Trailing dots stripped (one kept after *; `X:.` becomes `X:\`).
        while dst > 0 && buf[dst - 1] == DOT {
            if dst >= 2 && buf[dst - 2] == u16::from(b'*') {
                break;
            } else if dst >= 3 && buf[dst - 2] == u16::from(b':') && dst - 2 == 1 {
                buf[dst - 1] = SLASH;
                break;
            } else {
                dst -= 1;
                buf[dst] = 0;
            }
        }
    }
    if dst == 0 {
        buf[0] = SLASH;
        dst = 1;
    }
    let mut len = dst;
    if ((len + 1 > MAX_PATH && drive_spec(&buf[..]))
        || (drive_spec(&buf[..]) && flags & PATHCCH_ENSURE_IS_EXTENDED_LENGTH_PATH != 0))
        && flags & PATHCCH_FORCE_ENABLE_LONG_NAME_PROCESS == 0
    {
        buf.copy_within(0..len, 4);
        buf[..4].copy_from_slice(&[SLASH, SLASH, u16::from(b'?'), SLASH]);
        len += 4;
    }
    if flags & PATHCCH_ENSURE_TRAILING_SLASH != 0 && buf[len - 1] != SLASH {
        buf[len] = SLASH;
        len += 1;
    }
    buf[len] = 0;
    Ok(len)
}

/// PathCchCanonicalizeEx(pszPathOut, cchPathOut, pszPathIn, dwFlags): the
/// canonical form; E_INVALIDARG for a size of 0 or conflicting flags,
/// ERROR_FILENAME_EXCED_RANGE for a path too long,
/// STRSAFE_E_INSUFFICIENT_BUFFER (the out buffer untouched) for a buffer
/// too small; `X:` in a buffer with room becomes `X:\`.
#[unsafe(no_mangle)]
pub extern "system" fn PathCchCanonicalizeEx(
    out: *mut u16,
    cch: usize,
    input: *const u16,
    flags: u32,
) -> i32 {
    if out.is_null() || cch == 0 || input.is_null() {
        return E_INVALIDARG;
    }
    let p = chars(input);
    let mut buf = [0u16; PATHCCH_MAX_CCH + 8];
    let len = match canonicalize(p, flags, &mut buf) {
        Ok(len) => len,
        Err(hr) => {
            if hr == HR_FILENAME_EXCED_RANGE {
                // SAFETY: the caller's buffer of at least one character.
                unsafe { *out = 0 };
            }
            return hr;
        }
    };
    if cch < len + 1 {
        let rooted = p.first() == Some(&SLASH) || (drive_spec(p) && p.get(2) == Some(&SLASH));
        return if len > MAX_PATH - 4 && !rooted {
            HR_FILENAME_EXCED_RANGE
        } else {
            HR_INSUFFICIENT_BUFFER
        };
    }
    // SAFETY: the caller's buffer of `cch` > `len` characters.
    unsafe {
        core::ptr::copy_nonoverlapping(buf.as_ptr(), out, len + 1);
        if len == 2 && drive_spec(&buf[..2]) && cch > 3 {
            *out.add(2) = SLASH;
            *out.add(3) = 0;
        }
    }
    S_OK
}

/// PathCchCanonicalize(pszPathOut, cchPathOut, pszPathIn):
/// PathCchCanonicalizeEx without flags; a path longer than MAX_PATH - 4
/// that is not `X:\...` is ERROR_FILENAME_EXCED_RANGE.
#[unsafe(no_mangle)]
pub extern "system" fn PathCchCanonicalize(out: *mut u16, cch: usize, input: *const u16) -> i32 {
    if input.is_null() {
        return E_INVALIDARG;
    }
    let p = chars(input);
    if p.len() > MAX_PATH - 4 && !(drive_spec(p) && p.get(2) == Some(&SLASH)) {
        if !out.is_null() && cch > 0 {
            // SAFETY: the caller's buffer of at least one character.
            unsafe { *out = 0 };
        }
        return HR_FILENAME_EXCED_RANGE;
    }
    PathCchCanonicalizeEx(out, cch, input, 0)
}

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

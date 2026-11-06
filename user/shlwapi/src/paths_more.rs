//! More path helpers (shlwapi.h), for what SQL Server LocalDB and
//! wxWidgets import: canonicalizing and combining, extensions, trailing
//! backslashes, relative paths and URLs, wildcard specs, qualifying.
//! Behaviour as documented and as Wine's conformance tests (validated on
//! Windows) show it; see user/shlwapi_test.

#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use crate::paths::{BACKSLASH, MAX_PATH, len};

const SLASH: u16 = b'/' as u16;
const DOT: u16 = b'.' as u16;
const COLON: u16 = b':' as u16;
const ERROR_INVALID_PARAMETER: u32 = 87;
const ERROR_FILENAME_EXCED_RANGE: u32 = 206;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn SetLastError(error: u32);
    fn GetFullPathNameW(path: *const u16, size: u32, buf: *mut u16, file: *mut *mut u16) -> u32;
    fn SearchPathW(
        path: *const u16,
        file: *const u16,
        ext: *const u16,
        size: u32,
        buf: *mut u16,
        part: *mut *mut u16,
    ) -> u32;
}

fn set_last_error(e: u32) {
    // SAFETY: plain call.
    unsafe { SetLastError(e) }
}

/// The characters of the caller's NUL-terminated string (empty for NULL).
pub(crate) fn chars<'a>(p: *const u16) -> &'a [u16] {
    if p.is_null() {
        return &[];
    }
    // SAFETY: the caller's string of `len(p)` characters.
    unsafe { core::slice::from_raw_parts(p, len(p)) }
}

/// Copies `s` and a NUL into the caller's buffer (checked by the callers
/// to hold them).
fn store(dst: *mut u16, s: &[u16]) {
    // SAFETY: the caller's buffer of at least `s.len() + 1` characters.
    unsafe {
        core::ptr::copy(s.as_ptr(), dst, s.len());
        dst.add(s.len()).write(0);
    }
}

/// Empties the caller's buffer.
fn clear(dst: *mut u16) {
    // SAFETY: the caller's buffer (non-null, checked by the callers).
    unsafe { dst.write(0) };
}

/// A path's root for canonicalizing: `X:\`, `X:`, `\\` (UNC) or `\`.
fn canonical_root(s: &[u16]) -> usize {
    match s {
        [_, COLON, BACKSLASH, ..] => 3,
        [_, COLON, ..] => 2,
        [BACKSLASH, BACKSLASH, ..] => 2,
        [BACKSLASH, ..] => 1,
        _ => 0,
    }
}

/// PathCanonicalize's work: `.` components dropped, `..` removes the one
/// before (never the root; kept in a relative path with nothing before
/// it); only backslashes separate components (`/` is part of a name); a
/// trailing backslash is kept after a kept component; empty components
/// are dropped. An empty result is `\`. Into `out`; its length, or None
/// when it does not fit.
fn canonicalize(s: &[u16], out: &mut [u16; 2 * MAX_PATH]) -> Option<usize> {
    let root = canonical_root(s);
    out.get_mut(..root)?.copy_from_slice(&s[..root]);
    let mut n = root;
    // Where each kept component's text ends before it (to pop on `..`).
    let mut starts = [0usize; MAX_PATH];
    let mut depth = 0;
    // Leading `..` components kept in a relative path (never popped).
    let mut kept_up = 0;
    let body = &s[root..];
    let trailing = body.last() == Some(&BACKSLASH);
    let mut last_kept = false;
    for part in body.split(|&c| c == BACKSLASH) {
        if part.is_empty() {
            continue;
        }
        if part == [DOT] {
            last_kept = false;
            continue;
        }
        let up = part == [DOT, DOT];
        if up && (depth > kept_up || root > 0) {
            if depth > kept_up {
                depth -= 1;
                n = starts[depth];
            }
            last_kept = false;
            continue;
        }
        let at = if n > root { n + 1 } else { n };
        if at + part.len() >= out.len() || depth == starts.len() {
            return None;
        }
        if n > root {
            out[n] = BACKSLASH;
        }
        starts[depth] = n;
        depth += 1;
        if up {
            kept_up += 1;
        }
        out[at..at + part.len()].copy_from_slice(part);
        n = at + part.len();
        last_kept = true;
    }
    if trailing && last_kept {
        if n + 1 >= out.len() {
            return None;
        }
        out[n] = BACKSLASH;
        n += 1;
    }
    if n == 0 {
        out[0] = BACKSLASH;
        n = 1;
    }
    Some(n)
}

/// PathCanonicalizeW: `src` canonicalized into `dst` (MAX_PATH
/// characters); FALSE with ERROR_INVALID_PARAMETER for a NULL argument,
/// ERROR_FILENAME_EXCED_RANGE when it does not fit.
#[unsafe(no_mangle)]
pub extern "system" fn PathCanonicalizeW(dst: *mut u16, src: *const u16) -> i32 {
    if dst.is_null() || src.is_null() {
        if !dst.is_null() {
            clear(dst);
        }
        set_last_error(ERROR_INVALID_PARAMETER);
        return 0;
    }
    let mut out = [0u16; 2 * MAX_PATH];
    match canonicalize(chars(src), &mut out) {
        Some(n) if n < MAX_PATH => {
            store(dst, &out[..n]);
            1
        }
        _ => {
            clear(dst);
            set_last_error(ERROR_FILENAME_EXCED_RANGE);
            0
        }
    }
}

/// PathCombineW: `file` after `dir`, canonicalized, into `dst` (MAX_PATH):
/// a `file` with a drive or a UNC start is the result by itself, one
/// starting with `\` goes under `dir`'s drive; NULL (and an empty `dst`)
/// when both are NULL or the result does not fit. `dst` may be `dir`.
#[unsafe(no_mangle)]
pub extern "system" fn PathCombineW(dst: *mut u16, dir: *const u16, file: *const u16) -> *mut u16 {
    if dst.is_null() {
        return dst;
    }
    if dir.is_null() && file.is_null() {
        clear(dst);
        return core::ptr::null_mut();
    }
    let (d, f) = (chars(dir), chars(file));
    let mut joined = [0u16; 2 * MAX_PATH + 2];
    let mut n = 0;
    let mut push = |s: &[u16]| -> Option<()> {
        joined.get_mut(n..n + s.len())?.copy_from_slice(s);
        n += s.len();
        Some(())
    };
    let drive = |s: &[u16]| s.len() >= 2 && s[1] == COLON;
    let unc = |s: &[u16]| s.starts_with(&[BACKSLASH, BACKSLASH]);
    let pushed = if !f.is_empty() && (drive(f) || unc(f)) {
        push(f)
    } else if f.first() == Some(&BACKSLASH) {
        let root: &[u16] = if drive(d) { &d[..2] } else { &[] };
        push(root).and_then(|()| push(f))
    } else {
        let separate = !d.is_empty() && !f.is_empty() && d.last() != Some(&BACKSLASH);
        push(d)
            .and_then(|()| if separate { push(&[BACKSLASH]) } else { Some(()) })
            .and_then(|()| push(f))
    };
    let mut out = [0u16; 2 * MAX_PATH];
    match pushed.and_then(|()| canonicalize(&joined[..n], &mut out)) {
        Some(len) if len < MAX_PATH => {
            store(dst, &out[..len]);
            dst
        }
        _ => {
            clear(dst);
            core::ptr::null_mut()
        }
    }
}

/// The index of the extension's `.` in `s` (the last component's last
/// `.`, with no space after it), or `s.len()`.
fn extension(s: &[u16]) -> usize {
    let mut dot = None;
    for (i, &c) in s.iter().enumerate() {
        match c {
            BACKSLASH | 0x20 => dot = None,
            DOT => dot = Some(i),
            _ => {}
        }
    }
    dot.unwrap_or(s.len())
}

/// PathFindExtensionW: the extension's `.`, or the terminating NUL.
#[unsafe(no_mangle)]
pub extern "system" fn PathFindExtensionW(path: *const u16) -> *const u16 {
    if path.is_null() {
        return path;
    }
    path.wrapping_add(extension(chars(path)))
}

/// PathRemoveExtensionW: the extension, if any, removed.
#[unsafe(no_mangle)]
pub extern "system" fn PathRemoveExtensionW(path: *mut u16) {
    if path.is_null() {
        return;
    }
    let s = chars(path);
    let at = extension(s);
    if at < s.len() {
        // SAFETY: inside the caller's string.
        unsafe { path.add(at).write(0) };
    }
}

/// PathRemoveBackslashW: a trailing backslash removed unless it is a
/// root's (`C:\`, `\`); returns the new end, or the last character when
/// nothing was removed.
#[unsafe(no_mangle)]
pub extern "system" fn PathRemoveBackslashW(path: *mut u16) -> *mut u16 {
    if path.is_null() {
        return path;
    }
    let s = chars(path);
    let Some(last) = s.len().checked_sub(1) else { return path };
    let is_root = s.len() == 1 || (s.len() == 3 && s[1] == COLON);
    if s[last] == BACKSLASH && !is_root {
        // SAFETY: inside the caller's string.
        unsafe { path.add(last).write(0) };
    }
    path.wrapping_add(last)
}

/// PathIsFileSpecW: TRUE when the path has no ':' or '\\' (a file
/// name alone, as documented).
#[unsafe(no_mangle)]
pub extern "system" fn PathIsFileSpecW(path: *const u16) -> i32 {
    if path.is_null() {
        return 0;
    }
    i32::from(!chars(path).iter().any(|&c| c == u16::from(b':') || c == u16::from(b'\\')))
}

/// PathIsRelativeW: TRUE unless the path starts with a backslash or a
/// drive (`X:`); NULL and "" are relative.
#[unsafe(no_mangle)]
pub extern "system" fn PathIsRelativeW(path: *const u16) -> i32 {
    let s = chars(path);
    let absolute = s.first() == Some(&BACKSLASH) || (s.len() >= 2 && s[1] == COLON);
    i32::from(!absolute)
}

/// The length of the URL scheme `s` starts with (before its colon): a
/// letter, then letters, digits, `+`, `-` or `.`, two characters at
/// least (one is a drive letter).
pub(crate) fn scheme_len(s: &[u16]) -> Option<usize> {
    let ascii = |c: u16| u8::try_from(c).ok();
    if !ascii(*s.first()?)?.is_ascii_alphabetic() {
        return None;
    }
    let n = s.iter().position(|&c| {
        !ascii(c).is_some_and(|b| b.is_ascii_alphanumeric() || b"+-.".contains(&b))
    })?;
    (n >= 2 && s[n] == COLON).then_some(n)
}

/// PathIsURLW: whether the string starts with a URL scheme (`http:`,
/// `about:`, any of two or more characters) and its colon.
#[unsafe(no_mangle)]
pub extern "system" fn PathIsURLW(path: *const u16) -> i32 {
    i32::from(scheme_len(chars(path)).is_some())
}

/// A character folded for case-insensitive matching: ASCII and Latin-1
/// letters.
pub(crate) fn fold(c: u16) -> u16 {
    match c {
        0x41..=0x5a => c + 0x20,
        0xc0..=0xde if c != 0xd7 => c + 0x20,
        _ => c,
    }
}

/// `name` against a pattern of `*` (any run) and `?` (one character),
/// case ignored.
fn wildcard(name: &[u16], pattern: &[u16]) -> bool {
    let (mut n, mut p) = (0, 0);
    // The last `*` seen and where in `name` its run ends so far.
    let mut star: Option<(usize, usize)> = None;
    while n < name.len() {
        if p < pattern.len() && pattern[p] == u16::from(b'*') {
            star = Some((p, n));
            p += 1;
        } else if p < pattern.len()
            && (pattern[p] == u16::from(b'?') || fold(pattern[p]) == fold(name[n]))
        {
            p += 1;
            n += 1;
        } else if let Some((sp, sn)) = star {
            p = sp + 1;
            n = sn + 1;
            star = Some((sp, sn + 1));
        } else {
            return false;
        }
    }
    pattern[p..].iter().all(|&c| c == u16::from(b'*'))
}

/// PathMatchSpecW: the whole path against each `;`-separated pattern of
/// `spec` (leading spaces skipped; trailing ones are part of the
/// pattern); `*.*` matches everything.
#[unsafe(no_mangle)]
pub extern "system" fn PathMatchSpecW(path: *const u16, spec: *const u16) -> i32 {
    if path.is_null() || spec.is_null() {
        return 0;
    }
    let (name, spec) = (chars(path), chars(spec));
    if spec == [u16::from(b'*'), DOT, u16::from(b'*')] {
        return 1;
    }
    let matched = spec.split(|&c| c == u16::from(b';')).any(|pattern| {
        let start = pattern.iter().position(|&c| c != 0x20).unwrap_or(pattern.len());
        let pattern = &pattern[start..];
        !pattern.is_empty() && wildcard(name, pattern)
    });
    i32::from(matched)
}

/// PathSearchAndQualifyW: the full path of `path` in `buf` (`size`
/// characters); a bare file name is first looked for along the search
/// path (SearchPathW), else taken in the current directory.
#[unsafe(no_mangle)]
pub extern "system" fn PathSearchAndQualifyW(path: *const u16, buf: *mut u16, size: u32) -> i32 {
    if path.is_null() || buf.is_null() || size == 0 {
        return 0;
    }
    let s = chars(path);
    let bare = !s.iter().any(|&c| c == BACKSLASH || c == SLASH || c == COLON);
    if bare {
        // SAFETY: the caller's strings and buffer of `size` characters.
        let n = unsafe {
            SearchPathW(
                core::ptr::null(),
                path,
                core::ptr::null(),
                size,
                buf,
                core::ptr::null_mut(),
            )
        };
        if n != 0 && n < size {
            return 1;
        }
    }
    // SAFETY: as above.
    let n = unsafe { GetFullPathNameW(path, size, buf, core::ptr::null_mut()) };
    i32::from(n != 0 && n < size)
}

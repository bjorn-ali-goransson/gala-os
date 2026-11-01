//! Path helpers (shlwapi.h), on NUL-terminated UTF-16 paths in the
//! caller's MAX_PATH buffers.

#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetFileAttributesW(path: *const u16) -> u32;
}

const MAX_PATH: usize = 260;
const BACKSLASH: u16 = b'\\' as u16;

fn len(p: *const u16) -> usize {
    let mut n = 0;
    // SAFETY: the caller's NUL-terminated string.
    while unsafe { *p.add(n) } != 0 {
        n += 1;
    }
    n
}

/// The length of the root at the start of `s` (`C:\`, `C:`, `\`, or 0).
fn root_len(s: &[u16]) -> usize {
    let drive = s.len() >= 2 && s[1] == u16::from(b':');
    match (drive, s.get(2) == Some(&BACKSLASH)) {
        (true, true) => 3,
        (true, false) => 2,
        _ if s.first() == Some(&BACKSLASH) => 1,
        _ => 0,
    }
}

/// PathRemoveFileSpecW: removes the last component and its backslash (a
/// root keeps its own); FALSE if nothing was removed.
#[unsafe(no_mangle)]
pub extern "system" fn PathRemoveFileSpecW(path: *mut u16) -> i32 {
    if path.is_null() {
        return 0;
    }
    let n = len(path);
    // SAFETY: the caller's string of `n` characters.
    let s = unsafe { core::slice::from_raw_parts(path, n) };
    let root = root_len(s);
    let cut = match s.iter().rposition(|&c| c == BACKSLASH) {
        Some(i) if i < root => root,
        Some(i) => i,
        None => root,
    };
    if cut >= n {
        return 0;
    }
    // SAFETY: inside the caller's string.
    unsafe { path.add(cut).write(0) };
    1
}

/// PathAppendW: `more` after `path` with one backslash between them (a
/// leading backslash of `more` is dropped); FALSE if the result would not
/// fit in MAX_PATH.
#[unsafe(no_mangle)]
pub extern "system" fn PathAppendW(path: *mut u16, more: *const u16) -> i32 {
    if path.is_null() || more.is_null() {
        return 0;
    }
    let (n, m) = (len(path), len(more));
    // SAFETY: the caller's strings.
    let (head, tail) =
        unsafe { (core::slice::from_raw_parts(path, n), core::slice::from_raw_parts(more, m)) };
    let tail = tail.strip_prefix(&[BACKSLASH]).unwrap_or(tail);
    let separator = usize::from(n != 0 && head[n - 1] != BACKSLASH && !tail.is_empty());
    if n + separator + tail.len() + 1 > MAX_PATH {
        return 0;
    }
    // SAFETY: the caller's MAX_PATH buffer (checked to fit).
    unsafe {
        if separator == 1 {
            path.add(n).write(BACKSLASH);
        }
        core::ptr::copy(tail.as_ptr(), path.add(n + separator), tail.len());
        path.add(n + separator + tail.len()).write(0);
    }
    1
}

/// PathFindFileNameW: the last component (after the last `\`, `/` or `:`).
#[unsafe(no_mangle)]
pub extern "system" fn PathFindFileNameW(path: *const u16) -> *const u16 {
    if path.is_null() {
        return path;
    }
    let n = len(path);
    // SAFETY: the caller's string of `n` characters.
    let s = unsafe { core::slice::from_raw_parts(path, n) };
    let separators = [BACKSLASH, u16::from(b'/'), u16::from(b':')];
    // A trailing separator does not start an (empty) last component.
    let body = s.strip_suffix(&[BACKSLASH]).unwrap_or(s);
    match body.iter().rposition(|c| separators.contains(c)) {
        Some(i) => path.wrapping_add(i + 1),
        None => path,
    }
}

/// PathFileExistsW: whether the file or directory exists.
#[unsafe(no_mangle)]
pub extern "system" fn PathFileExistsW(path: *const u16) -> i32 {
    // SAFETY: the caller's path, passed on.
    i32::from(!path.is_null() && unsafe { GetFileAttributesW(path) } != u32::MAX)
}

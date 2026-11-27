//! Path helpers (shlwapi.h), on NUL-terminated UTF-16 paths in the
//! caller's MAX_PATH buffers.

#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use crate::paths_more::{chars, fold};

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GetFileAttributesW(path: *const u16) -> u32;
}

pub(crate) const MAX_PATH: usize = 260;
pub(crate) const BACKSLASH: u16 = b'\\' as u16;

pub(crate) fn len(p: *const u16) -> usize {
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
/// PathBuildRootW(szRoot, iDrive): `X:\` for drive 0 (A) to 25 (Z) in
/// the caller's buffer; another drive number leaves it empty (as Windows
/// since Vista, Wine's shlwapi path.c test on Windows); szRoot returned.
#[unsafe(no_mangle)]
pub extern "system" fn PathBuildRootW(root: *mut u16, drive: i32) -> *mut u16 {
    if !root.is_null() {
        let text: [u16; 4] = if (0..26).contains(&drive) {
            [u16::from(b'A') + drive as u16, u16::from(b':'), u16::from(b'\\'), 0]
        } else {
            [0; 4]
        };
        let n = if text[0] == 0 { 1 } else { 4 };
        // SAFETY: the caller's buffer of at least four characters.
        unsafe { core::ptr::copy_nonoverlapping(text.as_ptr(), root, n) };
    }
    root
}

/// PathBuildRootA: PathBuildRootW's in ANSI.
#[unsafe(no_mangle)]
pub extern "system" fn PathBuildRootA(root: *mut u8, drive: i32) -> *mut u8 {
    if !root.is_null() {
        let text: [u8; 4] =
            if (0..26).contains(&drive) { [b'A' + drive as u8, b':', b'\\', 0] } else { [0; 4] };
        let n = if text[0] == 0 { 1 } else { 4 };
        // SAFETY: the caller's buffer of at least four bytes.
        unsafe { core::ptr::copy_nonoverlapping(text.as_ptr(), root, n) };
    }
    root
}

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

/// A drive letter: a letter of Latin-1. (Build 26300 answers so over
/// all of U+0001 to U+FFFF: `Ä:\` is a root and Greek `Α:\` is not. The
/// two signs that sit among those letters, `×` and `÷`, are not ones.)
fn is_drive_letter(c: u16) -> bool {
    match c {
        0x41..=0x5a | 0x61..=0x7a => true,
        0xc0..=0xff => c != 0xd7 && c != 0xf7,
        _ => false,
    }
}

/// `X:\` and nothing after it.
fn is_drive_root(s: &[u16]) -> bool {
    matches!(s, [drive, colon, slash]
        if is_drive_letter(*drive) && *colon == u16::from(b':') && *slash == BACKSLASH)
}

/// What follows two backslashes is a root while at most one backslash
/// separates the server from the share and none ends it: `\\`,
/// `\\server` and `\\server\share` are roots, `\\server\`,
/// `\\server\share\` and `\\server\share\dir` are not. Only backslashes
/// separate, so a `/` and a trailing space are part of a name.
fn is_unc_body_root(body: &[u16]) -> bool {
    body.last() != Some(&BACKSLASH) && body.iter().filter(|&&c| c == BACKSLASH).count() < 2
}

/// `s` without `prefix` (lower-case ASCII, matched with the case of
/// `s` folded), or None if it does not start with it.
fn strip_fold<'a>(s: &'a [u16], prefix: &[u8]) -> Option<&'a [u16]> {
    if s.len() < prefix.len() {
        return None;
    }
    let (head, rest) = s.split_at(prefix.len());
    head.iter().zip(prefix).all(|(&c, &p)| fold(c) == u16::from(p)).then_some(rest)
}

/// A volume's own root after the extended prefix: `Volume{`, a GUID of
/// 8-4-4-4-12 hex digits (either case), `}` and one backslash, and
/// nothing else.
fn is_volume_root(s: &[u16]) -> bool {
    let Some(guid) = strip_fold(s, b"volume{") else {
        return false;
    };
    let Some(guid) = guid.strip_suffix(&[u16::from(b'}'), BACKSLASH]) else {
        return false;
    };
    let mut at = 0;
    for (group, digits) in [8, 4, 4, 4, 12].into_iter().enumerate() {
        if group > 0 {
            if guid.get(at) != Some(&u16::from(b'-')) {
                return false;
            }
            at += 1;
        }
        for _ in 0..digits {
            match guid.get(at) {
                Some(&c) if u8::try_from(c).is_ok_and(|b| b.is_ascii_hexdigit()) => at += 1,
                _ => return false,
            }
        }
    }
    at == guid.len()
}

/// PathIsRootW: whether the path is a root and nothing under one, read
/// from the string alone - it never looks at the file system and does
/// not care how long the path is. A drive's `X:\` (and not `X:`), one
/// backslash by itself, or two backslashes and a UNC body
/// (`is_unc_body_root`); after the extended prefix, which is exactly
/// `\\?\`, a drive's `X:\`, a volume's `Volume{<guid>}\` or `UNC\` and
/// a body read as two backslashes' body is. A `?` right after the two
/// backslashes starts that prefix and nothing else, so `\\?a` is not a
/// root where `\\.a` is. NULL and the empty string are not roots.
/// (Measured on build 26300; shlwapi_test's `root_checks` holds the
/// shapes, since the documentation says only "a root".)
#[unsafe(no_mangle)]
pub extern "system" fn PathIsRootW(path: *const u16) -> i32 {
    let s = chars(path);
    let question = u16::from(b'?');
    let answer = if let Some(rest) = s.strip_prefix(&[BACKSLASH, BACKSLASH, question, BACKSLASH]) {
        match strip_fold(rest, b"unc\\") {
            Some(body) => is_unc_body_root(body),
            None => is_drive_root(rest) || is_volume_root(rest),
        }
    } else if s.starts_with(&[BACKSLASH, BACKSLASH, question]) {
        false
    } else if let Some(body) = s.strip_prefix(&[BACKSLASH, BACKSLASH]) {
        is_unc_body_root(body)
    } else {
        is_drive_root(s) || s == [BACKSLASH]
    };
    i32::from(answer)
}

/// PathFileExistsW: whether the file or directory exists.
#[unsafe(no_mangle)]
pub extern "system" fn PathFileExistsW(path: *const u16) -> i32 {
    // SAFETY: the caller's path, passed on.
    i32::from(!path.is_null() && unsafe { GetFileAttributesW(path) } != u32::MAX)
}

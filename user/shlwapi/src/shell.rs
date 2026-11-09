//! The path and string helpers ODT's setup.exe imports beside the others
//! (shlwapi.h), as Microsoft documents them and Wine's conformance tests
//! (dlls/shlwapi/tests path.c test_PathGetDriveNumber, string.c
//! test_StrXXX_overflows, run on Windows) show:
//!
//! - PathIsUNCW: the path starts with two backslashes.
//! - PathGetArgsW: what follows the first space outside double quotes,
//!   or the end of the string; NULL for NULL.
//! - PathGetCharTypeW: GCT_INVALID for NUL, controls and `<>"|/`;
//!   GCT_WILD for `*?`; GCT_SEPARATOR for `\:`; otherwise GCT_LFNCHAR,
//!   with GCT_SHORTCHAR for what an 8.3 name may hold (Wine's table of
//!   Windows' answers: letters, digits and every odd-valued ASCII
//!   punctuation but `;`, plus `$&(.@^'\``, and everything from 126 up).
//! - PathGetDriveNumberW: 0 to 25 for `A:` to `Z:` (either case), after
//!   an optional `\\?\`; -1 otherwise, NULL included.
//! - StrRetToBufW: the STRRET's string (STRRET_WSTR: its CoTaskMem
//!   string, then freed; STRRET_CSTR: its ANSI text; STRRET_OFFSET: ANSI
//!   text at that offset in the item ID list) into `cch` characters with
//!   a NUL. Too long a STRRET_WSTR is E_NOT_SUFFICIENT_BUFFER with an
//!   empty buffer (Vista's answer); no string or another type E_FAIL with
//!   an empty buffer.

#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use crate::paths::BACKSLASH;
use crate::paths_more::chars;

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn MultiByteToWideChar(
        code_page: u32,
        flags: u32,
        text: *const u8,
        length: i32,
        out: *mut u16,
        out_length: i32,
    ) -> i32;
    fn LoadLibraryW(name: *const u16) -> usize;
    fn GetProcAddress(module: usize, name: *const u8) -> usize;
}

const GCT_INVALID: u32 = 0;
const GCT_LFNCHAR: u32 = 1;
const GCT_SHORTCHAR: u32 = 2;
const GCT_WILD: u32 = 4;
const GCT_SEPARATOR: u32 = 8;

const STRRET_WSTR: u32 = 0;
const STRRET_OFFSET: u32 = 1;
const STRRET_CSTR: u32 = 2;

const S_OK: i32 = 0;
const E_FAIL: i32 = 0x8000_4005_u32 as i32;
const E_NOT_SUFFICIENT_BUFFER: i32 = 0x8007_007a_u32 as i32;
const CP_ACP: u32 = 0;

#[unsafe(no_mangle)]
pub extern "system" fn PathIsUNCW(path: *const u16) -> i32 {
    i32::from(chars(path).starts_with(&[BACKSLASH, BACKSLASH]))
}

#[unsafe(no_mangle)]
pub extern "system" fn PathGetArgsW(path: *const u16) -> *const u16 {
    if path.is_null() {
        return path;
    }
    let s = chars(path);
    let mut quoted = false;
    for (i, &c) in s.iter().enumerate() {
        if c == u16::from(b' ') && !quoted {
            return path.wrapping_add(i + 1);
        }
        if c == u16::from(b'"') {
            quoted = !quoted;
        }
    }
    path.wrapping_add(s.len())
}

#[unsafe(no_mangle)]
pub extern "system" fn PathGetCharTypeW(c: u16) -> u32 {
    let Ok(b) = u8::try_from(c) else {
        return GCT_LFNCHAR | GCT_SHORTCHAR;
    };
    match b {
        0..=0x1f | b'<' | b'>' | b'"' | b'|' | b'/' => GCT_INVALID,
        b'*' | b'?' => GCT_WILD,
        b'\\' | b':' => GCT_SEPARATOR,
        126.. => GCT_LFNCHAR | GCT_SHORTCHAR,
        _ => {
            let short =
                (b & 1 == 1 && b != b';') || b.is_ascii_alphanumeric() || b"$&(.@^'`".contains(&b);
            GCT_LFNCHAR | if short { GCT_SHORTCHAR } else { 0 }
        }
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn PathGetDriveNumberW(path: *const u16) -> i32 {
    let s = chars(path);
    let prefix = [BACKSLASH, BACKSLASH, u16::from(b'?'), BACKSLASH];
    let s = s.strip_prefix(&prefix).unwrap_or(s);
    match s {
        [d, c, ..] if *c == u16::from(b':') => match u8::try_from(*d) {
            Ok(d) if d.is_ascii_alphabetic() => i32::from(d.to_ascii_uppercase() - b'A'),
            _ => -1,
        },
        _ => -1,
    }
}

/// STRRET (shtypes.h): the type, then a union of a pointer, an offset and
/// MAX_PATH ANSI characters (pointer-aligned: offset 8 on x64, 4 on x86).
#[repr(C)]
pub struct StrRet {
    kind: u32,
    data: StrRetData,
}

#[repr(C)]
union StrRetData {
    ole: *mut u16,
    offset: u32,
    text: [u8; 260],
}

const _: () = {
    let x86 = cfg!(target_arch = "x86");
    assert!(core::mem::size_of::<StrRet>() == if x86 { 264 } else { 272 });
    assert!(core::mem::offset_of!(StrRet, data) == if x86 { 4 } else { 8 });
};

/// ole32's CoTaskMemFree, loaded when first needed (a static import would
/// make every program that loads shlwapi load ole32).
fn co_task_mem_free(p: *mut u16) {
    const OLE32: [u16; 10] = [111, 108, 101, 51, 50, 46, 100, 108, 108, 0]; // "ole32.dll"
    // SAFETY: NUL-terminated names; CoTaskMemFree(void *).
    unsafe {
        let module = LoadLibraryW(OLE32.as_ptr());
        let f =
            if module == 0 { 0 } else { GetProcAddress(module, c"CoTaskMemFree".as_ptr().cast()) };
        if f != 0 {
            let f: extern "system" fn(*mut u16) = core::mem::transmute(f);
            f(p);
        }
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn StrRetToBufW(
    strret: *mut StrRet,
    pidl: *const u8,
    buf: *mut u16,
    cch: u32,
) -> i32 {
    if buf.is_null() || cch == 0 {
        return E_FAIL;
    }
    // SAFETY: the caller's buffer of `cch` characters.
    let out = unsafe { core::slice::from_raw_parts_mut(buf, cch as usize) };
    out[0] = 0;
    if strret.is_null() {
        return E_FAIL;
    }
    // SAFETY: the caller's STRRET.
    let r = unsafe { &mut *strret };
    // ANSI text into the buffer, cut to fit.
    let ansi = |p: *const u8, most: usize, out: &mut [u16]| {
        // SAFETY: the caller's text of at most `most` bytes.
        let n = (0..most).take_while(|&i| unsafe { *p.add(i) } != 0).count();
        // SAFETY: `n` bytes in, at most `cch - 1` characters out.
        let got = unsafe {
            MultiByteToWideChar(CP_ACP, 0, p, n as i32, out.as_mut_ptr(), out.len() as i32 - 1)
        };
        out[got.max(0) as usize] = 0;
    };
    match r.kind {
        STRRET_WSTR => {
            // SAFETY: the union as its type says.
            let ole = unsafe { r.data.ole };
            if ole.is_null() {
                return E_FAIL;
            }
            let s = chars(ole);
            let result = if s.len() < out.len() {
                out[..s.len()].copy_from_slice(s);
                out[s.len()] = 0;
                S_OK
            } else {
                let n = out.len() - 1;
                out[..n].copy_from_slice(&s[..n]);
                out[n] = 0;
                out[0] = 0;
                E_NOT_SUFFICIENT_BUFFER
            };
            co_task_mem_free(ole);
            r.data.ole = core::ptr::null_mut();
            result
        }
        STRRET_CSTR => {
            // SAFETY: the union as its type says.
            let text = unsafe { r.data.text };
            ansi(text.as_ptr(), text.len(), out);
            S_OK
        }
        STRRET_OFFSET => {
            if pidl.is_null() {
                return E_FAIL;
            }
            // SAFETY: the union as its type says; the text inside the
            // caller's item ID list.
            let p = unsafe { pidl.add(r.data.offset as usize) };
            ansi(p, usize::MAX, out);
            S_OK
        }
        _ => E_FAIL,
    }
}

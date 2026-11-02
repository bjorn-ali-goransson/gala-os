//! gala's user32.dll: windows on the desktop, classes, messages and
//! painting (window.rs), and the text helpers console programs import.

#![no_std]
#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

mod accel;
mod controls;
mod hook;
mod icon;
mod keyboard;
mod menu;
mod message;
mod monitor;
mod rect;
mod spi;
mod text;
mod timer;
mod tree;
mod unimplemented;
mod window;

#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memmove(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
    fn strlen(s: *const u8) -> usize;
    fn __CxxFrameHandler3();
    // The stack probe for frames over a page (ntdll exports it).
    fn __chkstk();
}

/// MSVC-target codegen references this marker symbol directly, so it cannot
/// come from an import. (It becomes a harmless extra data export.)
#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn WaitForMultipleObjects(count: u32, handles: *const usize, all: i32, ms: u32) -> u32;
    fn FindResourceExW(module: usize, kind: usize, name: usize, lang: u16) -> usize;
    fn LoadResource(module: usize, res: usize) -> *const u16;
    fn SizeofResource(module: usize, res: usize) -> u32;
    fn WideCharToMultiByte(
        cp: u32,
        flags: u32,
        w: *const u16,
        n: i32,
        s: *mut u8,
        m: i32,
        default: *const u8,
        used: *mut i32,
    ) -> i32;
}

// ---- Messages and windows: gala has no windows, so no messages ----

/// MsgWaitForMultipleObjects: a plain wait, since no message can arrive.
#[unsafe(no_mangle)]
pub extern "system" fn MsgWaitForMultipleObjects(
    count: u32,
    handles: *const usize,
    all: i32,
    ms: u32,
    _wake_mask: u32,
) -> u32 {
    // SAFETY: the caller's handle array.
    unsafe { WaitForMultipleObjects(count, handles, all, ms) }
}

#[unsafe(no_mangle)]
pub extern "system" fn IsIconic(_window: usize) -> i32 {
    0
}

/// CharToOemBuffA(src, dst, length): ANSI to the OEM code page. gala's
/// code pages are the same (UTF-8 for ANSI text), so this copies.
#[unsafe(no_mangle)]
pub extern "system" fn CharToOemBuffA(src: *const u8, dst: *mut u8, length: u32) -> i32 {
    if src.is_null() || dst.is_null() {
        return 0;
    }
    // SAFETY: the caller's buffers of `length` bytes (they may be the same).
    unsafe { memmove(dst, src, length as usize) };
    1
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn SetLastError(code: u32);
    fn GetStdHandle(which: u32) -> usize;
    fn WriteFile(h: usize, buf: *const u8, len: u32, written: *mut u32, ov: usize) -> i32;
}

/// GetProcessWindowStation: gala has no window stations (NULL), which is
/// how OpenSSL tells it is not on an interactive desktop.
#[unsafe(no_mangle)]
pub extern "system" fn GetProcessWindowStation() -> usize {
    // SAFETY: plain call.
    unsafe { SetLastError(50) }; // ERROR_NOT_SUPPORTED
    0
}

#[unsafe(no_mangle)]
pub extern "system" fn GetUserObjectInformationW(
    _object: usize,
    _index: i32,
    _info: *mut u8,
    _length: u32,
    _needed: *mut u32,
) -> i32 {
    // SAFETY: plain call.
    unsafe { SetLastError(6) }; // ERROR_INVALID_HANDLE
    0
}

/// GetUserObjectInformationA: as the W form (gala has no window stations).
#[unsafe(no_mangle)]
pub extern "system" fn GetUserObjectInformationA(
    object: usize,
    index: i32,
    info: *mut u8,
    length: u32,
    needed: *mut u32,
) -> i32 {
    GetUserObjectInformationW(object, index, info, length, needed)
}

/// CharNextExA: the next character of an ANSI string (gala's ANSI code
/// page is UTF-8: the next UTF-8 sequence); at the NUL, the NUL itself.
#[unsafe(no_mangle)]
pub extern "system" fn CharNextExA(_code_page: u16, s: *const u8, _flags: u32) -> *const u8 {
    if s.is_null() {
        return s;
    }
    // SAFETY: the caller's NUL-terminated string.
    unsafe {
        if *s == 0 {
            return s;
        }
        let mut p = s.add(1);
        while *p & 0xc0 == 0x80 {
            p = p.add(1);
        }
        p
    }
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn LCMapStringW(lcid: u32, flags: u32, src: *const u16, n: i32, dst: *mut u16, m: i32) -> i32;
}

const LCMAP_LOWERCASE: u32 = 0x100;
const LCMAP_UPPERCASE: u32 = 0x200;

/// Maps `n` characters at `s` in place (LCMapStringW of the user locale,
/// through a buffer: source and destination may not overlap), in chunks.
fn map_case(s: *mut u16, n: usize, flags: u32) {
    let mut done = 0;
    while done < n {
        let chunk = (n - done).min(256);
        let mut buf = [0u16; 256];
        // SAFETY: `chunk` characters of the caller's string, and our buffer.
        unsafe {
            let src = s.add(done);
            if LCMapStringW(0x0400, flags, src, chunk as i32, buf.as_mut_ptr(), 256) as usize
                == chunk
            {
                core::ptr::copy_nonoverlapping(buf.as_ptr(), src, chunk);
            }
        }
        done += chunk;
    }
}

/// The length of the NUL-terminated string at `s`.
fn wide_len(s: *const u16) -> usize {
    let mut n = 0;
    // SAFETY: the caller's NUL-terminated string.
    while unsafe { *s.add(n) } != 0 {
        n += 1;
    }
    n
}

/// CharUpperW: a string converted in place (returned), or, when the high
/// word of the argument is zero, the one character in its low word.
#[unsafe(no_mangle)]
pub extern "system" fn CharUpperW(s: *mut u16) -> *mut u16 {
    if (s as usize) >> 16 == 0 {
        let mut c = s as usize as u16;
        map_case(&mut c, 1, LCMAP_UPPERCASE);
        return c as usize as *mut u16;
    }
    map_case(s, wide_len(s), LCMAP_UPPERCASE);
    s
}

/// CharUpperBuffW: `n` characters converted in place; returns `n`.
#[unsafe(no_mangle)]
pub extern "system" fn CharUpperBuffW(s: *mut u16, n: u32) -> u32 {
    if s.is_null() {
        return 0;
    }
    map_case(s, n as usize, LCMAP_UPPERCASE);
    n
}

/// CharLowerBuffW: as CharUpperBuffW, to lower case.
#[unsafe(no_mangle)]
pub extern "system" fn CharLowerBuffW(s: *mut u16, n: u32) -> u32 {
    if s.is_null() {
        return 0;
    }
    map_case(s, n as usize, LCMAP_LOWERCASE);
    n
}

/// CharNextW: the next character (at the NUL, the NUL itself).
#[unsafe(no_mangle)]
pub extern "system" fn CharNextW(s: *const u16) -> *const u16 {
    // SAFETY: the caller's NUL-terminated string.
    if s.is_null() || unsafe { *s } == 0 { s } else { s.wrapping_add(1) }
}

/// CharNextA: CharNextExA in the ANSI code page.
#[unsafe(no_mangle)]
pub extern "system" fn CharNextA(s: *const u8) -> *const u8 {
    CharNextExA(0, s, 0)
}

/// MessageBoxA: as MessageBoxW (the text to standard error, IDOK).
#[unsafe(no_mangle)]
pub extern "system" fn MessageBoxA(
    window: usize,
    text: *const u8,
    caption: *const u8,
    kind: u32,
) -> i32 {
    let (mut t, mut c) = ([0u16; 1024], [0u16; 256]);
    let widen = |s: *const u8, out: &mut [u16]| -> *const u16 {
        if s.is_null() {
            return core::ptr::null();
        }
        let mut n = 0;
        // SAFETY: the caller's NUL-terminated string (ASCII is kept, other
        // bytes become '?').
        unsafe {
            while *s.add(n) != 0 && n < out.len() - 1 {
                let b = *s.add(n);
                out[n] = if b < 0x80 { b.into() } else { u16::from(b'?') };
                n += 1;
            }
        }
        out[n] = 0;
        out.as_ptr()
    };
    let (text, caption) = (widen(text, &mut t), widen(caption, &mut c));
    MessageBoxW(window, text, caption, kind)
}

/// MessageBoxW: no desktop; the caption and text go to standard error and
/// the answer is IDOK.
#[unsafe(no_mangle)]
pub extern "system" fn MessageBoxW(
    _window: usize,
    text: *const u16,
    caption: *const u16,
    _kind: u32,
) -> i32 {
    for s in [caption, text] {
        if s.is_null() {
            continue;
        }
        let mut buf = [0u8; 1024];
        let mut n = 0;
        // SAFETY: the caller's NUL-terminated UTF-16 string.
        unsafe {
            let mut i = 0;
            while *s.add(i) != 0 && n < buf.len() - 2 {
                let c = *s.add(i);
                buf[n] = if c < 0x80 { c as u8 } else { b'?' };
                n += 1;
                i += 1;
            }
        }
        buf[n] = b'\r';
        buf[n + 1] = b'\n';
        let mut written = 0;
        // SAFETY: a valid buffer.
        unsafe {
            WriteFile(GetStdHandle(-12i32 as u32), buf.as_ptr(), (n + 2) as u32, &mut written, 0)
        };
    }
    1 // IDOK
}

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

/// String `id` of the module's string table: RT_STRING block id / 16 + 1
/// holds 16 counted UTF-16 strings (ids 16 * (block - 1) and up), as the
/// resource format documents. Its text (not terminated), or None.
fn string_resource(module: usize, id: u32) -> Option<&'static [u16]> {
    let block = (id >> 4) as usize + 1;
    // SAFETY: plain calls; the resource's words inside the mapped image.
    unsafe {
        let res = FindResourceExW(module, 6, block & 0xffff, 0); // RT_STRING
        if res == 0 {
            return None;
        }
        let data = LoadResource(module, res);
        let words = SizeofResource(module, res) as usize / 2;
        let block = core::slice::from_raw_parts(data, words);
        let mut at = 0;
        for _ in 0..id & 15 {
            at += 1 + usize::from(*block.get(at)?);
        }
        let len = usize::from(*block.get(at)?);
        block.get(at + 1..at + 1 + len)
    }
}

/// LoadStringW: copies the string (truncated to the buffer, terminated)
/// and returns its length; with a zero-sized buffer, stores a read-only
/// pointer to the resource's own text instead. 0 when there is none.
#[unsafe(no_mangle)]
pub extern "system" fn LoadStringW(instance: usize, id: u32, buf: *mut u16, max: i32) -> i32 {
    if buf.is_null() {
        return 0;
    }
    let text = string_resource(instance, id).unwrap_or(&[]);
    // SAFETY: the caller's buffer of `max` characters (or a pointer's place).
    unsafe {
        if max == 0 {
            *buf.cast::<*const u16>() =
                if text.is_empty() { core::ptr::null() } else { text.as_ptr() };
            return text.len() as i32;
        }
        let n = text.len().min(max as usize - 1);
        core::ptr::copy_nonoverlapping(text.as_ptr(), buf, n);
        *buf.add(n) = 0;
        n as i32
    }
}

/// LoadStringA: the string in the ANSI code page, truncated to the buffer.
#[unsafe(no_mangle)]
pub extern "system" fn LoadStringA(instance: usize, id: u32, buf: *mut u8, max: i32) -> i32 {
    if buf.is_null() || max <= 0 {
        return 0;
    }
    let text = string_resource(instance, id).unwrap_or(&[]);
    // SAFETY: the caller's buffer of `max` bytes.
    unsafe {
        let n = WideCharToMultiByte(
            0,
            0,
            text.as_ptr(),
            text.len() as i32,
            buf,
            max - 1,
            core::ptr::null(),
            core::ptr::null_mut(),
        )
        .max(0);
        *buf.add(n as usize) = 0;
        n
    }
}

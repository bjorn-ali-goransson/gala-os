//! Initialization files: GetPrivateProfileStringW,
//! WritePrivateProfileStringW and their win.ini forms GetProfileStringW
//! and WriteProfileStringW, as Microsoft documents them (the details as
//! Wine's profile conformance tests record Windows doing them;
//! `kernel32_misc_test`'s `profile.rs`). Inno Setup writes its Internet
//! shortcuts (`.url`) with them.
//!
//! The file is read whole on each call and written back whole by each
//! change (Windows caches it; nothing a program sees differs, except
//! timing). A file name without a path is in the Windows directory.
//! Sections and keys match without regard to case; blanks around names
//! and values are not part of them; one pair of matching quotes around a
//! value is removed; lines starting with ';' are comments. Text is UTF-16
//! when the file starts with its byte-order mark, otherwise the ANSI code
//! page (UTF-8 on gala), and a file is written back in the encoding it was
//! read in (new files: ANSI). win.ini's sections are not mapped to the
//! registry (IniFileMapping): they live in the file.

use super::*;

type Units = Buf<u16>;

/// A growable buffer on the process heap (kernel32 has no allocator).
struct Buf<T: Copy> {
    p: *mut T,
    len: usize,
    cap: usize,
}

impl<T: Copy> Buf<T> {
    fn new() -> Self {
        Buf { p: core::ptr::null_mut(), len: 0, cap: 0 }
    }

    fn reserve(&mut self, more: usize) -> bool {
        let need = self.len + more;
        if need <= self.cap {
            return true;
        }
        let cap = need.max(self.cap * 2).max(64);
        let bytes = cap * size_of::<T>();
        let heap = heap::GetProcessHeap();
        let p = if self.p.is_null() {
            heap::HeapAlloc(heap, 0, bytes)
        } else {
            heap::HeapReAlloc(heap, 0, self.p.cast(), bytes)
        };
        if p.is_null() {
            return false;
        }
        self.p = p.cast();
        self.cap = cap;
        true
    }

    fn push_all(&mut self, items: &[T]) -> bool {
        if !self.reserve(items.len()) {
            return false;
        }
        // SAFETY: room for `items` after `len`.
        unsafe {
            core::ptr::copy_nonoverlapping(items.as_ptr(), self.p.add(self.len), items.len())
        };
        self.len += items.len();
        true
    }

    fn as_slice(&self) -> &[T] {
        if self.p.is_null() {
            return &[];
        }
        // SAFETY: `len` initialized items.
        unsafe { core::slice::from_raw_parts(self.p, self.len) }
    }
}

impl<T: Copy> Drop for Buf<T> {
    fn drop(&mut self) {
        if !self.p.is_null() {
            heap::HeapFree(heap::GetProcessHeap(), 0, self.p.cast());
        }
    }
}

/// The caller's NUL-terminated string (None for NULL).
fn arg<'a>(s: *const u16) -> Option<&'a [u16]> {
    (!s.is_null()).then(|| wide_slice(s))
}

/// The file's full path (NUL-terminated): a name without a path is in the
/// Windows directory.
fn file_path(name: &[u16]) -> Option<Units> {
    let mut p = Units::new();
    let has_path =
        name.iter().any(|&c| c == u16::from(b'\\') || c == u16::from(b'/') || c == u16::from(b':'));
    if !has_path {
        let mut dir = [0u16; 260];
        let n = files::GetWindowsDirectoryW(dir.as_mut_ptr(), 260) as usize;
        if n == 0 || n >= 260 || !p.push_all(&dir[..n]) || !p.push_all(&[u16::from(b'\\')]) {
            return None;
        }
    }
    (p.push_all(name) && p.push_all(&[0])).then_some(p)
}

fn is_blank(c: u16) -> bool {
    c == u16::from(b' ') || c == u16::from(b'\t')
}

fn trim(s: &[u16]) -> &[u16] {
    let start = s.iter().position(|&c| !is_blank(c)).unwrap_or(s.len());
    let end = s.iter().rposition(|&c| !is_blank(c)).map_or(start, |i| i + 1);
    &s[start..end.max(start)]
}

fn lower(c: u16) -> u16 {
    if (0x41..=0x5a).contains(&c) { c + 0x20 } else { c }
}

fn same(a: &[u16], b: &[u16]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(&x, &y)| lower(x) == lower(y))
}

/// The file's text (empty when it does not exist) and whether it was
/// UTF-16.
fn read(path: &Units) -> Option<(Units, bool)> {
    const GENERIC_READ: u32 = 0x8000_0000;
    let h = CreateFileW(
        path.as_slice().as_ptr(),
        GENERIC_READ,
        7,
        core::ptr::null_mut(),
        3,
        0,
        core::ptr::null_mut(),
    );
    let mut text = Units::new();
    if h == INVALID_HANDLE_VALUE {
        return Some((text, false));
    }
    let mut bytes = Buf::<u8>::new();
    let mut chunk = [0u8; 4096];
    loop {
        let mut n = 0u32;
        if ReadFile(h, chunk.as_mut_ptr(), chunk.len() as u32, &mut n, core::ptr::null_mut())
            == FALSE
        {
            CloseHandle(h);
            return None;
        }
        if n == 0 {
            break;
        }
        if !bytes.push_all(&chunk[..n as usize]) {
            CloseHandle(h);
            return None;
        }
    }
    CloseHandle(h);
    let b = bytes.as_slice();
    if b.starts_with(&[0xff, 0xfe]) {
        if !text.reserve(b.len() / 2) {
            return None;
        }
        for pair in b[2..].chunks_exact(2) {
            text.push_all(&[u16::from_le_bytes([pair[0], pair[1]])]);
        }
        return Some((text, true));
    }
    let b = b.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(b);
    if !text.reserve(b.len()) {
        return None;
    }
    for chunk in b.utf8_chunks() {
        for c in chunk.valid().encode_utf16() {
            text.push_all(&[c]);
        }
        if !chunk.invalid().is_empty() {
            text.push_all(&[0xfffd]);
        }
    }
    Some((text, false))
}

/// Writes `text` to the file (UTF-16 with its mark, or UTF-8).
fn write(path: &Units, text: &[u16], utf16: bool) -> bool {
    const GENERIC_WRITE: u32 = 0x4000_0000;
    let mut bytes = Buf::<u8>::new();
    if utf16 {
        if !bytes.push_all(&[0xff, 0xfe]) {
            return false;
        }
        for &c in text {
            if !bytes.push_all(&c.to_le_bytes()) {
                return false;
            }
        }
    } else {
        for c in char::decode_utf16(text.iter().copied()) {
            let mut b = [0u8; 4];
            let s = c.unwrap_or(char::REPLACEMENT_CHARACTER).encode_utf8(&mut b);
            if !bytes.push_all(s.as_bytes()) {
                return false;
            }
        }
    }
    let h = CreateFileW(
        path.as_slice().as_ptr(),
        GENERIC_WRITE,
        0,
        core::ptr::null_mut(),
        2,
        0x80,
        core::ptr::null_mut(),
    );
    if h == INVALID_HANDLE_VALUE {
        return false;
    }
    let b = bytes.as_slice();
    let mut n = 0u32;
    let ok = b.is_empty()
        || WriteFile(h, b.as_ptr(), b.len() as u32, &mut n, core::ptr::null_mut()) != FALSE;
    CloseHandle(h);
    ok
}

/// One line of the file: where it is and what it holds.
#[derive(Clone, Copy)]
enum Line<'a> {
    Section(&'a [u16]),
    /// Key and value (the value's quotes kept).
    Entry(&'a [u16], &'a [u16]),
    Other,
}

/// The lines of `text` with their spans (start, end including the line
/// break).
fn lines(text: &[u16]) -> impl Iterator<Item = (usize, usize, Line<'_>)> {
    let mut at = 0;
    core::iter::from_fn(move || {
        if at >= text.len() {
            return None;
        }
        let start = at;
        let nl = text[at..].iter().position(|&c| c == u16::from(b'\n'));
        let end = nl.map_or(text.len(), |i| at + i + 1);
        at = end;
        let mut body = &text[start..end];
        while let Some((&last, rest)) = body.split_last() {
            if last == u16::from(b'\n') || last == u16::from(b'\r') {
                body = rest;
            } else {
                break;
            }
        }
        let t = trim(body);
        let line = if t.first() == Some(&u16::from(b'[')) {
            let close = t.iter().position(|&c| c == u16::from(b']')).unwrap_or(t.len());
            Line::Section(trim(&t[1..close]))
        } else if t.is_empty() || t[0] == u16::from(b';') {
            Line::Other
        } else {
            match t.iter().position(|&c| c == u16::from(b'=')) {
                Some(i) => Line::Entry(trim(&t[..i]), trim(&t[i + 1..])),
                None => Line::Entry(t, &[]),
            }
        };
        Some((start, end, line))
    })
}

/// A value without one pair of matching quotes around it.
fn unquote(v: &[u16]) -> &[u16] {
    match (v.first(), v.last()) {
        (Some(&a), Some(&b))
            if v.len() >= 2 && a == b && (a == u16::from(b'"') || a == u16::from(b'\'')) =>
        {
            &v[1..v.len() - 1]
        }
        _ => v,
    }
}

/// Copies `s` into the caller's buffer of `size` characters as one string
/// (cut to size - 1); the count copied.
fn copy_string(out: *mut u16, size: u32, s: &[u16]) -> u32 {
    let n = s.len().min(size as usize - 1);
    // SAFETY: the caller's buffer of `size` characters.
    unsafe {
        core::ptr::copy_nonoverlapping(s.as_ptr(), out, n);
        *out.add(n) = 0;
    }
    n as u32
}

/// Copies the names as a list (each NUL-terminated, a NUL after the
/// last); cut short: size - 2 characters and two NULs. The count without
/// the final NUL.
fn copy_list<'a>(out: *mut u16, size: u32, names: impl Iterator<Item = &'a [u16]>) -> u32 {
    let size = size as usize;
    // SAFETY: the caller's buffer of `size` (at least 1) characters.
    let out = unsafe { core::slice::from_raw_parts_mut(out, size) };
    if size < 2 {
        out[0] = 0;
        return 0;
    }
    let mut at = 0;
    for name in names {
        for &c in name.iter().chain(core::iter::once(&0)) {
            if at >= size - 2 {
                out[size - 2] = 0;
                out[size - 1] = 0;
                return (size - 2) as u32;
            }
            out[at] = c;
            at += 1;
        }
    }
    out[at] = 0;
    if at == 0 {
        out[1] = 0;
    }
    at as u32
}

#[unsafe(no_mangle)]
pub extern "system" fn GetPrivateProfileStringW(
    app: *const u16,
    key: *const u16,
    default: *const u16,
    out: *mut u16,
    size: u32,
    file: *const u16,
) -> u32 {
    if out.is_null() || size == 0 {
        return 0;
    }
    let name = arg(file).unwrap_or(&[]);
    let name: &[u16] =
        if name.is_empty() { &[0x77, 0x69, 0x6e, 0x2e, 0x69, 0x6e, 0x69] } else { name }; // win.ini
    let text = file_path(name).and_then(|p| read(&p));
    let text = text.as_ref().map_or(&[][..], |(t, _)| t.as_slice());
    // The default without its trailing blanks.
    let default = arg(default).unwrap_or(&[]);
    let default = &default[..default.iter().rposition(|&c| !is_blank(c)).map_or(0, |i| i + 1)];
    let (Some(app), Some(key)) = (arg(app), arg(key)) else {
        return match arg(app) {
            // Every section's name.
            None => copy_list(
                out,
                size,
                lines(text).filter_map(|(_, _, l)| match l {
                    Line::Section(s) => Some(s),
                    _ => None,
                }),
            ),
            // Every key of the section.
            Some(app) => {
                let mut inside = false;
                copy_list(
                    out,
                    size,
                    lines(text).filter_map(move |(_, _, l)| match l {
                        Line::Section(s) => {
                            inside = same(s, app);
                            None
                        }
                        Line::Entry(k, _) if inside => Some(k),
                        _ => None,
                    }),
                )
            }
        };
    };
    let app = trim(app);
    let key = trim(key);
    let mut inside = false;
    for (_, _, l) in lines(text) {
        match l {
            Line::Section(s) => inside = same(s, app),
            Line::Entry(k, v) if inside && same(k, key) => {
                return copy_string(out, size, unquote(v));
            }
            _ => {}
        }
    }
    copy_string(out, size, default)
}

/// WritePrivateProfileStringW(app, key, value, file): sets the value
/// (adding the key after the section's last entry, or the section at the
/// end); a NULL value deletes the key, a NULL key the section.
#[unsafe(no_mangle)]
pub extern "system" fn WritePrivateProfileStringW(
    app: *const u16,
    key: *const u16,
    value: *const u16,
    file: *const u16,
) -> Bool {
    let Some(app) = arg(app).map(trim) else {
        // Flushing the cache (gala keeps none): nothing to do.
        return TRUE;
    };
    let name = arg(file).unwrap_or(&[]);
    let name: &[u16] =
        if name.is_empty() { &[0x77, 0x69, 0x6e, 0x2e, 0x69, 0x6e, 0x69] } else { name }; // win.ini
    let Some(path) = file_path(name) else { return fail_with(ERROR_NOT_ENOUGH_MEMORY, FALSE) };
    let Some((text, utf16)) = read(&path) else { return FALSE };
    let text = text.as_slice();
    let key = arg(key).map(trim);
    let value = arg(value);
    // Where the section is: its header line's start, the end of its last
    // entry line, the end of the section (before the next header).
    let mut section: Option<(usize, usize, usize)> = None;
    let mut entry: Option<(usize, usize, &[u16])> = None;
    for (start, end, l) in lines(text) {
        match l {
            Line::Section(s) => {
                if section.is_some() {
                    break;
                }
                if same(s, app) {
                    section = Some((start, end, end));
                }
            }
            Line::Entry(k, _) => {
                if let Some(sec) = section.as_mut() {
                    sec.1 = end;
                    sec.2 = end;
                    if key.is_some_and(|key| same(k, key)) {
                        entry = Some((start, end, k));
                    }
                }
            }
            Line::Other => {
                if let Some(sec) = section.as_mut() {
                    sec.2 = end;
                }
            }
        }
    }
    let crlf = [13u16, 10];
    let mut new = Units::new();
    let ok = match (key, value, section, entry) {
        // Delete the section.
        (None, _, Some((start, _, end)), _) => {
            new.push_all(&text[..start]) && new.push_all(&text[end..])
        }
        (None, _, None, _) | (Some(_), None, _, None) => return TRUE,
        // Delete the key.
        (Some(_), None, _, Some((start, end, _))) => {
            new.push_all(&text[..start]) && new.push_all(&text[end..])
        }
        // Replace the value (the key spelled as it was).
        (Some(_), Some(v), _, Some((start, end, k))) => {
            new.push_all(&text[..start])
                && new.push_all(k)
                && new.push_all(&[u16::from(b'=')])
                && new.push_all(v)
                && new.push_all(&crlf)
                && new.push_all(&text[end..])
        }
        // A new key after the section's last entry.
        (Some(k), Some(v), Some((_, last, _)), None) => {
            let needs_break = last > 0 && text[last - 1] != 10;
            new.push_all(&text[..last])
                && (!needs_break || new.push_all(&crlf))
                && new.push_all(k)
                && new.push_all(&[u16::from(b'=')])
                && new.push_all(v)
                && new.push_all(&crlf)
                && new.push_all(&text[last..])
        }
        // A new section at the end.
        (Some(k), Some(v), None, None) => {
            let needs_break = !text.is_empty() && text[text.len() - 1] != 10;
            new.push_all(text)
                && (!needs_break || new.push_all(&crlf))
                && new.push_all(&[u16::from(b'[')])
                && new.push_all(app)
                && new.push_all(&[u16::from(b']')])
                && new.push_all(&crlf)
                && new.push_all(k)
                && new.push_all(&[u16::from(b'=')])
                && new.push_all(v)
                && new.push_all(&crlf)
        }
    };
    if !ok {
        return fail_with(ERROR_NOT_ENOUGH_MEMORY, FALSE);
    }
    Bool::from(write(&path, new.as_slice(), utf16))
}

/// GetProfileStringW: GetPrivateProfileStringW of win.ini.
#[unsafe(no_mangle)]
pub extern "system" fn GetProfileStringW(
    app: *const u16,
    key: *const u16,
    default: *const u16,
    out: *mut u16,
    size: u32,
) -> u32 {
    GetPrivateProfileStringW(app, key, default, out, size, core::ptr::null())
}

/// WriteProfileStringW: WritePrivateProfileStringW of win.ini.
#[unsafe(no_mangle)]
pub extern "system" fn WriteProfileStringW(
    app: *const u16,
    key: *const u16,
    value: *const u16,
) -> Bool {
    WritePrivateProfileStringW(app, key, value, core::ptr::null())
}

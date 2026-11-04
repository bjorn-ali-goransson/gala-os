//! URL helpers (shlwapi.h) for what SQL Server LocalDB (msxmlsql,
//! sqllang, xe, xmlrw) imports: UrlIsW, UrlEscapeW, UrlUnescapeW,
//! UrlCanonicalizeW, UrlCombineW, UrlCreateFromPathW and
//! PathCreateFromUrlW, with the rules Wine's conformance tests show
//! Windows following (see user/shlwapi_test/url.c):
//!
//! - Schemes: http, https, file, gopher, news, nntp, snews and telnet
//!   read backslashes as slashes; ftp, local, res and wais keep them;
//!   about, javascript, mailto, shell and vbscript are opaque (no path).
//! - Escaping: controls, space, `"#<>[\]^`{|}&` and Latin-1 become %XX
//!   (`%` only with URL_ESCAPE_PERCENT; with URL_ESCAPE_AS_UTF8 all
//!   non-ASCII as UTF-8); `?` ends what is escaped (also `#`, except in
//!   file URLs); a `/` is kept in a hierarchical part (one that starts
//!   with `/`) and escaped otherwise.
//! - Simplifying (UrlCanonicalize, UrlCombine): in the path after the
//!   host, `.` is dropped and `..` removes the segment before it (or is
//!   dropped when there is none); a path whose first segment is `.` or
//!   `..` is left alone.
//!
//! Forms outside these (other schemes' canonical forms, the mk: protocol)
//! are reported to the ledger and passed through.

#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use crate::paths_more::{chars, scheme_len};

const S_OK: i32 = 0;
const S_FALSE: i32 = 1;
const E_POINTER: i32 = 0x8000_4003u32 as i32;
const E_INVALIDARG: i32 = 0x8007_0057u32 as i32;

const URL_ESCAPE_SPACES_ONLY: u32 = 0x0400_0000;
const URL_DONT_SIMPLIFY: u32 = 0x0800_0000;
const URL_ESCAPE_PERCENT: u32 = 0x0000_1000;
const URL_ESCAPE_SEGMENT_ONLY: u32 = 0x0000_2000;
const URL_ESCAPE_AS_UTF8: u32 = 0x0004_0000;
const URL_UNESCAPE_INPLACE: u32 = 0x0010_0000;
const URL_DONT_UNESCAPE_EXTRA_INFO: u32 = 0x0200_0000;
const URL_ESCAPE_UNSAFE: u32 = 0x2000_0000;
const URL_WININET_COMPATIBILITY: u32 = 0x8000_0000;
const URL_PLUGGABLE_PROTOCOL: u32 = 0x4000_0000;

fn unsupported(what: &[u8], detail: u64) {
    crate::strings::unsupported(what, detail);
}

/// A growing UTF-16 string (escaping can triple a URL's length).
pub(crate) struct Buf {
    data: [u16; 8192],
    len: usize,
    overflow: bool,
}

impl Buf {
    pub(crate) fn new() -> Self {
        Buf { data: [0; 8192], len: 0, overflow: false }
    }
    pub(crate) fn push(&mut self, c: u16) {
        if self.len + 1 < self.data.len() {
            self.data[self.len] = c;
            self.len += 1;
        } else {
            self.overflow = true;
        }
    }
    pub(crate) fn extend(&mut self, s: &[u16]) {
        s.iter().for_each(|&c| self.push(c));
    }
    fn ascii(&mut self, s: &[u8]) {
        s.iter().for_each(|&c| self.push(c.into()));
    }
    fn hex(&mut self, b: u8) {
        const D: &[u8; 16] = b"0123456789ABCDEF";
        self.push(u16::from(b'%'));
        self.push(D[usize::from(b >> 4)].into());
        self.push(D[usize::from(b & 15)].into());
    }
    pub(crate) fn text(&self) -> &[u16] {
        &self.data[..self.len]
    }
}

/// Hands `b` to the caller: `dst` of `*count` characters gets it and a
/// NUL (S_OK, `*count` its length); E_POINTER and the size needed (with
/// the NUL) when it does not fit; E_INVALIDARG without a buffer.
pub(crate) fn give(b: &Buf, dst: *mut u16, count: *mut u32, ok: i32) -> i32 {
    if b.overflow {
        return E_INVALIDARG;
    }
    if count.is_null() || dst.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's count.
    let room = unsafe { *count } as usize;
    if room == 0 && b.len > 0 && dst.is_null() {
        return E_INVALIDARG;
    }
    if room < b.len + 1 {
        // SAFETY: as above.
        unsafe { *count = (b.len + 1) as u32 };
        return E_POINTER;
    }
    // SAFETY: the caller's buffer of `room` > len characters (may be the
    // source: our copy is separate).
    unsafe {
        core::ptr::copy(b.data.as_ptr(), dst, b.len);
        dst.add(b.len).write(0);
        *count = b.len as u32;
    }
    ok
}

fn lower(c: u16) -> u16 {
    if (0x41..=0x5a).contains(&c) { c + 0x20 } else { c }
}

/// Whether `s` (a scheme) is `name`, case ignored.
fn is(s: &[u16], name: &[u8]) -> bool {
    s.len() == name.len() && s.iter().zip(name).all(|(&c, &n)| lower(c) == u16::from(n))
}

fn any(s: &[u16], names: &[&[u8]]) -> bool {
    names.iter().any(|n| is(s, n))
}

const OPAQUE: &[&[u8]] = &[b"about", b"javascript", b"mailto", b"shell", b"vbscript"];
const TRANSLATING: &[&[u8]] =
    &[b"http", b"https", b"file", b"gopher", b"news", b"nntp", b"snews", b"telnet"];
const KEEPING: &[&[u8]] = &[b"ftp", b"local", b"res", b"wais"];

/// The scheme of `s` and what follows its colon.
fn split_scheme(s: &[u16]) -> Option<(&[u16], &[u16])> {
    let n = scheme_len(s)?;
    Some((&s[..n], &s[n + 1..]))
}

const SLASH: u16 = b'/' as u16;
const BACKSLASH: u16 = b'\\' as u16;
const QUESTION: u16 = b'?' as u16;
const HASH: u16 = b'#' as u16;

/// Characters UrlEscape always escapes (outside `/`, `\`, `?`, `#`,
/// whose treatment depends on where they are).
fn unsafe_char(c: u16) -> bool {
    c < 0x20 || c == 0x7f || b" \"<>[]^`{|}&".contains(&(c as u8)) && c < 0x80
}

/// Escapes character `c` into `out` if it needs it (`segment`: `/`, `\`,
/// `?` and `#` too).
fn escape_char(c: u16, flags: u32, segment: bool, out: &mut Buf) {
    let spaces_only = flags & URL_ESCAPE_SPACES_ONLY != 0;
    if spaces_only {
        if c == 0x20 {
            out.hex(0x20)
        } else {
            out.push(c)
        }
        return;
    }
    let structural = c == SLASH || c == BACKSLASH || c == QUESTION || c == HASH;
    if unsafe_char(c)
        || (segment && structural)
        || (c == u16::from(b'%') && flags & URL_ESCAPE_PERCENT != 0)
    {
        out.hex(c as u8);
    } else if c >= 0x80 && flags & URL_ESCAPE_AS_UTF8 != 0 {
        let mut b = [0u8; 4];
        let ch = char::from_u32(c.into()).unwrap_or('\u{fffd}');
        ch.encode_utf8(&mut b).bytes().for_each(|x| out.hex(x));
    } else if (0x80..=0xff).contains(&c) {
        out.hex(c as u8);
    } else {
        out.push(c);
    }
}

/// UrlEscape's work on `s` into `out`.
fn escape(s: &[u16], flags: u32, out: &mut Buf) {
    let mut units = s.iter().copied().peekable();
    if flags & URL_ESCAPE_SEGMENT_ONLY != 0 {
        while let Some(c) = units.next() {
            escape_utf16(c, &mut units, flags, true, out);
        }
        return;
    }
    let (scheme, rest) = match split_scheme(s) {
        Some((sc, rest)) => {
            out.extend(&s[..sc.len() + 1]);
            (Some(sc), rest)
        }
        None => (None, s),
    };
    let opaque = scheme.is_some_and(|sc| any(sc, OPAQUE));
    let file = scheme.is_some_and(|sc| is(sc, b"file"));
    let translate = scheme.is_some_and(|sc| any(sc, TRANSLATING));
    let keep_backslash = scheme.is_some_and(|sc| any(sc, KEEPING));
    let stop = |c: u16| !opaque && (c == QUESTION || (c == HASH && !file));
    // Backslashes read as slashes, up to where escaping stops.
    let mut path = Buf::new();
    let mut stopped = false;
    for &c in rest {
        stopped |= stop(c);
        path.push(if translate && !stopped && c == BACKSLASH { SLASH } else { c });
    }
    let rest = path.text();
    let hierarchical = !opaque && (scheme.is_none() || rest.first() == Some(&SLASH));
    let mut i = 0;
    while i < rest.len() {
        let c = rest[i];
        if stop(c) {
            out.extend(&rest[i..]);
            return;
        }
        match c {
            SLASH if hierarchical => out.push(c),
            SLASH => out.hex(b'/'),
            BACKSLASH if keep_backslash => out.push(c),
            BACKSLASH => out.hex(b'\\'),
            HASH => out.hex(b'#'),
            _ => {
                // Surrogate pairs as one character for UTF-8.
                if (0xd800..0xdc00).contains(&c)
                    && flags & URL_ESCAPE_AS_UTF8 != 0
                    && rest.get(i + 1).is_some_and(|d| (0xdc00..0xe000).contains(d))
                {
                    let v = 0x10000
                        + ((u32::from(c) - 0xd800) << 10)
                        + (u32::from(rest[i + 1]) - 0xdc00);
                    let mut b = [0u8; 4];
                    char::from_u32(v)
                        .unwrap_or('\u{fffd}')
                        .encode_utf8(&mut b)
                        .bytes()
                        .for_each(|x| out.hex(x));
                    i += 2;
                    continue;
                }
                escape_char(c, flags, opaque, out);
            }
        }
        i += 1;
    }
}

fn escape_utf16(
    c: u16,
    rest: &mut core::iter::Peekable<impl Iterator<Item = u16>>,
    flags: u32,
    segment: bool,
    out: &mut Buf,
) {
    if (0xd800..0xdc00).contains(&c)
        && flags & URL_ESCAPE_AS_UTF8 != 0
        && let Some(&d) = rest.peek()
        && (0xdc00..0xe000).contains(&d)
    {
        {
            rest.next();
            let v = 0x10000 + ((u32::from(c) - 0xd800) << 10) + (u32::from(d) - 0xdc00);
            let mut b = [0u8; 4];
            char::from_u32(v)
                .unwrap_or('\u{fffd}')
                .encode_utf8(&mut b)
                .bytes()
                .for_each(|x| out.hex(x));
            return;
        }
    }
    escape_char(c, flags, segment, out);
}

/// UrlEscapeW: the URL with unsafe characters as %XX (see the module
/// documentation); E_POINTER with the size needed when `out` is short.
#[unsafe(no_mangle)]
pub extern "system" fn UrlEscapeW(
    url: *const u16,
    out: *mut u16,
    count: *mut u32,
    flags: u32,
) -> i32 {
    if url.is_null() || out.is_null() || count.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's count.
    if unsafe { *count } == 0 {
        return E_INVALIDARG;
    }
    let known = URL_ESCAPE_SPACES_ONLY
        | URL_ESCAPE_PERCENT
        | URL_ESCAPE_SEGMENT_ONLY
        | URL_ESCAPE_AS_UTF8
        | URL_ESCAPE_UNSAFE
        | URL_DONT_UNESCAPE_EXTRA_INFO
        | URL_WININET_COMPATIBILITY;
    if flags & !known != 0 {
        unsupported(b"shlwapi.dll!UrlEscapeW(flags)", flags.into());
    }
    let mut b = Buf::new();
    escape(chars(url), flags, &mut b);
    give(&b, out, count, S_OK)
}

fn hex_digit(c: u16) -> Option<u8> {
    match c {
        0x30..=0x39 => Some((c - 0x30) as u8),
        0x41..=0x46 => Some((c - 0x41 + 10) as u8),
        0x61..=0x66 => Some((c - 0x61 + 10) as u8),
        _ => None,
    }
}

/// The byte a `%XX` at `s[i]` stands for.
fn escaped_byte(s: &[u16], i: usize) -> Option<u8> {
    if s.get(i) != Some(&u16::from(b'%')) {
        return None;
    }
    Some(hex_digit(*s.get(i + 1)?)? << 4 | hex_digit(*s.get(i + 2)?)?)
}

/// Decodes %XX sequences of `s` into `out`: each byte as a character, or
/// with `utf8` as UTF-8 (a broken sequence is U+FFFD); with
/// `stop_extra`, `?` and `#` and what follows stay as they are.
pub(crate) fn unescape(s: &[u16], utf8: bool, stop_extra: bool, out: &mut Buf) {
    let mut i = 0;
    while i < s.len() {
        let c = s[i];
        if stop_extra && (c == QUESTION || c == HASH) {
            out.extend(&s[i..]);
            return;
        }
        let Some(b) = escaped_byte(s, i) else {
            out.push(c);
            i += 1;
            continue;
        };
        i += 3;
        if !utf8 || b < 0x80 {
            out.push(b.into());
            continue;
        }
        let need = match b {
            0xc2..=0xdf => 1,
            0xe0..=0xef => 2,
            0xf0..=0xf4 => 3,
            _ => {
                out.push(0xfffd);
                continue;
            }
        };
        let mut v = u32::from(b) & (0x3f >> need);
        let mut got = 0;
        while got < need {
            match escaped_byte(s, i) {
                Some(x) if x & 0xc0 == 0x80 => {
                    v = v << 6 | u32::from(x & 0x3f);
                    i += 3;
                    got += 1;
                }
                _ => break,
            }
        }
        match char::from_u32(v).filter(|_| got == need) {
            Some(ch) => {
                let mut w = [0u16; 2];
                ch.encode_utf16(&mut w).iter().for_each(|&u| out.push(u));
            }
            None => out.push(0xfffd),
        }
    }
}

/// UrlUnescapeW: %XX sequences decoded (URL_UNESCAPE_AS_UTF8: as UTF-8;
/// URL_DONT_UNESCAPE_EXTRA_INFO: not after `?` or `#`), into `out` or,
/// with URL_UNESCAPE_INPLACE, into the URL itself.
#[unsafe(no_mangle)]
pub extern "system" fn UrlUnescapeW(
    url: *mut u16,
    out: *mut u16,
    count: *mut u32,
    flags: u32,
) -> i32 {
    if url.is_null() {
        return E_INVALIDARG;
    }
    let inplace = flags & URL_UNESCAPE_INPLACE != 0;
    if !inplace && (out.is_null() || count.is_null()) {
        if !out.is_null() {
            // SAFETY: the caller's buffer.
            unsafe { out.write(0) };
        }
        return E_INVALIDARG;
    }
    let mut b = Buf::new();
    unescape(
        chars(url),
        flags & URL_ESCAPE_AS_UTF8 != 0,
        flags & URL_DONT_UNESCAPE_EXTRA_INFO != 0,
        &mut b,
    );
    if inplace {
        // SAFETY: the result is never longer than the URL.
        unsafe {
            core::ptr::copy(b.data.as_ptr(), url, b.len);
            url.add(b.len).write(0);
        }
        return S_OK;
    }
    give(&b, out, count, S_OK)
}

/// UrlIsW: URLIS_URL (a scheme), URLIS_OPAQUE (about, javascript,
/// mailto, shell, vbscript), URLIS_FILEURL, URLIS_DIRECTORY (ends with a
/// slash), URLIS_HASQUERY; URLIS_NOHISTORY and URLIS_APPLIABLE are
/// approximated (recorded).
#[unsafe(no_mangle)]
pub extern "system" fn UrlIsW(url: *const u16, what: u32) -> i32 {
    if url.is_null() {
        return 0;
    }
    let s = chars(url);
    let scheme = split_scheme(s).map(|(sc, _)| sc);
    let r = match what {
        0 => scheme.is_some(),
        1 => scheme.is_some_and(|sc| any(sc, OPAQUE)),
        2 => {
            unsupported(b"shlwapi.dll!UrlIsW(URLIS_NOHISTORY)", 0);
            scheme.is_some_and(|sc| any(sc, OPAQUE))
        }
        3 => scheme.is_some_and(|sc| is(sc, b"file")),
        4 => {
            unsupported(b"shlwapi.dll!UrlIsW(URLIS_APPLIABLE)", 0);
            scheme.is_some()
        }
        5 => matches!(s.last(), Some(&SLASH) | Some(&BACKSLASH)),
        6 => s.contains(&QUESTION),
        _ => false,
    };
    i32::from(r)
}

/// The http-like simplification of `path` (it starts with `/`) into
/// `out` (see the module documentation).
fn simplify(path: &[u16], out: &mut Buf) {
    let body = &path[1..];
    let first = body.split(|&c| c == SLASH).next().unwrap_or(&[]);
    let dot = [u16::from(b'.')];
    let dots = [u16::from(b'.'), u16::from(b'.')];
    if first == dot || first == dots {
        out.extend(path);
        return;
    }
    // The kept segments as (start, end) in `body`.
    let mut kept = [(0usize, 0usize); 512];
    let mut n: usize = 0;
    let mut trailing = false;
    let mut at = 0;
    for seg in body.split(|&c| c == SLASH) {
        let range = (at, at + seg.len());
        at += seg.len() + 1;
        if seg == dot {
            trailing = true;
        } else if seg == dots {
            n = n.saturating_sub(1);
            trailing = true;
        } else if n < kept.len() {
            kept[n] = range;
            n += 1;
            trailing = false;
        }
    }
    out.push(SLASH);
    for (i, &(a, b)) in kept[..n].iter().enumerate() {
        if i > 0 {
            out.push(SLASH);
        }
        out.extend(&body[a..b]);
    }
    if trailing && n > 0 {
        out.push(SLASH);
    }
}

/// A hierarchical URL's parts: `scheme:` + `//authority` (if any), the
/// path, and what follows it (`?...`, `#...`).
struct Parts<'a> {
    scheme: &'a [u16],
    authority: Option<&'a [u16]>,
    path: &'a [u16],
    tail: &'a [u16],
}

/// Where a path ends in a URL of scheme `scheme`: at `?`, or `#`
/// (in a file URL only after a .htm/.html name).
fn path_end(scheme: &[u16], s: &[u16]) -> usize {
    let file = is(scheme, b"file");
    for (i, &c) in s.iter().enumerate() {
        if c == QUESTION {
            return i;
        }
        if c == HASH {
            if !file {
                return i;
            }
            let before = &s[..i];
            let html = |ext: &[u8]| {
                before.len() >= ext.len() && is(&before[before.len() - ext.len()..], ext)
            };
            if html(b".htm") || html(b".html") {
                return i;
            }
        }
    }
    s.len()
}

/// Splits `rest` (after `scheme:`, backslashes already read as slashes
/// where the scheme does) into parts.
fn parts<'a>(scheme: &'a [u16], rest: &'a [u16]) -> Parts<'a> {
    let (authority, after) = if rest.starts_with(&[SLASH, SLASH]) {
        let r = &rest[2..];
        let end =
            r.iter().position(|&c| c == SLASH || c == QUESTION || c == HASH).unwrap_or(r.len());
        (Some(&r[..end]), &r[end..])
    } else {
        (None, rest)
    };
    let end = path_end(scheme, after);
    Parts { scheme, authority, path: &after[..end], tail: &after[end..] }
}

/// `rest` with backslashes as slashes up to its path's end, for the
/// schemes that read them so.
fn translated(scheme: &[u16], rest: &[u16], out: &mut Buf) {
    if !any(scheme, TRANSLATING) {
        out.extend(rest);
        return;
    }
    let mut stopped = false;
    for &c in rest {
        stopped |= c == QUESTION || (c == HASH && !is(scheme, b"file"));
        out.push(if !stopped && c == BACKSLASH { SLASH } else { c });
    }
}

/// Writes the parts back, the scheme in lower case and the path
/// simplified unless `keep`.
fn assemble(p: &Parts, keep: bool, out: &mut Buf) {
    p.scheme.iter().for_each(|&c| out.push(lower(c)));
    out.push(u16::from(b':'));
    if let Some(a) = p.authority {
        out.extend(&[SLASH, SLASH]);
        out.extend(a);
    }
    if p.path.is_empty() && p.authority.is_some() {
        out.push(SLASH);
    } else if keep || p.path.first() != Some(&SLASH) {
        out.extend(p.path);
    } else {
        simplify(p.path, out);
    }
    out.extend(p.tail);
}

fn hierarchical_scheme(sc: &[u16]) -> bool {
    any(sc, TRANSLATING) || any(sc, KEEPING)
}

/// UrlCanonicalizeW: for the http-like schemes with a `//host`,
/// backslashes as slashes (where the scheme reads them so), an empty
/// path as `/`, the path simplified (URL_DONT_SIMPLIFY: not), and with
/// URL_ESCAPE_SPACES_ONLY or URL_ESCAPE_UNSAFE escaped as UrlEscapeW
/// does. Other forms are copied (recorded when they might change).
#[unsafe(no_mangle)]
pub extern "system" fn UrlCanonicalizeW(
    url: *const u16,
    out: *mut u16,
    count: *mut u32,
    flags: u32,
) -> i32 {
    if url.is_null() || out.is_null() || count.is_null() {
        return E_INVALIDARG;
    }
    let s = chars(url);
    let mut b = Buf::new();
    match split_scheme(s).filter(|(sc, _)| hierarchical_scheme(sc)) {
        Some((sc, rest)) => {
            let mut t = Buf::new();
            translated(sc, rest, &mut t);
            let p = parts(sc, t.text());
            if p.authority.is_none() {
                unsupported(b"shlwapi.dll!UrlCanonicalizeW(no host)", 0);
                b.extend(s);
            } else {
                let mut assembled = Buf::new();
                assemble(&p, flags & URL_DONT_SIMPLIFY != 0, &mut assembled);
                // assemble lower-cases the scheme; keep the caller's.
                b.extend(&s[..sc.len()]);
                b.extend(&assembled.text()[sc.len()..]);
            }
        }
        None => {
            let changes =
                s.windows(2).any(|w| w == [SLASH, u16::from(b'.')]) || s.contains(&BACKSLASH);
            if changes {
                unsupported(b"shlwapi.dll!UrlCanonicalizeW(form)", 0);
            }
            b.extend(s);
        }
    }
    let escaping = flags & (URL_ESCAPE_SPACES_ONLY | URL_ESCAPE_UNSAFE);
    if escaping != 0 {
        let mut e = Buf::new();
        escape(b.text(), escaping & URL_ESCAPE_SPACES_ONLY, &mut e);
        return give(&e, out, count, S_OK);
    }
    let known = URL_DONT_SIMPLIFY | URL_WININET_COMPATIBILITY | URL_DONT_UNESCAPE_EXTRA_INFO;
    if flags & !known & !(URL_ESCAPE_SPACES_ONLY | URL_ESCAPE_UNSAFE) != 0 {
        unsupported(b"shlwapi.dll!UrlCanonicalizeW(flags)", flags.into());
    }
    give(&b, out, count, S_OK)
}

/// A DOS path (`X:` or `\\`) as a file URL (UrlCreateFromPath's work).
fn path_url(p: &[u16], out: &mut Buf) {
    let drive =
        p.len() >= 2 && p[1] == u16::from(b':') && p[0] < 128 && (p[0] as u8).is_ascii_alphabetic();
    out.ascii(b"file:");
    let converted = |c: u16| if c == BACKSLASH { SLASH } else { c };
    if drive {
        out.ascii(b"///");
    } else if p.first().is_some_and(|&c| c == BACKSLASH || c == SLASH)
        && !p.get(1).is_some_and(|&c| c == BACKSLASH || c == SLASH)
    {
        out.ascii(b"//");
    }
    for &c in p {
        let c = converted(c);
        match c {
            0x20 => out.hex(0x20),
            0x25 => out.hex(0x25),
            0x23 => out.hex(0x23),
            _ => out.push(c),
        }
    }
}

fn dos_path(s: &[u16]) -> bool {
    (s.len() >= 2 && s[1] == u16::from(b':') && s[0] < 128 && (s[0] as u8).is_ascii_alphabetic())
        || s.starts_with(&[BACKSLASH, BACKSLASH])
}

/// UrlCreateFromPathW: a DOS path as a file URL (`c:\a b` is
/// `file:///c:/a%20b`, `\\host\share` `file://host/share`, a relative
/// path `file:a/b`); a URL is copied with S_FALSE.
#[unsafe(no_mangle)]
pub extern "system" fn UrlCreateFromPathW(
    path: *const u16,
    out: *mut u16,
    count: *mut u32,
    _reserved: u32,
) -> i32 {
    if path.is_null() || out.is_null() || count.is_null() {
        return E_INVALIDARG;
    }
    let s = chars(path);
    let mut b = Buf::new();
    if split_scheme(s).is_some() {
        b.extend(s);
        return give(&b, out, count, S_FALSE);
    }
    path_url(s, &mut b);
    give(&b, out, count, S_OK)
}

/// UrlCombineW: `relative` resolved against `base` (RFC 3986's way, with
/// Windows' simplification; DOS paths become file URLs first; a fragment
/// alone replaces the base's; URL_DONT_SIMPLIFY keeps dots).
#[unsafe(no_mangle)]
pub extern "system" fn UrlCombineW(
    base: *const u16,
    relative: *const u16,
    out: *mut u16,
    count: *mut u32,
    flags: u32,
) -> i32 {
    if base.is_null() || relative.is_null() || count.is_null() {
        return E_INVALIDARG;
    }
    let mut base_buf = Buf::new();
    let mut rel_buf = Buf::new();
    let (bs, rs) = (chars(base), chars(relative));
    if split_scheme(bs).is_some() {
        base_buf.extend(bs);
    } else {
        path_url(bs, &mut base_buf);
    }
    if dos_path(rs) && !rs.starts_with(&[BACKSLASH, BACKSLASH]) {
        path_url(rs, &mut rel_buf);
    } else {
        rel_buf.extend(rs);
    }
    let keep = flags & URL_DONT_SIMPLIFY != 0;
    if flags
        & !(URL_DONT_SIMPLIFY
            | URL_PLUGGABLE_PROTOCOL
            | URL_ESCAPE_SPACES_ONLY
            | URL_DONT_UNESCAPE_EXTRA_INFO)
        != 0
    {
        unsupported(b"shlwapi.dll!UrlCombineW(flags)", flags.into());
    }
    let Some((scheme, base_rest)) = split_scheme(base_buf.text()) else { return E_INVALIDARG };
    if is(scheme, b"mk") {
        unsupported(b"shlwapi.dll!UrlCombineW(mk:)", 0);
    }
    let mut rel = rel_buf.text();
    let mut b = Buf::new();
    if let Some((rsc, rrest)) = split_scheme(rel) {
        let same = rsc.len() == scheme.len()
            && rsc.iter().zip(scheme).all(|(&a, &c)| lower(a) == lower(c));
        if !same || rrest.first() == Some(&SLASH) {
            // An absolute URL of its own.
            rsc.iter().for_each(|&c| b.push(lower(c)));
            b.push(u16::from(b':'));
            b.extend(rrest);
            return give(&b, out, count, S_OK);
        }
        rel = rrest;
    }
    let mut t = Buf::new();
    translated(scheme, base_rest, &mut t);
    let p = parts(scheme, t.text());
    let mut r = Buf::new();
    translated(scheme, rel, &mut r);
    let rel = r.text();
    let hierarchical = p.authority.is_some() || p.path.first() == Some(&SLASH);
    if !hierarchical {
        // An opaque base: the relative part replaces what follows the
        // scheme.
        scheme.iter().for_each(|&c| b.push(lower(c)));
        b.push(u16::from(b':'));
        b.extend(rel);
        return give(&b, out, count, S_OK);
    }
    let mut path = Buf::new();
    let tail: &[u16];
    let mut authority = p.authority;
    if rel.starts_with(&[SLASH, SLASH]) {
        let q = parts(scheme, rel);
        authority = q.authority;
        path.extend(q.path);
        tail = q.tail;
    } else if rel.first() == Some(&HASH) {
        path.extend(p.path);
        // The base's query stays; its fragment goes.
        let query_end = p.tail.iter().position(|&c| c == HASH).unwrap_or(p.tail.len());
        let mut whole = Buf::new();
        whole.extend(&p.tail[..query_end]);
        whole.extend(rel);
        let parts2 = Parts { scheme, authority, path: path.text(), tail: whole.text() };
        assemble(&parts2, keep, &mut b);
        return give(&b, out, count, S_OK);
    } else if rel.first() == Some(&QUESTION) {
        path.extend(p.path);
        tail = rel;
    } else if rel.first() == Some(&SLASH) {
        let end = path_end(scheme, rel);
        path.extend(&rel[..end]);
        tail = &rel[end..];
    } else {
        let dir = p.path.iter().rposition(|&c| c == SLASH).map_or(0, |i| i + 1);
        if dir == 0 {
            path.push(SLASH);
        } else {
            path.extend(&p.path[..dir]);
        }
        let end = path_end(scheme, rel);
        path.extend(&rel[..end]);
        tail = &rel[end..];
    }
    let parts2 = Parts { scheme, authority, path: path.text(), tail };
    assemble(&parts2, keep, &mut b);
    give(&b, out, count, S_OK)
}

/// PathCreateFromUrlW: a file URL as a DOS path: `file:///c:/a%20b` is
/// `c:\a b`, `file://host/share` `\\host\share`, `file://localhost/...`
/// the local path; `c|` is `c:`. With two or four slashes before a drive
/// the escapes stay (as Windows does). E_INVALIDARG for other URLs.
#[unsafe(no_mangle)]
pub extern "system" fn PathCreateFromUrlW(
    url: *const u16,
    path: *mut u16,
    count: *mut u32,
    _flags: u32,
) -> i32 {
    if url.is_null() || path.is_null() || count.is_null() {
        return E_INVALIDARG;
    }
    let s = chars(url);
    let Some((scheme, rest)) = split_scheme(s).filter(|(sc, _)| is(sc, b"file")) else {
        return E_INVALIDARG;
    };
    let _ = scheme;
    let slash = |c: u16| c == SLASH || c == BACKSLASH;
    let n = rest.iter().take_while(|&&c| slash(c)).count();
    let body = &rest[n..];
    let drive = |b: &[u16]| {
        b.len() >= 2
            && b[0] < 128
            && (b[0] as u8).is_ascii_alphabetic()
            && (b[1] == u16::from(b':') || b[1] == u16::from(b'|'))
    };
    let mut prefix: &[u8] = b"";
    let mut unescaping = true;
    let mut body = body;
    match n {
        0 => {}
        1 | 3 => {
            if !drive(body) {
                prefix = b"\\";
            }
        }
        2 => {
            let host_end = body.iter().position(|&c| slash(c)).unwrap_or(body.len());
            if is(&body[..host_end], b"localhost") {
                body = body.get(host_end + 1..).unwrap_or(&[]);
                if !drive(body) {
                    prefix = b"\\";
                }
            } else if drive(body) {
                unescaping = false;
            } else {
                prefix = b"\\\\";
            }
        }
        4 => {
            unescaping = false;
            if !drive(body) {
                prefix = b"\\\\";
            }
        }
        _ => prefix = b"\\\\",
    }
    let mut converted = Buf::new();
    for (i, &c) in body.iter().enumerate() {
        let c = if c == SLASH { BACKSLASH } else { c };
        let c = if i == 1 && drive(body) { u16::from(b':') } else { c };
        converted.push(c);
    }
    let mut b = Buf::new();
    b.ascii(prefix);
    if unescaping {
        unescape(converted.text(), true, false, &mut b);
    } else {
        b.extend(converted.text());
    }
    give(&b, path, count, S_OK)
}

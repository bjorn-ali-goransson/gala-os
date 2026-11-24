//! The URL moniker: MkParseDisplayNameEx, CreateURLMoniker,
//! CreateURLMonikerEx and the moniker they make -- all of it the part
//! that needs no URL fetched, which is what Word asks for on its way up
//! (ROADMAP OF).
//!
//! MkParseDisplayNameEx is ole32's MkParseDisplayName with a scheme
//! table in front: a name beginning with one of the schemes urlmon
//! knows (ftp, http, https, file, mailto, telnet, mk, about, res,
//! javascript, vbscript, local, its, ms-its) becomes a URL moniker, and
//! every other name -- a file path, an unknown scheme, a prefix of a
//! known one, a one-letter drive -- gets ole32's answer unchanged. Each
//! of the four NULL arguments and the empty name is E_INVALIDARG with
//! both out parameters untouched, where every other refusal zeroes
//! them.
//!
//! The canonical name, one transformation at a time (all of it measured
//! on Windows 11, `user/urlmon_test`):
//!
//! - the scheme is matched without regard to case and lowercased,
//!   except `local:`, which is kept exactly as written, scheme and all;
//! - trailing whitespace is trimmed and an inner tab, carriage return
//!   or newline is removed, the host included; leading whitespace hides
//!   the scheme altogether, so the name goes to ole32;
//! - a scheme with a host (ftp, http, https, telnet) and a `//` after
//!   its colon has its host lowercased and its escapes decoded, its
//!   user information kept in the case it was written, its own default
//!   port removed (80, 443, 21, 23) along with an empty one -- a
//!   wrong-scheme default and `:0` stay -- its backslashes turned into
//!   slashes, its dot segments resolved, and an empty path turned into
//!   `/`; the query and the fragment are kept exactly and imply that
//!   slash. Without the `//` the name is opaque;
//! - an escape of an unreserved character is decoded in the path and
//!   the host and nowhere else, and a decoded dot then takes part in
//!   the dot segments like any other; everything else -- `%2F`, `%25`,
//!   `%GG`, `%4` -- is kept exactly, in the case it was written;
//! - `file:` is a Windows path: `file:///C:/x` and `file://C:/x` are
//!   `file://C:\x`, a `localhost` or empty authority is dropped, a
//!   drive written with a bar becomes a colon, any other authority
//!   becomes a UNC root (`file://server/share/x` is
//!   `file://\\server\share\x`) with its host lowercased, and the path
//!   keeps its case, its spaces and its dot segments;
//! - `its:` and `ms-its:` gain `::/` where they have no `::` of their
//!   own; every other scheme is opaque, keeping everything after the
//!   colon as written.
//!
//! The moniker answers IUnknown, IMoniker, IPersist, IPersistStream and
//! IAsyncMoniker as itself, is always dirty, hashes and compares by the
//! canonical name (case and all), has no inverse and no common prefix,
//! reduces to itself, runs nothing, composes with another URL moniker
//! into the right-hand one and with anything else into a generic
//! composite (an anti moniker does not annihilate it), tells a relative
//! path from another URL under the same scheme and host, and parses a
//! name after it by resolving it against its own. Save writes the
//! length, the name and the 24 bytes Windows writes after it, which
//! Load reads back.
//!
//! The limits, each recording its own ledger row rather than answering
//! with a lie: nothing is fetched, so BindToObject, BindToStorage and
//! GetTimeOfLastChange are E_NOTIMPL; the IROTData, IUriContainer and
//! IMarshal tear-offs are E_NOINTERFACE; and a URL longer than 1024
//! characters is refused with E_OUTOFMEMORY.

use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

#[cfg_attr(
    target_arch = "x86",
    link(name = "ntdll", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "ntdll", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GalaRecordMissing(name: *const u8, length: usize, what: u32, detail: u64) -> u32;
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GetProcessHeap() -> usize;
    fn HeapAlloc(heap: usize, flags: u32, size: usize) -> *mut u8;
    fn HeapFree(heap: usize, flags: u32, p: *mut u8) -> i32;
    fn LoadLibraryW(name: *const u16) -> usize;
    fn GetProcAddress(module: usize, name: *const u8) -> usize;
}

type HResult = i32;
type Guid = [u8; 16];

const S_OK: HResult = 0;
const S_FALSE: HResult = 1;
const E_NOTIMPL: HResult = 0x8000_4001u32 as i32;
const E_NOINTERFACE: HResult = 0x8000_4002u32 as i32;
const E_POINTER: HResult = 0x8000_4003u32 as i32;
const E_FAIL: HResult = 0x8000_4005u32 as i32;
const E_INVALIDARG: HResult = 0x8007_0057u32 as i32;
const E_OUTOFMEMORY: HResult = 0x8007_000eu32 as i32;
const MK_E_SYNTAX: HResult = 0x8004_01e4u32 as i32;
const MK_E_NEEDGENERIC: HResult = 0x8004_01e2u32 as i32;
const MK_E_NOINVERSE: HResult = 0x8004_01ecu32 as i32;
const MK_S_REDUCED: HResult = 0x0004_01e2;
const MK_S_HIM: HResult = 0x0004_01e5;

/// MKSYS_URLMONIKER.
const MKSYS_URLMONIKER: u32 = 6;

/// The ledger's kinds: "unsupported" (answered E_NOTIMPL; the process
/// goes on), "other".
const LEDGER_UNSUPPORTED: u32 = 2;
const LEDGER_OTHER: u32 = 5;

/// The longest URL a moniker here carries. Longer is recorded and
/// refused, as nothing of Word's has needed more.
const MAX_URL: usize = 1024;

/// A GUID in its binary layout from its text form's fields.
const fn guid(d1: u32, d2: u16, d3: u16, d4: [u8; 8]) -> Guid {
    let a = d1.to_le_bytes();
    let b = d2.to_le_bytes();
    let c = d3.to_le_bytes();
    [
        a[0], a[1], a[2], a[3], b[0], b[1], c[0], c[1], d4[0], d4[1], d4[2], d4[3], d4[4], d4[5],
        d4[6], d4[7],
    ]
}

const COM: [u8; 8] = [0xc0, 0, 0, 0, 0, 0, 0, 0x46];
const IID_IUNKNOWN: Guid = guid(0, 0, 0, COM);
const IID_IMONIKER: Guid = guid(0x0f, 0, 0, COM);
const IID_IPERSIST: Guid = guid(0x010c, 0, 0, COM);
const IID_IPERSISTSTREAM: Guid = guid(0x0109, 0, 0, COM);
const IID_IMARSHAL: Guid = guid(3, 0, 0, COM);
/// {79EAC9D3-BAF9-11CE-8C82-00AA004BA90B}
const IID_IASYNCMONIKER: Guid =
    guid(0x79eac9d3, 0xbaf9, 0x11ce, [0x8c, 0x82, 0x00, 0xaa, 0x00, 0x4b, 0xa9, 0x0b]);
/// {79EAC9E0-BAF9-11CE-8C82-00AA004BA90B}, CLSID_StdURLMoniker.
const CLSID_STDURLMONIKER: Guid =
    guid(0x79eac9e0, 0xbaf9, 0x11ce, [0x8c, 0x82, 0x00, 0xaa, 0x00, 0x4b, 0xa9, 0x0b]);
/// {F29F6BC0-5021-11CE-AA15-00006901293F}
const IID_IROTDATA: Guid =
    guid(0xf29f6bc0, 0x5021, 0x11ce, [0xaa, 0x15, 0x00, 0x00, 0x69, 0x01, 0x29, 0x3f]);
/// {A158A630-ED6F-45FB-B987-F68676F57752}
const IID_IURICONTAINER: Guid =
    guid(0xa158a630, 0xed6f, 0x45fb, [0xb9, 0x87, 0xf6, 0x86, 0x76, 0xf5, 0x77, 0x52]);

/// The 24 bytes Windows writes after the name: a GUID and two words
/// nothing has been seen to read, copied as measured so a stream saved
/// here loads anywhere.
const TAIL: [u8; 24] = [
    0x79, 0x58, 0x81, 0xf4, 0x3b, 0x1d, 0x7f, 0x48, 0xaf, 0x2c, 0x82, 0x5d, 0xc4, 0x85, 0x27, 0x63,
    0x00, 0x00, 0x00, 0x00, 0xa5, 0xab, 0x00, 0x03,
];

/// Reports `parts` to the ledger as `what` (printable ASCII).
fn record(parts: &[&[u8]], what: u32, detail: u64) {
    let mut line = [0u8; 200];
    let mut n = 0;
    for &b in parts.iter().flat_map(|p| p.iter()) {
        if n == line.len() {
            break;
        }
        line[n] = if (0x20..0x7f).contains(&b) && b != b'"' && b != b'\\' { b } else { b'?' };
        n += 1;
    }
    // SAFETY: a valid buffer of n bytes.
    unsafe { GalaRecordMissing(line.as_ptr(), n, what, detail) };
}

/// A name too long for `MAX_URL`, recorded once per entry point.
fn too_long(what: &[u8], len: usize) -> HResult {
    record(&[b"urlmon.dll!", what, b"(URL longer than 1024)"], LEDGER_UNSUPPORTED, len as u64);
    E_OUTOFMEMORY
}

fn put<T>(out: *mut T, value: T) {
    // SAFETY: the caller's output pointer (checked non-null by callers).
    unsafe { out.write(value) };
}

// ---- ole32, resolved when first needed -------------------------------

/// ole32's entry points, resolved on first use: a static import would
/// make every program that loads urlmon load ole32 as well.
static OLE32: [AtomicUsize; 4] =
    [AtomicUsize::new(0), AtomicUsize::new(0), AtomicUsize::new(0), AtomicUsize::new(0)];

const PARSE: usize = 0;
const COMPOSITE: usize = 1;
const TASK_ALLOC: usize = 2;
const TASK_FREE: usize = 3;

/// ole32's `name`, or 0 (recorded) when it cannot be had.
fn ole32(which: usize, name: &[u8]) -> usize {
    let slot = &OLE32[which];
    let got = slot.load(Ordering::Relaxed);
    if got != 0 {
        return got;
    }
    // "ole32.dll"
    const DLL: [u16; 10] = [111, 108, 101, 51, 50, 46, 100, 108, 108, 0];
    // SAFETY: a NUL-terminated module name and a NUL-terminated export
    // name (every caller passes a `c"..."` literal's bytes).
    let f = unsafe {
        let module = LoadLibraryW(DLL.as_ptr());
        if module == 0 { 0 } else { GetProcAddress(module, name.as_ptr()) }
    };
    if f == 0 {
        record(&[b"urlmon.dll!ole32.dll(", &name[..name.len() - 1], b")"], LEDGER_UNSUPPORTED, 0);
        return 0;
    }
    slot.store(f, Ordering::Relaxed);
    f
}

/// ole32's MkParseDisplayName, for every name urlmon does not know.
fn parse_in_ole32(
    bc: *mut c_void,
    name: *const u16,
    eaten: *mut u32,
    out: *mut *mut c_void,
) -> HResult {
    let f = ole32(PARSE, c"MkParseDisplayName".to_bytes_with_nul());
    if f == 0 {
        put(eaten, 0);
        put(out, core::ptr::null_mut());
        return MK_E_SYNTAX;
    }
    // SAFETY: ole32's MkParseDisplayName(IBindCtx*, LPWSTR, ULONG*,
    // IMoniker**), the caller's arguments passed on.
    unsafe {
        let f: extern "system" fn(*mut c_void, *const u16, *mut u32, *mut *mut c_void) -> HResult =
            core::mem::transmute(f);
        f(bc, name, eaten, out)
    }
}

/// ole32's CreateGenericComposite: it asks this moniker to compose with
/// a composite refused, and joins the two display names when it will
/// not.
fn generic_composite(first: *mut c_void, rest: *mut c_void, out: *mut *mut c_void) -> HResult {
    let f = ole32(COMPOSITE, c"CreateGenericComposite".to_bytes_with_nul());
    if f == 0 {
        return E_NOTIMPL;
    }
    // SAFETY: ole32's CreateGenericComposite(IMoniker*, IMoniker*,
    // IMoniker**).
    unsafe {
        let f: extern "system" fn(*mut c_void, *mut c_void, *mut *mut c_void) -> HResult =
            core::mem::transmute(f);
        f(first, rest, out)
    }
}

/// A copy of `text` and a NUL in task memory, which the caller frees
/// with CoTaskMemFree.
fn task_copy(text: &[u16]) -> *mut u16 {
    let f = ole32(TASK_ALLOC, c"CoTaskMemAlloc".to_bytes_with_nul());
    if f == 0 {
        return core::ptr::null_mut();
    }
    // SAFETY: ole32's CoTaskMemAlloc(SIZE_T); the block it returns is
    // ours to fill, and is text.len() + 1 units long.
    unsafe {
        let f: extern "system" fn(usize) -> *mut c_void = core::mem::transmute(f);
        let p = f((text.len() + 1) * 2).cast::<u16>();
        if p.is_null() {
            return p;
        }
        for (i, &c) in text.iter().enumerate() {
            p.add(i).write(c);
        }
        p.add(text.len()).write(0);
        p
    }
}

fn task_free(p: *mut u16) {
    let f = ole32(TASK_FREE, c"CoTaskMemFree".to_bytes_with_nul());
    if f == 0 || p.is_null() {
        return;
    }
    // SAFETY: ole32's CoTaskMemFree(LPVOID) on a block it allocated.
    unsafe {
        let f: extern "system" fn(*mut c_void) = core::mem::transmute(f);
        f(p.cast());
    }
}

// ---- text helpers ---------------------------------------------------

const fn ch(c: u8) -> u16 {
    c as u16
}

fn is_alpha(c: u16) -> bool {
    (ch(b'a')..=ch(b'z')).contains(&c) || (ch(b'A')..=ch(b'Z')).contains(&c)
}

fn is_digit(c: u16) -> bool {
    (ch(b'0')..=ch(b'9')).contains(&c)
}

fn lower(c: u16) -> u16 {
    if (ch(b'A')..=ch(b'Z')).contains(&c) { c + 32 } else { c }
}

fn is_sep(c: u16) -> bool {
    c == ch(b'/') || c == ch(b'\\')
}

fn eq_ci(text: &[u16], ascii: &[u8]) -> bool {
    text.len() == ascii.len() && text.iter().zip(ascii).all(|(&c, &b)| lower(c) == lower(ch(b)))
}

fn same_text(a: &[u16], b: &[u16]) -> bool {
    a == b
}

/// Appends `text`, or None when it does not fit.
fn push(out: &mut [u16], at: usize, text: &[u16]) -> Option<usize> {
    let end = at.checked_add(text.len())?;
    if end > out.len() {
        return None;
    }
    out[at..end].copy_from_slice(text);
    Some(end)
}

fn push_ascii(out: &mut [u16], at: usize, text: &[u8]) -> Option<usize> {
    let mut at = at;
    for &b in text {
        at = push_one(out, at, ch(b))?;
    }
    Some(at)
}

fn push_one(out: &mut [u16], at: usize, c: u16) -> Option<usize> {
    if at >= out.len() {
        return None;
    }
    out[at] = c;
    Some(at + 1)
}

/// The caller's NUL-terminated name into `out`: how many units it took,
/// and whether that was all of it.
fn fill(name: *const u16, out: &mut [u16; MAX_URL]) -> (usize, bool) {
    let mut n = 0;
    while n < out.len() {
        // SAFETY: the caller's NUL-terminated string, read to its NUL.
        let c = unsafe { *name.add(n) };
        if c == 0 {
            return (n, true);
        }
        out[n] = c;
        n += 1;
    }
    // SAFETY: as above: the unit that would follow what we took.
    (n, unsafe { *name.add(n) } == 0)
}

/// A hex digit's value.
fn hex(c: u16) -> Option<u8> {
    let c = u8::try_from(c).ok()?;
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// RFC 3986's unreserved characters, the only escapes that are decoded.
fn unreserved(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'-' || b == b'.' || b == b'_' || b == b'~'
}

/// The character at `i`, decoded when it is an escape of an unreserved
/// one, and how many units it took.
fn decode_at(s: &[u16], i: usize) -> (u16, usize) {
    if s[i] == ch(b'%')
        && i + 2 < s.len()
        && let (Some(hi), Some(lo)) = (hex(s[i + 1]), hex(s[i + 2]))
    {
        let b = hi * 16 + lo;
        if unreserved(b) {
            return (ch(b), 3);
        }
    }
    (s[i], 1)
}

/// `path` with its escapes decoded and, when `slashes`, its backslashes
/// turned into slashes.
fn decoded(path: &[u16], out: &mut [u16], slashes: bool) -> Option<usize> {
    let mut at = 0;
    let mut i = 0;
    while i < path.len() {
        let (c, step) = decode_at(path, i);
        let c = if slashes && c == ch(b'\\') { ch(b'/') } else { c };
        at = push_one(out, at, c)?;
        i += step;
    }
    Some(at)
}

/// The host, lowercased and decoded.
fn push_host(out: &mut [u16], at: usize, host: &[u16]) -> Option<usize> {
    let mut at = at;
    let mut i = 0;
    while i < host.len() {
        let (c, step) = decode_at(host, i);
        at = push_one(out, at, lower(c))?;
        i += step;
    }
    Some(at)
}

/// Drops the last segment of the path written at `out[base..at]`, its
/// separator with it.
fn climb(out: &[u16], base: usize, at: usize) -> usize {
    match out[base..at].iter().rposition(|&c| c == ch(b'/')) {
        Some(i) => base + i,
        None => base,
    }
}

/// Resolves `.` and `..` in `path` (slashes only, already decoded) into
/// `out` at `base`, which is the floor the climb stops at. A `.` or
/// `..` at the end of the path leaves the separator behind it, as
/// Windows leaves it.
fn resolve_dots(path: &[u16], out: &mut [u16], base: usize) -> Option<usize> {
    let mut at = base;
    let mut i = 0;
    while i < path.len() {
        if path[i] != ch(b'/') {
            // A path that does not begin with a separator: its first
            // segment stands as written.
            let e = path[i..].iter().position(|&c| c == ch(b'/')).map_or(path.len(), |p| i + p);
            at = push(out, at, &path[i..e])?;
            i = e;
            continue;
        }
        let s = i + 1;
        let e = path[s..].iter().position(|&c| c == ch(b'/')).map_or(path.len(), |p| s + p);
        let segment = &path[s..e];
        let last = e == path.len();
        if segment.len() == 1 && segment[0] == ch(b'.') {
            if last {
                at = push_one(out, at, ch(b'/'))?;
            }
        } else if segment.len() == 2 && segment[0] == ch(b'.') && segment[1] == ch(b'.') {
            at = climb(out, base, at);
            if last {
                at = push_one(out, at, ch(b'/'))?;
            }
        } else {
            at = push_one(out, at, ch(b'/'))?;
            at = push(out, at, segment)?;
        }
        i = e;
    }
    Some(at)
}

// ---- the scheme table and the canonical name ------------------------

/// What a scheme's remainder looks like.
#[derive(Copy, Clone, PartialEq, Eq)]
enum Shape {
    /// `//host[:port]/path`, the scheme's own default port removed.
    Host(u32),
    /// A Windows path, with its own shape.
    File,
    /// Everything after the colon, exactly as written.
    Opaque,
    /// Opaque, with `::/` appended where there is no `::`.
    Its,
    /// Nothing at all is touched, the scheme's own case included.
    Local,
}

/// The schemes urlmon parses. Every other name is ole32's.
static SCHEMES: [(&[u8], Shape); 14] = [
    (b"ftp", Shape::Host(21)),
    (b"http", Shape::Host(80)),
    (b"https", Shape::Host(443)),
    (b"file", Shape::File),
    (b"mailto", Shape::Opaque),
    (b"telnet", Shape::Host(23)),
    (b"mk", Shape::Opaque),
    (b"about", Shape::Opaque),
    (b"res", Shape::Opaque),
    (b"javascript", Shape::Opaque),
    (b"vbscript", Shape::Opaque),
    (b"local", Shape::Local),
    (b"its", Shape::Its),
    (b"ms-its", Shape::Its),
];

/// Where the scheme's colon is, for a name that begins with one.
fn scheme_end(s: &[u16]) -> Option<usize> {
    if s.is_empty() || !is_alpha(s[0]) {
        return None;
    }
    for (i, &c) in s.iter().enumerate().skip(1) {
        if c == ch(b':') {
            return Some(i);
        }
        if !(is_alpha(c) || is_digit(c) || c == ch(b'+') || c == ch(b'-') || c == ch(b'.')) {
            return None;
        }
    }
    None
}

/// The table's row for the scheme `s` begins with, if it has one.
fn scheme_of(s: &[u16]) -> Option<(usize, &'static [u8], Shape)> {
    let colon = scheme_end(s)?;
    let (name, shape) = SCHEMES.iter().find(|(name, _)| eq_ci(&s[..colon], name))?;
    Some((colon, name, *shape))
}

/// What `canonical` made of a display name.
enum Canon {
    /// Not a scheme urlmon knows: the name is ole32's.
    NotOurs,
    /// `out[..n]` is the canonical name.
    Ours(usize),
    /// A name urlmon knows but cannot carry.
    Failed(HResult),
}

/// The canonical form of `given` (its scheme's rules, module doc).
fn canonical(given: &[u16], out: &mut [u16; MAX_URL], what: &[u8]) -> Canon {
    // A tab, a carriage return or a newline is removed wherever it is,
    // and trailing whitespace is trimmed.
    let mut clean = [0u16; MAX_URL];
    let mut n = 0;
    for &c in given {
        if c == 9 || c == 10 || c == 13 {
            continue;
        }
        match push_one(&mut clean, n, c) {
            Some(at) => n = at,
            None => return Canon::Failed(too_long(what, given.len())),
        }
    }
    while n > 0 && clean[n - 1] == ch(b' ') {
        n -= 1;
    }
    let clean = &clean[..n];
    let Some((colon, name, shape)) = scheme_of(clean) else { return Canon::NotOurs };
    if shape == Shape::Local {
        return match push(out, 0, clean) {
            Some(end) => Canon::Ours(end),
            None => Canon::Failed(too_long(what, clean.len())),
        };
    }
    let rest = &clean[colon + 1..];
    let built =
        push_ascii(out, 0, name).and_then(|at| push_one(out, at, ch(b':'))).and_then(|at| {
            match shape {
                Shape::Host(port) => with_host(rest, port, out, at),
                Shape::File => as_file(rest, out, at),
                Shape::Its => {
                    let at = push(out, at, rest)?;
                    if rest.windows(2).any(|w| w[0] == ch(b':') && w[1] == ch(b':')) {
                        Some(at)
                    } else {
                        push_ascii(out, at, b"::/")
                    }
                }
                Shape::Opaque | Shape::Local => push(out, at, rest),
            }
        });
    match built {
        Some(end) => Canon::Ours(end),
        None => Canon::Failed(too_long(what, clean.len())),
    }
}

/// The host and the port of an authority (`[::1]`'s colons are its
/// own).
fn split_port(s: &[u16]) -> (&[u16], Option<&[u16]>) {
    let from = if s.first() == Some(&ch(b'[')) {
        match s.iter().position(|&c| c == ch(b']')) {
            Some(i) => i,
            None => return (s, None),
        }
    } else {
        0
    };
    match s[from..].iter().rposition(|&c| c == ch(b':')) {
        Some(i) => (&s[..from + i], Some(&s[from + i + 1..])),
        None => (s, None),
    }
}

/// Whether the port as written is the number `want`.
fn is_port(port: &[u16], want: u32) -> bool {
    if port.is_empty() || !port.iter().all(|&c| is_digit(c)) {
        return false;
    }
    let mut value = 0u32;
    for &c in port {
        value = match value.checked_mul(10).and_then(|v| v.checked_add(u32::from(c - ch(b'0')))) {
            Some(v) => v,
            None => return false,
        };
    }
    value == want
}

/// A scheme that takes a host: the authority, the path, and the query
/// and fragment as written. Without the `//` the name is opaque.
fn with_host(rest: &[u16], port: u32, out: &mut [u16; MAX_URL], at: usize) -> Option<usize> {
    if !(rest.len() >= 2 && rest[0] == ch(b'/') && rest[1] == ch(b'/')) {
        return push(out, at, rest);
    }
    let body = &rest[2..];
    let mut at = push_ascii(out, at, b"//")?;
    let end = body
        .iter()
        .position(|&c| is_sep(c) || c == ch(b'?') || c == ch(b'#'))
        .unwrap_or(body.len());
    let authority = &body[..end];
    let tail = &body[end..];
    let (user, hostport) = match authority.iter().rposition(|&c| c == ch(b'@')) {
        Some(i) => (Some(&authority[..i]), &authority[i + 1..]),
        None => (None, authority),
    };
    if let Some(user) = user {
        at = push(out, at, user)?;
        at = push_one(out, at, ch(b'@'))?;
    }
    let (host, given_port) = split_port(hostport);
    at = push_host(out, at, host)?;
    if let Some(given_port) = given_port
        && !given_port.is_empty()
        && !is_port(given_port, port)
    {
        at = push_one(out, at, ch(b':'))?;
        at = push(out, at, given_port)?;
    }
    let path_end = tail.iter().position(|&c| c == ch(b'?') || c == ch(b'#')).unwrap_or(tail.len());
    let path = &tail[..path_end];
    if path.is_empty() {
        at = push_one(out, at, ch(b'/'))?;
    } else {
        let mut work = [0u16; MAX_URL];
        let end = decoded(path, &mut work, true)?;
        at = resolve_dots(&work[..end], out, at)?;
    }
    push(out, at, &tail[path_end..])
}

/// A drive, written with a colon or with a bar.
fn is_drive(s: &[u16]) -> bool {
    s.len() == 2 && is_alpha(s[0]) && (s[1] == ch(b':') || s[1] == ch(b'|'))
}

/// `file:`: a Windows path, with the authority it may carry (module
/// doc). Nothing in the path is decoded or resolved.
fn as_file(rest: &[u16], out: &mut [u16; MAX_URL], at: usize) -> Option<usize> {
    if !(rest.len() >= 2 && rest[0] == ch(b'/') && rest[1] == ch(b'/')) {
        return push(out, at, rest);
    }
    let body = &rest[2..];
    let mut at = push_ascii(out, at, b"//")?;
    if body.is_empty() {
        return Some(at);
    }
    // A UNC path already written as one stands as it is.
    if body.len() >= 2 && body[0] == ch(b'\\') && body[1] == ch(b'\\') {
        return push(out, at, body);
    }
    let local;
    if is_sep(body[0]) {
        local = &body[1..];
    } else {
        let e = body.iter().position(|&c| is_sep(c)).unwrap_or(body.len());
        let head = &body[..e];
        let after = &body[e..];
        if eq_ci(head, b"localhost") {
            local = if after.is_empty() { after } else { &after[1..] };
        } else if is_drive(head) {
            at = push_one(out, at, head[0])?;
            at = push_one(out, at, ch(b':'))?;
            local = after;
        } else {
            at = push_ascii(out, at, b"\\\\")?;
            at = push_host(out, at, head)?;
            local = after;
        }
    }
    // A drive at the start of the path, its bar turned into a colon.
    let mut from = 0;
    if local.len() >= 2 && is_drive(&local[..2]) && (local.len() == 2 || is_sep(local[2])) {
        at = push_one(out, at, local[0])?;
        at = push_one(out, at, ch(b':'))?;
        from = 2;
    }
    for &c in &local[from..] {
        at = push_one(out, at, if c == ch(b'/') { ch(b'\\') } else { c })?;
    }
    Some(at)
}

/// A name resolved against `base`: one with a scheme of its own stands
/// alone, one that begins with a separator takes the base's host, and
/// anything else takes the base's directory.
fn resolved(base: &[u16], rel: &[u16], out: &mut [u16; MAX_URL]) -> Option<usize> {
    if scheme_of(rel).is_some() {
        return push(out, 0, rel);
    }
    let cut = base.iter().position(|&c| c == ch(b'?') || c == ch(b'#')).unwrap_or(base.len());
    let head = &base[..cut];
    if !rel.is_empty() && is_sep(rel[0]) {
        let start = match head.windows(2).position(|w| w[0] == ch(b'/') && w[1] == ch(b'/')) {
            Some(i) => i + 2,
            None => head.len(),
        };
        let end = head[start..].iter().position(|&c| is_sep(c)).map_or(head.len(), |i| start + i);
        let at = push(out, 0, &head[..end])?;
        return push(out, at, rel);
    }
    let dir = match head.iter().rposition(|&c| is_sep(c)) {
        Some(i) => i + 1,
        None => head.len(),
    };
    let at = push(out, 0, &head[..dir])?;
    push(out, at, rel)
}

/// The canonical name `given` resolves to against `base` (which may be
/// empty: no base at all), into `out`. A name whose scheme urlmon does
/// not know is kept as it stands, as Windows keeps it.
fn resolved_canonical(base: &[u16], given: &[u16], out: &mut [u16; MAX_URL], what: &[u8]) -> Canon {
    if base.is_empty() {
        return match canonical(given, out, what) {
            Canon::NotOurs => match push(out, 0, given) {
                Some(end) => Canon::Ours(end),
                None => Canon::Failed(too_long(what, given.len())),
            },
            other => other,
        };
    }
    let mut joined = [0u16; MAX_URL];
    let Some(end) = resolved(base, given, &mut joined) else {
        return Canon::Failed(too_long(what, given.len()));
    };
    match canonical(&joined[..end], out, what) {
        Canon::NotOurs => match push(out, 0, &joined[..end]) {
            Some(end) => Canon::Ours(end),
            None => Canon::Failed(too_long(what, end)),
        },
        other => other,
    }
}

// ---- the moniker ----------------------------------------------------

#[repr(C)]
struct Url {
    vtable: *const usize,
    refs: AtomicU32,
    len: u32,
    name: [u16; MAX_URL],
}

fn url<'a>(this: *mut Url) -> &'a mut Url {
    // SAFETY: our object, alive while the caller holds a reference.
    unsafe { &mut *this }
}

impl Url {
    fn text(&self) -> &[u16] {
        &self.name[..self.len as usize]
    }
}

/// Whether `obj` is a URL moniker of ours (its vtable says so).
fn ours(obj: *mut c_void) -> bool {
    if obj.is_null() {
        return false;
    }
    // SAFETY: a COM object: its first field is its vtable pointer.
    let vtable = unsafe { *obj.cast::<*const usize>() };
    vtable == VTABLE.0.as_ptr().cast()
}

/// A new URL moniker carrying `name`, or null when the heap is spent.
fn new_url(name: &[u16]) -> *mut Url {
    if name.len() > MAX_URL {
        return core::ptr::null_mut();
    }
    // SAFETY: plain allocation of the object, initialized below.
    let p = unsafe { HeapAlloc(GetProcessHeap(), 0, size_of::<Url>()) }.cast::<Url>();
    if p.is_null() {
        return p;
    }
    let mut buffer = [0u16; MAX_URL];
    buffer[..name.len()].copy_from_slice(name);
    // SAFETY: a fresh block of the object's size (heap blocks are
    // aligned for it).
    unsafe {
        p.write(Url {
            vtable: VTABLE.0.as_ptr().cast(),
            refs: AtomicU32::new(1),
            len: name.len() as u32,
            name: buffer,
        })
    };
    p
}

/// Hands `p` to the caller as S_OK, or turns a failed `new_url` into
/// its HRESULT.
fn handed(p: *mut Url, out: *mut *mut c_void) -> HResult {
    if p.is_null() {
        put(out, core::ptr::null_mut());
        return E_OUTOFMEMORY;
    }
    put(out, p.cast::<c_void>());
    S_OK
}

/// Another moniker's display name: a read for one of ours, two calls
/// for a moniker from elsewhere (recorded, as gala has met none yet).
fn other_name(obj: *mut c_void, out: &mut [u16; MAX_URL]) -> Option<usize> {
    if obj.is_null() {
        return None;
    }
    if ours(obj) {
        let other = url(obj.cast::<Url>());
        let len = other.len as usize;
        out[..len].copy_from_slice(other.text());
        return Some(len);
    }
    record(&[b"urlmon.dll!IMoniker(a moniker from elsewhere)"], LEDGER_OTHER, 0);
    let mut text: *mut u16 = core::ptr::null_mut();
    // SAFETY: a COM object: IMoniker::GetDisplayName is slot 20, and a
    // URL moniker's own takes no bind context either.
    let hr = unsafe {
        let vtable = *obj.cast::<*const usize>();
        let f: extern "system" fn(*mut c_void, *mut c_void, *mut c_void, *mut *mut u16) -> HResult =
            core::mem::transmute(*vtable.add(20));
        f(obj, core::ptr::null_mut(), core::ptr::null_mut(), &mut text)
    };
    if hr < 0 || text.is_null() {
        return None;
    }
    let (len, whole) = fill(text, out);
    task_free(text);
    if whole { Some(len) } else { None }
}

// ---- IMoniker -------------------------------------------------------

struct VTable<const N: usize>([*const (); N]);
// SAFETY: function addresses only.
unsafe impl<const N: usize> Sync for VTable<N> {}

extern "system" fn query(this: *mut Url, iid: *const Guid, out: *mut *mut c_void) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    if iid.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's IID.
    let iid = unsafe { *iid };
    if iid == IID_IUNKNOWN
        || iid == IID_IMONIKER
        || iid == IID_IPERSIST
        || iid == IID_IPERSISTSTREAM
        || iid == IID_IASYNCMONIKER
    {
        add_ref(this);
        put(out, this.cast::<c_void>());
        return S_OK;
    }
    // The tear-offs a URL moniker has on Windows and gala has not.
    let named: &[u8] = if iid == IID_IROTDATA {
        b"IROTData"
    } else if iid == IID_IURICONTAINER {
        b"IUriContainer"
    } else if iid == IID_IMARSHAL {
        b"IMarshal"
    } else {
        b""
    };
    if !named.is_empty() {
        record(&[b"urlmon.dll!URL moniker::QueryInterface(", named, b")"], LEDGER_UNSUPPORTED, 0);
    }
    E_NOINTERFACE
}

extern "system" fn add_ref(this: *mut Url) -> u32 {
    url(this).refs.fetch_add(1, Ordering::Relaxed) + 1
}

extern "system" fn release(this: *mut Url) -> u32 {
    let left = url(this).refs.fetch_sub(1, Ordering::AcqRel) - 1;
    if left == 0 {
        // SAFETY: the block `new_url` allocated, no longer referenced.
        unsafe { HeapFree(GetProcessHeap(), 0, this.cast::<u8>()) };
    }
    left
}

extern "system" fn get_class_id(_this: *mut Url, out: *mut Guid) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, CLSID_STDURLMONIKER);
    S_OK
}

/// IsDirty: a URL moniker reports itself dirty, always (measured).
extern "system" fn is_dirty(_this: *mut Url) -> HResult {
    S_OK
}

/// IStream::Read (slot 3) of exactly `bytes.len()` bytes.
fn stream_read(stream: *mut c_void, bytes: &mut [u8]) -> HResult {
    let mut got = 0u32;
    // SAFETY: a live IStream: Read(pv, cb, pcbRead).
    let hr = unsafe {
        let vtable = *stream.cast::<*const usize>();
        let f: extern "system" fn(*mut c_void, *mut u8, u32, *mut u32) -> HResult =
            core::mem::transmute(*vtable.add(3));
        f(stream, bytes.as_mut_ptr(), bytes.len() as u32, &mut got)
    };
    if hr < 0 {
        return hr;
    }
    if got as usize != bytes.len() { E_FAIL } else { S_OK }
}

/// IStream::Write (slot 4) of all of `bytes`.
fn stream_write(stream: *mut c_void, bytes: &[u8]) -> HResult {
    let mut written = 0u32;
    // SAFETY: a live IStream: Write(pv, cb, pcbWritten).
    let hr = unsafe {
        let vtable = *stream.cast::<*const usize>();
        let f: extern "system" fn(*mut c_void, *const u8, u32, *mut u32) -> HResult =
            core::mem::transmute(*vtable.add(4));
        f(stream, bytes.as_ptr(), bytes.len() as u32, &mut written)
    };
    if hr < 0 {
        return hr;
    }
    if written as usize != bytes.len() { E_FAIL } else { S_OK }
}

/// The name's bytes, UTF-16 with its NUL.
fn name_bytes(text: &[u16], out: &mut [u8; (MAX_URL + 1) * 2]) -> usize {
    for (i, &c) in text.iter().enumerate() {
        out[i * 2..i * 2 + 2].copy_from_slice(&c.to_le_bytes());
    }
    out[text.len() * 2] = 0;
    out[text.len() * 2 + 1] = 0;
    (text.len() + 1) * 2
}

/// Load: the length, the name and the bytes after it, as Save wrote
/// them.
extern "system" fn load(this: *mut Url, stream: *mut c_void) -> HResult {
    if stream.is_null() {
        return E_INVALIDARG;
    }
    let mut head = [0u8; 4];
    let hr = stream_read(stream, &mut head);
    if hr < 0 {
        return hr;
    }
    let length = u32::from_le_bytes(head) as usize;
    if length < TAIL.len() + 2 || !(length - TAIL.len()).is_multiple_of(2) {
        return E_FAIL;
    }
    let bytes = length - TAIL.len();
    if bytes > (MAX_URL + 1) * 2 {
        return too_long(b"IMoniker::Load", bytes / 2);
    }
    let mut buffer = [0u8; (MAX_URL + 1) * 2];
    let hr = stream_read(stream, &mut buffer[..bytes]);
    if hr < 0 {
        return hr;
    }
    let mut tail = [0u8; TAIL.len()];
    let hr = stream_read(stream, &mut tail);
    if hr < 0 {
        return hr;
    }
    let mut name = [0u16; MAX_URL];
    let mut len = 0;
    while len < bytes / 2 {
        let c = u16::from_le_bytes([buffer[len * 2], buffer[len * 2 + 1]]);
        if c == 0 {
            break;
        }
        name[len] = c;
        len += 1;
    }
    let m = url(this);
    m.name = name;
    m.len = len as u32;
    S_OK
}

/// Save: the length of everything after it, the name with its NUL, and
/// the 24 bytes Windows writes after that.
extern "system" fn save(this: *mut Url, stream: *mut c_void, _clear: i32) -> HResult {
    if stream.is_null() {
        return E_INVALIDARG;
    }
    let m = url(this);
    let mut buffer = [0u8; (MAX_URL + 1) * 2];
    let bytes = name_bytes(m.text(), &mut buffer);
    let length = (bytes + TAIL.len()) as u32;
    let hr = stream_write(stream, &length.to_le_bytes());
    if hr < 0 {
        return hr;
    }
    let hr = stream_write(stream, &buffer[..bytes]);
    if hr < 0 {
        return hr;
    }
    stream_write(stream, &TAIL)
}

/// GetSizeMax: what Save writes, to the byte.
extern "system" fn get_size_max(this: *mut Url, out: *mut u64) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    let len = u64::from(url(this).len);
    put(out, 4 + (len + 1) * 2 + TAIL.len() as u64);
    S_OK
}

/// BindToObject: nothing is fetched here, so there is nothing to bind
/// to; the ledger carries the URL that was asked for.
extern "system" fn bind_to_object(
    this: *mut Url,
    _bc: *mut c_void,
    _left: *mut c_void,
    _iid: *const Guid,
    out: *mut *mut c_void,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    not_fetched(b"BindToObject", this)
}

extern "system" fn bind_to_storage(
    this: *mut Url,
    _bc: *mut c_void,
    _left: *mut c_void,
    _iid: *const Guid,
    out: *mut *mut c_void,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    not_fetched(b"BindToStorage", this)
}

/// What a method that would need the URL fetched records.
fn not_fetched(method: &[u8], this: *mut Url) -> HResult {
    let m = url(this);
    let mut text = [0u8; 64];
    let mut n = 0;
    for &c in m.text().iter().take(text.len()) {
        text[n] = if c < 0x80 { c as u8 } else { b'?' };
        n += 1;
    }
    record(&[b"urlmon.dll!IMoniker::", method, b"(", &text[..n], b")"], LEDGER_UNSUPPORTED, 0);
    E_NOTIMPL
}

/// Reduce: a URL moniker is as reduced as it gets, and says so with
/// itself and a reference of its own.
extern "system" fn reduce(
    this: *mut Url,
    _bc: *mut c_void,
    _how: u32,
    _left: *mut *mut c_void,
    out: *mut *mut c_void,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    add_ref(this);
    put(out, this.cast::<c_void>());
    MK_S_REDUCED
}

/// ComposeWith: a URL on the right is the whole name of the
/// composition; anything else makes a generic composite, which ole32
/// builds (an anti moniker does not annihilate a URL).
extern "system" fn compose_with(
    this: *mut Url,
    right: *mut c_void,
    only_if_not_generic: i32,
    out: *mut *mut c_void,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    if right.is_null() {
        return E_INVALIDARG;
    }
    if ours(right) {
        let other = url(right.cast::<Url>());
        return handed(new_url(other.text()), out);
    }
    if only_if_not_generic != 0 {
        return MK_E_NEEDGENERIC;
    }
    generic_composite(this.cast::<c_void>(), right, out)
}

/// Enum: a URL moniker has no parts, which Windows answers with S_OK
/// and no enumerator at all.
extern "system" fn enum_parts(_this: *mut Url, _forward: i32, out: *mut *mut c_void) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    S_OK
}

/// IsEqual: the canonical name, case and all.
extern "system" fn is_equal(this: *mut Url, other: *mut c_void) -> HResult {
    if other.is_null() {
        return E_INVALIDARG;
    }
    let mut name = [0u16; MAX_URL];
    let Some(len) = other_name(other, &mut name) else { return S_FALSE };
    if same_text(url(this).text(), &name[..len]) { S_OK } else { S_FALSE }
}

/// Hash: a number equal URL monikers share.
extern "system" fn hash(this: *mut Url, out: *mut u32) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    let mut value = 0u32;
    for &c in url(this).text() {
        value = value.rotate_left(5) ^ u32::from(c);
    }
    put(out, value);
    S_OK
}

/// IsRunning: nothing runs under a URL moniker here.
extern "system" fn is_running(
    _this: *mut Url,
    _bc: *mut c_void,
    _left: *mut c_void,
    _newly: *mut c_void,
) -> HResult {
    S_FALSE
}

/// GetTimeOfLastChange: the answer is in the URL's headers, which needs
/// it fetched.
extern "system" fn time_of_last_change(
    this: *mut Url,
    _bc: *mut c_void,
    _left: *mut c_void,
    out: *mut u64,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, 0);
    not_fetched(b"GetTimeOfLastChange", this)
}

/// Inverse: a URL moniker has none (measured).
extern "system" fn inverse(_this: *mut Url, out: *mut *mut c_void) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    MK_E_NOINVERSE
}

/// CommonPrefixWith: E_NOTIMPL, as Windows answers it, whatever the
/// two URLs share.
extern "system" fn common_prefix_with(
    _this: *mut Url,
    _other: *mut c_void,
    out: *mut *mut c_void,
) -> HResult {
    if !out.is_null() {
        put(out, core::ptr::null_mut());
    }
    E_NOTIMPL
}

/// The URL's scheme and authority, and the path after them.
fn split_root(text: &[u16]) -> (&[u16], &[u16]) {
    let start = match text.windows(2).position(|w| w[0] == ch(b'/') && w[1] == ch(b'/')) {
        Some(i) => i + 2,
        None => return (text, &text[text.len()..]),
    };
    match text[start..].iter().position(|&c| c == ch(b'/')) {
        Some(i) => text.split_at(start + i),
        None => (text, &text[text.len()..]),
    }
}

/// The path's directory (up to its last separator) and the name after
/// it.
fn split_file(path: &[u16]) -> (&[u16], &[u16]) {
    match path.iter().rposition(|&c| c == ch(b'/')) {
        Some(i) => path.split_at(i + 1),
        None => (&path[..0], path),
    }
}

/// The path of `theirs` relative to `mine`: one `..` for each directory
/// of mine past the two paths' shared part.
fn relative(mine: &[u16], theirs: &[u16], out: &mut [u16; MAX_URL]) -> Option<usize> {
    let (my_dir, _) = split_file(mine);
    let (their_dir, their_file) = split_file(theirs);
    let mut shared = 0;
    while shared < my_dir.len() && shared < their_dir.len() && my_dir[shared] == their_dir[shared] {
        shared += 1;
    }
    let cut = match my_dir[..shared].iter().rposition(|&c| c == ch(b'/')) {
        Some(i) => i + 1,
        None => 0,
    };
    let mut at = 0;
    for _ in 0..my_dir[cut..].iter().filter(|&&c| c == ch(b'/')).count() {
        at = push_ascii(out, at, b"../")?;
    }
    at = push(out, at, &their_dir[cut..])?;
    push(out, at, their_file)
}

/// RelativePathTo: a relative path under the same scheme and host, the
/// other moniker itself (MK_S_HIM) where there is none.
extern "system" fn relative_path_to(
    this: *mut Url,
    other: *mut c_void,
    out: *mut *mut c_void,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    if other.is_null() {
        return E_INVALIDARG;
    }
    let mut name = [0u16; MAX_URL];
    let Some(len) = other_name(other, &mut name) else { return E_INVALIDARG };
    let theirs = &name[..len];
    let mine = url(this).text();
    let (my_root, my_path) = split_root(mine);
    let (their_root, their_path) = split_root(theirs);
    if !same_text(my_root, their_root) {
        let hr = handed(new_url(theirs), out);
        return if hr == S_OK { MK_S_HIM } else { hr };
    }
    let mut text = [0u16; MAX_URL];
    let Some(end) = relative(my_path, their_path, &mut text) else {
        return too_long(b"IMoniker::RelativePathTo", theirs.len());
    };
    handed(new_url(&text[..end]), out)
}

/// GetDisplayName: the canonical URL, in task memory.
extern "system" fn get_display_name(
    this: *mut Url,
    _bc: *mut c_void,
    _left: *mut c_void,
    out: *mut *mut u16,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    let text = task_copy(url(this).text());
    if text.is_null() {
        return E_OUTOFMEMORY;
    }
    put(out, text);
    S_OK
}

/// ParseDisplayName: the name resolved against this URL.
extern "system" fn parse_display_name(
    this: *mut Url,
    _bc: *mut c_void,
    _left: *mut c_void,
    name: *const u16,
    eaten: *mut u32,
    out: *mut *mut c_void,
) -> HResult {
    if out.is_null() || eaten.is_null() {
        return E_INVALIDARG;
    }
    put(out, core::ptr::null_mut());
    put(eaten, 0);
    if name.is_null() {
        return E_INVALIDARG;
    }
    let mut given = [0u16; MAX_URL];
    let (len, whole) = fill(name, &mut given);
    if !whole {
        return too_long(b"IMoniker::ParseDisplayName", len);
    }
    let mut text = [0u16; MAX_URL];
    let end = match resolved_canonical(
        url(this).text(),
        &given[..len],
        &mut text,
        b"IMoniker::ParseDisplayName",
    ) {
        Canon::Ours(end) => end,
        Canon::Failed(e) => return e,
        // `resolved_canonical` keeps a name it does not know.
        Canon::NotOurs => return E_FAIL,
    };
    let hr = handed(new_url(&text[..end]), out);
    if hr == S_OK {
        put(eaten, len as u32);
    }
    hr
}

extern "system" fn is_system_moniker(_this: *mut Url, out: *mut u32) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, MKSYS_URLMONIKER);
    S_OK
}

static VTABLE: VTable<23> = VTable([
    query as *const (),
    add_ref as *const (),
    release as *const (),
    get_class_id as *const (),
    is_dirty as *const (),
    load as *const (),
    save as *const (),
    get_size_max as *const (),
    bind_to_object as *const (),
    bind_to_storage as *const (),
    reduce as *const (),
    compose_with as *const (),
    enum_parts as *const (),
    is_equal as *const (),
    hash as *const (),
    is_running as *const (),
    time_of_last_change as *const (),
    inverse as *const (),
    common_prefix_with as *const (),
    relative_path_to as *const (),
    get_display_name as *const (),
    parse_display_name as *const (),
    is_system_moniker as *const (),
]);

// ---- the exports ----------------------------------------------------

/// MkParseDisplayNameEx(pbc, szDisplayName, pchEaten, ppmk): ole32's
/// MkParseDisplayName with urlmon's scheme table in front of it (module
/// doc).
#[unsafe(no_mangle)]
pub extern "system" fn MkParseDisplayNameEx(
    bc: *mut c_void,
    name: *const u16,
    eaten: *mut u32,
    out: *mut *mut c_void,
) -> HResult {
    // Each of the four, and the empty name, leaves both out parameters
    // as they were -- not even zeroed (measured).
    if bc.is_null() || name.is_null() || eaten.is_null() || out.is_null() {
        return E_INVALIDARG;
    }
    let mut given = [0u16; MAX_URL];
    let (len, whole) = fill(name, &mut given);
    if len == 0 && whole {
        return E_INVALIDARG;
    }
    if !whole {
        // A name this long is urlmon's only when its scheme is.
        if scheme_of(&given).is_some() {
            put(eaten, 0);
            put(out, core::ptr::null_mut());
            return too_long(b"MkParseDisplayNameEx", len);
        }
        return parse_in_ole32(bc, name, eaten, out);
    }
    let mut text = [0u16; MAX_URL];
    match canonical(&given[..len], &mut text, b"MkParseDisplayNameEx") {
        Canon::NotOurs => parse_in_ole32(bc, name, eaten, out),
        Canon::Failed(e) => {
            put(eaten, 0);
            put(out, core::ptr::null_mut());
            e
        }
        Canon::Ours(end) => {
            let hr = handed(new_url(&text[..end]), out);
            put(eaten, if hr == S_OK { len as u32 } else { 0 });
            hr
        }
    }
}

/// The moniker both CreateURLMoniker and its Ex form make.
fn create(base: *mut c_void, name: *const u16, out: *mut *mut c_void) -> HResult {
    if out.is_null() || name.is_null() {
        return E_INVALIDARG;
    }
    put(out, core::ptr::null_mut());
    let mut given = [0u16; MAX_URL];
    let (len, whole) = fill(name, &mut given);
    if !whole {
        return too_long(b"CreateURLMoniker", len);
    }
    let mut from = [0u16; MAX_URL];
    let at = if base.is_null() {
        0
    } else {
        match other_name(base, &mut from) {
            Some(at) => at,
            None => return E_INVALIDARG,
        }
    };
    let mut text = [0u16; MAX_URL];
    match resolved_canonical(&from[..at], &given[..len], &mut text, b"CreateURLMoniker") {
        Canon::Ours(end) => handed(new_url(&text[..end]), out),
        Canon::Failed(e) => e,
        Canon::NotOurs => E_FAIL,
    }
}

/// CreateURLMoniker(pmkContext, szURL, ppmk): a URL moniker for `szURL`,
/// resolved against the context moniker where there is one. A name
/// whose scheme urlmon does not know is kept as it stands, and is still
/// a URL moniker (measured).
#[unsafe(no_mangle)]
pub extern "system" fn CreateURLMoniker(
    base: *mut c_void,
    name: *const u16,
    out: *mut *mut c_void,
) -> HResult {
    create(base, name, out)
}

/// CreateURLMonikerEx(pmkContext, szURL, ppmk, dwFlags): the same, with
/// the two flags Windows takes (URL_MK_LEGACY and URL_MK_UNIFORM, which
/// name the same canonical form here); any other flag is E_INVALIDARG,
/// recorded with its value.
#[unsafe(no_mangle)]
pub extern "system" fn CreateURLMonikerEx(
    base: *mut c_void,
    name: *const u16,
    out: *mut *mut c_void,
    flags: u32,
) -> HResult {
    if flags > 1 {
        record(&[b"urlmon.dll!CreateURLMonikerEx(flags)"], LEDGER_OTHER, u64::from(flags));
        return E_INVALIDARG;
    }
    create(base, name, out)
}

//! The run-time library's string, SID, ACL, security descriptor and
//! critical section routines (what Cygwin's runtime, msys-2.0.dll, calls
//! directly), as documented for ntdll: counted strings (UNICODE_STRING,
//! ANSI_STRING, lengths in bytes without the NUL), SIDs, ACLs and
//! descriptors in their documented layouts (absolute descriptors hold
//! pointers; self-relative ones offsets). Case folding is ASCII only
//! (TODO: the Unicode upcase table).

use crate::heap::{RtlAllocateHeap, RtlFreeHeap, process_heap};
use crate::{PARAMS_CURRENT_DIRECTORY, PTR};
use core::sync::atomic::{AtomicI32, AtomicUsize, Ordering};

const STATUS_SUCCESS: u32 = 0;
const STATUS_BUFFER_TOO_SMALL: u32 = 0xc000_0023;
const STATUS_INVALID_PARAMETER: u32 = 0xc000_000d;
const STATUS_NO_MEMORY: u32 = 0xc000_0017;
const STATUS_INVALID_SID: u32 = 0xc000_0078;
const STATUS_INVALID_ACL: u32 = 0xc000_0077;
const STATUS_ALLOTTED_SPACE_EXCEEDED: u32 = 0xc000_0099;
const STATUS_UNKNOWN_REVISION: u32 = 0xc000_0058;
const STATUS_BAD_DESCRIPTOR_FORMAT: u32 = 0xc000_00e7;

#[repr(C)]
pub struct UnicodeString {
    length: u16,
    maximum: u16,
    buffer: *mut u16,
}

#[repr(C)]
pub struct AnsiString {
    length: u16,
    maximum: u16,
    buffer: *mut u8,
}

fn alloc(bytes: usize) -> *mut u8 {
    RtlAllocateHeap(process_heap(), 0, bytes)
}

/// The length of a NUL-terminated string of `T`.
///
/// # Safety
/// `p` is null or NUL-terminated.
unsafe fn len_of<T: Copy + PartialEq + Default>(p: *const T) -> usize {
    let mut n = 0;
    if !p.is_null() {
        // SAFETY: up to the terminator.
        while unsafe { *p.add(n) } != T::default() {
            n += 1;
        }
    }
    n
}

/// The characters of a UNICODE_STRING.
///
/// # Safety
/// `s` is a valid string.
unsafe fn chars<'a>(s: *const UnicodeString) -> &'a [u16] {
    // SAFETY: Length bytes at Buffer.
    unsafe {
        let s = &*s;
        if s.buffer.is_null() {
            return &[];
        }
        core::slice::from_raw_parts(s.buffer, usize::from(s.length) / 2)
    }
}

fn upcase(c: u16) -> u16 {
    if (u16::from(b'a')..=u16::from(b'z')).contains(&c) { c - 32 } else { c }
}

fn downcase(c: u16) -> u16 {
    if (u16::from(b'A')..=u16::from(b'Z')).contains(&c) { c + 32 } else { c }
}

#[unsafe(no_mangle)]
pub extern "system" fn RtlInitUnicodeString(dst: *mut UnicodeString, src: *const u16) {
    // SAFETY: the caller's string and NUL-terminated source.
    unsafe {
        let n = len_of(src);
        *dst = UnicodeString {
            length: (2 * n) as u16,
            maximum: if src.is_null() { 0 } else { (2 * n + 2) as u16 },
            buffer: src.cast_mut(),
        };
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn RtlInitAnsiString(dst: *mut AnsiString, src: *const u8) {
    // SAFETY: as RtlInitUnicodeString.
    unsafe {
        let n = len_of(src);
        *dst = AnsiString {
            length: n as u16,
            maximum: if src.is_null() { 0 } else { (n + 1) as u16 },
            buffer: src.cast_mut(),
        };
    }
}

/// RtlCompareUnicodeString(a, b, CaseInsensitive): the first differing
/// character decides, else the length.
#[unsafe(no_mangle)]
pub extern "system" fn RtlCompareUnicodeString(
    a: *const UnicodeString,
    b: *const UnicodeString,
    ci: u8,
) -> i32 {
    // SAFETY: valid strings.
    let (a, b) = unsafe { (chars(a), chars(b)) };
    let fold = |c: u16| if ci != 0 { upcase(c) } else { c };
    for (&x, &y) in a.iter().zip(b) {
        let (x, y) = (fold(x), fold(y));
        if x != y {
            return i32::from(x) - i32::from(y);
        }
    }
    a.len() as i32 - b.len() as i32
}

#[unsafe(no_mangle)]
pub extern "system" fn RtlEqualUnicodeString(
    a: *const UnicodeString,
    b: *const UnicodeString,
    ci: u8,
) -> u8 {
    u8::from(RtlCompareUnicodeString(a, b, ci) == 0)
}

/// RtlPrefixUnicodeString(prefix, s, CaseInsensitive).
#[unsafe(no_mangle)]
pub extern "system" fn RtlPrefixUnicodeString(
    prefix: *const UnicodeString,
    s: *const UnicodeString,
    ci: u8,
) -> u8 {
    // SAFETY: valid strings.
    let (p, s) = unsafe { (chars(prefix), chars(s)) };
    let fold = |c: u16| if ci != 0 { upcase(c) } else { c };
    u8::from(p.len() <= s.len() && p.iter().zip(s).all(|(&x, &y)| fold(x) == fold(y)))
}

#[unsafe(no_mangle)]
pub extern "system" fn RtlUpcaseUnicodeChar(c: u16) -> u16 {
    upcase(c)
}

const STATUS_BUFFER_OVERFLOW: u32 = 0x8000_0005;

/// RtlIntegerToUnicodeString(value, base, string): `value` (unsigned) in
/// base 2, 8, 10 or 16 (0: 10), upper-case digits, into the string's
/// buffer with a NUL if it fits; STATUS_BUFFER_OVERFLOW (the buffer
/// untouched) if the digits do not; STATUS_INVALID_PARAMETER for another
/// base.
#[unsafe(no_mangle)]
pub extern "system" fn RtlIntegerToUnicodeString(
    value: u32,
    base: u32,
    string: *mut UnicodeString,
) -> u32 {
    let base = if base == 0 { 10 } else { base };
    if ![2, 8, 10, 16].contains(&base) {
        return STATUS_INVALID_PARAMETER;
    }
    let mut digits = [0u16; 32];
    let mut at = digits.len();
    let mut v = value;
    loop {
        at -= 1;
        digits[at] = u16::from(b"0123456789ABCDEF"[(v % base) as usize]);
        v /= base;
        if v == 0 {
            break;
        }
    }
    let text = &digits[at..];
    // SAFETY: the caller's string with `maximum` bytes at its buffer.
    unsafe {
        let s = &mut *string;
        let room = usize::from(s.maximum) / 2;
        if text.len() > room {
            return STATUS_BUFFER_OVERFLOW;
        }
        core::ptr::copy_nonoverlapping(text.as_ptr(), s.buffer, text.len());
        if text.len() < room {
            *s.buffer.add(text.len()) = 0;
        }
        s.length = (2 * text.len()) as u16;
    }
    STATUS_SUCCESS
}

const DOS_STAR: u16 = b'<' as u16;
const DOS_QM: u16 = b'>' as u16;
const DOS_DOT: u16 = b'"' as u16;
const STAR: u16 = b'*' as u16;
const QM: u16 = b'?' as u16;
const DOT: u16 = b'.' as u16;

/// RtlIsNameInExpression(expression, name, ignore_case, upcase_table):
/// whether `name` matches `expression` (FsRtlIsNameInExpression's rules):
/// `*` any characters, `?` one, `<` (DOS_STAR) any characters up to the
/// name's last period, `>` (DOS_QM) one character, or none at a period or
/// the end, `"` (DOS_DOT) a period, or nothing at the end; others
/// themselves. With `ignore_case` the name is upper-cased (the expression
/// must already be). A table of all positions of the name that a prefix
/// of the expression can reach, row by row.
#[unsafe(no_mangle)]
pub extern "system" fn RtlIsNameInExpression(
    expression: *const UnicodeString,
    name: *const UnicodeString,
    ignore_case: u8,
    _upcase_table: *const u16,
) -> u8 {
    // SAFETY: the caller's strings.
    let (e, n) = unsafe { (chars(expression), chars(name)) };
    let fold = |c: u16| if ignore_case != 0 { upcase(c) } else { c };
    let last_dot = n.iter().rposition(|&c| c == DOT);
    let width = n.len() + 1;
    // Two rows of `width` flags: the positions reachable before and after
    // each expression character.
    let rows = alloc(2 * width);
    if rows.is_null() {
        return 0;
    }
    // SAFETY: our two rows.
    let (mut cur, mut next) = unsafe {
        core::ptr::write_bytes(rows, 0, 2 * width);
        (
            core::slice::from_raw_parts_mut(rows, width),
            core::slice::from_raw_parts_mut(rows.add(width), width),
        )
    };
    cur[0] = 1;
    for &x in e {
        next.fill(0);
        for i in 0..width {
            if cur[i] == 0 {
                continue;
            }
            match x {
                STAR => next[i..].fill(1),
                DOS_STAR => {
                    // Up to the last period (not past it): any count.
                    let stop = last_dot.map_or(n.len(), |d| if i <= d { d } else { n.len() });
                    next[i..=stop.max(i)].fill(1);
                }
                QM => {
                    if i < n.len() {
                        next[i + 1] = 1;
                    }
                }
                DOS_QM => {
                    if i < n.len() && n[i] != DOT {
                        next[i + 1] = 1;
                    } else {
                        next[i] = 1;
                    }
                }
                DOS_DOT => {
                    if i < n.len() && n[i] == DOT {
                        next[i + 1] = 1;
                    }
                    if i == n.len() {
                        next[i] = 1;
                    }
                }
                c => {
                    if i < n.len() && fold(n[i]) == c {
                        next[i + 1] = 1;
                    }
                }
            }
        }
        core::mem::swap(&mut cur, &mut next);
    }
    let matched = cur[n.len()];
    RtlFreeHeap(process_heap(), 0, rows);
    matched
}

/// RtlCopyUnicodeString(dst, src): as much as fits, NUL-terminated if
/// there is room; a null source empties `dst`.
#[unsafe(no_mangle)]
pub extern "system" fn RtlCopyUnicodeString(dst: *mut UnicodeString, src: *const UnicodeString) {
    // SAFETY: the caller's strings; `dst` has `maximum` bytes.
    unsafe {
        let d = &mut *dst;
        if src.is_null() {
            d.length = 0;
            return;
        }
        let from = chars(src);
        let n = from.len().min(usize::from(d.maximum) / 2);
        core::ptr::copy(from.as_ptr(), d.buffer, n);
        d.length = (2 * n) as u16;
        if 2 * n + 2 <= usize::from(d.maximum) {
            *d.buffer.add(n) = 0;
        }
    }
}

/// Appends `n` characters at `src` to `dst` (STATUS_BUFFER_TOO_SMALL if
/// they do not fit).
///
/// # Safety
/// Valid string and source.
unsafe fn append(dst: *mut UnicodeString, src: *const u16, n: usize) -> u32 {
    // SAFETY: the caller's string with `maximum` bytes.
    unsafe {
        let d = &mut *dst;
        let have = usize::from(d.length) / 2;
        if 2 * (have + n) > usize::from(d.maximum) {
            return STATUS_BUFFER_TOO_SMALL;
        }
        core::ptr::copy(src, d.buffer.add(have), n);
        d.length = (2 * (have + n)) as u16;
        if 2 * (have + n) + 2 <= usize::from(d.maximum) {
            *d.buffer.add(have + n) = 0;
        }
    }
    STATUS_SUCCESS
}

#[unsafe(no_mangle)]
pub extern "system" fn RtlAppendUnicodeToString(dst: *mut UnicodeString, src: *const u16) -> u32 {
    // SAFETY: a NUL-terminated source.
    unsafe { append(dst, src, len_of(src)) }
}

#[unsafe(no_mangle)]
pub extern "system" fn RtlAppendUnicodeStringToString(
    dst: *mut UnicodeString,
    src: *const UnicodeString,
) -> u32 {
    // SAFETY: a valid source string.
    unsafe {
        let s = chars(src);
        append(dst, s.as_ptr(), s.len())
    }
}

/// Maps `src` into `dst` with `f`, allocating (with a NUL) if `alloc`.
fn map_case(
    dst: *mut UnicodeString,
    src: *const UnicodeString,
    alloc_new: u8,
    f: fn(u16) -> u16,
) -> u32 {
    // SAFETY: the caller's strings.
    unsafe {
        let s = chars(src);
        let d = &mut *dst;
        if alloc_new != 0 {
            let p = alloc(2 * s.len() + 2).cast::<u16>();
            if p.is_null() {
                return STATUS_NO_MEMORY;
            }
            d.buffer = p;
            d.maximum = (2 * s.len() + 2) as u16;
            *p.add(s.len()) = 0;
        } else if usize::from(d.maximum) < 2 * s.len() {
            return STATUS_BUFFER_TOO_SMALL;
        }
        for (i, &c) in s.iter().enumerate() {
            *d.buffer.add(i) = f(c);
        }
        d.length = (2 * s.len()) as u16;
    }
    STATUS_SUCCESS
}

#[unsafe(no_mangle)]
pub extern "system" fn RtlUpcaseUnicodeString(
    dst: *mut UnicodeString,
    src: *const UnicodeString,
    alloc_new: u8,
) -> u32 {
    map_case(dst, src, alloc_new, upcase)
}

#[unsafe(no_mangle)]
pub extern "system" fn RtlDowncaseUnicodeString(
    dst: *mut UnicodeString,
    src: *const UnicodeString,
    alloc_new: u8,
) -> u32 {
    map_case(dst, src, alloc_new, downcase)
}

/// RtlUnicodeStringToAnsiString(dst, src, Allocate): UTF-8 (gala's ANSI
/// code page), NUL-terminated.
#[unsafe(no_mangle)]
pub extern "system" fn RtlUnicodeStringToAnsiString(
    dst: *mut AnsiString,
    src: *const UnicodeString,
    alloc_new: u8,
) -> u32 {
    // SAFETY: the caller's strings.
    unsafe {
        let s = chars(src);
        let need: usize =
            char::decode_utf16(s.iter().copied()).map(|c| c.map_or(3, char::len_utf8)).sum();
        let d = &mut *dst;
        if alloc_new != 0 {
            let p = alloc(need + 1);
            if p.is_null() {
                return STATUS_NO_MEMORY;
            }
            d.buffer = p;
            d.maximum = (need + 1) as u16;
        } else if usize::from(d.maximum) < need + 1 {
            return STATUS_BUFFER_TOO_SMALL;
        }
        let mut at = 0;
        for c in char::decode_utf16(s.iter().copied()) {
            let mut b = [0u8; 4];
            let e = c.unwrap_or(char::REPLACEMENT_CHARACTER).encode_utf8(&mut b);
            core::ptr::copy_nonoverlapping(e.as_ptr(), d.buffer.add(at), e.len());
            at += e.len();
        }
        *d.buffer.add(at) = 0;
        d.length = at as u16;
    }
    STATUS_SUCCESS
}

/// RtlFormatCurrentUserKeyPath(*CurrentUserKeyPath): `\REGISTRY\USER\<SID>`
/// for the user of the thread's token (or the process's), allocated from
/// the process heap (freed with RtlFreeUnicodeString).
#[unsafe(no_mangle)]
pub extern "system" fn RtlFormatCurrentUserKeyPath(out: *mut UnicodeString) -> u32 {
    const TOKEN_QUERY: u32 = 8;
    const TOKEN_USER: u32 = 1;
    let mut token = 0usize;
    if crate::NtOpenThreadToken(-2, TOKEN_QUERY, 1, &mut token) != STATUS_SUCCESS {
        let status = crate::NtOpenProcessToken(-1, TOKEN_QUERY, &mut token);
        if status != STATUS_SUCCESS {
            return status;
        }
    }
    // TOKEN_USER { SID_AND_ATTRIBUTES { PSID Sid; DWORD Attributes } } and
    // the SID after it.
    let mut info = [0u64; 16];
    let mut ret = 0u32;
    let status =
        crate::NtQueryInformationToken(token, TOKEN_USER, info.as_mut_ptr().cast(), 128, &mut ret);
    crate::NtClose(token);
    if status != STATUS_SUCCESS {
        return status;
    }
    let mut sid = UnicodeString { length: 0, maximum: 0, buffer: core::ptr::null_mut() };
    let status = RtlConvertSidToUnicodeString(&mut sid, info[0] as *const u8, 1);
    if status != STATUS_SUCCESS {
        return status;
    }
    let prefix = b"\\REGISTRY\\USER\\";
    // SAFETY: the converted SID string; the caller's output.
    unsafe {
        let n = usize::from(sid.length) / 2;
        let total = prefix.len() + n;
        let p = alloc(2 * total + 2).cast::<u16>();
        if p.is_null() {
            RtlFreeUnicodeString(&mut sid);
            return STATUS_NO_MEMORY;
        }
        for (i, &c) in prefix.iter().enumerate() {
            *p.add(i) = u16::from(c);
        }
        core::ptr::copy_nonoverlapping(sid.buffer, p.add(prefix.len()), n);
        *p.add(total) = 0;
        RtlFreeUnicodeString(&mut sid);
        *out = UnicodeString {
            length: (2 * total) as u16,
            maximum: (2 * total + 2) as u16,
            buffer: p,
        };
    }
    STATUS_SUCCESS
}

#[unsafe(no_mangle)]
pub extern "system" fn RtlFreeUnicodeString(s: *mut UnicodeString) {
    // SAFETY: a string from an allocating Rtl call.
    unsafe {
        if !(*s).buffer.is_null() {
            RtlFreeHeap(process_heap(), 0, (*s).buffer.cast());
        }
        *s = UnicodeString { length: 0, maximum: 0, buffer: core::ptr::null_mut() };
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn RtlFreeAnsiString(s: *mut AnsiString) {
    // SAFETY: as RtlFreeUnicodeString.
    unsafe {
        if !(*s).buffer.is_null() {
            RtlFreeHeap(process_heap(), 0, (*s).buffer);
        }
        *s = AnsiString { length: 0, maximum: 0, buffer: core::ptr::null_mut() };
    }
}

// ---- SIDs: Revision, SubAuthorityCount, IdentifierAuthority[6],
// SubAuthority[count] ----

/// # Safety
/// `sid` points at a SID header.
unsafe fn sid_len(sid: *const u8) -> usize {
    // SAFETY: the count byte.
    8 + 4 * usize::from(unsafe { *sid.add(1) })
}

#[unsafe(no_mangle)]
pub extern "system" fn RtlLengthSid(sid: *const u8) -> u32 {
    // SAFETY: a valid SID.
    unsafe { sid_len(sid) as u32 }
}

#[unsafe(no_mangle)]
pub extern "system" fn RtlInitializeSid(sid: *mut u8, authority: *const [u8; 6], count: u8) -> u32 {
    if count > 15 {
        return STATUS_INVALID_PARAMETER;
    }
    // SAFETY: the caller's buffer of at least 8 + 4 * count bytes.
    unsafe {
        *sid = 1;
        *sid.add(1) = count;
        core::ptr::copy_nonoverlapping((*authority).as_ptr(), sid.add(2), 6);
    }
    STATUS_SUCCESS
}

#[unsafe(no_mangle)]
pub extern "system" fn RtlSubAuthoritySid(sid: *mut u8, index: u32) -> *mut u32 {
    // SAFETY: within the caller's SID.
    unsafe { sid.add(8 + 4 * index as usize).cast() }
}

#[unsafe(no_mangle)]
pub extern "system" fn RtlSubAuthorityCountSid(sid: *mut u8) -> *mut u8 {
    // SAFETY: within the caller's SID.
    unsafe { sid.add(1) }
}

#[unsafe(no_mangle)]
pub extern "system" fn RtlIdentifierAuthoritySid(sid: *mut u8) -> *mut [u8; 6] {
    // SAFETY: within the caller's SID.
    unsafe { sid.add(2).cast() }
}

#[unsafe(no_mangle)]
pub extern "system" fn RtlCopySid(len: u32, dst: *mut u8, src: *const u8) -> u32 {
    // SAFETY: a valid source SID; `dst` holds `len` bytes.
    unsafe {
        let n = sid_len(src);
        if (len as usize) < n {
            return STATUS_BUFFER_TOO_SMALL;
        }
        core::ptr::copy(src, dst, n);
    }
    STATUS_SUCCESS
}

#[unsafe(no_mangle)]
pub extern "system" fn RtlEqualSid(a: *const u8, b: *const u8) -> u8 {
    // SAFETY: valid SIDs.
    unsafe {
        let (n, m) = (sid_len(a), sid_len(b));
        u8::from(n == m && core::slice::from_raw_parts(a, n) == core::slice::from_raw_parts(b, m))
    }
}

/// RtlEqualPrefixSid: the same but for the last sub-authority.
#[unsafe(no_mangle)]
pub extern "system" fn RtlEqualPrefixSid(a: *const u8, b: *const u8) -> u8 {
    // SAFETY: valid SIDs.
    unsafe {
        let (n, m) = (sid_len(a), sid_len(b));
        if n != m || *a.add(1) == 0 {
            return u8::from(n == m && *a.add(1) == 0 && *a == *b);
        }
        let prefix = n - 4;
        u8::from(core::slice::from_raw_parts(a, prefix) == core::slice::from_raw_parts(b, prefix))
    }
}

/// RtlConvertSidToUnicodeString(dst, sid, Allocate): "S-1-5-32-544" (the
/// authority in decimal, or hex beyond 32 bits).
#[unsafe(no_mangle)]
pub extern "system" fn RtlConvertSidToUnicodeString(
    dst: *mut UnicodeString,
    sid: *const u8,
    alloc_new: u8,
) -> u32 {
    let mut text = [0u16; 192];
    let mut n = 0;
    let mut push = |s: &[u8]| {
        for &b in s {
            if n < text.len() {
                text[n] = u16::from(b);
                n += 1;
            }
        }
    };
    let decimal = |mut v: u64, out: &mut [u8; 20]| -> usize {
        let mut tmp = [0u8; 20];
        let mut k = 0;
        loop {
            tmp[k] = b'0' + (v % 10) as u8;
            k += 1;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        for i in 0..k {
            out[i] = tmp[k - 1 - i];
        }
        k
    };
    // SAFETY: a valid SID.
    unsafe {
        if *sid != 1 {
            return STATUS_INVALID_SID;
        }
        let mut buf = [0u8; 20];
        push(b"S-");
        let k = decimal(u64::from(*sid), &mut buf);
        push(&buf[..k]);
        push(b"-");
        let mut authority = 0u64;
        for i in 0..6 {
            authority = authority << 8 | u64::from(*sid.add(2 + i));
        }
        let k = decimal(authority, &mut buf);
        push(&buf[..k]);
        for i in 0..usize::from(*sid.add(1)) {
            push(b"-");
            let v = sid.add(8 + 4 * i).cast::<u32>().read_unaligned();
            let k = decimal(u64::from(v), &mut buf);
            push(&buf[..k]);
        }
    }
    let src = UnicodeString {
        length: (2 * n) as u16,
        maximum: (2 * n) as u16,
        buffer: text.as_mut_ptr(),
    };
    // SAFETY: the caller's string.
    unsafe {
        let d = &mut *dst;
        if alloc_new != 0 {
            let p = alloc(2 * n + 2).cast::<u16>();
            if p.is_null() {
                return STATUS_NO_MEMORY;
            }
            *d = UnicodeString { length: 0, maximum: (2 * n + 2) as u16, buffer: p };
        } else if usize::from(d.maximum) < 2 * n {
            return STATUS_BUFFER_TOO_SMALL;
        }
    }
    RtlCopyUnicodeString(dst, &src);
    STATUS_SUCCESS
}

// ---- ACLs: AclRevision, Sbz1, AclSize, AceCount, Sbz2; ACEs: Type,
// Flags, Size, Mask, SID ----

/// # Safety
/// `acl` is a valid ACL.
unsafe fn word(acl: *const u8, at: usize) -> u16 {
    // SAFETY: inside the header.
    unsafe { acl.add(at).cast::<u16>().read_unaligned() }
}

/// The ACL's ACEs end here (bytes from the ACL's start).
///
/// # Safety
/// `acl` is a valid ACL.
unsafe fn used(acl: *const u8) -> Option<usize> {
    // SAFETY: walks the ACEs inside AclSize.
    unsafe {
        let size = usize::from(word(acl, 2));
        let mut at = 8;
        for _ in 0..word(acl, 4) {
            let n = usize::from(word(acl, at + 2));
            if n < 4 || at + n > size {
                return None;
            }
            at += n;
        }
        Some(at)
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn RtlCreateAcl(acl: *mut u8, len: u32, revision: u32) -> u32 {
    if !(2..=4).contains(&revision) || !(8..=0xffff).contains(&len) {
        return STATUS_INVALID_PARAMETER;
    }
    // SAFETY: the caller's buffer of `len` bytes.
    unsafe {
        core::ptr::write_bytes(acl, 0, 8);
        *acl = revision as u8;
        acl.add(2).cast::<u16>().write_unaligned((len & !3) as u16);
    }
    STATUS_SUCCESS
}

/// Appends an ACE of `kind` to the ACL.
fn add_ace(acl: *mut u8, revision: u32, kind: u8, flags: u32, mask: u32, sid: *const u8) -> u32 {
    // SAFETY: a valid ACL and SID.
    unsafe {
        let Some(at) = used(acl) else { return STATUS_INVALID_ACL };
        let n = sid_len(sid);
        let size = 8 + n;
        if at + size > usize::from(word(acl, 2)) {
            return STATUS_ALLOTTED_SPACE_EXCEEDED;
        }
        let ace = acl.add(at);
        *ace = kind;
        *ace.add(1) = flags as u8;
        ace.add(2).cast::<u16>().write_unaligned(size as u16);
        ace.add(4).cast::<u32>().write_unaligned(mask);
        core::ptr::copy_nonoverlapping(sid, ace.add(8), n);
        acl.add(4).cast::<u16>().write_unaligned(word(acl, 4) + 1);
        if (revision as u8) > *acl {
            *acl = revision as u8;
        }
    }
    STATUS_SUCCESS
}

#[unsafe(no_mangle)]
pub extern "system" fn RtlAddAccessAllowedAce(
    acl: *mut u8,
    revision: u32,
    mask: u32,
    sid: *const u8,
) -> u32 {
    add_ace(acl, revision, 0, 0, mask, sid)
}

#[unsafe(no_mangle)]
pub extern "system" fn RtlAddAccessAllowedAceEx(
    acl: *mut u8,
    revision: u32,
    flags: u32,
    mask: u32,
    sid: *const u8,
) -> u32 {
    add_ace(acl, revision, 0, flags, mask, sid)
}

#[unsafe(no_mangle)]
pub extern "system" fn RtlAddAccessDeniedAceEx(
    acl: *mut u8,
    revision: u32,
    flags: u32,
    mask: u32,
    sid: *const u8,
) -> u32 {
    add_ace(acl, revision, 1, flags, mask, sid)
}

#[unsafe(no_mangle)]
pub extern "system" fn RtlGetAce(acl: *const u8, index: u32, ace: *mut *mut u8) -> u32 {
    // SAFETY: a valid ACL; the output pointer.
    unsafe {
        if index >= u32::from(word(acl, 4)) {
            return STATUS_INVALID_PARAMETER;
        }
        let mut at = 8;
        for _ in 0..index {
            at += usize::from(word(acl, at + 2));
        }
        *ace = acl.add(at).cast_mut();
    }
    STATUS_SUCCESS
}

/// RtlFirstFreeAce(acl, &ace): where the next ACE goes (null if the ACL
/// is full); FALSE for a damaged ACL.
#[unsafe(no_mangle)]
pub extern "system" fn RtlFirstFreeAce(acl: *const u8, ace: *mut *mut u8) -> u8 {
    // SAFETY: a valid ACL; the output pointer.
    unsafe {
        let Some(at) = used(acl) else { return 0 };
        *ace = if at < usize::from(word(acl, 2)) {
            acl.add(at).cast_mut()
        } else {
            core::ptr::null_mut()
        };
    }
    1
}

// ---- Security descriptors: Revision, Sbz1, Control, then Owner, Group,
// Sacl, Dacl: pointers (absolute: 40 bytes on x64, 20 on x86) or offsets
// (self-relative, 20 bytes) ----

/// The absolute form's size: the header, padded to a pointer, and four
/// pointers.
const ABSOLUTE_SIZE: usize = 5 * size_of::<usize>();

const SE_OWNER_DEFAULTED: u16 = 0x1;
const SE_GROUP_DEFAULTED: u16 = 0x2;
const SE_DACL_PRESENT: u16 = 0x4;
const SE_DACL_DEFAULTED: u16 = 0x8;
const SE_SACL_PRESENT: u16 = 0x10;
const SE_SELF_RELATIVE: u16 = 0x8000;
/// The control bits RtlSetControlSecurityDescriptor may change.
const SETTABLE: u16 = 0x0100 | 0x0200 | 0x0400 | 0x0800 | 0x1000 | 0x2000;

/// # Safety
/// `sd` is a valid descriptor.
unsafe fn control(sd: *const u8) -> u16 {
    // SAFETY: the header.
    unsafe { word(sd, 2) }
}

/// # Safety
/// `sd` is a valid descriptor.
unsafe fn set_control(sd: *mut u8, c: u16) {
    // SAFETY: the header.
    unsafe { sd.add(2).cast::<u16>().write_unaligned(c) }
}

/// Field `index` (0 owner, 1 group, 2 SACL, 3 DACL) of either form.
///
/// # Safety
/// `sd` is a valid descriptor.
unsafe fn field(sd: *const u8, index: usize) -> *mut u8 {
    // SAFETY: inside the header.
    unsafe {
        if control(sd) & SE_SELF_RELATIVE != 0 {
            let off = sd.add(4 + 4 * index).cast::<u32>().read_unaligned() as usize;
            if off == 0 { core::ptr::null_mut() } else { sd.add(off).cast_mut() }
        } else {
            sd.add(size_of::<usize>() * (1 + index)).cast::<*mut u8>().read_unaligned()
        }
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn RtlCreateSecurityDescriptor(sd: *mut u8, revision: u32) -> u32 {
    if revision != 1 {
        return STATUS_UNKNOWN_REVISION;
    }
    // SAFETY: the caller's SECURITY_DESCRIPTOR.
    unsafe {
        core::ptr::write_bytes(sd, 0, ABSOLUTE_SIZE);
        *sd = 1;
    }
    STATUS_SUCCESS
}

/// Sets field `index` of an absolute descriptor and its control bits.
fn set_field(sd: *mut u8, index: usize, value: *mut u8, bits: u16, on: u16) -> u32 {
    // SAFETY: the caller's descriptor.
    unsafe {
        if control(sd) & SE_SELF_RELATIVE != 0 {
            return STATUS_BAD_DESCRIPTOR_FORMAT;
        }
        sd.add(size_of::<usize>() * (1 + index)).cast::<*mut u8>().write_unaligned(value);
        set_control(sd, control(sd) & !bits | on);
    }
    STATUS_SUCCESS
}

#[unsafe(no_mangle)]
pub extern "system" fn RtlSetDaclSecurityDescriptor(
    sd: *mut u8,
    present: u8,
    acl: *mut u8,
    defaulted: u8,
) -> u32 {
    let mut on = 0;
    if present != 0 {
        on |= SE_DACL_PRESENT;
        if defaulted != 0 {
            on |= SE_DACL_DEFAULTED;
        }
    }
    let acl = if present != 0 { acl } else { core::ptr::null_mut() };
    set_field(sd, 3, acl, SE_DACL_PRESENT | SE_DACL_DEFAULTED, on)
}

#[unsafe(no_mangle)]
pub extern "system" fn RtlGetDaclSecurityDescriptor(
    sd: *const u8,
    present: *mut u8,
    acl: *mut *mut u8,
    defaulted: *mut u8,
) -> u32 {
    // SAFETY: the caller's descriptor and outputs.
    unsafe {
        let c = control(sd);
        *present = u8::from(c & SE_DACL_PRESENT != 0);
        if c & SE_DACL_PRESENT != 0 {
            *acl = field(sd, 3);
            *defaulted = u8::from(c & SE_DACL_DEFAULTED != 0);
        }
    }
    STATUS_SUCCESS
}

#[unsafe(no_mangle)]
pub extern "system" fn RtlSetOwnerSecurityDescriptor(
    sd: *mut u8,
    owner: *mut u8,
    defaulted: u8,
) -> u32 {
    let on = if defaulted != 0 { SE_OWNER_DEFAULTED } else { 0 };
    set_field(sd, 0, owner, SE_OWNER_DEFAULTED, on)
}

#[unsafe(no_mangle)]
pub extern "system" fn RtlGetOwnerSecurityDescriptor(
    sd: *const u8,
    owner: *mut *mut u8,
    defaulted: *mut u8,
) -> u32 {
    // SAFETY: the caller's descriptor and outputs.
    unsafe {
        *owner = field(sd, 0);
        *defaulted = u8::from(control(sd) & SE_OWNER_DEFAULTED != 0);
    }
    STATUS_SUCCESS
}

#[unsafe(no_mangle)]
pub extern "system" fn RtlSetGroupSecurityDescriptor(
    sd: *mut u8,
    group: *mut u8,
    defaulted: u8,
) -> u32 {
    let on = if defaulted != 0 { SE_GROUP_DEFAULTED } else { 0 };
    set_field(sd, 1, group, SE_GROUP_DEFAULTED, on)
}

#[unsafe(no_mangle)]
pub extern "system" fn RtlGetGroupSecurityDescriptor(
    sd: *const u8,
    group: *mut *mut u8,
    defaulted: *mut u8,
) -> u32 {
    // SAFETY: the caller's descriptor and outputs.
    unsafe {
        *group = field(sd, 1);
        *defaulted = u8::from(control(sd) & SE_GROUP_DEFAULTED != 0);
    }
    STATUS_SUCCESS
}

#[unsafe(no_mangle)]
pub extern "system" fn RtlGetControlSecurityDescriptor(
    sd: *const u8,
    c: *mut u16,
    revision: *mut u32,
) -> u32 {
    // SAFETY: the caller's descriptor and outputs.
    unsafe {
        *c = control(sd);
        *revision = u32::from(*sd);
    }
    STATUS_SUCCESS
}

/// RtlSetControlSecurityDescriptor(sd, mask, bits): the masked bits set as
/// in `bits` (only the inheritance and protection bits may change).
#[unsafe(no_mangle)]
pub extern "system" fn RtlSetControlSecurityDescriptor(sd: *mut u8, mask: u16, bits: u16) -> u32 {
    if mask & !SETTABLE != 0 || bits & !SETTABLE != 0 {
        return STATUS_INVALID_PARAMETER;
    }
    // SAFETY: the caller's descriptor.
    unsafe { set_control(sd, control(sd) & !mask | bits & mask) };
    STATUS_SUCCESS
}

/// RtlAbsoluteToSelfRelativeSD(abs, rel, &len): header, owner, group,
/// SACL, DACL in that order (STATUS_BUFFER_TOO_SMALL and the size needed if
/// `len` is too small).
#[unsafe(no_mangle)]
pub extern "system" fn RtlAbsoluteToSelfRelativeSD(
    abs: *const u8,
    rel: *mut u8,
    len: *mut u32,
) -> u32 {
    // SAFETY: the caller's descriptor, buffer of `*len` bytes and length.
    unsafe {
        let c = control(abs);
        if c & SE_SELF_RELATIVE != 0 {
            return STATUS_BAD_DESCRIPTOR_FORMAT;
        }
        let parts = [field(abs, 0), field(abs, 1), field(abs, 2), field(abs, 3)];
        let sizes = [
            if parts[0].is_null() { 0 } else { sid_len(parts[0]) },
            if parts[1].is_null() { 0 } else { sid_len(parts[1]) },
            if parts[2].is_null() || c & SE_SACL_PRESENT == 0 {
                0
            } else {
                usize::from(word(parts[2], 2))
            },
            if parts[3].is_null() || c & SE_DACL_PRESENT == 0 {
                0
            } else {
                usize::from(word(parts[3], 2))
            },
        ];
        let total = 20 + sizes.iter().sum::<usize>();
        if (*len as usize) < total {
            *len = total as u32;
            return STATUS_BUFFER_TOO_SMALL;
        }
        core::ptr::write_bytes(rel, 0, 20);
        *rel = *abs;
        set_control(rel, c | SE_SELF_RELATIVE);
        let mut at = 20;
        for i in 0..4 {
            if sizes[i] == 0 {
                continue;
            }
            core::ptr::copy_nonoverlapping(parts[i], rel.add(at), sizes[i]);
            rel.add(4 + 4 * i).cast::<u32>().write_unaligned(at as u32);
            at += sizes[i];
        }
        *len = total as u32;
    }
    STATUS_SUCCESS
}

// ---- Critical sections (RTL_CRITICAL_SECTION, x64): DebugInfo,
// LockCount, RecursionCount, OwningThread, LockSemaphore, SpinCount.
//
// LockCount is -1 when free and, while taken, the number of threads
// waiting for it (0: none). An entering thread spins a little for it to
// come free, then counts itself in and sleeps on a process-wide keyed
// event keyed by the section's address. A leaving owner with waiters
// does not free the section: it hands it to one of them (releasing the
// key, which waits until that waiter is there), so no wake is lost and
// no waiter polls. ----

#[repr(C)]
pub struct CriticalSection {
    debug_info: usize,
    lock_count: AtomicI32,
    recursion: i32,
    owner: AtomicUsize,
    semaphore: usize,
    spin_count: usize,
}

fn thread_id() -> usize {
    crate::thread_id()
}

#[unsafe(no_mangle)]
pub extern "system" fn RtlInitializeCriticalSection(cs: *mut CriticalSection) -> u32 {
    // SAFETY: the caller's section.
    unsafe {
        cs.write(CriticalSection {
            debug_info: 0,
            lock_count: AtomicI32::new(-1),
            recursion: 0,
            owner: AtomicUsize::new(0),
            semaphore: 0,
            spin_count: 0,
        })
    };
    STATUS_SUCCESS
}

#[unsafe(no_mangle)]
pub extern "system" fn RtlInitializeCriticalSectionAndSpinCount(
    cs: *mut CriticalSection,
    _spin: u32,
) -> u32 {
    RtlInitializeCriticalSection(cs)
}

#[unsafe(no_mangle)]
pub extern "system" fn RtlDeleteCriticalSection(_cs: *mut CriticalSection) -> u32 {
    STATUS_SUCCESS
}

#[unsafe(no_mangle)]
pub extern "system" fn RtlEnterCriticalSection(cs: *mut CriticalSection) -> u32 {
    let me = thread_id();
    // SAFETY: an initialized section; recursion is the owner's.
    let cs = unsafe { &mut *cs };
    if cs.owner.load(Ordering::Acquire) == me {
        cs.recursion += 1;
        return STATUS_SUCCESS;
    }
    let mut spins = 0u32;
    loop {
        if cs.lock_count.compare_exchange(-1, 0, Ordering::Acquire, Ordering::Relaxed).is_ok() {
            break;
        }
        spins += 1;
        if spins < CS_SPINS {
            core::hint::spin_loop();
            continue;
        }
        // Count in as a waiter; -1 before means it came free meanwhile.
        if cs.lock_count.fetch_add(1, Ordering::Acquire) == -1 {
            break;
        }
        let e = cs_event();
        if e == NO_EVENT {
            // No keyed event (no memory): the leaving owner hands the
            // section over through the owner field instead.
            while cs
                .owner
                .compare_exchange(HANDED, me, Ordering::Acquire, Ordering::Relaxed)
                .is_err()
            {
                crate::NtYieldExecution();
            }
        } else {
            crate::NtWaitForKeyedEvent(e, key_of(cs), 0, core::ptr::null());
        }
        // Handed over by the leaving owner.
        break;
    }
    cs.owner.store(me, Ordering::Release);
    cs.recursion = 1;
    STATUS_SUCCESS
}

#[unsafe(no_mangle)]
pub extern "system" fn RtlTryEnterCriticalSection(cs: *mut CriticalSection) -> u8 {
    let me = thread_id();
    // SAFETY: as RtlEnterCriticalSection.
    let cs = unsafe { &mut *cs };
    if cs.owner.load(Ordering::Acquire) == me {
        cs.recursion += 1;
        return 1;
    }
    if cs.lock_count.compare_exchange(-1, 0, Ordering::Acquire, Ordering::Relaxed).is_err() {
        return 0;
    }
    cs.owner.store(me, Ordering::Release);
    cs.recursion = 1;
    1
}

#[unsafe(no_mangle)]
pub extern "system" fn RtlLeaveCriticalSection(cs: *mut CriticalSection) -> u32 {
    // SAFETY: called by the owner.
    let cs = unsafe { &mut *cs };
    cs.recursion -= 1;
    if cs.recursion == 0 {
        cs.owner.store(0, Ordering::Release);
        // 0 → -1: nobody waits, free. n > 0 → n - 1: hand it to a waiter.
        if cs.lock_count.fetch_sub(1, Ordering::Release) > 0 {
            let e = cs_event();
            if e == 0
                || crate::NtReleaseKeyedEvent(e, key_of(cs), 0, core::ptr::null()) != STATUS_SUCCESS
            {
                cs.owner.store(HANDED, Ordering::Release);
            }
        }
    }
    STATUS_SUCCESS
}

/// How many rounds an entering thread spins before it sleeps.
const CS_SPINS: u32 = 100;
/// OwningThread while a section is being handed over without the keyed
/// event (an impossible thread id).
const HANDED: usize = usize::MAX;
/// The keyed event critical sections sleep on (created on first use; or
/// NO_EVENT for good once creating it failed, so that waiters and owners
/// always agree on how a handover goes).
static CS_EVENT: AtomicUsize = AtomicUsize::new(0);
const NO_EVENT: usize = usize::MAX;

fn key_of(cs: &CriticalSection) -> usize {
    core::ptr::from_ref(cs) as usize
}

/// The process's critical-section keyed event, or NO_EVENT.
fn cs_event() -> usize {
    let e = CS_EVENT.load(Ordering::Acquire);
    if e != 0 {
        return e;
    }
    let mut h = 0usize;
    if crate::NtCreateKeyedEvent(&mut h, 0xf_0003, core::ptr::null(), 0) != STATUS_SUCCESS {
        h = NO_EVENT;
    }
    match CS_EVENT.compare_exchange(0, h, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => h,
        Err(other) => {
            if h != NO_EVENT {
                crate::NtClose(h);
            }
            other
        }
    }
}

// ---- Misc ----

/// NtQueryTimerResolution(&Maximum, &Minimum, &Current), in 100 ns: the
/// coarsest and finest periods and the current one (gala's tick).
#[unsafe(no_mangle)]
pub extern "system" fn NtQueryTimerResolution(
    max: *mut u32,
    min: *mut u32,
    current: *mut u32,
) -> u32 {
    // SAFETY: the caller's outputs.
    unsafe {
        *max = 156_250;
        *min = 5_000;
        *current = 10_000; // gala ticks every millisecond
    }
    STATUS_SUCCESS
}

/// The process parameters (PEB.ProcessParameters).
fn process_parameters() -> usize {
    crate::rdp(crate::peb() + crate::PEB_PROCESS_PARAMETERS)
}

/// RtlSetCurrentDirectory_U(path): a full DOS path (`C:\dir`, also as
/// `\\?\C:\dir`) or one relative to the current directory (no `..`
/// processing); the directory is opened (FILE_TRAVERSE) and becomes
/// ProcessParameters.CurrentDirectory (DosPath with a trailing backslash,
/// and the handle).
#[unsafe(no_mangle)]
pub extern "system" fn RtlSetCurrentDirectory_U(path: *const UnicodeString) -> u32 {
    const BACKSLASH: u16 = b'\\' as u16;
    let params = process_parameters();
    let mut dos = [0u16; 520];
    let mut n = 0;
    // CurrentDirectory: DosPath (Length, MaximumLength, Buffer), then
    // Handle.
    let (length, maximum) =
        (params + PARAMS_CURRENT_DIRECTORY, params + PARAMS_CURRENT_DIRECTORY + 2);
    let (buffer, handle) =
        (params + PARAMS_CURRENT_DIRECTORY + PTR, params + PARAMS_CURRENT_DIRECTORY + 2 * PTR);
    // SAFETY: the caller's string; the parameters' CurrentDirectory.
    unsafe {
        let mut p = chars(path);
        let verbatim = [BACKSLASH, BACKSLASH, u16::from(b'?'), BACKSLASH];
        if p.starts_with(&verbatim) {
            p = &p[4..];
        }
        let absolute = p.get(1) == Some(&u16::from(b':')) || p.first() == Some(&BACKSLASH);
        if !absolute {
            let len = usize::from(*(length as *const u16)) / 2;
            let cur = core::slice::from_raw_parts(*(buffer as *const *const u16), len);
            if cur.len() >= dos.len() {
                return STATUS_INVALID_PARAMETER;
            }
            dos[..cur.len()].copy_from_slice(cur);
            n = cur.len();
        }
        if n + p.len() + 2 > dos.len() {
            return STATUS_INVALID_PARAMETER;
        }
        dos[n..n + p.len()].copy_from_slice(p);
        n += p.len();
        if n == 0 || dos[n - 1] != BACKSLASH {
            dos[n] = BACKSLASH;
            n += 1;
        }
        // The NT name: \??\ + the DOS path.
        let mut nt = [0u16; 524];
        for (d, &b) in nt.iter_mut().zip(b"\\??\\") {
            *d = u16::from(b);
        }
        nt[4..4 + n].copy_from_slice(&dos[..n]);
        let name = crate::UnicodeString {
            length: (2 * (n + 4)) as u16,
            maximum_length: (2 * (n + 4)) as u16,
            buffer: nt.as_ptr(),
        };
        let attributes = crate::ObjectAttributes {
            length: core::mem::size_of::<crate::ObjectAttributes>() as u32,
            root_directory: 0,
            object_name: &name,
            attributes: 0x40, // OBJ_CASE_INSENSITIVE
            security_descriptor: 0,
            security_quality_of_service: 0,
        };
        let (mut h, mut iosb) = (0usize, [0usize; 2]);
        // FILE_TRAVERSE | SYNCHRONIZE; FILE_DIRECTORY_FILE |
        // FILE_SYNCHRONOUS_IO_NONALERT.
        let status = crate::NtOpenFile(&mut h, 0x10_0020, &attributes, &mut iosb, 3, 0x21);
        if status != STATUS_SUCCESS {
            return status;
        }
        let max = usize::from(*(maximum as *const u16)) / 2;
        if n + 1 > max {
            crate::NtClose(h);
            return STATUS_INVALID_PARAMETER;
        }
        let buf = *(buffer as *const *mut u16);
        core::ptr::copy_nonoverlapping(dos.as_ptr(), buf, n);
        *buf.add(n) = 0;
        *(length as *mut u16) = (2 * n) as u16;
        let old = core::mem::replace(&mut *(handle as *mut usize), h);
        if old != 0 {
            crate::NtClose(old);
        }
    }
    STATUS_SUCCESS
}

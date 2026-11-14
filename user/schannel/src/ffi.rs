//! SSPI's structures and the helpers every entry point uses: handles,
//! buffers, process-heap blocks (what FreeContextBuffer frees), strings,
//! the ledger.

use alloc::string::String;
use alloc::vec::Vec;

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GetProcessHeap() -> usize;
    fn HeapAlloc(heap: usize, flags: u32, size: usize) -> *mut u8;
    fn HeapFree(heap: usize, flags: u32, block: *mut u8) -> i32;
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "ntdll", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "ntdll", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GalaRecordMissing(name: *const u8, length: usize, what: u32, detail: u64) -> u32;
}

/// A zeroed process-heap block (FreeContextBuffer frees it), or null.
pub fn allocate(size: usize) -> *mut u8 {
    // SAFETY: plain allocation, zeroed (HEAP_ZERO_MEMORY).
    unsafe { HeapAlloc(GetProcessHeap(), 8, size.max(1)) }
}

pub fn free(block: *mut u8) {
    if !block.is_null() {
        // SAFETY: a block from the process heap.
        unsafe { HeapFree(GetProcessHeap(), 0, block) };
    }
}

/// Moves `value` to the process heap (null when out of memory).
pub fn boxed<T>(value: T) -> *mut T {
    let p = allocate(size_of::<T>()).cast::<T>();
    if !p.is_null() {
        // SAFETY: a fresh block of the right size (heap blocks are 16-byte
        // aligned).
        unsafe { p.write(value) };
    }
    p
}

/// Drops and frees a block from `boxed`.
pub fn unbox<T>(p: *mut T) {
    if !p.is_null() {
        // SAFETY: a block from `boxed`, not used again.
        unsafe { p.drop_in_place() };
        free(p.cast());
    }
}

/// Records a gap in the ledger (`schannel.dll!...`, "unsupported").
pub fn missing(name: &[u8], detail: u64) {
    // SAFETY: a valid buffer.
    unsafe { GalaRecordMissing(name.as_ptr(), name.len(), 2, detail) };
}

/// Records why a context failed (`schannel.dll!<what>: <reason>`, kind
/// "other", the SSPI status as detail): the reason is rustls' error, so
/// that a failed handshake says why (`what` is short: ISC, ASC,
/// EncryptMessage...; the ledger keeps 64 characters).
pub fn failed(what: &str, reason: &dyn core::fmt::Debug, status: u32) {
    use core::fmt::Write;
    let mut text = String::new();
    let _ = write!(text, "schannel.dll!{what}: {reason:?}");
    // The ledger keeps 64 characters: drop the wrapper of certificate
    // errors (InvalidCertificate(UnknownIssuer) -> UnknownIssuer).
    let text = text.replacen("InvalidCertificate(", "", 1);
    let clean: Vec<u8> = text
        .bytes()
        .map(|b| if (0x20..0x7f).contains(&b) && b != b'"' && b != b'\\' { b } else { b'_' })
        .take(120)
        .collect();
    // SAFETY: a valid buffer.
    unsafe { GalaRecordMissing(clean.as_ptr(), clean.len(), 5, status.into()) };
}

/// SecHandle.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SecHandle {
    pub lower: usize,
    pub upper: usize,
}

/// Tags in SecHandle.dwUpper (secur32 forwards handles with these; a
/// ULONG_PTR, so their low half on x86).
pub const CREDENTIAL_TAG: usize = 0x6761_6c61_5343_4843_u64 as usize; // "galaSCHC"
pub const CONTEXT_TAG: usize = 0x6761_6c61_5343_4858_u64 as usize; // "galaSCHX"

/// The state behind a handle with `tag`, if `handle` is one.
pub fn state<'a, T>(handle: *const SecHandle, tag: usize) -> Option<&'a mut T> {
    if handle.is_null() {
        return None;
    }
    // SAFETY: the caller's SecHandle; a tagged one points at our state.
    unsafe {
        let h = &*handle;
        (h.upper == tag && h.lower != 0).then(|| &mut *(h.lower as *mut T))
    }
}

/// SecBuffer.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SecBuffer {
    pub size: u32,
    pub kind: u32,
    pub data: *mut u8,
}

/// SecBufferDesc.
#[repr(C)]
pub struct SecBufferDesc {
    pub version: u32,
    pub count: u32,
    pub buffers: *mut SecBuffer,
}

pub const SECBUFFER_EMPTY: u32 = 0;
pub const SECBUFFER_DATA: u32 = 1;
pub const SECBUFFER_TOKEN: u32 = 2;
pub const SECBUFFER_MISSING: u32 = 4;
pub const SECBUFFER_EXTRA: u32 = 5;
pub const SECBUFFER_STREAM_TRAILER: u32 = 6;
pub const SECBUFFER_STREAM_HEADER: u32 = 7;
pub const SECBUFFER_ALERT: u32 = 17;
pub const SECBUFFER_APPLICATION_PROTOCOLS: u32 = 18;
pub const SECBUFFER_ATTRMASK: u32 = 0xf000_0000;

/// A buffer's type without its attribute bits (SECBUFFER_READONLY...).
pub fn kind(b: &SecBuffer) -> u32 {
    b.kind & !SECBUFFER_ATTRMASK
}

pub fn buffers<'a>(desc: *const SecBufferDesc) -> &'a mut [SecBuffer] {
    if desc.is_null() {
        return &mut [];
    }
    // SAFETY: the caller's SecBufferDesc and its buffers.
    unsafe {
        let d = &*desc;
        if d.buffers.is_null() {
            return &mut [];
        }
        core::slice::from_raw_parts_mut(d.buffers, d.count as usize)
    }
}

pub fn bytes<'a>(b: &SecBuffer) -> &'a mut [u8] {
    if b.data.is_null() || b.size == 0 {
        return &mut [];
    }
    // SAFETY: the caller's buffer of `size` bytes.
    unsafe { core::slice::from_raw_parts_mut(b.data, b.size as usize) }
}

/// The index of the first buffer of type `k`.
pub fn find(list: &[SecBuffer], k: u32) -> Option<usize> {
    list.iter().position(|b| kind(b) == k)
}

/// Puts `token` in the descriptor's token buffer: a new block with
/// ALLOCATE_MEMORY, else the caller's buffer if it fits. Returns whether
/// it allocated. Without a SECBUFFER_TOKEN buffer the first
/// SECBUFFER_EMPTY one takes the token and becomes SECBUFFER_TOKEN (curl
/// passes its output buffer so, with ISC_REQ_ALLOCATE_MEMORY).
pub fn output_token(
    desc: *mut SecBufferDesc,
    token: &[u8],
    allocate_memory: bool,
) -> Result<bool, u32> {
    let list = buffers(desc);
    let i = match find(list, SECBUFFER_TOKEN) {
        Some(i) => i,
        None if token.is_empty() => return Ok(false),
        None => match find(list, SECBUFFER_EMPTY) {
            Some(i) => {
                list[i].kind = SECBUFFER_TOKEN;
                i
            }
            None => {
                missing(b"schannel.dll!output token (no TOKEN or EMPTY buffer)", list.len() as u64);
                return Err(crate::SEC_E_INVALID_TOKEN);
            }
        },
    };
    let b = &mut list[i];
    if token.is_empty() {
        b.size = 0;
        if allocate_memory {
            b.data = core::ptr::null_mut();
        }
        return Ok(false);
    }
    if allocate_memory {
        let p = allocate(token.len());
        if p.is_null() {
            return Err(crate::SEC_E_INSUFFICIENT_MEMORY);
        }
        b.data = p;
    } else if b.data.is_null() || (b.size as usize) < token.len() {
        return Err(crate::SEC_E_BUFFER_TOO_SMALL);
    }
    // SAFETY: the buffer has room for the token (checked or allocated).
    unsafe { core::ptr::copy_nonoverlapping(token.as_ptr(), b.data, token.len()) };
    b.size = token.len() as u32;
    Ok(allocate_memory)
}

/// The NUL-terminated UTF-16 string at `p`, as text (lossy).
pub fn wide(p: *const u16) -> String {
    let mut s = String::new();
    if p.is_null() {
        return s;
    }
    let mut n = 0;
    // SAFETY: the caller's NUL-terminated string.
    let units = unsafe {
        while *p.add(n) != 0 {
            n += 1;
        }
        core::slice::from_raw_parts(p, n)
    };
    for c in char::decode_utf16(units.iter().copied()) {
        s.push(c.unwrap_or('?'));
    }
    s
}

/// The NUL-terminated ANSI string at `p` (ASCII; others lossy).
pub fn ansi(p: *const u8) -> String {
    let mut s = String::new();
    if p.is_null() {
        return s;
    }
    let mut n = 0;
    // SAFETY: the caller's NUL-terminated string.
    unsafe {
        while *p.add(n) != 0 {
            let b = *p.add(n);
            s.push(if b < 0x80 { b as char } else { '?' });
            n += 1;
        }
    }
    s
}

/// `s` NUL-terminated in UTF-16, into `out` (truncated).
pub fn put_wide(out: &mut [u16], s: &str) {
    let mut n = 0;
    for c in s.encode_utf16() {
        if n + 1 >= out.len() {
            break;
        }
        out[n] = c;
        n += 1;
    }
    if n < out.len() {
        out[n] = 0;
    }
}

/// UNISP_NAME / SCHANNEL_NAME (ASCII, ignoring case).
pub fn is_schannel(name: &str) -> bool {
    name.eq_ignore_ascii_case(crate::PACKAGE) || name.eq_ignore_ascii_case("Schannel")
}

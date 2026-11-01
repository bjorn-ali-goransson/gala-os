//! UUIDs (rpcdce.h): UuidCreate, the string form and its parsing.

#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetProcessHeap() -> usize;
    fn HeapAlloc(heap: usize, flags: u32, size: usize) -> *mut u8;
    fn HeapFree(heap: usize, flags: u32, block: *mut u8) -> i32;
}

#[link(name = "bcryptprimitives", kind = "raw-dylib")]
unsafe extern "system" {
    fn ProcessPrng(buffer: *mut u8, length: usize) -> i32;
}

const RPC_S_OK: i32 = 0;
const RPC_S_OUT_OF_MEMORY: i32 = 14;
const RPC_S_INVALID_ARG: i32 = 87;
const RPC_S_INVALID_STRING_UUID: i32 = 1705;

/// UUID: Data1 (u32), Data2 and Data3 (u16), Data4 (8 bytes).
type Uuid = [u8; 16];

/// UuidCreate: a random UUID (version 4, RFC 4122 variant).
#[unsafe(no_mangle)]
pub extern "system" fn UuidCreate(uuid: *mut Uuid) -> i32 {
    if uuid.is_null() {
        return RPC_S_INVALID_ARG;
    }
    let mut bytes = [0u8; 16];
    // SAFETY: a buffer of ours.
    if unsafe { ProcessPrng(bytes.as_mut_ptr(), bytes.len()) } == 0 {
        return RPC_S_OUT_OF_MEMORY;
    }
    bytes[7] = (bytes[7] & 0x0f) | 0x40; // Data3's high nibble: version 4
    bytes[8] = (bytes[8] & 0x3f) | 0x80; // Data4[0]: the RFC 4122 variant
    // SAFETY: the caller's UUID (checked non-null).
    unsafe { uuid.write_unaligned(bytes) };
    RPC_S_OK
}

/// The 36 characters of `uuid`'s string form (lowercase), as RPC writes
/// it: Data1-Data2-Data3-Data4[0..2]-Data4[2..8], the integers in hex.
fn format(uuid: &Uuid) -> [u8; 36] {
    let mut out = [b'-'; 36];
    // (byte index, little-endian field?) per pair of characters.
    let order: [usize; 16] = [3, 2, 1, 0, 5, 4, 7, 6, 8, 9, 10, 11, 12, 13, 14, 15];
    let mut at = 0;
    for (i, &b) in order.iter().enumerate() {
        if matches!(i, 4 | 6 | 8 | 10) {
            at += 1;
        }
        let digit = |d: u8| if d < 10 { b'0' + d } else { b'a' + d - 10 };
        out[at] = digit(uuid[b] >> 4);
        out[at + 1] = digit(uuid[b] & 0xf);
        at += 2;
    }
    out
}

/// Parses the 36-character form (either case); None if malformed.
fn parse(text: &[u16]) -> Option<Uuid> {
    if text.len() != 36 {
        return None;
    }
    let order: [usize; 16] = [3, 2, 1, 0, 5, 4, 7, 6, 8, 9, 10, 11, 12, 13, 14, 15];
    let mut uuid = [0u8; 16];
    let mut at = 0;
    for (i, &b) in order.iter().enumerate() {
        if matches!(i, 4 | 6 | 8 | 10) {
            if text[at] != u16::from(b'-') {
                return None;
            }
            at += 1;
        }
        let digit = |c: u16| char::from_u32(c.into())?.to_digit(16);
        uuid[b] = (digit(text[at])? << 4 | digit(text[at + 1])?) as u8;
        at += 2;
    }
    Some(uuid)
}

/// Allocates `n` units of `T` on the process heap (RpcStringFree frees).
fn allocate<T>(n: usize) -> *mut T {
    // SAFETY: plain heap allocation.
    unsafe { HeapAlloc(GetProcessHeap(), 0, n * size_of::<T>()).cast() }
}

/// UuidToStringW: a new string (free with RpcStringFreeW).
#[unsafe(no_mangle)]
pub extern "system" fn UuidToStringW(uuid: *const Uuid, out: *mut *mut u16) -> i32 {
    if out.is_null() {
        return RPC_S_INVALID_ARG;
    }
    // A null UUID is the nil UUID, as documented.
    // SAFETY: the caller's UUID, if any.
    let uuid = if uuid.is_null() { [0; 16] } else { unsafe { uuid.read_unaligned() } };
    let text = format(&uuid);
    let p = allocate::<u16>(37);
    if p.is_null() {
        return RPC_S_OUT_OF_MEMORY;
    }
    // SAFETY: 37 units just allocated; the caller's output pointer.
    unsafe {
        for (i, &c) in text.iter().chain(&[0]).enumerate() {
            p.add(i).write(c.into());
        }
        out.write(p);
    }
    RPC_S_OK
}

/// UuidToStringA: as UuidToStringW, in ANSI.
#[unsafe(no_mangle)]
pub extern "system" fn UuidToStringA(uuid: *const Uuid, out: *mut *mut u8) -> i32 {
    if out.is_null() {
        return RPC_S_INVALID_ARG;
    }
    // SAFETY: the caller's UUID, if any.
    let uuid = if uuid.is_null() { [0; 16] } else { unsafe { uuid.read_unaligned() } };
    let text = format(&uuid);
    let p = allocate::<u8>(37);
    if p.is_null() {
        return RPC_S_OUT_OF_MEMORY;
    }
    // SAFETY: 37 bytes just allocated; the caller's output pointer.
    unsafe {
        core::ptr::copy_nonoverlapping(text.as_ptr(), p, 36);
        p.add(36).write(0);
        out.write(p);
    }
    RPC_S_OK
}

/// UuidFromStringW: parses the string form; a null string gives the nil
/// UUID; RPC_S_INVALID_STRING_UUID if malformed.
#[unsafe(no_mangle)]
pub extern "system" fn UuidFromStringW(text: *const u16, uuid: *mut Uuid) -> i32 {
    if uuid.is_null() {
        return RPC_S_INVALID_ARG;
    }
    let parsed = if text.is_null() {
        Some([0; 16])
    } else {
        // Up to 37 units: a longer string is malformed anyway.
        let mut n = 0;
        // SAFETY: the caller's NUL-terminated string, read up to its NUL.
        while n < 37 && unsafe { *text.add(n) } != 0 {
            n += 1;
        }
        // SAFETY: `n` units were just read.
        parse(unsafe { core::slice::from_raw_parts(text, n) })
    };
    let Some(parsed) = parsed else { return RPC_S_INVALID_STRING_UUID };
    // SAFETY: the caller's UUID (checked non-null).
    unsafe { uuid.write_unaligned(parsed) };
    RPC_S_OK
}

/// RpcStringFreeW: frees a string RPC returned and nulls the pointer.
#[unsafe(no_mangle)]
pub extern "system" fn RpcStringFreeW(text: *mut *mut u16) -> i32 {
    free(text.cast())
}

/// RpcStringFreeA: as RpcStringFreeW.
#[unsafe(no_mangle)]
pub extern "system" fn RpcStringFreeA(text: *mut *mut u8) -> i32 {
    free(text)
}

fn free(text: *mut *mut u8) -> i32 {
    if text.is_null() {
        return RPC_S_INVALID_ARG;
    }
    // SAFETY: the caller's pointer to a string RPC allocated (or null).
    unsafe {
        let p = text.read();
        if !p.is_null() {
            HeapFree(GetProcessHeap(), 0, p);
        }
        text.write(core::ptr::null_mut());
    }
    RPC_S_OK
}

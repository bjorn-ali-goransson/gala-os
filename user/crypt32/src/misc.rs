//! Small crypt32 functions libcurl, GnuTLS, the JDK's sunmscapi and SQL
//! Server LocalDB import, as documented: CryptMemAlloc/Realloc/Free (the
//! process heap), CryptProtectMemory/CryptUnprotectMemory (in place,
//! blocks of CRYPTPROTECTMEMORY_BLOCK_SIZE), CertGetNameStringA (the W
//! form in the ANSI code page) and PFXIsPFXBlob (the outer layer of a
//! PKCS #12 PFX: SEQUENCE { INTEGER 3, ContentInfo, ... }).

#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use core::ffi::c_void;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetProcessHeap() -> usize;
    fn HeapAlloc(heap: usize, flags: u32, size: usize) -> *mut u8;
    fn HeapReAlloc(heap: usize, flags: u32, p: *mut u8, size: usize) -> *mut u8;
    fn HeapFree(heap: usize, flags: u32, p: *mut u8) -> i32;
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
    fn SetLastError(e: u32);
}

/// CryptMemAlloc: `size` bytes from the process heap (NULL if none).
#[unsafe(no_mangle)]
pub extern "system" fn CryptMemAlloc(size: u32) -> *mut u8 {
    // SAFETY: plain call.
    unsafe { HeapAlloc(GetProcessHeap(), 0, size as usize) }
}

/// CryptMemRealloc: `p` (from CryptMemAlloc, or NULL) resized, its
/// contents kept; NULL (and `p` untouched) if there is no memory.
#[unsafe(no_mangle)]
pub extern "system" fn CryptMemRealloc(p: *mut u8, size: u32) -> *mut u8 {
    if p.is_null() {
        return CryptMemAlloc(size);
    }
    // SAFETY: a block of the process heap.
    unsafe { HeapReAlloc(GetProcessHeap(), 0, p, size as usize) }
}

/// CryptMemFree: frees memory from CryptMemAlloc (NULL is ignored).
#[unsafe(no_mangle)]
pub extern "system" fn CryptMemFree(p: *mut u8) {
    if !p.is_null() {
        // SAFETY: a block of the process heap.
        unsafe { HeapFree(GetProcessHeap(), 0, p) };
    }
}

const CRYPTPROTECTMEMORY_BLOCK_SIZE: u32 = 16;
const CRYPTPROTECTMEMORY_SAME_PROCESS: u32 = 0;
const CRYPTPROTECTMEMORY_CROSS_PROCESS: u32 = 1;
const CRYPTPROTECTMEMORY_SAME_LOGON: u32 = 2;
const ERROR_INVALID_PARAMETER: u32 = 87;

fn protect(data: *mut u8, size: u32, flags: u32, encrypt: bool) -> i32 {
    let scope = match flags {
        CRYPTPROTECTMEMORY_SAME_PROCESS => crate::dpapi::MemoryScope::Process,
        CRYPTPROTECTMEMORY_CROSS_PROCESS => crate::dpapi::MemoryScope::Machine,
        CRYPTPROTECTMEMORY_SAME_LOGON => crate::dpapi::MemoryScope::Logon,
        _ => {
            // SAFETY: plain call.
            unsafe { SetLastError(ERROR_INVALID_PARAMETER) };
            return 0;
        }
    };
    if data.is_null() || !size.is_multiple_of(CRYPTPROTECTMEMORY_BLOCK_SIZE) {
        // SAFETY: plain call.
        unsafe { SetLastError(ERROR_INVALID_PARAMETER) };
        return 0;
    }
    if size == 0 {
        return 1;
    }
    // SAFETY: the caller's buffer of `size` bytes.
    let buf = unsafe { core::slice::from_raw_parts_mut(data, size as usize) };
    i32::from(crate::dpapi::protect_memory(buf, scope, encrypt))
}

/// CryptProtectMemory: encrypts `size` bytes (a multiple of
/// CRYPTPROTECTMEMORY_BLOCK_SIZE, 16) in place, so that only this
/// process (CRYPTPROTECTMEMORY_SAME_PROCESS), any process
/// (CROSS_PROCESS) or this logon session's processes (SAME_LOGON; gala
/// has one) decrypt them, until the machine restarts. FALSE with
/// ERROR_INVALID_PARAMETER for another size or flag.
#[unsafe(no_mangle)]
pub extern "system" fn CryptProtectMemory(data: *mut u8, size: u32, flags: u32) -> i32 {
    protect(data, size, flags, true)
}

/// CryptUnprotectMemory: CryptProtectMemory undone (the same flags).
#[unsafe(no_mangle)]
pub extern "system" fn CryptUnprotectMemory(data: *mut u8, size: u32, flags: u32) -> i32 {
    protect(data, size, flags, false)
}

/// CertGetNameStringA: CertGetNameStringW's name in the ANSI code page;
/// the size in characters with the NUL (with a NULL buffer or `len` 0,
/// the size needed; with a buffer too small, 1 and an empty string).
#[unsafe(no_mangle)]
pub extern "system" fn CertGetNameStringA(
    cert: *const crate::cert::CertContext,
    kind: u32,
    flags: u32,
    kind_para: *const c_void,
    out: *mut u8,
    len: u32,
) -> u32 {
    let mut wide = [0u16; 1024];
    let n = crate::cert::CertGetNameStringW(cert, kind, flags, kind_para, wide.as_mut_ptr(), 1024);
    if n == 0 {
        return 0;
    }
    // SAFETY: our `n` characters (the NUL included); the caller's buffer
    // of `len` bytes, or a size query.
    let needed = unsafe {
        WideCharToMultiByte(
            0,
            0,
            wide.as_ptr(),
            n as i32,
            core::ptr::null_mut(),
            0,
            core::ptr::null(),
            core::ptr::null_mut(),
        )
    };
    if out.is_null() || len == 0 {
        return needed.max(0) as u32;
    }
    // A buffer too small is not filled with a truncated name: the result
    // is 1, an empty string (Windows 11 25H2, cert_test; that the buffer
    // then holds a NUL is gala's choice, it was not measured).
    if needed as u32 > len {
        // SAFETY: the caller's buffer of `len` >= 1 bytes.
        unsafe { out.write(0) };
        return 1;
    }
    let mut bytes = [0u8; 4096];
    // SAFETY: as above, into our buffer.
    let m = unsafe {
        WideCharToMultiByte(
            0,
            0,
            wide.as_ptr(),
            n as i32,
            bytes.as_mut_ptr(),
            4096,
            core::ptr::null(),
            core::ptr::null_mut(),
        )
    };
    let m = (m.max(1) as usize).min(len as usize);
    // SAFETY: the caller's buffer of `len` >= `m` bytes (the name fits);
    // NUL-terminated.
    unsafe {
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), out, m);
        out.add(m - 1).write(0);
    }
    m as u32
}

/// CRYPT_DATA_BLOB.
#[repr(C)]
pub struct DataBlob {
    pub(crate) size: u32,
    pub(crate) data: *const u8,
}

/// The DER element at `at` in `b`: (tag, content start, content end).
fn element(b: &[u8], at: usize) -> Option<(u8, usize, usize)> {
    let tag = *b.get(at)?;
    let first = *b.get(at + 1)?;
    let (len, start) = if first < 0x80 {
        (first as usize, at + 2)
    } else {
        let count = (first & 0x7f) as usize;
        if count == 0 || count > 4 {
            return None;
        }
        let mut len = 0usize;
        for i in 0..count {
            len = (len << 8) | *b.get(at + 2 + i)? as usize;
        }
        (len, at + 2 + count)
    };
    let end = start.checked_add(len)?;
    (end <= b.len()).then_some((tag, start, end))
}

/// PFXIsPFXBlob: whether the blob's outer layer decodes as a PFX: a
/// SEQUENCE of INTEGER 3 (the version) and a ContentInfo (a SEQUENCE
/// starting with an OBJECT IDENTIFIER).
#[unsafe(no_mangle)]
pub extern "system" fn PFXIsPFXBlob(pfx: *const DataBlob) -> i32 {
    if pfx.is_null() {
        return 0;
    }
    // SAFETY: the caller's blob.
    let b = unsafe { &*pfx };
    if b.data.is_null() || b.size == 0 {
        return 0;
    }
    // SAFETY: the blob's bytes.
    let b = unsafe { core::slice::from_raw_parts(b.data, b.size as usize) };
    let is_pfx = || -> Option<bool> {
        let (tag, start, end) = element(b, 0)?;
        if tag != 0x30 {
            return Some(false);
        }
        let outer = &b[..end];
        let (tag, vs, ve) = element(outer, start)?;
        if tag != 0x02 || outer[vs..ve] != [3] {
            return Some(false);
        }
        let (tag, cs, _) = element(outer, ve)?;
        let (oid, _, _) = element(outer, cs)?;
        Some(tag == 0x30 && oid == 0x06)
    };
    i32::from(is_pfx() == Some(true))
}

//! gala's machine root store in crypt32: the certificates (DER, or PEM
//! with any number of certificates) in %SystemRoot%\System32\gala\roots,
//! the trust anchors schannel.dll validates against too. The "ROOT" system
//! store is opened with them, so programs that build their trust from
//! crypt32's stores (Chromium's TrustStoreWin, curl) trust what Schannel
//! trusts. See user/cert_test.

use core::ffi::c_void;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetSystemDirectoryW(buf: *mut u16, size: u32) -> u32;
    fn FindFirstFileW(pattern: *const u16, data: *mut FindData) -> usize;
    fn FindNextFileW(find: usize, data: *mut FindData) -> i32;
    fn FindClose(find: usize) -> i32;
    fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        security: *const c_void,
        disposition: u32,
        flags: u32,
        template: usize,
    ) -> usize;
    fn ReadFile(h: usize, buf: *mut u8, n: u32, read: *mut u32, overlapped: *mut c_void) -> i32;
    fn GetFileSizeEx(file: usize, size: *mut i64) -> i32;
    fn CloseHandle(h: usize) -> i32;
    fn GetProcessHeap() -> usize;
    fn HeapAlloc(heap: usize, flags: u32, size: usize) -> *mut u8;
    fn HeapFree(heap: usize, flags: u32, p: *mut u8) -> i32;
}

/// WIN32_FIND_DATAW.
#[repr(C)]
struct FindData {
    attributes: u32,
    times: [u32; 6],
    size_high: u32,
    size_low: u32,
    reserved: [u32; 2],
    name: [u16; 260],
    short_name: [u16; 14],
    _obsolete: [u32; 3],
}

const X509_ASN_ENCODING: u32 = 1;
const CERT_STORE_ADD_USE_EXISTING: u32 = 2;
/// Files larger than this are not certificates.
const MAX_FILE: i64 = 1 << 20;

/// Adds the machine's root certificates to `store`; returns how many.
pub fn load(store: *mut c_void) -> u32 {
    let mut path = [0u16; 400];
    // SAFETY: our buffer.
    let n = unsafe { GetSystemDirectoryW(path.as_mut_ptr(), 260) } as usize;
    if n == 0 || n > 260 {
        return 0;
    }
    let suffix = "\\gala\\roots\\";
    let mut len = n;
    for c in suffix.encode_utf16() {
        path[len] = c;
        len += 1;
    }
    let base = len;
    path[len] = u16::from(b'*');
    path[len + 1] = 0;
    // SAFETY: plain data, written by the call.
    let mut data: FindData = unsafe { core::mem::zeroed() };
    // SAFETY: a NUL-terminated pattern; our output.
    let find = unsafe { FindFirstFileW(path.as_ptr(), &mut data) };
    if find == usize::MAX {
        super::store::unsupported(b"crypt32.dll!ROOT store (no gala/roots directory)", 0);
        return 0;
    }
    let mut added = 0;
    loop {
        let name_len = data.name.iter().position(|&c| c == 0).unwrap_or(0);
        if data.attributes & 0x10 == 0 && base + name_len < path.len() - 1 {
            path[base..base + name_len].copy_from_slice(&data.name[..name_len]);
            path[base + name_len] = 0;
            added += load_file(store, &path);
        }
        // SAFETY: the search handle; our output.
        if unsafe { FindNextFileW(find, &mut data) } == 0 {
            break;
        }
    }
    // SAFETY: the search handle.
    unsafe { FindClose(find) };
    added
}

/// A heap buffer, freed when dropped.
struct Buffer(*mut u8, usize);

impl Buffer {
    fn new(len: usize) -> Option<Buffer> {
        // SAFETY: plain heap allocation.
        let p = unsafe { HeapAlloc(GetProcessHeap(), 0, len.max(1)) };
        (!p.is_null()).then_some(Buffer(p, len))
    }
    fn bytes(&self) -> &[u8] {
        // SAFETY: our allocation of that length.
        unsafe { core::slice::from_raw_parts(self.0, self.1) }
    }
}

impl Drop for Buffer {
    fn drop(&mut self) {
        // SAFETY: our allocation.
        unsafe { HeapFree(GetProcessHeap(), 0, self.0) };
    }
}

fn add(store: *mut c_void, der: &[u8]) -> u32 {
    let ok = super::store::CertAddEncodedCertificateToStore(
        store,
        X509_ASN_ENCODING,
        der.as_ptr(),
        der.len() as u32,
        CERT_STORE_ADD_USE_EXISTING,
        core::ptr::null_mut(),
    );
    if ok == 0 {
        // Not a trust anchor then: say so (the length as detail).
        super::store::unsupported(
            b"crypt32.dll!ROOT store (a certificate that does not decode)",
            der.len() as u64,
        );
    }
    u32::from(ok != 0)
}

/// The certificates of one file (NUL-terminated `path`).
fn load_file(store: *mut c_void, path: &[u16]) -> u32 {
    // SAFETY: a NUL-terminated path; GENERIC_READ, FILE_SHARE_READ,
    // OPEN_EXISTING.
    let file = unsafe { CreateFileW(path.as_ptr(), 0x8000_0000, 1, core::ptr::null(), 3, 0, 0) };
    if file == usize::MAX {
        return 0;
    }
    let mut size = 0i64;
    // SAFETY: our handle and output.
    let sized = unsafe { GetFileSizeEx(file, &mut size) } != 0;
    let contents =
        if sized && (1..=MAX_FILE).contains(&size) { Buffer::new(size as usize) } else { None };
    let mut got = 0usize;
    if let Some(b) = &contents {
        while got < b.1 {
            let mut n = 0u32;
            // SAFETY: inside our buffer.
            let ok = unsafe {
                ReadFile(file, b.0.add(got), (b.1 - got) as u32, &mut n, core::ptr::null_mut())
            };
            if ok == 0 || n == 0 {
                break;
            }
            got += n as usize;
        }
    }
    // SAFETY: our handle.
    unsafe { CloseHandle(file) };
    let Some(contents) = contents else { return 0 };
    let bytes = &contents.bytes()[..got];
    if bytes.first() == Some(&0x30) {
        return add(store, bytes);
    }
    // PEM: every CERTIFICATE block.
    const BEGIN: &[u8] = b"-----BEGIN CERTIFICATE-----";
    const END: &[u8] = b"-----END CERTIFICATE-----";
    let find = |hay: &[u8], needle: &[u8]| hay.windows(needle.len()).position(|w| w == needle);
    let mut added = 0;
    let mut rest = bytes;
    while let Some(start) = find(rest, BEGIN) {
        let body = &rest[start + BEGIN.len()..];
        let Some(end) = find(body, END) else { break };
        if let Some(der) = Buffer::new(end * 3 / 4 + 3)
            && let Some(n) = base64(&body[..end], &der)
        {
            added += add(store, &der.bytes()[..n]);
        }
        rest = &body[end + END.len()..];
    }
    added
}

/// Decodes base64 `text` (whitespace skipped) into `out`; its length.
fn base64(text: &[u8], out: &Buffer) -> Option<usize> {
    let (mut acc, mut bits, mut n) = (0u32, 0, 0usize);
    for &c in text {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' | b'\r' | b'\n' | b' ' | b'\t' => continue,
            _ => return None,
        };
        acc = acc << 6 | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            if n >= out.1 {
                return None;
            }
            // SAFETY: inside the buffer (checked).
            unsafe { *out.0.add(n) = (acc >> bits) as u8 };
            n += 1;
        }
    }
    Some(n)
}

//! CryptQueryObject (what a file or blob holds: here a certificate, DER
//! or base64/PEM) and CryptDecodeObjectEx (for now the structure libcurl
//! asks for: a subjectAltName as CERT_ALT_NAME_INFO), as documented.
//! Other content kinds and structure types are recorded.

#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]
#![allow(clippy::too_many_arguments)]

use crate::cert::{Blob, CertCreateCertificateContext, CertFreeCertificateContext};
use crate::layout::{AltNameEntry, AltNameInfo, AltNameValue};
use crate::store::{CertAddCertificateContextToStore, CertOpenStore, error, unsupported};
use core::ffi::c_void;

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
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
    fn GetFileSizeEx(h: usize, size: *mut i64) -> i32;
    fn CloseHandle(h: usize) -> i32;
    fn GetProcessHeap() -> usize;
    fn HeapAlloc(heap: usize, flags: u32, size: usize) -> *mut u8;
    fn HeapFree(heap: usize, flags: u32, p: *mut u8) -> i32;
    fn LocalAlloc(flags: u32, size: usize) -> *mut u8;
}

const X509_ASN_ENCODING: u32 = 1;
const PKCS_7_ASN_ENCODING: u32 = 0x1_0000;
const E_INVALIDARG: u32 = 0x8007_0057;
const CRYPT_E_NO_MATCH: u32 = 0x8009_2009;
const CRYPT_E_ASN1_BADTAG: u32 = 0x8009_310b;
const ERROR_MORE_DATA: u32 = 234;
const ERROR_NOT_ENOUGH_MEMORY: u32 = 8;
const ERROR_FILE_NOT_FOUND: u32 = 2;

/// Heap memory freed on drop.
struct Owned {
    p: *mut u8,
    len: usize,
}

impl Drop for Owned {
    fn drop(&mut self) {
        if !self.p.is_null() {
            // SAFETY: our block.
            unsafe { HeapFree(GetProcessHeap(), 0, self.p) };
        }
    }
}

impl Owned {
    fn new(len: usize) -> Option<Owned> {
        // SAFETY: plain call.
        let p = unsafe { HeapAlloc(GetProcessHeap(), 0, len.max(1)) };
        (!p.is_null()).then_some(Owned { p, len })
    }
    fn bytes(&self) -> &[u8] {
        // SAFETY: our block of `len` bytes.
        unsafe { core::slice::from_raw_parts(self.p, self.len) }
    }
}

fn read_file(path: *const u16) -> Option<Owned> {
    // SAFETY: the caller's NUL-terminated path; our handle and buffer.
    unsafe {
        let h = CreateFileW(path, 0x8000_0000, 7, core::ptr::null(), 3, 0, 0);
        if h == usize::MAX {
            return None;
        }
        let mut size = 0i64;
        let mut data = None;
        if GetFileSizeEx(h, &mut size) != 0
            && (0..=64 << 20).contains(&size)
            && let Some(mut o) = Owned::new(size as usize)
        {
            let mut got = 0u32;
            if ReadFile(h, o.p, size as u32, &mut got, core::ptr::null_mut()) != 0 {
                o.len = got as usize;
                data = Some(o);
            }
        }
        CloseHandle(h);
        data
    }
}

/// The DER of a PEM block (`-----BEGIN ...-----` base64
/// `-----END ...-----`) or of bare base64 text in `text`.
fn from_base64(text: &[u8]) -> Option<Owned> {
    let start = match text.windows(10).position(|w| w == b"-----BEGIN") {
        Some(b) => b + text[b..].iter().position(|&c| c == b'\n')? + 1,
        None => 0,
    };
    let end =
        text[start..].windows(8).position(|w| w == b"-----END").map_or(text.len(), |e| start + e);
    let body = &text[start..end];
    let mut len = 0u32;
    // CRYPT_STRING_BASE64 (1).
    if crate::base64::CryptStringToBinaryA(
        body.as_ptr(),
        body.len() as u32,
        1,
        core::ptr::null_mut(),
        &mut len,
        core::ptr::null_mut(),
        core::ptr::null_mut(),
    ) == 0
    {
        return None;
    }
    let mut out = Owned::new(len as usize)?;
    let mut n = len;
    if crate::base64::CryptStringToBinaryA(
        body.as_ptr(),
        body.len() as u32,
        1,
        out.p,
        &mut n,
        core::ptr::null_mut(),
        core::ptr::null_mut(),
    ) == 0
    {
        return None;
    }
    out.len = n as usize;
    Some(out)
}

const CERT_QUERY_OBJECT_FILE: u32 = 1;
const CERT_QUERY_OBJECT_BLOB: u32 = 2;
const CERT_QUERY_CONTENT_CERT: u32 = 1;
const CERT_QUERY_CONTENT_FLAG_CERT: u32 = 1 << CERT_QUERY_CONTENT_CERT;
const CERT_QUERY_FORMAT_BINARY: u32 = 1;
const CERT_QUERY_FORMAT_BASE64_ENCODED: u32 = 2;
const CERT_QUERY_FORMAT_FLAG_BINARY: u32 = 1 << CERT_QUERY_FORMAT_BINARY;
const CERT_QUERY_FORMAT_FLAG_BASE64_ENCODED: u32 = 1 << CERT_QUERY_FORMAT_BASE64_ENCODED;
const CERT_STORE_ADD_ALWAYS: u32 = 4;

/// DATA_BLOB.
#[repr(C)]
struct DataBlob {
    size: u32,
    data: *const u8,
}

fn put<T>(p: *mut T, v: T) {
    if !p.is_null() {
        // SAFETY: the caller's output pointer.
        unsafe { p.write(v) };
    }
}

/// CryptQueryObject: what a file (CERT_QUERY_OBJECT_FILE) or blob holds;
/// gala recognises a certificate, DER (CERT_QUERY_FORMAT_BINARY) or
/// base64 with or without PEM lines (CERT_QUERY_FORMAT_BASE64_ENCODED),
/// giving its context and a memory store holding it. Anything else, or a
/// certificate the flags do not ask for, is CRYPT_E_NO_MATCH (other
/// content kinds are recorded).
#[unsafe(no_mangle)]
pub extern "system" fn CryptQueryObject(
    object_type: u32,
    object: *const c_void,
    content_flags: u32,
    format_flags: u32,
    _flags: u32,
    encoding: *mut u32,
    content: *mut u32,
    format: *mut u32,
    store: *mut *mut c_void,
    msg: *mut *mut c_void,
    context: *mut *const c_void,
) -> i32 {
    if object.is_null() {
        return error(E_INVALIDARG, 0);
    }
    let file;
    let data: &[u8] = match object_type {
        CERT_QUERY_OBJECT_FILE => {
            let Some(f) = read_file(object.cast()) else { return error(ERROR_FILE_NOT_FOUND, 0) };
            file = f;
            file.bytes()
        }
        CERT_QUERY_OBJECT_BLOB => {
            // SAFETY: the caller's blob.
            let b = unsafe { &*(object as *const DataBlob) };
            if b.data.is_null() {
                return error(E_INVALIDARG, 0);
            }
            // SAFETY: as above.
            unsafe { core::slice::from_raw_parts(b.data, b.size as usize) }
        }
        _ => return error(E_INVALIDARG, 0),
    };
    if content_flags & !CERT_QUERY_CONTENT_FLAG_CERT != 0 {
        unsupported(b"crypt32.dll!CryptQueryObject(content)", content_flags.into());
    }
    if content_flags & CERT_QUERY_CONTENT_FLAG_CERT == 0 {
        return error(CRYPT_E_NO_MATCH, 0);
    }
    let decoded;
    let (der, fmt): (&[u8], u32) =
        if data.first() == Some(&0x30) && format_flags & CERT_QUERY_FORMAT_FLAG_BINARY != 0 {
            (data, CERT_QUERY_FORMAT_BINARY)
        } else if format_flags & CERT_QUERY_FORMAT_FLAG_BASE64_ENCODED != 0
            && let Some(d) = from_base64(data)
        {
            decoded = d;
            (decoded.bytes(), CERT_QUERY_FORMAT_BASE64_ENCODED)
        } else {
            return error(CRYPT_E_NO_MATCH, 0);
        };
    let cert = CertCreateCertificateContext(X509_ASN_ENCODING, der.as_ptr(), der.len() as u32);
    if cert.is_null() {
        return error(CRYPT_E_NO_MATCH, 0);
    }
    put(encoding, X509_ASN_ENCODING | PKCS_7_ASN_ENCODING);
    put(content, CERT_QUERY_CONTENT_CERT);
    put(format, fmt);
    put(msg, core::ptr::null_mut());
    if !store.is_null() {
        // CERT_STORE_PROV_MEMORY (2).
        let s = CertOpenStore(2 as *const u8, 0, 0, 0, core::ptr::null());
        if s.is_null() {
            CertFreeCertificateContext(cert);
            return error(ERROR_NOT_ENOUGH_MEMORY, 0);
        }
        CertAddCertificateContextToStore(s, cert, CERT_STORE_ADD_ALWAYS, core::ptr::null_mut());
        put(store, s);
    }
    if context.is_null() {
        CertFreeCertificateContext(cert);
    } else {
        put(context, cert.cast());
    }
    1
}

// ---- CryptDecodeObjectEx ----

const X509_ALTERNATE_NAME: usize = 12;
const CRYPT_DECODE_ALLOC_FLAG: u32 = 0x8000;

/// CRYPT_DECODE_PARA: its size and an allocator.
#[repr(C)]
struct DecodePara {
    size: u32,
    alloc: Option<extern "system" fn(usize) -> *mut u8>,
    free: Option<extern "system" fn(*mut u8)>,
}

/// Whether the structure type (an LPCSTR: a small number or an OID
/// string) is the subjectAltName.
fn is_alt_name(t: *const u8) -> bool {
    let n = t as usize;
    if n < 0x10000 {
        return n == X509_ALTERNATE_NAME;
    }
    let mut i = 0;
    // SAFETY: the caller's NUL-terminated OID string.
    let s = unsafe {
        while *t.add(i) != 0 && i < 64 {
            i += 1;
        }
        core::slice::from_raw_parts(t, i)
    };
    // szOID_SUBJECT_ALT_NAME2, szOID_ISSUER_ALT_NAME2, szOID_SUBJECT_ALT_NAME.
    s == b"2.5.29.17" || s == b"2.5.29.18" || s == b"2.5.29.7"
}

/// A DER TLV at `at`: (tag, content start, end).
fn tlv(b: &[u8], at: usize) -> Option<(u8, usize, usize)> {
    let tag = *b.get(at)?;
    let first = *b.get(at + 1)?;
    let (len, start) = if first < 0x80 {
        (first as usize, at + 2)
    } else {
        let n = (first & 0x7f) as usize;
        if n == 0 || n > 4 {
            return None;
        }
        let mut len = 0;
        for i in 0..n {
            len = len << 8 | *b.get(at + 2 + i)? as usize;
        }
        (len, at + 2 + n)
    };
    let end = start.checked_add(len)?;
    (end <= b.len()).then_some((tag, start, end))
}

/// CERT_ALT_NAME_ENTRY's choices.
const CERT_ALT_NAME_RFC822_NAME: u32 = 2;
const CERT_ALT_NAME_DNS_NAME: u32 = 3;
const CERT_ALT_NAME_URL: u32 = 7;
const CERT_ALT_NAME_IP_ADDRESS: u32 = 8;

/// The entries of a GeneralNames SEQUENCE: (choice, its content range).
fn alt_entries(der: &[u8], mut f: impl FnMut(u32, &[u8])) -> Result<usize, u32> {
    let (tag, start, end) = tlv(der, 0).ok_or(CRYPT_E_ASN1_BADTAG)?;
    if tag != 0x30 {
        return Err(CRYPT_E_ASN1_BADTAG);
    }
    let mut at = start;
    let mut n = 0;
    while at < end {
        let (t, s, e) = tlv(der, at).ok_or(CRYPT_E_ASN1_BADTAG)?;
        let choice = match t {
            0x81 => CERT_ALT_NAME_RFC822_NAME,
            0x82 => CERT_ALT_NAME_DNS_NAME,
            0x86 => CERT_ALT_NAME_URL,
            0x87 => CERT_ALT_NAME_IP_ADDRESS,
            _ => {
                // otherName, x400, directoryName, ediParty, registeredID.
                unsupported(b"crypt32.dll!CryptDecodeObjectEx(GeneralName)", t.into());
                at = e;
                continue;
            }
        };
        f(choice, &der[s..e]);
        n += 1;
        at = e;
    }
    Ok(n)
}

/// CERT_ALT_NAME_INFO and its entries (CERT_ALT_NAME_ENTRY: the choice,
/// then a string pointer or a CRYPT_DATA_BLOB; `layout`).
fn alt_name(der: &[u8], out: *mut u8, room: usize) -> Result<usize, u32> {
    use core::mem::size_of;
    // The size: header, entries, then the strings (UTF-16, NUL) and IPs.
    let mut extra = 0;
    let count = alt_entries(der, |choice, v| {
        extra += if choice == CERT_ALT_NAME_IP_ADDRESS { v.len() } else { (v.len() + 1) * 2 };
    })?;
    let header = size_of::<AltNameInfo>();
    let entry = size_of::<AltNameEntry>();
    let size = header + count * entry + extra.next_multiple_of(size_of::<usize>());
    if out.is_null() || room < size {
        return Ok(size);
    }
    // SAFETY: the caller's (or our allocated) buffer of `size` bytes.
    unsafe {
        core::ptr::write_bytes(out, 0, size);
        let entries = out.add(header).cast::<AltNameEntry>();
        out.cast::<AltNameInfo>().write_unaligned(AltNameInfo { count: count as u32, entries });
        let mut text = out.add(header + count * entry);
        let mut i = 0;
        alt_entries(der, |choice, v| {
            let e = entries.add(i);
            if choice == CERT_ALT_NAME_IP_ADDRESS {
                let blob = Blob { size: v.len() as u32, data: text };
                e.write_unaligned(AltNameEntry { choice, value: AltNameValue { blob } });
                core::ptr::copy_nonoverlapping(v.as_ptr(), text, v.len());
                text = text.add(v.len());
            } else {
                e.write_unaligned(AltNameEntry {
                    choice,
                    value: AltNameValue { text: text.cast() },
                });
                for (k, &b) in v.iter().enumerate() {
                    (text.add(k * 2) as *mut u16).write_unaligned(b.into());
                }
                (text.add(v.len() * 2) as *mut u16).write_unaligned(0);
                text = text.add((v.len() + 1) * 2);
            }
            i += 1;
        })?;
    }
    Ok(size)
}

/// CryptDecodeObjectEx: decodes `data` as the structure `struct_type`
/// into the caller's buffer (a size query with no buffer, ERROR_MORE_DATA
/// when short) or, with CRYPT_DECODE_ALLOC_FLAG, into memory it
/// allocates (LocalAlloc, or the CRYPT_DECODE_PARA's allocator). gala
/// decodes the alternative names (X509_ALTERNATE_NAME and the
/// subject/issuer alt name OIDs: e-mail, DNS, URL and IP entries); other
/// types are recorded and fail with CRYPT_E_ASN1_BADTAG.
#[unsafe(no_mangle)]
pub extern "system" fn CryptDecodeObjectEx(
    encoding: u32,
    struct_type: *const u8,
    data: *const u8,
    len: u32,
    flags: u32,
    para: *const c_void,
    out: *mut c_void,
    out_len: *mut u32,
) -> i32 {
    if encoding & X509_ASN_ENCODING == 0
        || data.is_null()
        || out_len.is_null()
        || struct_type.is_null()
    {
        return error(E_INVALIDARG, 0);
    }
    if !is_alt_name(struct_type) {
        unsupported(b"crypt32.dll!CryptDecodeObjectEx(type)", struct_type as u64 & 0xffff_ffff);
        return error(CRYPT_E_ASN1_BADTAG, 0);
    }
    // SAFETY: the caller's encoding.
    let der = unsafe { core::slice::from_raw_parts(data, len as usize) };
    let size = match alt_name(der, core::ptr::null_mut(), 0) {
        Ok(s) => s,
        Err(e) => return error(e, 0),
    };
    if flags & CRYPT_DECODE_ALLOC_FLAG != 0 {
        if out.is_null() {
            return error(E_INVALIDARG, 0);
        }
        // SAFETY: the caller's CRYPT_DECODE_PARA, if any.
        let alloc = unsafe { (para as *const DecodePara).as_ref() }.and_then(|p| p.alloc);
        let p = match alloc {
            Some(a) => a(size),
            // SAFETY: plain call (LMEM_FIXED).
            None => unsafe { LocalAlloc(0, size) },
        };
        if p.is_null() {
            return error(ERROR_NOT_ENOUGH_MEMORY, 0);
        }
        let _ = alt_name(der, p, size);
        // SAFETY: the caller's output pointer and size.
        unsafe {
            *(out as *mut *mut u8) = p;
            *out_len = size as u32;
        }
        return 1;
    }
    // SAFETY: the caller's size.
    let room = unsafe { *out_len } as usize;
    // SAFETY: as above.
    unsafe { *out_len = size as u32 };
    if out.is_null() {
        return 1;
    }
    if room < size {
        return error(ERROR_MORE_DATA, 0);
    }
    let _ = alt_name(der, out.cast(), room);
    1
}

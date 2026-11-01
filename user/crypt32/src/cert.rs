//! Certificate contexts: CertCreateCertificateContext decodes a DER
//! X.509 certificate (RFC 5280) into the documented CERT_CONTEXT and
//! CERT_INFO, in one block on the process heap that also holds a copy of
//! the encoding (the blobs point into it), the extensions and the OID
//! strings. A context is reference-counted (CertDuplicate/Free) and keeps
//! a list of properties; the hash properties are computed with bcrypt
//! (loaded on first use, like DPAPI's). Names are read from the encoded
//! Name (CertGetNameStringW, CertNameToStrW); CryptImportPublicKeyInfoEx2
//! makes a bcrypt RSA key of the public key.
//!
//! No certificate stores yet: hCertStore is null.

use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, Ordering};

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetProcessHeap() -> usize;
    fn HeapAlloc(heap: usize, flags: u32, size: usize) -> *mut u8;
    fn HeapFree(heap: usize, flags: u32, p: *mut u8) -> i32;
    fn SetLastError(code: u32);
    fn LoadLibraryA(name: *const u8) -> usize;
    fn GetProcAddress(module: usize, name: *const u8) -> usize;
}

const X509_ASN_ENCODING: u32 = 1;
const E_INVALIDARG: u32 = 0x8007_0057;
const CRYPT_E_ASN1_CORRUPT: u32 = 0x8009_3100;
const CRYPT_E_ASN1_EOD: u32 = 0x8009_3102;
const CRYPT_E_ASN1_BADTAG: u32 = 0x8009_310b;
const CRYPT_E_NOT_FOUND: u32 = 0x8009_2004;
const ERROR_MORE_DATA: u32 = 234;
const ERROR_INVALID_PARAMETER: u32 = 87;
const ERROR_NOT_ENOUGH_MEMORY: u32 = 8;
const NTE_BAD_ALGID: u32 = 0x8009_0008;

fn error<T>(code: u32, value: T) -> T {
    // SAFETY: plain call.
    unsafe { SetLastError(code) };
    value
}

// ---- DER ----

/// One TLV: its tag, contents and the whole encoding.
#[derive(Clone, Copy)]
struct Tlv<'a> {
    tag: u8,
    content: &'a [u8],
    whole: &'a [u8],
}

/// Reads TLVs from `data` in order.
struct Der<'a> {
    data: &'a [u8],
}

impl<'a> Der<'a> {
    fn new(data: &'a [u8]) -> Der<'a> {
        Der { data }
    }

    fn peek_tag(&self) -> Option<u8> {
        self.data.first().copied()
    }

    fn next(&mut self) -> Result<Tlv<'a>, u32> {
        let d = self.data;
        if d.len() < 2 {
            return Err(CRYPT_E_ASN1_EOD);
        }
        let tag = d[0];
        if tag & 0x1f == 0x1f {
            return Err(CRYPT_E_ASN1_BADTAG); // multi-byte tags: not in X.509
        }
        let (len, head) = match d[1] {
            n if n < 0x80 => (usize::from(n), 2),
            0x80 => return Err(CRYPT_E_ASN1_CORRUPT), // indefinite: not DER
            n => {
                let count = usize::from(n & 0x7f);
                if count > 4 || d.len() < 2 + count {
                    return Err(CRYPT_E_ASN1_EOD);
                }
                let len = d[2..2 + count].iter().fold(0usize, |a, &b| (a << 8) | usize::from(b));
                (len, 2 + count)
            }
        };
        let end = head.checked_add(len).ok_or(CRYPT_E_ASN1_EOD)?;
        if d.len() < end {
            return Err(CRYPT_E_ASN1_EOD);
        }
        self.data = &d[end..];
        Ok(Tlv { tag, content: &d[head..end], whole: &d[..end] })
    }

    /// The next TLV, which must have tag `tag`.
    fn expect(&mut self, tag: u8) -> Result<Tlv<'a>, u32> {
        let t = self.next()?;
        if t.tag != tag { Err(CRYPT_E_ASN1_BADTAG) } else { Ok(t) }
    }
}

const SEQUENCE: u8 = 0x30;
const SET: u8 = 0x31;
const INTEGER: u8 = 0x02;
const BIT_STRING: u8 = 0x03;
const OCTET_STRING: u8 = 0x04;
const OID: u8 = 0x06;
const BOOLEAN: u8 = 0x01;

/// The dotted text of OID contents `oid` into `out` (NUL-terminated);
/// its length without the NUL.
fn oid_text(oid: &[u8], out: &mut [u8]) -> Result<usize, u32> {
    let mut at = 0;
    let mut put = |text: &[u8]| -> Result<(), u32> {
        if at + text.len() + 1 > out.len() {
            return Err(CRYPT_E_ASN1_CORRUPT);
        }
        out[at..at + text.len()].copy_from_slice(text);
        at += text.len();
        Ok(())
    };
    let number = |v: u64, buf: &mut [u8; 20]| -> usize {
        let mut n = 20;
        let mut v = v;
        loop {
            n -= 1;
            buf[n] = b'0' + (v % 10) as u8;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        buf.copy_within(n.., 0);
        20 - n
    };
    let mut value = 0u64;
    let mut first = true;
    if oid.is_empty() {
        return Err(CRYPT_E_ASN1_CORRUPT);
    }
    for &b in oid {
        if value > u64::MAX >> 7 {
            return Err(CRYPT_E_ASN1_CORRUPT);
        }
        value = (value << 7) | u64::from(b & 0x7f);
        if b & 0x80 != 0 {
            continue;
        }
        let mut buf = [0u8; 20];
        if first {
            let (a, b) = if value < 80 { (value / 40, value % 40) } else { (2, value - 80) };
            let n = number(a, &mut buf);
            put(&buf[..n])?;
            put(b".")?;
            let n = number(b, &mut buf);
            put(&buf[..n])?;
            first = false;
        } else {
            put(b".")?;
            let n = number(value, &mut buf);
            put(&buf[..n])?;
        }
        value = 0;
    }
    out[at] = 0;
    Ok(at)
}

/// Days from 1601-01-01 to `y`-`m`-`d` (proleptic Gregorian).
fn days_from_1601(y: u64, m: u64, d: u64) -> u64 {
    let leap = |y: u64| (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let before = y - 1601;
    let mut days = before * 365 + before / 4 - before / 100 + before / 400;
    const CUM: [u64; 12] = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
    days += CUM[(m - 1) as usize];
    if m > 2 && leap(y) {
        days += 1;
    }
    days + d - 1
}

/// A UTCTime or GeneralizedTime (Z form) as a FILETIME.
fn time(t: Tlv<'_>) -> Result<u64, u32> {
    let s = t.content;
    let digits = |r: core::ops::Range<usize>| -> Result<u64, u32> {
        let part = s.get(r).ok_or(CRYPT_E_ASN1_CORRUPT)?;
        part.iter().try_fold(0u64, |a, &c| {
            if c.is_ascii_digit() {
                Ok(a * 10 + u64::from(c - b'0'))
            } else {
                Err(CRYPT_E_ASN1_CORRUPT)
            }
        })
    };
    let (year, rest) = match t.tag {
        0x17 => {
            let yy = digits(0..2)?;
            (if yy >= 50 { 1900 + yy } else { 2000 + yy }, 2)
        }
        0x18 => (digits(0..4)?, 4),
        _ => return Err(CRYPT_E_ASN1_BADTAG),
    };
    let (mo, d, h, mi) = (
        digits(rest..rest + 2)?,
        digits(rest + 2..rest + 4)?,
        digits(rest + 4..rest + 6)?,
        digits(rest + 6..rest + 8)?,
    );
    let sec = digits(rest + 8..rest + 10).unwrap_or(0);
    if year < 1601 || !(1..=12).contains(&mo) || !(1..=31).contains(&d) {
        return Err(CRYPT_E_ASN1_CORRUPT);
    }
    let secs = ((days_from_1601(year, mo, d) * 24 + h) * 60 + mi) * 60 + sec;
    Ok(secs * 10_000_000)
}

// ---- The structures (x64 layouts) ----

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Blob {
    pub size: u32,
    pub data: *mut u8,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Algorithm {
    pub oid: *mut u8,
    pub parameters: Blob,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct BitBlob {
    pub size: u32,
    pub data: *mut u8,
    pub unused: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct PublicKeyInfo {
    pub algorithm: Algorithm,
    pub key: BitBlob,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Extension {
    pub oid: *mut u8,
    pub critical: i32,
    pub value: Blob,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct CertInfo {
    pub version: u32,
    pub serial: Blob,
    pub signature: Algorithm,
    pub issuer: Blob,
    pub not_before: u64,
    pub not_after: u64,
    pub subject: Blob,
    pub public_key: PublicKeyInfo,
    pub issuer_id: BitBlob,
    pub subject_id: BitBlob,
    pub extension_count: u32,
    pub extensions: *mut Extension,
}

#[repr(C)]
pub struct CertContext {
    pub encoding: u32,
    pub encoded: *mut u8,
    pub encoded_len: u32,
    pub info: *mut CertInfo,
    pub store: *mut c_void,
}

const CERT_MAGIC: u32 = 0x6761_6c63; // "galc"

/// The heap block of a context: this header, then the extensions, the
/// serial number (reversed), the OID strings and the encoding.
#[repr(C)]
struct Block {
    magic: u32,
    refs: AtomicU32,
    props: *mut Prop,
    context: CertContext,
    info: CertInfo,
}

/// A property: `len` bytes follow the header.
#[repr(C)]
struct Prop {
    next: *mut Prop,
    id: u32,
    len: u32,
}

/// The block of context `p`, if it is one of ours.
fn block<'a>(p: *const CertContext) -> Option<&'a mut Block> {
    if p.is_null() {
        return None;
    }
    let offset = core::mem::offset_of!(Block, context);
    let b = (p as usize).checked_sub(offset)? as *mut Block;
    // SAFETY: a context the caller got from us starts `offset` bytes into
    // its block, whose magic we check.
    let b = unsafe { &mut *b };
    (b.magic == CERT_MAGIC).then_some(b)
}

/// An algorithm identifier: (OID contents, the encoded parameters).
fn algorithm_id<'a>(t: Tlv<'a>) -> Result<(&'a [u8], &'a [u8]), u32> {
    if t.tag != SEQUENCE {
        return Err(CRYPT_E_ASN1_BADTAG);
    }
    let mut d = Der::new(t.content);
    let oid = d.expect(OID)?;
    Ok((oid.content, d.data))
}

struct Parsed<'a> {
    version: u32,
    serial: &'a [u8],
    signature: (&'a [u8], &'a [u8]),
    issuer: &'a [u8],
    not_before: u64,
    not_after: u64,
    subject: &'a [u8],
    key_alg: (&'a [u8], &'a [u8]),
    key: &'a [u8],
    key_unused: u32,
    issuer_id: Option<&'a [u8]>,
    subject_id: Option<&'a [u8]>,
    /// The extensions (encoded SEQUENCE OF), if any.
    extensions: &'a [u8],
}

fn parse(cert: &[u8]) -> Result<(Parsed<'_>, usize), u32> {
    let mut outer = Der::new(cert);
    let whole = outer.expect(SEQUENCE)?;
    let mut c = Der::new(whole.content);
    let tbs = c.expect(SEQUENCE)?;
    let mut t = Der::new(tbs.content);
    let mut version = 0;
    if t.peek_tag() == Some(0xa0) {
        let v = t.next()?;
        let int = Der::new(v.content).expect(INTEGER)?;
        version = u32::from(*int.content.last().ok_or(CRYPT_E_ASN1_CORRUPT)?);
    }
    let serial = t.expect(INTEGER)?.content;
    let signature = algorithm_id(t.next()?)?;
    let issuer = t.expect(SEQUENCE)?.whole;
    let validity = t.expect(SEQUENCE)?;
    let mut v = Der::new(validity.content);
    let not_before = time(v.next()?)?;
    let not_after = time(v.next()?)?;
    let subject = t.expect(SEQUENCE)?.whole;
    let spki = t.expect(SEQUENCE)?;
    let mut s = Der::new(spki.content);
    let key_alg = algorithm_id(s.next()?)?;
    let bits = s.expect(BIT_STRING)?.content;
    let (&key_unused, key) = bits.split_first().ok_or(CRYPT_E_ASN1_CORRUPT)?;
    let (mut issuer_id, mut subject_id, mut extensions) = (None, None, &[][..]);
    while let Some(tag) = t.peek_tag() {
        let x = t.next()?;
        match tag {
            0x81 => issuer_id = Some(x.content),
            0x82 => subject_id = Some(x.content),
            0xa3 => extensions = Der::new(x.content).expect(SEQUENCE)?.content,
            _ => return Err(CRYPT_E_ASN1_BADTAG),
        }
    }
    // The signature algorithm and value must follow.
    algorithm_id(c.next()?)?;
    c.expect(BIT_STRING)?;
    let parsed = Parsed {
        version,
        serial,
        signature,
        issuer,
        not_before,
        not_after,
        subject,
        key_alg,
        key,
        key_unused: u32::from(key_unused),
        issuer_id,
        subject_id,
        extensions,
    };
    Ok((parsed, whole.whole.len()))
}

fn alloc(size: usize) -> *mut u8 {
    // SAFETY: plain call; zeroed (HEAP_ZERO_MEMORY).
    unsafe { HeapAlloc(GetProcessHeap(), 8, size) }
}

fn free(p: *mut u8) {
    // SAFETY: a block of ours.
    unsafe { HeapFree(GetProcessHeap(), 0, p) };
}

/// Room for one OID string.
const OID_TEXT: usize = 64;

#[unsafe(no_mangle)]
pub extern "system" fn CertCreateCertificateContext(
    encoding: u32,
    data: *const u8,
    len: u32,
) -> *const CertContext {
    if encoding & X509_ASN_ENCODING == 0 || data.is_null() {
        return error(E_INVALIDARG, core::ptr::null());
    }
    // SAFETY: the caller's buffer of `len` bytes.
    let input = unsafe { core::slice::from_raw_parts(data, len as usize) };
    let (parsed, used) = match parse(input) {
        Ok(p) => p,
        Err(e) => return error(e, core::ptr::null()),
    };
    // Count the extensions (and check them).
    let mut count = 0usize;
    let mut d = Der::new(parsed.extensions);
    while d.peek_tag().is_some() {
        match d.expect(SEQUENCE) {
            Ok(_) => count += 1,
            Err(e) => return error(e, core::ptr::null()),
        }
    }
    let header = core::mem::size_of::<Block>();
    let ext_bytes = count * core::mem::size_of::<Extension>();
    let oids = (2 + count) * OID_TEXT;
    let total = header + ext_bytes + parsed.serial.len() + oids + used;
    let p = alloc(total);
    if p.is_null() {
        return error(ERROR_NOT_ENOUGH_MEMORY, core::ptr::null());
    }
    // SAFETY: `p` has `total` zeroed bytes laid out as described; blobs
    // point into the copy of the encoding made first.
    unsafe {
        let exts = p.add(header).cast::<Extension>();
        let serial = p.add(header + ext_bytes);
        let mut oid_at = serial.add(parsed.serial.len());
        let encoded = p.add(total - used);
        core::ptr::copy_nonoverlapping(data, encoded, used);
        // A slice of the input as a pointer into the copy.
        let at = |s: &[u8]| encoded.add(s.as_ptr() as usize - data as usize);
        let blob = |s: &[u8]| Blob { size: s.len() as u32, data: at(s) };
        let mut oid = |contents: &[u8]| -> Result<*mut u8, u32> {
            let dst = oid_at;
            oid_text(contents, core::slice::from_raw_parts_mut(dst, OID_TEXT))?;
            oid_at = oid_at.add(OID_TEXT);
            Ok(dst)
        };
        let mut fill = || -> Result<CertInfo, u32> {
            for (i, &b) in parsed.serial.iter().rev().enumerate() {
                *serial.add(i) = b;
            }
            let mut d = Der::new(parsed.extensions);
            for i in 0..count {
                let e = d.expect(SEQUENCE)?;
                let mut x = Der::new(e.content);
                let id = x.expect(OID)?;
                let mut critical = 0;
                if x.peek_tag() == Some(BOOLEAN) {
                    critical = i32::from(x.next()?.content.first().is_some_and(|&b| b != 0));
                }
                let value = x.expect(OCTET_STRING)?;
                exts.add(i).write(Extension {
                    oid: oid(id.content)?,
                    critical,
                    value: blob(value.content),
                });
            }
            let bits = |s: Option<&[u8]>| match s {
                Some(s) if !s.is_empty() => {
                    BitBlob { size: s.len() as u32 - 1, data: at(&s[1..]), unused: u32::from(s[0]) }
                }
                _ => BitBlob { size: 0, data: core::ptr::null_mut(), unused: 0 },
            };
            Ok(CertInfo {
                version: parsed.version,
                serial: Blob { size: parsed.serial.len() as u32, data: serial },
                signature: Algorithm {
                    oid: oid(parsed.signature.0)?,
                    parameters: blob(parsed.signature.1),
                },
                issuer: blob(parsed.issuer),
                not_before: parsed.not_before,
                not_after: parsed.not_after,
                subject: blob(parsed.subject),
                public_key: PublicKeyInfo {
                    algorithm: Algorithm {
                        oid: oid(parsed.key_alg.0)?,
                        parameters: blob(parsed.key_alg.1),
                    },
                    key: BitBlob {
                        size: parsed.key.len() as u32,
                        data: at(parsed.key),
                        unused: parsed.key_unused,
                    },
                },
                issuer_id: bits(parsed.issuer_id),
                subject_id: bits(parsed.subject_id),
                extension_count: count as u32,
                extensions: if count == 0 { core::ptr::null_mut() } else { exts },
            })
        };
        let info = match fill() {
            Ok(i) => i,
            Err(e) => {
                free(p);
                return error(e, core::ptr::null());
            }
        };
        let b = p.cast::<Block>();
        b.write(Block {
            magic: CERT_MAGIC,
            refs: AtomicU32::new(1),
            props: core::ptr::null_mut(),
            context: CertContext {
                encoding,
                encoded,
                encoded_len: used as u32,
                info: core::ptr::null_mut(),
                store: core::ptr::null_mut(),
            },
            info,
        });
        (*b).context.info = &raw mut (*b).info;
        &raw const (*b).context
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn CertDuplicateCertificateContext(
    p: *const CertContext,
) -> *const CertContext {
    if let Some(b) = block(p) {
        b.refs.fetch_add(1, Ordering::AcqRel);
    }
    p
}

#[unsafe(no_mangle)]
pub extern "system" fn CertFreeCertificateContext(p: *const CertContext) -> i32 {
    let Some(b) = block(p) else { return 1 };
    if b.refs.fetch_sub(1, Ordering::AcqRel) == 1 {
        let mut prop = b.props;
        while !prop.is_null() {
            // SAFETY: our property list.
            let next = unsafe { (*prop).next };
            free(prop.cast());
            prop = next;
        }
        b.magic = 0;
        free((b as *mut Block).cast());
    }
    1
}

// ---- Properties ----

const CERT_KEY_PROV_HANDLE_PROP_ID: u32 = 1;
const CERT_KEY_PROV_INFO_PROP_ID: u32 = 2;
const CERT_SHA1_HASH_PROP_ID: u32 = 3;
const CERT_MD5_HASH_PROP_ID: u32 = 4;
const CERT_KEY_CONTEXT_PROP_ID: u32 = 5;
const CERT_KEY_SPEC_PROP_ID: u32 = 6;
const CERT_KEY_IDENTIFIER_PROP_ID: u32 = 20;
const CERT_NCRYPT_KEY_HANDLE_PROP_ID: u32 = 78;
const CERT_HCRYPTPROV_OR_NCRYPT_KEY_HANDLE_PROP_ID: u32 = 79;
const CERT_SHA256_HASH_PROP_ID: u32 = 107;

fn find_prop(b: &Block, id: u32) -> Option<&'static [u8]> {
    let mut p = b.props;
    while !p.is_null() {
        // SAFETY: our property list; data follows each header.
        unsafe {
            if (*p).id == id {
                let data = p.cast::<u8>().add(core::mem::size_of::<Prop>());
                return Some(core::slice::from_raw_parts(data, (*p).len as usize));
            }
            p = (*p).next;
        }
    }
    None
}

fn remove_prop(b: &mut Block, id: u32) {
    let mut link: *mut *mut Prop = &raw mut b.props;
    // SAFETY: our property list.
    unsafe {
        while !(*link).is_null() {
            let p = *link;
            if (*p).id == id {
                *link = (*p).next;
                free(p.cast());
                return;
            }
            link = &raw mut (*p).next;
        }
    }
}

fn add_prop(b: &mut Block, id: u32, data: &[u8]) -> bool {
    remove_prop(b, id);
    let p = alloc(core::mem::size_of::<Prop>() + data.len()).cast::<Prop>();
    if p.is_null() {
        return false;
    }
    // SAFETY: a fresh block of the header plus `data`.
    unsafe {
        p.write(Prop { next: b.props, id, len: data.len() as u32 });
        core::ptr::copy_nonoverlapping(
            data.as_ptr(),
            p.cast::<u8>().add(core::mem::size_of::<Prop>()),
            data.len(),
        );
    }
    b.props = p;
    true
}

/// A bcrypt function, resolved by name.
fn bcrypt(name: &core::ffi::CStr) -> Option<usize> {
    // SAFETY: loading a system DLL and looking up a name.
    unsafe {
        let m = LoadLibraryA(c"bcrypt.dll".as_ptr().cast());
        if m == 0 {
            return None;
        }
        let f = GetProcAddress(m, name.as_ptr().cast());
        (f != 0).then_some(f)
    }
}

/// BCryptHash with algorithm pseudo-handle `alg` (0x21 MD5, 0x31 SHA-1,
/// 0x41 SHA-256).
fn digest(alg: usize, data: &[u8], out: &mut [u8]) -> bool {
    type HashFn =
        unsafe extern "system" fn(usize, *const u8, u32, *const u8, u32, *mut u8, u32) -> u32;
    let Some(f) = bcrypt(c"BCryptHash") else { return false };
    // SAFETY: BCryptHash's documented signature; our buffers.
    unsafe {
        let f = core::mem::transmute::<usize, HashFn>(f);
        f(
            alg,
            core::ptr::null(),
            0,
            data.as_ptr(),
            data.len() as u32,
            out.as_mut_ptr(),
            out.len() as u32,
        ) == 0
    }
}

/// Copies `data` out as documented: the size with a null buffer,
/// ERROR_MORE_DATA (and the size) when short.
fn give(data: &[u8], out: *mut u8, len: *mut u32) -> i32 {
    if len.is_null() {
        return error(ERROR_INVALID_PARAMETER, 0);
    }
    // SAFETY: the caller's size and buffer.
    unsafe {
        let room = *len as usize;
        *len = data.len() as u32;
        if out.is_null() {
            return 1;
        }
        if room < data.len() {
            return error(ERROR_MORE_DATA, 0);
        }
        core::ptr::copy_nonoverlapping(data.as_ptr(), out, data.len());
    }
    1
}

/// The subject key identifier extension's key id, if there is one.
fn subject_key_id(info: &CertInfo) -> Option<&'static [u8]> {
    let ext =
        CertFindExtension(c"2.5.29.14".as_ptr().cast(), info.extension_count, info.extensions);
    if ext.is_null() {
        return None;
    }
    // SAFETY: an extension of the context.
    let value = unsafe { (*ext).value };
    // SAFETY: the blob points into the context's encoding.
    let bytes = unsafe { core::slice::from_raw_parts(value.data, value.size as usize) };
    Der::new(bytes).expect(OCTET_STRING).ok().map(|t| t.content)
}

#[unsafe(no_mangle)]
pub extern "system" fn CertGetCertificateContextProperty(
    p: *const CertContext,
    id: u32,
    out: *mut u8,
    len: *mut u32,
) -> i32 {
    let Some(b) = block(p) else { return error(E_INVALIDARG, 0) };
    if id == CERT_KEY_PROV_INFO_PROP_ID {
        return get_prov_info(b, out, len);
    }
    if let Some(data) = find_prop(b, id) {
        return give(data, out, len);
    }
    // SAFETY: the context's encoding.
    let encoded =
        unsafe { core::slice::from_raw_parts(b.context.encoded, b.context.encoded_len as usize) };
    let mut h = [0u8; 32];
    let computed: Option<&[u8]> = match id {
        CERT_SHA1_HASH_PROP_ID => digest(0x31, encoded, &mut h[..20]).then_some(&h[..20]),
        CERT_MD5_HASH_PROP_ID => digest(0x21, encoded, &mut h[..16]).then_some(&h[..16]),
        CERT_SHA256_HASH_PROP_ID => digest(0x41, encoded, &mut h).then_some(&h[..]),
        CERT_KEY_IDENTIFIER_PROP_ID => match subject_key_id(&b.info) {
            Some(id) => Some(id),
            None => {
                let key = b.info.public_key.key;
                // SAFETY: the key bits inside the encoding.
                let bits = unsafe { core::slice::from_raw_parts(key.data, key.size as usize) };
                digest(0x31, bits, &mut h[..20]).then_some(&h[..20])
            }
        },
        _ => None,
    };
    match computed {
        Some(data) => {
            let mut copy = [0u8; 32];
            copy[..data.len()].copy_from_slice(data);
            let n = data.len();
            if id != CERT_KEY_IDENTIFIER_PROP_ID {
                add_prop(b, id, &copy[..n]);
            }
            give(&copy[..n], out, len)
        }
        None => error(CRYPT_E_NOT_FOUND, 0),
    }
}

/// CRYPT_KEY_PROV_INFO (x64): container name @0, provider name @8,
/// dwProvType @16, dwFlags @20, cProvParam @24, rgProvParam @32,
/// dwKeySpec @40; 48 bytes. Kept as the 48 bytes followed by the two
/// strings (UTF-16, NUL-terminated); the pointers are rebuilt on output.
/// Provider parameters are not kept.
fn set_prov_info(b: &mut Block, data: *const u8) -> i32 {
    let wide_len = |p: *const u16| {
        let mut n = 0;
        if !p.is_null() {
            // SAFETY: the caller's NUL-terminated string.
            while unsafe { *p.add(n) } != 0 {
                n += 1;
            }
        }
        n
    };
    // SAFETY: the caller's CRYPT_KEY_PROV_INFO.
    let (container, provider) = unsafe {
        (
            data.cast::<*const u16>().read_unaligned(),
            data.add(8).cast::<*const u16>().read_unaligned(),
        )
    };
    let (cn, pn) = (wide_len(container), wide_len(provider));
    let size = 48 + 2 * (cn + 1) + 2 * (pn + 1);
    let mut buf = [0u8; 48 + 2 * 2 * 260];
    if size > buf.len() {
        return error(ERROR_INVALID_PARAMETER, 0);
    }
    // SAFETY: as above; the strings have the lengths measured.
    unsafe {
        core::ptr::copy_nonoverlapping(data, buf.as_mut_ptr(), 48);
        if !container.is_null() {
            core::ptr::copy_nonoverlapping(
                container.cast::<u8>(),
                buf.as_mut_ptr().add(48),
                2 * cn,
            );
        }
        if !provider.is_null() {
            core::ptr::copy_nonoverlapping(
                provider.cast::<u8>(),
                buf.as_mut_ptr().add(48 + 2 * (cn + 1)),
                2 * pn,
            );
        }
    }
    // Remember which strings were null (flags in the pointer slots).
    buf[0..8].copy_from_slice(&u64::from(!container.is_null()).to_le_bytes());
    buf[8..16].copy_from_slice(&u64::from(!provider.is_null()).to_le_bytes());
    buf[24..28].copy_from_slice(&0u32.to_le_bytes());
    buf[32..40].copy_from_slice(&0u64.to_le_bytes());
    // The container string's length, for the output.
    buf[28..32].copy_from_slice(&(cn as u32).to_le_bytes());
    if add_prop(b, CERT_KEY_PROV_INFO_PROP_ID, &buf[..size]) {
        1
    } else {
        error(ERROR_NOT_ENOUGH_MEMORY, 0)
    }
}

fn get_prov_info(b: &Block, out: *mut u8, len: *mut u32) -> i32 {
    let Some(data) = find_prop(b, CERT_KEY_PROV_INFO_PROP_ID) else {
        return error(CRYPT_E_NOT_FOUND, 0);
    };
    if len.is_null() {
        return error(ERROR_INVALID_PARAMETER, 0);
    }
    let mut buf = [0u8; 48 + 2 * 2 * 260];
    buf[..data.len()].copy_from_slice(data);
    let flag = |i: usize| buf[i] != 0;
    let (has_container, has_provider) = (flag(0), flag(8));
    let cn = u32::from_le_bytes([buf[28], buf[29], buf[30], buf[31]]) as usize;
    buf[24..32].copy_from_slice(&[0; 8]);
    // SAFETY: the caller's size and buffer.
    unsafe {
        let room = *len as usize;
        *len = data.len() as u32;
        if out.is_null() {
            return 1;
        }
        if room < data.len() {
            return error(ERROR_MORE_DATA, 0);
        }
        let base = out as u64;
        let container = if has_container { base + 48 } else { 0 };
        let provider = if has_provider { base + 48 + 2 * (cn as u64 + 1) } else { 0 };
        buf[0..8].copy_from_slice(&container.to_le_bytes());
        buf[8..16].copy_from_slice(&provider.to_le_bytes());
        core::ptr::copy_nonoverlapping(buf.as_ptr(), out, data.len());
    }
    1
}

#[unsafe(no_mangle)]
pub extern "system" fn CertSetCertificateContextProperty(
    p: *const CertContext,
    id: u32,
    _flags: u32,
    data: *const c_void,
) -> i32 {
    let Some(b) = block(p) else { return error(E_INVALIDARG, 0) };
    if data.is_null() {
        remove_prop(b, id);
        return 1;
    }
    let data = data.cast::<u8>();
    let ok = match id {
        CERT_KEY_PROV_INFO_PROP_ID => return set_prov_info(b, data),
        // A handle (HCRYPTPROV, NCRYPT_KEY_HANDLE): the value itself.
        CERT_KEY_PROV_HANDLE_PROP_ID
        | CERT_NCRYPT_KEY_HANDLE_PROP_ID
        | CERT_HCRYPTPROV_OR_NCRYPT_KEY_HANDLE_PROP_ID => {
            // SAFETY: the caller's handle value.
            let v = unsafe { data.cast::<u64>().read_unaligned() };
            add_prop(b, id, &v.to_le_bytes())
        }
        CERT_KEY_SPEC_PROP_ID => {
            // SAFETY: the caller's DWORD.
            let v = unsafe { data.cast::<u32>().read_unaligned() };
            add_prop(b, id, &v.to_le_bytes())
        }
        CERT_KEY_CONTEXT_PROP_ID => {
            // SAFETY: CERT_KEY_CONTEXT starts with its cbSize.
            let size = unsafe { data.cast::<u32>().read_unaligned() } as usize;
            if !(4..=64).contains(&size) {
                return error(E_INVALIDARG, 0);
            }
            // SAFETY: `size` bytes of the caller's structure.
            add_prop(b, id, unsafe { core::slice::from_raw_parts(data, size) })
        }
        _ => {
            // CRYPT_DATA_BLOB.
            // SAFETY: the caller's blob.
            let blob = unsafe { data.cast::<Blob>().read_unaligned() };
            let bytes = if blob.size == 0 {
                &[][..]
            } else {
                // SAFETY: the blob's data.
                unsafe { core::slice::from_raw_parts(blob.data, blob.size as usize) }
            };
            add_prop(b, id, bytes)
        }
    };
    if ok { 1 } else { error(ERROR_NOT_ENOUGH_MEMORY, 0) }
}

#[unsafe(no_mangle)]
pub extern "system" fn CertEnumCertificateContextProperties(
    p: *const CertContext,
    prev: u32,
) -> u32 {
    let Some(b) = block(p) else { return 0 };
    // The smallest id above `prev` (ids in increasing order).
    let mut best = 0u32;
    let mut q = b.props;
    while !q.is_null() {
        // SAFETY: our property list.
        let id = unsafe { (*q).id };
        if id > prev && (best == 0 || id < best) {
            best = id;
        }
        // SAFETY: as above.
        q = unsafe { (*q).next };
    }
    best
}

#[unsafe(no_mangle)]
pub extern "system" fn CertFindExtension(
    oid: *const u8,
    count: u32,
    extensions: *const Extension,
) -> *const Extension {
    if oid.is_null() || extensions.is_null() {
        return error(CRYPT_E_NOT_FOUND, core::ptr::null());
    }
    let same = |a: *const u8| {
        let mut i = 0;
        // SAFETY: two NUL-terminated strings.
        unsafe {
            loop {
                let (x, y) = (*a.add(i), *oid.add(i));
                if x != y {
                    return false;
                }
                if x == 0 {
                    return true;
                }
                i += 1;
            }
        }
    };
    for i in 0..count as usize {
        // SAFETY: the caller's array of `count` extensions.
        let e = unsafe { extensions.add(i) };
        // SAFETY: as above.
        if same(unsafe { (*e).oid }) {
            return e;
        }
    }
    error(CRYPT_E_NOT_FOUND, core::ptr::null())
}

/// The RSA public key (modulus, exponent) of a SubjectPublicKeyInfo.
fn rsa_key(info: &PublicKeyInfo) -> Option<(&'static [u8], &'static [u8])> {
    // SAFETY: the OID string of a CERT_PUBLIC_KEY_INFO.
    let text = unsafe { c_text(info.algorithm.oid) };
    if text != b"1.2.840.113549.1.1.1" {
        return None;
    }
    // SAFETY: the key bits of the caller's structure.
    let bits = unsafe { core::slice::from_raw_parts(info.key.data, info.key.size as usize) };
    let seq = Der::new(bits).expect(SEQUENCE).ok()?;
    let mut d = Der::new(seq.content);
    let strip = |s: &'static [u8]| {
        let lead = s.iter().take_while(|&&b| b == 0).count();
        &s[lead.min(s.len().saturating_sub(1))..]
    };
    let n = strip(d.expect(INTEGER).ok()?.content);
    let e = strip(d.expect(INTEGER).ok()?.content);
    Some((n, e))
}

/// The bytes of NUL-terminated C string `p`.
///
/// # Safety
/// `p` is a valid NUL-terminated string that outlives the result.
unsafe fn c_text(p: *const u8) -> &'static [u8] {
    if p.is_null() {
        return &[];
    }
    let mut n = 0;
    // SAFETY: as the caller promises.
    unsafe {
        while *p.add(n) != 0 {
            n += 1;
        }
        core::slice::from_raw_parts(p, n)
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn CertGetPublicKeyLength(_encoding: u32, info: *const PublicKeyInfo) -> u32 {
    if info.is_null() {
        return error(E_INVALIDARG, 0);
    }
    // SAFETY: the caller's structure.
    match rsa_key(unsafe { &*info }) {
        Some((n, _)) => {
            let top = n.first().copied().unwrap_or(0);
            (n.len() as u32 - 1) * 8 + (8 - top.leading_zeros())
        }
        None => error(NTE_BAD_ALGID, 0),
    }
}

/// CryptImportPublicKeyInfoEx2: an RSA public key as a bcrypt key (an
/// RSAPUBLICBLOB imported on bcrypt's RSA provider).
#[unsafe(no_mangle)]
pub extern "system" fn CryptImportPublicKeyInfoEx2(
    _encoding: u32,
    info: *const PublicKeyInfo,
    _flags: u32,
    _aux: *const c_void,
    key: *mut usize,
) -> i32 {
    if info.is_null() || key.is_null() {
        return error(E_INVALIDARG, 0);
    }
    // SAFETY: the caller's structure.
    let Some((n, e)) = rsa_key(unsafe { &*info }) else { return error(NTE_BAD_ALGID, 0) };
    let mut blob = [0u8; 24 + 8 + 2048];
    if e.len() > 8 || n.len() > 2048 {
        return error(NTE_BAD_ALGID, 0);
    }
    let top = n.first().copied().unwrap_or(0);
    let bits = (n.len() as u32 - 1) * 8 + (8 - top.leading_zeros());
    for (i, v) in [0x3141_5352u32, bits, e.len() as u32, n.len() as u32, 0, 0].iter().enumerate() {
        blob[4 * i..4 * i + 4].copy_from_slice(&v.to_le_bytes());
    }
    blob[24..24 + e.len()].copy_from_slice(e);
    blob[24 + e.len()..24 + e.len() + n.len()].copy_from_slice(n);
    let size = 24 + e.len() + n.len();
    type Open = unsafe extern "system" fn(*mut usize, *const u16, *const u16, u32) -> u32;
    type Import =
        unsafe extern "system" fn(usize, usize, *const u16, *mut usize, *const u8, u32, u32) -> u32;
    let (Some(open), Some(import)) =
        (bcrypt(c"BCryptOpenAlgorithmProvider"), bcrypt(c"BCryptImportKeyPair"))
    else {
        return error(NTE_BAD_ALGID, 0);
    };
    let rsa: [u16; 4] = [0x52, 0x53, 0x41, 0];
    let public: [u16; 14] =
        [0x52, 0x53, 0x41, 0x50, 0x55, 0x42, 0x4c, 0x49, 0x43, 0x42, 0x4c, 0x4f, 0x42, 0]; // "RSAPUBLICBLOB"
    // SAFETY: bcrypt's documented signatures; our buffers. The provider
    // stays open (the key needs it; bcrypt keys do not keep a reference).
    unsafe {
        let open = core::mem::transmute::<usize, Open>(open);
        let import = core::mem::transmute::<usize, Import>(import);
        let mut alg = 0;
        if open(&mut alg, rsa.as_ptr(), core::ptr::null(), 0) != 0 {
            return error(NTE_BAD_ALGID, 0);
        }
        let status = import(alg, 0, public.as_ptr(), key, blob.as_ptr(), size as u32, 0);
        if status != 0 {
            return error(status, 0);
        }
    }
    1
}

// ---- Names ----

/// Each attribute of encoded Name `name`: (OID contents, string tag,
/// value bytes), in order.
fn for_each_attribute<'a>(name: &'a [u8], mut f: impl FnMut(&'a [u8], u8, &'a [u8]) -> bool) {
    let Ok(seq) = Der::new(name).expect(SEQUENCE) else { return };
    let mut rdns = Der::new(seq.content);
    while let Ok(set) = rdns.expect(SET) {
        let mut atvs = Der::new(set.content);
        while let Ok(atv) = atvs.expect(SEQUENCE) {
            let mut d = Der::new(atv.content);
            let (Ok(oid), Ok(value)) = (d.expect(OID), d.next()) else { return };
            if !f(oid.content, value.tag, value.content) {
                return;
            }
        }
    }
}

/// Appends a string value (by its ASN.1 type) as UTF-16.
fn push_value(out: &mut WideBuf, tag: u8, v: &[u8]) {
    match tag {
        0x1e => {
            // BMPString: UTF-16 big-endian.
            for c in v.chunks_exact(2) {
                out.push(u16::from_be_bytes([c[0], c[1]]));
            }
        }
        0x0c => {
            // UTF8String.
            let text = core::str::from_utf8(v).unwrap_or("");
            for c in text.encode_utf16() {
                out.push(c);
            }
        }
        _ => {
            // PrintableString, IA5String, T61String...: one byte each.
            for &b in v {
                out.push(u16::from(b));
            }
        }
    }
}

struct WideBuf {
    data: [u16; 512],
    len: usize,
}

impl WideBuf {
    fn new() -> WideBuf {
        WideBuf { data: [0; 512], len: 0 }
    }
    fn push(&mut self, c: u16) {
        if self.len < self.data.len() - 1 {
            self.data[self.len] = c;
            self.len += 1;
        }
    }
    fn push_str(&mut self, s: &str) {
        for c in s.encode_utf16() {
            self.push(c);
        }
    }
}

/// OID contents of the attributes the simple display name looks at.
const CN: &[u8] = &[0x55, 0x04, 0x03];
const OU: &[u8] = &[0x55, 0x04, 0x0b];
const O: &[u8] = &[0x55, 0x04, 0x0a];
const EMAIL: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x09, 0x01];

/// The X.500 key of an attribute OID (CertNameToStr's names).
fn x500_key(oid: &[u8]) -> Option<&'static str> {
    Some(match oid {
        [0x55, 0x04, 0x03] => "CN",
        [0x55, 0x04, 0x06] => "C",
        [0x55, 0x04, 0x07] => "L",
        [0x55, 0x04, 0x08] => "S",
        [0x55, 0x04, 0x09] => "STREET",
        [0x55, 0x04, 0x0a] => "O",
        [0x55, 0x04, 0x0b] => "OU",
        [0x55, 0x04, 0x0c] => "T",
        [0x55, 0x04, 0x2a] => "G",
        [0x55, 0x04, 0x04] => "SN",
        [0x55, 0x04, 0x05] => "SERIALNUMBER",
        [0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x09, 0x01] => "E",
        [0x09, 0x92, 0x26, 0x89, 0x93, 0xf2, 0x2c, 0x64, 0x01, 0x19] => "DC",
        _ => return None,
    })
}

/// The string form of encoded Name `name` (CERT_X500_NAME_STR: "CN=a,
/// O=b", in encoded order or reversed; attributes without a key as their
/// dotted OID).
fn name_to_str(name: &[u8], reverse: bool, out: &mut WideBuf) {
    let mut items: [(&[u8], u8, &[u8]); 32] = [(&[], 0, &[]); 32];
    let mut count = 0;
    for_each_attribute(name, |oid, tag, value| {
        if count < items.len() {
            items[count] = (oid, tag, value);
            count += 1;
        }
        true
    });
    for i in 0..count {
        let (oid, tag, value) = items[if reverse { count - 1 - i } else { i }];
        if i > 0 {
            out.push_str(", ");
        }
        match x500_key(oid) {
            Some(k) => out.push_str(k),
            None => {
                let mut text = [0u8; OID_TEXT];
                if let Ok(n) = oid_text(oid, &mut text) {
                    for &b in &text[..n] {
                        out.push(u16::from(b));
                    }
                }
            }
        }
        out.push(u16::from(b'='));
        push_value(out, tag, value);
    }
}

/// Copies a wide string out as CertGetNameString does: the characters
/// with the NUL, or the size with no buffer; truncated (NUL-terminated)
/// when short.
fn give_wide(s: &WideBuf, out: *mut u16, len: u32) -> u32 {
    let needed = s.len as u32 + 1;
    if out.is_null() || len == 0 {
        return needed;
    }
    let n = (s.len).min(len as usize - 1);
    // SAFETY: the caller's buffer of `len` characters.
    unsafe {
        core::ptr::copy_nonoverlapping(s.data.as_ptr(), out, n);
        *out.add(n) = 0;
    }
    n as u32 + 1
}

const CERT_NAME_EMAIL_TYPE: u32 = 1;
const CERT_NAME_RDN_TYPE: u32 = 2;
const CERT_NAME_ATTR_TYPE: u32 = 3;
const CERT_NAME_SIMPLE_DISPLAY_TYPE: u32 = 4;
const CERT_NAME_FRIENDLY_DISPLAY_TYPE: u32 = 5;
const CERT_NAME_ISSUER_FLAG: u32 = 1;
const CERT_NAME_STR_REVERSE_FLAG: u32 = 0x0200_0000;
const CERT_FRIENDLY_NAME_PROP_ID: u32 = 11;

/// The first value of attribute `oid` in `name`.
fn attribute(name: &[u8], oid: &[u8], out: &mut WideBuf) -> bool {
    let mut found = false;
    for_each_attribute(name, |o, tag, v| {
        if o == oid {
            push_value(out, tag, v);
            found = true;
            return false;
        }
        true
    });
    found
}

#[unsafe(no_mangle)]
pub extern "system" fn CertGetNameStringW(
    p: *const CertContext,
    kind: u32,
    flags: u32,
    kind_para: *const c_void,
    out: *mut u16,
    len: u32,
) -> u32 {
    let mut s = WideBuf::new();
    let Some(b) = block(p) else { return give_wide(&s, out, len) };
    let blob = if flags & CERT_NAME_ISSUER_FLAG != 0 { b.info.issuer } else { b.info.subject };
    // SAFETY: the name inside the context's encoding.
    let name = unsafe { core::slice::from_raw_parts(blob.data, blob.size as usize) };
    match kind {
        CERT_NAME_FRIENDLY_DISPLAY_TYPE | CERT_NAME_SIMPLE_DISPLAY_TYPE => {
            if kind == CERT_NAME_FRIENDLY_DISPLAY_TYPE
                && let Some(f) = find_prop(b, CERT_FRIENDLY_NAME_PROP_ID)
            {
                for c in f.chunks_exact(2) {
                    let c = u16::from_le_bytes([c[0], c[1]]);
                    if c == 0 {
                        break;
                    }
                    s.push(c);
                }
            } else if !(attribute(name, CN, &mut s)
                || attribute(name, OU, &mut s)
                || attribute(name, O, &mut s))
            {
                // The first attribute, or the e-mail address.
                let mut first = true;
                for_each_attribute(name, |_, tag, v| {
                    if first {
                        push_value(&mut s, tag, v);
                        first = false;
                    }
                    false
                });
            }
        }
        CERT_NAME_EMAIL_TYPE => {
            attribute(name, EMAIL, &mut s);
        }
        CERT_NAME_ATTR_TYPE => {
            // pvTypePara: the attribute's OID string.
            // SAFETY: the caller's OID string.
            let want = unsafe { c_text(kind_para.cast()) };
            let mut text = [0u8; OID_TEXT];
            for_each_attribute(name, |o, tag, v| {
                if oid_text(o, &mut text).is_ok_and(|n| &text[..n] == want) {
                    push_value(&mut s, tag, v);
                    return false;
                }
                true
            });
        }
        CERT_NAME_RDN_TYPE => {
            // pvTypePara: a DWORD of CERT_*_NAME_STR flags.
            let str_flags =
                // SAFETY: the caller's DWORD, if any.
                if kind_para.is_null() { 0 } else { unsafe { kind_para.cast::<u32>().read_unaligned() } };
            name_to_str(name, str_flags & CERT_NAME_STR_REVERSE_FLAG != 0, &mut s);
        }
        _ => {
            super::missing_detail(b"crypt32.dll!CertGetNameStringW(type)", u64::from(kind));
        }
    }
    give_wide(&s, out, len)
}

/// CertNameToStrW(dwCertEncodingType, pName, dwStrType, psz, csz).
#[unsafe(no_mangle)]
pub extern "system" fn CertNameToStrW(
    _encoding: u32,
    name: *const Blob,
    str_type: u32,
    out: *mut u16,
    len: u32,
) -> u32 {
    let mut s = WideBuf::new();
    if !name.is_null() {
        // SAFETY: the caller's name blob.
        let bytes = unsafe { core::slice::from_raw_parts((*name).data, (*name).size as usize) };
        name_to_str(bytes, str_type & CERT_NAME_STR_REVERSE_FLAG != 0, &mut s);
    }
    give_wide(&s, out, len)
}

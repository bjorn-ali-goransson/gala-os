//! CryptDecodeObjectEx's and CryptDecodeObject's structure types beyond
//! the alternative names (`query`): the Authenticode ones Windows'
//! wintrust registers with crypt32 (SPC_INDIRECT_DATA_CONTENT,
//! SPC_SP_OPUS_INFO), a SignerInfo as CMSG_SIGNER_INFO (PKCS7_SIGNER_INFO),
//! a time as FILETIME (PKCS_UTC_TIME, X509_CHOICE_OF_TIME, the PKCS #9
//! signingTime OID) and an OCTET STRING (X509_OCTET_STRING); and
//! CryptVerifyTimeStampSignature, which checks an RFC 3161 timestamp token
//! (a SignedData of a TSTInfo) against the data it stamps.
//!
//! Each decoder builds its documented structure with `msg::Out` (sized
//! first, then written, pointers into the same block). Types not here are
//! recorded (`crypt32.dll!CryptDecodeObjectEx(type)`, the type's number
//! or, for an OID string, 0) and fail with CRYPT_E_ASN1_BADTAG; an SPC_LINK
//! moniker is recorded and left out.

#![allow(clippy::not_unsafe_ptr_arg_deref)]

use core::ffi::c_void;

use crate::cert::{
    Algorithm, BOOLEAN, Blob, CertContext, Der, Extension, INTEGER, OCTET_STRING, OID, SEQUENCE,
    Tlv, algorithm_id, digest_algorithm, hash_with,
};
use crate::msg::{Out, build_signer_info, give_built, parse_signer, put};
use crate::store::{error, unsupported};

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GetProcessHeap() -> usize;
    fn HeapAlloc(heap: usize, flags: u32, size: usize) -> *mut u8;
    fn LocalAlloc(flags: u32, size: usize) -> *mut u8;
}

const E_INVALIDARG: u32 = 0x8007_0057;
const ERROR_NOT_ENOUGH_MEMORY: u32 = 8;
const NTE_BAD_ALGID: u32 = 0x8009_0008;
const CRYPT_E_ASN1_BADTAG: u32 = 0x8009_310b;
const CRYPT_E_HASH_VALUE: u32 = 0x8009_1007;
const CRYPT_E_SIGNER_NOT_FOUND: u32 = 0x8009_100e;
const CRYPT_E_INVALID_MSG_TYPE: u32 = 0x8009_1004;
const CRYPT_DECODE_ALLOC_FLAG: u32 = 0x8000;

/// The structure types decoded here.
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    AltName,
    IndirectData,
    OpusInfo,
    SignerInfo,
    UtcTime,
    ChoiceOfTime,
    OctetString,
}

/// A structure type (an LPCSTR: a small number or an OID string).
fn kind(t: *const u8) -> Option<Kind> {
    let n = t as usize;
    if n < 0x10000 {
        return match n {
            12 => Some(Kind::AltName),        // X509_ALTERNATE_NAME
            17 => Some(Kind::UtcTime),        // PKCS_UTC_TIME
            25 => Some(Kind::OctetString),    // X509_OCTET_STRING
            30 => Some(Kind::ChoiceOfTime),   // X509_CHOICE_OF_TIME
            500 => Some(Kind::SignerInfo),    // PKCS7_SIGNER_INFO
            2003 => Some(Kind::IndirectData), // SPC_INDIRECT_DATA_CONTENT_STRUCT
            2007 => Some(Kind::OpusInfo),     // SPC_SP_OPUS_INFO_STRUCT
            _ => None,
        };
    }
    let mut i = 0;
    // SAFETY: the caller's NUL-terminated OID string.
    let s = unsafe {
        while *t.add(i) != 0 && i < 64 {
            i += 1;
        }
        core::slice::from_raw_parts(t, i)
    };
    match s {
        b"2.5.29.17" | b"2.5.29.18" | b"2.5.29.7" => Some(Kind::AltName),
        b"1.3.6.1.4.1.311.2.1.4" => Some(Kind::IndirectData),
        b"1.3.6.1.4.1.311.2.1.12" => Some(Kind::OpusInfo),
        b"1.2.840.113549.1.9.5" => Some(Kind::UtcTime), // szOID_RSA_signingTime
        _ => None,
    }
}

/// SPC_ATTRIBUTE_TYPE_AND_OPTIONAL_VALUE / SPC_INDIRECT_DATA_CONTENT.
#[repr(C)]
struct IndirectData {
    data_oid: *mut u8,
    data_value: Blob,
    digest_algorithm: Algorithm,
    digest: Blob,
}

/// SPC_LINK: the choice, then a string pointer (URL, file) or an
/// SPC_SERIALIZED_OBJECT (moniker: a class id and a blob), the union's
/// largest member.
#[repr(C)]
struct SpcLink {
    choice: u32,
    value: LinkValue,
}

#[repr(C)]
union LinkValue {
    text: *mut u16,
    moniker: Moniker,
}

/// SPC_SERIALIZED_OBJECT.
#[repr(C)]
#[derive(Clone, Copy)]
struct Moniker {
    class_id: [u8; 16],
    serialized: Blob,
}

/// SPC_SP_OPUS_INFO.
#[repr(C)]
struct OpusInfo {
    program_name: *mut u16,
    more_info: *mut SpcLink,
    publisher_info: *mut SpcLink,
}

crate::layout::check!(core::mem::size_of::<IndirectData>(), 32, 64);
crate::layout::check!(core::mem::size_of::<OpusInfo>(), 12, 24);
crate::layout::check!(core::mem::size_of::<SpcLink>(), 28, 40);
crate::layout::check!(core::mem::offset_of!(SpcLink, value), 4, 8);

/// A string of `bytes` as NUL-terminated UTF-16: BMPString's big-endian
/// pairs (`bmp`), else one byte per character.
fn wide(w: &mut Out, bytes: &[u8], bmp: bool) -> *mut u16 {
    let n = if bmp { bytes.len() / 2 } else { bytes.len() };
    let p: *mut u16 = w.place((n + 1) * 2, 2).cast();
    if !p.is_null() {
        for i in 0..n {
            let c = if bmp {
                u16::from_be_bytes([bytes[2 * i], bytes[2 * i + 1]])
            } else {
                u16::from(bytes[i])
            };
            // SAFETY: room for n + 1 characters at `p`.
            unsafe { p.add(i).write_unaligned(c) };
        }
        // SAFETY: as above.
        unsafe { p.add(n).write_unaligned(0) };
    }
    p
}

/// An SpcString (CHOICE { unicode [0] IMPLICIT BMPString, ascii [1]
/// IMPLICIT IA5String }).
fn spc_string(w: &mut Out, t: Tlv<'_>) -> Result<*mut u16, u32> {
    match t.tag {
        0x80 => Ok(wide(w, t.content, true)),
        0x81 => Ok(wide(w, t.content, false)),
        _ => Err(CRYPT_E_ASN1_BADTAG),
    }
}

/// An SpcLink (CHOICE { url [0] IMPLICIT IA5String, moniker [1], file [2]
/// EXPLICIT SpcString }).
fn spc_link(w: &mut Out, t: Tlv<'_>) -> Result<*mut SpcLink, u32> {
    let p = w.slot::<SpcLink>();
    let (choice, text) = match t.tag {
        0x80 => (1, wide(w, t.content, false)),
        0xa2 => (3, spc_string(w, Der::new(t.content).next()?)?),
        0xa1 => {
            unsupported(b"crypt32.dll!CryptDecodeObjectEx(SPC_LINK moniker)", 0);
            (2, core::ptr::null_mut())
        }
        _ => return Err(CRYPT_E_ASN1_BADTAG),
    };
    let empty =
        Moniker { class_id: [0; 16], serialized: Blob { size: 0, data: core::ptr::null_mut() } };
    let mut link = SpcLink { choice, value: LinkValue { moniker: empty } };
    link.value.text = text;
    put(p, link);
    Ok(p)
}

/// A time (UTCTime, or with `choice` GeneralizedTime too) as a FILETIME,
/// fractions of a second included.
pub(crate) fn filetime(t: Tlv<'_>, choice: bool) -> Result<u64, u32> {
    if t.tag != 0x17 && !(choice && t.tag == 0x18) {
        return Err(CRYPT_E_ASN1_BADTAG);
    }
    let whole = crate::cert::time(t)?;
    // GeneralizedTime's fraction: YYYYMMDDHHMMSS.fff...Z
    let mut frac = 0u64;
    if t.tag == 0x18 && t.content.get(14) == Some(&b'.') {
        let mut scale = 1_000_000u64; // 100 ns units in 0.1 s
        for &c in t.content[15..].iter().take_while(|c| c.is_ascii_digit()) {
            frac += u64::from(c - b'0') * scale;
            scale /= 10;
        }
    }
    Ok(whole + frac)
}

fn build(w: &mut Out, k: Kind, der: &[u8]) -> Result<(), u32> {
    let top = Der::new(der).next()?;
    match k {
        Kind::AltName => unreachable!("alternative names are query.rs's"),
        Kind::IndirectData => {
            if top.tag != SEQUENCE {
                return Err(CRYPT_E_ASN1_BADTAG);
            }
            let p = w.slot::<IndirectData>();
            let mut d = Der::new(top.content);
            let data = d.expect(SEQUENCE)?;
            let mut x = Der::new(data.content);
            let data_oid = w.oid(x.expect(OID)?.content)?;
            let data_value = w.blob(x.data);
            let info = d.expect(SEQUENCE)?;
            let mut y = Der::new(info.content);
            let digest_algorithm = w.algorithm(algorithm_id(y.next()?)?)?;
            let digest = w.blob(y.expect(OCTET_STRING)?.content);
            put(p, IndirectData { data_oid, data_value, digest_algorithm, digest });
        }
        Kind::OpusInfo => {
            if top.tag != SEQUENCE {
                return Err(CRYPT_E_ASN1_BADTAG);
            }
            let p = w.slot::<OpusInfo>();
            let mut o = OpusInfo {
                program_name: core::ptr::null_mut(),
                more_info: core::ptr::null_mut(),
                publisher_info: core::ptr::null_mut(),
            };
            let mut d = Der::new(top.content);
            while d.peek_tag().is_some() {
                let f = d.next()?;
                let inner = Der::new(f.content).next()?;
                match f.tag {
                    0xa0 => o.program_name = spc_string(w, inner)?,
                    0xa1 => o.more_info = spc_link(w, inner)?,
                    0xa2 => o.publisher_info = spc_link(w, inner)?,
                    _ => return Err(CRYPT_E_ASN1_BADTAG),
                }
            }
            put(p, o);
        }
        Kind::SignerInfo => build_signer_info(w, &parse_signer(top)?)?,
        Kind::UtcTime | Kind::ChoiceOfTime => {
            let p = w.slot::<[u32; 2]>();
            let t = filetime(top, k == Kind::ChoiceOfTime)?;
            put(p, [t as u32, (t >> 32) as u32]);
        }
        Kind::OctetString => {
            if top.tag != OCTET_STRING {
                return Err(CRYPT_E_ASN1_BADTAG);
            }
            let p = w.slot::<Blob>();
            let b = w.blob(top.content);
            put(p, b);
        }
    }
    Ok(())
}

/// CRYPT_DECODE_PARA: its size and an allocator.
#[repr(C)]
struct DecodePara {
    size: u32,
    alloc: Option<extern "system" fn(usize) -> *mut u8>,
    free: Option<extern "system" fn(*mut u8)>,
}

/// CryptDecodeObjectEx for the types here; None for the others (the
/// caller records them).
pub(crate) fn decode_ex(
    struct_type: *const u8,
    der: &[u8],
    flags: u32,
    para: *const c_void,
    out: *mut c_void,
    out_len: *mut u32,
) -> Option<i32> {
    let k = kind(struct_type)?;
    if k == Kind::AltName {
        return None;
    }
    if flags & CRYPT_DECODE_ALLOC_FLAG == 0 {
        return Some(give_built(out, out_len, |w| build(w, k, der)));
    }
    if out.is_null() {
        return Some(error(E_INVALIDARG, 0));
    }
    let mut m = Out::measure();
    if let Err(e) = build(&mut m, k, der) {
        return Some(error(e, 0));
    }
    let size = m.at;
    // SAFETY: the caller's CRYPT_DECODE_PARA, if any.
    let alloc = unsafe { (para as *const DecodePara).as_ref() }.and_then(|p| p.alloc);
    let p = match alloc {
        Some(a) => a(size.max(1)),
        // SAFETY: plain call (LMEM_FIXED).
        None => unsafe { LocalAlloc(0, size.max(1)) },
    };
    if p.is_null() {
        return Some(error(ERROR_NOT_ENOUGH_MEMORY, 0));
    }
    let mut len = size as u32;
    let r = give_built(p.cast(), &mut len, |w| build(w, k, der));
    // SAFETY: the caller's output pointer and size.
    unsafe {
        *(out as *mut *mut u8) = p;
        if !out_len.is_null() {
            *out_len = size as u32;
        }
    }
    Some(r)
}

pub(crate) fn is_alt_name(t: *const u8) -> bool {
    kind(t) == Some(Kind::AltName)
}

/// CryptDecodeObject: CryptDecodeObjectEx without its allocation flags
/// and decode parameters.
#[unsafe(no_mangle)]
pub extern "system" fn CryptDecodeObject(
    encoding: u32,
    struct_type: *const u8,
    data: *const u8,
    len: u32,
    flags: u32,
    out: *mut c_void,
    out_len: *mut u32,
) -> i32 {
    crate::query::CryptDecodeObjectEx(
        encoding,
        struct_type,
        data,
        len,
        flags & !CRYPT_DECODE_ALLOC_FLAG,
        core::ptr::null(),
        out,
        out_len,
    )
}

// ---- RFC 3161 timestamps ----

/// CRYPT_TIMESTAMP_INFO.
#[repr(C)]
struct TimestampInfo {
    version: u32,
    policy: *mut u8,
    hash_algorithm: Algorithm,
    hashed_message: Blob,
    serial: Blob,
    time: [u32; 2],
    accuracy: *mut [u32; 3],
    ordering: i32,
    nonce: Blob,
    tsa: Blob,
    extension_count: u32,
    extensions: *mut Extension,
}

/// CRYPT_TIMESTAMP_CONTEXT.
#[repr(C)]
struct TimestampContext {
    encoded_len: u32,
    encoded: *mut u8,
    info: *mut TimestampInfo,
}

crate::layout::check!(core::mem::size_of::<TimestampInfo>(), 76, 144);
crate::layout::check!(core::mem::offset_of!(TimestampInfo, time), 36, 72);
crate::layout::check!(core::mem::offset_of!(TimestampInfo, accuracy), 44, 80);
crate::layout::check!(core::mem::offset_of!(TimestampInfo, extensions), 72, 136);
crate::layout::check!(core::mem::size_of::<TimestampContext>(), 12, 24);

/// TSTInfo's fields (RFC 3161 section 2.4.2).
struct TstInfo<'a> {
    version: u32,
    policy: &'a [u8],
    imprint_alg: (&'a [u8], &'a [u8]),
    imprint: &'a [u8],
    serial: &'a [u8],
    time: u64,
    accuracy: Option<[u32; 3]>,
    ordering: bool,
    nonce: &'a [u8],
    tsa: &'a [u8],
}

fn small_int(t: Tlv<'_>) -> u32 {
    t.content.iter().fold(0u32, |a, &b| a.wrapping_shl(8) | u32::from(b))
}

fn parse_tst(der: &[u8]) -> Result<TstInfo<'_>, u32> {
    let top = Der::new(der).expect(SEQUENCE)?;
    let mut d = Der::new(top.content);
    let version = small_int(d.expect(INTEGER)?);
    let policy = d.expect(OID)?.content;
    let imprint = d.expect(SEQUENCE)?;
    let mut i = Der::new(imprint.content);
    let imprint_alg = algorithm_id(i.next()?)?;
    let hashed = i.expect(OCTET_STRING)?.content;
    let serial = d.expect(INTEGER)?.content;
    let time = filetime(d.next()?, true)?;
    let mut t = TstInfo {
        version,
        policy,
        imprint_alg,
        imprint: hashed,
        serial,
        time,
        accuracy: None,
        ordering: false,
        nonce: &[],
        tsa: &[],
    };
    while let Some(tag) = d.peek_tag() {
        let f = d.next()?;
        match tag {
            SEQUENCE => {
                let mut a = [0u32; 3];
                let mut x = Der::new(f.content);
                while x.peek_tag().is_some() {
                    let v = x.next()?;
                    match v.tag {
                        INTEGER => a[0] = small_int(v),
                        0x80 => a[1] = small_int(v),
                        0x81 => a[2] = small_int(v),
                        _ => return Err(CRYPT_E_ASN1_BADTAG),
                    }
                }
                t.accuracy = Some(a);
            }
            BOOLEAN => t.ordering = crate::msg::boolean(f)?,
            INTEGER => t.nonce = f.content,
            0xa0 => t.tsa = f.whole,
            0xa1 => unsupported(b"crypt32.dll!CryptVerifyTimeStampSignature(extensions)", 0),
            _ => return Err(CRYPT_E_ASN1_BADTAG),
        }
    }
    Ok(t)
}

const TST_INFO: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x09, 0x10, 0x01, 0x04];

/// CryptVerifyTimeStampSignature(token, len, data, len, additional,
/// context, signer, store): `token` is an RFC 3161 timestamp token (a
/// ContentInfo of a SignedData whose content is a TSTInfo) stamping
/// `data`: the message imprint must be `data`'s hash (CRYPT_E_HASH_VALUE),
/// and the signer, found by its issuer and serial number (or key
/// identifier) among the token's certificates and `additional`, must have
/// signed it (CryptMsgControl's checks). Gives the CRYPT_TIMESTAMP_CONTEXT
/// (one CryptMemAlloc block: free it with CryptMemFree), the signer's
/// certificate and a store of the token's certificates. The signer's chain
/// is the caller's to check.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn CryptVerifyTimeStampSignature(
    token: *const u8,
    token_len: u32,
    data: *const u8,
    data_len: u32,
    additional: *mut c_void,
    context: *mut *mut c_void,
    signer_out: *mut *const CertContext,
    store_out: *mut *mut c_void,
) -> i32 {
    for p in [context.cast::<usize>(), signer_out.cast(), store_out.cast()] {
        if !p.is_null() {
            // SAFETY: the caller's output.
            unsafe { p.write(0) };
        }
    }
    if token.is_null() || (data.is_null() && data_len != 0) {
        return error(E_INVALIDARG, 0);
    }
    // SAFETY: the caller's buffers.
    let (der, stamped) = unsafe {
        (
            core::slice::from_raw_parts(token, token_len as usize),
            core::slice::from_raw_parts(data, data_len as usize),
        )
    };
    let h = crate::msg::decode(der);
    if h.is_null() {
        return 0; // its error is set
    }
    let r = verify_timestamp(h, der, stamped, additional, context, signer_out, store_out);
    crate::msg::CryptMsgClose(h);
    match r {
        Ok(()) => 1,
        Err(e) => error(e, 0),
    }
}

#[allow(clippy::too_many_arguments)]
fn verify_timestamp(
    h: *mut c_void,
    der: &[u8],
    stamped: &[u8],
    additional: *mut c_void,
    context: *mut *mut c_void,
    signer_out: *mut *const CertContext,
    store_out: *mut *mut c_void,
) -> Result<(), u32> {
    let s = crate::msg::signed_of(h)?;
    if s.content_type != TST_INFO {
        return Err(CRYPT_E_INVALID_MSG_TYPE);
    }
    let tst = parse_tst(s.content_param())?;
    let Some((alg, len, _)) = digest_algorithm(tst.imprint_alg.0) else {
        unsupported(b"crypt32.dll!CryptVerifyTimeStampSignature(imprint algorithm)", 0);
        return Err(NTE_BAD_ALGID);
    };
    let mut digest = [0u8; 64];
    hash_with(alg, stamped, &mut digest[..len]).map_err(|_| NTE_BAD_ALGID)?;
    if tst.imprint != &digest[..len] {
        return Err(CRYPT_E_HASH_VALUE);
    }
    let signer = parse_signer(crate::msg::nth(s.signers, 0)?)?;
    // The token's certificates, with `additional` searched too.
    let store = crate::store::CertOpenStore(core::ptr::without_provenance(1), 0x1_0001, 0, 0, h);
    if store.is_null() {
        return Err(ERROR_NOT_ENOUGH_MEMORY);
    }
    let mut cert = crate::msg::signer_certificate(store, &signer);
    if cert.is_null() && !additional.is_null() {
        cert = crate::msg::signer_certificate(additional, &signer);
    }
    let close = |c: *const CertContext| {
        if !c.is_null() {
            crate::cert::CertFreeCertificateContext(c);
        }
        crate::store::CertCloseStore(store, 0);
    };
    if cert.is_null() {
        close(cert);
        return Err(CRYPT_E_SIGNER_NOT_FOUND);
    }
    // SAFETY: a context of ours.
    let key = unsafe { &raw const (*(*cert).info).public_key };
    if let Err(e) = crate::msg::verify_signer(s.digested(), &signer, key) {
        close(cert);
        return Err(e);
    }
    // The context: CRYPT_TIMESTAMP_CONTEXT, its CRYPT_TIMESTAMP_INFO, then
    // what they point to, in one block.
    let fill = |w: &mut Out| -> Result<(), u32> {
        let c = w.slot::<TimestampContext>();
        let i = w.slot::<TimestampInfo>();
        let encoded = w.bytes(der);
        let policy = w.oid(tst.policy)?;
        let hash_algorithm = w.algorithm(tst.imprint_alg)?;
        let hashed_message = w.blob(tst.imprint);
        let serial = w.reversed(tst.serial);
        let accuracy = match tst.accuracy {
            Some(a) => {
                let p = w.slot::<[u32; 3]>();
                put(p, a);
                p
            }
            None => core::ptr::null_mut(),
        };
        let nonce = w.reversed(tst.nonce);
        let tsa = w.blob(tst.tsa);
        put(
            i,
            TimestampInfo {
                version: tst.version,
                policy,
                hash_algorithm,
                hashed_message,
                serial,
                time: [tst.time as u32, (tst.time >> 32) as u32],
                accuracy,
                ordering: i32::from(tst.ordering),
                nonce,
                tsa,
                extension_count: 0,
                extensions: core::ptr::null_mut(),
            },
        );
        put(c, TimestampContext { encoded_len: der.len() as u32, encoded, info: i });
        Ok(())
    };
    if !context.is_null() {
        let mut m = Out::measure();
        fill(&mut m)?;
        // SAFETY: plain allocation (CryptMemAlloc's heap).
        let p = unsafe { HeapAlloc(GetProcessHeap(), 0, m.at) };
        if p.is_null() {
            close(cert);
            return Err(ERROR_NOT_ENOUGH_MEMORY);
        }
        let mut n = m.at as u32;
        give_built(p.cast(), &mut n, fill);
        // SAFETY: the caller's output.
        unsafe { context.write(p.cast()) };
    }
    if signer_out.is_null() {
        crate::cert::CertFreeCertificateContext(cert);
    } else {
        // SAFETY: the caller's output.
        unsafe { signer_out.write(cert) };
    }
    if store_out.is_null() {
        crate::store::CertCloseStore(store, 0);
    } else {
        // SAFETY: the caller's output.
        unsafe { store_out.write(store) };
    }
    Ok(())
}

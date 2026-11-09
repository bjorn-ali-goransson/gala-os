//! CRLs and revocation, as Microsoft documents them (CertCreateCRLContext,
//! CRL_CONTEXT, CRL_INFO, CRL_ENTRY, CertFindCertificateInCRL,
//! CertVerifyCRLRevocation, CertVerifyCRLTimeValidity, CertVerifyRevocation
//! with CERT_REVOCATION_PARA and CERT_REVOCATION_STATUS), with RFC 5280
//! (5. CRL profile, 6.3 CRL validation) for what a CRL says. Tests:
//! `cert_test` (crl.rs, chain.rs), `schannel_test`, `curl_schannel`.
//!
//! - **Contexts**: CertCreateCRLContext decodes a DER CRL (v1 or v2) into
//!   CRL_CONTEXT and CRL_INFO in one heap block with a copy of the
//!   encoding (blobs point into it), the entries (serial numbers
//!   little-endian, as CERT_INFO's), their extensions and the CRL's, and
//!   the dotted OID strings. Reference-counted like certificate contexts,
//!   and like them a context a caller holds holds its store (store.rs).
//! - **Revocation status** (`check`, the chain engine's and
//!   CertVerifyRevocation's): **offline** (a decision: crypt32 does not
//!   fetch; ROADMAP TLS): the CRLs of the certificate's issuer in the
//!   stores given (hAdditionalStore, the engine's stores; gala's CA store
//!   holds those in System32\gala\ca) whose signature verifies with the
//!   issuer's key (an issuer whose key usage lacks cRLSign signs none),
//!   that are current (thisUpdate <= now <= nextUpdate; the current time,
//!   not the chain's pTime, as documented) and whose scope covers the
//!   certificate (a delta CRL, or an issuing distribution point limited
//!   to other kinds of certificate, to some reasons or to an indirect CRL,
//!   is not used, recorded). One that lists the serial number: revoked
//!   (CRL_REASON_* from the entry's reason code). A current one that does
//!   not: good. Only out-of-date ones: offline. None: offline (the CRL is
//!   not fetched, recorded). Only a certificate naming a CRL distribution
//!   point is checked by CRL, as Windows' default provider does (Wine's
//!   tests): one naming only an OCSP responder is offline (OCSP is not
//!   asked, recorded), one naming neither has no check
//!   (CRYPT_E_NO_REVOCATION_CHECK) whatever the stores hold.
//! - CertVerifyRevocation is the default revocation provider over that:
//!   the issuer is pIssuerCert, the next context with
//!   CERT_VERIFY_REV_CHAIN_FLAG, else a certificate of the stores given
//!   whose key signed it; CRLs from hCrlStore, rgCertStore and the CA
//!   store. Failures are recorded (`crypt32.dll!CertVerifyRevocation:
//!   CRYPT_E_...`, the context's index as detail).

#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use core::ffi::c_void;

use crate::layout::{RevocationPara, RevocationStatus, get, has};
use core::sync::atomic::{AtomicU32, Ordering};

use crate::cert::{
    Algorithm, BIT_STRING, BOOLEAN, Blob, CertContext, CertFindExtension, CertInfo,
    CryptVerifyCertificateSignatureEx, Der, Extension, INTEGER, OCTET_STRING, OID, SEQUENCE, Tlv,
    algorithm_id, extension_value, oid_text, time,
};
use crate::store::{CertCloseStore, CertEnumCRLsInStore, CertFindCertificateInStore, unsupported};

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GetProcessHeap() -> usize;
    fn HeapAlloc(heap: usize, flags: u32, size: usize) -> *mut u8;
    fn HeapFree(heap: usize, flags: u32, p: *mut u8) -> i32;
    fn SetLastError(code: u32);
    fn GetSystemTimeAsFileTime(time: *mut u64);
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "ntdll", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "ntdll", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GalaRecordMissing(name: *const u8, length: usize, what: u32, detail: u64) -> u32;
}

const X509_ASN_ENCODING: u32 = 1;
const E_INVALIDARG: u32 = 0x8007_0057;
const ERROR_NOT_ENOUGH_MEMORY: u32 = 8;
const CRYPT_E_ASN1_CORRUPT: u32 = 0x8009_3100;
pub(crate) const CRYPT_E_REVOKED: u32 = 0x8009_2010;
const CRYPT_E_NO_REVOCATION_DLL: u32 = 0x8009_2011;
pub(crate) const CRYPT_E_NO_REVOCATION_CHECK: u32 = 0x8009_2012;
pub(crate) const CRYPT_E_REVOCATION_OFFLINE: u32 = 0x8009_2013;

fn error<T>(code: u32, value: T) -> T {
    // SAFETY: plain call.
    unsafe { SetLastError(code) };
    value
}

/// A ledger line of kind "other" (5): what a revocation decision rested
/// on, so a refused (or passed) certificate says why.
pub(crate) fn note(what: &[u8], detail: u64) {
    // SAFETY: a valid buffer.
    unsafe { GalaRecordMissing(what.as_ptr(), what.len(), 5, detail) };
}

/// The current time as a FILETIME.
pub(crate) fn now() -> u64 {
    let mut t = 0u64;
    // SAFETY: our output.
    unsafe { GetSystemTimeAsFileTime(&mut t) };
    t
}

// ---- The structures (wincrypt.h; sizes checked in `layout`) ----

/// CRL_ENTRY (24 bytes on x86, 40 on x64).
#[repr(C)]
pub struct CrlEntry {
    pub serial: Blob,
    pub revoked_at: u64,
    pub extension_count: u32,
    pub extensions: *mut Extension,
}

/// CRL_INFO (56 or 96 bytes).
#[repr(C)]
pub struct CrlInfo {
    pub version: u32,
    pub signature: Algorithm,
    pub issuer: Blob,
    pub this_update: u64,
    /// Zero when the CRL has none.
    pub next_update: u64,
    pub entry_count: u32,
    pub entries: *mut CrlEntry,
    pub extension_count: u32,
    pub extensions: *mut Extension,
}

/// CRL_CONTEXT (20 or 40 bytes).
#[repr(C)]
pub struct CrlContext {
    pub encoding: u32,
    pub encoded: *mut u8,
    pub encoded_len: u32,
    pub info: *mut CrlInfo,
    pub store: *mut c_void,
}

const CRL_MAGIC: u32 = 0x6761_6c72; // "galr"

/// `CrlBlock::refs`' flag: the context is in its store's list (as
/// cert.rs's `OWNED`: while a caller also holds it, it holds the store).
const OWNED: u32 = 0x8000_0000;

/// The heap block of a CRL context: this header, then the entries, the
/// extensions, the serial numbers (reversed), the OID strings and the
/// encoding.
#[repr(C)]
struct CrlBlock {
    magic: u32,
    refs: AtomicU32,
    context: CrlContext,
    info: CrlInfo,
}

fn block<'a>(p: *const CrlContext) -> Option<&'a mut CrlBlock> {
    if p.is_null() {
        return None;
    }
    let offset = core::mem::offset_of!(CrlBlock, context);
    let b = (p as usize).checked_sub(offset)? as *mut CrlBlock;
    // SAFETY: a context the caller got from us starts `offset` bytes into
    // its block, whose magic we check.
    let b = unsafe { &mut *b };
    (b.magic == CRL_MAGIC).then_some(b)
}

/// Whether `p` is a CRL context of ours.
pub(crate) fn is_crl(p: *const CrlContext) -> bool {
    block(p).is_some()
}

// ---- Decoding ----

/// What one pass over the encoding finds.
struct Parsed<'a> {
    version: u32,
    signature: (&'a [u8], &'a [u8]),
    issuer: &'a [u8],
    this_update: u64,
    next_update: u64,
    /// revokedCertificates' contents (SEQUENCE OF), maybe empty.
    entries: &'a [u8],
    /// crlExtensions' contents (SEQUENCE OF), maybe empty.
    extensions: &'a [u8],
}

fn parse(crl: &[u8]) -> Result<(Parsed<'_>, usize), u32> {
    let whole = Der::new(crl).expect(SEQUENCE)?;
    let mut c = Der::new(whole.content);
    let tbs = c.expect(SEQUENCE)?;
    let mut t = Der::new(tbs.content);
    let mut version = 0;
    if t.peek_tag() == Some(INTEGER) {
        version = u32::from(*t.next()?.content.last().ok_or(CRYPT_E_ASN1_CORRUPT)?);
    }
    let signature = algorithm_id(t.next()?)?;
    let issuer = t.expect(SEQUENCE)?.whole;
    let this_update = time(t.next()?)?;
    let mut next_update = 0;
    if matches!(t.peek_tag(), Some(0x17 | 0x18)) {
        next_update = time(t.next()?)?;
    }
    let mut entries: &[u8] = &[];
    if t.peek_tag() == Some(SEQUENCE) {
        entries = t.next()?.content;
    }
    let mut extensions: &[u8] = &[];
    if t.peek_tag() == Some(0xa0) {
        extensions = Der::new(t.next()?.content).expect(SEQUENCE)?.content;
    }
    if t.peek_tag().is_some() {
        return Err(CRYPT_E_ASN1_CORRUPT);
    }
    algorithm_id(c.next()?)?;
    c.expect(BIT_STRING)?;
    let parsed =
        Parsed { version, signature, issuer, this_update, next_update, entries, extensions };
    Ok((parsed, whole.whole.len()))
}

/// The number of extensions in `list` (a SEQUENCE OF's contents), checked.
fn count_extensions(list: &[u8]) -> Result<usize, u32> {
    let mut d = Der::new(list);
    let mut n = 0;
    while d.peek_tag().is_some() {
        let e = d.expect(SEQUENCE)?;
        let mut x = Der::new(e.content);
        x.expect(OID)?;
        if x.peek_tag() == Some(BOOLEAN) {
            x.next()?;
        }
        x.expect(OCTET_STRING)?;
        n += 1;
    }
    Ok(n)
}

/// One entry: (serial INTEGER contents, revocationDate, extensions'
/// contents).
fn entry(t: Tlv<'_>) -> Result<(&[u8], u64, &[u8]), u32> {
    if t.tag != SEQUENCE {
        return Err(CRYPT_E_ASN1_CORRUPT);
    }
    let mut d = Der::new(t.content);
    let serial = d.expect(INTEGER)?.content;
    let at = time(d.next()?)?;
    let mut ext: &[u8] = &[];
    if d.peek_tag() == Some(SEQUENCE) {
        ext = d.next()?.content;
    }
    Ok((serial, at, ext))
}

/// Room for one OID string.
const OID_TEXT: usize = 64;

/// CertCreateCRLContext: the DER CRL decoded into a new context (module
/// comment); E_INVALIDARG without X509_ASN_ENCODING, CRYPT_E_ASN1_* for
/// what does not decode.
#[unsafe(no_mangle)]
pub extern "system" fn CertCreateCRLContext(
    encoding: u32,
    data: *const u8,
    len: u32,
) -> *const CrlContext {
    if encoding & X509_ASN_ENCODING == 0 {
        return error(E_INVALIDARG, core::ptr::null());
    }
    // SAFETY: the caller's buffer of `len` bytes (none: nothing to decode,
    // CRYPT_E_ASN1_EOD as Wine's tests record).
    let input = if data.is_null() {
        &[][..]
    } else {
        unsafe { core::slice::from_raw_parts(data, len as usize) }
    };
    match create(encoding, input) {
        Ok(p) => p,
        Err(e) => error(e, core::ptr::null()),
    }
}

fn create(encoding: u32, input: &[u8]) -> Result<*const CrlContext, u32> {
    let (parsed, used) = parse(input)?;
    // Count what the block holds.
    let mut entry_count = 0usize;
    let mut serial_bytes = 0usize;
    let mut ext_count = count_extensions(parsed.extensions)?;
    let mut d = Der::new(parsed.entries);
    while d.peek_tag().is_some() {
        let (serial, _, ext) = entry(d.next()?)?;
        entry_count += 1;
        serial_bytes += serial.len();
        ext_count += count_extensions(ext)?;
    }
    let header = core::mem::size_of::<CrlBlock>();
    let entry_bytes = entry_count * core::mem::size_of::<CrlEntry>();
    let ext_bytes = ext_count * core::mem::size_of::<Extension>();
    let oids = (1 + ext_count) * OID_TEXT;
    let total = header + entry_bytes + ext_bytes + serial_bytes + oids + used;
    // SAFETY: plain allocation, zeroed (HEAP_ZERO_MEMORY).
    let p = unsafe { HeapAlloc(GetProcessHeap(), 8, total) };
    if p.is_null() {
        return Err(ERROR_NOT_ENOUGH_MEMORY);
    }
    // SAFETY: `p` has `total` zeroed bytes laid out as described; blobs
    // point into the copy of the encoding made first.
    let filled = unsafe {
        let entries = p.add(header).cast::<CrlEntry>();
        let exts = p.add(header + entry_bytes).cast::<Extension>();
        let mut serial_at = p.add(header + entry_bytes + ext_bytes);
        let mut oid_at = serial_at.add(serial_bytes);
        let encoded = p.add(total - used);
        core::ptr::copy_nonoverlapping(input.as_ptr(), encoded, used);
        let base = input.as_ptr() as usize;
        let at = |s: &[u8]| encoded.add(s.as_ptr() as usize - base);
        let blob = |s: &[u8]| Blob { size: s.len() as u32, data: at(s) };
        let mut oid = |contents: &[u8]| -> Result<*mut u8, u32> {
            let dst = oid_at;
            oid_text(contents, core::slice::from_raw_parts_mut(dst, OID_TEXT))?;
            oid_at = oid_at.add(OID_TEXT);
            Ok(dst)
        };
        let mut ext_at = 0usize;
        // The extensions of `list`, written from `ext_at` on: (count,
        // first).
        let mut extensions = |list: &[u8],
                              oid: &mut dyn FnMut(&[u8]) -> Result<*mut u8, u32>|
         -> Result<(u32, *mut Extension), u32> {
            let first = exts.add(ext_at);
            let mut n = 0u32;
            let mut d = Der::new(list);
            while d.peek_tag().is_some() {
                let e = d.expect(SEQUENCE)?;
                let mut x = Der::new(e.content);
                let id = x.expect(OID)?;
                let mut critical = 0;
                if x.peek_tag() == Some(BOOLEAN) {
                    critical = i32::from(x.next()?.content.first().is_some_and(|&b| b != 0));
                }
                let value = x.expect(OCTET_STRING)?;
                exts.add(ext_at).write(Extension {
                    oid: oid(id.content)?,
                    critical,
                    value: blob(value.content),
                });
                ext_at += 1;
                n += 1;
            }
            Ok((n, if n == 0 { core::ptr::null_mut() } else { first }))
        };
        (|| -> Result<CrlInfo, u32> {
            let mut d = Der::new(parsed.entries);
            for i in 0..entry_count {
                let (serial, revoked_at, ext) = entry(d.next()?)?;
                let s = serial_at;
                for (k, &b) in serial.iter().rev().enumerate() {
                    *s.add(k) = b;
                }
                serial_at = serial_at.add(serial.len());
                let (n, first) = extensions(ext, &mut oid)?;
                entries.add(i).write(CrlEntry {
                    serial: Blob { size: serial.len() as u32, data: s },
                    revoked_at,
                    extension_count: n,
                    extensions: first,
                });
            }
            let (n, first) = extensions(parsed.extensions, &mut oid)?;
            Ok(CrlInfo {
                version: parsed.version,
                signature: Algorithm {
                    oid: oid(parsed.signature.0)?,
                    parameters: blob(parsed.signature.1),
                },
                issuer: blob(parsed.issuer),
                this_update: parsed.this_update,
                next_update: parsed.next_update,
                entry_count: entry_count as u32,
                entries: if entry_count == 0 { core::ptr::null_mut() } else { entries },
                extension_count: n,
                extensions: first,
            })
        })()
        .map(|info| (info, encoded))
    };
    let (info, encoded) = match filled {
        Ok(f) => f,
        Err(e) => {
            // SAFETY: our block.
            unsafe { HeapFree(GetProcessHeap(), 0, p) };
            return Err(e);
        }
    };
    let b = p.cast::<CrlBlock>();
    // SAFETY: the block's header, written once; the context points at the
    // block's own CRL_INFO.
    unsafe {
        b.write(CrlBlock {
            magic: CRL_MAGIC,
            refs: AtomicU32::new(1),
            context: CrlContext {
                encoding,
                encoded,
                encoded_len: used as u32,
                info: core::ptr::null_mut(),
                store: core::ptr::null_mut(),
            },
            info,
        });
        (*b).context.info = &raw mut (*b).info;
        Ok(&raw const (*b).context)
    }
}

/// CertDuplicateCRLContext: one more reference to the same context.
#[unsafe(no_mangle)]
pub extern "system" fn CertDuplicateCRLContext(p: *const CrlContext) -> *const CrlContext {
    if let Some(b) = block(p) {
        let old = b.refs.fetch_add(1, Ordering::AcqRel);
        if old == OWNED | 1 {
            // The first caller's reference: it keeps the store.
            crate::store::hold(b.context.store);
        }
    }
    p
}

/// Puts a new copy `p` (with the reference the store will hold) in store
/// `h`'s list: its hCertStore.
pub(crate) fn adopt(p: *const CrlContext, h: *mut c_void) {
    if let Some(b) = block(p) {
        b.context.store = h;
        b.refs.fetch_or(OWNED, Ordering::AcqRel);
    }
}

/// Takes `p` out of its store's list (as cert.rs's `disown`).
pub(crate) fn disown(p: *const CrlContext) {
    if let Some(b) = block(p) {
        let old = b.refs.fetch_and(!OWNED, Ordering::AcqRel);
        if old & OWNED != 0 && old & !OWNED >= 2 {
            crate::store::release(b.context.store);
        }
    }
}

/// CertFreeCRLContext: one reference less; the last frees the context.
#[unsafe(no_mangle)]
pub extern "system" fn CertFreeCRLContext(p: *const CrlContext) -> i32 {
    let Some(b) = block(p) else { return 1 };
    let old = b.refs.fetch_sub(1, Ordering::AcqRel);
    if old == OWNED | 2 {
        crate::store::release(b.context.store);
        return 1;
    }
    if old & !OWNED == 1 {
        b.magic = 0;
        // SAFETY: our block.
        unsafe { HeapFree(GetProcessHeap(), 0, (b as *mut CrlBlock).cast()) };
    }
    1
}

// ---- Entries ----

fn crl_info<'a>(p: *const CrlContext) -> &'a CrlInfo {
    // SAFETY: a context of ours (or the caller's CRL_CONTEXT).
    unsafe { &*(*p).info }
}

fn bytes<'a>(b: &Blob) -> &'a [u8] {
    if b.data.is_null() {
        return &[];
    }
    // SAFETY: a blob of a context, pointing into its block.
    unsafe { core::slice::from_raw_parts(b.data, b.size as usize) }
}

fn entries<'a>(info: &CrlInfo) -> &'a [CrlEntry] {
    if info.entries.is_null() {
        return &[];
    }
    // SAFETY: the CRL_INFO's array of cCRLEntry entries.
    unsafe { core::slice::from_raw_parts(info.entries, info.entry_count as usize) }
}

/// The entry of `info` for the serial number `serial` (little-endian, as
/// CERT_INFO's).
fn entry_for<'a>(info: &CrlInfo, serial: &[u8]) -> Option<&'a CrlEntry> {
    entries(info).iter().find(|e| bytes(&e.serial) == serial)
}

/// The entry's CRL_REASON_* (its reasonCode extension; 0, unspecified,
/// without one).
fn reason(e: &CrlEntry) -> u32 {
    let ext = CertFindExtension(c"2.5.29.21".as_ptr().cast(), e.extension_count, e.extensions);
    if ext.is_null() {
        return 0;
    }
    // SAFETY: an extension of the entry.
    let value = bytes(unsafe { &(*ext).value });
    match Der::new(value).next() {
        Ok(t) if t.tag == 0x0a && t.content.len() == 1 => u32::from(t.content[0]),
        _ => 0,
    }
}

/// CertFindCertificateInCRL(cert, crl, flags, reserved, entry): TRUE,
/// with the CRL's entry for the certificate's serial number in *entry,
/// or NULL when it is not listed.
#[unsafe(no_mangle)]
pub extern "system" fn CertFindCertificateInCRL(
    cert: *const CertContext,
    crl: *const CrlContext,
    _flags: u32,
    _reserved: *const c_void,
    out: *mut *const CrlEntry,
) -> i32 {
    if cert.is_null() || crl.is_null() || out.is_null() {
        return error(E_INVALIDARG, 0);
    }
    // SAFETY: the caller's certificate context.
    let serial = bytes(unsafe { &(*(*cert).info).serial });
    let found = entry_for(crl_info(crl), serial).map_or(core::ptr::null(), |e| e as *const _);
    // SAFETY: the caller's output.
    unsafe { out.write(found) };
    1
}

/// CertVerifyCRLRevocation(encoding, certInfo, count, crlInfos): FALSE
/// if one of the CRLs lists the certificate's serial number, else TRUE.
#[unsafe(no_mangle)]
pub extern "system" fn CertVerifyCRLRevocation(
    _encoding: u32,
    cert: *const CertInfo,
    count: u32,
    crls: *const *const CrlInfo,
) -> i32 {
    if cert.is_null() {
        return error(E_INVALIDARG, 0);
    }
    // SAFETY: the caller's CERT_INFO.
    let serial = bytes(unsafe { &(*cert).serial });
    for i in 0..count as usize {
        // SAFETY: the caller's array of `count` CRL_INFO pointers.
        let info = unsafe { *crls.add(i) };
        // SAFETY: the caller's CRL_INFO.
        if !info.is_null() && entry_for(unsafe { &*info }, serial).is_some() {
            return 0;
        }
    }
    1
}

/// CertVerifyCRLTimeValidity(time, crlInfo): -1 before thisUpdate, 1
/// after nextUpdate (if the CRL has one), else 0; `time` NULL is now.
#[unsafe(no_mangle)]
pub extern "system" fn CertVerifyCRLTimeValidity(time: *const u64, info: *const CrlInfo) -> i32 {
    // SAFETY: the caller's FILETIME and CRL_INFO.
    let (t, info) = unsafe { (if time.is_null() { now() } else { time.read_unaligned() }, &*info) };
    if t < info.this_update {
        -1
    } else if info.next_update != 0 && t > info.next_update {
        1
    } else {
        0
    }
}

// ---- Revocation status ----

/// What the CRLs say of a certificate (module comment).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Status {
    /// A current CRL of the issuer does not list it (its thisUpdate).
    Good(u64),
    /// Listed, with the CRL_REASON_*, in a CRL of that thisUpdate.
    Revoked(u32, u64),
    /// The issuer's CRLs are out of date, or the CRL the certificate names
    /// was not fetched.
    Offline,
    /// Nothing to check it by.
    NoCheck,
}

/// cRLSign in the key usage's first byte.
const CRL_SIGN: u8 = 0x02;

/// Whether `issuer` may sign CRLs: no key usage, or one with cRLSign.
fn may_sign_crls(issuer: *const CertContext) -> bool {
    // SAFETY: a context of ours.
    let Some(value) = extension_value(unsafe { &*(*issuer).info }, c"2.5.29.15") else {
        return true;
    };
    Der::new(value)
        .expect(BIT_STRING)
        .is_ok_and(|bits| bits.content.get(1).is_some_and(|&b| b & CRL_SIGN != 0))
}

/// Whether the CRL's signature verifies with `issuer`'s key.
fn crl_signed_by(crl: *const CrlContext, issuer: *const CertContext) -> bool {
    CryptVerifyCertificateSignatureEx(
        0,
        X509_ASN_ENCODING,
        3,
        crl.cast(),
        2,
        issuer.cast(),
        0,
        core::ptr::null(),
    ) != 0
}

/// Whether the certificate is a CA (basic constraints with cA).
fn is_ca(cert: *const CertContext) -> bool {
    // SAFETY: a context of ours.
    let Some(value) = extension_value(unsafe { &*(*cert).info }, c"2.5.29.19") else {
        return false;
    };
    let Ok(seq) = Der::new(value).expect(SEQUENCE) else { return false };
    let mut d = Der::new(seq.content);
    d.peek_tag() == Some(BOOLEAN)
        && d.next().is_ok_and(|t| t.content.first().is_some_and(|&b| b != 0))
}

/// Why a CRL of the right issuer does not decide for `cert` (None: it
/// does): RFC 5280's scope (5.2.4 delta CRLs, 5.2.5 the issuing
/// distribution point) and critical extensions it does not know.
fn out_of_scope(crl: &CrlInfo, cert: *const CertContext) -> Option<&'static [u8]> {
    let exts = if crl.extensions.is_null() {
        &[][..]
    } else {
        // SAFETY: the CRL_INFO's extensions.
        unsafe { core::slice::from_raw_parts(crl.extensions, crl.extension_count as usize) }
    };
    for e in exts {
        // SAFETY: the extension's OID string.
        let id = unsafe { crate::cert::c_text(e.oid) };
        match id {
            b"2.5.29.27" => return Some(b"crypt32.dll!revocation: a delta CRL (not used)"),
            b"2.5.29.28" => {
                // IssuingDistributionPoint: [1] onlyContainsUserCerts, [2]
                // onlyContainsCACerts, [3] onlySomeReasons, [4] indirectCRL,
                // [5] onlyContainsAttributeCerts.
                let Ok(seq) = Der::new(bytes(&e.value)).expect(SEQUENCE) else {
                    return Some(b"crypt32.dll!revocation: a CRL whose scope does not decode");
                };
                let mut d = Der::new(seq.content);
                while let Ok(t) = d.next() {
                    let on = t.content.first().is_some_and(|&b| b != 0);
                    match t.tag {
                        0x81 if on && is_ca(cert) => {
                            return Some(b"crypt32.dll!revocation: a CRL of end certificates only");
                        }
                        0x82 if on && !is_ca(cert) => {
                            return Some(b"crypt32.dll!revocation: a CRL of CAs only");
                        }
                        // onlySomeReasons (a BIT STRING), or set flags.
                        0x83 => {
                            return Some(b"crypt32.dll!revocation: a CRL of some reasons only");
                        }
                        0x84 | 0x85 if on => {
                            return Some(b"crypt32.dll!revocation: an indirect CRL (not used)");
                        }
                        _ => {}
                    }
                }
            }
            b"2.5.29.20" | b"2.5.29.35" | b"2.5.29.46" => {}
            _ if e.critical != 0 => {
                return Some(b"crypt32.dll!revocation: a CRL with an unknown critical extension");
            }
            _ => {}
        }
    }
    None
}

/// Where the certificate says its revocation is published: (a CRL
/// distribution point, an OCSP responder in its authority information
/// access).
fn sources(cert: *const CertContext) -> (bool, bool) {
    // SAFETY: a context of ours.
    let i = unsafe { &*(*cert).info };
    let crl = extension_value(i, c"2.5.29.31").is_some();
    // AuthorityInfoAccessSyntax: SEQUENCE OF { accessMethod OID, ... };
    // id-ad-ocsp is 1.3.6.1.5.5.7.48.1.
    const OCSP: &[u8] = &[0x2b, 0x06, 0x01, 0x05, 0x05, 0x07, 0x30, 0x01];
    let ocsp = extension_value(i, c"1.3.6.1.5.5.7.1.1").is_some_and(|v| {
        let Ok(seq) = Der::new(v).expect(SEQUENCE) else { return false };
        let mut d = Der::new(seq.content);
        while let Ok(desc) = d.expect(SEQUENCE) {
            if Der::new(desc.content).expect(OID).is_ok_and(|m| m.content == OCSP) {
                return true;
            }
        }
        false
    });
    (crl, ocsp)
}

/// The revocation status of `cert`, issued by `issuer` (null: not known,
/// so no CRL can be verified), from the CRLs in `stores`, at `now`
/// (module comment).
pub(crate) fn check(
    cert: *const CertContext,
    issuer: *const CertContext,
    stores: &[*mut c_void],
    now: u64,
) -> Status {
    // A certificate that names no CRL distribution point is not checked
    // by CRL, whatever the stores hold (Windows' default provider answers
    // CRYPT_E_NO_REVOCATION_CHECK for one even with its issuer's CRL in
    // hCrlStore: Wine's tests, testVerifyRevocation); OCSP is not asked
    // (offline only).
    let (crl, ocsp) = sources(cert);
    if !crl {
        if ocsp {
            unsupported(b"crypt32.dll!revocation: OCSP not asked (offline only)", 0);
            return Status::Offline;
        }
        return Status::NoCheck;
    }
    // SAFETY: a context of ours.
    let ci = unsafe { &*(*cert).info };
    let issuer_name = bytes(&ci.issuer);
    let serial = bytes(&ci.serial);
    let signer_ok = !issuer.is_null() && may_sign_crls(issuer);
    let mut good = None;
    let mut stale = false;
    for &s in stores.iter().filter(|s| !s.is_null()) {
        let mut p: *const CrlContext = core::ptr::null();
        loop {
            // The enumeration frees the previous one.
            p = CertEnumCRLsInStore(s, p);
            if p.is_null() {
                break;
            }
            let info = crl_info(p);
            if bytes(&info.issuer) != issuer_name || !signer_ok {
                continue;
            }
            if !crl_signed_by(p, issuer) {
                note(
                    b"crypt32.dll!revocation: a CRL whose signature does not verify (not used)",
                    0,
                );
                continue;
            }
            if let Some(why) = out_of_scope(info, cert) {
                unsupported(why, 0);
                continue;
            }
            if now < info.this_update || (info.next_update != 0 && now > info.next_update) {
                stale = true;
                continue;
            }
            if let Some(e) = entry_for(info, serial) {
                let r = Status::Revoked(reason(e), info.this_update);
                CertFreeCRLContext(p);
                return r;
            }
            good = Some(good.map_or(info.this_update, |t: u64| t.max(info.this_update)));
        }
    }
    if let Some(t) = good {
        Status::Good(t)
    } else if stale {
        Status::Offline
    } else {
        // Offline only: the CRL the certificate names is not fetched.
        unsupported(b"crypt32.dll!revocation: CRL not fetched (offline only)", 0);
        Status::Offline
    }
}

/// The issuer of `cert` among `stores`: a certificate whose subject is
/// its issuer and whose key signed it (one more reference), or null.
fn find_issuer(cert: *const CertContext, stores: &[*mut c_void]) -> *const CertContext {
    const CERT_FIND_SUBJECT_NAME: u32 = 0x2_0007;
    // SAFETY: a context of ours.
    let name = unsafe { &(*(*cert).info).issuer } as *const Blob;
    for &s in stores.iter().filter(|s| !s.is_null()) {
        let mut c = core::ptr::null();
        loop {
            c = CertFindCertificateInStore(
                s,
                X509_ASN_ENCODING,
                0,
                CERT_FIND_SUBJECT_NAME,
                name.cast(),
                c,
            );
            if c.is_null() {
                break;
            }
            let signed = CryptVerifyCertificateSignatureEx(
                0,
                X509_ASN_ENCODING,
                2,
                cert.cast(),
                2,
                c.cast(),
                0,
                core::ptr::null(),
            ) != 0;
            if signed {
                return c;
            }
        }
    }
    core::ptr::null()
}

const CERT_CONTEXT_REVOCATION_TYPE: u32 = 1;
const CERT_VERIFY_REV_CHAIN_FLAG: u32 = 1;

/// CERT_REVOCATION_STATUS: dwIndex, dwError, dwReason, and (cbSize 24)
/// fHasFreshnessTime, dwFreshnessTime.
fn set_status(status: *mut u8, size: u32, index: u32, error: u32, reason: u32, fresh: Option<u32>) {
    let s = status.cast::<RevocationStatus>();
    // SAFETY: the caller's structure of `size` bytes (16 or 24).
    unsafe {
        core::ptr::addr_of_mut!((*s).index).write_unaligned(index);
        core::ptr::addr_of_mut!((*s).error).write_unaligned(error);
        core::ptr::addr_of_mut!((*s).reason).write_unaligned(reason);
        if has!(size, RevocationStatus, freshness) {
            core::ptr::addr_of_mut!((*s).has_freshness).write_unaligned(i32::from(fresh.is_some()));
            core::ptr::addr_of_mut!((*s).freshness).write_unaligned(fresh.unwrap_or(0));
        }
    }
}

/// CertVerifyRevocation(encoding, revType, count, contexts, flags, para,
/// status): the default revocation provider (module comment). TRUE when
/// no context is revoked or unchecked; else FALSE with the first such
/// context's index, CRYPT_E_REVOKED (and the reason),
/// CRYPT_E_REVOCATION_OFFLINE or CRYPT_E_NO_REVOCATION_CHECK in the
/// status and as the last error. A status of another size than 16 or 24
/// bytes is E_INVALIDARG; an encoding without X509_ASN_ENCODING has no
/// provider (CRYPT_E_NO_REVOCATION_DLL).
#[unsafe(no_mangle)]
pub extern "system" fn CertVerifyRevocation(
    encoding: u32,
    kind: u32,
    count: u32,
    contexts: *const *const CertContext,
    flags: u32,
    para: *const u8,
    status: *mut u8,
) -> i32 {
    if status.is_null() {
        return error(E_INVALIDARG, 0);
    }
    // SAFETY: the caller's structure starts with its cbSize.
    let size = unsafe { status.cast::<u32>().read_unaligned() };
    if size != 16 && size != 24 {
        return error(E_INVALIDARG, 0);
    }
    if count == 0 {
        return 1;
    }
    let fail = |index: u32, code: u32, reason: u32| {
        set_status(status, size, index, code, reason, None);
        let name: &[u8] = match code {
            CRYPT_E_REVOKED => b"crypt32.dll!CertVerifyRevocation: CRYPT_E_REVOKED",
            CRYPT_E_REVOCATION_OFFLINE => {
                b"crypt32.dll!CertVerifyRevocation: CRYPT_E_REVOCATION_OFFLINE"
            }
            CRYPT_E_NO_REVOCATION_CHECK => {
                b"crypt32.dll!CertVerifyRevocation: CRYPT_E_NO_REVOCATION_CHECK"
            }
            _ => b"crypt32.dll!CertVerifyRevocation: CRYPT_E_NO_REVOCATION_DLL",
        };
        note(name, index.into());
        error(code, 0)
    };
    if encoding & X509_ASN_ENCODING == 0 || contexts.is_null() {
        return fail(0, CRYPT_E_NO_REVOCATION_DLL, 0);
    }
    if kind != CERT_CONTEXT_REVOCATION_TYPE {
        unsupported(b"crypt32.dll!CertVerifyRevocation(revocation type)", kind.into());
        return fail(0, CRYPT_E_NO_REVOCATION_CHECK, 0);
    }
    // CERT_REVOCATION_PARA: the fields its cbSize covers.
    // SAFETY: the caller's structure starts with its cbSize.
    let psize = if para.is_null() { 0 } else { unsafe { para.cast::<u32>().read_unaligned() } };
    let issuer = if has!(psize, RevocationPara, issuer) {
        get!(para, RevocationPara, issuer).cast::<CertContext>()
    } else {
        core::ptr::null()
    };
    let store_count = if has!(psize, RevocationPara, store_count) {
        get!(para, RevocationPara, store_count)
    } else {
        0
    };
    let cert_stores = if has!(psize, RevocationPara, stores) {
        get!(para, RevocationPara, stores)
    } else {
        core::ptr::null()
    };
    let crl_store = if has!(psize, RevocationPara, crl_store) {
        get!(para, RevocationPara, crl_store)
    } else {
        core::ptr::null_mut()
    };
    let time = if has!(psize, RevocationPara, time) {
        get!(para, RevocationPara, time)
    } else {
        core::ptr::null()
    };
    // SAFETY: the caller's FILETIME.
    let at = if time.is_null() { now() } else { unsafe { time.read_unaligned() } };
    // Where CRLs and issuers are found: hCrlStore, rgCertStore, the CA store.
    let mut stores = [core::ptr::null_mut(); 18];
    stores[0] = crl_store;
    let n = (store_count as usize).min(16);
    if !cert_stores.is_null() {
        for (i, slot) in stores[1..=n].iter_mut().enumerate() {
            // SAFETY: the caller's array of cCertStore handles.
            *slot = unsafe { *cert_stores.add(i) };
        }
    }
    let ca = crate::store::CertOpenSystemStoreA(0, c"CA".as_ptr().cast());
    stores[n + 1] = ca;
    let stores = &stores[..n + 2];
    let mut result = 1;
    let mut freshness = None;
    for i in 0..count as usize {
        // SAFETY: the caller's array of `count` certificate contexts.
        let cert = unsafe { *contexts.add(i) };
        if cert.is_null() {
            result = fail(i as u32, CRYPT_E_NO_REVOCATION_CHECK, 0);
            break;
        }
        let chained = flags & CERT_VERIFY_REV_CHAIN_FLAG != 0 && i + 1 < count as usize;
        let (by, found) = if chained {
            // SAFETY: as above.
            (unsafe { *contexts.add(i + 1) }, false)
        } else if !issuer.is_null() {
            (issuer, false)
        } else {
            (find_issuer(cert, stores), true)
        };
        let s = check(cert, by, stores, at);
        if found && !by.is_null() {
            crate::cert::CertFreeCertificateContext(by);
        }
        let seconds = |t: u64| u32::try_from(at.saturating_sub(t) / 10_000_000).unwrap_or(u32::MAX);
        match s {
            Status::Good(t) => freshness = Some(seconds(t)),
            Status::Revoked(reason, t) => {
                result = fail(i as u32, CRYPT_E_REVOKED, reason);
                set_status(status, size, i as u32, CRYPT_E_REVOKED, reason, Some(seconds(t)));
                break;
            }
            Status::Offline => {
                result = fail(i as u32, CRYPT_E_REVOCATION_OFFLINE, 0);
                break;
            }
            Status::NoCheck => {
                result = fail(i as u32, CRYPT_E_NO_REVOCATION_CHECK, 0);
                break;
            }
        }
    }
    CertCloseStore(ca, 0);
    if result != 0 {
        set_status(status, size, 0, 0, 0, freshness);
    }
    result
}

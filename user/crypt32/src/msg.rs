//! Cryptographic messages, decoding side: CryptMsgOpenToDecode,
//! CryptMsgUpdate, CryptMsgGetParam, CryptMsgControl (the signer's
//! signature), CryptMsgDuplicate and CryptMsgClose over a PKCS #7 /
//! CMS SignedData (RFC 2315, RFC 5652), the message type Authenticode
//! signatures and RFC 3161 timestamp tokens are (wintrust.dll's
//! WinVerifyTrust reads them through these functions, as on Windows).
//!
//! A message handle is a heap block holding the encoding the updates
//! gave; the final update checks that it decodes, and every parameter is
//! read from the encoding again when asked (nothing is decoded ahead).
//! The documented structures a parameter returns (CMSG_SIGNER_INFO,
//! CRYPT_ATTRIBUTES, CERT_INFO with the signer's issuer and serial number)
//! are built in the caller's buffer, their pointers pointing into it
//! (`Out`: one pass measures, a second writes).
//!
//! Not here (each recorded `crypt32.dll!CryptMsg...(...)` in the ledger):
//! encoding (CryptMsgOpenToEncode), streamed decoding (a
//! CMSG_STREAM_INFO), detached content, message types other than
//! SignedData and Data (enveloped, hashed, ...), BER's indefinite lengths
//! (gala's DER reader), signers identified by a subject key identifier
//! (CMS version 3; a timestamp's signer identified so is looked up in
//! CryptVerifyTimeStampSignature by it), CMSG_CTRL_VERIFY_SIGNATURE_EX
//! with a chain or no signer, the other parameters and controls.

#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, Ordering};

use crate::cert::{
    Algorithm, BOOLEAN, Blob, CertContext, CertInfo, Der, INTEGER, OCTET_STRING, OID,
    PublicKeyInfo, SEQUENCE, SET, Tlv, algorithm_id, digest_algorithm, hash_with, oid_text,
    signer_scheme, verify_hashed,
};
use crate::store::{error, unsupported};

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GetProcessHeap() -> usize;
    fn HeapAlloc(heap: usize, flags: u32, size: usize) -> *mut u8;
    fn HeapReAlloc(heap: usize, flags: u32, p: *mut u8, size: usize) -> *mut u8;
    fn HeapFree(heap: usize, flags: u32, p: *mut u8) -> i32;
}

const E_INVALIDARG: u32 = 0x8007_0057;
const ERROR_MORE_DATA: u32 = 234;
const ERROR_NOT_ENOUGH_MEMORY: u32 = 8;
const NTE_BAD_SIGNATURE: u32 = 0x8009_0006;
const NTE_BAD_ALGID: u32 = 0x8009_0008;
const CRYPT_E_MSG_ERROR: u32 = 0x8009_1001;
const CRYPT_E_INVALID_MSG_TYPE: u32 = 0x8009_1004;
const CRYPT_E_HASH_VALUE: u32 = 0x8009_1007;
const CRYPT_E_INVALID_INDEX: u32 = 0x8009_1008;
const CRYPT_E_CONTROL_TYPE: u32 = 0x8009_100c;
const CRYPT_E_SIGNER_NOT_FOUND: u32 = 0x8009_100e;
const CRYPT_E_AUTH_ATTR_MISSING: u32 = 0x8009_1006;
const CRYPT_E_ASN1_BADTAG: u32 = 0x8009_310b;
const CRYPT_E_ASN1_CORRUPT: u32 = 0x8009_3100;

pub(crate) const CMSG_DATA: u32 = 1;
pub(crate) const CMSG_SIGNED: u32 = 2;

// CryptMsgGetParam's parameters.
const CMSG_TYPE_PARAM: u32 = 1;
const CMSG_CONTENT_PARAM: u32 = 2;
const CMSG_INNER_CONTENT_TYPE_PARAM: u32 = 4;
const CMSG_SIGNER_COUNT_PARAM: u32 = 5;
const CMSG_SIGNER_INFO_PARAM: u32 = 6;
const CMSG_SIGNER_CERT_INFO_PARAM: u32 = 7;
const CMSG_SIGNER_HASH_ALGORITHM_PARAM: u32 = 8;
const CMSG_SIGNER_AUTH_ATTR_PARAM: u32 = 9;
const CMSG_SIGNER_UNAUTH_ATTR_PARAM: u32 = 10;
const CMSG_CERT_COUNT_PARAM: u32 = 11;
const CMSG_CERT_PARAM: u32 = 12;
const CMSG_CRL_COUNT_PARAM: u32 = 13;
const CMSG_CRL_PARAM: u32 = 14;
const CMSG_ENCODED_SIGNER: u32 = 28;
const CMSG_ENCODED_MESSAGE: u32 = 29;
const CMSG_VERSION_PARAM: u32 = 30;

// CryptMsgControl's controls and signer kinds.
const CMSG_CTRL_VERIFY_SIGNATURE: u32 = 1;
const CMSG_CTRL_VERIFY_SIGNATURE_EX: u32 = 19;
const CMSG_VERIFY_SIGNER_PUBKEY: u32 = 1;
const CMSG_VERIFY_SIGNER_CERT: u32 = 2;

// OID contents.
const SIGNED_DATA: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x07, 0x02];
const DATA: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x07, 0x01];
const MESSAGE_DIGEST: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x09, 0x04];

// ---- Building the documented structures in the caller's buffer ----

/// CRYPT_ATTRIBUTE.
#[repr(C)]
pub(crate) struct Attribute {
    pub oid: *mut u8,
    pub count: u32,
    pub values: *mut Blob,
}

/// CRYPT_ATTRIBUTES.
#[repr(C)]
pub(crate) struct Attributes {
    pub count: u32,
    pub attrs: *mut Attribute,
}

/// CMSG_SIGNER_INFO.
#[repr(C)]
pub(crate) struct SignerInfo {
    pub version: u32,
    pub issuer: Blob,
    pub serial: Blob,
    pub hash_algorithm: Algorithm,
    pub hash_encryption: Algorithm,
    pub encrypted_hash: Blob,
    pub auth: Attributes,
    pub unauth: Attributes,
}

/// CMSG_CTRL_VERIFY_SIGNATURE_EX_PARA.
#[repr(C)]
struct VerifyExPara {
    size: u32,
    prov: usize,
    signer_index: u32,
    signer_type: u32,
    signer: *const c_void,
}

crate::layout::check!(core::mem::size_of::<Attribute>(), 12, 24);
crate::layout::check!(core::mem::size_of::<Attributes>(), 8, 16);
crate::layout::check!(core::mem::size_of::<SignerInfo>(), 68, 136);
crate::layout::check!(core::mem::offset_of!(SignerInfo, hash_algorithm), 20, 40);
crate::layout::check!(core::mem::offset_of!(SignerInfo, encrypted_hash), 44, 88);
crate::layout::check!(core::mem::offset_of!(SignerInfo, unauth), 60, 120);
crate::layout::check!(core::mem::size_of::<VerifyExPara>(), 20, 32);
crate::layout::check!(core::mem::offset_of!(VerifyExPara, signer), 16, 24);

/// A bump writer over the caller's buffer: with no buffer it only
/// measures, so the same building code gives the size first and then the
/// structure (the pointers it writes point into the buffer).
pub(crate) struct Out {
    base: *mut u8,
    pub at: usize,
}

impl Out {
    pub fn measure() -> Out {
        Out { base: core::ptr::null_mut(), at: 0 }
    }

    /// Room for `size` bytes aligned to `align` (from the buffer's start).
    pub fn place(&mut self, size: usize, align: usize) -> *mut u8 {
        self.at = self.at.next_multiple_of(align);
        let p = if self.base.is_null() {
            core::ptr::null_mut()
        } else {
            // SAFETY: within the buffer the measuring pass sized.
            unsafe { self.base.add(self.at) }
        };
        self.at += size;
        p
    }

    /// Room for one `T`, filled later with `put`.
    pub fn slot<T>(&mut self) -> *mut T {
        self.place(core::mem::size_of::<T>(), core::mem::align_of::<T>()).cast()
    }

    /// A copy of `data`.
    pub fn bytes(&mut self, data: &[u8]) -> *mut u8 {
        let p = self.place(data.len(), 1);
        if !p.is_null() {
            // SAFETY: room for `data.len()` bytes at `p`.
            unsafe { core::ptr::copy_nonoverlapping(data.as_ptr(), p, data.len()) };
        }
        p
    }

    /// A blob holding a copy of `data`.
    pub fn blob(&mut self, data: &[u8]) -> Blob {
        Blob { size: data.len() as u32, data: self.bytes(data) }
    }

    /// A blob holding `data` reversed (CryptoAPI's little-endian integers).
    pub fn reversed(&mut self, data: &[u8]) -> Blob {
        let p = self.place(data.len(), 1);
        if !p.is_null() {
            for (i, &b) in data.iter().rev().enumerate() {
                // SAFETY: room for `data.len()` bytes at `p`.
                unsafe { *p.add(i) = b };
            }
        }
        Blob { size: data.len() as u32, data: p }
    }

    /// The dotted text of OID contents `oid`, NUL-terminated.
    pub fn oid(&mut self, oid: &[u8]) -> Result<*mut u8, u32> {
        let mut text = [0u8; 128];
        let n = oid_text(oid, &mut text)?;
        Ok(self.bytes(&text[..=n]))
    }

    /// An algorithm identifier: its OID text and encoded parameters.
    pub fn algorithm(&mut self, alg: (&[u8], &[u8])) -> Result<Algorithm, u32> {
        Ok(Algorithm { oid: self.oid(alg.0)?, parameters: self.blob(alg.1) })
    }
}

/// Writes `v` at `p` (nothing while measuring).
pub(crate) fn put<T>(p: *mut T, v: T) {
    if !p.is_null() {
        // SAFETY: a slot `Out` placed for a `T`.
        unsafe { p.write_unaligned(v) };
    }
}

/// Builds a structure into the caller's buffer as CryptoAPI's getters do:
/// the size alone with no buffer, ERROR_MORE_DATA (and the size) when it
/// is short.
pub(crate) fn give_built(
    out: *mut c_void,
    len: *mut u32,
    build: impl Fn(&mut Out) -> Result<(), u32>,
) -> i32 {
    if len.is_null() {
        return error(E_INVALIDARG, 0);
    }
    let mut m = Out::measure();
    if let Err(e) = build(&mut m) {
        return error(e, 0);
    }
    let size = m.at;
    // SAFETY: the caller's size.
    let room = unsafe { *len } as usize;
    // SAFETY: as above.
    unsafe { *len = size as u32 };
    if out.is_null() {
        return 1;
    }
    if room < size {
        return error(ERROR_MORE_DATA, 0);
    }
    // SAFETY: the caller's buffer of at least `size` bytes.
    unsafe { core::ptr::write_bytes(out.cast::<u8>(), 0, size) };
    let mut w = Out { base: out.cast(), at: 0 };
    match build(&mut w) {
        Ok(()) => 1,
        Err(e) => error(e, 0),
    }
}

/// Copies `data` out as CryptoAPI's getters do.
fn give_bytes(data: &[u8], out: *mut c_void, len: *mut u32) -> i32 {
    give_built(out, len, |w| {
        w.bytes(data);
        Ok(())
    })
}

fn give_u32(v: u32, out: *mut c_void, len: *mut u32) -> i32 {
    give_bytes(&v.to_le_bytes(), out, len)
}

// ---- The SignedData ----

/// A decoded SignedData: slices of the encoding.
pub(crate) struct Signed<'a> {
    pub version: u32,
    /// The encapsulated content's type (OID contents).
    pub content_type: &'a [u8],
    /// The encapsulated content: the TLV inside its [0], if present.
    pub content: Option<Tlv<'a>>,
    /// The certificates' and CRLs' fields' contents (empty if absent).
    pub certs: &'a [u8],
    pub crls: &'a [u8],
    /// The SignerInfos SET's contents.
    pub signers: &'a [u8],
}

/// The SignedData of `der`: a ContentInfo whose type is signedData, or,
/// for a message opened as CMSG_SIGNED, the bare SignedData.
pub(crate) fn parse_signed(der: &[u8]) -> Result<Signed<'_>, u32> {
    let outer = Der::new(der).expect(SEQUENCE)?;
    let mut d = Der::new(outer.content);
    let body = if d.peek_tag() == Some(OID) {
        let oid = d.expect(OID)?;
        if oid.content != SIGNED_DATA {
            return Err(CRYPT_E_INVALID_MSG_TYPE);
        }
        let explicit = d.expect(0xa0)?;
        Der::new(explicit.content).expect(SEQUENCE)?.content
    } else {
        outer.content
    };
    let mut s = Der::new(body);
    let version = s.expect(INTEGER)?.content.last().copied().unwrap_or(0).into();
    s.expect(SET)?; // digestAlgorithms
    let encap = s.expect(SEQUENCE)?;
    let mut e = Der::new(encap.content);
    let content_type = e.expect(OID)?.content;
    let content =
        if e.peek_tag() == Some(0xa0) { Some(Der::new(e.next()?.content).next()?) } else { None };
    let (mut certs, mut crls) = (&[][..], &[][..]);
    if s.peek_tag() == Some(0xa0) {
        certs = s.next()?.content;
    }
    if s.peek_tag() == Some(0xa1) {
        crls = s.next()?.content;
    }
    let signers = s.expect(SET)?.content;
    Ok(Signed { version, content_type, content, certs, crls, signers })
}

impl Signed<'_> {
    /// The content as CMSG_CONTENT_PARAM gives it: an OCTET STRING's
    /// octets (data, CMS content), else the inner TLV whole (PKCS #7's
    /// other types: Authenticode's SpcIndirectDataContent).
    pub fn content_param(&self) -> &[u8] {
        match self.content {
            Some(t) if t.tag == OCTET_STRING => t.content,
            Some(t) => t.whole,
            None => &[],
        }
    }

    /// What the signers' messageDigest covers: an OCTET STRING's octets,
    /// else the inner TLV's contents (PKCS #7 section 9.3: the contents
    /// octets of the content's DER, without its identifier and length).
    pub fn digested(&self) -> &[u8] {
        match self.content {
            Some(t) => t.content,
            None => &[],
        }
    }
}

/// The `index`th TLV in a SET's or SEQUENCE's contents.
pub(crate) fn nth(contents: &[u8], index: u32) -> Result<Tlv<'_>, u32> {
    let mut d = Der::new(contents);
    let mut i = 0;
    while d.peek_tag().is_some() {
        let t = d.next()?;
        if i == index {
            return Ok(t);
        }
        i += 1;
    }
    Err(CRYPT_E_INVALID_INDEX)
}

/// The number of TLVs in a SET's or SEQUENCE's contents.
pub(crate) fn count(contents: &[u8]) -> Result<u32, u32> {
    let mut d = Der::new(contents);
    let mut n = 0;
    while d.peek_tag().is_some() {
        d.next()?;
        n += 1;
    }
    Ok(n)
}

/// A SignerInfo.
pub(crate) struct Signer<'a> {
    pub whole: &'a [u8],
    pub version: u32,
    /// issuerAndSerialNumber: the issuer Name's TLV and the serial
    /// number's contents (big-endian); empty for a key identifier.
    pub issuer: &'a [u8],
    pub serial: &'a [u8],
    /// subjectKeyIdentifier (CMS version 3).
    pub key_id: Option<&'a [u8]>,
    pub digest: (&'a [u8], &'a [u8]),
    /// The signed attributes: their [0] TLV.
    pub auth: Option<Tlv<'a>>,
    pub sig_alg: (&'a [u8], &'a [u8]),
    pub signature: &'a [u8],
    /// The unsigned attributes' contents.
    pub unauth: Option<&'a [u8]>,
}

pub(crate) fn parse_signer(t: Tlv<'_>) -> Result<Signer<'_>, u32> {
    if t.tag != SEQUENCE {
        return Err(CRYPT_E_ASN1_BADTAG);
    }
    let mut d = Der::new(t.content);
    let version = d.expect(INTEGER)?.content.last().copied().unwrap_or(0).into();
    let sid = d.next()?;
    let (issuer, serial, key_id) = match sid.tag {
        SEQUENCE => {
            let mut s = Der::new(sid.content);
            let name = s.expect(SEQUENCE)?.whole;
            (name, s.expect(INTEGER)?.content, None)
        }
        0x80 => (&[][..], &[][..], Some(sid.content)),
        _ => return Err(CRYPT_E_ASN1_BADTAG),
    };
    let digest = algorithm_id(d.next()?)?;
    let auth = if d.peek_tag() == Some(0xa0) { Some(d.next()?) } else { None };
    let sig_alg = algorithm_id(d.next()?)?;
    let signature = d.expect(OCTET_STRING)?.content;
    let unauth = if d.peek_tag() == Some(0xa1) { Some(d.next()?.content) } else { None };
    Ok(Signer {
        whole: t.whole,
        version,
        issuer,
        serial,
        key_id,
        digest,
        auth,
        sig_alg,
        signature,
        unauth,
    })
}

/// The values of attribute `oid` (OID contents) in attributes' contents.
pub(crate) fn attribute<'a>(attrs: &'a [u8], oid: &[u8]) -> Option<&'a [u8]> {
    let mut d = Der::new(attrs);
    while d.peek_tag().is_some() {
        let a = d.expect(SEQUENCE).ok()?;
        let mut x = Der::new(a.content);
        let id = x.expect(OID).ok()?;
        if id.content == oid {
            return Some(x.expect(SET).ok()?.content);
        }
    }
    None
}

/// CRYPT_ATTRIBUTES of attributes' contents (each value's whole TLV).
pub(crate) fn build_attributes(w: &mut Out, contents: &[u8]) -> Result<Attributes, u32> {
    let n = count(contents)? as usize;
    let array: *mut Attribute =
        w.place(n * core::mem::size_of::<Attribute>(), core::mem::align_of::<Attribute>()).cast();
    let mut d = Der::new(contents);
    for i in 0..n {
        let a = d.expect(SEQUENCE)?;
        let mut x = Der::new(a.content);
        let oid = w.oid(x.expect(OID)?.content)?;
        let set = x.expect(SET)?.content;
        let values_n = count(set)? as usize;
        let values: *mut Blob =
            w.place(values_n * core::mem::size_of::<Blob>(), core::mem::align_of::<Blob>()).cast();
        let mut v = Der::new(set);
        for j in 0..values_n {
            let value = v.next()?;
            let blob = w.blob(value.whole);
            // SAFETY: within the array placed above (null while measuring).
            put(if values.is_null() { values } else { unsafe { values.add(j) } }, blob);
        }
        let attr = Attribute { oid, count: values_n as u32, values };
        // SAFETY: as above.
        put(if array.is_null() { array } else { unsafe { array.add(i) } }, attr);
    }
    Ok(Attributes { count: n as u32, attrs: if n == 0 { core::ptr::null_mut() } else { array } })
}

/// CMSG_SIGNER_INFO of a SignerInfo (also CryptDecodeObjectEx's
/// PKCS7_SIGNER_INFO).
pub(crate) fn build_signer_info(w: &mut Out, s: &Signer<'_>) -> Result<(), u32> {
    if s.key_id.is_some() {
        unsupported(b"crypt32.dll!CMSG_SIGNER_INFO(signer by key identifier)", 0);
    }
    let p = w.slot::<SignerInfo>();
    let issuer = w.blob(s.issuer);
    let serial = w.reversed(s.serial);
    let hash_algorithm = w.algorithm(s.digest)?;
    let hash_encryption = w.algorithm(s.sig_alg)?;
    let encrypted_hash = w.blob(s.signature);
    let auth = build_attributes(w, s.auth.map_or(&[][..], |t| t.content))?;
    let unauth = build_attributes(w, s.unauth.unwrap_or(&[]))?;
    put(
        p,
        SignerInfo {
            version: s.version,
            issuer,
            serial,
            hash_algorithm,
            hash_encryption,
            encrypted_hash,
            auth,
            unauth,
        },
    );
    Ok(())
}

/// Whether the signer is the certificate with issuer `issuer` (encoded
/// Name) and serial number `serial` (little-endian, as CERT_INFO's).
fn signer_is(s: &Signer<'_>, issuer: &[u8], serial: &[u8]) -> bool {
    s.issuer == issuer && s.serial.len() == serial.len() && s.serial.iter().rev().eq(serial.iter())
}

/// Checks a signer's signature with the public key `key`: the signed
/// attributes' messageDigest against the content's digest, then the
/// signature over the attributes (as a SET), or over the content's digest
/// when there are none. The failing step's error: CRYPT_E_HASH_VALUE for
/// a digest that does not match, NTE_BAD_SIGNATURE for a signature that
/// does not verify, NTE_BAD_ALGID for an algorithm gala lacks.
pub(crate) fn verify_signer(
    content: &[u8],
    s: &Signer<'_>,
    key: *const PublicKeyInfo,
) -> Result<(), u32> {
    let Some((alg, len, name)) = digest_algorithm(s.digest.0) else {
        unsupported(b"crypt32.dll!CryptMsgControl(digest algorithm)", 0);
        return Err(NTE_BAD_ALGID);
    };
    let Some(ecdsa) = signer_scheme(s.sig_alg.0) else {
        unsupported(b"crypt32.dll!CryptMsgControl(signature algorithm)", 0);
        return Err(NTE_BAD_ALGID);
    };
    let mut digest = [0u8; 64];
    hash_with(alg, content, &mut digest[..len]).map_err(|_| NTE_BAD_ALGID)?;
    if let Some(auth) = s.auth {
        let value = attribute(auth.content, MESSAGE_DIGEST).ok_or(CRYPT_E_AUTH_ATTR_MISSING)?;
        let want = Der::new(value).expect(OCTET_STRING)?.content;
        if want != &digest[..len] {
            return Err(CRYPT_E_HASH_VALUE);
        }
        // The signature covers the attributes' DER with a SET's tag.
        let n = auth.whole.len();
        // SAFETY: plain allocation.
        let copy = unsafe { HeapAlloc(GetProcessHeap(), 0, n.max(1)) };
        if copy.is_null() {
            return Err(ERROR_NOT_ENOUGH_MEMORY);
        }
        // SAFETY: our block of `n` bytes.
        let hashed = unsafe {
            core::ptr::copy_nonoverlapping(auth.whole.as_ptr(), copy, n);
            *copy = SET;
            let r = hash_with(alg, core::slice::from_raw_parts(copy, n), &mut digest[..len]);
            HeapFree(GetProcessHeap(), 0, copy);
            r
        };
        hashed.map_err(|_| NTE_BAD_ALGID)?;
    }
    match verify_hashed(key, ecdsa, name, &digest[..len], s.signature) {
        Ok(()) => Ok(()),
        Err(f) if f.step == b"BCryptVerifySignature" => Err(NTE_BAD_SIGNATURE),
        Err(f) => {
            unsupported(
                b"crypt32.dll!CryptMsgControl(signature check could not run)",
                f.status.into(),
            );
            Err(NTE_BAD_ALGID)
        }
    }
}

// ---- The handle ----

const MSG_MAGIC: u32 = 0x6761_6c6d; // "galm"

#[repr(C)]
struct Msg {
    magic: u32,
    refs: AtomicU32,
    /// CMSG_SIGNED or CMSG_DATA as opened (0: from the ContentInfo).
    opened_as: u32,
    /// The type found by the final update.
    kind: u32,
    done: bool,
    data: *mut u8,
    len: usize,
    cap: usize,
}

fn msg<'a>(h: *mut c_void) -> Option<&'a mut Msg> {
    if h.is_null() {
        return None;
    }
    // SAFETY: a handle CryptMsgOpenToDecode returned starts with our
    // header, whose magic we check.
    let m = unsafe { &mut *h.cast::<Msg>() };
    (m.magic == MSG_MAGIC).then_some(m)
}

impl Msg {
    fn bytes(&self) -> &[u8] {
        if self.data.is_null() {
            return &[];
        }
        // SAFETY: our block holding `len` bytes.
        unsafe { core::slice::from_raw_parts(self.data, self.len) }
    }
}

/// The SignedData of a decoded message handle (CRYPT_E_INVALID_MSG_TYPE
/// before its final update or for another type).
pub(crate) fn signed_of<'a>(h: *mut c_void) -> Result<Signed<'a>, u32> {
    let m = msg(h).ok_or(E_INVALIDARG)?;
    if !m.done || m.kind != CMSG_SIGNED {
        return Err(CRYPT_E_INVALID_MSG_TYPE);
    }
    // SAFETY: the message lives while the caller holds its handle; its
    // encoding does not change after the final update.
    let bytes: &'a [u8] = unsafe { core::slice::from_raw_parts(m.data, m.len) };
    parse_signed(bytes)
}

/// The encoded certificates of a message (for CERT_STORE_PROV_MSG).
pub(crate) fn certificates<'a>(h: *mut c_void) -> Result<&'a [u8], u32> {
    Ok(signed_of(h)?.certs)
}

/// A new message handle holding a copy of `der`, decoded.
pub(crate) fn decode(der: &[u8]) -> *mut c_void {
    let h = CryptMsgOpenToDecode(0x1_0001, 0, 0, 0, core::ptr::null(), core::ptr::null());
    if h.is_null() {
        return h;
    }
    if CryptMsgUpdate(h, der.as_ptr(), der.len() as u32, 1) == 0 {
        CryptMsgClose(h);
        return core::ptr::null_mut();
    }
    h
}

/// CryptMsgOpenToDecode(encoding, flags, type, prov, recipient, stream):
/// a message to decode, of type `type` (CMSG_SIGNED, CMSG_DATA, or 0: from
/// the encoding's ContentInfo). Streaming, detached content and the other
/// types are recorded and refused.
#[unsafe(no_mangle)]
pub extern "system" fn CryptMsgOpenToDecode(
    encoding: u32,
    flags: u32,
    kind: u32,
    _prov: usize,
    _recipient: *const c_void,
    stream: *const c_void,
) -> *mut c_void {
    // PKCS_7_ASN_ENCODING.
    if encoding & 0x1_0000 == 0 {
        return error(E_INVALIDARG, core::ptr::null_mut());
    }
    if !stream.is_null() {
        unsupported(b"crypt32.dll!CryptMsgOpenToDecode(streamed)", 0);
        return error(E_INVALIDARG, core::ptr::null_mut());
    }
    if flags & 4 != 0 {
        unsupported(b"crypt32.dll!CryptMsgOpenToDecode(CMSG_DETACHED_FLAG)", 0);
    }
    if !matches!(kind, 0 | CMSG_DATA | CMSG_SIGNED) {
        unsupported(b"crypt32.dll!CryptMsgOpenToDecode(type)", kind.into());
        return error(CRYPT_E_INVALID_MSG_TYPE, core::ptr::null_mut());
    }
    // SAFETY: plain allocation, zeroed.
    let p = unsafe { HeapAlloc(GetProcessHeap(), 8, core::mem::size_of::<Msg>()) }.cast::<Msg>();
    if p.is_null() {
        return error(ERROR_NOT_ENOUGH_MEMORY, core::ptr::null_mut());
    }
    // SAFETY: our fresh block.
    unsafe {
        p.write(Msg {
            magic: MSG_MAGIC,
            refs: AtomicU32::new(1),
            opened_as: kind,
            kind: 0,
            done: false,
            data: core::ptr::null_mut(),
            len: 0,
            cap: 0,
        })
    };
    p.cast()
}

/// CryptMsgUpdate: appends the encoding; the final update decodes it
/// (CRYPT_E_ASN1_* when it does not, CRYPT_E_INVALID_MSG_TYPE for a type
/// not opened for). An update after the final one is CRYPT_E_MSG_ERROR.
#[unsafe(no_mangle)]
pub extern "system" fn CryptMsgUpdate(h: *mut c_void, data: *const u8, len: u32, last: i32) -> i32 {
    let Some(m) = msg(h) else { return error(E_INVALIDARG, 0) };
    if m.done {
        return error(CRYPT_E_MSG_ERROR, 0);
    }
    let len = len as usize;
    if len > 0 {
        if data.is_null() {
            return error(E_INVALIDARG, 0);
        }
        let need = m.len + len;
        if need > m.cap {
            let cap = need.max(m.cap * 2);
            // SAFETY: plain (re)allocation of our block.
            let p = unsafe {
                if m.data.is_null() {
                    HeapAlloc(GetProcessHeap(), 0, cap)
                } else {
                    HeapReAlloc(GetProcessHeap(), 0, m.data, cap)
                }
            };
            if p.is_null() {
                return error(ERROR_NOT_ENOUGH_MEMORY, 0);
            }
            m.data = p;
            m.cap = cap;
        }
        // SAFETY: room for `len` more bytes; the caller's buffer.
        unsafe { core::ptr::copy_nonoverlapping(data, m.data.add(m.len), len) };
        m.len += len;
    }
    if last == 0 {
        return 1;
    }
    let bytes = m.bytes();
    let kind = match m.opened_as {
        CMSG_DATA => Ok(CMSG_DATA),
        CMSG_SIGNED => parse_signed(bytes).map(|_| CMSG_SIGNED),
        _ => content_type(bytes),
    };
    match kind {
        Ok(k) => {
            m.kind = k;
            m.done = true;
            1
        }
        Err(e) => error(e, 0),
    }
}

/// The type of a ContentInfo: signedData (checked to decode) or data.
fn content_type(der: &[u8]) -> Result<u32, u32> {
    let outer = Der::new(der).expect(SEQUENCE)?;
    let oid = Der::new(outer.content).expect(OID)?;
    if oid.content == SIGNED_DATA {
        parse_signed(der).map(|_| CMSG_SIGNED)
    } else if oid.content == DATA {
        Ok(CMSG_DATA)
    } else {
        unsupported(b"crypt32.dll!CryptMsgUpdate(content type)", 0);
        Err(CRYPT_E_INVALID_MSG_TYPE)
    }
}

/// A data message's content: the ContentInfo's [0] OCTET STRING, or the
/// bare OCTET STRING of one opened as CMSG_DATA.
fn data_content(m: &Msg) -> Result<&[u8], u32> {
    let bytes = m.bytes();
    let t = Der::new(bytes).next()?;
    if t.tag == OCTET_STRING {
        return Ok(t.content);
    }
    let mut d = Der::new(t.content);
    d.expect(OID)?;
    let explicit = d.expect(0xa0)?;
    Ok(Der::new(explicit.content).expect(OCTET_STRING)?.content)
}

/// CryptMsgGetParam(msg, param, index, data, size): of a SignedData the
/// type, content, inner content type, counts, signers (CMSG_SIGNER_INFO,
/// its CERT_INFO, hash algorithm, attributes, encoding), certificates,
/// CRLs, the whole encoding and the version; of a Data message its type
/// and content. Others are recorded and E_INVALIDARG.
#[unsafe(no_mangle)]
pub extern "system" fn CryptMsgGetParam(
    h: *mut c_void,
    param: u32,
    index: u32,
    out: *mut c_void,
    len: *mut u32,
) -> i32 {
    let Some(m) = msg(h) else { return error(E_INVALIDARG, 0) };
    if !m.done {
        return error(CRYPT_E_INVALID_MSG_TYPE, 0);
    }
    if m.kind == CMSG_DATA {
        return match param {
            CMSG_TYPE_PARAM => give_u32(CMSG_DATA, out, len),
            CMSG_CONTENT_PARAM => match data_content(m) {
                Ok(c) => give_bytes(c, out, len),
                Err(e) => error(e, 0),
            },
            CMSG_ENCODED_MESSAGE => give_bytes(m.bytes(), out, len),
            _ => {
                unsupported(b"crypt32.dll!CryptMsgGetParam(data message)", param.into());
                error(CRYPT_E_INVALID_MSG_TYPE, 0)
            }
        };
    }
    let s = match signed_of(h) {
        Ok(s) => s,
        Err(e) => return error(e, 0),
    };
    let signer = |index: u32| nth(s.signers, index).and_then(parse_signer);
    let r: Result<i32, u32> = (|| {
        Ok(match param {
            CMSG_TYPE_PARAM => give_u32(CMSG_SIGNED, out, len),
            CMSG_CONTENT_PARAM => give_bytes(s.content_param(), out, len),
            CMSG_INNER_CONTENT_TYPE_PARAM => give_built(out, len, |w| {
                w.oid(s.content_type)?;
                Ok(())
            }),
            CMSG_SIGNER_COUNT_PARAM => give_u32(count(s.signers)?, out, len),
            CMSG_SIGNER_INFO_PARAM => {
                let sg = signer(index)?;
                give_built(out, len, |w| build_signer_info(w, &sg))
            }
            CMSG_SIGNER_CERT_INFO_PARAM => {
                let sg = signer(index)?;
                give_built(out, len, |w| {
                    let p = w.slot::<CertInfo>();
                    let issuer = w.blob(sg.issuer);
                    let serial = w.reversed(sg.serial);
                    if !p.is_null() {
                        // SAFETY: the zeroed CERT_INFO placed above.
                        unsafe {
                            core::ptr::addr_of_mut!((*p).issuer).write_unaligned(issuer);
                            core::ptr::addr_of_mut!((*p).serial).write_unaligned(serial);
                        }
                    }
                    Ok(())
                })
            }
            CMSG_SIGNER_HASH_ALGORITHM_PARAM => {
                let sg = signer(index)?;
                give_built(out, len, |w| {
                    let p = w.slot::<Algorithm>();
                    let alg = w.algorithm(sg.digest)?;
                    put(p, alg);
                    Ok(())
                })
            }
            CMSG_SIGNER_AUTH_ATTR_PARAM | CMSG_SIGNER_UNAUTH_ATTR_PARAM => {
                let sg = signer(index)?;
                let attrs = if param == CMSG_SIGNER_AUTH_ATTR_PARAM {
                    sg.auth.map(|t| t.content)
                } else {
                    sg.unauth
                };
                // No attributes of the kind: CRYPT_E_ATTRIBUTES_MISSING.
                let attrs = attrs.ok_or(0x8009_100f_u32)?;
                give_built(out, len, |w| {
                    let p = w.slot::<Attributes>();
                    let a = build_attributes(w, attrs)?;
                    put(p, a);
                    Ok(())
                })
            }
            CMSG_ENCODED_SIGNER => give_bytes(signer(index)?.whole, out, len),
            CMSG_CERT_COUNT_PARAM => give_u32(count(s.certs)?, out, len),
            CMSG_CERT_PARAM => give_bytes(nth(s.certs, index)?.whole, out, len),
            CMSG_CRL_COUNT_PARAM => give_u32(count(s.crls)?, out, len),
            CMSG_CRL_PARAM => give_bytes(nth(s.crls, index)?.whole, out, len),
            CMSG_ENCODED_MESSAGE => give_bytes(m.bytes(), out, len),
            CMSG_VERSION_PARAM => give_u32(s.version, out, len),
            _ => {
                unsupported(b"crypt32.dll!CryptMsgGetParam", param.into());
                error(E_INVALIDARG, 0)
            }
        })
    })();
    r.unwrap_or_else(|e| error(e, 0))
}

/// CryptMsgControl(msg, flags, control, para): CMSG_CTRL_VERIFY_SIGNATURE
/// (the signer whose issuer and serial number are those of the CERT_INFO
/// given, with its public key; CRYPT_E_SIGNER_NOT_FOUND if none is) and
/// CMSG_CTRL_VERIFY_SIGNATURE_EX (a signer by index, with a public key or
/// a certificate's); `verify_signer`'s errors. Other controls are
/// recorded and CRYPT_E_CONTROL_TYPE.
#[unsafe(no_mangle)]
pub extern "system" fn CryptMsgControl(
    h: *mut c_void,
    _flags: u32,
    control: u32,
    para: *const c_void,
) -> i32 {
    let s = match signed_of(h) {
        Ok(s) => s,
        Err(e) => return error(e, 0),
    };
    if para.is_null() {
        return error(E_INVALIDARG, 0);
    }
    let r = match control {
        CMSG_CTRL_VERIFY_SIGNATURE => (|| {
            // SAFETY: the caller's CERT_INFO.
            let info = unsafe { &*para.cast::<CertInfo>() };
            // SAFETY: its blobs.
            let (issuer, serial) = unsafe {
                (
                    core::slice::from_raw_parts(info.issuer.data, info.issuer.size as usize),
                    core::slice::from_raw_parts(info.serial.data, info.serial.size as usize),
                )
            };
            let mut d = Der::new(s.signers);
            while d.peek_tag().is_some() {
                let sg = parse_signer(d.next()?)?;
                if signer_is(&sg, issuer, serial) {
                    return verify_signer(s.digested(), &sg, &info.public_key);
                }
            }
            Err(CRYPT_E_SIGNER_NOT_FOUND)
        })(),
        CMSG_CTRL_VERIFY_SIGNATURE_EX => (|| {
            let p = para.cast::<VerifyExPara>();
            // SAFETY: the caller's structure.
            let (index, kind, signer) =
                unsafe { ((*p).signer_index, (*p).signer_type, (*p).signer) };
            let sg = parse_signer(nth(s.signers, index)?)?;
            let key: *const PublicKeyInfo = match kind {
                CMSG_VERIFY_SIGNER_PUBKEY => signer.cast(),
                CMSG_VERIFY_SIGNER_CERT => {
                    let c = signer.cast::<CertContext>();
                    if c.is_null() {
                        return Err(E_INVALIDARG);
                    }
                    // SAFETY: the caller's certificate context.
                    unsafe { &raw const (*(*c).info).public_key }
                }
                _ => {
                    unsupported(
                        b"crypt32.dll!CryptMsgControl(VERIFY_SIGNATURE_EX signer)",
                        kind.into(),
                    );
                    return Err(E_INVALIDARG);
                }
            };
            if key.is_null() {
                return Err(E_INVALIDARG);
            }
            verify_signer(s.digested(), &sg, key)
        })(),
        _ => {
            unsupported(b"crypt32.dll!CryptMsgControl", control.into());
            Err(CRYPT_E_CONTROL_TYPE)
        }
    };
    match r {
        Ok(()) => 1,
        Err(e) => error(e, 0),
    }
}

/// CryptMsgDuplicate: one more reference to the same message.
#[unsafe(no_mangle)]
pub extern "system" fn CryptMsgDuplicate(h: *mut c_void) -> *mut c_void {
    if let Some(m) = msg(h) {
        m.refs.fetch_add(1, Ordering::AcqRel);
    }
    h
}

/// CryptMsgClose: one reference less; the last frees the message. NULL
/// is TRUE.
#[unsafe(no_mangle)]
pub extern "system" fn CryptMsgClose(h: *mut c_void) -> i32 {
    if h.is_null() {
        return 1;
    }
    let Some(m) = msg(h) else {
        unsupported(b"crypt32.dll!CryptMsgClose(a message gala did not make)", 0);
        return 1;
    };
    if m.refs.fetch_sub(1, Ordering::AcqRel) != 1 {
        return 1;
    }
    m.magic = 0;
    // SAFETY: our blocks.
    unsafe {
        if !m.data.is_null() {
            HeapFree(GetProcessHeap(), 0, m.data);
        }
        HeapFree(GetProcessHeap(), 0, h.cast());
    }
    1
}

/// The certificate in `store` whose issuer and serial number are the
/// signer's (one more reference), or null.
pub(crate) fn signer_certificate(store: *mut c_void, s: &Signer<'_>) -> *const CertContext {
    let mut c = core::ptr::null();
    loop {
        c = crate::store::CertEnumCertificatesInStore(store, c);
        if c.is_null() {
            return c;
        }
        // SAFETY: a context of ours.
        let info = unsafe { &*(*c).info };
        // SAFETY: its blobs.
        let (issuer, serial) = unsafe {
            (
                core::slice::from_raw_parts(info.issuer.data, info.issuer.size as usize),
                core::slice::from_raw_parts(info.serial.data, info.serial.size as usize),
            )
        };
        let by_key =
            s.key_id.is_some_and(|k| crate::cert::subject_key_id(info).is_some_and(|id| id == k));
        if by_key || (s.key_id.is_none() && signer_is(s, issuer, serial)) {
            return c;
        }
    }
}

/// A BOOLEAN's value (for DER readers elsewhere).
pub(crate) fn boolean(t: Tlv<'_>) -> Result<bool, u32> {
    if t.tag != BOOLEAN {
        return Err(CRYPT_E_ASN1_BADTAG);
    }
    t.content.first().map(|&b| b != 0).ok_or(CRYPT_E_ASN1_CORRUPT)
}

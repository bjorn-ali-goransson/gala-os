//! The certificate chain engine, as Microsoft documents it
//! (CertCreateCertificateChainEngine, CertGetCertificateChain,
//! CERT_CHAIN_CONTEXT, CERT_TRUST_STATUS, CertVerifyCertificateChainPolicy,
//! CERT_CHAIN_POLICY_STATUS), with the element each error is reported on
//! and the order the policies test them taken from Wine's conformance
//! tests (dlls/crypt32/tests/chain.c, checked on Windows; THIRD_PARTY.md).
//! Tests: `cert_test` (chain.rs), `schannel_test`, curl's `--cacert`.
//!
//! - **Engines**: the default engine (HCCE_CURRENT_USER, null, and
//!   HCCE_LOCAL_MACHINE: gala has one machine root store) trusts the ROOT
//!   system store (gala's machine roots, System32\gala\roots) and finds
//!   issuers there and in the CA and MY stores; it is made once per
//!   process. CertCreateCertificateChainEngine's engines trust
//!   hExclusiveRoot (curl's `--cacert`), else hRestrictedRoot, else ROOT,
//!   and search their rghAdditionalStore too.
//! - **Building** (`build`): from the end certificate, each issuer is
//!   looked up by its subject name in hAdditionalStore and the engine's
//!   stores; of several candidates the one whose key the authority key
//!   identifier names, whose signature verifies, that is trusted and time
//!   valid wins. The chain ends at a self-signed certificate (subject =
//!   issuer, and its authority key identifier, if any, is its own key);
//!   no issuer gives CERT_TRUST_IS_PARTIAL_CHAIN (on the chain, not an
//!   element), as do cycles and more than `MAX_ELEMENTS`.
//! - **Checks**: validity at pTime (or now) on every element; each
//!   signature by the next element's key (CERT_TRUST_IS_NOT_SIGNATURE_VALID
//!   on the signed one; RSA and ECDSA P-256/P-384 through bcrypt); the
//!   root's presence in the trusted store (CERT_TRUST_IS_UNTRUSTED_ROOT);
//!   basic constraints (CA, path length; v1/v2 certificates count as CAs
//!   only from the engine's own stores) and keyCertSign; critical
//!   extensions gala does not know; RequestedUsage against the end
//!   certificate's extended key usage (AND/OR). Name constraints are not
//!   evaluated: a CA that has them gets CERT_TRUST_INVALID_NAME_CONSTRAINTS
//!   | CERT_TRUST_HAS_NOT_SUPPORTED_NAME_CONSTRAINT (fails closed,
//!   recorded; ROADMAP TLS). Revocation, with a
//!   CERT_CHAIN_REVOCATION_CHECK_* flag (END_CERT, CHAIN,
//!   CHAIN_EXCLUDE_ROOT; never a self-signed root), is checked offline
//!   from the issuer's CRLs in hAdditionalStore and the engine's stores
//!   (crl.rs `check`; CERT_CHAIN_REVOCATION_CHECK_CACHE_ONLY changes
//!   nothing, nothing is fetched): CERT_TRUST_IS_REVOKED when a current
//!   one lists the certificate, CERT_TRUST_REVOCATION_STATUS_UNKNOWN |
//!   CERT_TRUST_IS_OFFLINE_REVOCATION when they are out of date or the
//!   CRL or OCSP responder the certificate names is not there,
//!   REVOCATION_STATUS_UNKNOWN alone when there is nothing to check it
//!   by; the policies then give CRYPT_E_REVOKED,
//!   CRYPT_E_REVOCATION_OFFLINE or CRYPT_E_NO_REVOCATION_CHECK.
//!   pRevocationInfo stays NULL.
//! - **Policies**: BASE, SSL (with SSL_EXTRA_CERT_CHAIN_POLICY_PARA: the
//!   server name against the DNS and IP subject alternative names, else
//!   the subject's CN; one-label wildcards), BASIC_CONSTRAINTS; the others
//!   are checked as BASE (recorded). A failed policy is recorded in the
//!   ledger as `crypt32.dll!<policy>: <error>` (kind "other") with the
//!   chain's error bits as detail: why a program refused a certificate.
//!
//! Layouts (x64) as wincrypt.h gives them: CERT_CHAIN_CONTEXT (72
//! bytes), CERT_SIMPLE_CHAIN (40), CERT_CHAIN_ELEMENT (56),
//! CERT_CHAIN_PARA (32 without its later fields, 96), CERT_CHAIN_ENGINE_CONFIG
//! (64, 80 or 88), CERT_CHAIN_POLICY_STATUS (24).

use core::ffi::c_void;
use core::sync::atomic::{AtomicPtr, AtomicU32, Ordering};

use super::cert::{
    BIT_STRING, BOOLEAN, CertContext, CertDuplicateCertificateContext, CertFreeCertificateContext,
    CertGetCertificateContextProperty, CertInfo, CertVerifyTimeValidity,
    CryptVerifyCertificateSignatureEx, Der, INTEGER, OID, SEQUENCE, SET, c_text, extension_value,
    oid_text,
};
use super::store::{
    CertAddStoreToCollection, CertCloseStore, CertDuplicateStore, CertFindCertificateInStore,
    CertOpenStore, CertOpenSystemStoreA, unsupported,
};
use super::{SetLastError, crl};

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetProcessHeap() -> usize;
    fn HeapAlloc(heap: usize, flags: u32, size: usize) -> *mut u8;
    fn HeapFree(heap: usize, flags: u32, p: *mut u8) -> i32;
}

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn GalaRecordMissing(name: *const u8, length: usize, what: u32, detail: u64) -> u32;
}

const E_INVALIDARG: u32 = 0x8007_0057;
const ERROR_NOT_ENOUGH_MEMORY: u32 = 8;
const CRYPT_E_NOT_FOUND: u32 = 0x8009_2004;

// CERT_TRUST_STATUS dwErrorStatus.
const IS_NOT_TIME_VALID: u32 = 0x1;
const IS_REVOKED: u32 = 0x4;
const IS_NOT_SIGNATURE_VALID: u32 = 0x8;
const IS_NOT_VALID_FOR_USAGE: u32 = 0x10;
const IS_UNTRUSTED_ROOT: u32 = 0x20;
const REVOCATION_STATUS_UNKNOWN: u32 = 0x40;
const IS_CYCLIC: u32 = 0x80;
const INVALID_EXTENSION: u32 = 0x100;
const INVALID_BASIC_CONSTRAINTS: u32 = 0x400;
const INVALID_NAME_CONSTRAINTS: u32 = 0x800;
const HAS_NOT_SUPPORTED_NAME_CONSTRAINT: u32 = 0x1000;
const NAME_CONSTRAINT_ERRORS: u32 = 0xf800;
const IS_PARTIAL_CHAIN: u32 = 0x1_0000;
const IS_OFFLINE_REVOCATION: u32 = 0x100_0000;
const HAS_NOT_SUPPORTED_CRITICAL_EXT: u32 = 0x800_0000;

// dwInfoStatus.
const HAS_KEY_MATCH_ISSUER: u32 = 0x2;
const HAS_NAME_MATCH_ISSUER: u32 = 0x4;
const IS_SELF_SIGNED: u32 = 0x8;
const HAS_PREFERRED_ISSUER: u32 = 0x100;
const IS_FROM_EXCLUSIVE_TRUST_STORE: u32 = 0x2000;

// CertGetCertificateChain dwFlags.
const REVOCATION_CHECK_END_CERT: u32 = 0x1000_0000;
const REVOCATION_CHECK_CHAIN: u32 = 0x2000_0000;
const REVOCATION_CHECK_CHAIN_EXCLUDE_ROOT: u32 = 0x4000_0000;

/// The most elements a chain has (deeper ones are partial).
const MAX_ELEMENTS: usize = 8;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct TrustStatus {
    error: u32,
    info: u32,
}

/// CERT_CHAIN_ELEMENT.
#[repr(C)]
struct Element {
    size: u32,
    cert: *const CertContext,
    status: TrustStatus,
    revocation: *const c_void,
    issuance: *const c_void,
    application: *const c_void,
    extended: *const u16,
}

/// CERT_SIMPLE_CHAIN.
#[repr(C)]
struct SimpleChain {
    size: u32,
    status: TrustStatus,
    count: u32,
    elements: *const *const Element,
    trust_list: *const c_void,
    has_freshness: i32,
    freshness: u32,
}

/// CERT_CHAIN_CONTEXT.
#[repr(C)]
pub struct ChainContext {
    size: u32,
    status: TrustStatus,
    count: u32,
    chains: *const *const SimpleChain,
    lower_count: u32,
    lower: *const c_void,
    has_freshness: i32,
    freshness: u32,
    create_flags: u32,
    id: [u8; 16],
}

const CHAIN_MAGIC: u32 = 0x6761_6c6e; // "galn"

/// One heap block per chain: a reference count, then the documented
/// structures, pointing into the block.
#[repr(C)]
struct ChainBlock {
    magic: u32,
    refs: AtomicU32,
    context: ChainContext,
    chain_ptr: *const SimpleChain,
    simple: SimpleChain,
    element_ptrs: [*const Element; MAX_ELEMENTS],
    elements: [Element; MAX_ELEMENTS],
}

fn chain_block<'a>(p: *const ChainContext) -> Option<&'a mut ChainBlock> {
    if p.is_null() {
        return None;
    }
    let offset = core::mem::offset_of!(ChainBlock, context);
    let b = (p as usize).checked_sub(offset)? as *mut ChainBlock;
    // SAFETY: a chain the caller got from us starts `offset` bytes into
    // its block, whose magic we check.
    let b = unsafe { &mut *b };
    (b.magic == CHAIN_MAGIC).then_some(b)
}

// ---- Engines ----

const ENGINE_MAGIC: u32 = 0x6761_6c65 ^ 0x0100_0000; // "gale" variant

#[repr(C)]
struct Engine {
    magic: u32,
    refs: AtomicU32,
    /// The trusted roots.
    root: *mut c_void,
    /// Where issuers are found: the roots, CA, MY and the configuration's
    /// additional stores (a collection).
    world: *mut c_void,
    /// The roots are hExclusiveRoot.
    exclusive: bool,
}

/// The default engine (HCCE_CURRENT_USER, HCCE_LOCAL_MACHINE).
static DEFAULT: AtomicPtr<Engine> = AtomicPtr::new(core::ptr::null_mut());

const CERT_STORE_PROV_COLLECTION: usize = 11;

fn new_collection() -> *mut c_void {
    CertOpenStore(CERT_STORE_PROV_COLLECTION as *const u8, 0, 0, 0, core::ptr::null())
}

/// An engine trusting `root` (a reference it keeps), finding issuers also
/// in CA, MY and `additional`.
fn new_engine(root: *mut c_void, additional: &[*mut c_void], exclusive: bool) -> *mut Engine {
    let world = new_collection();
    if root.is_null() || world.is_null() {
        CertCloseStore(root, 0);
        CertCloseStore(world, 0);
        return core::ptr::null_mut();
    }
    CertAddStoreToCollection(world, root, 0, 0);
    for name in [c"CA", c"MY"] {
        let s = CertOpenSystemStoreA(0, name.as_ptr().cast());
        if !s.is_null() {
            CertAddStoreToCollection(world, s, 0, 0);
            CertCloseStore(s, 0);
        }
    }
    for &s in additional {
        if !s.is_null() {
            CertAddStoreToCollection(world, s, 0, 0);
        }
    }
    // SAFETY: plain allocation.
    let p =
        unsafe { HeapAlloc(GetProcessHeap(), 8, core::mem::size_of::<Engine>()) }.cast::<Engine>();
    if p.is_null() {
        CertCloseStore(root, 0);
        CertCloseStore(world, 0);
        return core::ptr::null_mut();
    }
    // SAFETY: a fresh block of the engine's size.
    unsafe {
        p.write(Engine { magic: ENGINE_MAGIC, refs: AtomicU32::new(1), root, world, exclusive })
    };
    p
}

fn free_engine(e: *mut Engine) {
    // SAFETY: an engine of ours.
    unsafe {
        if (*e).refs.fetch_sub(1, Ordering::AcqRel) != 1 {
            return;
        }
        (*e).magic = 0;
        CertCloseStore((*e).root, 0);
        CertCloseStore((*e).world, 0);
        HeapFree(GetProcessHeap(), 0, e.cast());
    }
}

/// The engine a handle names: the default one for HCCE_CURRENT_USER (0),
/// HCCE_LOCAL_MACHINE (1) and HCCE_SERIAL_LOCAL_MACHINE (2), made on
/// first use.
fn engine<'a>(h: *mut c_void) -> Option<&'a Engine> {
    if (h as usize) <= 2 {
        let mut e = DEFAULT.load(Ordering::Acquire);
        if e.is_null() {
            let made = new_engine(CertOpenSystemStoreA(0, c"ROOT".as_ptr().cast()), &[], false);
            if made.is_null() {
                return None;
            }
            match DEFAULT.compare_exchange(
                core::ptr::null_mut(),
                made,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => e = made,
                Err(other) => {
                    free_engine(made);
                    e = other;
                }
            }
        }
        // SAFETY: the default engine lives as long as the process.
        return Some(unsafe { &*e });
    }
    // SAFETY: an engine the caller got from us (magic checked).
    let e = unsafe { &*(h as *const Engine) };
    (e.magic == ENGINE_MAGIC).then_some(e)
}

/// CERT_CHAIN_ENGINE_CONFIG: cbSize, hRestrictedRoot (8),
/// hRestrictedTrust (16), hRestrictedOther (24), cAdditionalStore (32),
/// rghAdditionalStore (40), dwFlags (48), ..., hExclusiveRoot (64),
/// hExclusiveTrustedPeople (72), dwExclusiveFlags (80).
fn read<T: Copy>(p: *const u8, at: usize) -> T {
    // SAFETY: inside the caller's structure (its size checked).
    unsafe { p.add(at).cast::<T>().read_unaligned() }
}

/// CertCreateCertificateChainEngine: an engine whose roots are
/// hExclusiveRoot (CERT_CHAIN_ENGINE_CONFIG of 80 or 88 bytes), else
/// hRestrictedRoot, else the ROOT store, searching rghAdditionalStore for
/// issuers too. Other sizes are E_INVALIDARG; hRestrictedTrust,
/// hRestrictedOther, hExclusiveTrustedPeople and dwExclusiveFlags are
/// recorded and not used.
#[unsafe(no_mangle)]
pub extern "system" fn CertCreateCertificateChainEngine(
    config: *const u8,
    out: *mut *mut c_void,
) -> i32 {
    if config.is_null() || out.is_null() {
        // SAFETY: plain call.
        unsafe { SetLastError(E_INVALIDARG) };
        return 0;
    }
    let size: u32 = read(config, 0);
    if !matches!(size, 64 | 80 | 88) {
        // SAFETY: plain call.
        unsafe { SetLastError(E_INVALIDARG) };
        return 0;
    }
    let restricted_root: *mut c_void = read(config, 8);
    let others: [*mut c_void; 2] = [read(config, 16), read(config, 24)];
    let count: u32 = read(config, 32);
    let stores: *const *mut c_void = read(config, 40);
    let (exclusive_root, people, exclusive_flags): (*mut c_void, *mut c_void, u32) = (
        if size >= 80 { read(config, 64) } else { core::ptr::null_mut() },
        if size >= 80 { read(config, 72) } else { core::ptr::null_mut() },
        if size >= 88 { read(config, 80) } else { 0 },
    );
    if others.iter().any(|s| !s.is_null()) || !people.is_null() || exclusive_flags != 0 {
        unsupported(
            b"crypt32.dll!CertCreateCertificateChainEngine(trust stores)",
            exclusive_flags.into(),
        );
    }
    let mut additional = [core::ptr::null_mut(); 16];
    let n = (count as usize).min(additional.len());
    if !stores.is_null() {
        for (i, slot) in additional[..n].iter_mut().enumerate() {
            // SAFETY: the caller's array of cAdditionalStore handles.
            *slot = unsafe { *stores.add(i) };
        }
    }
    let (root, exclusive) = if !exclusive_root.is_null() {
        (CertDuplicateStore(exclusive_root), true)
    } else if !restricted_root.is_null() {
        (CertDuplicateStore(restricted_root), false)
    } else {
        (CertOpenSystemStoreA(0, c"ROOT".as_ptr().cast()), false)
    };
    let e = new_engine(root, &additional[..n], exclusive);
    if e.is_null() {
        // SAFETY: plain call.
        unsafe { SetLastError(ERROR_NOT_ENOUGH_MEMORY) };
        return 0;
    }
    // SAFETY: the caller's output.
    unsafe { *out = e.cast() };
    1
}

/// CertFreeCertificateChainEngine: an engine CertCreateCertificateChainEngine
/// made (the default engine stays).
#[unsafe(no_mangle)]
pub extern "system" fn CertFreeCertificateChainEngine(h: *mut c_void) {
    if (h as usize) <= 2 {
        return;
    }
    if engine(h).is_some() {
        free_engine(h.cast());
    }
}

/// CertResyncCertificateChainEngine: gala's stores change only in this
/// process, so there is nothing to read again.
#[unsafe(no_mangle)]
pub extern "system" fn CertResyncCertificateChainEngine(_h: *mut c_void) -> i32 {
    1
}

// ---- Certificates ----

fn info<'a>(c: *const CertContext) -> &'a CertInfo {
    // SAFETY: a context of ours.
    unsafe { &*(*c).info }
}

fn blob_bytes<'a>(b: &super::cert::Blob) -> &'a [u8] {
    if b.data.is_null() {
        return &[];
    }
    // SAFETY: a blob of a context, pointing into its encoding.
    unsafe { core::slice::from_raw_parts(b.data, b.size as usize) }
}

fn encoding<'a>(c: *const CertContext) -> &'a [u8] {
    // SAFETY: a context of ours.
    unsafe { core::slice::from_raw_parts((*c).encoded, (*c).encoded_len as usize) }
}

/// The authority key identifier's keyIdentifier ([0] in the extension's
/// SEQUENCE), if the certificate has one.
fn authority_key_id<'a>(c: *const CertContext) -> Option<&'a [u8]> {
    let value = extension_value(info(c), c"2.5.29.35")?;
    let seq = Der::new(value).expect(SEQUENCE).ok()?;
    let mut d = Der::new(seq.content);
    while let Ok(t) = d.next() {
        if t.tag == 0x80 {
            return Some(t.content);
        }
    }
    None
}

/// The certificate's key identifier (its subject key identifier, or the
/// SHA-1 of its public key).
fn key_id(c: *const CertContext, out: &mut [u8; 64]) -> usize {
    let mut n = out.len() as u32;
    if CertGetCertificateContextProperty(c, 20, out.as_mut_ptr(), &mut n) == 0 {
        return 0;
    }
    n as usize
}

/// Whether the certificate is self-signed (its subject is its issuer, and
/// an authority key identifier names its own key): the issuer match it
/// reports.
fn self_signed(c: *const CertContext) -> Option<u32> {
    let i = info(c);
    if blob_bytes(&i.subject) != blob_bytes(&i.issuer) {
        return None;
    }
    match authority_key_id(c) {
        None => Some(HAS_NAME_MATCH_ISSUER | IS_SELF_SIGNED),
        Some(aki) => {
            let mut own = [0u8; 64];
            let n = key_id(c, &mut own);
            (aki == &own[..n]).then_some(HAS_KEY_MATCH_ISSUER | IS_SELF_SIGNED)
        }
    }
}

/// Whether `store` holds the certificate.
fn holds(store: *mut c_void, c: *const CertContext) -> bool {
    const CERT_FIND_EXISTING: u32 = 0xd_0000;
    let found =
        CertFindCertificateInStore(store, 1, 0, CERT_FIND_EXISTING, c.cast(), core::ptr::null());
    if found.is_null() {
        return false;
    }
    CertFreeCertificateContext(found);
    true
}

/// Whether `subject`'s signature verifies with `issuer`'s key.
fn signed_by(subject: *const CertContext, issuer: *const CertContext) -> bool {
    let ok = CryptVerifyCertificateSignatureEx(
        0,
        1,
        2, // CRYPT_VERIFY_CERT_SIGN_SUBJECT_CERT
        subject.cast(),
        2, // CRYPT_VERIFY_CERT_SIGN_ISSUER_CERT
        issuer.cast(),
        0,
        core::ptr::null(),
    );
    ok != 0
}

/// Basic constraints: Ok(None) without the extension, Ok(Some((cA,
/// pathLenConstraint))), Err for one that does not decode.
#[allow(clippy::type_complexity)]
fn basic_constraints(c: *const CertContext) -> Result<Option<(bool, Option<u32>)>, ()> {
    let Some(value) = extension_value(info(c), c"2.5.29.19") else { return Ok(None) };
    let seq = Der::new(value).expect(SEQUENCE).map_err(|_| ())?;
    let mut d = Der::new(seq.content);
    let mut ca = false;
    let mut path = None;
    if d.peek_tag() == Some(BOOLEAN) {
        ca = d.next().map_err(|_| ())?.content.first().is_some_and(|&b| b != 0);
    }
    if d.peek_tag() == Some(INTEGER) {
        let n = d.next().map_err(|_| ())?.content;
        if n.len() > 4 {
            return Err(());
        }
        path = Some(n.iter().fold(0u32, |a, &b| (a << 8) | u32::from(b)));
    }
    Ok(Some((ca, path)))
}

/// The key usage extension's first byte, if the certificate has one.
fn key_usage(c: *const CertContext) -> Option<u8> {
    let value = extension_value(info(c), c"2.5.29.15")?;
    let bits = Der::new(value).expect(BIT_STRING).ok()?;
    Some(bits.content.get(1).copied().unwrap_or(0))
}

/// keyCertSign in the key usage's first byte.
const KEY_CERT_SIGN: u8 = 0x04;

/// The critical extensions gala knows (Wine's list, as Windows checks
/// them, with policy constraints and inhibitAnyPolicy, which only matter
/// to issuance policies gala does not request).
const KNOWN_CRITICAL: [&[u8]; 11] = [
    b"2.5.29.19",
    b"2.5.29.10",
    b"2.5.29.30",
    b"2.5.29.15",
    b"2.5.29.17",
    b"2.5.29.7",
    b"2.5.29.32",
    b"1.3.6.1.4.1.311.21.10",
    b"2.5.29.37",
    b"2.5.29.36",
    b"2.5.29.54",
];

/// Whether every critical extension of the certificate is known.
fn critical_known(c: *const CertContext) -> bool {
    let i = info(c);
    (0..i.extension_count as usize).all(|k| {
        // SAFETY: the context's extension array.
        let e = unsafe { &*i.extensions.add(k) };
        // SAFETY: the extension's OID string.
        e.critical == 0 || KNOWN_CRITICAL.contains(&unsafe { c_text(e.oid) })
    })
}

/// The end certificate's extended key usages as dotted OIDs, compared
/// with the requested ones: AND (every one present) or OR (any).
fn usage_ok(c: *const CertContext, requested: &[&[u8]], and: bool) -> bool {
    let Some(value) = extension_value(info(c), c"2.5.29.37") else {
        return true; // no extension: every usage
    };
    let Ok(seq) = Der::new(value).expect(SEQUENCE) else { return false };
    let has = |want: &[u8]| {
        let mut d = Der::new(seq.content);
        while let Ok(t) = d.expect(OID) {
            let mut text = [0u8; 64];
            if let Ok(n) = oid_text(t.content, &mut text)
                && &text[..n] == want
            {
                return true;
            }
        }
        false
    };
    if and { requested.iter().all(|r| has(r)) } else { requested.iter().any(|r| has(r)) }
}

// ---- Building ----

struct Built {
    certs: [*const CertContext; MAX_ELEMENTS],
    status: [TrustStatus; MAX_ELEMENTS],
    /// Element i's signature verifies with element i + 1's key.
    signed: [bool; MAX_ELEMENTS],
    n: usize,
    /// The chain-level errors (CERT_TRUST_IS_PARTIAL_CHAIN).
    chain_error: u32,
}

/// The best issuer of `subject` in `world` (one more reference), with the
/// match it was found by and whether `subject`'s signature verifies.
fn find_issuer(
    e: &Engine,
    world: *mut c_void,
    subject: *const CertContext,
    time: u64,
    chain: &[*const CertContext],
) -> Option<(*const CertContext, u32, bool)> {
    const CERT_FIND_SUBJECT_NAME: u32 = 0x2_0007;
    let issuer_name = &info(subject).issuer as *const super::cert::Blob;
    let aki = authority_key_id(subject);
    let mut best: Option<(*const CertContext, u32, bool, u32)> = None;
    let mut c = core::ptr::null();
    loop {
        // The enumeration frees the previous candidate.
        c = CertFindCertificateInStore(world, 1, 0, CERT_FIND_SUBJECT_NAME, issuer_name.cast(), c);
        if c.is_null() {
            break;
        }
        if chain.iter().any(|&x| encoding(x) == encoding(c)) {
            continue; // a cycle
        }
        let mut kid = [0u8; 64];
        let n = key_id(c, &mut kid);
        let key_match = aki.is_some_and(|a| a == &kid[..n]);
        let sig = signed_by(subject, c);
        let trusted = self_signed(c).is_some() && holds(e.root, c);
        let valid = CertVerifyTimeValidity(&time, info(c)) == 0;
        let score = u32::from(sig) * 8
            + u32::from(key_match || aki.is_none()) * 4
            + u32::from(trusted) * 2
            + u32::from(valid);
        if best.is_none_or(|b| score > b.3) {
            if let Some(b) = best {
                CertFreeCertificateContext(b.0);
            }
            let how = if key_match { HAS_KEY_MATCH_ISSUER } else { HAS_NAME_MATCH_ISSUER };
            best = Some((CertDuplicateCertificateContext(c), how, sig, score));
        }
    }
    best.map(|b| (b.0, b.1, b.2))
}

fn build(e: &Engine, cert: *const CertContext, time: u64, additional: *mut c_void) -> Built {
    let mut b = Built {
        certs: [core::ptr::null(); MAX_ELEMENTS],
        status: [TrustStatus::default(); MAX_ELEMENTS],
        signed: [false; MAX_ELEMENTS],
        n: 1,
        chain_error: 0,
    };
    b.certs[0] = CertDuplicateCertificateContext(cert);
    // Where issuers are looked for: hAdditionalStore, then the engine's.
    let world = new_collection();
    if !world.is_null() {
        if !additional.is_null() {
            CertAddStoreToCollection(world, additional, 0, 1);
        }
        CertAddStoreToCollection(world, e.world, 0, 0);
    }
    loop {
        let last = b.certs[b.n - 1];
        if self_signed(last).is_some() {
            break;
        }
        if b.n == MAX_ELEMENTS || world.is_null() {
            b.chain_error |= IS_PARTIAL_CHAIN;
            break;
        }
        let Some((issuer, how, signed)) = find_issuer(e, world, last, time, &b.certs[..b.n]) else {
            b.chain_error |= IS_PARTIAL_CHAIN;
            break;
        };
        b.status[b.n - 1].info |= how;
        b.signed[b.n - 1] = signed;
        b.certs[b.n] = issuer;
        b.n += 1;
    }
    CertCloseStore(world, 0);
    b
}

/// The checks on a built chain (module comment), into its statuses.
fn check(e: &Engine, b: &mut Built, time: u64) {
    let n = b.n;
    let partial = b.chain_error & IS_PARTIAL_CHAIN != 0;
    // The root.
    if !partial && let Some(how) = self_signed(b.certs[n - 1]) {
        let root = &mut b.status[n - 1];
        root.info |= how;
        if holds(e.root, b.certs[n - 1]) {
            root.info |= HAS_PREFERRED_ISSUER;
            if e.exclusive {
                root.info |= IS_FROM_EXCLUSIVE_TRUST_STORE;
            }
        } else {
            root.error |= IS_UNTRUSTED_ROOT;
        }
    }
    // From the root down: CA constraints, path lengths.
    for i in (0..n).rev() {
        let c = b.certs[i];
        let status = &mut b.status[i];
        if CertVerifyTimeValidity(&time, info(c)) != 0 {
            status.error |= IS_NOT_TIME_VALID;
        }
        if i + 1 < n && !b.signed[i] {
            status.error |= IS_NOT_SIGNATURE_VALID;
        }
        let is_root = i == n - 1 && !partial;
        let constraints = basic_constraints(c);
        let ca = if i == 0 {
            if constraints.is_err() {
                status.error |= INVALID_BASIC_CONSTRAINTS;
            }
            false
        } else {
            let ca = match constraints {
                Ok(Some((ca, path))) => {
                    // Below a CA: i - 1 more CAs before the end certificate.
                    if path.is_some_and(|p| (i - 1) as u32 > p) {
                        status.error |= INVALID_BASIC_CONSTRAINTS;
                    }
                    ca
                }
                // v1 and v2 certificates have no extensions: a CA if it
                // is the root or one of the engine's own certificates.
                Ok(None) => is_root || (info(c).version < 2 && holds(e.world, c)),
                Err(()) => false,
            };
            if !ca {
                status.error |= INVALID_BASIC_CONSTRAINTS;
            }
            if extension_value(info(c), c"2.5.29.30").is_some() {
                status.error |= INVALID_NAME_CONSTRAINTS | HAS_NOT_SUPPORTED_NAME_CONSTRAINT;
                unsupported(b"crypt32.dll!CertGetCertificateChain(name constraints)", i as u64);
            }
            ca
        };
        match key_usage(c) {
            Some(bits) if ca && bits & KEY_CERT_SIGN == 0 => status.error |= IS_NOT_VALID_FOR_USAGE,
            Some(bits) if !ca && bits & KEY_CERT_SIGN != 0 => {
                status.error |= IS_NOT_VALID_FOR_USAGE
            }
            _ => {}
        }
        if !critical_known(c) {
            status.error |= INVALID_EXTENSION | HAS_NOT_SUPPORTED_CRITICAL_EXT;
        }
    }
}

/// CERT_CHAIN_PARA's RequestedUsage (cbSize at least 32): its type and
/// OID strings.
fn requested_usage<'a>(para: *const u8) -> (bool, [&'a [u8]; 8], usize) {
    let mut ids: [&[u8]; 8] = [&[]; 8];
    let size: u32 = read(para, 0);
    if size < 32 {
        return (false, ids, 0);
    }
    let kind: u32 = read(para, 8);
    let count: u32 = read(para, 16);
    let list: *const *const u8 = read(para, 24);
    let n = (count as usize).min(ids.len());
    if list.is_null() {
        return (false, ids, 0);
    }
    for (i, slot) in ids[..n].iter_mut().enumerate() {
        // SAFETY: the caller's array of cUsageIdentifier OID strings.
        *slot = unsafe { c_text(*list.add(i)) };
    }
    (kind == 0, ids, n) // USAGE_MATCH_TYPE_AND is 0
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetSystemTimeAsFileTime(time: *mut u64);
}

/// CertGetCertificateChain: the chain of `cert` (module comment).
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn CertGetCertificateChain(
    engine_handle: *mut c_void,
    cert: *const CertContext,
    time: *const u64,
    additional: *mut c_void,
    para: *const u8,
    flags: u32,
    _reserved: *mut c_void,
    out: *mut *const ChainContext,
) -> i32 {
    if !out.is_null() {
        // SAFETY: the caller's output.
        unsafe { out.write(core::ptr::null()) };
    }
    let Some(e) = engine(engine_handle) else {
        // SAFETY: plain call.
        unsafe { SetLastError(E_INVALIDARG) };
        return 0;
    };
    if cert.is_null() || para.is_null() {
        // SAFETY: plain call.
        unsafe { SetLastError(E_INVALIDARG) };
        return 0;
    }
    let mut now = 0u64;
    // SAFETY: the caller's FILETIME, or ours.
    unsafe {
        if time.is_null() {
            GetSystemTimeAsFileTime(&mut now);
        } else {
            now = time.read_unaligned();
        }
    }
    let mut b = build(e, cert, now, additional);
    check(e, &mut b, now);
    // RequestedUsage: the end certificate's extended key usage.
    let (and, ids, count) = requested_usage(para);
    if count > 0 && !usage_ok(b.certs[0], &ids[..count], and) {
        b.status[0].error |= IS_NOT_VALID_FOR_USAGE;
    }
    // Revocation, where it was asked for (never for a self-signed root):
    // offline, from the CRLs in hAdditionalStore and the engine's stores
    // (crl.rs), at the current time (pTime does not apply to revocation).
    let checked = if flags & REVOCATION_CHECK_END_CERT != 0 {
        1
    } else if flags & REVOCATION_CHECK_CHAIN != 0 {
        b.n
    } else if flags & REVOCATION_CHECK_CHAIN_EXCLUDE_ROOT != 0 {
        b.n - 1
    } else {
        0
    };
    let current = if checked > 0 { crl::now() } else { 0 };
    for i in 0..checked.min(b.n) {
        if i == b.n - 1 && self_signed(b.certs[i]).is_some() {
            continue;
        }
        let issuer = if i + 1 < b.n { b.certs[i + 1] } else { core::ptr::null() };
        let status = crl::check(b.certs[i], issuer, &[additional, e.world], current);
        b.status[i].error |= match status {
            crl::Status::Good(_) => 0,
            crl::Status::Revoked(..) => IS_REVOKED,
            crl::Status::Offline => REVOCATION_STATUS_UNKNOWN | IS_OFFLINE_REVOCATION,
            crl::Status::NoCheck => REVOCATION_STATUS_UNKNOWN,
        };
    }
    // The block.
    // SAFETY: plain allocation, zeroed (HEAP_ZERO_MEMORY).
    let p = unsafe { HeapAlloc(GetProcessHeap(), 8, core::mem::size_of::<ChainBlock>()) }
        .cast::<ChainBlock>();
    if p.is_null() {
        for &c in &b.certs[..b.n] {
            CertFreeCertificateContext(c);
        }
        // SAFETY: plain call.
        unsafe { SetLastError(ERROR_NOT_ENOUGH_MEMORY) };
        return 0;
    }
    let mut status = TrustStatus { error: b.chain_error, info: 0 };
    // SAFETY: a fresh zeroed block; the pointers point into it.
    unsafe {
        let blk = &mut *p;
        blk.magic = CHAIN_MAGIC;
        blk.refs = AtomicU32::new(1);
        for i in 0..b.n {
            let el = &mut blk.elements[i];
            el.size = core::mem::size_of::<Element>() as u32;
            el.cert = b.certs[i];
            el.status = b.status[i];
            status.error |= b.status[i].error;
            // The bottom nibble (how the issuer was matched) is the
            // element's own.
            status.info |= b.status[i].info & !0xf;
            blk.element_ptrs[i] = &raw const blk.elements[i];
        }
        blk.simple.size = core::mem::size_of::<SimpleChain>() as u32;
        blk.simple.status = status;
        blk.simple.count = b.n as u32;
        blk.simple.elements = blk.element_ptrs.as_ptr();
        blk.chain_ptr = &raw const blk.simple;
        blk.context.size = core::mem::size_of::<ChainContext>() as u32;
        blk.context.status = status;
        blk.context.count = 1;
        blk.context.chains = &raw const blk.chain_ptr;
    }
    // SAFETY: the block's context.
    let context = unsafe { &raw const (*p).context };
    if out.is_null() {
        CertFreeCertificateChain(context);
    } else {
        // SAFETY: the caller's output.
        unsafe { out.write(context) };
    }
    1
}

/// CertDuplicateCertificateChain: one more reference to the same chain.
#[unsafe(no_mangle)]
pub extern "system" fn CertDuplicateCertificateChain(
    chain: *const ChainContext,
) -> *const ChainContext {
    if let Some(b) = chain_block(chain) {
        b.refs.fetch_add(1, Ordering::AcqRel);
    }
    chain
}

/// CertFreeCertificateChain: one reference less; the last frees the
/// chain and its certificate references.
#[unsafe(no_mangle)]
pub extern "system" fn CertFreeCertificateChain(chain: *const ChainContext) {
    let Some(b) = chain_block(chain) else { return };
    if b.refs.fetch_sub(1, Ordering::AcqRel) != 1 {
        return;
    }
    for el in &b.elements[..b.simple.count as usize] {
        CertFreeCertificateContext(el.cert);
    }
    b.magic = 0;
    // SAFETY: our block.
    unsafe { HeapFree(GetProcessHeap(), 0, (b as *mut ChainBlock).cast()) };
}

// ---- Policies ----

const CERT_E_EXPIRED: u32 = 0x800b_0101;
const CERT_E_CRITICAL: u32 = 0x800b_0105;
const CERT_E_UNTRUSTEDROOT: u32 = 0x800b_0109;
const CERT_E_CHAINING: u32 = 0x800b_010a;
const CERT_E_CN_NO_MATCH: u32 = 0x800b_010f;
const CERT_E_WRONG_USAGE: u32 = 0x800b_0110;
const CERT_E_INVALID_NAME: u32 = 0x800b_0114;
const TRUST_E_CERT_SIGNATURE: u32 = 0x8009_6004;
const TRUST_E_BASIC_CONSTRAINTS: u32 = 0x8009_6019;
const CRYPT_E_REVOKED: u32 = 0x8009_2010;
const CRYPT_E_NO_REVOCATION_CHECK: u32 = 0x8009_2012;
const CRYPT_E_REVOCATION_OFFLINE: u32 = 0x8009_2013;

// CERT_CHAIN_POLICY_PARA dwFlags.
const IGNORE_NOT_TIME_VALID: u32 = 0x1;
const IGNORE_INVALID_BASIC_CONSTRAINTS: u32 = 0x8;
const ALLOW_UNKNOWN_CA: u32 = 0x10;
const IGNORE_WRONG_USAGE: u32 = 0x20;
const IGNORE_INVALID_NAME: u32 = 0x40;
const IGNORE_END_REV_UNKNOWN: u32 = 0x100;
const IGNORE_CA_REV_UNKNOWN: u32 = 0x400;
const IGNORE_ROOT_REV_UNKNOWN: u32 = 0x800;
const IGNORE_NOT_SUPPORTED_CRITICAL_EXT: u32 = 0x2000;

// SSL_EXTRA_CERT_CHAIN_POLICY_PARA fdwChecks.
const SSL_IGNORE_REVOCATION: u32 = 0x80;
const SSL_IGNORE_UNKNOWN_CA: u32 = 0x100;
const SSL_IGNORE_WRONG_USAGE: u32 = 0x200;
const SSL_IGNORE_CERT_CN_INVALID: u32 = 0x1000;
const SSL_IGNORE_CERT_DATE_INVALID: u32 = 0x2000;
const AUTHTYPE_CLIENT: u32 = 1;
const AUTHTYPE_SERVER: u32 = 2;

/// A policy's result: dwError, lChainIndex, lElementIndex.
type Verdict = (u32, i32, i32);

const PASS: Verdict = (0, -1, -1);

/// The first element whose errors include `bits`.
fn first_with(b: &ChainBlock, bits: u32) -> i32 {
    b.elements[..b.simple.count as usize]
        .iter()
        .position(|el| el.status.error & bits != 0)
        .map_or(-1, |i| i as i32)
}

fn at(b: &ChainBlock, code: u32, bits: u32) -> Verdict {
    (code, 0, first_with(b, bits))
}

/// The checks every policy here makes, in Windows' order (Wine's tests):
/// signatures, an unknown root (or a chain that does not reach one), time,
/// usage, then what the chain engine reports beyond those. `ssl` has the
/// SSL policy's fdwChecks.
fn base(b: &ChainBlock, flags: u32, ssl: Option<u32>) -> Verdict {
    let error = b.context.status.error;
    let checks = ssl.unwrap_or(0);
    let unknown_ca_ok = flags & ALLOW_UNKNOWN_CA != 0 || checks & SSL_IGNORE_UNKNOWN_CA != 0;
    if error & IS_NOT_SIGNATURE_VALID != 0 {
        return at(b, TRUST_E_CERT_SIGNATURE, IS_NOT_SIGNATURE_VALID);
    }
    if error & IS_UNTRUSTED_ROOT != 0 && !unknown_ca_ok {
        return at(b, CERT_E_UNTRUSTEDROOT, IS_UNTRUSTED_ROOT);
    }
    if error & (IS_PARTIAL_CHAIN | IS_CYCLIC) != 0 && !unknown_ca_ok {
        // No element is at fault. The SSL policy reports an unknown CA,
        // as it does for a cyclic chain.
        return (if ssl.is_some() { CERT_E_UNTRUSTEDROOT } else { CERT_E_CHAINING }, 0, -1);
    }
    if error & IS_NOT_TIME_VALID != 0
        && flags & IGNORE_NOT_TIME_VALID == 0
        && checks & SSL_IGNORE_CERT_DATE_INVALID == 0
    {
        return at(b, CERT_E_EXPIRED, IS_NOT_TIME_VALID);
    }
    if error & IS_NOT_VALID_FOR_USAGE != 0
        && flags & IGNORE_WRONG_USAGE == 0
        && checks & SSL_IGNORE_WRONG_USAGE == 0
    {
        return at(b, CERT_E_WRONG_USAGE, IS_NOT_VALID_FOR_USAGE);
    }
    if error & IS_REVOKED != 0 && checks & SSL_IGNORE_REVOCATION == 0 {
        return at(b, CRYPT_E_REVOKED, IS_REVOKED);
    }
    if error & REVOCATION_STATUS_UNKNOWN != 0 && checks & SSL_IGNORE_REVOCATION == 0 {
        let n = b.simple.count as usize;
        let ignored = |i: usize| {
            let bit = if i == 0 {
                IGNORE_END_REV_UNKNOWN
            } else if i == n - 1 {
                IGNORE_ROOT_REV_UNKNOWN
            } else {
                IGNORE_CA_REV_UNKNOWN
            };
            flags & bit != 0
        };
        let unknown = (0..n)
            .find(|&i| b.elements[i].status.error & REVOCATION_STATUS_UNKNOWN != 0 && !ignored(i));
        if let Some(i) = unknown {
            let offline = b.elements[i].status.error & IS_OFFLINE_REVOCATION != 0;
            let code =
                if offline { CRYPT_E_REVOCATION_OFFLINE } else { CRYPT_E_NO_REVOCATION_CHECK };
            return (code, 0, i as i32);
        }
    }
    if error & INVALID_BASIC_CONSTRAINTS != 0 && flags & IGNORE_INVALID_BASIC_CONSTRAINTS == 0 {
        return at(b, TRUST_E_BASIC_CONSTRAINTS, INVALID_BASIC_CONSTRAINTS);
    }
    if error & NAME_CONSTRAINT_ERRORS != 0 && flags & IGNORE_INVALID_NAME == 0 {
        return at(b, CERT_E_INVALID_NAME, NAME_CONSTRAINT_ERRORS);
    }
    if error & HAS_NOT_SUPPORTED_CRITICAL_EXT != 0 && flags & IGNORE_NOT_SUPPORTED_CRITICAL_EXT == 0
    {
        return at(b, CERT_E_CRITICAL, HAS_NOT_SUPPORTED_CRITICAL_EXT);
    }
    PASS
}

/// A server name as lowercase ASCII (at most 255 characters).
fn ascii_name(p: *const u16, out: &mut [u8; 256]) -> Option<usize> {
    let mut n = 0;
    loop {
        // SAFETY: the caller's NUL-terminated string, read up to its end.
        let c = unsafe { *p.add(n) };
        if c == 0 {
            return Some(n);
        }
        if n == 255 || c >= 0x80 {
            return None;
        }
        out[n] = (c as u8).to_ascii_lowercase();
        n += 1;
    }
}

/// Whether DNS name `pattern` (from the certificate) matches `name`: equal
/// (case ignored), or a leading "*." label standing for one label.
fn dns_matches(pattern: &[u8], name: &[u8]) -> bool {
    if pattern.eq_ignore_ascii_case(name) {
        return true;
    }
    match (pattern.strip_prefix(b"*"), name.iter().position(|&c| c == b'.')) {
        (Some(rest), Some(dot)) if rest.first() == Some(&b'.') && dot > 0 => {
            rest.eq_ignore_ascii_case(&name[dot..])
        }
        _ => false,
    }
}

/// An IPv4 (dotted decimal) or IPv6 address literal as its bytes.
fn ip_bytes(name: &[u8], out: &mut [u8; 16]) -> Option<usize> {
    if name.contains(&b':') {
        // IPv6: groups of hex digits, one "::".
        let text = core::str::from_utf8(name).ok()?;
        let (head, tail) = match text.find("::") {
            Some(i) => (&text[..i], Some(&text[i + 2..])),
            None => (text, None),
        };
        let mut groups = [0u16; 8];
        let parse = |part: &str, dst: &mut [u16]| -> Option<usize> {
            if part.is_empty() {
                return Some(0);
            }
            let mut n = 0;
            for g in part.split(':') {
                if n == dst.len() || g.is_empty() || g.len() > 4 {
                    return None;
                }
                dst[n] = u16::from_str_radix(g, 16).ok()?;
                n += 1;
            }
            Some(n)
        };
        let mut front = [0u16; 8];
        let a = parse(head, &mut front)?;
        let mut back = [0u16; 8];
        let b = match tail {
            Some(t) => parse(t, &mut back)?,
            None => 0,
        };
        if (tail.is_none() && a != 8) || a + b > 8 || (tail.is_some() && a + b == 8) {
            return None;
        }
        groups[..a].copy_from_slice(&front[..a]);
        groups[8 - b..].copy_from_slice(&back[..b]);
        for (i, g) in groups.iter().enumerate() {
            out[2 * i..2 * i + 2].copy_from_slice(&g.to_be_bytes());
        }
        return Some(16);
    }
    let mut parts = 0;
    for part in name.split(|&c| c == b'.') {
        if parts == 4 || part.is_empty() || part.len() > 3 || !part.iter().all(u8::is_ascii_digit) {
            return None;
        }
        let v = part.iter().fold(0u32, |a, &c| a * 10 + u32::from(c - b'0'));
        out[parts] = u8::try_from(v).ok()?;
        parts += 1;
    }
    (parts == 4).then_some(4)
}

/// Whether the end certificate is for `name`: its subject alternative
/// names (DNS names, IP addresses) if it has the extension, else its
/// subject's common names.
fn name_matches(c: *const CertContext, name: &[u8]) -> bool {
    let mut ip = [0u8; 16];
    let ip = ip_bytes(name, &mut ip).map(|n| &ip[..n]);
    if let Some(value) = extension_value(info(c), c"2.5.29.17") {
        let Ok(seq) = Der::new(value).expect(SEQUENCE) else { return false };
        let mut d = Der::new(seq.content);
        while let Ok(t) = d.next() {
            let ok = match t.tag {
                0x82 => ip.is_none() && dns_matches(t.content, name),
                0x87 => ip == Some(t.content),
                _ => false,
            };
            if ok {
                return true;
            }
        }
        return false;
    }
    // The subject's CNs: SEQUENCE OF SET OF { OID, value }.
    let Ok(seq) = Der::new(blob_bytes(&info(c).subject)).expect(SEQUENCE) else { return false };
    let mut rdns = Der::new(seq.content);
    while let Ok(set) = rdns.expect(SET) {
        let mut atvs = Der::new(set.content);
        while let Ok(atv) = atvs.expect(SEQUENCE) {
            let mut d = Der::new(atv.content);
            if let (Ok(oid), Ok(value)) = (d.expect(OID), d.next())
                && oid.content == [0x55, 0x04, 0x03]
                && (if ip.is_some() {
                    value.content == name
                } else {
                    dns_matches(value.content, name)
                })
            {
                return true;
            }
        }
    }
    false
}

/// The SSL policy's own checks after `base`: the end certificate's
/// extended key usage for the side (AUTHTYPE_SERVER: serverAuth or a
/// server-gated-crypto usage; AUTHTYPE_CLIENT: clientAuth), then the
/// server name.
fn ssl_checks(b: &ChainBlock, extra: *const u8) -> Verdict {
    if extra.is_null() || read::<u32>(extra, 0) < 24 {
        return PASS;
    }
    let auth: u32 = read(extra, 4);
    let checks: u32 = read(extra, 8);
    let server: *const u16 = read(extra, 16);
    let end = b.elements[0].cert;
    if checks & SSL_IGNORE_WRONG_USAGE == 0 {
        let wanted: &[&[u8]] = match auth {
            AUTHTYPE_SERVER => {
                &[b"1.3.6.1.5.5.7.3.1", b"1.3.6.1.4.1.311.10.3.3", b"2.16.840.1.113730.4.1"]
            }
            AUTHTYPE_CLIENT => &[b"1.3.6.1.5.5.7.3.2"],
            _ => &[],
        };
        if !wanted.is_empty() && !usage_ok(end, wanted, false) {
            return (CERT_E_WRONG_USAGE, 0, 0);
        }
    }
    if auth == AUTHTYPE_SERVER && !server.is_null() && checks & SSL_IGNORE_CERT_CN_INVALID == 0 {
        let mut name = [0u8; 256];
        let ok = ascii_name(server, &mut name).is_some_and(|n| name_matches(end, &name[..n]));
        if !ok {
            return (CERT_E_CN_NO_MATCH, 0, 0);
        }
    }
    PASS
}

/// The name of a policy error, for the ledger.
fn error_name(code: u32) -> &'static [u8] {
    match code {
        CERT_E_EXPIRED => b"CERT_E_EXPIRED",
        CERT_E_CRITICAL => b"CERT_E_CRITICAL",
        CERT_E_UNTRUSTEDROOT => b"CERT_E_UNTRUSTEDROOT",
        CERT_E_CHAINING => b"CERT_E_CHAINING",
        CERT_E_CN_NO_MATCH => b"CERT_E_CN_NO_MATCH",
        CERT_E_WRONG_USAGE => b"CERT_E_WRONG_USAGE",
        CERT_E_INVALID_NAME => b"CERT_E_INVALID_NAME",
        TRUST_E_CERT_SIGNATURE => b"TRUST_E_CERT_SIGNATURE",
        TRUST_E_BASIC_CONSTRAINTS => b"TRUST_E_BASIC_CONSTRAINTS",
        CRYPT_E_REVOKED => b"CRYPT_E_REVOKED",
        CRYPT_E_REVOCATION_OFFLINE => b"CRYPT_E_REVOCATION_OFFLINE",
        CRYPT_E_NO_REVOCATION_CHECK => b"CRYPT_E_NO_REVOCATION_CHECK",
        _ => b"?",
    }
}

/// CertVerifyCertificateChainPolicy: CERT_CHAIN_POLICY_BASE (1), SSL (4)
/// and BASIC_CONSTRAINTS (5); AUTHENTICODE (2), AUTHENTICODE_TS (3),
/// NT_AUTH (6), MICROSOFT_ROOT (7), EV (8) and SSL_F12 (9) are checked as
/// BASE (SSL_F12 as SSL) and recorded; a policy named by an OID string
/// is recorded and fails (CRYPT_E_NOT_FOUND).
#[unsafe(no_mangle)]
pub extern "system" fn CertVerifyCertificateChainPolicy(
    policy: usize,
    chain: *const ChainContext,
    para: *const u8,
    status: *mut u8,
) -> i32 {
    let Some(b) = chain_block(chain) else {
        // SAFETY: plain call.
        unsafe { SetLastError(E_INVALIDARG) };
        return 0;
    };
    if status.is_null() {
        // SAFETY: plain call.
        unsafe { SetLastError(E_INVALIDARG) };
        return 0;
    }
    let (flags, extra) = if para.is_null() || read::<u32>(para, 0) < 16 {
        (0u32, core::ptr::null())
    } else {
        (read::<u32>(para, 4), read::<*const u8>(para, 8))
    };
    let ssl_flags =
        if extra.is_null() || read::<u32>(extra, 0) < 24 { 0 } else { read::<u32>(extra, 8) };
    let (name, verdict): (&[u8], Verdict) = match policy {
        1 => (b"BASE", base(b, flags, None)),
        4 | 9 => {
            if policy == 9 {
                unsupported(b"crypt32.dll!CertVerifyCertificateChainPolicy(SSL_F12)", 0);
            }
            let v = base(b, flags, Some(ssl_flags));
            (b"SSL", if v.0 != 0 { v } else { ssl_checks(b, extra) })
        }
        5 => {
            let v = if b.context.status.error & INVALID_BASIC_CONSTRAINTS != 0 {
                at(b, TRUST_E_BASIC_CONSTRAINTS, INVALID_BASIC_CONSTRAINTS)
            } else {
                PASS
            };
            (b"BASIC_CONSTRAINTS", v)
        }
        2 | 3 | 6..=8 => {
            unsupported(b"crypt32.dll!CertVerifyCertificateChainPolicy(as BASE)", policy as u64);
            (b"BASE", base(b, flags, None))
        }
        _ => {
            unsupported(b"crypt32.dll!CertVerifyCertificateChainPolicy(policy)", policy as u64);
            // SAFETY: plain call.
            unsafe { SetLastError(CRYPT_E_NOT_FOUND) };
            return 0;
        }
    };
    if verdict.0 != 0 {
        // The ledger says why: `crypt32.dll!SSL: CERT_E_UNTRUSTEDROOT`,
        // the chain's error bits as detail.
        let mut text = [0u8; 80];
        let mut n = 0;
        for part in [&b"crypt32.dll!"[..], name, b": ", error_name(verdict.0)] {
            text[n..n + part.len()].copy_from_slice(part);
            n += part.len();
        }
        // SAFETY: a valid buffer; 5 is the ledger's "other".
        unsafe { GalaRecordMissing(text.as_ptr(), n, 5, b.context.status.error.into()) };
    }
    // CERT_CHAIN_POLICY_STATUS: dwError, lChainIndex, lElementIndex.
    // SAFETY: the caller's structure.
    unsafe {
        status.add(4).cast::<u32>().write_unaligned(verdict.0);
        status.add(8).cast::<i32>().write_unaligned(verdict.1);
        status.add(12).cast::<i32>().write_unaligned(verdict.2);
    }
    1
}

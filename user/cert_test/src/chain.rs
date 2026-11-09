//! The chain engine (TLS step 5), with the results Microsoft documents
//! for CertGetCertificateChain, CERT_TRUST_STATUS,
//! CertVerifyCertificateChainPolicy and CERT_CHAIN_POLICY_STATUS, and
//! those Wine's conformance tests (dlls/crypt32/tests/chain.c, checked on
//! Windows) record: which element carries an error and which policy error
//! and element index each kind of chain gives.
//!
//! The certificates (`chain/make.sh`, valid 2025..2045, so the times are
//! fixed): root (RSA) -> intermediate (ECDSA P-384, signed with RSA) ->
//! leaf (ECDSA P-256, ecdsa-with-SHA384; DNS www.chain.test,
//! *.wild.chain.test, IP 10.0.0.1, serverAuth); a clientAuth-only leaf; a
//! CA:FALSE certificate that signs another. The root is trusted through
//! an engine whose hExclusiveRoot holds it (as curl does for --cacert),
//! so the checks hold on Windows without changing its root store; one
//! part uses the machine's own roots, which hold the schannel test CA in
//! the scenario (on Windows, run-all.ps1 imports it as administrator).
//!
//! 1. CertCreateCertificateChainEngine takes the documented sizes of
//!    CERT_CHAIN_ENGINE_CONFIG (88; 80 as curl passes it) and refuses
//!    another (E_INVALIDARG).
//! 2. A full chain: three elements leaf to root, their contexts, no
//!    error; the issuer match each element reports (key identifier for
//!    the leaf and intermediate, name and self-signed for the root);
//!    CertDuplicateCertificateChain; the SSL policy matches the DNS
//!    names (case ignored), a wildcard for one label only, the IP
//!    address, not the subject CN when there are alternative names
//!    (CERT_E_CN_NO_MATCH at element 0), and nothing with
//!    SECURITY_FLAG_IGNORE_CERT_CN_INVALID; the base policy passes;
//!    CertGetNameStringW(CERT_NAME_DNS_TYPE): the first DNS name, all of
//!    them with CERT_NAME_SEARCH_ALL_NAMES_FLAG, the CN without them.
//! 3. Without the intermediate: CERT_TRUST_IS_PARTIAL_CHAIN, one element;
//!    the base policy gives CERT_E_CHAINING.
//! 4. The default engine with the intermediate and root as additional
//!    certificates: CERT_TRUST_IS_UNTRUSTED_ROOT on the root (element
//!    2), CERT_E_UNTRUSTEDROOT there, none with
//!    CERT_CHAIN_POLICY_ALLOW_UNKNOWN_CA_FLAG or
//!    SECURITY_FLAG_IGNORE_UNKNOWN_CA.
//! 5. Past (and before) the validity: CERT_TRUST_IS_NOT_TIME_VALID on
//!    every element, CERT_E_EXPIRED at element 0, none when ignored.
//! 6. A leaf whose signature is changed: CERT_TRUST_IS_NOT_SIGNATURE_VALID
//!    on it, TRUST_E_CERT_SIGNATURE at element 0;
//!    CryptVerifyCertificateSignatureEx of ECDSA (P-256 by P-384) and RSA
//!    signatures.
//! 7. RequestedUsage serverAuth: the clientAuth leaf gets
//!    CERT_TRUST_IS_NOT_VALID_FOR_USAGE (CERT_E_WRONG_USAGE, ignored with
//!    CERT_CHAIN_POLICY_IGNORE_WRONG_USAGE_FLAG); the serverAuth leaf
//!    passes (a full-size CERT_CHAIN_PARA).
//! 8. A certificate signed by a CA:FALSE one: CERT_TRUST_INVALID_BASIC_
//!    CONSTRAINTS on the signer (element 1), TRUST_E_BASIC_CONSTRAINTS
//!    from CERT_CHAIN_POLICY_BASIC_CONSTRAINTS.
//! 9. Revocation (CERT_CHAIN_REVOCATION_CHECK_CHAIN_EXCLUDE_ROOT with
//!    CERT_CHAIN_REVOCATION_CHECK_CACHE_ONLY: the CRLs given are all
//!    there is, and the distribution points, on crl.chain.test, do not
//!    resolve), with the CRLs of `chain/make.sh` in hAdditionalStore:
//!    - none: CERT_TRUST_REVOCATION_STATUS_UNKNOWN |
//!      CERT_TRUST_IS_OFFLINE_REVOCATION on the leaf and intermediate
//!      (each names a CRL), not the root; the SSL policy's
//!      CRYPT_E_REVOCATION_OFFLINE at element 0, at element 1 with
//!      CERT_CHAIN_POLICY_IGNORE_END_REV_UNKNOWN_FLAG, none with
//!      SECURITY_FLAG_IGNORE_REVOCATION; only the leaf with
//!      CERT_CHAIN_REVOCATION_CHECK_END_CERT;
//!    - the intermediate's and the root's current CRLs: no error, also
//!      with CERT_CHAIN_REVOCATION_CHECK_CHAIN (a self-signed root is not
//!      checked); a leaf that names no CRL (client.cer) is not checked by
//!      them: CERT_TRUST_REVOCATION_STATUS_UNKNOWN alone and
//!      CRYPT_E_NO_REVOCATION_CHECK (what Schannel reports for such
//!      certificates on Windows), none with
//!      CERT_CHAIN_POLICY_IGNORE_END_REV_UNKNOWN_FLAG;
//!    - a leaf the intermediate's CRL lists: CERT_TRUST_IS_REVOKED on it
//!      (and the chain), CRYPT_E_REVOKED at element 0 from the SSL and
//!      base policies, none with SECURITY_FLAG_IGNORE_REVOCATION;
//!    - that leaf (it names a distribution point) with only an
//!      out-of-date CRL of the intermediate, or none:
//!      CERT_TRUST_REVOCATION_STATUS_UNKNOWN |
//!      CERT_TRUST_IS_OFFLINE_REVOCATION, CRYPT_E_REVOCATION_OFFLINE;
//!      with the current CRL's signature broken it is not revoked
//!      either (a CRL that does not verify is not used).
//! 10. The machine's roots (the default engine, HCCE_CURRENT_USER and
//!     HCCE_LOCAL_MACHINE): the schannel test CA's RSA localhost
//!     certificate is trusted with no additional store, matches
//!     "localhost" and "127.0.0.1", not "example.com"; the CA and MY
//!     system stores open.

use super::{
    Bool, CertAddEncodedCertificateToStore, CertCloseStore, CertContext,
    CertCreateCertificateContext, CertFreeCertificateContext, CertOpenStore, CertOpenSystemStoreW,
    Checks, GetLastError, Handle, X509_ASN_ENCODING, wide_text,
};
use core::ffi::c_void;
use core::ptr::{null, null_mut};

const ROOT: &[u8] = include_bytes!("../chain/root.cer");
const INTER: &[u8] = include_bytes!("../chain/inter.cer");
const LEAF: &[u8] = include_bytes!("../chain/leaf.cer");
const CLIENT: &[u8] = include_bytes!("../chain/client.cer");
const NOTCA: &[u8] = include_bytes!("../chain/notca.cer");
const UNDERLING: &[u8] = include_bytes!("../chain/underling.cer");
const REVOKED: &[u8] = include_bytes!("../chain/revoked.cer");
/// The intermediate's CRL (lists REVOKED), its CRL of 2020..2021, the
/// root's (empty).
const INTER_CRL: &[u8] = include_bytes!("../chain/inter.crl");
const INTER_OLD_CRL: &[u8] = include_bytes!("../chain/inter-old.crl");
const ROOT_CRL: &[u8] = include_bytes!("../chain/root.crl");
/// The schannel test CA's RSA localhost certificate (DNS:localhost,
/// IP:127.0.0.1), issued by the CA in the machine's roots.
const LOCALHOST: &[u8] = include_bytes!("../../schannel_test/certs/server-rsa.cer");

/// FILETIMEs: 2030-06-01, 2050-06-01, 2020-06-01 (UTC).
const T2030: u64 = 0x01e1_6d68_a466_0000;
const T2050: u64 = 0x01f7_d9b3_e7fd_c000;
const T2020: u64 = 0x01d6_37a7_97cf_0000;

#[repr(C)]
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
    has_freshness: Bool,
    freshness: u32,
}

/// CERT_CHAIN_CONTEXT.
#[repr(C)]
struct ChainContext {
    size: u32,
    status: TrustStatus,
    count: u32,
    chains: *const *const SimpleChain,
    lower_count: u32,
    lower: *const c_void,
    has_freshness: Bool,
    freshness: u32,
    create_flags: u32,
    id: [u8; 16],
}

/// CERT_ENHKEY_USAGE.
#[repr(C)]
struct EnhKeyUsage {
    count: u32,
    ids: *const *const u8,
}

/// CERT_USAGE_MATCH.
#[repr(C)]
struct UsageMatch {
    kind: u32,
    usage: EnhKeyUsage,
}

/// CERT_CHAIN_PARA (the full structure; cbSize may say less).
#[repr(C)]
struct ChainPara {
    size: u32,
    usage: UsageMatch,
    issuance: UsageMatch,
    url_timeout: u32,
    check_freshness: Bool,
    freshness: u32,
    cache_resync: *const u64,
    strong_sign: *const c_void,
    strong_sign_flags: u32,
}

/// CERT_CHAIN_ENGINE_CONFIG.
#[repr(C)]
struct EngineConfig {
    size: u32,
    restricted_root: Handle,
    restricted_trust: Handle,
    restricted_other: Handle,
    additional_count: u32,
    additional: *const Handle,
    flags: u32,
    url_timeout: u32,
    max_cached: u32,
    cycle_modulus: u32,
    exclusive_root: Handle,
    exclusive_people: Handle,
    exclusive_flags: u32,
}

/// CERT_CHAIN_POLICY_PARA.
#[repr(C)]
struct PolicyPara {
    size: u32,
    flags: u32,
    extra: *const SslPara,
}

/// SSL_EXTRA_CERT_CHAIN_POLICY_PARA.
#[repr(C)]
struct SslPara {
    size: u32,
    auth_type: u32,
    checks: u32,
    server_name: *const u16,
}

/// CERT_CHAIN_POLICY_STATUS.
#[repr(C)]
struct PolicyStatus {
    size: u32,
    error: u32,
    chain_index: i32,
    element_index: i32,
    extra: *mut c_void,
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "crypt32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "crypt32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn CertCreateCertificateChainEngine(config: *const EngineConfig, engine: *mut Handle) -> Bool;
    fn CertFreeCertificateChainEngine(engine: Handle);
    fn CertGetCertificateChain(
        engine: Handle,
        cert: *const CertContext,
        time: *const u64,
        additional: Handle,
        para: *const ChainPara,
        flags: u32,
        reserved: *const c_void,
        chain: *mut *const ChainContext,
    ) -> Bool;
    fn CertDuplicateCertificateChain(chain: *const ChainContext) -> *const ChainContext;
    fn CertFreeCertificateChain(chain: *const ChainContext);
    fn CertVerifyCertificateChainPolicy(
        policy: usize,
        chain: *const ChainContext,
        para: *const PolicyPara,
        status: *mut PolicyStatus,
    ) -> Bool;
    fn CertAddEncodedCRLToStore(
        store: Handle,
        encoding: u32,
        data: *const u8,
        len: u32,
        disposition: u32,
        out: *mut *const super::crl::CrlContext,
    ) -> Bool;
}

const E_INVALIDARG: u64 = 0x8007_0057;
const NTE_BAD_SIGNATURE: u64 = 0x8009_0006;

const CERT_TRUST_IS_NOT_TIME_VALID: u32 = 0x1;
const CERT_TRUST_IS_NOT_SIGNATURE_VALID: u32 = 0x8;
const CERT_TRUST_IS_NOT_VALID_FOR_USAGE: u32 = 0x10;
const CERT_TRUST_IS_UNTRUSTED_ROOT: u32 = 0x20;
const CERT_TRUST_IS_REVOKED: u32 = 0x4;
const CERT_TRUST_REVOCATION_STATUS_UNKNOWN: u32 = 0x40;
const CERT_TRUST_IS_OFFLINE_REVOCATION: u32 = 0x100_0000;
const CERT_TRUST_INVALID_BASIC_CONSTRAINTS: u32 = 0x400;
const CERT_TRUST_IS_PARTIAL_CHAIN: u32 = 0x1_0000;
const CERT_TRUST_HAS_KEY_MATCH_ISSUER: u32 = 0x2;
const CERT_TRUST_HAS_NAME_MATCH_ISSUER: u32 = 0x4;
const CERT_TRUST_IS_SELF_SIGNED: u32 = 0x8;

const CERT_CHAIN_POLICY_BASE: usize = 1;
const CERT_CHAIN_POLICY_SSL: usize = 4;
const CERT_CHAIN_POLICY_BASIC_CONSTRAINTS: usize = 5;
const CERT_CHAIN_POLICY_IGNORE_ALL_NOT_TIME_VALID_FLAGS: u32 = 0x7;
const CERT_CHAIN_POLICY_ALLOW_UNKNOWN_CA_FLAG: u32 = 0x10;
const CERT_CHAIN_POLICY_IGNORE_WRONG_USAGE_FLAG: u32 = 0x20;
const SECURITY_FLAG_IGNORE_REVOCATION: u32 = 0x80;
const SECURITY_FLAG_IGNORE_UNKNOWN_CA: u32 = 0x100;
const SECURITY_FLAG_IGNORE_CERT_CN_INVALID: u32 = 0x1000;
const SECURITY_FLAG_IGNORE_CERT_DATE_INVALID: u32 = 0x2000;
const AUTHTYPE_SERVER: u32 = 2;
const CERT_CHAIN_REVOCATION_CHECK_END_CERT: u32 = 0x1000_0000;
const CERT_CHAIN_REVOCATION_CHECK_CHAIN: u32 = 0x2000_0000;
const CERT_CHAIN_REVOCATION_CHECK_CHAIN_EXCLUDE_ROOT: u32 = 0x4000_0000;
const CERT_CHAIN_REVOCATION_CHECK_CACHE_ONLY: u32 = 0x8000_0000;
const CERT_CHAIN_POLICY_IGNORE_END_REV_UNKNOWN_FLAG: u32 = 0x100;
const USAGE_MATCH_TYPE_OR: u32 = 1;

const CERT_E_EXPIRED: u64 = 0x800b_0101;
const CERT_E_UNTRUSTEDROOT: u64 = 0x800b_0109;
const CERT_E_CHAINING: u64 = 0x800b_010a;
const CERT_E_CN_NO_MATCH: u64 = 0x800b_010f;
const CERT_E_WRONG_USAGE: u64 = 0x800b_0110;
const TRUST_E_CERT_SIGNATURE: u64 = 0x8009_6004;
const TRUST_E_BASIC_CONSTRAINTS: u64 = 0x8009_6019;
const CRYPT_E_REVOKED: u64 = 0x8009_2010;
const CRYPT_E_NO_REVOCATION_CHECK: u64 = 0x8009_2012;
const CRYPT_E_REVOCATION_OFFLINE: u64 = 0x8009_2013;

const CERT_STORE_PROV_MEMORY: usize = 2;
const CERT_STORE_ADD_ALWAYS: u32 = 4;

/// A memory store holding `certs`.
fn store_of(c: &mut Checks, certs: &[&[u8]]) -> Result<Handle, u32> {
    // SAFETY: crypt32 calls with our buffers.
    unsafe {
        let s = CertOpenStore(CERT_STORE_PROV_MEMORY as *const u8, 0, 0, 0, null());
        c.ok(!s.is_null(), GetLastError().into())?;
        for der in certs {
            c.ok(
                CertAddEncodedCertificateToStore(
                    s,
                    X509_ASN_ENCODING,
                    der.as_ptr(),
                    der.len() as u32,
                    CERT_STORE_ADD_ALWAYS,
                    null_mut(),
                ) != 0,
                GetLastError().into(),
            )?;
        }
        Ok(s)
    }
}

/// A memory store holding `certs` and the CRLs `crls`.
fn store_with_crls(c: &mut Checks, certs: &[&[u8]], crls: &[&[u8]]) -> Result<Handle, u32> {
    let s = store_of(c, certs)?;
    for der in crls {
        // SAFETY: our store and DER CRL.
        let ok = unsafe {
            CertAddEncodedCRLToStore(
                s,
                X509_ASN_ENCODING,
                der.as_ptr(),
                der.len() as u32,
                CERT_STORE_ADD_ALWAYS,
                null_mut(),
            )
        };
        // SAFETY: plain call.
        c.ok(ok != 0, unsafe { GetLastError() }.into())?;
    }
    Ok(s)
}

/// Each element's revocation bits (IS_REVOKED, REVOCATION_STATUS_UNKNOWN,
/// IS_OFFLINE_REVOCATION).
fn revocation_bits(chain: *const ChainContext) -> [u32; 3] {
    let mask = CERT_TRUST_IS_REVOKED
        | CERT_TRUST_REVOCATION_STATUS_UNKNOWN
        | CERT_TRUST_IS_OFFLINE_REVOCATION;
    let mut bits = [0; 3];
    for (i, slot) in bits.iter_mut().enumerate().take(elements(chain).len()) {
        *slot = element(chain, i).status.error & mask;
    }
    bits
}

fn context(c: &mut Checks, der: &[u8]) -> Result<*const CertContext, u32> {
    // SAFETY: a DER certificate of ours.
    let p =
        unsafe { CertCreateCertificateContext(X509_ASN_ENCODING, der.as_ptr(), der.len() as u32) };
    c.ok(!p.is_null(), 0)?;
    Ok(p)
}

fn para() -> ChainPara {
    let none = UsageMatch { kind: 0, usage: EnhKeyUsage { count: 0, ids: null() } };
    ChainPara {
        // CERT_CHAIN_PARA without its later fields (16 bytes on x86, 32 on
        // x64).
        size: core::mem::offset_of!(ChainPara, issuance) as u32,
        usage: none,
        issuance: UsageMatch { kind: 0, usage: EnhKeyUsage { count: 0, ids: null() } },
        url_timeout: 0,
        check_freshness: 0,
        freshness: 0,
        cache_resync: null(),
        strong_sign: null(),
        strong_sign_flags: 0,
    }
}

/// CertGetCertificateChain, which must succeed.
fn chain(
    c: &mut Checks,
    engine: Handle,
    cert: *const CertContext,
    time: Option<u64>,
    additional: Handle,
    para: &ChainPara,
    flags: u32,
) -> Result<*const ChainContext, u32> {
    let t = time.unwrap_or(0);
    let mut out: *const ChainContext = null();
    // SAFETY: our context, store and structures.
    let ok = unsafe {
        CertGetCertificateChain(
            engine,
            cert,
            if time.is_some() { &t } else { null() },
            additional,
            para,
            flags,
            null(),
            &mut out,
        )
    };
    // SAFETY: plain call.
    c.ok(ok != 0 && !out.is_null(), unsafe { GetLastError() }.into())?;
    // SAFETY: the chain crypt32 gave.
    unsafe {
        c.eq((*out).size.into(), super::by_width(56, 72))?;
        c.eq((*out).count.into(), 1)?;
        let simple = *(*out).chains;
        c.eq((*simple).size.into(), super::by_width(32, 40))?;
        // The simple chain's errors are the context's.
        c.eq((*simple).status.error.into(), (*out).status.error.into())?;
    }
    Ok(out)
}

/// The simple chain's elements.
fn elements<'a>(chain: *const ChainContext) -> &'a [*const Element] {
    // SAFETY: a chain crypt32 gave (one simple chain).
    unsafe {
        let simple = &**(*chain).chains;
        core::slice::from_raw_parts(simple.elements, simple.count as usize)
    }
}

fn element<'a>(chain: *const ChainContext, i: usize) -> &'a Element {
    // SAFETY: an element of the chain.
    unsafe { &*elements(chain)[i] }
}

/// The element's certificate is `der`.
fn is_cert(e: &Element, der: &[u8]) -> bool {
    // SAFETY: the element's context.
    unsafe {
        let ctx = &*e.cert;
        core::slice::from_raw_parts(ctx.encoded, ctx.encoded_len as usize) == der
    }
}

/// CertVerifyCertificateChainPolicy: (dwError, lChainIndex, lElementIndex).
fn policy(
    c: &mut Checks,
    oid: usize,
    chain: *const ChainContext,
    flags: u32,
    ssl: Option<(&str, u32)>,
) -> Result<(u64, i32, i32), u32> {
    let mut name = [0u16; 32];
    let extra = ssl.map(|(server, checks)| SslPara {
        size: core::mem::size_of::<SslPara>() as u32,
        auth_type: AUTHTYPE_SERVER,
        checks,
        server_name: wide_text(server, &mut name),
    });
    let para = PolicyPara {
        size: core::mem::size_of::<PolicyPara>() as u32,
        flags,
        extra: extra.as_ref().map_or(null(), |e| e as *const SslPara),
    };
    let mut status = PolicyStatus {
        size: core::mem::size_of::<PolicyStatus>() as u32,
        error: 0xdead,
        chain_index: 7,
        element_index: 7,
        extra: null_mut(),
    };
    // SAFETY: our structures and the chain crypt32 gave.
    c.ok(unsafe { CertVerifyCertificateChainPolicy(oid, chain, &para, &mut status) } != 0, 0)?;
    Ok((status.error.into(), status.chain_index, status.element_index))
}

fn ssl(
    c: &mut Checks,
    chain: *const ChainContext,
    name: &str,
    checks: u32,
) -> Result<(u64, i32, i32), u32> {
    policy(c, CERT_CHAIN_POLICY_SSL, chain, 0, Some((name, checks)))
}

fn free(chain: *const ChainContext) {
    // SAFETY: a chain crypt32 gave.
    unsafe { CertFreeCertificateChain(chain) };
}

const PASS: (u64, i32, i32) = (0, -1, -1);

pub fn chains(c: &mut Checks) -> Result<(), u32> {
    let root_store = store_of(c, &[ROOT])?;
    let inter_store = store_of(c, &[INTER])?;
    let leaf = context(c, LEAF)?;
    let none: Handle = null_mut();

    super::say(b"chain: engines\r\n");
    // 1. The engine: hExclusiveRoot trusts the test root.
    let mut config = EngineConfig {
        size: 12,
        restricted_root: none,
        restricted_trust: none,
        restricted_other: none,
        additional_count: 0,
        additional: null(),
        flags: 0,
        url_timeout: 0,
        max_cached: 0,
        cycle_modulus: 0,
        exclusive_root: root_store,
        exclusive_people: none,
        exclusive_flags: 0,
    };
    let mut engine: Handle = null_mut();
    // SAFETY: our configuration and output.
    unsafe {
        c.eq(CertCreateCertificateChainEngine(&config, &mut engine) as u64, 0)?;
        c.eq(GetLastError().into(), E_INVALIDARG)?;
        // curl's: without dwExclusiveFlags (48 bytes on x86, 80 on x64).
        config.size = core::mem::offset_of!(EngineConfig, exclusive_flags) as u32;
        c.ok(CertCreateCertificateChainEngine(&config, &mut engine) != 0, GetLastError().into())?;
        CertFreeCertificateChainEngine(engine);
        config.size = core::mem::size_of::<EngineConfig>() as u32;
        c.eq(config.size.into(), super::by_width(52, 88))?;
        c.ok(CertCreateCertificateChainEngine(&config, &mut engine) != 0, GetLastError().into())?;
    }

    super::say(b"chain: full chain\r\n");
    // 2. The full chain.
    let p = para();
    let full = chain(c, engine, leaf, Some(T2030), inter_store, &p, 0)?;
    // SAFETY: the chain crypt32 gave.
    unsafe { c.eq((*full).status.error.into(), 0)? };
    let els = elements(full);
    c.eq(els.len() as u64, 3)?;
    for (i, der) in [LEAF, INTER, ROOT].iter().enumerate() {
        let e = element(full, i);
        c.eq(e.size.into(), super::by_width(32, 56))?;
        c.ok(is_cert(e, der), i as u64)?;
        c.eq(e.status.error.into(), 0)?;
    }
    c.eq((element(full, 0).status.info & 0xf).into(), CERT_TRUST_HAS_KEY_MATCH_ISSUER.into())?;
    c.eq((element(full, 1).status.info & 0xf).into(), CERT_TRUST_HAS_KEY_MATCH_ISSUER.into())?;
    c.eq(
        (element(full, 2).status.info & 0xf).into(),
        (CERT_TRUST_HAS_NAME_MATCH_ISSUER | CERT_TRUST_IS_SELF_SIGNED).into(),
    )?;
    // SAFETY: the chain crypt32 gave; one more reference, freed below.
    let dup = unsafe { CertDuplicateCertificateChain(full) };
    c.eq(dup as u64, full as u64)?;
    free(dup);
    let r = ssl(c, full, "www.chain.test", 0)?;
    c.eq(r.0, PASS.0)?;
    let r = ssl(c, full, "www.chain.test", 0)?;
    c.ok(r == PASS, r.0)?;
    let r = ssl(c, full, "WWW.Chain.TEST", 0)?;
    c.ok(r == PASS, r.0)?;
    let r = ssl(c, full, "x.wild.chain.test", 0)?;
    c.ok(r == PASS, r.0)?;
    let r = ssl(c, full, "10.0.0.1", 0)?;
    c.ok(r == PASS, r.0)?;
    let r = ssl(c, full, "a.b.wild.chain.test", 0)?;
    c.ok(r == (CERT_E_CN_NO_MATCH, 0, 0), r.0)?;
    let r = ssl(c, full, "chain.test", 0)?;
    c.ok(r == (CERT_E_CN_NO_MATCH, 0, 0), r.0)?;
    let r = ssl(c, full, "10.0.0.2", 0)?;
    c.ok(r == (CERT_E_CN_NO_MATCH, 0, 0), r.0)?;
    let r = ssl(c, full, "other.test", SECURITY_FLAG_IGNORE_CERT_CN_INVALID)?;
    c.ok(r == PASS, r.0)?;
    let r = policy(c, CERT_CHAIN_POLICY_BASE, full, 0, None)?;
    c.ok(r == PASS, r.0)?;
    free(full);
    // CERT_NAME_DNS_TYPE: the first DNS name, all of them as a
    // multi-string with CERT_NAME_SEARCH_ALL_NAMES_FLAG, the CN without
    // alternative names (what curl's host check reads).
    let mut text = [0u16; 64];
    // SAFETY: our contexts and buffer.
    unsafe {
        let n = super::CertGetNameStringW(leaf, 6, 0, null(), text.as_mut_ptr(), 64);
        c.eq(n.into(), 15)?;
        c.ok(text[..15].iter().copied().eq("www.chain.test\0".encode_utf16()), 0)?;
        let n = super::CertGetNameStringW(leaf, 6, 2, null(), null_mut(), 0);
        c.eq(n.into(), 34)?;
        let n = super::CertGetNameStringW(leaf, 6, 2, null(), text.as_mut_ptr(), 64);
        c.eq(n.into(), 34)?;
        let all = "www.chain.test\0*.wild.chain.test\0\0";
        c.ok(text[..34].iter().copied().eq(all.encode_utf16()), 0)?;
        let root = context(c, ROOT)?;
        let n = super::CertGetNameStringW(root, 6, 0, null(), text.as_mut_ptr(), 64);
        c.eq(n.into(), 21)?;
        c.ok(text[..21].iter().copied().eq("gala chain test root\0".encode_utf16()), 0)?;
        CertFreeCertificateContext(root);
    }

    super::say(b"chain: partial chain\r\n");
    // 3. No intermediate: a partial chain.
    let partial = chain(c, engine, leaf, Some(T2030), none, &p, 0)?;
    // SAFETY: the chain crypt32 gave.
    unsafe { c.eq((*partial).status.error.into(), CERT_TRUST_IS_PARTIAL_CHAIN.into())? };
    c.eq(elements(partial).len() as u64, 1)?;
    let r = policy(c, CERT_CHAIN_POLICY_BASE, partial, 0, None)?;
    c.eq(r.0, CERT_E_CHAINING)?;
    free(partial);

    super::say(b"chain: untrusted root\r\n");
    // 4. The default engine: the test root is not trusted there.
    let both = store_of(c, &[INTER, ROOT])?;
    let untrusted = chain(c, none, leaf, Some(T2030), both, &p, 0)?;
    // SAFETY: the chain crypt32 gave.
    unsafe { c.eq((*untrusted).status.error.into(), CERT_TRUST_IS_UNTRUSTED_ROOT.into())? };
    c.eq(elements(untrusted).len() as u64, 3)?;
    c.ok(is_cert(element(untrusted, 2), ROOT), 0)?;
    c.eq(element(untrusted, 2).status.error.into(), CERT_TRUST_IS_UNTRUSTED_ROOT.into())?;
    c.eq(element(untrusted, 0).status.error.into(), 0)?;
    let r = policy(c, CERT_CHAIN_POLICY_BASE, untrusted, 0, None)?;
    c.ok(r == (CERT_E_UNTRUSTEDROOT, 0, 2), r.0)?;
    let r = policy(
        c,
        CERT_CHAIN_POLICY_BASE,
        untrusted,
        CERT_CHAIN_POLICY_ALLOW_UNKNOWN_CA_FLAG,
        None,
    )?;
    c.ok(r == PASS, r.0)?;
    let r = ssl(c, untrusted, "www.chain.test", 0)?;
    c.ok(r == (CERT_E_UNTRUSTEDROOT, 0, 2), r.0)?;
    let r = ssl(c, untrusted, "www.chain.test", SECURITY_FLAG_IGNORE_UNKNOWN_CA)?;
    c.ok(r == PASS, r.0)?;
    free(untrusted);

    super::say(b"chain: time\r\n");
    // 5. Time.
    for t in [T2050, T2020] {
        let late = chain(c, engine, leaf, Some(t), inter_store, &p, 0)?;
        // SAFETY: the chain crypt32 gave.
        unsafe { c.eq((*late).status.error.into(), CERT_TRUST_IS_NOT_TIME_VALID.into())? };
        for i in 0..3 {
            c.eq(element(late, i).status.error.into(), CERT_TRUST_IS_NOT_TIME_VALID.into())?;
        }
        let r = ssl(c, late, "www.chain.test", 0)?;
        c.ok(r == (CERT_E_EXPIRED, 0, 0), r.0)?;
        let r = ssl(c, late, "www.chain.test", SECURITY_FLAG_IGNORE_CERT_DATE_INVALID)?;
        c.ok(r == PASS, r.0)?;
        let r = policy(c, CERT_CHAIN_POLICY_BASE, late, 0, None)?;
        c.ok(r == (CERT_E_EXPIRED, 0, 0), r.0)?;
        let flags = CERT_CHAIN_POLICY_IGNORE_ALL_NOT_TIME_VALID_FLAGS;
        let r = policy(c, CERT_CHAIN_POLICY_BASE, late, flags, None)?;
        c.ok(r == PASS, r.0)?;
        free(late);
    }

    super::say(b"chain: signatures\r\n");
    // 6. Signatures.
    let mut bad_bytes = [0u8; 1024];
    let bad_bytes = &mut bad_bytes[..LEAF.len()];
    bad_bytes.copy_from_slice(LEAF);
    let last = bad_bytes.len() - 1;
    bad_bytes[last] ^= 1; // inside the signature's s
    let bad = context(c, bad_bytes)?;
    let inter = context(c, INTER)?;
    let root = context(c, ROOT)?;
    // SAFETY: our contexts.
    unsafe {
        let verify = |subject: *const CertContext, issuer: *const CertContext| {
            super::CryptVerifyCertificateSignatureEx(
                0,
                X509_ASN_ENCODING,
                2,
                subject.cast(),
                2,
                issuer.cast(),
                0,
                null(),
            )
        };
        c.ok(verify(leaf, inter) != 0, GetLastError().into())?;
        c.ok(verify(inter, root) != 0, GetLastError().into())?;
        c.eq(verify(bad, inter) as u64, 0)?;
        c.eq(GetLastError().into(), NTE_BAD_SIGNATURE)?;
    }
    let forged = chain(c, engine, bad, Some(T2030), inter_store, &p, 0)?;
    c.eq(elements(forged).len() as u64, 3)?;
    c.eq(element(forged, 0).status.error.into(), CERT_TRUST_IS_NOT_SIGNATURE_VALID.into())?;
    c.eq(element(forged, 1).status.error.into(), 0)?;
    let r = policy(c, CERT_CHAIN_POLICY_BASE, forged, 0, None)?;
    c.ok(r == (TRUST_E_CERT_SIGNATURE, 0, 0), r.0)?;
    let r = ssl(c, forged, "www.chain.test", 0)?;
    c.ok(r == (TRUST_E_CERT_SIGNATURE, 0, 0), r.0)?;
    free(forged);

    super::say(b"chain: usage\r\n");
    // 7. Usage.
    let server_auth = c"1.3.6.1.5.5.7.3.1";
    let ids = [server_auth.as_ptr().cast::<u8>()];
    let mut usage = para();
    usage.usage = UsageMatch {
        kind: USAGE_MATCH_TYPE_OR,
        usage: EnhKeyUsage { count: 1, ids: ids.as_ptr() },
    };
    let client = context(c, CLIENT)?;
    let wrong = chain(c, engine, client, Some(T2030), inter_store, &usage, 0)?;
    // SAFETY: the chain crypt32 gave.
    unsafe { c.eq((*wrong).status.error.into(), CERT_TRUST_IS_NOT_VALID_FOR_USAGE.into())? };
    c.eq(element(wrong, 0).status.error.into(), CERT_TRUST_IS_NOT_VALID_FOR_USAGE.into())?;
    let r = policy(c, CERT_CHAIN_POLICY_BASE, wrong, 0, None)?;
    c.ok(r == (CERT_E_WRONG_USAGE, 0, 0), r.0)?;
    let flags = CERT_CHAIN_POLICY_IGNORE_WRONG_USAGE_FLAG;
    let r = policy(c, CERT_CHAIN_POLICY_BASE, wrong, flags, None)?;
    c.ok(r == PASS, r.0)?;
    free(wrong);
    usage.size = core::mem::size_of::<ChainPara>() as u32;
    c.eq(usage.size.into(), super::by_width(52, 96))?;
    let right = chain(c, engine, leaf, Some(T2030), inter_store, &usage, 0)?;
    // SAFETY: the chain crypt32 gave.
    unsafe { c.eq((*right).status.error.into(), 0)? };
    free(right);

    super::say(b"chain: basic constraints\r\n");
    // 8. Basic constraints.
    let signers = store_of(c, &[INTER, NOTCA])?;
    let underling = context(c, UNDERLING)?;
    let constrained = chain(c, engine, underling, Some(T2030), signers, &p, 0)?;
    c.eq(elements(constrained).len() as u64, 4)?;
    c.ok(is_cert(element(constrained, 1), NOTCA), 0)?;
    c.eq(element(constrained, 1).status.error.into(), CERT_TRUST_INVALID_BASIC_CONSTRAINTS.into())?;
    let r = policy(c, CERT_CHAIN_POLICY_BASIC_CONSTRAINTS, constrained, 0, None)?;
    c.ok(r == (TRUST_E_BASIC_CONSTRAINTS, 0, 1), r.0)?;
    free(constrained);

    super::say(b"chain: revocation\r\n");
    // 9. Revocation, from the CRLs in hAdditionalStore (offline).
    let unknown = CERT_TRUST_REVOCATION_STATUS_UNKNOWN;
    let offline = unknown | CERT_TRUST_IS_OFFLINE_REVOCATION;
    let cache = CERT_CHAIN_REVOCATION_CHECK_CACHE_ONLY;
    let flags = CERT_CHAIN_REVOCATION_CHECK_CHAIN_EXCLUDE_ROOT | cache;
    // No CRLs: the ones they name are out of reach.
    let revoke = chain(c, engine, leaf, Some(T2030), inter_store, &p, flags)?;
    let bits = revocation_bits(revoke);
    c.ok(bits == [offline, offline, 0], bits[0].into())?;
    let r = ssl(c, revoke, "www.chain.test", 0)?;
    c.ok(r == (CRYPT_E_REVOCATION_OFFLINE, 0, 0), r.0)?;
    let flags_end = CERT_CHAIN_POLICY_IGNORE_END_REV_UNKNOWN_FLAG;
    let r = policy(c, CERT_CHAIN_POLICY_BASE, revoke, flags_end, None)?;
    c.ok(r == (CRYPT_E_REVOCATION_OFFLINE, 0, 1), r.0)?;
    let r = ssl(c, revoke, "www.chain.test", SECURITY_FLAG_IGNORE_REVOCATION)?;
    c.ok(r == PASS, r.0)?;
    free(revoke);
    let end_only = CERT_CHAIN_REVOCATION_CHECK_END_CERT | cache;
    let revoke = chain(c, engine, leaf, Some(T2030), inter_store, &p, end_only)?;
    let bits = revocation_bits(revoke);
    c.ok(bits == [offline, 0, 0], bits[1].into())?;
    free(revoke);
    // The current CRLs: not revoked.
    let current = store_with_crls(c, &[INTER], &[INTER_CRL, ROOT_CRL])?;
    for f in [flags, CERT_CHAIN_REVOCATION_CHECK_CHAIN | cache] {
        let good = chain(c, engine, leaf, Some(T2030), current, &p, f)?;
        // SAFETY: the chain crypt32 gave.
        unsafe { c.eq((*good).status.error.into(), 0)? };
        let bits = revocation_bits(good);
        c.ok(bits == [0, 0, 0], bits[0].into())?;
        let r = ssl(c, good, "www.chain.test", 0)?;
        c.ok(r == PASS, r.0)?;
        free(good);
    }
    // A leaf naming no CRL: not checked by them.
    let client = context(c, CLIENT)?;
    let unchecked = chain(c, engine, client, Some(T2030), current, &p, flags)?;
    let bits = revocation_bits(unchecked);
    c.ok(bits == [unknown, 0, 0], bits[0].into())?;
    let r = policy(c, CERT_CHAIN_POLICY_BASE, unchecked, 0, None)?;
    c.ok(r == (CRYPT_E_NO_REVOCATION_CHECK, 0, 0), r.0)?;
    let r = policy(c, CERT_CHAIN_POLICY_BASE, unchecked, flags_end, None)?;
    c.ok(r == PASS, r.0)?;
    free(unchecked);
    // SAFETY: ours.
    unsafe { CertFreeCertificateContext(client) };
    // A revoked leaf.
    let revoked = context(c, REVOKED)?;
    let gone = chain(c, engine, revoked, Some(T2030), current, &p, flags)?;
    // SAFETY: the chain crypt32 gave.
    unsafe { c.eq((*gone).status.error.into(), CERT_TRUST_IS_REVOKED.into())? };
    let bits = revocation_bits(gone);
    c.ok(bits == [CERT_TRUST_IS_REVOKED, 0, 0], bits[0].into())?;
    let r = ssl(c, gone, "revoked.chain.test", 0)?;
    c.ok(r == (CRYPT_E_REVOKED, 0, 0), r.0)?;
    let r = policy(c, CERT_CHAIN_POLICY_BASE, gone, 0, None)?;
    c.ok(r == (CRYPT_E_REVOKED, 0, 0), r.0)?;
    let r = ssl(c, gone, "revoked.chain.test", SECURITY_FLAG_IGNORE_REVOCATION)?;
    c.ok(r == PASS, r.0)?;
    free(gone);
    // Out of date, absent or not verifying: offline (it names a CRL).
    let mut forged = [0u8; 512];
    let forged = &mut forged[..INTER_CRL.len()];
    forged.copy_from_slice(INTER_CRL);
    let last = forged.len() - 1;
    forged[last] ^= 1; // inside the signature's s
    let old = store_with_crls(c, &[INTER], &[INTER_OLD_CRL, ROOT_CRL])?;
    let broken = store_with_crls(c, &[INTER], &[forged, ROOT_CRL])?;
    for (i, s) in [old, inter_store, broken].into_iter().enumerate() {
        let stale = chain(c, engine, revoked, Some(T2030), s, &p, flags)?;
        let bits = revocation_bits(stale);
        c.ok(bits[0] == offline, i as u64)?;
        let r = ssl(c, stale, "revoked.chain.test", 0)?;
        c.ok(r == (CRYPT_E_REVOCATION_OFFLINE, 0, 0), r.0)?;
        free(stale);
    }
    // SAFETY: what we made.
    unsafe {
        CertFreeCertificateContext(revoked);
        for s in [current, old, broken] {
            CertCloseStore(s, 0);
        }
    }

    super::say(b"chain: machine roots\r\n");
    // 10. The machine's roots.
    let localhost = context(c, LOCALHOST)?;
    for default in [none, 1usize as Handle] {
        let trusted = chain(c, default, localhost, None, none, &p, 0)?;
        // SAFETY: the chain crypt32 gave.
        unsafe { c.eq((*trusted).status.error.into(), 0)? };
        c.eq(elements(trusted).len() as u64, 2)?;
        let r = ssl(c, trusted, "localhost", 0)?;
        c.ok(r == PASS, r.0)?;
        let r = ssl(c, trusted, "127.0.0.1", 0)?;
        c.ok(r == PASS, r.0)?;
        let r = ssl(c, trusted, "example.com", 0)?;
        c.ok(r == (CERT_E_CN_NO_MATCH, 0, 0), r.0)?;
        free(trusted);
    }
    let mut name = [0u16; 32];
    // SAFETY: crypt32 calls with our strings.
    unsafe {
        for store in ["CA", "MY"] {
            let s = CertOpenSystemStoreW(0, wide_text(store, &mut name));
            c.ok(!s.is_null(), GetLastError().into())?;
            CertCloseStore(s, 0);
        }
    }

    // SAFETY: what we made.
    unsafe {
        for cert in [leaf, bad, inter, root, client, underling, localhost] {
            CertFreeCertificateContext(cert);
        }
        CertFreeCertificateChainEngine(engine);
        for s in [root_store, inter_store, both, signers] {
            CertCloseStore(s, 0);
        }
    }
    Ok(())
}

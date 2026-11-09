//! CRLs (TLS step 6, revocation), with the results Microsoft documents
//! for CertCreateCRLContext, CRL_CONTEXT, CRL_INFO, CRL_ENTRY,
//! CertAddCRLContextToStore / CertAddEncodedCRLToStore,
//! CertEnumCRLsInStore, CertFindCRLInStore, CertDeleteCRLFromStore,
//! CertFindCertificateInCRL, CertVerifyCRLRevocation and
//! CertVerifyRevocation (CERT_REVOCATION_PARA, CERT_REVOCATION_STATUS),
//! and where the documentation is silent those Wine's conformance tests
//! record (dlls/crypt32/tests/crl.c and cert.c, checked on Windows):
//! E_INVALIDARG for encoding type 0 and for a CERT_REVOCATION_STATUS of
//! the wrong size, CRYPT_E_NO_REVOCATION_DLL for an encoding no
//! revocation provider takes, CRYPT_E_EXISTS for a second CRL of the same
//! issuer added with CERT_STORE_ADD_NEW (and an older one with
//! CERT_STORE_ADD_NEWER).
//!
//! The CRLs are `chain/make.sh`'s: the intermediate's (v2: authority key
//! identifier and CRL number, valid 2025..2045, listing the revoked leaf,
//! serial 0x16, keyCompromise), its out-of-date one (2020..2021) and the
//! root's (empty).
//!
//! 1. CertCreateCRLContext: the encoding copied, CRL_INFO's version,
//!    signature algorithm, issuer (the intermediate's subject), this and
//!    next update as FILETIMEs, the entry (serial little-endian, its
//!    reason code extension) and the CRL's extensions; encoding type 0
//!    is E_INVALIDARG, bytes that are no CRL fail; duplicate and free.
//! 2. A memory store: the added CRL belongs to it (hCertStore) and is
//!    enumerated (the end is CRYPT_E_NOT_FOUND); the same issuer's CRL
//!    again with CERT_STORE_ADD_NEW and its older one with
//!    CERT_STORE_ADD_NEWER are CRYPT_E_EXISTS; another issuer's is
//!    added; CertFindCRLInStore(CRL_FIND_ISSUED_BY) finds each issuer's,
//!    none for a certificate that issued none; CertDeleteCRLFromStore.
//! 3. CertFindCertificateInCRL: the revoked leaf's entry, none for the
//!    other leaf; CertVerifyCRLRevocation: FALSE for the revoked leaf,
//!    TRUE for the other and for no CRLs.
//! 4. CertVerifyRevocation (the default provider, offline: the CRLs in
//!    hCrlStore, CERT_VERIFY_CACHE_ONLY_BASED_REVOCATION): no contexts
//!    TRUE (a status of the wrong size E_INVALIDARG); encoding type 0
//!    CRYPT_E_NO_REVOCATION_DLL; a certificate that names no CRL
//!    CRYPT_E_NO_REVOCATION_CHECK, even with its issuer's current CRL in
//!    hCrlStore (Wine's tests record this on Windows); one that names a
//!    CRL out of reach CRYPT_E_REVOCATION_OFFLINE; the revoked leaf
//!    CRYPT_E_REVOKED with its index and reason
//!    (CRL_REASON_KEY_COMPROMISE); the other leaf TRUE; only the
//!    out-of-date CRL CRYPT_E_REVOCATION_OFFLINE.

use super::{
    Algorithm, Blob, Bool, CertCloseStore, CertContext, CertCreateCertificateContext,
    CertEnumCRLsInStore, CertFreeCertificateContext, CertInfo, CertOpenStore, Checks, Extension,
    GetLastError, Handle, X509_ASN_ENCODING, blob, c_str,
};
use core::ffi::c_void;
use core::ptr::{null, null_mut};

const INTER: &[u8] = include_bytes!("../chain/inter.cer");
const LEAF: &[u8] = include_bytes!("../chain/leaf.cer");
/// A leaf naming no CRL distribution point.
const CLIENT: &[u8] = include_bytes!("../chain/client.cer");
const REVOKED: &[u8] = include_bytes!("../chain/revoked.cer");
const INTER_CRL: &[u8] = include_bytes!("../chain/inter.crl");
const INTER_OLD_CRL: &[u8] = include_bytes!("../chain/inter-old.crl");
const ROOT_CRL: &[u8] = include_bytes!("../chain/root.crl");

/// FILETIMEs: 2025-01-01, 2045-01-01 (UTC).
const T2025: u64 = 0x01db_5be0_19ba_4000;
const T2045: u64 = 0x01f1_c82b_5d52_0000;

/// CRL_ENTRY.
#[repr(C)]
struct CrlEntry {
    serial: Blob,
    revoked_at: u64,
    extension_count: u32,
    extensions: *const Extension,
}

/// CRL_INFO.
#[repr(C)]
struct CrlInfo {
    version: u32,
    signature: Algorithm,
    issuer: Blob,
    this_update: u64,
    next_update: u64,
    entry_count: u32,
    entries: *const CrlEntry,
    extension_count: u32,
    extensions: *const Extension,
}

/// CRL_CONTEXT.
#[repr(C)]
pub(crate) struct CrlContext {
    encoding: u32,
    encoded: *const u8,
    encoded_len: u32,
    info: *const CrlInfo,
    store: Handle,
}

/// CERT_REVOCATION_PARA (up to pftTimeToUse; cbSize says so).
#[repr(C)]
struct RevocationPara {
    size: u32,
    issuer: *const CertContext,
    store_count: u32,
    stores: *const Handle,
    crl_store: Handle,
    time: *const u64,
}

/// CERT_REVOCATION_STATUS.
#[repr(C)]
struct RevocationStatus {
    size: u32,
    index: u32,
    error: u32,
    reason: u32,
    has_freshness: Bool,
    freshness: u32,
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "crypt32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "crypt32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn CertCreateCRLContext(encoding: u32, data: *const u8, len: u32) -> *const CrlContext;
    fn CertDuplicateCRLContext(crl: *const CrlContext) -> *const CrlContext;
    fn CertFreeCRLContext(crl: *const CrlContext) -> Bool;
    fn CertAddCRLContextToStore(
        store: Handle,
        crl: *const CrlContext,
        disposition: u32,
        out: *mut *const CrlContext,
    ) -> Bool;
    fn CertAddEncodedCRLToStore(
        store: Handle,
        encoding: u32,
        data: *const u8,
        len: u32,
        disposition: u32,
        out: *mut *const CrlContext,
    ) -> Bool;
    fn CertFindCRLInStore(
        store: Handle,
        encoding: u32,
        flags: u32,
        kind: u32,
        para: *const c_void,
        prev: *const CrlContext,
    ) -> *const CrlContext;
    fn CertDeleteCRLFromStore(crl: *const CrlContext) -> Bool;
    fn CertFindCertificateInCRL(
        cert: *const CertContext,
        crl: *const CrlContext,
        flags: u32,
        reserved: *const c_void,
        entry: *mut *const CrlEntry,
    ) -> Bool;
    fn CertVerifyCRLRevocation(
        encoding: u32,
        cert: *const CertInfo,
        count: u32,
        crls: *const *const CrlInfo,
    ) -> Bool;
    fn CertVerifyRevocation(
        encoding: u32,
        kind: u32,
        count: u32,
        contexts: *const *const CertContext,
        flags: u32,
        para: *const RevocationPara,
        status: *mut RevocationStatus,
    ) -> Bool;
}

const CERT_STORE_PROV_MEMORY: usize = 2;
const CERT_STORE_ADD_NEW: u32 = 1;
const CERT_STORE_ADD_ALWAYS: u32 = 4;
const CERT_STORE_ADD_NEWER: u32 = 6;
const CRL_FIND_ISSUED_BY: u32 = 1;
const CERT_CONTEXT_REVOCATION_TYPE: u32 = 1;
const CERT_VERIFY_CACHE_ONLY_BASED_REVOCATION: u32 = 2;
const CRL_REASON_KEY_COMPROMISE: u64 = 1;

const E_INVALIDARG: u64 = 0x8007_0057;
const CRYPT_E_NOT_FOUND: u64 = 0x8009_2004;
const CRYPT_E_EXISTS: u64 = 0x8009_2005;
const CRYPT_E_REVOKED: u64 = 0x8009_2010;
const CRYPT_E_NO_REVOCATION_DLL: u64 = 0x8009_2011;
const CRYPT_E_NO_REVOCATION_CHECK: u64 = 0x8009_2012;
const CRYPT_E_REVOCATION_OFFLINE: u64 = 0x8009_2013;

fn last_error() -> u64 {
    // SAFETY: plain call.
    unsafe { GetLastError() }.into()
}

fn cert(c: &mut Checks, der: &[u8]) -> Result<*const CertContext, u32> {
    // SAFETY: a DER certificate of ours.
    let p =
        unsafe { CertCreateCertificateContext(X509_ASN_ENCODING, der.as_ptr(), der.len() as u32) };
    c.ok(!p.is_null(), last_error())?;
    Ok(p)
}

fn crl(c: &mut Checks, der: &[u8]) -> Result<*const CrlContext, u32> {
    // SAFETY: a DER CRL of ours.
    let p = unsafe { CertCreateCRLContext(X509_ASN_ENCODING, der.as_ptr(), der.len() as u32) };
    c.ok(!p.is_null(), last_error())?;
    Ok(p)
}

/// The CRL context's encoding is `der`.
fn is_crl(p: *const CrlContext, der: &[u8]) -> bool {
    // SAFETY: a context crypt32 gave.
    !p.is_null()
        && unsafe { core::slice::from_raw_parts((*p).encoded, (*p).encoded_len as usize) } == der
}

/// A memory store holding the CRLs `crls`.
fn crl_store(c: &mut Checks, crls: &[&[u8]]) -> Result<Handle, u32> {
    // SAFETY: crypt32 calls with our buffers.
    unsafe {
        let s = CertOpenStore(CERT_STORE_PROV_MEMORY as *const u8, 0, 0, 0, null());
        c.ok(!s.is_null(), last_error())?;
        for der in crls {
            let ok = CertAddEncodedCRLToStore(
                s,
                X509_ASN_ENCODING,
                der.as_ptr(),
                der.len() as u32,
                CERT_STORE_ADD_ALWAYS,
                null_mut(),
            );
            c.ok(ok != 0, last_error())?;
        }
        Ok(s)
    }
}

/// CertVerifyRevocation of `certs`: (result, last error, status).
fn verify(
    certs: &[*const CertContext],
    encoding: u32,
    para: Option<&RevocationPara>,
) -> (Bool, u64, RevocationStatus) {
    let mut status = RevocationStatus {
        size: core::mem::size_of::<RevocationStatus>() as u32,
        index: 0xdead,
        error: 0xdead,
        reason: 0xdead,
        has_freshness: 0,
        freshness: 0,
    };
    // SAFETY: our contexts and structures.
    let ok = unsafe {
        CertVerifyRevocation(
            encoding,
            CERT_CONTEXT_REVOCATION_TYPE,
            certs.len() as u32,
            certs.as_ptr(),
            CERT_VERIFY_CACHE_ONLY_BASED_REVOCATION,
            para.map_or(null(), |p| p as *const RevocationPara),
            &mut status,
        )
    };
    (ok, last_error(), status)
}

pub fn crls(c: &mut Checks) -> Result<(), u32> {
    let inter = cert(c, INTER)?;
    let leaf = cert(c, LEAF)?;
    let revoked = cert(c, REVOKED)?;
    let client = cert(c, CLIENT)?;

    super::say(b"crl: contexts\r\n");
    // 1. Decoding.
    // SAFETY: crypt32 calls with our buffers; the structures it gives.
    unsafe {
        c.ok(CertCreateCRLContext(0, INTER_CRL.as_ptr(), INTER_CRL.len() as u32).is_null(), 0)?;
        c.eq(last_error(), E_INVALIDARG)?;
        c.ok(CertCreateCRLContext(X509_ASN_ENCODING, INTER.as_ptr(), 3).is_null(), 0)?;
        let x = crl(c, INTER_CRL)?;
        c.eq((*x).encoding.into(), X509_ASN_ENCODING.into())?;
        c.ok(is_crl(x, INTER_CRL), 0)?;
        c.ok((*x).encoded != INTER_CRL.as_ptr(), 0)?; // a copy
        c.ok((*x).store.is_null(), 0)?;
        let info = &*(*x).info;
        c.eq(info.version.into(), 1)?; // CRL_V2
        c.ok(c_str(info.signature.oid) == b"1.2.840.10045.4.3.2", 0)?;
        c.ok(blob(&info.issuer) == blob(&(*(*inter).info).subject), 0)?;
        c.eq(info.this_update, T2025)?;
        c.eq(info.next_update, T2045)?;
        c.eq(info.entry_count.into(), 1)?;
        let entry = &*info.entries;
        c.ok(blob(&entry.serial) == [0x16], 0)?;
        c.eq(entry.extension_count.into(), 1)?;
        c.ok(c_str((*entry.extensions).oid) == b"2.5.29.21", 0)?;
        c.eq(info.extension_count.into(), 2)?;
        let ext = core::slice::from_raw_parts(info.extensions, 2);
        c.ok(c_str(ext[0].oid) == b"2.5.29.35" && c_str(ext[1].oid) == b"2.5.29.20", 0)?;
        let dup = CertDuplicateCRLContext(x);
        c.ok(dup == x, 0)?;
        c.ok(CertFreeCRLContext(dup) != 0, 0)?;
        let root = crl(c, ROOT_CRL)?;
        c.eq((*(*root).info).entry_count.into(), 0)?;
        CertFreeCRLContext(root);

        super::say(b"crl: stores\r\n");
        // 2. Stores.
        let store = CertOpenStore(CERT_STORE_PROV_MEMORY as *const u8, 0, 0, 0, null());
        c.ok(!store.is_null(), last_error())?;
        let mut added = null();
        c.ok(CertAddCRLContextToStore(store, x, CERT_STORE_ADD_ALWAYS, &mut added) != 0, 0)?;
        c.ok(added != x && is_crl(added, INTER_CRL), 0)?;
        c.ok((*added).store == store, 0)?;
        CertFreeCRLContext(added);
        let first = CertEnumCRLsInStore(store, null());
        c.ok(is_crl(first, INTER_CRL), 0)?;
        c.ok(CertEnumCRLsInStore(store, first).is_null(), 0)?;
        c.eq(last_error(), CRYPT_E_NOT_FOUND)?;
        let add = |der: &[u8], disposition: u32| {
            CertAddEncodedCRLToStore(
                store,
                X509_ASN_ENCODING,
                der.as_ptr(),
                der.len() as u32,
                disposition,
                null_mut(),
            )
        };
        c.eq(add(INTER_CRL, CERT_STORE_ADD_NEW) as u64, 0)?;
        c.eq(last_error(), CRYPT_E_EXISTS)?;
        c.eq(add(INTER_OLD_CRL, CERT_STORE_ADD_NEWER) as u64, 0)?;
        c.eq(last_error(), CRYPT_E_EXISTS)?;
        c.ok(add(ROOT_CRL, CERT_STORE_ADD_NEW) != 0, last_error())?;
        let find = |issuer: *const CertContext, prev: *const CrlContext| {
            CertFindCRLInStore(store, X509_ASN_ENCODING, 0, CRL_FIND_ISSUED_BY, issuer.cast(), prev)
        };
        let by_inter = find(inter, null());
        c.ok(is_crl(by_inter, INTER_CRL), 0)?;
        c.ok(find(inter, by_inter).is_null(), 0)?;
        c.ok(find(leaf, null()).is_null(), 0)?;
        c.eq(last_error(), CRYPT_E_NOT_FOUND)?;
        // Delete the root's: the intermediate's alone is left.
        let mut n = 0;
        let mut doomed = null();
        let mut p = CertEnumCRLsInStore(store, null());
        while !p.is_null() {
            n += 1;
            if is_crl(p, ROOT_CRL) {
                doomed = CertDuplicateCRLContext(p);
            }
            p = CertEnumCRLsInStore(store, p);
        }
        c.eq(n, 2)?;
        c.ok(!doomed.is_null(), 0)?;
        c.ok(CertDeleteCRLFromStore(doomed) != 0, last_error())?;
        let only = CertEnumCRLsInStore(store, null());
        c.ok(is_crl(only, INTER_CRL), 0)?;
        c.ok(CertEnumCRLsInStore(store, only).is_null(), 0)?;
        CertCloseStore(store, 0);

        super::say(b"crl: entries\r\n");
        // 3. Entries.
        let mut entry: *const CrlEntry = null();
        c.ok(CertFindCertificateInCRL(revoked, x, 0, null(), &mut entry) != 0, last_error())?;
        c.ok(!entry.is_null() && blob(&(*entry).serial) == [0x16], 0)?;
        c.ok(CertFindCertificateInCRL(leaf, x, 0, null(), &mut entry) != 0, last_error())?;
        c.ok(entry.is_null(), 0)?;
        let infos = [(*x).info];
        c.eq(
            CertVerifyCRLRevocation(X509_ASN_ENCODING, (*revoked).info, 1, infos.as_ptr()) as u64,
            0,
        )?;
        c.ok(CertVerifyCRLRevocation(X509_ASN_ENCODING, (*leaf).info, 1, infos.as_ptr()) != 0, 0)?;
        c.ok(CertVerifyCRLRevocation(X509_ASN_ENCODING, (*revoked).info, 0, null()) != 0, 0)?;
        CertFreeCRLContext(x);

        super::say(b"crl: CertVerifyRevocation\r\n");
        // 4. The revocation provider.
        let mut status = RevocationStatus {
            size: 0,
            index: 0,
            error: 0,
            reason: 0,
            has_freshness: 0,
            freshness: 0,
        };
        let ok = CertVerifyRevocation(
            X509_ASN_ENCODING,
            CERT_CONTEXT_REVOCATION_TYPE,
            0,
            null(),
            0,
            null(),
            &mut status,
        );
        c.eq(ok as u64, 0)?;
        c.eq(last_error(), E_INVALIDARG)?;
        let (ok, _, _) = verify(&[], X509_ASN_ENCODING, None);
        c.ok(ok != 0, 0)?;
        let (ok, e, st) = verify(&[leaf], 0, None);
        c.ok(ok == 0 && e == CRYPT_E_NO_REVOCATION_DLL, e)?;
        c.ok(u64::from(st.error) == CRYPT_E_NO_REVOCATION_DLL && st.index == 0, st.error.into())?;
        let (ok, e, st) = verify(&[client], X509_ASN_ENCODING, None);
        c.ok(ok == 0 && e == CRYPT_E_NO_REVOCATION_CHECK, e)?;
        c.ok(u64::from(st.error) == CRYPT_E_NO_REVOCATION_CHECK && st.index == 0, st.error.into())?;
        let (ok, e, st) = verify(&[leaf], X509_ASN_ENCODING, None);
        c.ok(ok == 0 && e == CRYPT_E_REVOCATION_OFFLINE, e)?;
        c.ok(u64::from(st.error) == CRYPT_E_REVOCATION_OFFLINE && st.index == 0, st.error.into())?;
        let current = crl_store(c, &[INTER_CRL])?;
        let para = RevocationPara {
            size: core::mem::size_of::<RevocationPara>() as u32,
            issuer: inter,
            store_count: 0,
            stores: null(),
            crl_store: current,
            time: null(),
        };
        let (ok, e, st) = verify(&[revoked], X509_ASN_ENCODING, Some(&para));
        c.ok(ok == 0 && e == CRYPT_E_REVOKED, e)?;
        c.ok(u64::from(st.error) == CRYPT_E_REVOKED && st.index == 0, st.error.into())?;
        c.eq(st.reason.into(), CRL_REASON_KEY_COMPROMISE)?;
        let (ok, e, st) = verify(&[leaf, revoked], X509_ASN_ENCODING, Some(&para));
        c.ok(ok == 0 && e == CRYPT_E_REVOKED, e)?;
        c.eq(st.index.into(), 1)?;
        let (ok, e, st) = verify(&[leaf], X509_ASN_ENCODING, Some(&para));
        c.ok(ok != 0, e)?;
        c.eq(st.error.into(), 0)?;
        let (ok, e, _) = verify(&[client], X509_ASN_ENCODING, Some(&para));
        c.ok(ok == 0 && e == CRYPT_E_NO_REVOCATION_CHECK, e)?;
        let old = crl_store(c, &[INTER_OLD_CRL])?;
        let stale = RevocationPara { crl_store: old, ..para };
        let (ok, e, st) = verify(&[revoked], X509_ASN_ENCODING, Some(&stale));
        c.ok(ok == 0 && e == CRYPT_E_REVOCATION_OFFLINE, e)?;
        c.eq(st.error.into(), CRYPT_E_REVOCATION_OFFLINE)?;
        CertCloseStore(current, 0);
        CertCloseStore(old, 0);

        for p in [inter, leaf, revoked, client] {
            CertFreeCertificateContext(p);
        }
    }
    Ok(())
}

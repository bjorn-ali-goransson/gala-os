//! Certificate stores, as documented: memory stores (CertOpenStore's
//! CERT_STORE_PROV_MEMORY and the system stores: ROOT and CA filled from
//! gala's directories, `roots.rs`; the others empty, as gala has no
//! persistent certificate store yet, recorded once per open), adding
//! encoded certificates, CRLs and contexts with the CERT_STORE_ADD_*
//! dispositions, enumerating, finding, deleting, duplicating and closing.
//! A store holds one reference to each of its contexts, which are copies
//! whose hCertStore is the store; a context a caller holds holds the
//! store (cert.rs `Block`, crl.rs `CrlBlock`), so the store lives until
//! its last handle is closed and its last context held outside is freed,
//! as on Windows (a context's hCertStore stays usable). CRLs are a second
//! list beside the certificates (crl.rs); a CRL "exists" in a store when
//! one of the same issuer is there (CERT_STORE_ADD_NEWER compares their
//! thisUpdate). No CTLs. Built on cert.rs's and crl.rs's contexts through
//! their public functions.

#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use crate::cert::{
    CertContext, CertCreateCertificateContext, CertDuplicateCertificateContext,
    CertFreeCertificateContext, CertGetCertificateContextProperty, CertGetNameStringW, adopt,
    disown,
};
use crate::crl::{CertCreateCRLContext, CertDuplicateCRLContext, CertFreeCRLContext, CrlContext};
use core::ffi::c_void;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

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
    fn SetLastError(code: u32);
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "ntdll", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "ntdll", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GalaRecordMissing(name: *const u8, length: usize, what: u32, detail: u64) -> u32;
}

pub(crate) fn unsupported(what: &[u8], detail: u64) {
    // SAFETY: a valid buffer; 2 is GalaRecordMissing's "unsupported".
    unsafe { GalaRecordMissing(what.as_ptr(), what.len(), 2, detail) };
}

pub(crate) fn error<T>(code: u32, value: T) -> T {
    // SAFETY: plain call.
    unsafe { SetLastError(code) };
    value
}

const E_INVALIDARG: u32 = 0x8007_0057;
const CRYPT_E_NOT_FOUND: u32 = 0x8009_2004;
const CRYPT_E_EXISTS: u32 = 0x8009_2005;
const ERROR_NOT_SUPPORTED: u32 = 50;
const ERROR_NOT_ENOUGH_MEMORY: u32 = 8;
const STORE_MAGIC: u32 = 0x7473_6c67; // "glst"

/// A store: its certificate contexts (each holding one reference).
#[repr(C)]
pub struct Store {
    magic: u32,
    refs: AtomicU32,
    lock: AtomicBool,
    certs: *mut *const CertContext,
    len: usize,
    cap: usize,
    /// A collection's member stores (each a reference) with their
    /// CertAddStoreToCollection flags and priority, highest priority
    /// first.
    members: [(*mut c_void, u32, u32); MAX_MEMBERS],
    member_count: usize,
    /// The store's CRL contexts (each holding one reference).
    crls: List<CrlContext>,
}

/// A growable array of context pointers in the process heap.
#[repr(C)]
struct List<T> {
    items: *mut *const T,
    len: usize,
    cap: usize,
}

impl<T> List<T> {
    const fn new() -> List<T> {
        List { items: core::ptr::null_mut(), len: 0, cap: 0 }
    }
    fn items(&self) -> &[*const T] {
        if self.items.is_null() {
            return &[];
        }
        // SAFETY: our array of `len` pointers.
        unsafe { core::slice::from_raw_parts(self.items, self.len) }
    }
    fn push(&mut self, p: *const T) -> bool {
        if self.len == self.cap {
            let cap = (self.cap * 2).max(8);
            let bytes = cap * core::mem::size_of::<*const T>();
            // SAFETY: our array (or a new one).
            let n = unsafe {
                if self.items.is_null() {
                    HeapAlloc(GetProcessHeap(), 0, bytes)
                } else {
                    HeapReAlloc(GetProcessHeap(), 0, self.items.cast(), bytes)
                }
            };
            if n.is_null() {
                return false;
            }
            self.items = n.cast();
            self.cap = cap;
        }
        // SAFETY: inside the array's capacity.
        unsafe { *self.items.add(self.len) = p };
        self.len += 1;
        true
    }
    fn remove(&mut self, i: usize) -> *const T {
        // SAFETY: `i` < len; the rest moves down.
        unsafe {
            let p = *self.items.add(i);
            core::ptr::copy(self.items.add(i + 1), self.items.add(i), self.len - i - 1);
            self.len -= 1;
            p
        }
    }
    fn free(&mut self) {
        if !self.items.is_null() {
            // SAFETY: our array.
            unsafe { HeapFree(GetProcessHeap(), 0, self.items.cast()) };
        }
    }
}

/// The most stores a collection holds.
const MAX_MEMBERS: usize = 16;
/// The most stores one enumeration walks (a collection and its members,
/// nested), and how deep.
const MAX_WALKED: usize = 64;
const MAX_DEPTH: usize = 4;

/// Appends references to `s`'s members to `out` (depth first: each
/// member, then its own members), skipping stores already there (a
/// collection that contains itself) and past `MAX_WALKED`/`MAX_DEPTH`.
/// The caller closes them.
fn walk_members(s: &mut Store, out: &mut [*mut c_void; MAX_WALKED], n: &mut usize, depth: usize) {
    let mut direct = [core::ptr::null_mut(); MAX_MEMBERS];
    let count = s.with(|s| {
        for (slot, m) in direct.iter_mut().zip(&s.members[..s.member_count]) {
            *slot = m.0;
        }
        s.member_count
    });
    for &m in &direct[..count] {
        if *n == MAX_WALKED || out[..*n].contains(&m) {
            continue;
        }
        out[*n] = CertDuplicateStore(m);
        *n += 1;
        if depth + 1 < MAX_DEPTH
            && let Some(inner) = store(m)
        {
            walk_members(inner, out, n, depth + 1);
        }
    }
}
const CERT_PHYSICAL_STORE_ADD_ENABLE_FLAG: u32 = 1;

fn store<'a>(h: *mut c_void) -> Option<&'a mut Store> {
    if h.is_null() {
        return None;
    }
    // SAFETY: a store handle the caller got from us (magic checked).
    let s = unsafe { &mut *(h as *mut Store) };
    (s.magic == STORE_MAGIC).then_some(s)
}

impl Store {
    fn with<T>(&mut self, f: impl FnOnce(&mut Store) -> T) -> T {
        while self.lock.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err()
        {
            core::hint::spin_loop();
        }
        let r = f(self);
        self.lock.store(false, Ordering::Release);
        r
    }
    fn items(&self) -> &[*const CertContext] {
        if self.certs.is_null() {
            return &[];
        }
        // SAFETY: our array of `len` contexts.
        unsafe { core::slice::from_raw_parts(self.certs, self.len) }
    }
    fn push(&mut self, c: *const CertContext) -> bool {
        if self.len == self.cap {
            let cap = (self.cap * 2).max(8);
            let bytes = cap * core::mem::size_of::<*const CertContext>();
            // SAFETY: our array (or a new one).
            let p = unsafe {
                if self.certs.is_null() {
                    HeapAlloc(GetProcessHeap(), 0, bytes)
                } else {
                    HeapReAlloc(GetProcessHeap(), 0, self.certs.cast(), bytes)
                }
            };
            if p.is_null() {
                return false;
            }
            self.certs = p.cast();
            self.cap = cap;
        }
        // SAFETY: inside the array's capacity.
        unsafe { *self.certs.add(self.len) = c };
        self.len += 1;
        true
    }
    fn remove(&mut self, i: usize) -> *const CertContext {
        // SAFETY: `i` < len; the rest moves down.
        unsafe {
            let c = *self.certs.add(i);
            core::ptr::copy(self.certs.add(i + 1), self.certs.add(i), self.len - i - 1);
            self.len -= 1;
            c
        }
    }
}

fn new_store() -> *mut c_void {
    // SAFETY: plain call.
    let p = unsafe { HeapAlloc(GetProcessHeap(), 8, core::mem::size_of::<Store>()) } as *mut Store;
    if p.is_null() {
        return error(ERROR_NOT_ENOUGH_MEMORY, core::ptr::null_mut());
    }
    // SAFETY: a new block of the store's size.
    unsafe {
        p.write(Store {
            magic: STORE_MAGIC,
            refs: AtomicU32::new(1),
            lock: AtomicBool::new(false),
            certs: core::ptr::null_mut(),
            len: 0,
            cap: 0,
            members: [(core::ptr::null_mut(), 0, 0); MAX_MEMBERS],
            member_count: 0,
            crls: List::new(),
        })
    };
    p.cast()
}

const CERT_STORE_PROV_MSG: usize = 1;
const CERT_STORE_PROV_MEMORY: usize = 2;
const CERT_STORE_PROV_PKCS7: usize = 5;
const CERT_STORE_PROV_SYSTEM_A: usize = 9;
const CERT_STORE_PROV_SYSTEM_W: usize = 10;
const CERT_STORE_PROV_COLLECTION: usize = 11;
const CERT_STORE_PROV_SYSTEM_REGISTRY_W: usize = 13;

/// Whether the provider string `p` (a NUL-terminated ASCII name) is
/// `name`, case ignored.
fn provider_is(p: *const u8, name: &[u8]) -> bool {
    for (i, &n) in name.iter().enumerate() {
        // SAFETY: the caller's NUL-terminated string, read up to its end.
        let c = unsafe { *p.add(i) };
        if !c.eq_ignore_ascii_case(&n) {
            return false;
        }
    }
    // SAFETY: as above.
    unsafe { *p.add(name.len()) == 0 }
}

/// A system store by name: "ROOT" (any location) holds gala's machine
/// root store (`roots`: the certificates in System32\gala\roots, the
/// anchors Schannel trusts too), "CA" the intermediate certificates and
/// CRLs in System32\gala\ca (the CRLs revocation is checked with,
/// offline); the others are empty in gala, reported with the name's first
/// characters so a program that expects certificates there shows up.
fn system_store(name: &[u8]) -> *mut c_void {
    let store = new_store();
    if name.eq_ignore_ascii_case(b"ROOT") {
        if !store.is_null() {
            super::roots::load(store, super::roots::Directory::Roots);
        }
        return store;
    }
    if name.eq_ignore_ascii_case(b"CA") {
        if !store.is_null() {
            super::roots::load(store, super::roots::Directory::Ca);
        }
        return store;
    }
    let mut what = *b"crypt32.dll!CertOpenStore(system store, empty: ????????)";
    let at = what.len() - 9;
    for (slot, &c) in what[at..at + 8].iter_mut().zip(name) {
        *slot = if c.is_ascii_alphanumeric() { c } else { b'?' };
    }
    unsupported(&what, name.len() as u64);
    store
}

/// A wide system-store name as ASCII (at most 32 characters; others '?').
fn wide_name(p: *const u16, out: &mut [u8; 32]) -> usize {
    let n = wide_len(p).min(out.len());
    for (i, slot) in out[..n].iter_mut().enumerate() {
        // SAFETY: within the caller's string.
        let c = unsafe { *p.add(i) };
        *slot = u8::try_from(c).ok().filter(u8::is_ascii).unwrap_or(b'?');
    }
    n
}

/// An ASCII system-store name (at most 32 characters).
fn ascii_name(p: *const u8, out: &mut [u8; 32]) -> usize {
    if p.is_null() {
        return 0;
    }
    let mut n = 0;
    // SAFETY: the caller's NUL-terminated string, read up to its end.
    while n < out.len() && unsafe { *p.add(n) } != 0 {
        out[n] = unsafe { *p.add(n) };
        n += 1;
    }
    n
}

fn wide_len(p: *const u16) -> usize {
    if p.is_null() {
        return 0;
    }
    let mut n = 0;
    // SAFETY: the caller's NUL-terminated string.
    while unsafe { *p.add(n) } != 0 {
        n += 1;
    }
    n
}

/// CertOpenStore: a memory store (CERT_STORE_PROV_MEMORY, "Memory"), a
/// collection (CERT_STORE_PROV_COLLECTION, "Collection": a store whose
/// members CertAddStoreToCollection adds), a system store (empty), or a
/// memory store of a message's certificates (CERT_STORE_PROV_MSG with a
/// decoded message handle, CERT_STORE_PROV_PKCS7 with an encoded one).
/// Other providers fail with ERROR_NOT_SUPPORTED (recorded).
#[unsafe(no_mangle)]
pub extern "system" fn CertOpenStore(
    provider: *const u8,
    _encoding: u32,
    _prov: usize,
    _flags: u32,
    para: *const c_void,
) -> *mut c_void {
    let id = provider as usize;
    if id < 0x10000 {
        return match id {
            CERT_STORE_PROV_MEMORY | CERT_STORE_PROV_COLLECTION => new_store(),
            CERT_STORE_PROV_MSG => message_store(para as *mut c_void),
            CERT_STORE_PROV_PKCS7 => {
                let der = blob(para);
                let h = crate::msg::decode(der);
                if h.is_null() {
                    return core::ptr::null_mut(); // its error is set
                }
                let s = message_store(h);
                crate::msg::CryptMsgClose(h);
                s
            }
            CERT_STORE_PROV_SYSTEM_W | CERT_STORE_PROV_SYSTEM_REGISTRY_W => {
                let mut name = [0u8; 32];
                let n = wide_name(para.cast(), &mut name);
                system_store(&name[..n])
            }
            CERT_STORE_PROV_SYSTEM_A => {
                let mut name = [0u8; 32];
                let n = ascii_name(para.cast(), &mut name);
                system_store(&name[..n])
            }
            _ => {
                unsupported(b"crypt32.dll!CertOpenStore(provider)", id as u64);
                error(ERROR_NOT_SUPPORTED, core::ptr::null_mut())
            }
        };
    }
    if provider_is(provider, b"Memory") || provider_is(provider, b"Collection") {
        return new_store();
    }
    if provider_is(provider, b"System") {
        let mut name = [0u8; 32];
        let n = wide_name(para.cast(), &mut name);
        return system_store(&name[..n]);
    }
    unsupported(b"crypt32.dll!CertOpenStore(provider)", 0);
    error(ERROR_NOT_SUPPORTED, core::ptr::null_mut())
}

/// A memory store holding a decoded message's certificates
/// (CERT_STORE_PROV_MSG; CERT_STORE_PROV_PKCS7 decodes the blob first).
/// The message's CRLs are not added (recorded when it has any).
fn message_store(h: *mut c_void) -> *mut c_void {
    let certs = match crate::msg::certificates(h) {
        Ok(c) => c,
        Err(e) => return error(e, core::ptr::null_mut()),
    };
    if crate::msg::signed_of(h).is_ok_and(|s| !s.crls.is_empty()) {
        unsupported(b"crypt32.dll!CertOpenStore(message CRLs not added)", 0);
    }
    let store = new_store();
    if store.is_null() {
        return store;
    }
    let mut d = crate::cert::Der::new(certs);
    while d.peek_tag().is_some() {
        let Ok(t) = d.next() else { break };
        if t.tag != crate::cert::SEQUENCE {
            unsupported(
                b"crypt32.dll!CertOpenStore(message: an attribute certificate, skipped)",
                t.tag.into(),
            );
            continue;
        }
        CertAddEncodedCertificateToStore(
            store,
            1,
            t.whole.as_ptr(),
            t.whole.len() as u32,
            CERT_STORE_ADD_ALWAYS,
            core::ptr::null_mut(),
        );
    }
    store
}

/// CertOpenSystemStoreW: the system store `name` (see `system_store`).
#[unsafe(no_mangle)]
pub extern "system" fn CertOpenSystemStoreW(_prov: usize, name: *const u16) -> *mut c_void {
    let mut ascii = [0u8; 32];
    let n = wide_name(name, &mut ascii);
    system_store(&ascii[..n])
}

/// CertOpenSystemStoreA: as the W form.
#[unsafe(no_mangle)]
pub extern "system" fn CertOpenSystemStoreA(_prov: usize, name: *const u8) -> *mut c_void {
    let mut ascii = [0u8; 32];
    let n = ascii_name(name, &mut ascii);
    system_store(&ascii[..n])
}

/// CertDuplicateStore: another reference to the store.
#[unsafe(no_mangle)]
pub extern "system" fn CertDuplicateStore(h: *mut c_void) -> *mut c_void {
    if let Some(s) = store(h) {
        s.refs.fetch_add(1, Ordering::AcqRel);
    }
    h
}

/// A reference to the store held by a context a caller holds (cert.rs).
pub(crate) fn hold(h: *mut c_void) {
    if let Some(s) = store(h) {
        s.refs.fetch_add(1, Ordering::AcqRel);
    }
}

/// The reference `hold` took.
pub(crate) fn release(h: *mut c_void) {
    CertCloseStore(h, 0);
}

/// CertCloseStore: one reference less (handles, and the contexts callers
/// hold); the last releases the store's contexts.
#[unsafe(no_mangle)]
pub extern "system" fn CertCloseStore(h: *mut c_void, _flags: u32) -> i32 {
    let Some(s) = store(h) else { return 1 };
    if s.refs.fetch_sub(1, Ordering::AcqRel) == 1 {
        // No caller holds a context of it (each would hold the store).
        for &c in s.items() {
            disown(c);
            CertFreeCertificateContext(c);
        }
        for &c in s.crls.items() {
            crate::crl::disown(c);
            CertFreeCRLContext(c);
        }
        s.crls.free();
        let members = s.members;
        for m in &members[..s.member_count] {
            CertCloseStore(m.0, 0);
        }
        s.magic = 0;
        // SAFETY: our blocks.
        unsafe {
            if !s.certs.is_null() {
                HeapFree(GetProcessHeap(), 0, s.certs.cast());
            }
            HeapFree(GetProcessHeap(), 0, h.cast());
        }
    }
    1
}

/// The context's encoding.
fn encoding<'a>(c: *const CertContext) -> &'a [u8] {
    // SAFETY: a context of ours (its encoding).
    unsafe { core::slice::from_raw_parts((*c).encoded, (*c).encoded_len as usize) }
}

const CERT_STORE_ADD_NEW: u32 = 1;
const CERT_STORE_ADD_USE_EXISTING: u32 = 2;
const CERT_STORE_ADD_REPLACE_EXISTING: u32 = 3;
const CERT_STORE_ADD_ALWAYS: u32 = 4;
const CERT_STORE_ADD_REPLACE_EXISTING_INHERIT_PROPERTIES: u32 = 5;
const CERT_STORE_ADD_NEWER: u32 = 6;
const CERT_STORE_ADD_NEWER_INHERIT_PROPERTIES: u32 = 7;

/// Adds a copy of `cert` to the store as `disposition` says; the context
/// in the store (one more reference) into `out`.
fn add(
    h: *mut c_void,
    cert: *const CertContext,
    disposition: u32,
    out: *mut *const CertContext,
) -> i32 {
    if cert.is_null() || !(1..=7).contains(&disposition) {
        return error(E_INVALIDARG, 0);
    }
    if !out.is_null() {
        // SAFETY: the caller's output pointer.
        unsafe { *out = core::ptr::null() };
    }
    let bytes = encoding(cert);
    // SAFETY: a context of ours: its dwCertEncodingType, kept by the copy.
    let kind = unsafe { (*cert).encoding };
    let copy = CertCreateCertificateContext(kind, bytes.as_ptr(), bytes.len() as u32);
    if copy.is_null() {
        return 0;
    }
    // A collection's adds go to its member that takes them.
    let h = store(h).map_or(h, |s| {
        s.with(|s| {
            s.members[..s.member_count]
                .iter()
                .find(|m| m.1 & CERT_PHYSICAL_STORE_ADD_ENABLE_FLAG != 0)
                .map_or(h, |m| m.0)
        })
    });
    let Some(s) = store(h) else {
        // No store: just the copy.
        if out.is_null() {
            CertFreeCertificateContext(copy);
        } else {
            // SAFETY: as above.
            unsafe { *out = copy };
        }
        return 1;
    };
    if matches!(disposition, CERT_STORE_ADD_NEWER | CERT_STORE_ADD_NEWER_INHERIT_PROPERTIES) {
        unsupported(b"crypt32.dll!CertAddCertificateContextToStore(ADD_NEWER)", disposition.into());
    }
    let result = s.with(|s| {
        let existing = s.items().iter().position(|&c| encoding(c) == bytes);
        match (existing, disposition) {
            (Some(_), CERT_STORE_ADD_NEW) => Err(CRYPT_E_EXISTS),
            (Some(i), CERT_STORE_ADD_USE_EXISTING) => Ok((s.items()[i], true)),
            (Some(_), CERT_STORE_ADD_ALWAYS) | (None, _) => {
                if s.push(copy) {
                    adopt(copy, h);
                    Ok((copy, false))
                } else {
                    Err(ERROR_NOT_ENOUGH_MEMORY)
                }
            }
            (Some(i), _) => {
                // Replace (and the ADD_NEWER kinds, as replace).
                let old = s.remove(i);
                disown(old);
                CertFreeCertificateContext(old);
                if s.push(copy) {
                    adopt(copy, h);
                    Ok((copy, false))
                } else {
                    Err(ERROR_NOT_ENOUGH_MEMORY)
                }
            }
        }
    });
    let _ = (CERT_STORE_ADD_REPLACE_EXISTING, CERT_STORE_ADD_REPLACE_EXISTING_INHERIT_PROPERTIES);
    match result {
        Ok((c, existing)) => {
            if existing {
                CertFreeCertificateContext(copy);
            }
            if !out.is_null() {
                // SAFETY: as above.
                unsafe { *out = CertDuplicateCertificateContext(c) };
            }
            1
        }
        Err(e) => {
            CertFreeCertificateContext(copy);
            error(e, 0)
        }
    }
}

/// CertAddEncodedCertificateToStore: the DER certificate decoded and
/// added (see CertAddCertificateContextToStore).
#[unsafe(no_mangle)]
pub extern "system" fn CertAddEncodedCertificateToStore(
    h: *mut c_void,
    encoding_type: u32,
    data: *const u8,
    len: u32,
    disposition: u32,
    out: *mut *const CertContext,
) -> i32 {
    let c = CertCreateCertificateContext(encoding_type, data, len);
    if c.is_null() {
        return 0;
    }
    let r = add(h, c, disposition, out);
    CertFreeCertificateContext(c);
    r
}

/// CertAddCertificateContextToStore: a copy added: CERT_STORE_ADD_NEW
/// fails with CRYPT_E_EXISTS when the store has the certificate,
/// _USE_EXISTING gives that one, _REPLACE_EXISTING replaces it, _ALWAYS
/// adds a duplicate (the _NEWER kinds replace, recorded).
#[unsafe(no_mangle)]
pub extern "system" fn CertAddCertificateContextToStore(
    h: *mut c_void,
    cert: *const CertContext,
    disposition: u32,
    out: *mut *const CertContext,
) -> i32 {
    add(h, cert, disposition, out)
}

/// The context after `prev` in the store (one more reference), freeing
/// `prev`, or null with CRYPT_E_NOT_FOUND at the end.
fn next_after(
    h: *mut c_void,
    prev: *const CertContext,
    mut keep: impl FnMut(*const CertContext) -> bool,
) -> *const CertContext {
    let Some(s) = store(h) else {
        if !prev.is_null() {
            CertFreeCertificateContext(prev);
        }
        return error(E_INVALIDARG, core::ptr::null());
    };
    // The store's own certificates, then (a collection) its members', in
    // order and depth first (a member may be a collection itself:
    // Chromium's issuer lookup nests them): the next one after `prev`
    // that `keep` keeps.
    let mut members = [core::ptr::null_mut(); MAX_WALKED];
    let mut count = 0;
    walk_members(s, &mut members, &mut count, 0);
    let mut passed = prev.is_null();
    let mut found = None;
    for target in core::iter::once(h).chain(members[..count].iter().copied()) {
        if found.is_some() {
            break;
        }
        let Some(t) = store(target) else { continue };
        found = t.with(|t| {
            for &c in t.items() {
                if !passed {
                    passed = c == prev;
                    continue;
                }
                if keep(c) {
                    return Some(CertDuplicateCertificateContext(c));
                }
            }
            None
        });
    }
    for &m in &members[..count] {
        CertCloseStore(m, 0);
    }
    if !prev.is_null() {
        CertFreeCertificateContext(prev);
    }
    found.unwrap_or_else(|| error(CRYPT_E_NOT_FOUND, core::ptr::null()))
}

/// CertAddStoreToCollection(collection, sibling, flags, priority): the
/// collection also enumerates and finds `sibling`'s certificates, members
/// of higher priority first; with CERT_PHYSICAL_STORE_ADD_ENABLE_FLAG the
/// collection's adds go to it. The collection holds a reference.
#[unsafe(no_mangle)]
pub extern "system" fn CertAddStoreToCollection(
    collection: *mut c_void,
    sibling: *mut c_void,
    flags: u32,
    priority: u32,
) -> i32 {
    let (Some(s), Some(_)) = (store(collection), store(sibling)) else {
        return error(E_INVALIDARG, 0);
    };
    if collection == sibling {
        return error(E_INVALIDARG, 0);
    }
    let added = s.with(|s| {
        if s.member_count == MAX_MEMBERS {
            return false;
        }
        let at = s.members[..s.member_count]
            .iter()
            .position(|m| m.2 < priority)
            .unwrap_or(s.member_count);
        s.members.copy_within(at..s.member_count, at + 1);
        s.members[at] = (CertDuplicateStore(sibling), flags, priority);
        s.member_count += 1;
        true
    });
    if !added {
        unsupported(b"crypt32.dll!CertAddStoreToCollection(too many stores)", MAX_MEMBERS as u64);
        return error(ERROR_NOT_ENOUGH_MEMORY, 0);
    }
    1
}

/// CertRemoveStoreFromCollection: `sibling` is no longer a member.
#[unsafe(no_mangle)]
pub extern "system" fn CertRemoveStoreFromCollection(
    collection: *mut c_void,
    sibling: *mut c_void,
) {
    let Some(s) = store(collection) else { return };
    let removed = s.with(|s| {
        let i = s.members[..s.member_count].iter().position(|m| m.0 == sibling)?;
        let m = s.members[i].0;
        s.members.copy_within(i + 1..s.member_count, i);
        s.member_count -= 1;
        Some(m)
    });
    if let Some(m) = removed {
        CertCloseStore(m, 0);
    }
}

/// CertEnumCertificatesInStore: the next certificate.
#[unsafe(no_mangle)]
pub extern "system" fn CertEnumCertificatesInStore(
    h: *mut c_void,
    prev: *const CertContext,
) -> *const CertContext {
    next_after(h, prev, |_| true)
}

const CERT_FIND_ANY: u32 = 0;
const CERT_FIND_SHA1_HASH: u32 = 0x1_0000;
const CERT_FIND_SUBJECT_NAME: u32 = 0x2_0007;
const CERT_FIND_ISSUER_STR_W: u32 = 0x8_0004;
const CERT_FIND_SUBJECT_STR_W: u32 = 0x8_0007;
const CERT_FIND_EXISTING: u32 = 0xd_0000;
const CERT_FIND_SUBJECT_CERT: u32 = 0xb_0000;
const CERT_NAME_SIMPLE_DISPLAY_TYPE: u32 = 4;
const CERT_NAME_ISSUER_FLAG: u32 = 1;

/// CRYPT_DATA_BLOB.
#[repr(C)]
struct DataBlob {
    size: u32,
    data: *const u8,
}

fn blob<'a>(p: *const c_void) -> &'a [u8] {
    if p.is_null() {
        return &[];
    }
    // SAFETY: the caller's blob.
    unsafe {
        let b = &*(p as *const DataBlob);
        if b.data.is_null() { &[] } else { core::slice::from_raw_parts(b.data, b.size as usize) }
    }
}

fn fold(c: u16) -> u16 {
    if (0x41..=0x5a).contains(&c) { c + 0x20 } else { c }
}

/// Whether the certificate's subject (or issuer) name, as
/// CertNameToStr-like text (every attribute's value), contains `needle`,
/// case ignored.
fn name_contains(c: *const CertContext, issuer: bool, needle: &[u16]) -> bool {
    let mut text = [0u16; 512];
    let flags = if issuer { CERT_NAME_ISSUER_FLAG } else { 0 };
    // CERT_NAME_RDN_TYPE (2): all the attributes' values.
    let n = CertGetNameStringW(c, 2, flags, core::ptr::null(), text.as_mut_ptr(), 512) as usize;
    let hay = &text[..n.saturating_sub(1)];
    let mut simple = [0u16; 256];
    let m = CertGetNameStringW(
        c,
        CERT_NAME_SIMPLE_DISPLAY_TYPE,
        flags,
        core::ptr::null(),
        simple.as_mut_ptr(),
        256,
    ) as usize;
    let simple = &simple[..m.saturating_sub(1)];
    let contains = |h: &[u16]| {
        needle.is_empty()
            || h.windows(needle.len())
                .any(|w| w.iter().zip(needle).all(|(&a, &b)| fold(a) == fold(b)))
    };
    contains(hay) || contains(simple)
}

/// CertFindCertificateInStore: the next certificate after `prev` that
/// matches: CERT_FIND_ANY, CERT_FIND_SHA1_HASH (a hash blob),
/// CERT_FIND_SUBJECT_NAME (the encoded name), CERT_FIND_SUBJECT_STR_W /
/// CERT_FIND_ISSUER_STR_W (a substring of the name, case ignored),
/// CERT_FIND_EXISTING (the same certificate), CERT_FIND_SUBJECT_CERT (a
/// CERT_INFO's issuer and serial number). Others are recorded.
#[unsafe(no_mangle)]
pub extern "system" fn CertFindCertificateInStore(
    h: *mut c_void,
    _encoding: u32,
    _flags: u32,
    kind: u32,
    para: *const c_void,
    prev: *const CertContext,
) -> *const CertContext {
    match kind {
        CERT_FIND_ANY => next_after(h, prev, |_| true),
        CERT_FIND_SHA1_HASH => {
            let want = blob(para);
            next_after(h, prev, |c| {
                let mut hash = [0u8; 20];
                let mut n = 20u32;
                CertGetCertificateContextProperty(c, 3, hash.as_mut_ptr(), &mut n) != 0
                    && &hash[..n as usize] == want
            })
        }
        CERT_FIND_SUBJECT_NAME => {
            let want = blob(para);
            next_after(h, prev, |c| {
                // SAFETY: a context of ours: its CERT_INFO's subject blob.
                let subject = unsafe {
                    let info = &*(*c).info;
                    core::slice::from_raw_parts(info.subject.data, info.subject.size as usize)
                };
                subject == want
            })
        }
        CERT_FIND_SUBJECT_STR_W | CERT_FIND_ISSUER_STR_W => {
            let text = para as *const u16;
            // SAFETY: the caller's NUL-terminated string.
            let needle = unsafe { core::slice::from_raw_parts(text, wide_len(text)) };
            let issuer = kind == CERT_FIND_ISSUER_STR_W;
            next_after(h, prev, |c| name_contains(c, issuer, needle))
        }
        CERT_FIND_SUBJECT_CERT => {
            let want = para as *const crate::cert::CertInfo;
            if want.is_null() {
                return error(E_INVALIDARG, core::ptr::null());
            }
            // SAFETY: the caller's CERT_INFO: its issuer and serial number.
            let (issuer, serial) = unsafe { ((*want).issuer, (*want).serial) };
            let (issuer, serial) = (blob_of(&issuer), blob_of(&serial));
            next_after(h, prev, |c| {
                // SAFETY: a context of ours.
                let info = unsafe { &*(*c).info };
                blob_of(&info.issuer) == issuer && blob_of(&info.serial) == serial
            })
        }
        CERT_FIND_EXISTING => {
            let other = para as *const CertContext;
            if other.is_null() {
                return error(E_INVALIDARG, core::ptr::null());
            }
            let want = encoding(other);
            next_after(h, prev, |c| encoding(c) == want)
        }
        _ => {
            unsupported(b"crypt32.dll!CertFindCertificateInStore(type)", kind.into());
            if !prev.is_null() {
                CertFreeCertificateContext(prev);
            }
            error(CRYPT_E_NOT_FOUND, core::ptr::null())
        }
    }
}

fn blob_of<'a>(b: &crate::cert::Blob) -> &'a [u8] {
    if b.data.is_null() {
        return &[];
    }
    // SAFETY: a blob of a CERT_INFO (the caller's or ours).
    unsafe { core::slice::from_raw_parts(b.data, b.size as usize) }
}

/// CertGetSubjectCertificateFromStore: the certificate with the CERT_INFO's
/// issuer and serial number (CERT_FIND_SUBJECT_CERT), or CRYPT_E_NOT_FOUND.
#[unsafe(no_mangle)]
pub extern "system" fn CertGetSubjectCertificateFromStore(
    h: *mut c_void,
    encoding: u32,
    info: *const crate::cert::CertInfo,
) -> *const CertContext {
    if info.is_null() {
        return error(E_INVALIDARG, core::ptr::null());
    }
    CertFindCertificateInStore(
        h,
        encoding,
        0,
        CERT_FIND_SUBJECT_CERT,
        info.cast(),
        core::ptr::null(),
    )
}

/// CertDeleteCertificateFromStore: the context's certificate removed from
/// its store; the context is freed in any case.
#[unsafe(no_mangle)]
pub extern "system" fn CertDeleteCertificateFromStore(cert: *const CertContext) -> i32 {
    if cert.is_null() {
        return error(E_INVALIDARG, 0);
    }
    // SAFETY: a context of ours.
    let h = unsafe { (*cert).store };
    if let Some(s) = store(h) {
        let removed = s.with(|s| s.items().iter().position(|&c| c == cert).map(|i| s.remove(i)));
        if let Some(c) = removed {
            disown(c);
            CertFreeCertificateContext(c);
        }
    }
    CertFreeCertificateContext(cert);
    1
}

// ---- CRLs ----

fn crl_encoding<'a>(c: *const CrlContext) -> &'a [u8] {
    // SAFETY: a CRL context of ours (its encoding).
    unsafe { core::slice::from_raw_parts((*c).encoded, (*c).encoded_len as usize) }
}

fn crl_issuer<'a>(c: *const CrlContext) -> &'a [u8] {
    // SAFETY: a CRL context of ours: its CRL_INFO's issuer blob.
    unsafe {
        let info = &*(*c).info;
        core::slice::from_raw_parts(info.issuer.data, info.issuer.size as usize)
    }
}

fn this_update(c: *const CrlContext) -> u64 {
    // SAFETY: a CRL context of ours.
    unsafe { (*(*c).info).this_update }
}

/// Adds a copy of `crl` to the store as `disposition` says (a CRL of the
/// same issuer is the existing one); the CRL in the store (one more
/// reference) into `out`.
fn add_crl(
    h: *mut c_void,
    crl: *const CrlContext,
    disposition: u32,
    out: *mut *const CrlContext,
) -> i32 {
    if crl.is_null() || !(1..=7).contains(&disposition) {
        return error(E_INVALIDARG, 0);
    }
    if !out.is_null() {
        // SAFETY: the caller's output pointer.
        unsafe { *out = core::ptr::null() };
    }
    let bytes = crl_encoding(crl);
    // SAFETY: a context of ours: its encoding type, kept by the copy.
    let kind = unsafe { (*crl).encoding };
    let copy = CertCreateCRLContext(kind, bytes.as_ptr(), bytes.len() as u32);
    if copy.is_null() {
        return 0;
    }
    // A collection's adds go to its member that takes them.
    let h = store(h).map_or(h, |s| {
        s.with(|s| {
            s.members[..s.member_count]
                .iter()
                .find(|m| m.1 & CERT_PHYSICAL_STORE_ADD_ENABLE_FLAG != 0)
                .map_or(h, |m| m.0)
        })
    });
    let Some(s) = store(h) else {
        if out.is_null() {
            CertFreeCRLContext(copy);
        } else {
            // SAFETY: as above.
            unsafe { *out = copy };
        }
        return 1;
    };
    let result = s.with(|s| {
        let issuer = crl_issuer(copy);
        let existing = s.crls.items().iter().position(|&c| crl_issuer(c) == issuer);
        let newer = existing.is_none_or(|i| this_update(copy) > this_update(s.crls.items()[i]));
        match (existing, disposition) {
            (Some(_), CERT_STORE_ADD_NEW) => Err(CRYPT_E_EXISTS),
            (Some(i), CERT_STORE_ADD_USE_EXISTING) => Ok((s.crls.items()[i], true)),
            (Some(_), CERT_STORE_ADD_NEWER | CERT_STORE_ADD_NEWER_INHERIT_PROPERTIES) if !newer => {
                Err(CRYPT_E_EXISTS)
            }
            (Some(_), CERT_STORE_ADD_ALWAYS) | (None, _) => {
                if s.crls.push(copy) {
                    crate::crl::adopt(copy, h);
                    Ok((copy, false))
                } else {
                    Err(ERROR_NOT_ENOUGH_MEMORY)
                }
            }
            (Some(i), _) => {
                // Replace (and a newer one).
                let old = s.crls.remove(i);
                crate::crl::disown(old);
                CertFreeCRLContext(old);
                if s.crls.push(copy) {
                    crate::crl::adopt(copy, h);
                    Ok((copy, false))
                } else {
                    Err(ERROR_NOT_ENOUGH_MEMORY)
                }
            }
        }
    });
    match result {
        Ok((c, existing)) => {
            if existing {
                CertFreeCRLContext(copy);
            }
            if !out.is_null() {
                // SAFETY: as above.
                unsafe { *out = CertDuplicateCRLContext(c) };
            }
            1
        }
        Err(e) => {
            CertFreeCRLContext(copy);
            error(e, 0)
        }
    }
}

/// CertAddCRLContextToStore: a copy added: CERT_STORE_ADD_NEW fails with
/// CRYPT_E_EXISTS when the store has a CRL of the same issuer,
/// _USE_EXISTING gives that one, _REPLACE_EXISTING replaces it, _ALWAYS
/// adds another, _NEWER replaces it if the new one's thisUpdate is later
/// (else CRYPT_E_EXISTS).
#[unsafe(no_mangle)]
pub extern "system" fn CertAddCRLContextToStore(
    h: *mut c_void,
    crl: *const CrlContext,
    disposition: u32,
    out: *mut *const CrlContext,
) -> i32 {
    add_crl(h, crl, disposition, out)
}

/// CertAddEncodedCRLToStore: the DER CRL decoded and added (see
/// CertAddCRLContextToStore).
#[unsafe(no_mangle)]
pub extern "system" fn CertAddEncodedCRLToStore(
    h: *mut c_void,
    encoding_type: u32,
    data: *const u8,
    len: u32,
    disposition: u32,
    out: *mut *const CrlContext,
) -> i32 {
    let c = CertCreateCRLContext(encoding_type, data, len);
    if c.is_null() {
        return 0;
    }
    let r = add_crl(h, c, disposition, out);
    CertFreeCRLContext(c);
    r
}

/// The CRL after `prev` in the store and (a collection) its members, as
/// `next_after` walks certificates.
fn next_crl_after(
    h: *mut c_void,
    prev: *const CrlContext,
    mut keep: impl FnMut(*const CrlContext) -> bool,
) -> *const CrlContext {
    let Some(s) = store(h) else {
        if !prev.is_null() {
            CertFreeCRLContext(prev);
        }
        return error(E_INVALIDARG, core::ptr::null());
    };
    let mut members = [core::ptr::null_mut(); MAX_WALKED];
    let mut count = 0;
    walk_members(s, &mut members, &mut count, 0);
    let mut passed = prev.is_null();
    let mut found = None;
    for target in core::iter::once(h).chain(members[..count].iter().copied()) {
        if found.is_some() {
            break;
        }
        let Some(t) = store(target) else { continue };
        found = t.with(|t| {
            for &c in t.crls.items() {
                if !passed {
                    passed = c == prev;
                    continue;
                }
                if keep(c) {
                    return Some(CertDuplicateCRLContext(c));
                }
            }
            None
        });
    }
    for &m in &members[..count] {
        CertCloseStore(m, 0);
    }
    if !prev.is_null() {
        CertFreeCRLContext(prev);
    }
    found.unwrap_or_else(|| error(CRYPT_E_NOT_FOUND, core::ptr::null()))
}

/// CertEnumCRLsInStore: the next CRL (one more reference), freeing
/// `prev`; NULL with CRYPT_E_NOT_FOUND at the end.
#[unsafe(no_mangle)]
pub extern "system" fn CertEnumCRLsInStore(
    h: *mut c_void,
    prev: *const CrlContext,
) -> *const CrlContext {
    next_crl_after(h, prev, |_| true)
}

const CRL_FIND_ANY: u32 = 0;
const CRL_FIND_ISSUED_BY: u32 = 1;
const CRL_FIND_EXISTING: u32 = 2;
const CRL_FIND_ISSUED_BY_AKI_FLAG: u32 = 0x1;
const CRL_FIND_ISSUED_BY_SIGNATURE_FLAG: u32 = 0x2;

/// CertFindCRLInStore(store, encoding, flags, type, para, prev): the next
/// CRL that matches: CRL_FIND_ANY, CRL_FIND_ISSUED_BY (para: the issuer's
/// certificate context; its subject is the CRL's issuer, and with
/// CRL_FIND_ISSUED_BY_SIGNATURE_FLAG its key signed the CRL),
/// CRL_FIND_EXISTING (para: a CRL of the same issuer). Others are
/// recorded (CRYPT_E_NOT_FOUND).
#[unsafe(no_mangle)]
pub extern "system" fn CertFindCRLInStore(
    h: *mut c_void,
    _encoding: u32,
    flags: u32,
    kind: u32,
    para: *const c_void,
    prev: *const CrlContext,
) -> *const CrlContext {
    match kind {
        CRL_FIND_ANY => next_crl_after(h, prev, |_| true),
        CRL_FIND_ISSUED_BY => {
            let issuer = para as *const CertContext;
            if issuer.is_null() {
                return next_crl_after(h, prev, |_| true);
            }
            if flags & CRL_FIND_ISSUED_BY_AKI_FLAG != 0 {
                unsupported(b"crypt32.dll!CertFindCRLInStore(AKI flag: by name)", flags.into());
            }
            // SAFETY: the caller's certificate context.
            let subject = unsafe {
                let info = &*(*issuer).info;
                core::slice::from_raw_parts(info.subject.data, info.subject.size as usize)
            };
            next_crl_after(h, prev, |c| {
                crl_issuer(c) == subject
                    && (flags & CRL_FIND_ISSUED_BY_SIGNATURE_FLAG == 0
                        || crate::cert::CryptVerifyCertificateSignatureEx(
                            0,
                            1,
                            3,
                            c.cast(),
                            2,
                            issuer.cast(),
                            0,
                            core::ptr::null(),
                        ) != 0)
            })
        }
        CRL_FIND_EXISTING => {
            let other = para as *const CrlContext;
            if !crate::crl::is_crl(other) {
                return error(E_INVALIDARG, core::ptr::null());
            }
            let issuer = crl_issuer(other);
            next_crl_after(h, prev, |c| crl_issuer(c) == issuer)
        }
        _ => {
            unsupported(b"crypt32.dll!CertFindCRLInStore(type)", kind.into());
            if !prev.is_null() {
                CertFreeCRLContext(prev);
            }
            error(CRYPT_E_NOT_FOUND, core::ptr::null())
        }
    }
}

/// CertDeleteCRLFromStore: the CRL removed from its store; the context is
/// freed in any case.
#[unsafe(no_mangle)]
pub extern "system" fn CertDeleteCRLFromStore(crl: *const CrlContext) -> i32 {
    if crl.is_null() {
        return error(E_INVALIDARG, 0);
    }
    // SAFETY: a context of ours.
    let h = unsafe { (*crl).store };
    if let Some(s) = store(h) {
        let removed =
            s.with(|s| s.crls.items().iter().position(|&c| c == crl).map(|i| s.crls.remove(i)));
        if let Some(c) = removed {
            crate::crl::disown(c);
            CertFreeCRLContext(c);
        }
    }
    CertFreeCRLContext(crl);
    1
}

const CERT_STORE_CTRL_RESYNC: u32 = 1;
const CERT_STORE_CTRL_NOTIFY_CHANGE: u32 = 2;
const CERT_STORE_CTRL_COMMIT: u32 = 3;
const CERT_STORE_CTRL_AUTO_RESYNC: u32 = 4;
const CERT_STORE_CTRL_CANCEL_NOTIFY: u32 = 5;

/// CertControlStore(store, flags, ctrl, para): gala's stores are in memory
/// and nothing outside the process changes them, so a resync, commit or
/// automatic resync has nothing to do (TRUE), and a change notification
/// is registered but never signaled (TRUE). Other controls: E_INVALIDARG.
#[unsafe(no_mangle)]
pub extern "system" fn CertControlStore(
    h: *mut c_void,
    _flags: u32,
    ctrl: u32,
    _para: *const c_void,
) -> i32 {
    if store(h).is_none() {
        return error(E_INVALIDARG, 0);
    }
    match ctrl {
        CERT_STORE_CTRL_RESYNC
        | CERT_STORE_CTRL_COMMIT
        | CERT_STORE_CTRL_AUTO_RESYNC
        | CERT_STORE_CTRL_CANCEL_NOTIFY => 1,
        CERT_STORE_CTRL_NOTIFY_CHANGE => {
            unsupported(b"crypt32.dll!CertControlStore(NOTIFY_CHANGE: never signaled)", 0);
            1
        }
        _ => error(E_INVALIDARG, 0),
    }
}

/// CertFindChainInStore: the chains, in the store, of certificates that
/// have a private key matching the criteria. gala's stores keep no
/// private keys, so there are none: NULL and CRYPT_E_NOT_FOUND (a
/// previous chain passed in is freed, as documented).
#[unsafe(no_mangle)]
pub extern "system" fn CertFindChainInStore(
    h: *mut c_void,
    _encoding: u32,
    _flags: u32,
    _kind: u32,
    _para: *const c_void,
    prev: *const u8,
) -> *const u8 {
    if !prev.is_null() {
        crate::chain::CertFreeCertificateChain(prev.cast());
    }
    if store(h).is_none() {
        return error(E_INVALIDARG, core::ptr::null());
    }
    error(CRYPT_E_NOT_FOUND, core::ptr::null())
}

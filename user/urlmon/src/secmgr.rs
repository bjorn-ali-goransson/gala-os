//! CoInternetCreateSecurityManager: the default URL security manager
//! (IInternetSecurityManager), with Windows' default zone settings and no
//! zone mappings (a machine whose Internet Settings nobody changed).
//!
//! - MapUrlToZone: a local path or file: URL with a drive, and res:, are
//!   the Local Machine zone (0); http:, https:, ftp: and file: URLs with a
//!   host are the Internet zone (3) when the host has a dot, the Local
//!   Intranet zone (1) when it has none (the default "include all local
//!   sites not listed in other zones"); any other scheme (about:, zip:...)
//!   is the Internet zone; http: without "//" is URLZONE_INVALID with
//!   S_OK; a URL without a scheme (a relative one) is E_INVALIDARG with
//!   URLZONE_INVALID. These are the answers Wine's urlmon tests
//!   (sec_mgr.c test_SecurityManager) check on Windows.
//! - GetSecurityId: `<scheme>:<host>` (or the scheme's whole remainder
//!   for a scheme without a host, "file:" for the Local Machine zone),
//!   then the zone as a DWORD; the same tests' bytes.
//! - The security site is kept (SetSecuritySite/GetSecuritySite).
//! - ProcessUrlAction, QueryCustomPolicy, SetZoneMapping and
//!   GetZoneMappings are E_NOTIMPL, recorded ("unsupported",
//!   `urlmon.dll!<method>(<url or zone>)`), so a run shows what was
//!   asked; ProcessUrlAction without a URL is E_INVALIDARG, as Windows'.
//!
//! Every zone a caller asks for is recorded too ("other",
//! `urlmon.dll!MapUrlToZone(<url>)=<zone>`), as much as the ledger's 64
//! characters keep.

use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

#[cfg_attr(
    target_arch = "x86",
    link(name = "ntdll", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "ntdll", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GalaRecordMissing(name: *const u8, length: usize, what: u32, detail: u64) -> u32;
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GetProcessHeap() -> usize;
    fn HeapAlloc(heap: usize, flags: u32, size: usize) -> *mut u8;
    fn HeapFree(heap: usize, flags: u32, p: *mut u8) -> i32;
}

type HResult = i32;
type Guid = [u8; 16];

const S_OK: HResult = 0;
const E_NOTIMPL: HResult = 0x8000_4001u32 as i32;
const E_NOINTERFACE: HResult = 0x8000_4002u32 as i32;
const E_POINTER: HResult = 0x8000_4003u32 as i32;
const E_INVALIDARG: HResult = 0x8007_0057u32 as i32;
const E_OUTOFMEMORY: HResult = 0x8007_000eu32 as i32;
const E_NOT_SUFFICIENT_BUFFER: HResult = 0x8007_007au32 as i32;

/// The ledger's kinds: "unsupported" (answered E_NOTIMPL; the process
/// goes on: "not implemented" is a stub's, which ends it), "other".
const LEDGER_UNSUPPORTED: u32 = 2;
const LEDGER_OTHER: u32 = 5;

/// URLZONE_LOCAL_MACHINE, URLZONE_INTRANET, URLZONE_INTERNET,
/// URLZONE_INVALID.
const LOCAL_MACHINE: u32 = 0;
const INTRANET: u32 = 1;
const INTERNET: u32 = 3;
const INVALID: u32 = u32::MAX;

/// A GUID in its binary layout from its text form's fields.
const fn guid(d1: u32, d2: u16, d3: u16, d4: [u8; 8]) -> Guid {
    let a = d1.to_le_bytes();
    let b = d2.to_le_bytes();
    let c = d3.to_le_bytes();
    [
        a[0], a[1], a[2], a[3], b[0], b[1], c[0], c[1], d4[0], d4[1], d4[2], d4[3], d4[4], d4[5],
        d4[6], d4[7],
    ]
}

const IID_IUNKNOWN: Guid = guid(0, 0, 0, [0xc0, 0, 0, 0, 0, 0, 0, 0x46]);
/// {79EAC9EE-BAF9-11CE-8C82-00AA004BA90B}
const IID_IINTERNETSECURITYMANAGER: Guid =
    guid(0x79eac9ee, 0xbaf9, 0x11ce, [0x8c, 0x82, 0x00, 0xaa, 0x00, 0x4b, 0xa9, 0x0b]);

/// Reports `parts` to the ledger as `what` (printable ASCII).
fn record(parts: &[&[u8]], what: u32, detail: u64) {
    let mut line = [0u8; 200];
    let mut n = 0;
    for &b in parts.iter().flat_map(|p| p.iter()) {
        if n == line.len() {
            break;
        }
        line[n] = if (0x20..0x7f).contains(&b) && b != b'"' && b != b'\\' { b } else { b'?' };
        n += 1;
    }
    // SAFETY: a valid buffer of n bytes.
    unsafe { GalaRecordMissing(line.as_ptr(), n, what, detail) };
}

/// The longest URL read (INTERNET_MAX_URL_LENGTH is 2084).
const MAX_URL: usize = 2084;

/// The caller's NUL-terminated URL as bytes (UTF-16 units narrowed:
/// others '?'), at most MAX_URL.
fn narrow(url: *const u16, out: &mut [u8; MAX_URL]) -> usize {
    let mut n = 0;
    while n < out.len() {
        // SAFETY: the caller's NUL-terminated string, read up to its NUL.
        let c = unsafe { *url.add(n) };
        if c == 0 {
            break;
        }
        out[n] = if c < 0x80 { c as u8 } else { b'?' };
        n += 1;
    }
    n
}

/// What a URL maps to: its zone and its security ID's scheme and text
/// (`scheme:text`, the zone's DWORD after it), or None for a URL with no
/// security ID.
struct Mapping<'a> {
    zone: u32,
    id: Option<(&'a [u8], &'a [u8])>,
}

/// A host's zone with no mappings: a dotted name is on the Internet, a
/// plain one on the intranet.
fn host_zone(host: &[u8]) -> u32 {
    if host.contains(&b'.') { INTERNET } else { INTRANET }
}

/// A drive path: `X:` then the rest.
fn is_drive(s: &[u8]) -> bool {
    s.len() >= 2 && s[0].is_ascii_alphabetic() && s[1] == b':'
}

const LOCAL: Mapping<'static> = Mapping { zone: LOCAL_MACHINE, id: Some((b"file", b"")) };

/// The zone and security ID of `url` (E_INVALIDARG: no scheme).
fn map(url: &[u8]) -> Result<Mapping<'_>, HResult> {
    // A path: a drive, or a UNC path's host.
    if is_drive(url) {
        return Ok(LOCAL);
    }
    if let Some(rest) = url.strip_prefix(b"\\\\") {
        let host = rest.split(|&b| b == b'\\' || b == b'/').next().unwrap_or(&[]);
        return Ok(Mapping { zone: host_zone(host), id: Some((b"file", host)) });
    }
    // The scheme: a letter, then letters, digits, '+', '-', '.'.
    let colon = url.iter().position(|&b| b == b':').ok_or(E_INVALIDARG)?;
    let scheme = &url[..colon];
    if scheme.is_empty()
        || !scheme[0].is_ascii_alphabetic()
        || !scheme.iter().all(|&b| b.is_ascii_alphanumeric() || b"+-.".contains(&b))
    {
        return Err(E_INVALIDARG);
    }
    let rest = &url[colon + 1..];
    let is = |name: &[u8]| scheme.eq_ignore_ascii_case(name);
    if is(b"res") {
        return Ok(LOCAL);
    }
    if is(b"file") {
        let path = rest.strip_prefix(b"//").unwrap_or(rest);
        let path = path.strip_prefix(b"/").unwrap_or(path);
        if is_drive(path) {
            return Ok(LOCAL);
        }
        if let Some(after) = rest.strip_prefix(b"//") {
            let host = after.split(|&b| b == b'/' || b == b'\\').next().unwrap_or(&[]);
            return Ok(Mapping { zone: host_zone(host), id: Some((b"file", host)) });
        }
        return Ok(LOCAL);
    }
    if is(b"http") || is(b"https") || is(b"ftp") {
        let Some(after) = rest.strip_prefix(b"//") else {
            // http:google.com: no host, no zone.
            return Ok(Mapping { zone: INVALID, id: None });
        };
        let authority = after.split(|&b| b"/?#\\".contains(&b)).next().unwrap_or(&[]);
        let host = authority.rsplit(|&b| b == b'@').next().unwrap_or(&[]);
        let host = host.split(|&b| b == b':').next().unwrap_or(&[]);
        if host.is_empty() {
            return Ok(Mapping { zone: INVALID, id: None });
        }
        return Ok(Mapping { zone: host_zone(host), id: Some((scheme, host)) });
    }
    // Other schemes (about:, zip:, mailto:...): the Internet zone, the
    // whole URL as their ID.
    Ok(Mapping { zone: INTERNET, id: Some((scheme, rest)) })
}

// ---- The object ----

#[repr(C)]
struct Manager {
    vtable: *const (),
    refs: AtomicU32,
    /// The IInternetSecurityMgrSite (one reference held), or 0.
    site: AtomicUsize,
}

/// A vtable (function addresses).
struct VTable<const N: usize>([*const (); N]);
// SAFETY: function addresses only.
unsafe impl<const N: usize> Sync for VTable<N> {}

fn manager<'a>(this: usize) -> &'a Manager {
    // SAFETY: `this` is a Manager this module made.
    unsafe { &*(this as *const Manager) }
}

fn put<T>(out: *mut T, value: T) {
    // SAFETY: the caller's output pointer (checked non-null by callers).
    unsafe { out.write(value) };
}

/// IUnknown::AddRef or ::Release of a COM object of the caller's.
fn unknown_call(object: usize, slot: usize) {
    // SAFETY: a COM object: its vtable's IUnknown slots.
    unsafe {
        let vtable = *(object as *const *const usize);
        let f: extern "system" fn(usize) -> u32 = core::mem::transmute(*vtable.add(slot));
        f(object);
    }
}

extern "system" fn query(this: usize, iid: *const Guid, out: *mut usize) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, 0);
    if iid.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's IID.
    let iid = unsafe { *iid };
    if iid == IID_IUNKNOWN || iid == IID_IINTERNETSECURITYMANAGER {
        add_ref(this);
        put(out, this);
        return S_OK;
    }
    E_NOINTERFACE
}

extern "system" fn add_ref(this: usize) -> u32 {
    manager(this).refs.fetch_add(1, Ordering::Relaxed) + 1
}

extern "system" fn release(this: usize) -> u32 {
    let m = manager(this);
    let left = m.refs.fetch_sub(1, Ordering::AcqRel) - 1;
    if left == 0 {
        let site = m.site.swap(0, Ordering::AcqRel);
        if site != 0 {
            unknown_call(site, 2);
        }
        // SAFETY: the block `create` allocated, no longer referenced.
        unsafe { HeapFree(GetProcessHeap(), 0, this as *mut u8) };
    }
    left
}

extern "system" fn set_site(this: usize, site: usize) -> HResult {
    if site != 0 {
        unknown_call(site, 1);
    }
    let old = manager(this).site.swap(site, Ordering::AcqRel);
    if old != 0 {
        unknown_call(old, 2);
    }
    S_OK
}

extern "system" fn get_site(this: usize, out: *mut usize) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    let site = manager(this).site.load(Ordering::Acquire);
    if site != 0 {
        unknown_call(site, 1);
    }
    put(out, site);
    S_OK
}

extern "system" fn map_url_to_zone(
    _this: usize,
    url: *const u16,
    zone: *mut u32,
    _flags: u32,
) -> HResult {
    if zone.is_null() {
        return E_INVALIDARG;
    }
    if url.is_null() {
        return E_INVALIDARG;
    }
    let mut text = [0u8; MAX_URL];
    let n = narrow(url, &mut text);
    let (result, z) = match map(&text[..n]) {
        Ok(m) => (S_OK, m.zone),
        Err(e) => (e, INVALID),
    };
    put(zone, z);
    record(&[b"urlmon.dll!MapUrlToZone(", &text[..n.min(40)], b")"], LEDGER_OTHER, z.into());
    result
}

extern "system" fn get_security_id(
    _this: usize,
    url: *const u16,
    id: *mut u8,
    size: *mut u32,
    _reserved: usize,
) -> HResult {
    if url.is_null() || id.is_null() || size.is_null() {
        return E_INVALIDARG;
    }
    let mut text = [0u8; MAX_URL];
    let n = narrow(url, &mut text);
    let m = match map(&text[..n]) {
        Ok(m) => m,
        Err(e) => return e,
    };
    let Some((scheme, rest)) = m.id else { return E_INVALIDARG };
    // `scheme:` (lower case) `rest`, then the zone.
    let need = scheme.len() + 1 + rest.len() + 4;
    // SAFETY: the caller's DWORD.
    let room = unsafe { size.read() } as usize;
    put(size, need as u32);
    if room < need {
        return E_NOT_SUFFICIENT_BUFFER;
    }
    // SAFETY: the caller's buffer of `room` bytes (at least `need`).
    let out = unsafe { core::slice::from_raw_parts_mut(id, need) };
    let (head, tail) = out.split_at_mut(scheme.len());
    for (o, &b) in head.iter_mut().zip(scheme) {
        *o = b.to_ascii_lowercase();
    }
    tail[0] = b':';
    tail[1..1 + rest.len()].copy_from_slice(rest);
    tail[1 + rest.len()..].copy_from_slice(&m.zone.to_le_bytes());
    S_OK
}

/// What a method not implemented was asked, to the ledger.
fn not_implemented(method: &[u8], url: *const u16, detail: u64) -> HResult {
    let mut text = [0u8; MAX_URL];
    let n = if url.is_null() { 0 } else { narrow(url, &mut text) };
    record(&[b"urlmon.dll!", method, b"(", &text[..n.min(32)], b")"], LEDGER_UNSUPPORTED, detail);
    E_NOTIMPL
}

extern "system" fn process_url_action(
    _this: usize,
    url: *const u16,
    action: u32,
    _policy: *mut u8,
    _policy_size: u32,
    _context: *mut u8,
    _context_size: u32,
    _flags: u32,
    _reserved: u32,
) -> HResult {
    if url.is_null() {
        return E_INVALIDARG;
    }
    not_implemented(b"ProcessUrlAction", url, action.into())
}

extern "system" fn query_custom_policy(
    _this: usize,
    url: *const u16,
    _key: *const Guid,
    _policy: *mut *mut u8,
    _policy_size: *mut u32,
    _context: *mut u8,
    _context_size: u32,
    _reserved: u32,
) -> HResult {
    not_implemented(b"QueryCustomPolicy", url, 0)
}

extern "system" fn set_zone_mapping(
    _this: usize,
    zone: u32,
    pattern: *const u16,
    _flags: u32,
) -> HResult {
    not_implemented(b"SetZoneMapping", pattern, zone.into())
}

extern "system" fn get_zone_mappings(
    _this: usize,
    zone: u32,
    _enum: *mut *mut c_void,
    _flags: u32,
) -> HResult {
    not_implemented(b"GetZoneMappings", core::ptr::null(), zone.into())
}

static VTABLE: VTable<11> = VTable([
    query as *const (),
    add_ref as *const (),
    release as *const (),
    set_site as *const (),
    get_site as *const (),
    map_url_to_zone as *const (),
    get_security_id as *const (),
    process_url_action as *const (),
    query_custom_policy as *const (),
    set_zone_mapping as *const (),
    get_zone_mappings as *const (),
]);

/// CoInternetCreateSecurityManager(pSP, ppSM, dwReserved): a new security
/// manager (the service provider is not consulted: no custom manager is
/// asked for).
#[unsafe(no_mangle)]
pub extern "system" fn CoInternetCreateSecurityManager(
    _provider: *mut c_void,
    out: *mut usize,
    _reserved: u32,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: plain allocation of the object, initialized below.
    let p = unsafe { HeapAlloc(GetProcessHeap(), 0, size_of::<Manager>()) }.cast::<Manager>();
    if p.is_null() {
        put(out, 0);
        return E_OUTOFMEMORY;
    }
    // SAFETY: a fresh block of the object's size (heap blocks are aligned
    // for it).
    unsafe {
        p.write(Manager {
            vtable: VTABLE.0.as_ptr().cast(),
            refs: AtomicU32::new(1),
            site: AtomicUsize::new(0),
        })
    };
    put(out, p as usize);
    S_OK
}

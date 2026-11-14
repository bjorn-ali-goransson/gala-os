//! gala's sppc.dll: the Software Protection Platform's client API
//! (slpublic.h's SL* functions), which Office's Click-to-Run integrator
//! uses (through its delay loads of `osppc.dll`, the name Office's own
//! platform had before Windows 8) to install Office's licenses
//! (ROADMAP OF). Windows' sppc.dll asks the Software Protection service
//! (sppsvc), which keeps its store in `System32\spp\store`; gala has no
//! such service: this DLL keeps the store itself, in files, so every
//! process sees what another installed (ARCHITECTURE "sppc").
//!
//! The store, under `%SystemRoot%\System32\spp\gala`:
//! - `licenses\<license file id>.xrm-ms`: each license file (an XrML
//!   document: an `r:license` or an `rg:licenseGroup` of them) as it was
//!   installed. A license's identifier is its `licenseId`; its SKU and
//!   application are its `tm:infoStr` `productSkuId` and
//!   `applicationId` (a license without an application, such as an
//!   EUL's private half, takes that of another license of its SKU).
//! - no product keys: SLInstallProofOfPurchase refuses every key, since
//!   gala does not decode product keys against a license's key
//!   configuration (recorded; ROADMAP OF).
//!
//! Limits: XrML signatures are not verified (a license made by anyone is
//! taken, where Windows refuses it); a license file's identifier is a
//! hash of its bytes (the same file installed again has the same one;
//! what Windows derives it from is not measured); no right is ever
//! consumed, so no SKU is licensed: SLConsumeRight is
//! SL_E_RIGHT_NOT_GRANTED, the status of every SKU unlicensed (reason
//! SL_E_PKEY_NOT_INSTALLED) and no policy has a value.
//!
//! Observability: every call is recorded in the ledger as
//! `sppc.dll!<function>(<case>)`, "other", with the HRESULT returned as
//! the detail, so a run shows what a program asked and was answered; a
//! file in the store that cannot be read as a license is recorded as
//! `sppc.dll!store: unreadable license file`.

#![no_std]
#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

use gala_heap as _;

#[cfg(target_arch = "x86")]
use ntrt as _;

// The x64 build's memory routines come from gala's ntdll (as other DLLs
// on `alloc` take them).
#[cfg(target_arch = "x86_64")]
#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memmove(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
    fn strlen(s: *const u8) -> usize;
    fn __CxxFrameHandler3();
    fn __chkstk();
}

/// MSVC-target codegen references this marker symbol directly, so it cannot
/// come from an import.
#[cfg(target_arch = "x86_64")]
#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

#[cfg_attr(
    target_arch = "x86",
    link(name = "ntdll", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "ntdll", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GalaRecordMissing(name: *const u8, length: usize, what: u32, detail: u64) -> u32;
}

#[repr(C)]
struct FindData {
    attributes: u32,
    times: [u32; 6],
    size_high: u32,
    size_low: u32,
    reserved: [u32; 2],
    name: [u16; 260],
    alternate: [u16; 14],
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GetSystemDirectoryW(buffer: *mut u16, size: u32) -> u32;
    fn CreateDirectoryW(path: *const u16, security: usize) -> i32;
    fn CreateFileW(
        path: *const u16,
        access: u32,
        share: u32,
        security: usize,
        disposition: u32,
        flags: u32,
        template: usize,
    ) -> usize;
    fn ReadFile(h: usize, buf: *mut u8, len: u32, read: *mut u32, overlapped: usize) -> i32;
    fn WriteFile(h: usize, buf: *const u8, len: u32, written: *mut u32, overlapped: usize) -> i32;
    fn GetFileSizeEx(h: usize, size: *mut i64) -> i32;
    fn CloseHandle(h: usize) -> i32;
    fn DeleteFileW(path: *const u16) -> i32;
    fn FindFirstFileW(pattern: *const u16, data: *mut FindData) -> usize;
    fn FindNextFileW(h: usize, data: *mut FindData) -> i32;
    fn FindClose(h: usize) -> i32;
    fn GetLastError() -> u32;
    fn LocalAlloc(flags: u32, size: usize) -> *mut u8;
    fn GetProcessHeap() -> usize;
    fn HeapAlloc(heap: usize, flags: u32, size: usize) -> *mut u8;
    fn HeapFree(heap: usize, flags: u32, p: *mut u8) -> i32;
}

const S_OK: i32 = 0;
const E_INVALIDARG: i32 = 0x8007_0057_u32 as i32;
const E_OUTOFMEMORY: i32 = 0x8007_000e_u32 as i32;
const E_NOTIMPL: i32 = 0x8000_4001_u32 as i32;
/// HRESULT_FROM_WIN32(ERROR_INVALID_DATA): a blob that is not a license.
const E_INVALID_DATA: i32 = 0x8007_000d_u32 as i32;
/// slerror.h and the documented Software Licensing error codes.
const SL_E_LICENSE_FILE_NOT_INSTALLED: i32 = 0xc004_f011_u32 as i32;
const SL_E_VALUE_NOT_FOUND: i32 = 0xc004_f012_u32 as i32;
const SL_E_RIGHT_NOT_GRANTED: i32 = 0xc004_f013_u32 as i32;
const SL_E_PKEY_NOT_INSTALLED: i32 = 0xc004_f014_u32 as i32;
const SL_E_INVALID_PRODUCT_KEY: i32 = 0xc004_f050_u32 as i32;
const SL_E_PRODUCT_SKU_NOT_INSTALLED: i32 = 0xc004_f069_u32 as i32;

/// SLIDTYPE.
const SL_ID_APPLICATION: u32 = 0;
const SL_ID_PRODUCT_SKU: u32 = 1;
const SL_ID_LICENSE_FILE: u32 = 2;
const SL_ID_LICENSE: u32 = 3;
const SL_ID_PKEY: u32 = 4;
const SL_ID_ALL_LICENSES: u32 = 5;
const SL_ID_ALL_LICENSE_FILES: u32 = 6;
/// SL_DATA_SZ (REG_SZ).
const SL_DATA_SZ: u32 = 1;
/// SL_LICENSING_STATUS_UNLICENSED.
const SL_LICENSING_STATUS_UNLICENSED: u32 = 0;

const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const FILE_SHARE_READ: u32 = 1;
const CREATE_ALWAYS: u32 = 2;
const OPEN_EXISTING: u32 = 3;
const INVALID_HANDLE_VALUE: usize = usize::MAX;
const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
const ERROR_FILE_NOT_FOUND: u32 = 2;
const LMEM_FIXED: u32 = 0;
/// The ledger's "other".
const LEDGER_OTHER: u32 = 5;
/// The largest license file taken (Office's are under 100 KiB).
const MAX_LICENSE: usize = 4 << 20;

const REL_R_NS: &str = "urn:mpeg:mpeg21:2003:01-REL-R-NS";
const TM_NS: &str = "http://www.microsoft.com/DRM/XrML2/TM/v2";

/// SLID (a GUID) as its 16 bytes.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub struct Slid([u8; 16]);

impl Slid {
    /// `{xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx}`, braces optional, any case.
    fn parse(s: &str) -> Option<Slid> {
        let s = s.trim();
        let s = s.strip_prefix('{').and_then(|s| s.strip_suffix('}')).unwrap_or(s);
        let b = s.as_bytes();
        if b.len() != 36 || [8, 13, 18, 23].iter().any(|&i| b[i] != b'-') {
            return None;
        }
        let mut hex = [0u8; 32];
        let mut n = 0;
        for &c in b {
            if c == b'-' {
                continue;
            }
            hex[n] = match c {
                b'0'..=b'9' => c - b'0',
                b'a'..=b'f' => c - b'a' + 10,
                b'A'..=b'F' => c - b'A' + 10,
                _ => return None,
            };
            n += 1;
        }
        let byte = |i: usize| hex[2 * i] << 4 | hex[2 * i + 1];
        let mut g = [0u8; 16];
        // Data1, Data2, Data3 little-endian; Data4 as written.
        for (to, from) in [3, 2, 1, 0, 5, 4, 7, 6, 8, 9, 10, 11, 12, 13, 14, 15].iter().enumerate()
        {
            g[to] = byte(*from);
        }
        Some(Slid(g))
    }

    /// `{xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx}`, lower case.
    fn format(&self) -> [u8; 38] {
        let order = [3, 2, 1, 0, 5, 4, 7, 6, 8, 9, 10, 11, 12, 13, 14, 15];
        let mut out = [0u8; 38];
        out[0] = b'{';
        out[37] = b'}';
        let mut n = 1;
        for (i, &at) in order.iter().enumerate() {
            if [4, 6, 8, 10].contains(&i) {
                out[n] = b'-';
                n += 1;
            }
            let v = self.0[at];
            out[n] = b"0123456789abcdef"[usize::from(v >> 4)];
            out[n + 1] = b"0123456789abcdef"[usize::from(v & 15)];
            n += 2;
        }
        out
    }

    /// A license file's identifier: a name-based (version 5 layout)
    /// identifier from two FNV-1a hashes of its bytes.
    fn of_bytes(data: &[u8]) -> Slid {
        let fnv = |basis: u64| {
            data.iter().fold(basis, |h, &b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3))
        };
        let mut g = [0u8; 16];
        g[..8].copy_from_slice(&fnv(0xcbf2_9ce4_8422_2325).to_le_bytes());
        g[8..].copy_from_slice(&fnv(0x6c62_272e_07bb_0142).to_le_bytes());
        g[7] = (g[7] & 0x0f) | 0x50;
        g[8] = (g[8] & 0x3f) | 0x80;
        Slid(g)
    }
}

/// A license in the store.
struct License {
    id: Slid,
    file: Slid,
    sku: Option<Slid>,
    app: Option<Slid>,
    /// Its `tm:infoStr` values (name, value).
    info: Vec<(String, String)>,
}

/// Records `sppc.dll!<function>(<case>)` with the HRESULT as detail and
/// returns the HRESULT.
fn record(function: &str, case: &str, hr: i32) -> i32 {
    let mut text = [0u8; 96];
    let mut n = 0;
    let parts: [&[u8]; 5] = [b"sppc.dll!", function.as_bytes(), b"(", case.as_bytes(), b")"];
    for part in parts {
        let take = part.len().min(text.len() - n);
        text[n..n + take].copy_from_slice(&part[..take]);
        n += take;
    }
    // SAFETY: a valid buffer.
    unsafe { GalaRecordMissing(text.as_ptr(), n, LEDGER_OTHER, u64::from(hr as u32)) };
    hr
}

/// A NUL-terminated UTF-16 string as a String (None for NULL).
fn from_wide(p: *const u16) -> Option<String> {
    if p.is_null() {
        return None;
    }
    let mut s = String::new();
    let mut i = 0;
    loop {
        // SAFETY: the caller's NUL-terminated string.
        let c = unsafe { *p.add(i) };
        if c == 0 || i > 4096 {
            break;
        }
        s.push(char::from_u32(c.into()).unwrap_or('\u{fffd}'));
        i += 1;
    }
    Some(s)
}

/// `%SystemRoot%\System32\spp\gala[\<sub>]` as UTF-16 without the NUL.
fn store_dir(sub: &str) -> Vec<u16> {
    let mut buf = [0u16; 260];
    // SAFETY: our buffer.
    let n = unsafe { GetSystemDirectoryW(buf.as_mut_ptr(), 260) } as usize;
    let mut p: Vec<u16> = buf[..n.min(260)].to_vec();
    p.extend("\\spp\\gala".encode_utf16());
    if !sub.is_empty() {
        p.push(u16::from(b'\\'));
        p.extend(sub.encode_utf16());
    }
    p
}

fn nul(mut p: Vec<u16>) -> Vec<u16> {
    p.push(0);
    p
}

/// The path of license file `id` in the store, NUL-terminated.
fn license_path(id: &Slid) -> Vec<u16> {
    let mut p = store_dir("licenses");
    p.push(u16::from(b'\\'));
    p.extend(id.format().iter().map(|&b| u16::from(b)));
    p.extend(".xrm-ms".encode_utf16());
    nul(p)
}

/// Creates the store's directories (those that exist stay).
fn make_store() {
    for sub in ["", "licenses"] {
        let mut p = store_dir("");
        p.truncate(p.len() - "\\gala".len());
        let spp = nul(p);
        // SAFETY: NUL-terminated paths.
        unsafe {
            CreateDirectoryW(spp.as_ptr(), 0);
            CreateDirectoryW(nul(store_dir(sub)).as_ptr(), 0);
        }
    }
}

/// A whole file (None when it cannot be read or is larger than
/// MAX_LICENSE).
fn read_file(path: &[u16]) -> Option<Vec<u8>> {
    // SAFETY: a NUL-terminated path.
    let h = unsafe {
        CreateFileW(path.as_ptr(), GENERIC_READ, FILE_SHARE_READ, 0, OPEN_EXISTING, 0, 0)
    };
    if h == INVALID_HANDLE_VALUE {
        return None;
    }
    let mut size = 0i64;
    // SAFETY: our handle and output.
    let ok = unsafe { GetFileSizeEx(h, &mut size) } != 0;
    let mut data = Vec::new();
    let good = ok
        && (0..=MAX_LICENSE as i64).contains(&size)
        && data.try_reserve_exact(size as usize).is_ok()
        && {
            data.resize(size as usize, 0);
            let mut read = 0u32;
            // SAFETY: our handle and buffer of `size` bytes.
            let r = unsafe { ReadFile(h, data.as_mut_ptr(), size as u32, &mut read, 0) };
            r != 0 && read as i64 == size
        };
    // SAFETY: our handle.
    unsafe { CloseHandle(h) };
    good.then_some(data)
}

/// Writes a whole file; false when it could not be.
fn write_file(path: &[u16], data: &[u8]) -> bool {
    // SAFETY: a NUL-terminated path.
    let h = unsafe { CreateFileW(path.as_ptr(), GENERIC_WRITE, 0, 0, CREATE_ALWAYS, 0, 0) };
    if h == INVALID_HANDLE_VALUE {
        return false;
    }
    let mut written = 0u32;
    // SAFETY: our handle and data.
    let ok = unsafe { WriteFile(h, data.as_ptr(), data.len() as u32, &mut written, 0) } != 0
        && written as usize == data.len();
    // SAFETY: our handle.
    unsafe { CloseHandle(h) };
    ok
}

/// The licenses in an XrML document (empty when it is not one): the
/// root `r:license`, or those of an `rg:licenseGroup`.
fn licenses_of(data: &[u8], file: Slid) -> Vec<License> {
    let Ok(root) = gala_xml::parse(data) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut take = |l: &gala_xml::Element| {
        let Some(id) = l.attr("licenseId").and_then(Slid::parse) else {
            return;
        };
        let mut info = Vec::new();
        collect_info(l, &mut info);
        let get =
            |name: &str| info.iter().find(|(n, _)| n == name).and_then(|(_, v)| Slid::parse(v));
        let (sku, app) = (get("productSkuId"), get("applicationId"));
        out.push(License { id, file, sku, app, info });
    };
    if root.is(REL_R_NS, "license") {
        take(&root);
    } else if root.is(REL_R_NS, "licenseGroup") {
        for l in root.children_named(REL_R_NS, "license") {
            take(l);
        }
    }
    out
}

/// The `tm:infoStr` values under `e` (not those of a nested license).
fn collect_info(e: &gala_xml::Element, info: &mut Vec<(String, String)>) {
    for c in &e.children {
        if c.is(TM_NS, "infoStr") {
            if let Some(name) = c.attr("name") {
                info.push((String::from(name), String::from(c.text.trim())));
            }
        } else if !c.is(REL_R_NS, "license") {
            collect_info(c, info);
        }
    }
}

/// Every license in the store; a license without an application takes
/// that of another license of its SKU.
fn load() -> Vec<License> {
    let mut all = Vec::new();
    let mut pattern = store_dir("licenses");
    pattern.extend("\\*.xrm-ms".encode_utf16());
    let pattern = nul(pattern);
    // SAFETY: FindData is plain data, all zero valid.
    let mut fd: FindData = unsafe { core::mem::zeroed() };
    // SAFETY: a NUL-terminated pattern and our output.
    let h = unsafe { FindFirstFileW(pattern.as_ptr(), &mut fd) };
    if h == INVALID_HANDLE_VALUE {
        return all;
    }
    loop {
        let len = fd.name.iter().position(|&c| c == 0).unwrap_or(fd.name.len());
        let name = String::from_utf16_lossy(&fd.name[..len]);
        let id = name.strip_suffix(".xrm-ms").and_then(Slid::parse);
        if let (Some(id), false) = (id, fd.attributes & FILE_ATTRIBUTE_DIRECTORY != 0) {
            let licenses = read_file(&license_path(&id)).map(|d| licenses_of(&d, id));
            match licenses {
                Some(l) if !l.is_empty() => all.extend(l),
                _ => {
                    record("store", "unreadable license file", 0);
                }
            }
        }
        // SAFETY: our find handle and output.
        if unsafe { FindNextFileW(h, &mut fd) } == 0 {
            break;
        }
    }
    // SAFETY: our find handle.
    unsafe { FindClose(h) };
    for i in 0..all.len() {
        if all[i].app.is_none() {
            let sku = all[i].sku;
            let app = all.iter().find(|o| sku.is_some() && o.sku == sku && o.app.is_some());
            all[i].app = app.and_then(|o| o.app);
        }
    }
    all
}

/// Copies `items` into a LocalAlloc'd block, or None when out of memory.
fn local_copy<T: Copy>(items: &[T]) -> Option<*mut T> {
    let size = core::mem::size_of_val(items).max(1);
    // SAFETY: a plain allocation.
    let p = unsafe { LocalAlloc(LMEM_FIXED, size) }.cast::<T>();
    if p.is_null() {
        return None;
    }
    // SAFETY: room for items.len() values.
    unsafe { core::ptr::copy_nonoverlapping(items.as_ptr(), p, items.len()) };
    Some(p)
}

/// A string value (SL_DATA_SZ, with its NUL) in the outputs of
/// SLGetLicenseInformation and the like.
fn string_value(value: &str, data_type: *mut u32, size: *mut u32, out: *mut *mut u8) -> i32 {
    let mut w: Vec<u16> = value.encode_utf16().collect();
    w.push(0);
    let Some(p) = local_copy(&w) else {
        return E_OUTOFMEMORY;
    };
    // SAFETY: the caller's outputs (checked non-NULL).
    unsafe {
        if !data_type.is_null() {
            *data_type = SL_DATA_SZ;
        }
        *size = (w.len() * 2) as u32;
        *out = p.cast();
    }
    S_OK
}

/// HSLC: a block of the process heap holding this tag.
const HANDLE_TAG: u64 = 0x6370_7073_616c_6167;

fn valid(h: usize) -> bool {
    // SAFETY: a handle SLOpen gave holds the tag; NULL is refused first.
    h != 0 && h.is_multiple_of(8) && unsafe { *(h as *const u64) } == HANDLE_TAG
}

/// SLOpen(*phSLC): a handle to the store.
#[unsafe(no_mangle)]
pub extern "system" fn SLOpen(handle: *mut usize) -> i32 {
    if handle.is_null() {
        return record("SLOpen", "NULL", E_INVALIDARG);
    }
    // SAFETY: a plain allocation.
    let p = unsafe { HeapAlloc(GetProcessHeap(), 0, 8) }.cast::<u64>();
    if p.is_null() {
        return record("SLOpen", "out of memory", E_OUTOFMEMORY);
    }
    // SAFETY: our block; the caller's output.
    unsafe {
        *p = HANDLE_TAG;
        *handle = p as usize;
    }
    record("SLOpen", "", S_OK)
}

/// SLClose(hSLC).
#[unsafe(no_mangle)]
pub extern "system" fn SLClose(handle: usize) -> i32 {
    if !valid(handle) {
        return record("SLClose", "not a handle", E_INVALIDARG);
    }
    // SAFETY: our block, untagged before it goes.
    unsafe {
        *(handle as *mut u64) = 0;
        HeapFree(GetProcessHeap(), 0, handle as *mut u8);
    }
    record("SLClose", "", S_OK)
}

/// SLInstallLicense(hSLC, cbLicenseBlob, pbLicenseBlob, *pLicenseFileId).
#[unsafe(no_mangle)]
pub extern "system" fn SLInstallLicense(
    handle: usize,
    size: u32,
    blob: *const u8,
    file_id: *mut Slid,
) -> i32 {
    const F: &str = "SLInstallLicense";
    if !valid(handle) || blob.is_null() || size == 0 || file_id.is_null() {
        return record(F, "invalid argument", E_INVALIDARG);
    }
    if size as usize > MAX_LICENSE {
        return record(F, "too large", E_INVALID_DATA);
    }
    // SAFETY: the caller's blob of `size` bytes.
    let data = unsafe { core::slice::from_raw_parts(blob, size as usize) };
    let id = Slid::of_bytes(data);
    if licenses_of(data, id).is_empty() {
        return record(F, "not a license", E_INVALID_DATA);
    }
    make_store();
    if !write_file(&license_path(&id), data) {
        // SAFETY: reads this thread's last error.
        let e = unsafe { GetLastError() };
        return record(F, "store not written", (0x8007_0000 | (e & 0xffff)) as i32);
    }
    // SAFETY: the caller's output.
    unsafe { *file_id = id };
    record(F, "", S_OK)
}

/// SLUninstallLicense(hSLC, pLicenseFileId).
#[unsafe(no_mangle)]
pub extern "system" fn SLUninstallLicense(handle: usize, file_id: *const Slid) -> i32 {
    const F: &str = "SLUninstallLicense";
    if !valid(handle) || file_id.is_null() {
        return record(F, "invalid argument", E_INVALIDARG);
    }
    // SAFETY: the caller's identifier.
    let path = license_path(unsafe { &*file_id });
    // SAFETY: a NUL-terminated path.
    if unsafe { DeleteFileW(path.as_ptr()) } == 0 {
        // SAFETY: reads this thread's last error.
        let e = unsafe { GetLastError() };
        if e == ERROR_FILE_NOT_FOUND || e == 3 {
            return record(F, "not installed", SL_E_LICENSE_FILE_NOT_INSTALLED);
        }
        return record(F, "store not written", (0x8007_0000 | (e & 0xffff)) as i32);
    }
    record(F, "", S_OK)
}

/// Pushes `id` once.
fn push_once(out: &mut Vec<Slid>, id: Slid) {
    if !out.contains(&id) {
        out.push(id);
    }
}

/// The name of a SLIDTYPE for the ledger.
fn type_name(t: u32) -> &'static str {
    match t {
        SL_ID_APPLICATION => "app",
        SL_ID_PRODUCT_SKU => "sku",
        SL_ID_LICENSE_FILE => "file",
        SL_ID_LICENSE => "license",
        SL_ID_PKEY => "pkey",
        SL_ID_ALL_LICENSES => "all licenses",
        SL_ID_ALL_LICENSE_FILES => "all files",
        _ => "?",
    }
}

/// SLGetSLIDList(hSLC, eQueryIdType, pQueryId, eReturnIdType,
/// *pnReturnIds, **ppReturnIds): the identifiers of `eReturnIdType`
/// related to the one queried (the licenses that are of the
/// application, SKU, license file or license queried, or all of them,
/// mapped to the type asked for).
#[unsafe(no_mangle)]
pub extern "system" fn SLGetSLIDList(
    handle: usize,
    query_type: u32,
    query: *const Slid,
    return_type: u32,
    count: *mut u32,
    ids: *mut *mut Slid,
) -> i32 {
    const F: &str = "SLGetSLIDList";
    let mut case = [0u8; 40];
    let mut n = 0;
    for part in [type_name(query_type), " to ", type_name(return_type)] {
        let take = part.len().min(case.len() - n);
        case[n..n + take].copy_from_slice(&part.as_bytes()[..take]);
        n += take;
    }
    let case = core::str::from_utf8(&case[..n]).unwrap_or("?");
    let all = matches!(query_type, SL_ID_ALL_LICENSES | SL_ID_ALL_LICENSE_FILES);
    if !valid(handle) || count.is_null() || ids.is_null() || (!all && query.is_null()) {
        return record(F, case, E_INVALIDARG);
    }
    // SAFETY: the caller's outputs.
    unsafe {
        *count = 0;
        *ids = core::ptr::null_mut();
    }
    let q = if all { Slid::default() } else { unsafe { *query } };
    let licenses = load();
    let selected = licenses.iter().filter(|l| match query_type {
        SL_ID_APPLICATION => l.app == Some(q),
        SL_ID_PRODUCT_SKU => l.sku == Some(q),
        SL_ID_LICENSE_FILE => l.file == q,
        SL_ID_LICENSE => l.id == q,
        _ => all,
    });
    let mut out = Vec::new();
    match (query_type, return_type) {
        (SL_ID_APPLICATION..=SL_ID_LICENSE | SL_ID_ALL_LICENSES | SL_ID_ALL_LICENSE_FILES, _)
            if return_type <= SL_ID_LICENSE =>
        {
            for l in selected {
                let id = match return_type {
                    SL_ID_APPLICATION => l.app,
                    SL_ID_PRODUCT_SKU => l.sku,
                    SL_ID_LICENSE_FILE => Some(l.file),
                    _ => Some(l.id),
                };
                if let Some(id) = id {
                    push_once(&mut out, id);
                }
            }
        }
        // No product key is ever installed.
        (_, SL_ID_PKEY) | (SL_ID_PKEY, _) => {
            return record(F, case, SL_E_PKEY_NOT_INSTALLED);
        }
        _ => return record(F, case, E_INVALIDARG),
    }
    if out.is_empty() {
        return record(F, case, SL_E_VALUE_NOT_FOUND);
    }
    let Some(p) = local_copy(&out) else {
        return record(F, case, E_OUTOFMEMORY);
    };
    // SAFETY: the caller's outputs.
    unsafe {
        *count = out.len() as u32;
        *ids = p;
    }
    record(F, case, S_OK)
}

/// SLGetLicenseInformation(hSLC, pSLLicenseId, pwszValueName,
/// *peDataType, *pcbValue, **ppbValue): `licenseId`, `fileId`, or a
/// value of the license's `tm:infoStr` table (names compared without
/// case), as a string.
#[unsafe(no_mangle)]
pub extern "system" fn SLGetLicenseInformation(
    handle: usize,
    license: *const Slid,
    name: *const u16,
    data_type: *mut u32,
    size: *mut u32,
    value: *mut *mut u8,
) -> i32 {
    const F: &str = "SLGetLicenseInformation";
    let Some(name) = from_wide(name) else {
        return record(F, "invalid argument", E_INVALIDARG);
    };
    if !valid(handle) || license.is_null() || size.is_null() || value.is_null() {
        return record(F, &name, E_INVALIDARG);
    }
    // SAFETY: the caller's identifier.
    let id = unsafe { *license };
    let licenses = load();
    let Some(l) = licenses.iter().find(|l| l.id == id) else {
        return record(F, "license not installed", SL_E_VALUE_NOT_FOUND);
    };
    let text;
    let found = if name.eq_ignore_ascii_case("licenseId") {
        text = l.id.format();
        core::str::from_utf8(&text).ok()
    } else if name.eq_ignore_ascii_case("fileId") {
        text = l.file.format();
        core::str::from_utf8(&text).ok()
    } else {
        l.info.iter().find(|(n, _)| n.eq_ignore_ascii_case(&name)).map(|(_, v)| v.as_str())
    };
    match found {
        Some(v) => {
            let hr = string_value(v, data_type, size, value);
            record(F, &name, hr)
        }
        None => record(F, &name, SL_E_VALUE_NOT_FOUND),
    }
}

/// SL_LICENSING_STATUS.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct LicensingStatus {
    sku: Slid,
    status: u32,
    grace_time: u32,
    total_grace_days: u32,
    reason: i32,
    validity_expiration: u64,
}

/// The SKUs an application (or one SKU of it) has installed; the error
/// when there are none.
fn skus(app: *const Slid, sku: *const Slid) -> Result<Vec<Slid>, i32> {
    // SAFETY: the caller's identifiers, when not NULL.
    let (app, sku) = unsafe { (app.as_ref().copied(), sku.as_ref().copied()) };
    let mut out = Vec::new();
    for l in load() {
        let (Some(s), Some(a)) = (l.sku, l.app) else {
            continue;
        };
        if app.is_none_or(|x| x == a) && sku.is_none_or(|x| x == s) {
            push_once(&mut out, s);
        }
    }
    if out.is_empty() {
        return Err(SL_E_PRODUCT_SKU_NOT_INSTALLED);
    }
    Ok(out)
}

/// SLGetLicensingStatusInformation(hSLC, pAppID, pProductSkuId,
/// pwszRightName, *pnStatusCount, **ppLicensingStatus): each installed
/// SKU of the application (or the one asked), unlicensed: no product key
/// is installed.
#[unsafe(no_mangle)]
pub extern "system" fn SLGetLicensingStatusInformation(
    handle: usize,
    app: *const Slid,
    sku: *const Slid,
    _right: *const u16,
    count: *mut u32,
    status: *mut *mut LicensingStatus,
) -> i32 {
    const F: &str = "SLGetLicensingStatusInformation";
    if !valid(handle) || (app.is_null() && sku.is_null()) || count.is_null() || status.is_null() {
        return record(F, "invalid argument", E_INVALIDARG);
    }
    let found = match skus(app, sku) {
        Ok(s) => s,
        Err(hr) => return record(F, "no SKU", hr),
    };
    let list: Vec<LicensingStatus> = found
        .iter()
        .map(|&sku| LicensingStatus {
            sku,
            status: SL_LICENSING_STATUS_UNLICENSED,
            grace_time: 0,
            total_grace_days: 0,
            reason: SL_E_PKEY_NOT_INSTALLED,
            validity_expiration: 0,
        })
        .collect();
    let Some(p) = local_copy(&list) else {
        return record(F, "out of memory", E_OUTOFMEMORY);
    };
    // SAFETY: the caller's outputs.
    unsafe {
        *count = list.len() as u32;
        *status = p;
    }
    record(F, "unlicensed", S_OK)
}

/// SLConsumeRight(hSLC, pAppId, pProductSkuId, pwszRightName,
/// pvReserved): no SKU is licensed (no product key), so no right is
/// granted.
#[unsafe(no_mangle)]
pub extern "system" fn SLConsumeRight(
    handle: usize,
    app: *const Slid,
    sku: *const Slid,
    _right: *const u16,
    _reserved: usize,
) -> i32 {
    const F: &str = "SLConsumeRight";
    if !valid(handle) || app.is_null() {
        return record(F, "invalid argument", E_INVALIDARG);
    }
    match skus(app, sku) {
        Ok(_) => record(F, "no product key", SL_E_RIGHT_NOT_GRANTED),
        Err(hr) => record(F, "no SKU", hr),
    }
}

/// SLGetPolicyInformationDWORD(hSLC, pwszValueName, *pdwValue): policies
/// come with a consumed right, and none is.
#[unsafe(no_mangle)]
pub extern "system" fn SLGetPolicyInformationDWORD(
    handle: usize,
    name: *const u16,
    value: *mut u32,
) -> i32 {
    const F: &str = "SLGetPolicyInformationDWORD";
    let Some(name) = from_wide(name) else {
        return record(F, "invalid argument", E_INVALIDARG);
    };
    if !valid(handle) || value.is_null() {
        return record(F, &name, E_INVALIDARG);
    }
    record(F, &name, SL_E_VALUE_NOT_FOUND)
}

/// SLPersistApplicationPolicies(pApplicationId, pProductSkuId, dwFlags):
/// nothing consumed, nothing to keep.
#[unsafe(no_mangle)]
pub extern "system" fn SLPersistApplicationPolicies(
    app: *const Slid,
    _sku: *const Slid,
    _flags: u32,
) -> i32 {
    const F: &str = "SLPersistApplicationPolicies";
    if app.is_null() {
        return record(F, "invalid argument", E_INVALIDARG);
    }
    record(F, "nothing consumed", S_OK)
}

/// SLPersistRTSPayloadOverride: not documented (its arguments unknown).
#[unsafe(no_mangle)]
pub extern "system" fn SLPersistRTSPayloadOverride(_a: usize, _b: usize, _c: usize) -> i32 {
    record("SLPersistRTSPayloadOverride", "not implemented", E_NOTIMPL)
}

/// SLInstallProofOfPurchase(hSLC, pwszPKeyAlgorithm, pwszPKeyString,
/// cbPKeySpecificData, pbPKeySpecificData, *pPkeyId): gala does not
/// decode product keys, so none is configured by an installed license.
#[unsafe(no_mangle)]
pub extern "system" fn SLInstallProofOfPurchase(
    handle: usize,
    algorithm: *const u16,
    key: *const u16,
    _size: u32,
    _data: *const u8,
    pkey: *mut Slid,
) -> i32 {
    const F: &str = "SLInstallProofOfPurchase";
    if !valid(handle) || algorithm.is_null() || key.is_null() || pkey.is_null() {
        return record(F, "invalid argument", E_INVALIDARG);
    }
    let algorithm = from_wide(algorithm).unwrap_or_default();
    let _ = record(F, &algorithm, 0);
    record(F, "product keys not decoded", SL_E_INVALID_PRODUCT_KEY)
}

/// SLUninstallProofOfPurchase(hSLC, pPKeyId): none is installed.
#[unsafe(no_mangle)]
pub extern "system" fn SLUninstallProofOfPurchase(handle: usize, pkey: *const Slid) -> i32 {
    const F: &str = "SLUninstallProofOfPurchase";
    if !valid(handle) || pkey.is_null() {
        return record(F, "invalid argument", E_INVALIDARG);
    }
    record(F, "not installed", SL_E_PKEY_NOT_INSTALLED)
}

/// SLGetInstalledProductKeyIds(hSLC, pProductSkuId, *pnProductKeyIds,
/// **ppProductKeyIds): none is installed.
#[unsafe(no_mangle)]
pub extern "system" fn SLGetInstalledProductKeyIds(
    handle: usize,
    _sku: *const Slid,
    count: *mut u32,
    ids: *mut *mut Slid,
) -> i32 {
    const F: &str = "SLGetInstalledProductKeyIds";
    if !valid(handle) || count.is_null() || ids.is_null() {
        return record(F, "invalid argument", E_INVALIDARG);
    }
    // SAFETY: the caller's outputs.
    unsafe {
        *count = 0;
        *ids = core::ptr::null_mut();
    }
    record(F, "none installed", SL_E_PKEY_NOT_INSTALLED)
}

/// SLGetPKeyInformation(hSLC, pPKeyId, pwszValueName, *peDataType,
/// *pcbValue, **ppbValue): none is installed.
#[unsafe(no_mangle)]
pub extern "system" fn SLGetPKeyInformation(
    handle: usize,
    pkey: *const Slid,
    name: *const u16,
    _data_type: *mut u32,
    size: *mut u32,
    value: *mut *mut u8,
) -> i32 {
    const F: &str = "SLGetPKeyInformation";
    let name = from_wide(name).unwrap_or_default();
    if !valid(handle) || pkey.is_null() || size.is_null() || value.is_null() {
        return record(F, &name, E_INVALIDARG);
    }
    record(F, &name, SL_E_PKEY_NOT_INSTALLED)
}

/// DllMain (the entry point our link flags name).
#[unsafe(no_mangle)]
pub extern "system" fn mainCRTStartup(_instance: usize, _reason: u32, _reserved: usize) -> i32 {
    1
}

/// A panic is a bug (or the heap ran out inside the XML reader): its
/// location goes to the ledger (`sppc.dll!panic <file>`, the line as
/// detail) and the process ends with STATUS_STACK_BUFFER_OVERRUN
/// (fail-fast), as gala's other DLLs do.
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    let mut text = [0u8; 80];
    let mut n = 0;
    let line = info.location().map_or(0, |l| {
        for &b in b"sppc.dll!panic ".iter().chain(l.file().as_bytes()) {
            if n < text.len() {
                text[n] = b;
                n += 1;
            }
        }
        l.line()
    });
    // SAFETY: a valid buffer.
    unsafe { GalaRecordMissing(text.as_ptr(), n, LEDGER_OTHER, line.into()) };
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xc000_0409) }
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn ExitProcess(code: u32) -> !;
}

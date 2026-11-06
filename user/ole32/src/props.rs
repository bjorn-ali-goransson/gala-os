//! What wxWidgets and SQL Server LocalDB import from ole32 beyond the
//! COM basics, as documented: PROPVARIANT and STGMEDIUM clean-up
//! (PropVariantClear, ReleaseStgMedium), the IRunnableObject helpers
//! (OleRun, OleLockRunning, OleSetContainedObject), proxy blankets
//! (CoSetProxyBlanket/CoQueryProxyBlanket: gala has no proxies, so an
//! object without IClientSecurity answers E_NOINTERFACE, as an in-process
//! object does on Windows) and CoFreeUnusedLibraries. See
//! user/ole32_test.
//!
//! oleaut32 (SysFreeString, SafeArrayDestroy) and gdi32 (the GDI media)
//! are found at run time, as Windows' ole32 does not make every program
//! load them.

use super::stream::{GlobalFree, GlobalLock, GlobalUnlock};
use super::{
    E_INVALIDARG, Guid, HResult, LoadLibraryW, S_OK, com_call, free, missing, ole_iid, release,
};
use core::ffi::c_void;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn DeleteFileW(path: *const u16) -> i32;
}

const IID_IRUNNABLEOBJECT: Guid = ole_iid(0x126);
const IID_ICLIENTSECURITY: Guid = ole_iid(0x13d);
const STG_E_INVALIDPARAMETER: HResult = 0x8003_0057u32 as i32;

/// The export `name` (NUL-terminated) of the DLL `dll` (NUL-terminated
/// UTF-16), loading it; 0 if either is missing.
fn export(dll: &[u16], name: &[u8]) -> usize {
    // SAFETY: NUL-terminated names.
    unsafe {
        let module = LoadLibraryW(dll.as_ptr());
        if module.is_null() { 0 } else { super::GetProcAddress(module, name.as_ptr()) as usize }
    }
}

const fn wide<const N: usize>(s: &[u8; N]) -> [u16; N] {
    let mut w = [0u16; N];
    let mut i = 0;
    while i < N {
        w[i] = s[i] as u16;
        i += 1;
    }
    w
}

const OLEAUT32: [u16; 13] = wide(b"oleaut32.dll\0");
const GDI32: [u16; 10] = wide(b"gdi32.dll\0");

/// Calls oleaut32's (or gdi32's) one-pointer-argument function `name`
/// with `p`; recorded if it cannot be found.
fn call(dll: &[u16], name: &[u8], p: usize) {
    let f = export(dll, name);
    if f == 0 {
        missing(b"ole32.dll!PropVariantClear(helper DLL)", 0);
        return;
    }
    // SAFETY: SysFreeString / SafeArrayDestroy / DeleteObject /
    // DeleteMetaFile / DeleteEnhMetaFile all take one handle or pointer.
    let f: extern "system" fn(usize) -> usize = unsafe { core::mem::transmute(f) };
    f(p);
}

// VARTYPEs.
const VT_EMPTY: u16 = 0;
const VT_NULL: u16 = 1;
const VT_BSTR: u16 = 8;
const VT_DISPATCH: u16 = 9;
const VT_VARIANT: u16 = 12;
const VT_UNKNOWN: u16 = 13;
const VT_INT: u16 = 22;
const VT_UINT: u16 = 23;
const VT_LPSTR: u16 = 30;
const VT_LPWSTR: u16 = 31;
const VT_RECORD: u16 = 36;
const VT_FILETIME: u16 = 64;
const VT_BLOB: u16 = 65;
const VT_STREAM: u16 = 66;
const VT_STORAGE: u16 = 67;
const VT_STREAMED_OBJECT: u16 = 68;
const VT_STORED_OBJECT: u16 = 69;
const VT_BLOB_OBJECT: u16 = 70;
const VT_CF: u16 = 71;
const VT_CLSID: u16 = 72;
const VT_VECTOR: u16 = 0x1000;
const VT_ARRAY: u16 = 0x2000;
const VT_BYREF: u16 = 0x4000;

/// PROPVARIANT (x64): vt, three reserved words, 16 bytes of value.
#[repr(C)]
pub struct PropVariant {
    vt: u16,
    reserved: [u16; 3],
    value: [usize; 2],
}

/// The types a PROPVARIANT may hold by value (Wine's test_validtypes
/// table, which its suite checks on Windows).
fn valid_simple(t: u16) -> bool {
    matches!(t, 0..=14 | 16..=21 | VT_INT | VT_UINT | VT_LPSTR | VT_LPWSTR | VT_RECORD | 64..=72)
}

/// The element types of a VT_VECTOR.
fn valid_vector(t: u16) -> bool {
    matches!(t, 2..=8 | 10..=12 | 16..=21 | VT_LPSTR | VT_LPWSTR | VT_FILETIME | VT_CF | VT_CLSID)
}

/// The element types of a VT_ARRAY (a SAFEARRAY) and of VT_BYREF.
fn valid_array(t: u16) -> bool {
    matches!(t, 2..=14 | 16..=23 | VT_RECORD)
}

fn release_ptr(p: usize) {
    if p != 0 {
        release(p as *mut c_void);
    }
}

/// CLIPDATA: a size, the format, the data.
#[repr(C)]
struct ClipData {
    size: u32,
    format: i32,
    data: *mut u8,
}

/// Frees what one by-value element of type `t` at `p` owns.
fn clear_element(t: u16, p: *mut usize) {
    // SAFETY: an element of type `t` (8 bytes for the pointer types; a
    // whole PROPVARIANT for VT_VARIANT).
    let v = unsafe { *p };
    match t {
        VT_BSTR if v != 0 => call(&OLEAUT32, b"SysFreeString\0", v),
        VT_LPSTR | VT_LPWSTR | VT_CLSID => free(v as *mut u8),
        VT_CF if v != 0 => {
            // SAFETY: a CLIPDATA from the task allocator.
            free(unsafe { (*(v as *mut ClipData)).data });
            free(v as *mut u8);
        }
        VT_VARIANT => {
            PropVariantClear(p as *mut PropVariant);
        }
        _ => {}
    }
}

/// The size of one VT_VECTOR element of type `t`.
fn element_size(t: u16) -> usize {
    match t {
        16 | 17 => 1,
        2 | 11 | 18 => 2,
        3 | 4 | 10 | 19 => 4,
        VT_VARIANT => core::mem::size_of::<PropVariant>(),
        VT_CF | VT_BSTR | VT_LPSTR | VT_LPWSTR => 8,
        VT_CLSID => 16,
        _ => 8, // R8, CY, DATE, I8, UI8, FILETIME
    }
}

/// PropVariantClear: frees what the PROPVARIANT owns (strings with the
/// task allocator, BSTRs, interfaces, blobs, clipboard data, vectors and
/// their elements, SAFEARRAYs; by-reference values are not owned) and
/// zeroes it. An invalid type is STG_E_INVALIDPARAMETER, zeroed too.
#[unsafe(no_mangle)]
pub extern "system" fn PropVariantClear(pv: *mut PropVariant) -> HResult {
    if pv.is_null() {
        return S_OK;
    }
    // SAFETY: the caller's PROPVARIANT.
    let v = unsafe { &mut *pv };
    let (vt, base) = (v.vt, v.vt & 0xfff);
    let modifiers = vt & (VT_VECTOR | VT_ARRAY | VT_BYREF);
    let result = match modifiers {
        0 if valid_simple(base) => {
            match base {
                VT_BSTR if v.value[0] != 0 => call(&OLEAUT32, b"SysFreeString\0", v.value[0]),
                VT_DISPATCH | VT_UNKNOWN | VT_STREAM | VT_STORAGE | VT_STREAMED_OBJECT
                | VT_STORED_OBJECT => release_ptr(v.value[0]),
                VT_LPSTR | VT_LPWSTR | VT_CLSID | VT_CF => clear_element(base, &mut v.value[0]),
                // BLOB: a ULONG size, then the data pointer.
                VT_BLOB | VT_BLOB_OBJECT => free(v.value[1] as *mut u8),
                VT_RECORD if v.value[0] != 0 => {
                    missing(b"ole32.dll!PropVariantClear(VT_RECORD)", 0);
                }
                _ => {}
            }
            S_OK
        }
        VT_VECTOR if valid_vector(base) => {
            // CAxxx: a ULONG count, then the element array.
            let (count, elements) = (v.value[0] as u32 as usize, v.value[1] as *mut u8);
            if !elements.is_null()
                && matches!(base, VT_BSTR | VT_LPSTR | VT_LPWSTR | VT_CF | VT_VARIANT)
            {
                for i in 0..count {
                    // SAFETY: the vector's `count` elements.
                    clear_element(base, unsafe { elements.add(i * element_size(base)) }.cast());
                }
            }
            free(elements);
            S_OK
        }
        VT_ARRAY if valid_array(base) => {
            if v.value[0] != 0 {
                call(&OLEAUT32, b"SafeArrayDestroy\0", v.value[0]);
            }
            S_OK
        }
        VT_BYREF if valid_array(base) => S_OK,
        _ if vt == VT_EMPTY || vt == VT_NULL => S_OK,
        _ => STG_E_INVALIDPARAMETER,
    };
    v.vt = VT_EMPTY;
    v.reserved = [0; 3];
    v.value = [0; 2];
    result
}

/// A task-allocator copy of `n` bytes at `p` (NULL stays NULL); None
/// without memory.
fn copy_bytes(p: *const u8, n: usize) -> Option<usize> {
    if p.is_null() {
        return Some(0);
    }
    let q = super::alloc(n.max(1));
    if q.is_null() {
        return None;
    }
    // SAFETY: `n` bytes at `p`; the new block holds them.
    unsafe { core::ptr::copy_nonoverlapping(p, q, n) };
    Some(q as usize)
}

/// The length in bytes (with the terminator) of the string of `unit`-byte
/// characters at `p`.
fn string_bytes(p: *const u8, unit: usize) -> usize {
    let mut n = 0;
    // SAFETY: a NUL-terminated string.
    while unsafe { core::slice::from_raw_parts(p.add(n * unit), unit) }.iter().any(|&b| b != 0) {
        n += 1;
    }
    (n + 1) * unit
}

/// BSTR copy through oleaut32 (SysAllocStringByteLen keeps the length).
fn copy_bstr(b: usize) -> Option<usize> {
    if b == 0 {
        return Some(0);
    }
    let len = export(&OLEAUT32, b"SysStringByteLen\0");
    let make = export(&OLEAUT32, b"SysAllocStringByteLen\0");
    if len == 0 || make == 0 {
        missing(b"ole32.dll!PropVariantCopy(helper DLL)", 0);
        return None;
    }
    // SAFETY: oleaut32's SysStringByteLen(BSTR) and
    // SysAllocStringByteLen(const char*, UINT).
    let len = unsafe { core::mem::transmute::<usize, extern "system" fn(usize) -> u32>(len) };
    // SAFETY: as above.
    let make =
        unsafe { core::mem::transmute::<usize, extern "system" fn(usize, u32) -> usize>(make) };
    let copy = make(b, len(b));
    (copy != 0).then_some(copy)
}

/// One owned element of type `t` copied (strings and clipboard data
/// deep, the rest bitwise); None without memory.
fn copy_element(t: u16, v: usize) -> Option<usize> {
    match t {
        VT_BSTR => copy_bstr(v),
        VT_LPSTR if v != 0 => copy_bytes(v as *const u8, string_bytes(v as *const u8, 1)),
        VT_LPWSTR if v != 0 => copy_bytes(v as *const u8, string_bytes(v as *const u8, 2)),
        VT_CLSID => copy_bytes(v as *const u8, 16),
        VT_CF if v != 0 => {
            // SAFETY: the caller's CLIPDATA; its data is size - 4 bytes
            // (the size counts the format).
            let c = unsafe { &*(v as *const ClipData) };
            let data = copy_bytes(c.data, (c.size as usize).saturating_sub(4))?;
            let q = copy_bytes((c as *const ClipData).cast(), core::mem::size_of::<ClipData>())?;
            // SAFETY: the new CLIPDATA.
            unsafe { (*(q as *mut ClipData)).data = data as *mut u8 };
            Some(q)
        }
        _ => Some(v),
    }
}

/// PropVariantCopy(dest, src): a copy that owns its own data: strings,
/// BSTRs, blobs, clipboard data and vectors copied, interfaces AddRef'd,
/// SAFEARRAYs copied with SafeArrayCopy; by-reference values copied as
/// they are. An invalid type is STG_E_INVALIDPARAMETER (dest zeroed).
#[unsafe(no_mangle)]
pub extern "system" fn PropVariantCopy(dest: *mut PropVariant, src: *const PropVariant) -> HResult {
    if dest.is_null() || src.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's PROPVARIANTs.
    let s = unsafe { &*src };
    let mut d = PropVariant { vt: s.vt, reserved: s.reserved, value: s.value };
    let (vt, base) = (s.vt, s.vt & 0xfff);
    let ok = match vt & (VT_VECTOR | VT_ARRAY | VT_BYREF) {
        0 if valid_simple(base) => match base {
            VT_DISPATCH | VT_UNKNOWN | VT_STREAM | VT_STORAGE | VT_STREAMED_OBJECT
            | VT_STORED_OBJECT => {
                if s.value[0] != 0 {
                    super::add_ref(s.value[0] as *mut c_void);
                }
                Some(())
            }
            VT_BSTR | VT_LPSTR | VT_LPWSTR | VT_CLSID | VT_CF => {
                copy_element(base, s.value[0]).map(|v| d.value[0] = v)
            }
            VT_BLOB | VT_BLOB_OBJECT => {
                copy_bytes(s.value[1] as *const u8, s.value[0] as u32 as usize)
                    .map(|v| d.value[1] = v)
            }
            VT_RECORD if s.value[0] != 0 => {
                missing(b"ole32.dll!PropVariantCopy(VT_RECORD)", 0);
                None
            }
            _ => Some(()),
        },
        VT_VECTOR if valid_vector(base) => {
            let (count, elements) = (s.value[0] as u32 as usize, s.value[1] as *const u8);
            let size = element_size(base);
            match copy_bytes(elements, count * size) {
                None => None,
                Some(copy) => {
                    d.value[1] = copy;
                    let mut ok = Some(());
                    if copy != 0 {
                        for i in 0..count {
                            let at = (copy + i * size) as *mut u8;
                            let copied = if base == VT_VARIANT {
                                // SAFETY: element i of both vectors.
                                let from = unsafe { elements.add(i * size) };
                                PropVariantCopy(at.cast(), from.cast()) >= 0
                            } else if matches!(base, VT_BSTR | VT_LPSTR | VT_LPWSTR | VT_CF) {
                                // SAFETY: element i of the new vector.
                                match copy_element(base, unsafe { *at.cast::<usize>() }) {
                                    Some(v) => {
                                        // SAFETY: as above.
                                        unsafe { *at.cast::<usize>() = v };
                                        true
                                    }
                                    None => false,
                                }
                            } else {
                                true
                            };
                            if !copied {
                                // The elements from here on still alias
                                // the source's: zero them so the clear
                                // below frees only the copies.
                                // SAFETY: the new vector's elements i..count.
                                unsafe { core::ptr::write_bytes(at, 0, (count - i) * size) };
                                ok = None;
                                break;
                            }
                        }
                    }
                    ok
                }
            }
        }
        VT_ARRAY if valid_array(base) => {
            if s.value[0] == 0 {
                Some(())
            } else {
                let f = export(&OLEAUT32, b"SafeArrayCopy\0");
                if f == 0 {
                    missing(b"ole32.dll!PropVariantCopy(helper DLL)", 0);
                    None
                } else {
                    // SAFETY: oleaut32's SafeArrayCopy(SAFEARRAY*, SAFEARRAY**).
                    let f: extern "system" fn(usize, *mut usize) -> HResult =
                        unsafe { core::mem::transmute(f) };
                    (f(s.value[0], &mut d.value[0]) >= 0).then_some(())
                }
            }
        }
        VT_BYREF if valid_array(base) => Some(()),
        _ if vt == VT_EMPTY || vt == VT_NULL => Some(()),
        _ => {
            // SAFETY: the caller's PROPVARIANT.
            unsafe { dest.write(PropVariant { vt: VT_EMPTY, reserved: [0; 3], value: [0; 2] }) };
            return STG_E_INVALIDPARAMETER;
        }
    };
    match ok {
        Some(()) => {
            // SAFETY: as above.
            unsafe { dest.write(d) };
            S_OK
        }
        None => {
            // What was copied before running out is freed.
            PropVariantClear(&mut d);
            // SAFETY: as above.
            unsafe { dest.write(PropVariant { vt: VT_EMPTY, reserved: [0; 3], value: [0; 2] }) };
            super::E_OUTOFMEMORY
        }
    }
}

/// STGMEDIUM (x64): the medium's type, its handle or pointer, the object
/// that owns it.
#[repr(C)]
pub struct StgMedium {
    tymed: u32,
    handle: usize,
    owner: usize,
}

const TYMED_HGLOBAL: u32 = 1;
const TYMED_FILE: u32 = 2;
const TYMED_ISTREAM: u32 = 4;
const TYMED_ISTORAGE: u32 = 8;
const TYMED_GDI: u32 = 16;
const TYMED_MFPICT: u32 = 32;
const TYMED_ENHMF: u32 = 64;

/// ReleaseStgMedium: with no owner (pUnkForRelease), frees the medium:
/// GlobalFree for TYMED_HGLOBAL (and a METAFILEPICT's metafile), the file
/// deleted and its name freed for TYMED_FILE, the GDI object or metafile
/// deleted; interfaces (TYMED_ISTREAM/ISTORAGE) are always released.
/// With an owner, only the owner is released (besides the interfaces).
#[unsafe(no_mangle)]
pub extern "system" fn ReleaseStgMedium(m: *mut StgMedium) {
    if m.is_null() {
        return;
    }
    // SAFETY: the caller's STGMEDIUM.
    let m = unsafe { &mut *m };
    let owned = m.owner == 0;
    match m.tymed {
        TYMED_ISTREAM | TYMED_ISTORAGE => release_ptr(m.handle),
        TYMED_HGLOBAL if owned && m.handle != 0 => {
            // SAFETY: a global memory handle the medium owns.
            unsafe { GlobalFree(m.handle) };
        }
        TYMED_FILE if m.handle != 0 => {
            if owned {
                // SAFETY: the file's NUL-terminated name.
                unsafe { DeleteFileW(m.handle as *const u16) };
            }
            free(m.handle as *mut u8);
        }
        TYMED_GDI if owned => call(&GDI32, b"DeleteObject\0", m.handle),
        TYMED_ENHMF if owned => call(&GDI32, b"DeleteEnhMetaFile\0", m.handle),
        TYMED_MFPICT if owned && m.handle != 0 => {
            // A METAFILEPICT (mm, x, y, then the HMETAFILE at offset 16)
            // in global memory.
            // SAFETY: the medium's global block, locked while read.
            let metafile = unsafe {
                let p = GlobalLock(m.handle);
                let metafile = if p.is_null() { 0 } else { *(p.add(16) as *const usize) };
                GlobalUnlock(m.handle);
                metafile
            };
            if metafile != 0 {
                call(&GDI32, b"DeleteMetaFile\0", metafile);
            }
            // SAFETY: as for TYMED_HGLOBAL.
            unsafe { GlobalFree(m.handle) };
        }
        _ => {}
    }
    release_ptr(m.owner);
    m.tymed = 0;
    m.handle = 0;
    m.owner = 0;
}

/// The object's `iid` interface, or the QueryInterface failure.
fn query(obj: *mut c_void, iid: &Guid) -> Result<*mut c_void, HResult> {
    let mut out: *mut c_void = core::ptr::null_mut();
    let hr = com_call!(obj, 0, fn(*const Guid, *mut *mut c_void) -> HResult, iid, &mut out);
    if hr < 0 || out.is_null() {
        Err(if hr < 0 { hr } else { super::E_NOINTERFACE })
    } else {
        Ok(out)
    }
}

/// Calls `f` on the object's IRunnableObject, or S_OK when it has none.
fn runnable(obj: *mut c_void, f: impl FnOnce(*mut c_void) -> HResult) -> HResult {
    if obj.is_null() {
        return E_INVALIDARG;
    }
    match query(obj, &IID_IRUNNABLEOBJECT) {
        Ok(r) => {
            let hr = f(r);
            release(r);
            hr
        }
        Err(_) => S_OK,
    }
}

/// OleRun: IRunnableObject::Run(NULL), or S_OK for an object that is
/// always running (no IRunnableObject).
#[unsafe(no_mangle)]
pub extern "system" fn OleRun(obj: *mut c_void) -> HResult {
    runnable(obj, |r| com_call!(r, 4, fn(*mut c_void) -> HResult, core::ptr::null_mut()))
}

/// OleLockRunning: IRunnableObject::LockRunning, or S_OK.
#[unsafe(no_mangle)]
pub extern "system" fn OleLockRunning(
    obj: *mut c_void,
    lock: i32,
    last_unlock_closes: i32,
) -> HResult {
    runnable(obj, |r| com_call!(r, 6, fn(i32, i32) -> HResult, lock, last_unlock_closes))
}

/// OleSetContainedObject: IRunnableObject::SetContainedObject, or S_OK.
#[unsafe(no_mangle)]
pub extern "system" fn OleSetContainedObject(obj: *mut c_void, contained: i32) -> HResult {
    runnable(obj, |r| com_call!(r, 7, fn(i32) -> HResult, contained))
}

/// CoSetProxyBlanket: IClientSecurity::SetBlanket on the proxy; an object
/// that is not a proxy (all of them in gala) has no IClientSecurity:
/// E_NOINTERFACE.
#[unsafe(no_mangle)]
pub extern "system" fn CoSetProxyBlanket(
    proxy: *mut c_void,
    authn: u32,
    authz: u32,
    principal: *const u16,
    level: u32,
    impersonation: u32,
    auth_info: *mut c_void,
    capabilities: u32,
) -> HResult {
    if proxy.is_null() {
        return E_INVALIDARG;
    }
    match query(proxy, &IID_ICLIENTSECURITY) {
        Ok(security) => {
            let hr = com_call!(
                security,
                4,
                fn(*mut c_void, u32, u32, *const u16, u32, u32, *mut c_void, u32) -> HResult,
                proxy,
                authn,
                authz,
                principal,
                level,
                impersonation,
                auth_info,
                capabilities
            );
            release(security);
            hr
        }
        Err(hr) => hr,
    }
}

/// CoQueryProxyBlanket: IClientSecurity::QueryBlanket, E_NOINTERFACE for
/// an object that is not a proxy.
#[unsafe(no_mangle)]
pub extern "system" fn CoQueryProxyBlanket(
    proxy: *mut c_void,
    authn: *mut u32,
    authz: *mut u32,
    principal: *mut *mut u16,
    level: *mut u32,
    impersonation: *mut u32,
    auth_info: *mut *mut c_void,
    capabilities: *mut u32,
) -> HResult {
    if proxy.is_null() {
        return E_INVALIDARG;
    }
    match query(proxy, &IID_ICLIENTSECURITY) {
        Ok(security) => {
            let hr = com_call!(
                security,
                3,
                fn(
                    *mut c_void,
                    *mut u32,
                    *mut u32,
                    *mut *mut u16,
                    *mut u32,
                    *mut u32,
                    *mut *mut c_void,
                    *mut u32,
                ) -> HResult,
                proxy,
                authn,
                authz,
                principal,
                level,
                impersonation,
                auth_info,
                capabilities
            );
            release(security);
            hr
        }
        Err(hr) => hr,
    }
}

/// CoFreeUnusedLibraries: Windows unloads the in-process servers whose
/// DllCanUnloadNow says so, at a time of its choosing; gala keeps them
/// loaded (allowed: the documentation promises no unload).
#[unsafe(no_mangle)]
pub extern "system" fn CoFreeUnusedLibraries() {}

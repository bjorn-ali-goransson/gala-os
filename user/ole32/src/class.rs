//! Class activation: CoGetClassObject and CoCreateInstance for in-process
//! servers, as documented. A class is registered under
//! `HKCR\CLSID\{clsid}\InprocServer32`, whose default value is the DLL's
//! path (REG_SZ or REG_EXPAND_SZ); the DLL is loaded and its
//! `DllGetClassObject` gives the class object, whose IClassFactory makes
//! instances. The ThreadingModel value is not looked at: gala has no
//! cross-apartment calls, so every object is used directly. Local and
//! remote servers (CLSCTX_LOCAL_SERVER, ..._REMOTE_SERVER) are not
//! supported (recorded); an unregistered class gives REGDB_E_CLASSNOTREG.
//! A server process's reference count and suspension
//! (CoAddRefServerProcess, CoReleaseServerProcess, CoSuspendClassObjects,
//! CoResumeClassObjects) are kept as documented; with no local servers
//! nothing is refused while suspended.

use core::ffi::c_void;

use super::{
    E_INVALIDARG, E_POINTER, GetProcAddress, Guid, HResult, LoadLibraryW, apartment, com_call,
    guid_text, missing, ole_iid, put, release,
};

#[repr(C)]
struct UnicodeString {
    length: u16,
    maximum: u16,
    buffer: *const u16,
}

#[repr(C)]
struct ObjectAttributes {
    length: u32,
    root: usize,
    name: *const UnicodeString,
    attributes: u32,
    security: usize,
    qos: usize,
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "ntdll", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "ntdll", kind = "raw-dylib"))]
unsafe extern "system" {
    fn NtOpenKey(out: *mut usize, access: u32, attributes: *const ObjectAttributes) -> u32;
    fn NtQueryValueKey(
        key: usize,
        name: *const UnicodeString,
        class: u32,
        buf: *mut u8,
        len: u32,
        ret: *mut u32,
    ) -> u32;
    fn NtClose(handle: usize) -> u32;
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn ExpandEnvironmentStringsW(src: *const u16, dst: *mut u16, size: u32) -> u32;
}

const IID_ICLASSFACTORY: Guid = ole_iid(1);
const CO_E_NOTINITIALIZED: HResult = 0x8004_01f0u32 as i32;
const REGDB_E_CLASSNOTREG: HResult = 0x8004_0154u32 as i32;
const CO_E_DLLNOTFOUND: HResult = 0x8004_01f8u32 as i32;
const CO_E_ERRORINDLL: HResult = 0x8004_01f9u32 as i32;
const CLSCTX_INPROC: u32 = 0x1 | 0x2; // INPROC_SERVER | INPROC_HANDLER
const KEY_QUERY_VALUE: u32 = 0x1;
const OBJ_CASE_INSENSITIVE: u32 = 0x40;
const REG_SZ: u32 = 1;
const REG_EXPAND_SZ: u32 = 2;
const REG_DWORD: u32 = 4;
pub(crate) const MAX_PATH: usize = 260;

/// The default value of `HKCR\CLSID\{clsid}\<subkey>` (REG_SZ or
/// REG_EXPAND_SZ, unexpanded, NUL-terminated), if any.
fn class_value(clsid: &Guid, subkey: &str) -> Option<[u16; MAX_PATH]> {
    let prefix = "\\Registry\\Machine\\SOFTWARE\\Classes\\CLSID\\";
    let mut name = [0u16; 128];
    let mut n = 0;
    for c in prefix
        .encode_utf16()
        .chain(guid_text(clsid))
        .chain("\\".encode_utf16())
        .chain(subkey.encode_utf16())
    {
        *name.get_mut(n)? = c;
        n += 1;
    }
    string_value(&name[..n], "")
}

/// Opens the key of NT name `key` for querying values.
fn open_key(key: &[u16]) -> Option<usize> {
    let bytes = u16::try_from(key.len() * 2).ok()?;
    let us = UnicodeString { length: bytes, maximum: bytes, buffer: key.as_ptr() };
    let attributes = ObjectAttributes {
        length: size_of::<ObjectAttributes>() as u32,
        root: 0,
        name: &us,
        attributes: OBJ_CASE_INSENSITIVE,
        security: 0,
        qos: 0,
    };
    let mut handle = 0;
    // SAFETY: valid attributes and output.
    (unsafe { NtOpenKey(&mut handle, KEY_QUERY_VALUE, &attributes) } == 0).then_some(handle)
}

/// Whether the key of NT name `key` exists.
pub(crate) fn key_exists(key: &[u16]) -> bool {
    open_key(key)
        .map(|h| {
            // SAFETY: our handle.
            unsafe { NtClose(h) };
        })
        .is_some()
}

/// Value `value` ("" for the unnamed one) of key `key`: its type and up to
/// MAX_PATH units of data (KEY_VALUE_PARTIAL_INFORMATION's TitleIndex,
/// Type, DataLength, Data).
fn query_value(key: &[u16], value: &str) -> Option<(u32, [u32; 3 + MAX_PATH / 2])> {
    let handle = open_key(key)?;
    let mut name = [0u16; 32];
    let mut n = 0;
    for c in value.encode_utf16() {
        *name.get_mut(n)? = c;
        n += 1;
    }
    let us =
        UnicodeString { length: (n * 2) as u16, maximum: (n * 2) as u16, buffer: name.as_ptr() };
    let mut buf = [0u32; 3 + MAX_PATH / 2];
    let mut got = 0;
    // SAFETY: our key and buffer.
    let status = unsafe {
        let s = NtQueryValueKey(
            handle,
            &us,
            2,
            buf.as_mut_ptr().cast(),
            size_of_val(&buf) as u32,
            &mut got,
        );
        NtClose(handle);
        s
    };
    (status == 0).then_some((buf[1], buf))
}

/// A REG_DWORD value of key `key`.
pub(crate) fn dword_value(key: &[u16], value: &str) -> Option<u32> {
    let (kind, buf) = query_value(key, value)?;
    (kind == REG_DWORD && buf[2] == 4).then_some(buf[3])
}

/// A REG_SZ or REG_EXPAND_SZ value of key `key` (unexpanded,
/// NUL-terminated), if any and not empty.
pub(crate) fn string_value(key: &[u16], value: &str) -> Option<[u16; MAX_PATH]> {
    let (kind, buf) = query_value(key, value)?;
    let len = buf[2] as usize / 2;
    if (kind != REG_SZ && kind != REG_EXPAND_SZ) || len == 0 {
        return None;
    }
    let mut raw = [0u16; MAX_PATH];
    // SAFETY: the value's data follows the 12-byte header in `buf`.
    let data = unsafe {
        core::slice::from_raw_parts(buf.as_ptr().add(3).cast::<u16>(), len.min(MAX_PATH - 1))
    };
    raw[..data.len()].copy_from_slice(data);
    // Without a terminating NUL in the data: it ends at the data's end.
    if let Some(end) = raw.iter().position(|&c| c == 0) {
        raw[end..].fill(0);
    }
    Some(raw)
}

/// `raw` (NUL-terminated) with its environment variables expanded.
pub(crate) fn expand(raw: &[u16; MAX_PATH]) -> Option<[u16; MAX_PATH]> {
    let mut path = [0u16; MAX_PATH];
    // SAFETY: a NUL-terminated source and a buffer of MAX_PATH characters.
    let n = unsafe { ExpandEnvironmentStringsW(raw.as_ptr(), path.as_mut_ptr(), MAX_PATH as u32) };
    (n != 0 && n as usize <= MAX_PATH).then_some(path)
}

/// The registered in-process server of `clsid` (a NUL-terminated path
/// with environment variables expanded), if any.
fn inproc_server(clsid: &Guid) -> Option<[u16; MAX_PATH]> {
    expand(&class_value(clsid, "InprocServer32")?)
}

/// ProgIDFromCLSID(clsid, *lplpszProgID): the class's ProgID (the
/// default value of its `ProgID` subkey) in task memory;
/// REGDB_E_CLASSNOTREG (and a null output) when it has none.
#[unsafe(no_mangle)]
pub extern "system" fn ProgIDFromCLSID(clsid: *const Guid, progid: *mut *mut u16) -> HResult {
    if clsid.is_null() || progid.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's output.
    unsafe { *progid = core::ptr::null_mut() };
    // SAFETY: the caller's CLSID.
    let Some(text) = class_value(unsafe { &*clsid }, "ProgID") else {
        return REGDB_E_CLASSNOTREG;
    };
    let n = text.iter().position(|&c| c == 0).unwrap_or(MAX_PATH - 1);
    let p = super::CoTaskMemAlloc((n + 1) * 2).cast::<u16>();
    if p.is_null() {
        return super::E_OUTOFMEMORY;
    }
    // SAFETY: a fresh block of n + 1 characters; the caller's output.
    unsafe {
        core::ptr::copy_nonoverlapping(text.as_ptr(), p, n);
        *p.add(n) = 0;
        *progid = p;
    }
    super::S_OK
}

type GetClassObject = extern "system" fn(*const Guid, *const Guid, *mut *mut c_void) -> HResult;

/// CoGetClassObject: the class object of a registered in-process server.
#[unsafe(no_mangle)]
pub extern "system" fn CoGetClassObject(
    clsid: *const Guid,
    context: u32,
    _info: *mut c_void,
    iid: *const Guid,
    out: *mut *mut c_void,
) -> HResult {
    if out.is_null() || clsid.is_null() || iid.is_null() {
        return E_INVALIDARG;
    }
    put(out, core::ptr::null_mut());
    if apartment().is_none() {
        return CO_E_NOTINITIALIZED;
    }
    // SAFETY: the caller's CLSID.
    let clsid_value = unsafe { *clsid };
    let detail = u64::from(u32::from_le_bytes([
        clsid_value[0],
        clsid_value[1],
        clsid_value[2],
        clsid_value[3],
    ]));
    if context & CLSCTX_INPROC == 0 {
        missing(b"ole32.dll!CoGetClassObject(out-of-process server)", detail);
        return REGDB_E_CLASSNOTREG;
    }
    let Some(path) = inproc_server(&clsid_value) else {
        missing(b"ole32.dll!CoGetClassObject(class not registered)", detail);
        return REGDB_E_CLASSNOTREG;
    };
    // SAFETY: a NUL-terminated path and export name.
    let module = unsafe { LoadLibraryW(path.as_ptr()) };
    if module.is_null() {
        missing(b"ole32.dll!CoGetClassObject(server DLL not found)", detail);
        return CO_E_DLLNOTFOUND;
    }
    // SAFETY: as above.
    let f = unsafe { GetProcAddress(module, c"DllGetClassObject".as_ptr().cast()) };
    if f.is_null() {
        return CO_E_ERRORINDLL;
    }
    // SAFETY: DllGetClassObject's documented signature.
    let get: GetClassObject = unsafe { core::mem::transmute(f) };
    get(clsid, iid, out)
}

/// CoCreateInstance: the class object's IClassFactory::CreateInstance.
#[unsafe(no_mangle)]
pub extern "system" fn CoCreateInstance(
    clsid: *const Guid,
    outer: *mut c_void,
    context: u32,
    iid: *const Guid,
    out: *mut *mut c_void,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    if iid.is_null() {
        return E_INVALIDARG;
    }
    let mut factory: *mut c_void = core::ptr::null_mut();
    let hr =
        CoGetClassObject(clsid, context, core::ptr::null_mut(), &IID_ICLASSFACTORY, &mut factory);
    if hr < 0 {
        return hr;
    }
    let hr = com_call!(
        factory,
        3,
        fn(*mut c_void, *const Guid, *mut *mut c_void) -> HResult,
        outer,
        iid,
        out
    );
    release(factory);
    hr
}

// ---- The server process's lifetime ----

/// The process's server reference count (CoAddRefServerProcess).
static SERVER_REFS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
/// Whether the class objects are suspended (CoSuspendClassObjects, or
/// CoReleaseServerProcess reaching 0) until CoResumeClassObjects.
static SUSPENDED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// CoAddRefServerProcess: the process's server reference count, plus one
/// (the new count).
#[unsafe(no_mangle)]
pub extern "system" fn CoAddRefServerProcess() -> u32 {
    SERVER_REFS.fetch_add(1, core::sync::atomic::Ordering::AcqRel).wrapping_add(1)
}

/// CoReleaseServerProcess: the count minus one (the new count, never
/// below 0); at 0 the class objects are suspended, as documented, so no
/// new activation reaches a server that is shutting down.
#[unsafe(no_mangle)]
pub extern "system" fn CoReleaseServerProcess() -> u32 {
    use core::sync::atomic::Ordering;
    let old = SERVER_REFS
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| Some(n.saturating_sub(1)))
        .unwrap_or(0);
    let now = old.saturating_sub(1);
    if now == 0 {
        SUSPENDED.store(true, Ordering::Release);
    }
    now
}

/// CoSuspendClassObjects: the process's class objects take no new
/// activations (gala's activations are in-process, through
/// DllGetClassObject: nothing to refuse yet); S_OK.
#[unsafe(no_mangle)]
pub extern "system" fn CoSuspendClassObjects() -> HResult {
    SUSPENDED.store(true, core::sync::atomic::Ordering::Release);
    super::S_OK
}

/// CoResumeClassObjects: activations are taken again; S_OK.
#[unsafe(no_mangle)]
pub extern "system" fn CoResumeClassObjects() -> HResult {
    SUSPENDED.store(false, core::sync::atomic::Ordering::Release);
    super::S_OK
}

//! Class activation: CoGetClassObject and CoCreateInstance for in-process
//! servers, as documented. A class is registered under
//! `HKCR\CLSID\{clsid}\InprocServer32`, whose default value is the DLL's
//! path (REG_SZ or REG_EXPAND_SZ); the DLL is loaded and its
//! `DllGetClassObject` gives the class object, whose IClassFactory makes
//! instances. The ThreadingModel value is not looked at: gala has no
//! cross-apartment calls, so every object is used directly. Local and
//! remote servers (CLSCTX_LOCAL_SERVER, ..._REMOTE_SERVER) are not
//! supported (recorded); an unregistered class gives REGDB_E_CLASSNOTREG.
//! The process's activation filter (CoRegisterActivationFilter) has its
//! say on every activation before anything is looked up: it may refuse
//! the class or replace it.
//!
//! Before the registry comes the table of classes the process answers
//! for itself (CoRegisterClassObject): an OLE server registers its own
//! class objects and is then handed them back, which is how Word loads
//! what it saved. An apartment owns a registration -- every thread of
//! the multi-threaded apartment shares one, a single-threaded apartment
//! has its own -- and CoUninitialize takes the apartment's registrations
//! with it. A registration is read by an activation that names an
//! in-process context and by no other, whatever context the
//! registration itself named, because the table belongs to the process
//! and a local activation goes to the service control manager instead
//! (all of it measured, `ole32_test`).
//!
//! A server process's reference count (CoAddRefServerProcess,
//! CoReleaseServerProcess) is kept as documented. The suspension that
//! the documentation pairs with it is the service control manager's,
//! which gala has none of: a count reaching zero leaves these
//! registrations serving, as on Windows. CoSuspendClassObjects does
//! suspend them, and nothing lifts that -- CoResumeClassObjects in an
//! apartment is an error on Windows and here.

use core::cell::UnsafeCell;
use core::ffi::c_void;

use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use gala_lock::Lock;

use super::{
    E_INVALIDARG, E_POINTER, GetProcAddress, Guid, HResult, LoadLibraryW, add_ref, apartment,
    com_call, guid_text, missing, ole_iid, put, reached, release,
};

const CO_E_NOT_SUPPORTED: HResult = 0x8000_4021u32 as i32;
const E_ACCESSDENIED: HResult = 0x8007_0005u32 as i32;
/// The CLSID a filter leaves in place of a replacement: no replacement.
const GUID_NULL: Guid = [0; 16];

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
    fn GetCurrentThreadId() -> u32;
}

/// This thread, as a registration's owning apartment is written down.
pub(crate) fn thread_id() -> u32 {
    // SAFETY: no arguments, no output.
    unsafe { GetCurrentThreadId() }
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
    // SAFETY: the caller's CLSID.
    let asked = unsafe { *clsid };
    // The process's activation filter, if it registered one, speaks
    // first: before the context is looked at and before the registry is
    // read, as on Windows, where a filter is consulted even for the
    // contexts gala does not support.
    let clsid_value = match filtered(asked) {
        Ok(c) => c,
        Err(hr) => return hr,
    };
    let detail = u64::from(u32::from_le_bytes([
        clsid_value[0],
        clsid_value[1],
        clsid_value[2],
        clsid_value[3],
    ]));
    // A thread that never initialized COM is a member of the process's
    // MTA when there is one (the implicit MTA).
    if apartment().is_none() && !super::implicit_mta() {
        missing(b"ole32.dll!CoGetClassObject(COM not initialized, no MTA)", detail);
        return CO_E_NOTINITIALIZED;
    }
    if context & CLSCTX_INPROC == 0 {
        missing(b"ole32.dll!CoGetClassObject(out-of-process server)", detail);
        return REGDB_E_CLASSNOTREG;
    }
    // A class this process answers for comes before the registry, and
    // beats it (measured). Only an in-process activation reads the
    // table, which is what the check above has just established.
    if let Some((object, key, single_use)) = registered(&clsid_value) {
        // The class object is asked for the caller's interface, and it
        // is that reference the caller is given: a reference on the
        // class object itself goes with it (measured: one more on the
        // object than before, for an IClassFactory the caller releases).
        let hr = com_call!(object, 0, fn(*const Guid, *mut *mut c_void) -> HResult, iid, out);
        release(object);
        if hr < 0 {
            put(out, core::ptr::null_mut());
            return hr;
        }
        if single_use {
            spend(key);
        }
        return hr;
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
    get(&clsid_value, iid, out)
}

// ---- Activation filters ----

/// CoRegisterActivationFilter's filter: one per process, kept as the raw
/// pointer the caller gave (Windows asks it for no interface and holds
/// no reference on it). 0 until one is registered; never cleared.
static ACTIVATION_FILTER: AtomicUsize = AtomicUsize::new(0);

/// How many times a filter may replace the class before gala stops. On
/// Windows the chain has no bound and a filter that keeps naming a new
/// class loops for ever; gala stops and records the row instead.
const REPLACEMENTS: usize = 8;

/// CoRegisterActivationFilter(pActivationFilter): the process's
/// IActivationFilter, consulted on every activation.
///
/// Measured on Windows 11 (`ole32_test`), where little of this is
/// documented: it needs no apartment (S_OK before CoInitializeEx) and
/// outlives CoUninitialize, because the filter belongs to the process
/// and not to an apartment; the filter is kept as the bare pointer, with
/// no QueryInterface and no AddRef; the first one wins, so the same
/// pointer again is S_OK any number of times while a different one is
/// CO_E_NOT_SUPPORTED; and a NULL filter is E_INVALIDARG and does not
/// remove the registration -- nothing does.
///
/// Office's shared code registers one early in every program (Word's
/// MSO.DLL delay-loads this function), which is the reason gala has it.
#[unsafe(no_mangle)]
pub extern "system" fn CoRegisterActivationFilter(filter: *mut c_void) -> HResult {
    // Every answer leaves a row: a program that registers a filter and
    // then sees its classes refused is one row away from being
    // explained, and the rows are bounded -- one per answer, counted.
    if filter.is_null() {
        reached(b"ole32.dll!CoRegisterActivationFilter(no filter)", 0);
        return E_INVALIDARG;
    }
    let want = filter as usize;
    match ACTIVATION_FILTER.compare_exchange(0, want, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => {
            reached(b"ole32.dll!CoRegisterActivationFilter(registered)", 0);
            super::S_OK
        }
        Err(have) if have == want => {
            reached(b"ole32.dll!CoRegisterActivationFilter(the same filter again)", 0);
            super::S_OK
        }
        Err(_) => {
            reached(b"ole32.dll!CoRegisterActivationFilter(another filter is registered)", 0);
            CO_E_NOT_SUPPORTED
        }
    }
}

/// The class to activate once the process's activation filter has had
/// its say, or the error the activation fails with.
///
/// Measured on Windows 11 (`ole32_test`): HandleActivation is given the
/// activation type 0, the CLSID asked for, and a replacement CLSID that
/// is GUID_NULL on entry. A filter that leaves it alone (or writes the
/// class it was asked about) lets that class be activated; a filter that
/// writes another class is asked about that one too, until it stops
/// replacing. A filter that fails turns the activation into
/// E_ACCESSDENIED, whatever it returned.
fn filtered(asked: Guid) -> Result<Guid, HResult> {
    let filter = ACTIVATION_FILTER.load(Ordering::Acquire) as *mut c_void;
    if filter.is_null() {
        return Ok(asked);
    }
    let mut clsid = asked;
    for _ in 0..REPLACEMENTS {
        let mut replacement = GUID_NULL;
        let hr = com_call!(
            filter,
            3,
            fn(u32, *const Guid, *mut Guid) -> HResult,
            0u32,
            &clsid,
            &mut replacement
        );
        if hr < 0 {
            reached(b"ole32.dll!CoGetClassObject(the activation filter refused the class)", {
                #[allow(clippy::cast_sign_loss)]
                let detail = hr as u32;
                u64::from(detail)
            });
            return Err(E_ACCESSDENIED);
        }
        if replacement == GUID_NULL || replacement == clsid {
            return Ok(clsid);
        }
        reached(
            b"ole32.dll!CoGetClassObject(the activation filter replaced the class)",
            u64::from(u32::from_le_bytes([
                replacement[0],
                replacement[1],
                replacement[2],
                replacement[3],
            ])),
        );
        clsid = replacement;
    }
    // gala's bound, not Windows's: there a filter may replace for ever.
    missing(
        b"ole32.dll!CoGetClassObject(the activation filter kept replacing the class)",
        REPLACEMENTS as u64,
    );
    Err(E_ACCESSDENIED)
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
static SERVER_REFS: AtomicU32 = AtomicU32::new(0);

/// CoAddRefServerProcess: the process's server reference count, plus one
/// (the new count).
#[unsafe(no_mangle)]
pub extern "system" fn CoAddRefServerProcess() -> u32 {
    SERVER_REFS.fetch_add(1, Ordering::AcqRel).wrapping_add(1)
}

/// CoReleaseServerProcess: the count minus one (the new count, never
/// below 0).
///
/// The documentation says the class objects are suspended when the count
/// reaches 0, so that no new activation reaches a server that is shutting
/// down. What it means is the ones the service control manager hands out,
/// and this process has none of those: measured on Windows, a class
/// registered here goes on being activated here after the count reaches 0
/// (`ole32_test`). So the count is all this keeps.
#[unsafe(no_mangle)]
pub extern "system" fn CoReleaseServerProcess() -> u32 {
    let old = SERVER_REFS
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| Some(n.saturating_sub(1)))
        .unwrap_or(0);
    old.saturating_sub(1)
}

/// CoSuspendClassObjects: the class objects this process has registered
/// take no new activations; S_OK.
///
/// Nothing lifts it: CoResumeClassObjects, the call the documentation
/// pairs with this one, is an error in a process the service control
/// manager did not start as a server, which is every process here
/// (measured). A registration made after this one stands, as a class
/// object that did not exist to be suspended.
#[unsafe(no_mangle)]
pub extern "system" fn CoSuspendClassObjects() -> HResult {
    CLASSES.lock.with(|| {
        // SAFETY: under the lock.
        for r in unsafe { &mut *CLASSES.slots.get() }.iter_mut().filter(|r| r.key != 0) {
            r.suspended = true;
        }
    });
    super::S_OK
}

/// CoResumeClassObjects: on Windows this is the service control
/// manager's business, and a process it did not start as a server is
/// told so -- CO_E_RESUME_NOT_A_SERVER, with whatever was suspended left
/// suspended (measured, in every state an apartment can be in: with a
/// suspended registration, with a plain one, and with none at all).
/// With no apartment at all it is S_OK, where there can be no
/// registration either way.
#[unsafe(no_mangle)]
pub extern "system" fn CoResumeClassObjects() -> HResult {
    if apartment().is_some() || super::implicit_mta() {
        reached(b"ole32.dll!CoResumeClassObjects(not started as a server)", 0);
        return CO_E_RESUME_NOT_A_SERVER;
    }
    super::S_OK
}

// ---- The class objects this process answers for ----

/// REGCLS: how a registration is used (the low two bits) and the flags
/// over them. Anything outside these is E_INVALIDARG (measured).
const REGCLS_SINGLEUSE: u32 = 0;
const REGCLS_MULTI_SEPARATE: u32 = 2;
const REGCLS_SUSPENDED: u32 = 4;
const REGCLS_SURROGATE: u32 = 8;
const REGCLS_AGILE: u32 = 0x10;
const REGCLS_KNOWN: u32 = 3 | REGCLS_SUSPENDED | REGCLS_SURROGATE | REGCLS_AGILE;

/// What CoResumeClassObjects answers a process the service control
/// manager did not start as a server: Win32 1734 as an HRESULT, which is
/// what Windows says and is documented nowhere.
const CO_E_RESUME_NOT_A_SERVER: HResult = 0x8007_06c6u32 as i32;

/// One registration (CoRegisterClassObject).
#[derive(Clone, Copy)]
struct Registration {
    /// The key the caller was given, which is never 0; 0 in a free slot.
    /// Keys only ever rise, so the greatest key of a class is its
    /// newest registration -- the one that serves (measured).
    key: u32,
    clsid: Guid,
    /// The class object, holding the one reference the registration took.
    object: *mut c_void,
    /// The apartment it belongs to: the single-threaded apartment's
    /// thread, or 0 for the multi-threaded one, which all of its threads
    /// share.
    thread: u32,
    /// REGCLS_SINGLEUSE: it serves one activation.
    single_use: bool,
    /// A single-use registration that has served. It keeps its reference
    /// and its key still revokes it, but no activation finds it again
    /// (measured).
    spent: bool,
    /// Registered REGCLS_SUSPENDED, or suspended by
    /// CoSuspendClassObjects since. Nothing lifts it.
    suspended: bool,
    /// How many look-ups are between taking this registration out of
    /// the table and taking their reference on its class object. The
    /// registration's own reference stands until the last of them is
    /// done, so a revoke in that window cannot free the object under a
    /// thread that is about to call AddRef on it.
    borrows: u32,
    /// Revoked while borrowed: no look-up finds it, and the slot and
    /// the reference go when the last borrow ends.
    retired: bool,
}

const FREE: Registration = Registration {
    key: 0,
    clsid: GUID_NULL,
    object: core::ptr::null_mut(),
    thread: 0,
    single_use: false,
    spent: false,
    suspended: false,
    borrows: 0,
    retired: false,
};

/// How many registrations a process may hold at once. Windows has no
/// such limit; one past this is E_OUTOFMEMORY with a row to say so
/// (ARCHITECTURE "COM: ole32 and oleaut32", Limits).
const REGISTRATIONS: usize = 128;

struct Classes {
    lock: Lock,
    slots: UnsafeCell<[Registration; REGISTRATIONS]>,
    /// The next key. It only rises, so a key is never reused and a
    /// stale one names nothing.
    next_key: AtomicU32,
}

// SAFETY: `slots` is only touched under `lock`.
unsafe impl Sync for Classes {}

static CLASSES: Classes = Classes {
    lock: Lock::new(b"ole32.dll!the class objects"),
    slots: UnsafeCell::new([FREE; REGISTRATIONS]),
    next_key: AtomicU32::new(1),
};

/// Whether a registration owned by apartment `thread` (0 the MTA) is
/// this thread's to use.
fn mine(owner: u32, in_mta: bool) -> bool {
    if owner == 0 { in_mta } else { owner == thread_id() }
}

/// This thread's apartment, as a registration records it: Some(0) for
/// the multi-threaded apartment (the implicit one included), Some(this
/// thread) for a single-threaded one, None for a thread with no
/// apartment at all.
fn owning_apartment() -> Option<u32> {
    match apartment() {
        Some(1) => Some(0),
        Some(_) => Some(thread_id()),
        None if super::implicit_mta() => Some(0),
        None => None,
    }
}

/// The class object this process has registered for `clsid` and this
/// thread may use, with a reference taken on it: the newest such
/// registration, passing over the suspended, the spent and the retired.
///
/// AddRef is the program's code, so the lock is let go before it is
/// called (gala_lock: nothing is held across a call into a program).
/// What keeps the object alive in between is the borrow the look-up
/// leaves on the registration: a revoke meanwhile takes the
/// registration out of reach but leaves its reference until the borrow
/// ends.
fn registered(clsid: &Guid) -> Option<(*mut c_void, u32, bool)> {
    let in_mta = apartment() == Some(1) || (apartment().is_none() && super::implicit_mta());
    let (slot, object, key, single_use) = CLASSES.lock.with(|| {
        // SAFETY: under the lock.
        let slots = unsafe { &mut *CLASSES.slots.get() };
        let slot = slots
            .iter()
            .enumerate()
            .filter(|(_, r)| {
                r.key != 0
                    && r.clsid == *clsid
                    && !r.suspended
                    && !r.spent
                    && !r.retired
                    && mine(r.thread, in_mta)
            })
            .max_by_key(|(_, r)| r.key)?
            .0;
        let r = &mut slots[slot];
        r.borrows += 1;
        Some((slot, r.object, r.key, r.single_use))
    })?;
    add_ref(object);
    give_back(slot);
    Some((object, key, single_use))
}

/// Gives back the borrow `registered` took, and finishes the revoke
/// that was waiting on it, if there was one.
fn give_back(slot: usize) {
    let left = CLASSES.lock.with(|| {
        // SAFETY: under the lock.
        let r = &mut unsafe { &mut *CLASSES.slots.get() }[slot];
        r.borrows -= 1;
        if r.borrows == 0 && r.retired {
            let object = r.object;
            *r = FREE;
            return object;
        }
        core::ptr::null_mut()
    });
    // Released with the lock let go, as a revoke does.
    if !left.is_null() {
        release(left);
    }
}

/// Marks a single-use registration as having served, once it has: the
/// next activation of that class does not find it.
fn spend(key: u32) {
    CLASSES.lock.with(|| {
        // SAFETY: under the lock.
        if let Some(r) = unsafe { &mut *CLASSES.slots.get() }.iter_mut().find(|r| r.key == key) {
            r.spent = true;
        }
    });
}

/// CoRegisterClassObject(rclsid, pUnk, dwClsContext, flags, lpdwRegister):
/// the class object this process answers for `rclsid` with, until it is
/// revoked or the apartment ends.
///
/// Measured on Windows (`ole32_test`), where the documentation says
/// little of it: the registration takes one reference on the class object
/// and gives it back when it is revoked, while an activation leaves the
/// count where it found it; the same class may be registered twice, and
/// the newer serves until it is revoked, when the older serves again; a
/// registration beats the registry; the apartment owns it, so every
/// thread of the MTA shares one and another apartment sees nothing of it;
/// REGCLS_SINGLEUSE serves once and then holds its reference without
/// serving; REGCLS_SUSPENDED registers and never serves; and the key is
/// never 0. The refusals: a NULL key is E_INVALIDARG, no apartment is
/// CO_E_NOTINITIALIZED with the caller's key untouched, and a flag that
/// is not a flag, or no class object at all, is E_INVALIDARG with the key
/// zeroed -- so the apartment is looked at before the key is written and
/// the rest after.
#[unsafe(no_mangle)]
pub extern "system" fn CoRegisterClassObject(
    clsid: *const Guid,
    object: *mut c_void,
    context: u32,
    flags: u32,
    key: *mut u32,
) -> HResult {
    if clsid.is_null() || key.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's CLSID.
    let asked = unsafe { *clsid };
    let detail = u64::from(u32::from_le_bytes([asked[0], asked[1], asked[2], asked[3]]));
    let Some(thread) = owning_apartment() else {
        missing(b"ole32.dll!CoRegisterClassObject(COM not initialized, no MTA)", detail);
        return CO_E_NOTINITIALIZED;
    };
    put(key, 0);
    if flags & !REGCLS_KNOWN != 0 || object.is_null() {
        return E_INVALIDARG;
    }
    // What gala does not keep apart, said out loud: a separate
    // registration for the local server is the same thing here, where
    // there is no local server, and a surrogate is a process gala has no
    // way to be.
    if flags & 3 == REGCLS_MULTI_SEPARATE {
        reached(b"ole32.dll!CoRegisterClassObject(REGCLS_MULTI_SEPARATE)", detail);
    }
    if flags & REGCLS_SURROGATE != 0 {
        missing(b"ole32.dll!CoRegisterClassObject(REGCLS_SURROGATE)", detail);
    }
    // A class object registered for an out-of-process context alone is
    // one no other process can reach here: gala has no service control
    // manager to hand it out. It is registered all the same, and found
    // by this process, as on Windows.
    if context != 0 && context & CLSCTX_INPROC == 0 {
        missing(b"ole32.dll!CoRegisterClassObject(out-of-process server)", detail);
    }
    // The class object's own reference, taken before the lock: the
    // table must not be held across a call into a program's code.
    add_ref(object);
    let new = Registration {
        key: CLASSES.next_key.fetch_add(1, Ordering::AcqRel),
        clsid: asked,
        object,
        thread,
        single_use: flags & 3 == REGCLS_SINGLEUSE,
        spent: false,
        suspended: flags & REGCLS_SUSPENDED != 0,
        borrows: 0,
        retired: false,
    };
    let taken = CLASSES.lock.with(|| {
        // SAFETY: under the lock.
        let slot = unsafe { &mut *CLASSES.slots.get() }.iter_mut().find(|r| r.key == 0);
        match slot {
            Some(slot) => {
                *slot = new;
                true
            }
            None => false,
        }
    });
    if !taken {
        // The one failure Windows does not have. The row names the
        // class, so a program that cannot register its own is one row
        // away from being explained.
        missing(b"ole32.dll!CoRegisterClassObject(no room for another)", detail);
        release(object);
        return super::E_OUTOFMEMORY;
    }
    reached(b"ole32.dll!CoRegisterClassObject", detail);
    put(key, new.key);
    super::S_OK
}

/// CoRevokeClassObject(dwRegister): the registration ends and its
/// reference on the class object is given back. A key that was revoked
/// already, or was never one, is S_OK (measured).
#[unsafe(no_mangle)]
pub extern "system" fn CoRevokeClassObject(key: u32) -> HResult {
    let object = CLASSES.lock.with(|| {
        // SAFETY: under the lock.
        let slot = unsafe { &mut *CLASSES.slots.get() }
            .iter_mut()
            .find(|r| r.key != 0 && !r.retired && r.key == key)?;
        if slot.borrows > 0 {
            // A look-up is between this slot and its AddRef: the
            // registration goes out of reach now and its reference
            // when that borrow ends.
            slot.retired = true;
            return None;
        }
        let object = slot.object;
        *slot = FREE;
        Some(object)
    });
    // Released with the lock let go: Release is the program's code.
    if let Some(object) = object {
        release(object);
    }
    super::S_OK
}

/// The registrations of an apartment that is ending end with it, each
/// class object released (measured: CoUninitialize with a registration
/// standing revokes it and releases the object, and its key then names
/// nothing).
pub(crate) fn revoke_apartment(thread: u32) {
    let mut gone = [core::ptr::null_mut::<c_void>(); REGISTRATIONS];
    let n = CLASSES.lock.with(|| {
        // SAFETY: under the lock.
        let mut n = 0;
        for r in unsafe { &mut *CLASSES.slots.get() }.iter_mut() {
            if r.key == 0 || r.retired || r.thread != thread {
                continue;
            }
            if r.borrows > 0 {
                r.retired = true;
                continue;
            }
            gone[n] = r.object;
            n += 1;
            *r = FREE;
        }
        n
    });
    // Released with the lock let go, as a revoke does.
    for object in &gone[..n] {
        release(*object);
    }
}

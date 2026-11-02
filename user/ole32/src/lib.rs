//! gala's ole32.dll: the COM basics language runtimes use, as documented.
//! gala has no registry, so no classes, ProgIDs or proxies exist: class
//! lookups fail as on a machine where nothing is registered (recorded in
//! the ledger). What works:
//!
//! - Apartments (CoInitializeEx/CoUninitialize, RoInitialize): per-thread
//!   state behind TEB.ReservedForOle (x64 offset 0x1758): the apartment
//!   kind and its reference count, initialize spies, the error object.
//!   One process-wide MTA context and a context per STA thread
//!   (CoGetContextToken, CoGetObjectContext with IComThreadingInfo).
//! - Task memory on the process heap; CoCreateGuid (RFC 4122 version 4
//!   from ProcessPrng); StringFromGUID2.
//! - CreateStreamOnHGlobal without an HGLOBAL: an IStream on heap memory.
//! - CoCreateFreeThreadedMarshaler: an aggregatable object with IMarshal,
//!   whose marshaling methods are not there yet (E_NOTIMPL, recorded),
//!   like CoMarshalInterface and friends (no cross-apartment calls yet).
//! - CoWaitForMultipleHandles (waits; no message pumping, gala has no
//!   window messages).
//! - Get/SetErrorInfo (oleaut32 forwards its ordinals 200 and 201 here).

#![no_std]
#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, Ordering};

mod stream;
mod unimplemented;

#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memmove(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
    fn strlen(s: *const u8) -> usize;
    // MSVC-target unwind tables reference it (gala's ntdll provides it).
    fn __CxxFrameHandler3();
    // The stack probe for frames over a page (ntdll exports it).
    fn __chkstk();
}

/// MSVC-target codegen references this marker symbol directly.
#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn GalaRecordMissing(name: *const u8, length: usize, what: u32, detail: u64) -> u32;
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetProcessHeap() -> usize;
    fn HeapAlloc(heap: usize, flags: u32, size: usize) -> *mut u8;
    fn HeapReAlloc(heap: usize, flags: u32, p: *mut u8, size: usize) -> *mut u8;
    fn HeapFree(heap: usize, flags: u32, p: *mut u8) -> i32;
    fn HeapSize(heap: usize, flags: u32, p: *const u8) -> usize;
    fn WaitForMultipleObjectsEx(
        count: u32,
        handles: *const usize,
        all: i32,
        ms: u32,
        alertable: i32,
    ) -> u32;
    fn GetLastError() -> u32;
    fn LoadLibraryW(name: *const u16) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> *const c_void;
}

#[link(name = "bcryptprimitives", kind = "raw-dylib")]
unsafe extern "system" {
    fn ProcessPrng(data: *mut u8, length: usize) -> i32;
}

pub type HResult = i32;
pub type Guid = [u8; 16];

pub const S_OK: HResult = 0;
pub const S_FALSE: HResult = 1;
pub const E_NOTIMPL: HResult = 0x8000_4001u32 as i32;
pub const E_NOINTERFACE: HResult = 0x8000_4002u32 as i32;
pub const E_POINTER: HResult = 0x8000_4003u32 as i32;
pub const E_OUTOFMEMORY: HResult = 0x8007_000eu32 as i32;
pub const E_INVALIDARG: HResult = 0x8007_0057u32 as i32;
const CO_E_NOTINITIALIZED: HResult = 0x8004_01f0u32 as i32;
const CO_E_CLASSSTRING: HResult = 0x8004_01f3u32 as i32;
const REGDB_E_CLASSNOTREG: HResult = 0x8004_0154u32 as i32;
const RPC_E_CHANGED_MODE: HResult = 0x8001_0106u32 as i32;
const RPC_S_CALLPENDING: HResult = 0x8001_0115u32 as i32;

/// {xxxxxxxx-0000-0000-C000-000000000046}: the OLE interface IDs.
pub const fn ole_iid(data1: u32) -> Guid {
    let d = data1.to_le_bytes();
    [d[0], d[1], d[2], d[3], 0, 0, 0, 0, 0xc0, 0, 0, 0, 0, 0, 0, 0x46]
}

pub const IID_IUNKNOWN: Guid = ole_iid(0);
const IID_IMARSHAL: Guid = ole_iid(3);
const IID_IMALLOC: Guid = ole_iid(2);
const IID_IINITIALIZESPY: Guid = ole_iid(0x34);
const IID_ICOMTHREADINGINFO: Guid = ole_iid(0x1ce);

pub fn missing(function: &[u8], detail: u64) {
    // SAFETY: a valid buffer.
    unsafe { GalaRecordMissing(function.as_ptr(), function.len(), 2, detail) };
}

// ---- Memory ----

pub fn alloc(size: usize) -> *mut u8 {
    // SAFETY: plain call.
    unsafe { HeapAlloc(GetProcessHeap(), 0, size) }
}

pub fn alloc_zeroed(size: usize) -> *mut u8 {
    // SAFETY: plain call; HEAP_ZERO_MEMORY.
    unsafe { HeapAlloc(GetProcessHeap(), 0x8, size) }
}

pub fn free(p: *mut u8) {
    if !p.is_null() {
        // SAFETY: memory from `alloc`.
        unsafe { HeapFree(GetProcessHeap(), 0, p) };
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn CoTaskMemAlloc(size: usize) -> *mut u8 {
    alloc(size.max(1))
}

#[unsafe(no_mangle)]
pub extern "system" fn CoTaskMemRealloc(p: *mut u8, size: usize) -> *mut u8 {
    if p.is_null() {
        return alloc(size.max(1));
    }
    if size == 0 {
        free(p);
        return core::ptr::null_mut();
    }
    // SAFETY: memory from CoTaskMemAlloc.
    unsafe { HeapReAlloc(GetProcessHeap(), 0, p, size) }
}

#[unsafe(no_mangle)]
pub extern "system" fn CoTaskMemFree(p: *mut u8) {
    free(p);
}

// ---- The task allocator (IMalloc over CoTaskMem*) ----

/// The one IMalloc object: a pointer to its vtable (static; its reference
/// count does not matter).
#[repr(C)]
pub struct Malloc {
    vtable: *const VTable<9>,
}

// SAFETY: a pointer to a static vtable.
unsafe impl Sync for Malloc {}

static MALLOC_VTABLE: VTable<9> = VTable([
    malloc_query as *const (),
    malloc_add_ref as *const (),
    malloc_release as *const (),
    malloc_alloc as *const (),
    malloc_realloc as *const (),
    malloc_free as *const (),
    malloc_get_size as *const (),
    malloc_did_alloc as *const (),
    malloc_heap_minimize as *const (),
]);

static MALLOC: Malloc = Malloc { vtable: &MALLOC_VTABLE };

extern "system" fn malloc_query(this: usize, iid: *const Guid, out: *mut *mut c_void) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    match guid_at(iid) {
        IID_IUNKNOWN | IID_IMALLOC => {
            put(out, this as *mut c_void);
            S_OK
        }
        _ => {
            put(out, core::ptr::null_mut());
            E_NOINTERFACE
        }
    }
}
extern "system" fn malloc_add_ref(_this: usize) -> u32 {
    1
}
extern "system" fn malloc_release(_this: usize) -> u32 {
    1
}
extern "system" fn malloc_alloc(_this: usize, size: usize) -> *mut u8 {
    CoTaskMemAlloc(size)
}
extern "system" fn malloc_realloc(_this: usize, p: *mut u8, size: usize) -> *mut u8 {
    CoTaskMemRealloc(p, size)
}
extern "system" fn malloc_free(_this: usize, p: *mut u8) {
    CoTaskMemFree(p)
}
extern "system" fn malloc_get_size(_this: usize, p: *const u8) -> usize {
    if p.is_null() {
        return usize::MAX;
    }
    // SAFETY: a block from CoTaskMemAlloc (the caller's claim).
    unsafe { HeapSize(GetProcessHeap(), 0, p) }
}
/// DidAlloc: 1 for a block of the process heap (the task allocator's), 0
/// otherwise, -1 for NULL.
extern "system" fn malloc_did_alloc(_this: usize, p: *const u8) -> i32 {
    if p.is_null() {
        return -1;
    }
    // SAFETY: as above.
    i32::from(unsafe { HeapSize(GetProcessHeap(), 0, p) } != usize::MAX)
}
extern "system" fn malloc_heap_minimize(_this: usize) {}

/// CoGetMalloc(MEMCTX_TASK): the task allocator.
#[unsafe(no_mangle)]
pub extern "system" fn CoGetMalloc(context: u32, malloc: *mut *const Malloc) -> HResult {
    if malloc.is_null() {
        return E_POINTER;
    }
    if context != 1 {
        put(malloc, core::ptr::null());
        return E_INVALIDARG;
    }
    put(malloc, &raw const MALLOC);
    S_OK
}

// ---- GUID strings ----

/// The bytes of a GUID in string order (Data1..Data3 little-endian).
const GUID_ORDER: [usize; 16] = [3, 2, 1, 0, 5, 4, 7, 6, 8, 9, 10, 11, 12, 13, 14, 15];

/// "{XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX}" (38 characters, upper case).
fn guid_text(g: &Guid) -> [u16; 38] {
    let mut out = [u16::from(b'-'); 38];
    out[0] = u16::from(b'{');
    out[37] = u16::from(b'}');
    let mut at = 1;
    for (i, &b) in GUID_ORDER.iter().enumerate() {
        if matches!(i, 4 | 6 | 8 | 10) {
            at += 1;
        }
        let digit = |d: u8| u16::from(if d < 10 { b'0' + d } else { b'A' + d - 10 });
        out[at] = digit(g[b] >> 4);
        out[at + 1] = digit(g[b] & 0xf);
        at += 2;
    }
    out
}

/// Parses the braced form (either case).
fn parse_guid(text: *const u16) -> Option<Guid> {
    // SAFETY: the caller's NUL-terminated string, read up to 39 units.
    let units: [u16; 39] = unsafe {
        let mut u = [0u16; 39];
        for (i, slot) in u.iter_mut().enumerate() {
            *slot = *text.add(i);
            if *slot == 0 {
                break;
            }
        }
        u
    };
    if units[0] != u16::from(b'{') || units[37] != u16::from(b'}') || units[38] != 0 {
        return None;
    }
    let mut g = [0u8; 16];
    let mut at = 1;
    for (i, &b) in GUID_ORDER.iter().enumerate() {
        if matches!(i, 4 | 6 | 8 | 10) {
            if units[at] != u16::from(b'-') {
                return None;
            }
            at += 1;
        }
        let digit = |c: u16| char::from_u32(c.into())?.to_digit(16);
        g[b] = (digit(units[at])? << 4 | digit(units[at + 1])?) as u8;
        at += 2;
    }
    Some(g)
}

/// CLSIDFromString: the braced form (a ProgID is not looked up: no
/// registered classes); NULL is CLSID_NULL; CO_E_CLASSSTRING otherwise.
#[unsafe(no_mangle)]
pub extern "system" fn CLSIDFromString(text: *const u16, clsid: *mut Guid) -> HResult {
    if clsid.is_null() {
        return E_INVALIDARG;
    }
    if text.is_null() {
        put(clsid, [0; 16]);
        return S_OK;
    }
    match parse_guid(text) {
        Some(g) => {
            put(clsid, g);
            S_OK
        }
        None => CO_E_CLASSSTRING,
    }
}

/// IIDFromString: as CLSIDFromString, E_INVALIDARG when malformed.
#[unsafe(no_mangle)]
pub extern "system" fn IIDFromString(text: *const u16, iid: *mut Guid) -> HResult {
    match CLSIDFromString(text, iid) {
        CO_E_CLASSSTRING => E_INVALIDARG,
        r => r,
    }
}

/// StringFromCLSID: the braced form in CoTaskMem memory.
#[unsafe(no_mangle)]
pub extern "system" fn StringFromCLSID(clsid: *const Guid, text: *mut *mut u16) -> HResult {
    if clsid.is_null() || text.is_null() {
        return E_INVALIDARG;
    }
    let t = guid_text(&guid_at(clsid));
    let p = CoTaskMemAlloc(2 * 39).cast::<u16>();
    if p.is_null() {
        return E_OUTOFMEMORY;
    }
    // SAFETY: 39 units just allocated; the caller's output.
    unsafe {
        core::ptr::copy_nonoverlapping(t.as_ptr(), p, 38);
        p.add(38).write(0);
    }
    put(text, p);
    S_OK
}

/// StringFromIID: as StringFromCLSID.
#[unsafe(no_mangle)]
pub extern "system" fn StringFromIID(iid: *const Guid, text: *mut *mut u16) -> HResult {
    StringFromCLSID(iid, text)
}

// ---- COM object helpers ----

/// The vtable entry `i` of COM object `obj`, called as `fn(obj, args)`.
macro_rules! com_call {
    ($obj:expr, $i:expr, fn($($t:ty),*) -> $r:ty $(, $a:expr)*) => {{
        let obj = $obj as *mut *const usize;
        // SAFETY: a COM object whose vtable has entry $i of this type.
        let f: extern "system" fn(*mut *const usize $(, $t)*) -> $r =
            unsafe { core::mem::transmute(*(*obj).add($i)) };
        f(obj $(, $a)*)
    }};
}
#[allow(unused_imports)]
pub(crate) use com_call;

pub fn add_ref(obj: *mut c_void) -> u32 {
    com_call!(obj, 1, fn() -> u32)
}

pub fn release(obj: *mut c_void) -> u32 {
    com_call!(obj, 2, fn() -> u32)
}

fn guid_at(p: *const Guid) -> Guid {
    // SAFETY: the caller's GUID.
    unsafe { *p }
}

/// Stores `value` at the caller's output pointer.
pub fn put<T>(out: *mut T, value: T) {
    // SAFETY: the caller's output pointer (checked non-null by callers).
    unsafe { out.write(value) };
}

/// A vtable (function addresses).
pub struct VTable<const N: usize>(pub [*const (); N]);
// SAFETY: function addresses only.
unsafe impl<const N: usize> Sync for VTable<N> {}

// ---- Per-thread state ----

const MAX_SPIES: usize = 8;

struct ThreadState {
    /// 0: none, 1: MTA, 2: STA.
    kind: u32,
    refs: u32,
    spies: [usize; MAX_SPIES],
    cookies: [u64; MAX_SPIES],
    next_cookie: u64,
    /// The thread's error object (SetErrorInfo).
    error_info: usize,
    /// This STA thread's context object.
    sta_context: Context,
}

const TEB_RESERVED_FOR_OLE: usize = 0x1758;

fn teb() -> usize {
    let teb: usize;
    // SAFETY: gs:[0x30] is the TEB's self pointer.
    unsafe { core::arch::asm!("mov {}, gs:[0x30]", out(reg) teb, options(nostack, readonly)) };
    teb
}

/// This thread's state (made on first use; None without memory).
fn thread_state() -> Option<&'static mut ThreadState> {
    let slot = (teb() + TEB_RESERVED_FOR_OLE) as *mut *mut ThreadState;
    // SAFETY: this thread's TEB; the state is only used by this thread.
    unsafe {
        if (*slot).is_null() {
            let p = alloc_zeroed(core::mem::size_of::<ThreadState>()).cast::<ThreadState>();
            if p.is_null() {
                return None;
            }
            p.write(ThreadState {
                kind: 0,
                refs: 0,
                spies: [0; MAX_SPIES],
                cookies: [0; MAX_SPIES],
                next_cookie: 1,
                error_info: 0,
                sta_context: Context {
                    vtable: CONTEXT_UNKNOWN.0.as_ptr().cast(),
                    threading: CONTEXT_THREADING.0.as_ptr().cast(),
                },
            });
            *slot = p;
        }
        Some(&mut **slot)
    }
}

/// Threads in the MTA (any thread of the process may then use COM as an
/// implicit MTA member; not modelled yet).
static MTA_THREADS: AtomicU32 = AtomicU32::new(0);

// ---- Apartments ----

/// Set by the first CoInitializeSecurity.
static SECURITY_SET: AtomicU32 = AtomicU32::new(0);
const RPC_E_TOO_LATE: HResult = 0x8001_0119_u32 as HResult;

/// CoInitializeSecurity: the process's COM security settings, once (later
/// calls: RPC_E_TOO_LATE). They govern calls between processes and
/// machines, which gala's COM does not make, so they are accepted and
/// nothing more.
#[allow(clippy::too_many_arguments)]
#[unsafe(no_mangle)]
pub extern "system" fn CoInitializeSecurity(
    _descriptor: *mut c_void,
    _auth_count: i32,
    _auth_services: *mut c_void,
    _reserved1: *mut c_void,
    _authn_level: u32,
    _imp_level: u32,
    _auth_list: *mut c_void,
    _capabilities: u32,
    _reserved3: *mut c_void,
) -> HResult {
    if SECURITY_SET.swap(1, Ordering::AcqRel) != 0 {
        return RPC_E_TOO_LATE;
    }
    S_OK
}

#[unsafe(no_mangle)]
pub extern "system" fn CoInitializeEx(_reserved: *mut c_void, flags: u32) -> HResult {
    let Some(state) = thread_state() else { return E_OUTOFMEMORY };
    let kind = if flags & 2 != 0 { 2 } else { 1 }; // COINIT_APARTMENTTHREADED
    let spies = state.spies;
    for &spy in spies.iter().filter(|&&s| s != 0) {
        com_call!(spy, 3, fn(u32, u32) -> HResult, flags, state.refs);
    }
    let mut hr = if state.refs == 0 {
        state.kind = kind;
        state.refs = 1;
        if kind == 1 {
            MTA_THREADS.fetch_add(1, Ordering::Relaxed);
        }
        S_OK
    } else if state.kind != kind {
        RPC_E_CHANGED_MODE
    } else {
        state.refs += 1;
        S_FALSE
    };
    for &spy in spies.iter().filter(|&&s| s != 0) {
        hr = com_call!(spy, 4, fn(HResult, u32, u32) -> HResult, hr, flags, state.refs);
    }
    hr
}

#[unsafe(no_mangle)]
pub extern "system" fn CoInitialize(reserved: *mut c_void) -> HResult {
    CoInitializeEx(reserved, 2)
}

/// OleInitialize(reserved): COM in a single-threaded apartment
/// (CoInitializeEx's result: S_OK, then S_FALSE). OLE's own parts
/// (clipboard, drag and drop) need no setup here yet.
#[unsafe(no_mangle)]
pub extern "system" fn OleInitialize(reserved: *mut c_void) -> HResult {
    CoInitializeEx(reserved, 2)
}

#[unsafe(no_mangle)]
pub extern "system" fn OleUninitialize() {
    CoUninitialize();
}

#[unsafe(no_mangle)]
pub extern "system" fn CoUninitialize() {
    let Some(state) = thread_state() else { return };
    let spies = state.spies;
    for &spy in spies.iter().filter(|&&s| s != 0) {
        com_call!(spy, 5, fn(u32) -> HResult, state.refs);
    }
    if state.refs > 0 {
        state.refs -= 1;
        if state.refs == 0 {
            if state.kind == 1 {
                MTA_THREADS.fetch_sub(1, Ordering::Relaxed);
            }
            state.kind = 0;
        }
    }
    for &spy in spies.iter().filter(|&&s| s != 0) {
        com_call!(spy, 6, fn(u32) -> HResult, state.refs);
    }
}

/// RoInitialize: RO_INIT_SINGLETHREADED (0) is an STA, MULTITHREADED (1)
/// the MTA (api-ms-win-core-winrt-* resolve to this DLL).
#[unsafe(no_mangle)]
pub extern "system" fn RoInitialize(kind: u32) -> HResult {
    if kind > 1 {
        return E_INVALIDARG;
    }
    CoInitializeEx(core::ptr::null_mut(), if kind == 0 { 2 } else { 0 })
}

#[unsafe(no_mangle)]
pub extern "system" fn RoUninitialize() {
    CoUninitialize()
}

#[unsafe(no_mangle)]
pub extern "system" fn CoRegisterInitializeSpy(spy: *mut c_void, cookie: *mut u64) -> HResult {
    if spy.is_null() || cookie.is_null() {
        return E_INVALIDARG;
    }
    let Some(state) = thread_state() else { return E_OUTOFMEMORY };
    let mut iface: *mut c_void = core::ptr::null_mut();
    let hr = com_call!(
        spy,
        0,
        fn(*const Guid, *mut *mut c_void) -> HResult,
        &IID_IINITIALIZESPY,
        &mut iface
    );
    if hr < 0 {
        return hr;
    }
    let Some(i) = state.spies.iter().position(|&s| s == 0) else {
        release(iface);
        return E_OUTOFMEMORY;
    };
    state.spies[i] = iface as usize;
    state.cookies[i] = state.next_cookie;
    state.next_cookie += 1;
    put(cookie, state.cookies[i]);
    S_OK
}

#[unsafe(no_mangle)]
pub extern "system" fn CoRevokeInitializeSpy(cookie: u64) -> HResult {
    let Some(state) = thread_state() else { return E_INVALIDARG };
    match state.cookies.iter().position(|&c| c == cookie && c != 0) {
        Some(i) => {
            let spy = core::mem::replace(&mut state.spies[i], 0);
            state.cookies[i] = 0;
            release(spy as *mut c_void);
            S_OK
        }
        None => E_INVALIDARG,
    }
}

/// The current thread's apartment kind (1 MTA, 2 STA) if initialized.
fn apartment() -> Option<u32> {
    thread_state().filter(|s| s.refs > 0).map(|s| s.kind)
}

/// APTTYPE: STA 0, MTA 1.
#[unsafe(no_mangle)]
pub extern "system" fn CoGetApartmentType(kind: *mut i32, qualifier: *mut i32) -> HResult {
    if kind.is_null() || qualifier.is_null() {
        return E_INVALIDARG;
    }
    match apartment() {
        Some(k) => {
            put(kind, if k == 2 { 0 } else { 1 });
            put(qualifier, 0);
            S_OK
        }
        None => {
            put(kind, -1);
            put(qualifier, 0);
            CO_E_NOTINITIALIZED
        }
    }
}

// ---- Contexts ----

/// An object context: IUnknown and IComThreadingInfo (two vtables, the
/// object is never freed: the MTA's is static, an STA's lives with the
/// thread state).
#[repr(C)]
struct Context {
    vtable: *const usize,
    threading: *const usize,
}

struct SharedContext(Context);
// SAFETY: immutable after construction.
unsafe impl Sync for SharedContext {}

static MTA_CONTEXT: SharedContext = SharedContext(Context {
    vtable: &CONTEXT_UNKNOWN.0 as *const _ as *const usize,
    threading: &CONTEXT_THREADING.0 as *const _ as *const usize,
});

/// The context object of the current thread's apartment.
fn current_context() -> Option<*mut Context> {
    let state = thread_state()?;
    match (state.refs > 0).then_some(state.kind)? {
        2 => Some(&mut state.sta_context),
        _ => Some(&MTA_CONTEXT.0 as *const Context as *mut Context),
    }
}

/// The context's IUnknown from either of its interfaces.
fn context_of(this: usize, threading: bool) -> usize {
    if threading { this - 8 } else { this }
}

fn context_query(this: usize, iid: *const Guid, out: *mut *mut c_void) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    match guid_at(iid) {
        IID_IUNKNOWN => put(out, this as *mut c_void),
        IID_ICOMTHREADINGINFO => put(out, (this + 8) as *mut c_void),
        _ => {
            put(out, core::ptr::null_mut());
            return E_NOINTERFACE;
        }
    }
    S_OK
}

extern "system" fn ctx_query(this: usize, iid: *const Guid, out: *mut *mut c_void) -> HResult {
    context_query(context_of(this, false), iid, out)
}
extern "system" fn ctx_threading_query(
    this: usize,
    iid: *const Guid,
    out: *mut *mut c_void,
) -> HResult {
    context_query(context_of(this, true), iid, out)
}
/// Contexts live as long as their apartment: references are not counted.
extern "system" fn ctx_add_ref(_this: usize) -> u32 {
    2
}
extern "system" fn ctx_release(_this: usize) -> u32 {
    1
}
/// IComThreadingInfo::GetCurrentApartmentType.
extern "system" fn ctx_apartment_type(_this: usize, out: *mut i32) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    put(out, if apartment() == Some(2) { 0 } else { 1 });
    S_OK
}
/// GetCurrentThreadType: THDTYPE_BLOCKMESSAGES (0) for MTA threads,
/// THDTYPE_PROCESSMESSAGES (1) for STA threads.
extern "system" fn ctx_thread_type(_this: usize, out: *mut i32) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    put(out, if apartment() == Some(2) { 1 } else { 0 });
    S_OK
}
extern "system" fn ctx_not_impl(_this: usize, _guid: *mut Guid) -> HResult {
    missing(b"ole32.dll!IComThreadingInfo(logical thread id)", 0);
    E_NOTIMPL
}

static CONTEXT_UNKNOWN: VTable<3> =
    VTable([ctx_query as *const (), ctx_add_ref as *const (), ctx_release as *const ()]);
static CONTEXT_THREADING: VTable<7> = VTable([
    ctx_threading_query as *const (),
    ctx_add_ref as *const (),
    ctx_release as *const (),
    ctx_apartment_type as *const (),
    ctx_thread_type as *const (),
    ctx_not_impl as *const (),
    ctx_not_impl as *const (),
]);

/// CoGetContextToken: the address of the apartment's context object.
#[unsafe(no_mangle)]
pub extern "system" fn CoGetContextToken(token: *mut usize) -> HResult {
    if token.is_null() {
        return E_POINTER;
    }
    match current_context() {
        Some(c) => {
            put(token, c as usize);
            S_OK
        }
        None => CO_E_NOTINITIALIZED,
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn CoGetObjectContext(iid: *const Guid, out: *mut *mut c_void) -> HResult {
    match current_context() {
        Some(c) => context_query(c as usize, iid, out),
        None => {
            if !out.is_null() {
                put(out, core::ptr::null_mut());
            }
            CO_E_NOTINITIALIZED
        }
    }
}

// ---- GUIDs and classes ----

#[unsafe(no_mangle)]
pub extern "system" fn CoCreateGuid(guid: *mut Guid) -> HResult {
    if guid.is_null() {
        return E_INVALIDARG;
    }
    let mut g = [0u8; 16];
    // SAFETY: our buffer.
    unsafe { ProcessPrng(g.as_mut_ptr(), 16) };
    g[7] = (g[7] & 0x0f) | 0x40; // version 4 (Data3 is little-endian)
    g[8] = (g[8] & 0x3f) | 0x80; // the RFC 4122 variant
    put(guid, g);
    S_OK
}

/// StringFromGUID2: "{xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx}" (39 with
/// the NUL), or 0 if the buffer is too small.
#[unsafe(no_mangle)]
pub extern "system" fn StringFromGUID2(guid: *const Guid, out: *mut u16, max: i32) -> i32 {
    if max < 39 || out.is_null() {
        return 0;
    }
    let g = guid_at(guid);
    let order = [3, 2, 1, 0, 5, 4, 7, 6, 8, 9, 10, 11, 12, 13, 14, 15];
    let mut s = [0u16; 39];
    s[0] = u16::from(b'{');
    let mut n = 1;
    for (i, &b) in order.iter().enumerate() {
        if [4, 6, 8, 10].contains(&i) {
            s[n] = u16::from(b'-');
            n += 1;
        }
        for nibble in [g[b] >> 4, g[b] & 15] {
            s[n] = u16::from(b"0123456789ABCDEF"[usize::from(nibble)]);
            n += 1;
        }
    }
    s[37] = u16::from(b'}');
    // SAFETY: the caller's buffer of at least 39 characters.
    unsafe { core::ptr::copy_nonoverlapping(s.as_ptr(), out, 39) };
    39
}

/// CoGetClassObject / CoCreateInstance: no classes are registered.
#[unsafe(no_mangle)]
pub extern "system" fn CoGetClassObject(
    _clsid: *const Guid,
    context: u32,
    _info: *mut c_void,
    _iid: *const Guid,
    out: *mut *mut c_void,
) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    put(out, core::ptr::null_mut());
    if apartment().is_none() {
        return CO_E_NOTINITIALIZED;
    }
    missing(b"ole32.dll!CoGetClassObject(no registry)", context.into());
    REGDB_E_CLASSNOTREG
}

#[unsafe(no_mangle)]
pub extern "system" fn CoCreateInstance(
    _clsid: *const Guid,
    _outer: *mut c_void,
    context: u32,
    _iid: *const Guid,
    out: *mut *mut c_void,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    if apartment().is_none() {
        return CO_E_NOTINITIALIZED;
    }
    missing(b"ole32.dll!CoCreateInstance(no registry)", context.into());
    REGDB_E_CLASSNOTREG
}

/// CLSIDFromProgID: no ProgIDs are registered.
#[unsafe(no_mangle)]
pub extern "system" fn CLSIDFromProgID(progid: *const u16, clsid: *mut Guid) -> HResult {
    if progid.is_null() || clsid.is_null() {
        return E_INVALIDARG;
    }
    put(clsid, [0; 16]);
    missing(b"ole32.dll!CLSIDFromProgID(no registry)", 0);
    CO_E_CLASSSTRING
}

// ---- The free-threaded marshaler ----

/// The FTM: a non-delegating IUnknown (what the creator holds) and an
/// IMarshal that delegates IUnknown to the outer object when aggregated.
#[repr(C)]
struct Ftm {
    unknown: *const usize,
    marshal: *const usize,
    refs: AtomicU32,
    outer: usize,
}

extern "system" fn ftm_query(this: *mut Ftm, iid: *const Guid, out: *mut *mut c_void) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: our object.
    let this_ref = unsafe { &*this };
    let p = match guid_at(iid) {
        IID_IUNKNOWN => this as usize,
        IID_IMARSHAL => this as usize + 8,
        _ => {
            put(out, core::ptr::null_mut());
            return E_NOINTERFACE;
        }
    };
    this_ref.refs.fetch_add(1, Ordering::Relaxed);
    put(out, p as *mut c_void);
    S_OK
}
extern "system" fn ftm_add_ref(this: *mut Ftm) -> u32 {
    // SAFETY: our object.
    unsafe { (*this).refs.fetch_add(1, Ordering::Relaxed) + 1 }
}
extern "system" fn ftm_release(this: *mut Ftm) -> u32 {
    // SAFETY: our object, freed with its last reference.
    let left = unsafe { (*this).refs.fetch_sub(1, Ordering::AcqRel) - 1 };
    if left == 0 {
        free(this.cast());
    }
    left
}
/// IMarshal's IUnknown: the outer object's when aggregated.
extern "system" fn ftm_m_query(this: usize, iid: *const Guid, out: *mut *mut c_void) -> HResult {
    // SAFETY: `this` is the IMarshal pointer, 8 bytes into the object.
    let ftm = (this - 8) as *mut Ftm;
    match unsafe { (*ftm).outer } {
        0 => ftm_query(ftm, iid, out),
        outer => com_call!(outer, 0, fn(*const Guid, *mut *mut c_void) -> HResult, iid, out),
    }
}
extern "system" fn ftm_m_add_ref(this: usize) -> u32 {
    let ftm = (this - 8) as *mut Ftm;
    // SAFETY: as above.
    match unsafe { (*ftm).outer } {
        0 => ftm_add_ref(ftm),
        outer => add_ref(outer as *mut c_void),
    }
}
extern "system" fn ftm_m_release(this: usize) -> u32 {
    let ftm = (this - 8) as *mut Ftm;
    // SAFETY: as above.
    match unsafe { (*ftm).outer } {
        0 => ftm_release(ftm),
        outer => release(outer as *mut c_void),
    }
}
extern "system" fn ftm_m_not_impl(_this: usize) -> HResult {
    missing(b"ole32.dll!IMarshal(free-threaded marshaling)", 0);
    E_NOTIMPL
}

static FTM_UNKNOWN: VTable<3> =
    VTable([ftm_query as *const (), ftm_add_ref as *const (), ftm_release as *const ()]);
static FTM_MARSHAL: VTable<9> = VTable([
    ftm_m_query as *const (),
    ftm_m_add_ref as *const (),
    ftm_m_release as *const (),
    ftm_m_not_impl as *const (), // GetUnmarshalClass
    ftm_m_not_impl as *const (), // GetMarshalSizeMax
    ftm_m_not_impl as *const (), // MarshalInterface
    ftm_m_not_impl as *const (), // UnmarshalInterface
    ftm_m_not_impl as *const (), // ReleaseMarshalData
    ftm_m_not_impl as *const (), // DisconnectObject
]);

#[unsafe(no_mangle)]
pub extern "system" fn CoCreateFreeThreadedMarshaler(
    outer: *mut c_void,
    out: *mut *mut c_void,
) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    let p = alloc(core::mem::size_of::<Ftm>()).cast::<Ftm>();
    if p.is_null() {
        put(out, core::ptr::null_mut());
        return E_OUTOFMEMORY;
    }
    // SAFETY: fresh memory of the object's size.
    unsafe {
        p.write(Ftm {
            unknown: FTM_UNKNOWN.0.as_ptr().cast(),
            marshal: FTM_MARSHAL.0.as_ptr().cast(),
            refs: AtomicU32::new(1),
            outer: outer as usize,
        })
    };
    put(out, p.cast());
    S_OK
}

/// Marshaling across apartments is not there yet (recorded).
fn no_marshaling(function: &[u8]) -> HResult {
    missing(function, 0);
    E_NOTIMPL
}

#[unsafe(no_mangle)]
pub extern "system" fn CoGetMarshalSizeMax(
    _size: *mut u32,
    _iid: *const Guid,
    _unknown: *mut c_void,
    _context: u32,
    _reserved: *mut c_void,
    _flags: u32,
) -> HResult {
    no_marshaling(b"ole32.dll!CoGetMarshalSizeMax")
}

#[unsafe(no_mangle)]
pub extern "system" fn CoMarshalInterface(
    _stream: *mut c_void,
    _iid: *const Guid,
    _unknown: *mut c_void,
    _context: u32,
    _reserved: *mut c_void,
    _flags: u32,
) -> HResult {
    no_marshaling(b"ole32.dll!CoMarshalInterface")
}

#[unsafe(no_mangle)]
pub extern "system" fn CoUnmarshalInterface(
    _stream: *mut c_void,
    _iid: *const Guid,
    out: *mut *mut c_void,
) -> HResult {
    if !out.is_null() {
        put(out, core::ptr::null_mut());
    }
    no_marshaling(b"ole32.dll!CoUnmarshalInterface")
}

#[unsafe(no_mangle)]
pub extern "system" fn CoReleaseMarshalData(_stream: *mut c_void) -> HResult {
    no_marshaling(b"ole32.dll!CoReleaseMarshalData")
}

// ---- Waits ----

/// CoWaitForMultipleHandles: COWAIT_WAITALL (1), COWAIT_ALERTABLE (2);
/// RPC_S_CALLPENDING on timeout. No window messages to pump on gala.
#[unsafe(no_mangle)]
pub extern "system" fn CoWaitForMultipleHandles(
    flags: u32,
    ms: u32,
    count: u32,
    handles: *const usize,
    index: *mut u32,
) -> HResult {
    if index.is_null() || (count > 0 && handles.is_null()) || count > 64 {
        return E_INVALIDARG;
    }
    put(index, 0);
    // SAFETY: the caller's handle array.
    let r = unsafe {
        WaitForMultipleObjectsEx(count, handles, (flags & 1) as i32, ms, ((flags & 2) >> 1) as i32)
    };
    match r {
        0x102 => RPC_S_CALLPENDING,
        u32::MAX => {
            // HRESULT_FROM_WIN32.
            // SAFETY: plain call.
            let e = unsafe { GetLastError() };
            ((e & 0xffff) | 0x8007_0000) as i32
        }
        r => {
            put(index, r);
            S_OK
        }
    }
}

// ---- Error objects ----

#[unsafe(no_mangle)]
pub extern "system" fn SetErrorInfo(reserved: u32, info: *mut c_void) -> HResult {
    if reserved != 0 {
        return E_INVALIDARG;
    }
    let Some(state) = thread_state() else { return E_OUTOFMEMORY };
    if !info.is_null() {
        add_ref(info);
    }
    let old = core::mem::replace(&mut state.error_info, info as usize);
    if old != 0 {
        release(old as *mut c_void);
    }
    S_OK
}

/// GetErrorInfo: hands over the thread's error object (S_FALSE if none).
#[unsafe(no_mangle)]
pub extern "system" fn GetErrorInfo(reserved: u32, out: *mut *mut c_void) -> HResult {
    if reserved != 0 || out.is_null() {
        return E_INVALIDARG;
    }
    let Some(state) = thread_state() else { return E_OUTOFMEMORY };
    let info = core::mem::replace(&mut state.error_info, 0);
    put(out, info as *mut c_void);
    if info == 0 { S_FALSE } else { S_OK }
}

/// CreateErrorInfo: error objects are not there yet (recorded).
#[unsafe(no_mangle)]
pub extern "system" fn CreateErrorInfo(out: *mut *mut c_void) -> HResult {
    if !out.is_null() {
        put(out, core::ptr::null_mut());
    }
    missing(b"ole32.dll!CreateErrorInfo", 0);
    E_NOTIMPL
}

#[unsafe(no_mangle)]
pub extern "system" fn mainCRTStartup(_instance: usize, _reason: u32, _reserved: usize) -> i32 {
    1
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}

/// CoLockObjectExternal(object, lock, last_unlock_releases): a strong
/// reference held while locked (AddRef on lock, Release on unlock).
#[unsafe(no_mangle)]
pub extern "system" fn CoLockObjectExternal(object: *mut c_void, lock: i32, _last: i32) -> HResult {
    if object.is_null() {
        return E_INVALIDARG;
    }
    if lock != 0 {
        add_ref(object);
    } else {
        release(object);
    }
    S_OK
}

const DRAGDROP_E_NOTREGISTERED: HResult = 0x8004_0100_u32 as HResult;
const DRAGDROP_E_ALREADYREGISTERED: HResult = 0x8004_0101_u32 as HResult;
const DRAGDROP_E_INVALIDHWND: HResult = 0x8004_0102_u32 as HResult;

/// Windows registered as drop targets: (window, IDropTarget), each
/// holding a reference. Nothing drags yet (DoDragDrop is not there), so
/// the targets are only kept.
struct DropTargets {
    lock: core::sync::atomic::AtomicBool,
    entries: core::cell::UnsafeCell<[(usize, usize); 64]>,
}

// SAFETY: `entries` is only touched under `lock`.
unsafe impl Sync for DropTargets {}

static DROP_TARGETS: DropTargets = DropTargets {
    lock: core::sync::atomic::AtomicBool::new(false),
    entries: core::cell::UnsafeCell::new([(0, 0); 64]),
};

fn with_drop_targets<R>(f: impl FnOnce(&mut [(usize, usize); 64]) -> R) -> R {
    while DROP_TARGETS
        .lock
        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        core::hint::spin_loop();
    }
    // SAFETY: under the lock.
    let r = f(unsafe { &mut *DROP_TARGETS.entries.get() });
    DROP_TARGETS.lock.store(false, Ordering::Release);
    r
}

/// user32's IsWindow, loaded on first use (ole32 does not import user32:
/// programs without windows use it too).
fn is_window(hwnd: usize) -> bool {
    static IS_WINDOW: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
    let mut f = IS_WINDOW.load(Ordering::Acquire);
    if f == 0 {
        let name: [u16; 11] =
            core::array::from_fn(|i| u16::from(*b"user32.dll\0".get(i).unwrap_or(&0)));
        // SAFETY: NUL-terminated names.
        f = unsafe {
            let m = LoadLibraryW(name.as_ptr());
            if m.is_null() { 0 } else { GetProcAddress(m, c"IsWindow".as_ptr().cast()) as usize }
        };
        IS_WINDOW.store(f, Ordering::Release);
    }
    if f == 0 {
        return false;
    }
    // SAFETY: user32's IsWindow(HWND) -> BOOL.
    let is_window: extern "system" fn(usize) -> i32 = unsafe { core::mem::transmute(f) };
    is_window(hwnd) != 0
}

/// RegisterDragDrop(hwnd, target): keeps the target (with a reference)
/// for the window; DRAGDROP_E_ALREADYREGISTERED for a second one,
/// DRAGDROP_E_INVALIDHWND for no window.
#[unsafe(no_mangle)]
pub extern "system" fn RegisterDragDrop(hwnd: usize, target: *mut c_void) -> HResult {
    if target.is_null() {
        return E_INVALIDARG;
    }
    if !is_window(hwnd) {
        return DRAGDROP_E_INVALIDHWND;
    }
    let r = with_drop_targets(|t| {
        if t.iter().any(|e| e.0 == hwnd) {
            return DRAGDROP_E_ALREADYREGISTERED;
        }
        match t.iter_mut().find(|e| e.0 == 0) {
            Some(e) => {
                *e = (hwnd, target as usize);
                S_OK
            }
            None => E_OUTOFMEMORY,
        }
    });
    if r == S_OK {
        add_ref(target);
    }
    r
}

/// RevokeDragDrop(hwnd): forgets the window's target and releases it.
#[unsafe(no_mangle)]
pub extern "system" fn RevokeDragDrop(hwnd: usize) -> HResult {
    let target = with_drop_targets(|t| {
        t.iter_mut().find(|e| e.0 == hwnd && hwnd != 0).map(|e| core::mem::take(e).1)
    });
    match target {
        Some(p) => {
            release(p as *mut c_void);
            S_OK
        }
        None => DRAGDROP_E_NOTREGISTERED,
    }
}

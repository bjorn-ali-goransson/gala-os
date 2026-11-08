//! CoCreateInstanceEx, CreateBindCtx and the call-security functions SQL
//! Server LocalDB imports, as documented (see user/ole32_test):
//!
//! - CoCreateInstanceEx creates the object (CoCreateInstance for
//!   IUnknown) and asks it for each MULTI_QI's interface: S_OK when all
//!   were there, CO_S_NOTALLINTERFACES when some, E_NOINTERFACE when
//!   none; an activation error goes into every entry. A remote server
//!   name is not supported (recorded).
//! - CreateBindCtx: a bind context holding bound objects (AddRef'd until
//!   revoked or ReleaseBoundObjects), named object parameters and
//!   BIND_OPTS (defaults: grfMode STGM_READWRITE, the rest 0). There is
//!   no running object table (GetRunningObjectTable, and
//!   EnumObjectParam, are recorded and fail).
//! - CoImpersonateClient / CoRevertToSelf: gala has no cross-process COM
//!   calls, so there is never a call to impersonate the client of:
//!   RPC_E_CALL_COMPLETE, as outside a call on Windows.

use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, Ordering};

use super::{
    E_INVALIDARG, E_NOINTERFACE, E_NOTIMPL, E_OUTOFMEMORY, E_POINTER, Guid, HResult, IID_IUNKNOWN,
    S_FALSE, S_OK, VTable, add_ref, alloc_zeroed, com_call, free, missing, ole_iid, put, release,
};

const CO_S_NOTALLINTERFACES: HResult = 0x0008_0012;
const E_FAIL: HResult = 0x8000_4005u32 as i32;
const MK_E_NOTBOUND: HResult = 0x8004_01e9u32 as i32;
const RPC_E_CALL_COMPLETE: HResult = 0x8001_0117u32 as i32;
const IID_IBINDCTX: Guid = ole_iid(0xe);

/// MULTI_QI.
#[repr(C)]
pub struct MultiQi {
    iid: *const Guid,
    itf: *mut c_void,
    hr: HResult,
}

/// COSERVERINFO (the server's name is the second field).
#[repr(C)]
pub struct ServerInfo {
    reserved1: u32,
    name: *const u16,
    auth_info: *mut c_void,
    reserved2: u32,
}

/// CoCreateInstanceEx: the object, and each MULTI_QI's interface.
#[unsafe(no_mangle)]
pub extern "system" fn CoCreateInstanceEx(
    clsid: *const Guid,
    outer: *mut c_void,
    context: u32,
    server: *const ServerInfo,
    count: u32,
    results: *mut MultiQi,
) -> HResult {
    if count == 0 || results.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's array of `count` entries.
    let results = unsafe { core::slice::from_raw_parts_mut(results, count as usize) };
    // Every pItf must be NULL on input: E_INVALIDARG with the array
    // untouched otherwise (Windows 11 25H2).
    if results.iter().any(|r| !r.itf.is_null()) {
        return E_INVALIDARG;
    }
    for r in results.iter_mut() {
        r.itf = core::ptr::null_mut();
        r.hr = E_NOINTERFACE;
        if r.iid.is_null() {
            return E_INVALIDARG;
        }
    }
    // SAFETY: the caller's COSERVERINFO, if any.
    if !server.is_null() && !unsafe { (*server).name }.is_null() {
        missing(b"ole32.dll!CoCreateInstanceEx(remote server)", 0);
    }
    let mut unknown: *mut c_void = core::ptr::null_mut();
    let hr = super::class::CoCreateInstance(clsid, outer, context, &IID_IUNKNOWN, &mut unknown);
    if hr < 0 {
        for r in results.iter_mut() {
            r.hr = hr;
        }
        return hr;
    }
    let mut found = 0;
    for r in results.iter_mut() {
        let mut p: *mut c_void = core::ptr::null_mut();
        let qhr =
            com_call!(unknown, 0, fn(*const Guid, *mut *mut c_void) -> HResult, r.iid, &mut p);
        r.hr =
            if qhr < 0 || p.is_null() { if qhr < 0 { qhr } else { E_NOINTERFACE } } else { S_OK };
        if r.hr == S_OK {
            r.itf = p;
            found += 1;
        }
    }
    release(unknown);
    match found {
        0 => E_NOINTERFACE,
        n if n == results.len() => S_OK,
        _ => CO_S_NOTALLINTERFACES,
    }
}

const BOUND: usize = 32;
const PARAMS: usize = 16;
const NAME: usize = 64;

#[repr(C)]
struct BindCtx {
    vtable: *const usize,
    refs: AtomicU32,
    bound: [*mut c_void; BOUND],
    params: [([u16; NAME], *mut c_void); PARAMS],
    /// The BIND_OPTS (up to BIND_OPTS3, 0x40 bytes) after its cbStruct.
    options: [u8; 64],
}

fn ctx<'a>(this: *mut BindCtx) -> &'a mut BindCtx {
    // SAFETY: our object, alive while the caller holds a reference (one
    // apartment's object: not used from two threads at once).
    unsafe { &mut *this }
}

extern "system" fn query(this: *mut BindCtx, iid: *const Guid, out: *mut *mut c_void) -> HResult {
    if out.is_null() || iid.is_null() {
        return E_POINTER;
    }
    // SAFETY: the caller's IID.
    let iid = unsafe { *iid };
    if iid == IID_IUNKNOWN || iid == IID_IBINDCTX {
        add_ref(this.cast());
        put(out, this.cast());
        return S_OK;
    }
    put(out, core::ptr::null_mut());
    E_NOINTERFACE
}

extern "system" fn add(this: *mut BindCtx) -> u32 {
    ctx(this).refs.fetch_add(1, Ordering::AcqRel) + 1
}

extern "system" fn rel(this: *mut BindCtx) -> u32 {
    let left = ctx(this).refs.fetch_sub(1, Ordering::AcqRel) - 1;
    if left == 0 {
        release_bound(this);
        for slot in ctx(this).params.iter_mut() {
            if !slot.1.is_null() {
                release(slot.1);
                slot.1 = core::ptr::null_mut();
            }
        }
        free(this.cast());
    }
    left
}

extern "system" fn register_bound(this: *mut BindCtx, obj: *mut c_void) -> HResult {
    if obj.is_null() {
        return E_INVALIDARG;
    }
    let Some(slot) = ctx(this).bound.iter_mut().find(|s| s.is_null()) else {
        missing(b"ole32.dll!IBindCtx::RegisterObjectBound(more than 32)", 0);
        return E_OUTOFMEMORY;
    };
    add_ref(obj);
    *slot = obj;
    S_OK
}

extern "system" fn revoke_bound(this: *mut BindCtx, obj: *mut c_void) -> HResult {
    let Some(slot) = ctx(this).bound.iter_mut().find(|s| **s == obj && !obj.is_null()) else {
        return MK_E_NOTBOUND;
    };
    *slot = core::ptr::null_mut();
    release(obj);
    S_OK
}

fn release_bound(this: *mut BindCtx) {
    for slot in ctx(this).bound.iter_mut() {
        if !slot.is_null() {
            let o = *slot;
            *slot = core::ptr::null_mut();
            release(o);
        }
    }
}

extern "system" fn release_bound_objects(this: *mut BindCtx) -> HResult {
    release_bound(this);
    S_OK
}

extern "system" fn set_options(this: *mut BindCtx, opts: *const u8) -> HResult {
    if opts.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's BIND_OPTS of cbStruct bytes.
    unsafe {
        let size = (opts as *const u32).read_unaligned() as usize;
        if size < 4 {
            return E_INVALIDARG;
        }
        let n = (size - 4).min(64);
        core::ptr::copy_nonoverlapping(opts.add(4), ctx(this).options.as_mut_ptr(), n);
    }
    S_OK
}

extern "system" fn get_options(this: *mut BindCtx, opts: *mut u8) -> HResult {
    if opts.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's BIND_OPTS of cbStruct bytes.
    unsafe {
        let size = (opts as *const u32).read_unaligned() as usize;
        if size < 4 {
            return E_INVALIDARG;
        }
        let n = (size - 4).min(64);
        core::ptr::copy_nonoverlapping(ctx(this).options.as_ptr(), opts.add(4), n);
    }
    S_OK
}

extern "system" fn running_object_table(_: *mut BindCtx, out: *mut *mut c_void) -> HResult {
    if !out.is_null() {
        put(out, core::ptr::null_mut());
    }
    missing(b"ole32.dll!IBindCtx::GetRunningObjectTable(no running object table)", 0);
    E_NOTIMPL
}

/// `name` (NUL-terminated) as a fixed key, or None if it is too long.
fn key(name: *const u16) -> Option<[u16; NAME]> {
    if name.is_null() {
        return None;
    }
    let mut k = [0u16; NAME];
    for (i, slot) in k.iter_mut().enumerate() {
        // SAFETY: the caller's NUL-terminated string, read up to its NUL.
        let c = unsafe { *name.add(i) };
        if c == 0 {
            return Some(k);
        }
        *slot = c;
    }
    None
}

extern "system" fn register_param(
    this: *mut BindCtx,
    name: *const u16,
    obj: *mut c_void,
) -> HResult {
    let Some(k) = key(name) else {
        missing(b"ole32.dll!IBindCtx::RegisterObjectParam(long key)", 0);
        return E_INVALIDARG;
    };
    if obj.is_null() {
        return E_INVALIDARG;
    }
    let c = ctx(this);
    let at = c
        .params
        .iter()
        .position(|p| !p.1.is_null() && p.0 == k)
        .or_else(|| c.params.iter().position(|p| p.1.is_null()));
    let Some(at) = at else {
        missing(b"ole32.dll!IBindCtx::RegisterObjectParam(more than 16)", 0);
        return E_OUTOFMEMORY;
    };
    add_ref(obj);
    let old = c.params[at].1;
    c.params[at] = (k, obj);
    if !old.is_null() {
        release(old);
    }
    S_OK
}

extern "system" fn get_param(
    this: *mut BindCtx,
    name: *const u16,
    out: *mut *mut c_void,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    let Some(k) = key(name) else { return E_FAIL };
    match ctx(this).params.iter().find(|p| !p.1.is_null() && p.0 == k) {
        Some(p) => {
            add_ref(p.1);
            put(out, p.1);
            S_OK
        }
        None => E_FAIL,
    }
}

extern "system" fn enum_params(_: *mut BindCtx, out: *mut *mut c_void) -> HResult {
    if !out.is_null() {
        put(out, core::ptr::null_mut());
    }
    missing(b"ole32.dll!IBindCtx::EnumObjectParam", 0);
    E_NOTIMPL
}

extern "system" fn revoke_param(this: *mut BindCtx, name: *const u16) -> HResult {
    let Some(k) = key(name) else { return S_FALSE };
    match ctx(this).params.iter_mut().find(|p| !p.1.is_null() && p.0 == k) {
        Some(p) => {
            let o = p.1;
            p.1 = core::ptr::null_mut();
            release(o);
            S_OK
        }
        None => S_FALSE,
    }
}

static VTABLE: VTable<13> = VTable([
    query as *const (),
    add as *const (),
    rel as *const (),
    register_bound as *const (),
    revoke_bound as *const (),
    release_bound_objects as *const (),
    set_options as *const (),
    get_options as *const (),
    running_object_table as *const (),
    register_param as *const (),
    get_param as *const (),
    enum_params as *const (),
    revoke_param as *const (),
]);

const STGM_READWRITE: u32 = 2;

/// CreateBindCtx: a new bind context (`reserved` must be 0).
#[unsafe(no_mangle)]
pub extern "system" fn CreateBindCtx(reserved: u32, out: *mut *mut c_void) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    put(out, core::ptr::null_mut());
    if reserved != 0 {
        return E_INVALIDARG;
    }
    let p = alloc_zeroed(core::mem::size_of::<BindCtx>()).cast::<BindCtx>();
    if p.is_null() {
        return E_OUTOFMEMORY;
    }
    let mut options = [0u8; 64];
    // BIND_OPTS after cbStruct: grfFlags, grfMode, dwTickCountDeadline.
    options[4..8].copy_from_slice(&STGM_READWRITE.to_le_bytes());
    // SAFETY: fresh zeroed memory of the object's size.
    unsafe {
        p.write(BindCtx {
            vtable: VTABLE.0.as_ptr().cast(),
            refs: AtomicU32::new(1),
            bound: [core::ptr::null_mut(); BOUND],
            params: [([0; NAME], core::ptr::null_mut()); PARAMS],
            options,
        })
    };
    put(out, p.cast());
    S_OK
}

/// CoImpersonateClient: no COM call is being served (RPC_E_CALL_COMPLETE).
#[unsafe(no_mangle)]
pub extern "system" fn CoImpersonateClient() -> HResult {
    RPC_E_CALL_COMPLETE
}

/// CoRevertToSelf: as CoImpersonateClient (RPC_E_CALL_COMPLETE).
#[unsafe(no_mangle)]
pub extern "system" fn CoRevertToSelf() -> HResult {
    RPC_E_CALL_COMPLETE
}
